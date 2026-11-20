//! The media key tap: a session-level tap on system-defined events, on its own thread and
//! run loop, running only while media keys are on. Each press is routed when it goes down;
//! its repeats and key-up follow that decision, so macOS never sees half a press.

use parking_lot::Mutex;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGEvent, CGEventMask, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType,
};

use super::{MediaCommand, Route, Shared};
use crate::aux_keys::{
    self, KEYTYPE_FAST, KEYTYPE_NEXT, KEYTYPE_PLAY, KEYTYPE_PREVIOUS, KEYTYPE_REWIND,
    SYSTEM_DEFINED,
};
use crate::error::CoreError;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

pub(super) struct MediaKeyTap {
    shared: Arc<Shared>,
    /// `start` was called (and `stop` hasn't been since).
    started: Mutex<bool>,
    worker: Mutex<Option<Worker>>,
}

struct Worker {
    run_loop: RunLoopHandle,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// `CFRunLoopStop` is documented as safe to call from any thread, which is all the API
/// thread does with the tap thread's run loop.
struct RunLoopHandle(CFRetained<CFRunLoop>);
unsafe impl Send for RunLoopHandle {}

impl MediaKeyTap {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            started: Mutex::new(false),
            worker: Mutex::new(None),
        }
    }

    /// Fails with `PermissionDenied` without Accessibility. A no-op while running.
    pub fn start(&self) -> Result<(), CoreError> {
        let mut started = self.started.lock();
        // SAFETY: no arguments; reads the process's TCC state.
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }
        *started = true;
        let result = self.sync(true);
        if result.is_err() {
            *started = false;
        }
        result
    }

    pub fn stop(&self) {
        *self.started.lock() = false;
        self.stop_tap();
    }

    /// Call after the settings change: the tap only runs while media keys are on.
    pub fn refresh(&self) -> Result<(), CoreError> {
        let started = self.started.lock();
        self.sync(*started)
    }

    fn sync(&self, started: bool) -> Result<(), CoreError> {
        if started && self.shared.config.lock().enabled {
            self.start_tap()
        } else {
            self.stop_tap();
            Ok(())
        }
    }

    fn start_tap(&self) -> Result<(), CoreError> {
        let mut worker = self.worker.lock();
        if worker.is_some() {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shared = Arc::clone(&self.shared);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("turbo-media-keys".into())
            .spawn(move || run_tap_thread(shared, thread_stop, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(run_loop)) => {
                *worker = Some(Worker {
                    run_loop,
                    stop,
                    thread,
                });
                log::info!("media key tap started");
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(CoreError::Other {
                    message: "media key thread exited during startup".into(),
                })
            }
        }
    }

    fn stop_tap(&self) {
        let Some(worker) = self.worker.lock().take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        worker.run_loop.0.stop();
        let _ = worker.thread.join();
        log::info!("media key tap stopped");
    }
}

