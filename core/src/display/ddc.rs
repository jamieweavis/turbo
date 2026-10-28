//! One background worker per DDC-capable display: an initial brightness read, then a
//! "latest value wins" write slot, so slider drags and key repeats coalesce into a few
//! writes instead of queueing behind a slow I2C bus.

use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ddc::{Ddc, DdcCommandRaw};
use ddc_macos::Monitor;

/// MCCS VCP code for luminance (brightness).
const VCP_BRIGHTNESS: u8 = 0x10;
/// DDC/CI opcode for "Get VCP Feature".
const GET_VCP: u8 = 0x01;
const READ_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(60);
/// Pause between consecutive writes; many monitors drop commands sent back to back.
const WRITE_GAP: Duration = Duration::from_millis(40);
/// Every request goes out twice, this far apart. Some monitors (seen on a Dell U2723QE over
/// USB-C on Apple Silicon) answer a lone read with a null message and silently drop a lone
/// write, sometimes for days, but handle a request that's repeated before the reply is read.
const REPEAT_GAP: Duration = Duration::from_millis(10);
/// Waits between re-reads of a display whose DDC didn't answer, then every `RECOVERY_MAX`.
const RECOVERY_DELAYS: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];
const RECOVERY_MAX: Duration = Duration::from_secs(60);

/// `Monitor` holds a CoreFoundation `IOAVService` (or an IOKit port on Intel) as a raw
/// pointer, so it isn't `Send`. Each one is created on the scan thread and then used by
/// exactly one worker thread for the rest of its life, never concurrently, which is safe for
/// these CF/IOKit objects.
pub(super) struct SendMonitor(pub Monitor);
unsafe impl Send for SendMonitor {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Reading {
    pub current: u16,
    pub max: u16,
}

/// Brightness as 0.0–1.0 of the monitor's own range.
pub(super) fn to_unit(raw: u16, max: u16) -> f64 {
    if max == 0 {
        0.0
    } else {
        (f64::from(raw) / f64::from(max)).clamp(0.0, 1.0)
    }
}

pub(super) fn from_unit(unit: f64, max: u16) -> u16 {
    (unit.clamp(0.0, 1.0) * f64::from(max)).round() as u16
}

/// The pending write for one display. Only the newest value matters, and a value the
/// monitor already has is skipped.
#[derive(Debug, Default)]
pub(super) struct WriteSlot {
    pending: Option<u16>,
    /// What the monitor is known to hold; `None` after a failed write leaves it unknown.
    written: Option<u16>,
}

impl WriteSlot {
    pub fn put(&mut self, raw: u16) {
        self.pending = Some(raw);
    }

    /// The value to write next, if any.
    pub fn take(&mut self) -> Option<u16> {
        let raw = self.pending.take()?;
        (self.written != Some(raw)).then_some(raw)
    }

    pub fn record(&mut self, raw: u16, ok: bool) {
        self.written = ok.then_some(raw);
    }
}

pub(super) struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    slot: WriteSlot,
    shutdown: bool,
}

