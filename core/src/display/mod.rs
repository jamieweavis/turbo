//! External display brightness over DDC/CI, and the brightness keys that drive it.
//!
//! Module ownership (so parallel work doesn't collide):
//! - `mod.rs`, `ddc`: display enumeration, hotplug, cached brightness and the DDC writers.
//! - `keys`: the brightness-key event tap, which calls [`DisplayManager::adjust`].
//! - `builtin`: the built-in display's brightness, which macOS controls itself.
//!
//! DDC is slow (tens of ms per transaction) and flaky, so nothing here blocks a caller on
//! it: a scan thread enumerates displays (at startup and, debounced, after every display
//! reconfiguration), each DDC-capable display gets a [`ddc::Worker`], and brightness is
//! served from a cache. The bottom of the range goes below the monitor's own minimum with
//! gamma dimming (see [`dimming`]), so brightness 0 is black.

pub(crate) mod builtin;
mod ddc;
mod dimming;
pub(crate) mod keys;

use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayChangeSummaryFlags, CGDisplayRegisterReconfigurationCallback,
    CGDisplayRemoveReconfigurationCallback, CGError, CGGetActiveDisplayList,
};

use crate::config::MonitorConfig;
use crate::error::CoreError;
use ddc::{Reading, SendMonitor, Worker};
use dimming::Dimmer;

pub(crate) use keys::BrightnessKeyTap;

/// Displays tend to come and go in bursts (wake, dock connect, mirroring), so a rescan
/// waits for this much quiet after the last reconfiguration.
const HOTPLUG_DEBOUNCE: Duration = Duration::from_secs(1);
const MAX_DISPLAYS: usize = 32;

unsafe extern "C" {
    // Declared here rather than through objc2-core-graphics, whose wrapper needs its `libc`
    // feature just to convert `boolean_t`.
    fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> u32;
}

/// An external display Jaycast knows about.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DisplayInfo {
    /// The `CGDirectDisplayID`; stable while the display stays connected.
    pub id: u32,
    pub name: String,
    /// Whether brightness can be read and written over DDC/CI.
    pub supports_ddc: bool,
    /// 0.0–1.0, normalised from the monitor's own range. Meaningless without DDC.
    pub brightness: f64,
}

/// Lets the shell react to display changes. Called from background threads.
#[uniffi::export(with_foreign)]
pub trait DisplayObserver: Send + Sync {
    /// A display was connected or disconnected, or finished its initial DDC read.
    fn displays_changed(&self, displays: Vec<DisplayInfo>);
    /// Brightness changed. `from_keys` is true when the brightness keys did it, so the shell
    /// shows the on-screen indicator.
    fn brightness_changed(&self, display_id: u32, brightness: f64, from_keys: bool);
}

pub(crate) struct DisplayManager {
    config: Mutex<MonitorConfig>,
    observer: Mutex<Option<Arc<dyn DisplayObserver>>>,
    state: Mutex<State>,
    scan: Arc<ScanSignal>,
    /// The pointer registered with CoreGraphics, freed after unregistering in `Drop`.
    callback_info: *const ScanSignal,
    dimmer: Dimmer,
}

// SAFETY: `callback_info` is only an opaque token handed back to CoreGraphics in `Drop`;
// it points at an `Arc<ScanSignal>`'s contents, which are `Send + Sync`.
unsafe impl Send for DisplayManager {}
unsafe impl Sync for DisplayManager {}

#[derive(Default)]
struct State {
    displays: Vec<Entry>,
    /// Initial DDC reads still outstanding from the latest scan; `displays_changed` waits
    /// for them so the shell gets one update with real brightness values.
    pending_reads: usize,
}

struct Entry {
    info: DisplayInfo,
    /// The monitor's raw maximum, known once the initial read succeeds.
    max: u16,
    /// The worker has reported its first read (successful or not).
    reported: bool,
    worker: Option<Worker>,
}

impl DisplayManager {
    pub fn new(config: MonitorConfig) -> Arc<Self> {
        Self::with_scanning(config, true)
    }