impl Drop for MediaKeyTap {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_tap_thread(
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, CoreError>>,
) {
    // The state lives in a Box so the callback can hold a stable pointer to it; it is only
    // ever touched from this thread, and freed after the run loop has stopped.
    let state = Box::into_raw(Box::new(TapState {
        shared,
        presses: PressTracker::default(),
        port: None,
    }));

    let mask: CGEventMask = 1 << SYSTEM_DEFINED.0;
    // SAFETY: `tap_callback` matches the callback ABI; `state` outlives the tap.
    let port = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            mask,
            Some(tap_callback),
            state.cast(),
        )
    };
    let (Some(port), Some(run_loop)) = (port, CFRunLoop::current()) else {
        // SAFETY: nothing else holds the pointer; the tap was never created.
        drop(unsafe { Box::from_raw(state) });
        let _ = ready.send(Err(CoreError::PermissionDenied));
        return;
    };
    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
        // SAFETY: as above.
        drop(unsafe { Box::from_raw(state) });
        let _ = ready.send(Err(CoreError::Other {
            message: "could not create the media key tap's run loop source".into(),
        }));
        return;
    };

    // SAFETY: the mode constants are immutable CF globals.
    let common_modes = unsafe { kCFRunLoopCommonModes };
    run_loop.add_source(Some(&source), common_modes);
    // SAFETY: still the only user of `state`; the run loop isn't running yet.
    unsafe { (*state).port = Some(port.clone()) };
    CGEvent::tap_enable(&port, true);
    let _ = ready.send(Ok(RunLoopHandle(run_loop.clone())));

    // The timeout is a backstop in case a stop request lands between checks.
    // SAFETY: as above.
    let default_mode = unsafe { kCFRunLoopDefaultMode };
    while !stop.load(Ordering::Acquire) {
        CFRunLoop::run_in_mode(default_mode, 0.5, false);
    }

    CGEvent::tap_enable(&port, false);
    source.invalidate();
    port.invalidate();
    // SAFETY: the tap and source are invalidated, so no callback can run again.
    drop(unsafe { Box::from_raw(state) });
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user_info` is the TapState pointer, used only on this thread.
    let state = unsafe { &mut *user_info.cast::<TapState>() };
    // SAFETY: CG hands us a valid event for the duration of the callback.
    let event_ref = unsafe { event.as_ref() };
    // A panic must never unwind into CoreGraphics or eat the user's key press.
    let disposition = catch_unwind(AssertUnwindSafe(|| state.handle(event_type, event_ref)))
        .unwrap_or(Disposition::PassThrough);
    match disposition {
        Disposition::PassThrough => event.as_ptr(),
        Disposition::Swallow => ptr::null_mut(),
    }
}

/// Everything the tap thread owns.
struct TapState {
    shared: Arc<Shared>,
    presses: PressTracker,
    port: Option<CFRetained<CFMachPort>>,
}

impl TapState {
    fn handle(&mut self, event_type: CGEventType, event: &CGEvent) -> Disposition {
        if event_type == CGEventType::TapDisabledByTimeout
            || event_type == CGEventType::TapDisabledByUserInput
        {
            log::warn!("media key tap disabled by the system ({event_type:?}); re-enabling");
            if let Some(port) = &self.port {
                CGEvent::tap_enable(port, true);
            }
            // We may have missed key-ups while disabled, so held presses are stale.
            self.presses.reset();
            return Disposition::PassThrough;
        }
        if event_type != SYSTEM_DEFINED {
            return Disposition::PassThrough;
        }
        let Some(key) = aux_keys::key(event) else {
            return Disposition::PassThrough;
        };
        let Some(command) = command(key.key_type) else {
            return Disposition::PassThrough;
        };
        if !key.pressed {
            return self.presses.up(command);
        }
        let shared = &self.shared;
        self.presses.down(command, key.repeat, || {
            let route = shared.route(true);
            let result = match route {
                Route::Send(player) => shared.dispatch(player, command, false),
                Route::Launch(player) => shared.dispatch(player, command, true),
                Route::Ignore => {
                    log::debug!("media key {command:?} ignored: no player running");
                    Ok(())
                }
                Route::PassThrough => return false,
            };
            if let Err(error) = result {
                log::error!("couldn't queue media key {command:?}: {error}");
            }
            true
        })
    }
}

/// The command a media key type stands for.
fn command(key_type: isize) -> Option<MediaCommand> {
    Some(match key_type {
        KEYTYPE_PLAY => MediaCommand::PlayPause,
        KEYTYPE_NEXT | KEYTYPE_FAST => MediaCommand::Next,
        KEYTYPE_PREVIOUS | KEYTYPE_REWIND => MediaCommand::Previous,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    PassThrough,
    Swallow,
}

fn slot(command: MediaCommand) -> usize {
    match command {
        MediaCommand::PlayPause => 0,
        MediaCommand::Next => 1,
        MediaCommand::Previous => 2,
    }
}

/// Per key: `Some(true)` while a press Turbo took is held, `Some(false)` while one it
/// left to macOS is held.
#[derive(Debug, Default)]
struct PressTracker {
    held: [Option<bool>; 3],
}

impl PressTracker {
    /// `take` routes a new press and says whether Turbo took it; repeats of a known press
    /// follow it without routing again (holding next doesn't skip ten tracks).
    fn down(
        &mut self,
        command: MediaCommand,
        repeat: bool,
        take: impl FnOnce() -> bool,
    ) -> Disposition {
        let held = &mut self.held[slot(command)];
        let taken = match (*held, repeat) {
            (Some(taken), true) => taken,
            // A new press, or a repeat whose key-down we missed.
            _ => {
                let taken = take();
                *held = Some(taken);
                taken
            }
        };
        if taken {
            Disposition::Swallow
        } else {
            Disposition::PassThrough
        }
    }

    fn up(&mut self, command: MediaCommand) -> Disposition {
        match self.held[slot(command)].take() {
            Some(true) => Disposition::Swallow,
            _ => Disposition::PassThrough,
        }
    }

    fn reset(&mut self) {
        self.held = [None; 3];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_media_key_types() {
        assert_eq!(command(KEYTYPE_PLAY), Some(MediaCommand::PlayPause));
        assert_eq!(command(KEYTYPE_NEXT), Some(MediaCommand::Next));
        assert_eq!(command(KEYTYPE_FAST), Some(MediaCommand::Next));
        assert_eq!(command(KEYTYPE_PREVIOUS), Some(MediaCommand::Previous));
        assert_eq!(command(KEYTYPE_REWIND), Some(MediaCommand::Previous));
        // Brightness and volume keys aren't ours.
        assert_eq!(command(aux_keys::KEYTYPE_BRIGHTNESS_UP), None);
        assert_eq!(command(0), None);
    }

    #[test]
    fn taken_press_swallows_repeats_and_up_once() {
        let mut presses = PressTracker::default();
        let mut routed = 0;
        assert_eq!(
            presses.down(MediaCommand::Next, false, || {
                routed += 1;
                true
            }),
            Disposition::Swallow
        );
        assert_eq!(
            presses.down(MediaCommand::Next, true, || {
                routed += 1;
                true
            }),
            Disposition::Swallow
        );
        assert_eq!(routed, 1);
        assert_eq!(presses.up(MediaCommand::Next), Disposition::Swallow);
        assert_eq!(presses.up(MediaCommand::Next), Disposition::PassThrough);
    }

    #[test]
    fn passed_press_stays_with_macos() {
        let mut presses = PressTracker::default();
        assert_eq!(
            presses.down(MediaCommand::PlayPause, false, || false),
            Disposition::PassThrough
        );
        assert_eq!(
            presses.down(MediaCommand::PlayPause, true, || true),
            Disposition::PassThrough
        );
        assert_eq!(
            presses.up(MediaCommand::PlayPause),
            Disposition::PassThrough
        );
    }

    #[test]
    fn repeat_without_seen_down_routes_and_reset_forgets() {
        let mut presses = PressTracker::default();
        assert_eq!(
            presses.down(MediaCommand::Previous, true, || true),
            Disposition::Swallow
        );
        presses.reset();
        assert_eq!(presses.up(MediaCommand::Previous), Disposition::PassThrough);
    }

    #[test]
    fn event_mask_is_system_defined_only() {
        let mask: CGEventMask = 1 << SYSTEM_DEFINED.0;
        assert_eq!(mask, 1 << 14);
    }
}
