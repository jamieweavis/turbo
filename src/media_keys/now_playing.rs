//! Which supported player was used last, for `MediaTarget::LastActive`.
//!
//! Spotify and Music broadcast a distributed notification whenever playback starts, pauses
//! or changes track. Listening needs no permission and asks nothing of the apps, so it's
//! how a player you started by hand becomes the one the keys go to. Players that don't
//! broadcast (VLC) become the last active one when Jaycast sends them a key.

use std::ffi::c_void;
use std::ptr;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{
    CFDictionary, CFNotificationCenter, CFNotificationName, CFNotificationSuspensionBehavior,
    CFString,
};
use objc2_foundation::{NSDictionary, NSString};
use parking_lot::Mutex;

use super::players::{PLAYERS, Player};

/// One per process: the notification callback is a C function with no state of its own,
/// and its observer token is this static's address.
static LAST_ACTIVE: Mutex<Option<&'static str>> = Mutex::new(None);
static LISTENING: Mutex<bool> = Mutex::new(false);

pub(crate) fn last_active() -> Option<&'static str> {
    *LAST_ACTIVE.lock()
}

pub(crate) fn mark_active(player: &Player) {
    let mut last = LAST_ACTIVE.lock();
    if *last != Some(player.bundle_id) {
        log::debug!("{} is the last active player", player.name);
        *last = Some(player.bundle_id);
    }
}

fn observer_token() -> *const c_void {
    ptr::addr_of!(LAST_ACTIVE).cast()
}

/// Starts or stops listening. Notifications arrive on the main run loop.
pub(crate) fn listen(on: bool) {
    let mut listening = LISTENING.lock();
    if *listening == on {
        return;
    }
    let Some(center) = CFNotificationCenter::distributed_center() else {
        log::warn!("no distributed notification center; last active player can't be tracked");
        return;
    };
    for player in PLAYERS {
        let Some(name) = player.state_notification else {
            continue;
        };
        let name = CFString::from_str(name);
        // SAFETY: the token is a static's address, valid for the whole process; the
        // callback matches CFNotificationCallback.
        unsafe {
            if on {
                center.add_observer(
                    observer_token(),
                    Some(on_notification),
                    Some(&name),
                    ptr::null(),
                    CFNotificationSuspensionBehavior::DeliverImmediately,
                );
            } else {
                center.remove_observer(observer_token(), Some(&name), ptr::null());
            }
        }
    }
    *listening = on;
}

unsafe extern "C-unwind" fn on_notification(
    _center: *mut CFNotificationCenter,
    _observer: *mut c_void,
    name: *const CFNotificationName,
    _object: *const c_void,
    user_info: *const CFDictionary,
) {
    // A panic must not unwind into CoreFoundation.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: CF passes a valid name (a CFString) for the duration of the callback.
        let Some(name) = (unsafe { name.as_ref() }) else {
            return;
        };
        let name = name.to_string();
        let Some(player) = PLAYERS
            .iter()
            .find(|p| p.state_notification == Some(name.as_str()))
        else {
            return;
        };
        // SAFETY: a distributed notification's user info is a property-list dictionary,
        // toll-free bridged to NSDictionary, valid for the duration of the callback.
        let info = unsafe {
            user_info
                .cast::<NSDictionary<NSString, AnyObject>>()
                .as_ref()
        };
        if counts_as_active(info.and_then(player_state).as_deref()) {
            mark_active(player);
        }
    }));
}

fn player_state(info: &NSDictionary<NSString, AnyObject>) -> Option<String> {
    let value: Retained<AnyObject> = info.objectForKey(&NSString::from_str("Player State"))?;
    Some(value.downcast::<NSString>().ok()?.to_string())
}

/// Playing or paused means it was just used; stopped (or quitting) doesn't.
fn counts_as_active(state: Option<&str>) -> bool {
    matches!(state, Some("Playing" | "Paused"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playing_and_paused_count() {
        assert!(counts_as_active(Some("Playing")));
        assert!(counts_as_active(Some("Paused")));
        assert!(!counts_as_active(Some("Stopped")));
        assert!(!counts_as_active(None));
    }

    #[test]
    fn listening_toggles_without_crashing() {
        listen(true);
        listen(true);
        listen(false);
        listen(false);
    }
}