    /// `scanning: false` never touches display hardware, for tests: concurrent DDC reads from
    /// parallel tests can leave a real monitor's DDC unresponsive.
    fn with_scanning(config: MonitorConfig, scanning: bool) -> Arc<Self> {
        let scan = Arc::new(ScanSignal::default());
        let callback_info = Arc::into_raw(Arc::clone(&scan));
        // SAFETY: `on_reconfigure` matches the callback ABI, and `callback_info` stays valid
        // until it's unregistered in `Drop`. The callback is delivered on the main run loop.
        unsafe {
            CGDisplayRegisterReconfigurationCallback(
                Some(on_reconfigure),
                callback_info.cast_mut().cast(),
            );
        }

        let manager = Arc::new(Self {
            config: Mutex::new(config),
            observer: Mutex::new(None),
            state: Mutex::new(State::default()),
            scan: Arc::clone(&scan),
            callback_info,
            dimmer: Dimmer::default(),
        });

        if scanning {
            let weak = Arc::downgrade(&manager);
            thread::Builder::new()
                .name("jaycast-displays".into())
                .spawn(move || run_scanner(&scan, &weak))
                .expect("failed to spawn the display scan thread");
        }
        manager
    }

    pub fn set_config(&self, config: MonitorConfig) {
        let enabled = config.enabled;
        let was_enabled = std::mem::replace(&mut *self.config.lock().unwrap(), config).enabled;
        if enabled == was_enabled {
            return;
        }
        if enabled {
            self.restore_all();
        } else {
            self.release_all();
        }
    }

    /// Turning control off hands the displays back at full brightness. The cached
    /// brightness is kept, so the sliders stay put and turning control on restores it.
    fn release_all(&self) {
        {
            let state = self.state.lock().unwrap();
            for entry in state.displays.iter().filter(|e| e.info.supports_ddc) {
                if let Some(worker) = &entry.worker {
                    worker.write(entry.max);
                }
            }
        }
        self.dimmer.reset_all();
    }

    /// Re-applies the cached brightness after control is turned back on.
    fn restore_all(&self) {
        let values: Vec<(u32, f64)> = {
            let state = self.state.lock().unwrap();
            state
                .displays
                .iter()
                .filter(|e| e.info.supports_ddc)
                .map(|e| (e.info.id, e.info.brightness))
                .collect()
        };
        for (id, brightness) in values {
            self.apply(id, |_| brightness);
        }
    }

