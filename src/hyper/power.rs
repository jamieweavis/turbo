//! Sleep and wake notifications for the Hyper tap. A shortcut that sleeps the Mac (Hyper+T
//! running `pmset sleepnow`) fires on key-down, so the Hyper key's key-up happens while the
//! machine is asleep and never reaches the tap. Without this, Hyper stays held after wake
//! and every click carries ⌃⌥⇧⌘ (and starts a window drag) until the key is pressed again.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_foundation::{CFRunLoop, CFRunLoopSource, kCFRunLoopCommonModes};

type IoConnect = u32;
type IoObject = u32;
type IONotificationPortRef = *mut c_void;

/// `IOServiceInterestCallback`.
pub(super) type PowerCallback = unsafe extern "C-unwind" fn(
    refcon: *mut c_void,
    service: u32,
    message: u32,
    argument: *mut c_void,
);

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        port: *mut IONotificationPortRef,
        callback: PowerCallback,
        notifier: *mut IoObject,
    ) -> IoConnect;
    fn IODeregisterForSystemPower(notifier: *mut IoObject) -> i32;
    fn IONotificationPortGetRunLoopSource(
        port: IONotificationPortRef,
    ) -> Option<NonNull<CFRunLoopSource>>;
    fn IONotificationPortDestroy(port: IONotificationPortRef);
    fn IOAllowPowerChange(root_port: IoConnect, notification: isize) -> i32;
    fn IOServiceClose(connect: IoConnect) -> i32;
}

/// `iokit_common_msg(...)` values from IOMessage.h.
const CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
const SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
const SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PowerEvent {
    WillSleep,
    Woke,
}

/// What a power message means for the tap, if anything.
pub(super) fn classify(message: u32) -> Option<PowerEvent> {
    match message {
        SYSTEM_WILL_SLEEP => Some(PowerEvent::WillSleep),
        SYSTEM_HAS_POWERED_ON => Some(PowerEvent::Woke),
        _ => None,
    }
}

/// A registration for system power messages, delivered on the run loop it was added to.
pub(super) struct PowerNotifier {
    root_port: IoConnect,
    port: IONotificationPortRef,
    notifier: IoObject,
}

impl PowerNotifier {
    /// Registers `callback` (with `refcon`) on `run_loop`. The callback must pass every
    /// message to [`PowerNotifier::acknowledge`], or sleep is held up for 30 seconds.
    ///
    /// # Safety
    /// `refcon` must stay valid for as long as the notifier lives, and the callback may
    /// only run on `run_loop`'s thread.
    pub unsafe fn register(
        run_loop: &CFRunLoop,
        refcon: *mut c_void,
        callback: PowerCallback,
    ) -> Option<Self> {
        let mut port: IONotificationPortRef = std::ptr::null_mut();
        let mut notifier: IoObject = 0;
        // SAFETY: out-pointers are valid locals; the caller guarantees `refcon`.
        let root_port =
            unsafe { IORegisterForSystemPower(refcon, &mut port, callback, &mut notifier) };
        if root_port == 0 {
            return None;
        }
        // SAFETY: `port` came from a successful registration. The source is owned by the
        // port, so it's borrowed here, not released.
        let source = unsafe { IONotificationPortGetRunLoopSource(port) };
        let this = Self {
            root_port,
            port,
            notifier,
        };
        // SAFETY: the source lives as long as the port, which `this` owns.
        let source = unsafe { source?.as_ref() };
        // SAFETY: the mode constant is an immutable CF global.
        run_loop.add_source(Some(source), unsafe { kCFRunLoopCommonModes });
        Some(this)
    }

    /// Lets a sleep go ahead; every sleep-related message needs this answer.
    pub fn acknowledge(&self, message: u32, argument: *mut c_void) {
        if message == CAN_SYSTEM_SLEEP || message == SYSTEM_WILL_SLEEP {
            // SAFETY: `root_port` is our open connection; `argument` is the notification ID
            // IOKit passed with this message.
            unsafe { IOAllowPowerChange(self.root_port, argument as isize) };
        }
    }
}

impl Drop for PowerNotifier {
    fn drop(&mut self) {
        // SAFETY: all three came from one successful `IORegisterForSystemPower`, and are
        // released once, in the order Apple's sample does. Destroying the port removes its
        // run loop source.
        unsafe {
            IODeregisterForSystemPower(&mut self.notifier);
            IOServiceClose(self.root_port);
            IONotificationPortDestroy(self.port);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sleep_and_wake_matter() {
        assert_eq!(classify(SYSTEM_WILL_SLEEP), Some(PowerEvent::WillSleep));
        assert_eq!(classify(SYSTEM_HAS_POWERED_ON), Some(PowerEvent::Woke));
        assert_eq!(classify(CAN_SYSTEM_SLEEP), None);
    }
}
