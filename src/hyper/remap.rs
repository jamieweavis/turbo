//! The IOKit side of the Hyper key: remapping the chosen key, and toggling Caps Lock.
//!
//! An event tap can't use Caps Lock or a modifier key as Hyper directly: Caps Lock toggles
//! its lock (and light) below the tap, and a modifier's own flag would leak into every
//! event. So the key is remapped in the HID event system (`UserKeyMapping`, what `hidutil`
//! sets) to [`HYPER_KEY_CODE`], an F-key no keyboard has, and the tap turns that into
//! Hyper. The mapping isn't persistent, lives outside the process and survives a crash, so
//! applying it first removes any of ours left behind and keeps everyone else's.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};

use crate::config::HyperKeySource;
use crate::error::CoreError;

/// `kVK_F18`: the virtual key code the Hyper key arrives as.
pub(super) const HYPER_KEY_CODE: u16 = 0x4F;
/// Keyboard/keypad usage page (0x07) F18, in `UserKeyMapping`'s page-and-usage format.
const HYPER_USAGE: i64 = usage(0x6D);

const USER_KEY_MAPPING: &str = "UserKeyMapping";
const MAPPING_SRC: &str = "HIDKeyboardModifierMappingSrc";
const MAPPING_DST: &str = "HIDKeyboardModifierMappingDst";

const ALL_SOURCES: [HyperKeySource; 9] = [
    HyperKeySource::CapsLock,
    HyperKeySource::LeftControl,
    HyperKeySource::LeftShift,
    HyperKeySource::LeftOption,
    HyperKeySource::LeftCommand,
    HyperKeySource::RightControl,
    HyperKeySource::RightShift,
    HyperKeySource::RightOption,
    HyperKeySource::RightCommand,
];

type IOHIDEventSystemClientRef = *mut c_void;
type IoConnect = u32;
type IoService = u32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreateSimpleClient(
        allocator: *const c_void,
    ) -> IOHIDEventSystemClientRef;
    fn IOHIDEventSystemClientSetProperty(
        client: IOHIDEventSystemClientRef,
        key: &CFString,
        property: &CFType,
    ) -> u8;
    fn IOHIDEventSystemClientCopyProperty(
        client: IOHIDEventSystemClientRef,
        key: &CFString,
    ) -> Option<NonNull<CFType>>;

    fn IOServiceMatching(name: *const std::ffi::c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> IoService;
    fn IOServiceOpen(service: IoService, task: u32, kind: u32, connect: *mut IoConnect) -> i32;
    fn IOServiceClose(connect: IoConnect) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOHIDGetModifierLockState(connect: IoConnect, selector: i32, state: *mut bool) -> i32;
    fn IOHIDSetModifierLockState(connect: IoConnect, selector: i32, state: bool) -> i32;
}

unsafe extern "C" {
    static mach_task_self_: u32;
}

/// `kIOHIDParamConnectType` and `kIOHIDCapsLockState` (IOHIDShared.h, IOHIDParameter.h).
const HID_PARAM_CONNECT_TYPE: u32 = 1;
const CAPS_LOCK_STATE: i32 = 1;

const fn usage(id: i64) -> i64 {
    0x7_0000_0000 | id
}

fn source_usage(source: HyperKeySource) -> i64 {
    usage(match source {
        HyperKeySource::CapsLock => 0x39,
        HyperKeySource::LeftControl => 0xE0,
        HyperKeySource::LeftShift => 0xE1,
        HyperKeySource::LeftOption => 0xE2,
        HyperKeySource::LeftCommand => 0xE3,
        HyperKeySource::RightControl => 0xE4,
        HyperKeySource::RightShift => 0xE5,
        HyperKeySource::RightOption => 0xE6,
        HyperKeySource::RightCommand => 0xE7,
    })
}

/// A mapping Turbo made: one of the Hyper key choices onto the Hyper key code.
fn is_ours(src: Option<i64>, dst: Option<i64>) -> bool {
    dst == Some(HYPER_USAGE)
        && src.is_some_and(|src| ALL_SOURCES.iter().any(|&s| source_usage(s) == src))
}