    pub fn config(&self) -> MonitorConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn DisplayObserver>>) {
        *self.observer.lock().unwrap() = observer;
    }

    /// The external displays, with cached brightness (never blocks on DDC).
    pub fn displays(&self) -> Vec<DisplayInfo> {
        self.snapshot()
    }

    /// Sets brightness (clamped to 0.0–1.0). Returns immediately; the DDC write happens on a
    /// background thread, coalescing rapid changes (e.g. slider drags) to the latest value.
    pub fn set_brightness(&self, display_id: u32, brightness: f64) -> Result<(), CoreError> {
        if !self.config.lock().unwrap().enabled {
            return Err(CoreError::Other {
                message: "monitor control is turned off".into(),
            });
        }
        let value = self
            .apply(display_id, |_| brightness.clamp(0.0, 1.0))
            .ok_or_else(|| CoreError::Other {
                message: format!("display {display_id} doesn't support brightness control"),
            })?;
        self.notify_brightness(display_id, value, false);
        Ok(())
    }

    /// Nudges brightness by `delta` (e.g. ±1/16) for the brightness keys. Returns the new
    /// value, or `None` if this display can't be controlled, so the key should pass through.
    /// Must not block: the key tap calls it on its hot path. Notifies the observer with
    /// `from_keys: true`.
    pub fn adjust(&self, display_id: u32, delta: f64) -> Option<f64> {
        if !self.config.lock().unwrap().enabled {
            return None;
        }
        let value = self.apply(display_id, |current| step(current, delta))?;
        self.notify_brightness(display_id, value, true);
        Some(value)
    }

    /// Updates the cached brightness, queues the DDC write and sets the gamma dimming.
    /// `None` for an unknown display or one without DDC.
    fn apply(&self, display_id: u32, f: impl FnOnce(f64) -> f64) -> Option<f64> {
        let (value, split) = {
            let mut state = self.state.lock().unwrap();
            let entry = state
                .displays
                .iter_mut()
                .find(|e| e.info.id == display_id && e.info.supports_ddc)?;
            let worker = entry.worker.as_ref()?;
            let value = f(entry.info.brightness);
            let split = dimming::split(value);
            entry.info.brightness = value;
            worker.write(ddc::from_unit(split.ddc, entry.max));
            (value, split)
        };
        self.dimmer.set(display_id, split.gamma);
        Some(value)
    }

    fn snapshot(&self) -> Vec<DisplayInfo> {
        let state = self.state.lock().unwrap();
        state.displays.iter().map(|e| e.info.clone()).collect()
    }

    fn observer(&self) -> Option<Arc<dyn DisplayObserver>> {
        self.observer.lock().unwrap().clone()
    }

    fn notify_brightness(&self, display_id: u32, brightness: f64, from_keys: bool) {
        if let Some(observer) = self.observer() {
            observer.brightness_changed(display_id, brightness, from_keys);
        }
    }

    fn notify_displays(&self) {
        if let Some(observer) = self.observer() {
            observer.displays_changed(self.snapshot());
        }
    }

    /// Re-enumerates displays, keeping the workers of displays that stayed connected.
    fn rescan(self: &Arc<Self>, first: bool) {
        let found = enumerate();
        let removed;
        let notify;
        {
            let mut state = self.state.lock().unwrap();
            let before: Vec<u32> = state.displays.iter().map(|e| e.info.id).collect();
            let found_ids: Vec<u32> = found.iter().map(|f| f.id).collect();
            let diff = diff_ids(&before, &found_ids);
            let changed = !diff.added.is_empty() || !diff.removed.is_empty();

            let (gone, kept): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut state.displays)
                .into_iter()
                .partition(|e| diff.removed.contains(&e.info.id));
            removed = gone;
            state.displays = kept;

            for display in found.into_iter().filter(|f| diff.added.contains(&f.id)) {
                let worker = display.monitor.map(|monitor| {
                    state.pending_reads += 1;
                    let weak = Arc::downgrade(self);
                    let id = display.id;
                    Worker::spawn(id, monitor, move |reading| {
                        if let Some(manager) = weak.upgrade() {
                            manager.finish_read(id, reading);
                        }
                    })
                });
                state.displays.push(Entry {
                    info: DisplayInfo {
                        id: display.id,
                        name: display.name,
                        supports_ddc: false,
                        brightness: 0.0,
                    },
                    max: 0,
                    reported: false,
                    worker,
                });
            }
            state.displays.sort_by_key(|e| e.info.id);
            // With reads outstanding, `finish_read` sends the update once they've answered.
            notify = state.pending_reads == 0 && (changed || first);
        }
        // Workers join their threads on drop, which can wait on a DDC transaction; never do
        // that while holding the state lock their callbacks need.
        for entry in &removed {
            self.dimmer.forget(entry.info.id);
        }
        drop(removed);
        if notify {
            self.notify_displays();
        }
    }

    fn finish_read(&self, display_id: u32, reading: Option<Reading>) {
        let notify = {
            let mut state = self.state.lock().unwrap();
            let mut first = false;
            if let Some(entry) = state.displays.iter_mut().find(|e| e.info.id == display_id) {
                first = !entry.reported;
                entry.reported = true;
                if let Some(reading) = reading {
                    entry.info.supports_ddc = true;
                    entry.info.brightness =
                        dimming::from_ddc(ddc::to_unit(reading.current, reading.max));
                    entry.max = reading.max;
                }
            }
            if first {
                state.pending_reads = state.pending_reads.saturating_sub(1);
                state.pending_reads == 0
            } else {
                // A display whose DDC came back later.
                reading.is_some()
            }
        };
        if notify {
            self.notify_displays();
        }
    }
}