impl Worker {
    /// Starts the worker, which reads the current brightness first and reports it (or
    /// `None` if the display doesn't answer DDC) through `on_read`, from its own thread.
    /// After a `None` it keeps retrying with backoff and reports again once a read succeeds.
    pub fn spawn(
        display_id: u32,
        monitor: SendMonitor,
        on_read: impl FnMut(Option<Reading>) + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let thread = thread::Builder::new()
            .name(format!("jaycast-ddc-{display_id}"))
            .spawn({
                let shared = Arc::clone(&shared);
                move || run(&shared, monitor, on_read)
            })
            .expect("failed to spawn a DDC worker thread");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Queues a raw brightness value, replacing any value not yet written. Never blocks on DDC.
    pub fn write(&self, raw: u16) {
        self.shared.state.lock().slot.put(raw);
        self.shared.wake.notify_one();
    }
}

impl Drop for Worker {
    /// Waits for a DDC transaction in flight to finish (at most a few hundred ms).
    fn drop(&mut self) {
        self.shared.state.lock().shutdown = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            // The worker's own read callback can end up releasing the last reference to the
            // manager, and so to this worker; a thread can't join itself, so it just exits.
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

fn run(shared: &Shared, monitor: SendMonitor, mut on_read: impl FnMut(Option<Reading>)) {
    let mut monitor = monitor.0;
    let mut reading = read_brightness(&mut monitor);
    on_read(reading);

    // Some monitors' DDC wedges for a while (answering every request with a null message)
    // and recovers later, so keep trying in the background rather than giving up until the
    // next hotplug.
    let mut delays = RECOVERY_DELAYS
        .iter()
        .copied()
        .chain(std::iter::repeat(RECOVERY_MAX));
    let reading = loop {
        if let Some(reading) = reading {
            break reading;
        }
        let delay = delays.next().unwrap_or(RECOVERY_MAX);
        {
            let mut state = shared.state.lock();
            shared
                .wake
                .wait_while_for(&mut state, |state| !state.shutdown, delay);
            if state.shutdown {
                return;
            }
        }
        reading = read_brightness(&mut monitor);
        if reading.is_some() {
            on_read(reading);
        }
    };

    {
        let mut state = shared.state.lock();
        state.slot.record(reading.current, true);
        // Anything queued before the range was known is meaningless.
        state.slot.take();
    }

    loop {
        let raw = {
            let mut state = shared.state.lock();
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(raw) = state.slot.take() {
                    break raw;
                }
                shared.wake.wait(&mut state);
            }
        };

        // One retry: DDC over USB-C/DisplayPort fails intermittently on some monitors.
        let ok = write_brightness(&mut monitor, raw) || {
            thread::sleep(RETRY_DELAY);
            write_brightness(&mut monitor, raw)
        };
        shared.state.lock().slot.record(raw, ok);
        thread::sleep(WRITE_GAP);
    }
}

fn read_brightness(monitor: &mut Monitor) -> Option<Reading> {
    for attempt in 0..READ_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(RETRY_DELAY);
        }
        // The first copy of the request goes out without reading a reply; `get_vcp_feature`
        // sends the second and reads.
        let _ = monitor.execute_raw(&[GET_VCP, VCP_BRIGHTNESS], &mut [], Duration::ZERO);
        thread::sleep(REPEAT_GAP);
        if let Ok(value) = monitor.get_vcp_feature(VCP_BRIGHTNESS) {
            let max = value.maximum();
            if max > 0 {
                return Some(Reading {
                    current: value.value().min(max),
                    max,
                });
            }
        }
    }
    None
}

/// Sends the brightness write twice (see [`REPEAT_GAP`]).
fn write_brightness(monitor: &mut Monitor, raw: u16) -> bool {
    let _ = monitor.set_vcp_feature(VCP_BRIGHTNESS, raw);
    thread::sleep(REPEAT_GAP);
    monitor.set_vcp_feature(VCP_BRIGHTNESS, raw).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_conversion_round_trips_and_clamps() {
        assert_eq!(to_unit(50, 100), 0.5);
        assert_eq!(to_unit(150, 100), 1.0);
        assert_eq!(to_unit(10, 0), 0.0);
        assert_eq!(from_unit(0.5, 100), 50);
        assert_eq!(from_unit(1.7, 100), 100);
        assert_eq!(from_unit(-0.2, 100), 0);
        for raw in 0..=255 {
            assert_eq!(from_unit(to_unit(raw, 255), 255), raw);
        }
    }

    #[test]
    fn slot_keeps_only_the_latest_value() {
        let mut slot = WriteSlot::default();
        slot.put(10);
        slot.put(20);
        slot.put(30);
        assert_eq!(slot.take(), Some(30));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn slot_skips_the_value_the_monitor_already_has() {
        let mut slot = WriteSlot::default();
        slot.record(40, true);
        slot.put(40);
        assert_eq!(slot.take(), None);
        slot.put(41);
        assert_eq!(slot.take(), Some(41));
    }

    #[test]
    fn failed_write_makes_the_monitor_value_unknown() {
        let mut slot = WriteSlot::default();
        slot.record(40, false);
        slot.put(40);
        assert_eq!(slot.take(), Some(40), "should retry after a failure");
    }
}