/// Remaps `source` to the Hyper key code, or with `None` removes the remapping.
/// Mappings set by anyone else are kept.
pub(super) fn apply(source: Option<HyperKeySource>) -> Result<(), CoreError> {
    let client = Client::new()?;
    let key = CFString::from_static_str(USER_KEY_MAPPING);
    let src_key = CFString::from_static_str(MAPPING_SRC);
    let dst_key = CFString::from_static_str(MAPPING_DST);

    let existing = client.copy_array(&key);
    let count = existing.len();
    let mut mappings: Vec<CFRetained<CFType>> = existing
        .into_iter()
        .filter(|entry| {
            let Some(dict) = entry.downcast_ref::<CFDictionary>() else {
                return true;
            };
            // SAFETY: only reinterprets the key and value types for lookups; `get` returns
            // whatever is stored as a `CFType`, and keys are compared by CFEqual.
            let dict: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
            let number = |key: &CFString| {
                dict.get(key)
                    .and_then(|v| v.downcast::<CFNumber>().ok())
                    .and_then(|n| n.as_i64())
            };
            !is_ours(number(&src_key), number(&dst_key))
        })
        .collect();
    if source.is_none() && mappings.len() == count {
        // Nothing of ours to remove.
        return Ok(());
    }

    if let Some(source) = source {
        let src = CFNumber::new_i64(source_usage(source));
        let dst = CFNumber::new_i64(HYPER_USAGE);
        let entry =
            CFDictionary::<CFString, CFNumber>::from_slices(&[&src_key, &dst_key], &[&src, &dst]);
        // SAFETY: every CF object is a `CFType`.
        mappings.push(unsafe { CFRetained::cast_unchecked(entry) });
    }

    let array = CFArray::from_retained_objects(&mappings);
    if client.set(&key, &array) {
        Ok(())
    } else {
        Err(CoreError::Other {
            message: "couldn't remap the Hyper key".into(),
        })
    }
}

/// Flips Caps Lock, light included, as if it had been pressed.
pub(super) fn toggle_caps_lock() {
    // SAFETY: plain IOKit calls; the matching dictionary is consumed by
    // `IOServiceGetMatchingService`, and the service and connection are released below.
    unsafe {
        let service = IOServiceGetMatchingService(0, IOServiceMatching(c"IOHIDSystem".as_ptr()));
        if service == 0 {
            return;
        }
        let mut connect: IoConnect = 0;
        if IOServiceOpen(
            service,
            mach_task_self_,
            HID_PARAM_CONNECT_TYPE,
            &mut connect,
        ) == 0
        {
            let mut on = false;
            if IOHIDGetModifierLockState(connect, CAPS_LOCK_STATE, &mut on) == 0 {
                IOHIDSetModifierLockState(connect, CAPS_LOCK_STATE, !on);
            }
            IOServiceClose(connect);
        }
        IOObjectRelease(service);
    }
}

/// An `IOHIDEventSystemClient`, released on drop.
struct Client(NonNull<c_void>);

impl Client {
    fn new() -> Result<Self, CoreError> {
        // SAFETY: a null allocator means the default; returns a +1 reference or null.
        NonNull::new(unsafe { IOHIDEventSystemClientCreateSimpleClient(std::ptr::null()) })
            .map(Self)
            .ok_or_else(|| CoreError::Other {
                message: "couldn't connect to the HID event system".into(),
            })
    }

    /// The property's elements, or none if it's unset or not an array.
    fn copy_array(&self, key: &CFString) -> Vec<CFRetained<CFType>> {
        // SAFETY: the client is valid; the result is a +1 reference or null.
        let Some(value) = (unsafe { IOHIDEventSystemClientCopyProperty(self.0.as_ptr(), key) })
        else {
            return Vec::new();
        };
        // SAFETY: the copy is ours to release.
        let value = unsafe { CFRetained::from_raw(value) };
        match value.downcast::<CFArray>() {
            // SAFETY: arrays hold CF objects.
            Ok(array) => unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(array) }.to_vec(),
            Err(_) => Vec::new(),
        }
    }

    fn set(&self, key: &CFString, value: &CFType) -> bool {
        // SAFETY: the client, key and value are valid for the call.
        unsafe { IOHIDEventSystemClientSetProperty(self.0.as_ptr(), key, value) != 0 }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: we own the +1 reference from `CreateSimpleClient`.
        drop(unsafe { CFRetained::from_raw(self.0.cast::<CFType>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usages_match_hidutil() {
        // `hidutil` documents Caps Lock as 0x700000039 and F18 as 0x70000006D.
        assert_eq!(source_usage(HyperKeySource::CapsLock), 0x7_0000_0039);
        assert_eq!(HYPER_USAGE, 0x7_0000_006D);
        assert_eq!(source_usage(HyperKeySource::RightCommand), 0x7_0000_00E7);
    }

    #[test]
    fn recognises_only_our_mappings() {
        for source in ALL_SOURCES {
            assert!(is_ours(Some(source_usage(source)), Some(HYPER_USAGE)));
        }
        // Someone else's Caps Lock → Escape, or their own key → F18.
        assert!(!is_ours(Some(usage(0x39)), Some(usage(0x29))));
        assert!(!is_ours(Some(usage(0x35)), Some(HYPER_USAGE)));
        assert!(!is_ours(None, Some(HYPER_USAGE)));
        assert!(!is_ours(Some(usage(0x39)), None));
    }

    /// Reads the live mapping without changing it.
    #[test]
    fn reads_the_current_mapping() {
        let client = Client::new().unwrap();
        let _ = client.copy_array(&CFString::from_static_str(USER_KEY_MAPPING));
    }
}