impl Drop for DisplayManager {
    fn drop(&mut self) {
        // SAFETY: the same callback and token that were registered in `new`.
        unsafe {
            CGDisplayRemoveReconfigurationCallback(
                Some(on_reconfigure),
                self.callback_info.cast_mut().cast(),
            );
            drop(Arc::from_raw(self.callback_info));
        }
        self.scan.shut_down();
    }
}

/// One brightness-key step from `current`: snaps to the grid of `delta`-sized steps (as
/// macOS does, so 16 presses always cover the range) and moves one step along it.
fn step(current: f64, delta: f64) -> f64 {
    let size = delta.abs();
    if size < f64::EPSILON {
        return current;
    }
    let index = (current / size).round() + delta.signum();
    (index * size).clamp(0.0, 1.0)
}

#[derive(Debug, Default, PartialEq)]
struct IdDiff {
    added: Vec<u32>,
    removed: Vec<u32>,
}

fn diff_ids(before: &[u32], after: &[u32]) -> IdDiff {
    IdDiff {
        added: after
            .iter()
            .copied()
            .filter(|id| !before.contains(id))
            .collect(),
        removed: before
            .iter()
            .copied()
            .filter(|id| !after.contains(id))
            .collect(),
    }
}

struct FoundDisplay {
    id: u32,
    name: String,
    monitor: Option<SendMonitor>,
}

/// Every active external display, with a DDC handle where macOS exposes one. Displays
/// without one (AirPlay, Sidecar, DisplayLink, some docks) are listed without DDC.
fn enumerate() -> Vec<FoundDisplay> {
    let mut ids = [0 as CGDirectDisplayID; MAX_DISPLAYS];
    let mut count = 0u32;
    // SAFETY: the buffer holds MAX_DISPLAYS ids and CG writes at most that many.
    let error =
        unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success {
        return Vec::new();
    }
    let external: Vec<u32> = ids[..count as usize]
        .iter()
        .copied()
        // SAFETY: plain query on a display id.
        .filter(|&id| unsafe { CGDisplayIsBuiltin(id) } == 0)
        .collect();

    let mut monitors: Vec<ddc_macos::Monitor> = ddc_macos::Monitor::enumerate().unwrap_or_default();
    external
        .iter()
        .enumerate()
        .map(|(index, &id)| {
            let monitor = monitors
                .iter()
                .position(|m| m.handle().id == id)
                .map(|i| monitors.swap_remove(i));
            let name = monitor
                .as_ref()
                .and_then(|m| m.product_name())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("Display {}", index + 1));
            FoundDisplay {
                id,
                name,
                monitor: monitor.map(SendMonitor),
            }
        })
        .collect()
}

/// Wakes the scan thread; written to by the CoreGraphics callback on the main thread.
#[derive(Default)]
struct ScanSignal {
    state: Mutex<ScanState>,
    wake: Condvar,
}

#[derive(Default)]
struct ScanState {
    debounce: Debounce,
    shutdown: bool,
}

impl ScanSignal {
    fn poke(&self) {
        self.state.lock().unwrap().debounce.poke(Instant::now());
        self.wake.notify_one();
    }

    fn shut_down(&self) {
        self.state.lock().unwrap().shutdown = true;
        self.wake.notify_one();
    }
}

/// Fires once `HOTPLUG_DEBOUNCE` has passed since the last poke.
#[derive(Debug, Default)]
struct Debounce {
    last_poke: Option<Instant>,
}

impl Debounce {
    fn poke(&mut self, now: Instant) {
        self.last_poke = Some(now);
    }

