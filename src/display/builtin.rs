//! The built-in display's brightness, through the private DisplayServices framework (the
//! same calls Control Center uses). It's loaded at runtime so a missing or changed framework
//! only turns the feature off rather than failing to launch.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

use objc2_core_graphics::{CGDirectDisplayID, CGError, CGGetActiveDisplayList};

use super::{CGDisplayIsBuiltin, MAX_DISPLAYS};

const FRAMEWORK: &std::ffi::CStr =
    c"/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";
const RTLD_LAZY: c_int = 0x1;

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type GetBrightness = unsafe extern "C" fn(CGDirectDisplayID, *mut f32) -> c_int;
type SetBrightness = unsafe extern "C" fn(CGDirectDisplayID, f32) -> c_int;

struct DisplayServices {
    get: GetBrightness,
    set: SetBrightness,
}

fn display_services() -> Option<&'static DisplayServices> {
    static SERVICES: OnceLock<Option<DisplayServices>> = OnceLock::new();
    SERVICES
        .get_or_init(|| {
            // SAFETY: dlopen/dlsym with valid C strings; the handle is never closed, so the
            // function pointers stay valid. The signatures match DisplayServices' exports.
            unsafe {
                let handle = dlopen(FRAMEWORK.as_ptr(), RTLD_LAZY);
                if handle.is_null() {
                    return None;
                }
                let get = dlsym(handle, c"DisplayServicesGetBrightness".as_ptr());
                let set = dlsym(handle, c"DisplayServicesSetBrightness".as_ptr());
                if get.is_null() || set.is_null() {
                    return None;
                }
                Some(DisplayServices {
                    get: std::mem::transmute::<*mut c_void, GetBrightness>(get),
                    set: std::mem::transmute::<*mut c_void, SetBrightness>(set),
                })
            }
        })
        .as_ref()
}

/// The built-in display, if it's active (so not on a desktop Mac or a closed MacBook).
fn active_builtin() -> Option<CGDirectDisplayID> {
    let mut ids = [0 as CGDirectDisplayID; MAX_DISPLAYS];
    let mut count = 0u32;
    // SAFETY: the buffer holds MAX_DISPLAYS ids and CG writes at most that many.
    let error =
        unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success {
        return None;
    }
    ids[..count as usize]
        .iter()
        .copied()
        // SAFETY: plain query on a display id.
        .find(|&id| unsafe { CGDisplayIsBuiltin(id) } != 0)
}

/// The built-in display's brightness (0.0–1.0), or `None` when it's off, absent or can't be
/// read.
pub(crate) fn brightness() -> Option<f64> {
    let services = display_services()?;
    let id = active_builtin()?;
    let mut value = 0f32;
    // SAFETY: `value` is a valid out-pointer for the call.
    let status = unsafe { (services.get)(id, &mut value) };
    (status == 0).then_some(f64::from(value))
}

/// Sets the built-in display's brightness (clamped to 0.0–1.0). False when it's off, absent
/// or the write failed.
pub(crate) fn set_brightness(brightness: f64) -> bool {
    let Some(services) = display_services() else {
        return false;
    };
    let Some(id) = active_builtin() else {
        return false;
    };
    // SAFETY: plain call with a display id and a value.
    unsafe { (services.set)(id, brightness.clamp(0.0, 1.0) as f32) == 0 }
}