    /// `Ok(())` when due (and resets), `Err(Some(wait))` while settling, `Err(None)` idle.
    fn check(&mut self, now: Instant) -> Result<(), Option<Duration>> {
        let last = self.last_poke.ok_or(None)?;
        let quiet = now.saturating_duration_since(last);
        if quiet >= HOTPLUG_DEBOUNCE {
            self.last_poke = None;
            Ok(())
        } else {
            Err(Some(HOTPLUG_DEBOUNCE - quiet))
        }
    }
}

fn run_scanner(signal: &ScanSignal, manager: &Weak<DisplayManager>) {
    match manager.upgrade() {
        Some(manager) => manager.rescan(true),
        None => return,
    }
    loop {
        {
            let mut state = signal.state.lock().unwrap();
            loop {
                if state.shutdown {
                    return;
                }
                match state.debounce.check(Instant::now()) {
                    Ok(()) => break,
                    Err(Some(wait)) => state = signal.wake.wait_timeout(state, wait).unwrap().0,
                    Err(None) => state = signal.wake.wait(state).unwrap(),
                }
            }
        }
        match manager.upgrade() {
            Some(manager) => manager.rescan(false),
            None => return,
        }
    }
}

unsafe extern "C-unwind" fn on_reconfigure(
    _display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    info: *mut c_void,
) {
    // Each change arrives twice: once before (BeginConfiguration) and once after.
    if flags.contains(CGDisplayChangeSummaryFlags::BeginConfigurationFlag) || info.is_null() {
        return;
    }
    // SAFETY: `info` is the `ScanSignal` registered in `DisplayManager::new`, kept alive
    // until the callback is removed.
    let signal = unsafe { &*info.cast::<ScanSignal>() };
    signal.poke();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_snaps_to_the_grid_then_moves_one_step() {
        let s = 1.0 / 16.0;
        assert!((step(0.5, s) - 0.5625).abs() < 1e-9);
        assert!((step(0.53, s) - 0.5625).abs() < 1e-9, "snaps from off-grid");
        assert!((step(0.5, -s) - 0.4375).abs() < 1e-9);
        assert_eq!(step(1.0, s), 1.0);
        assert_eq!(step(0.0, -s), 0.0);
        assert_eq!(step(0.3, 0.0), 0.3);
    }

    #[test]
    fn sixteen_steps_cover_the_range() {
        let mut value = 0.0;
        for _ in 0..16 {
            value = step(value, 1.0 / 16.0);
        }
        assert!((value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn diff_finds_added_and_removed() {
        assert_eq!(
            diff_ids(&[1, 2, 3], &[2, 3, 4]),
            IdDiff {
                added: vec![4],
                removed: vec![1],
            }
        );
        assert_eq!(diff_ids(&[1], &[1]), IdDiff::default());
    }

    #[test]
    fn debounce_waits_for_quiet() {
        let start = Instant::now();
        let mut debounce = Debounce::default();
        assert_eq!(debounce.check(start), Err(None));

        debounce.poke(start);
        assert!(matches!(
            debounce.check(start + Duration::from_millis(400)),
            Err(Some(_))
        ));
        // Another change resets the wait.
        debounce.poke(start + Duration::from_millis(800));
        assert!(debounce.check(start + Duration::from_millis(1500)).is_err());
        assert_eq!(debounce.check(start + Duration::from_millis(1800)), Ok(()));
        assert_eq!(
            debounce.check(start + Duration::from_millis(1900)),
            Err(None),
            "fires once"
        );
    }

    #[test]
    fn disabled_config_refuses_changes() {
        let manager = DisplayManager::with_scanning(
            MonitorConfig {
                enabled: false,
                brightness_keys: true,
            },
            false,
        );
        assert!(manager.set_brightness(1, 0.5).is_err());
        assert_eq!(manager.adjust(1, 0.1), None);
    }

    #[test]
    fn unknown_display_is_not_controllable() {
        let manager = DisplayManager::with_scanning(MonitorConfig::default(), false);
        assert_eq!(manager.adjust(u32::MAX, 0.1), None);
        assert!(manager.set_brightness(u32::MAX, 0.5).is_err());
    }
}
