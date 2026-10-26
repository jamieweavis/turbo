//! A thin, safe wrapper over the parts of the Accessibility (AXUIElement) API that window
//! management needs: the focused window of an app, and its position and size.
//!
//! Attribute names are the strings behind Apple's `kAX…Attribute` macros (they are `CFSTR`
//! literals in the headers, not exported symbols).

use std::ffi::{c_char, c_void};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

use objc2_core_foundation::{CFBoolean, CFRetained, CFString, CFType, CGPoint, CGSize};

pub(super) const FOCUSED_WINDOW: &str = "AXFocusedWindow";
pub(super) const MAIN_WINDOW: &str = "AXMainWindow";
pub(super) const POSITION: &str = "AXPosition";
pub(super) const SIZE: &str = "AXSize";
pub(super) const FULL_SCREEN: &str = "AXFullScreen";
/// Set on an app by assistive tech (VoiceOver and others); while on, some apps animate
/// every frame change, so a set-size/set-position sequence takes visibly long.
pub(super) const ENHANCED_USER_INTERFACE: &str = "AXEnhancedUserInterface";

/// `AXError` codes (AXError.h).
pub(super) type AxError = i32;
const SUCCESS: AxError = 0;
pub(super) const CANNOT_COMPLETE: AxError = -25204;
pub(super) const API_DISABLED: AxError = -25211;

/// `AXValueType` (AXValue.h).
const VALUE_CG_POINT: u32 = 1;
const VALUE_CG_SIZE: u32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub(super) fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> Option<NonNull<CFType>>;
    fn AXUIElementCopyAttributeValue(
        element: &CFType,
        attribute: &CFString,
        value: *mut *const CFType,
    ) -> AxError;
    fn AXUIElementSetAttributeValue(
        element: &CFType,
        attribute: &CFString,
        value: &CFType,
    ) -> AxError;
    fn AXUIElementIsAttributeSettable(
        element: &CFType,
        attribute: &CFString,
        settable: *mut u8,
    ) -> AxError;
    fn AXUIElementSetMessagingTimeout(element: &CFType, seconds: f32) -> AxError;
    fn AXValueCreate(kind: u32, value: *const c_void) -> Option<NonNull<CFType>>;
    fn AXValueGetType(value: &CFType) -> u32;
    fn AXValueGetValue(value: &CFType, kind: u32, out: *mut c_void) -> u8;
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
/// `RTLD_DEFAULT`: search every image loaded into the process.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

type GetWindowFn = unsafe extern "C" fn(element: &CFType, window_id: *mut u32) -> AxError;

/// `_AXUIElementGetWindow`, the private but long-standing HIServices call that maps a window
/// element to its CGWindowID. Looked up at runtime so its absence only costs the memory.
fn get_window_fn() -> Option<GetWindowFn> {
    static FUNCTION: OnceLock<Option<GetWindowFn>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        // SAFETY: the name is NUL-terminated; ApplicationServices is linked (above), so the
        // symbol, if present, is in a loaded image.
        let symbol = unsafe { dlsym(RTLD_DEFAULT, c"_AXUIElementGetWindow".as_ptr()) };
        // SAFETY: when present, the function has this signature (AXUIElementRef,
        // CGWindowID *) -> AXError.
        (!symbol.is_null())
            .then(|| unsafe { std::mem::transmute::<*mut c_void, GetWindowFn>(symbol) })
    })
}

fn result(error: AxError) -> Result<(), AxError> {
    if error == SUCCESS { Ok(()) } else { Err(error) }
}

/// An `AXUIElementRef`.
pub(super) struct Element(CFRetained<CFType>);

impl Element {
    pub fn application(pid: i32) -> Option<Self> {
        // SAFETY: any pid is accepted; the result follows the Create rule.
        let element = unsafe { AXUIElementCreateApplication(pid) }?;
        // SAFETY: we own the +1 reference returned above.
        Some(Self(unsafe { CFRetained::from_raw(element) }))
    }

    /// Bounds how long calls on this element wait for an unresponsive app.
    pub fn set_timeout(&self, seconds: f32) {
        // SAFETY: `self.0` is a valid AXUIElement.
        unsafe { AXUIElementSetMessagingTimeout(&self.0, seconds) };
    }

    fn copy(&self, attribute: &str) -> Result<CFRetained<CFType>, AxError> {
        let name = CFString::from_str(attribute);
        let mut value: *const CFType = ptr::null();
        // SAFETY: valid element and string; on success `value` holds a +1 reference (Copy
        // rule) that we take ownership of.
        result(unsafe { AXUIElementCopyAttributeValue(&self.0, &name, &mut value) })?;
        let value = NonNull::new(value.cast_mut()).ok_or(CANNOT_COMPLETE)?;
        Ok(unsafe { CFRetained::from_raw(value) })
    }

    fn set(&self, attribute: &str, value: &CFType) -> Result<(), AxError> {
        let name = CFString::from_str(attribute);
        // SAFETY: valid element, string and value; the call doesn't keep our references.
        result(unsafe { AXUIElementSetAttributeValue(&self.0, &name, value) })
    }

    pub fn element(&self, attribute: &str) -> Result<Element, AxError> {
        // Element-valued attributes hold AXUIElements; were it anything else, calls on it
        // would just fail with an AXError.
        self.copy(attribute).map(Element)
    }

    pub fn bool(&self, attribute: &str) -> Option<bool> {
        let value = self.copy(attribute).ok()?;
        value.downcast_ref::<CFBoolean>().map(CFBoolean::as_bool)
    }

    pub fn set_bool(&self, attribute: &str, value: bool) -> Result<(), AxError> {
        self.set(attribute, CFBoolean::new(value))
    }

    pub fn is_settable(&self, attribute: &str) -> bool {
        let name = CFString::from_str(attribute);
        let mut settable = 0u8;
        // SAFETY: valid element, string and out pointer.
        let error = unsafe { AXUIElementIsAttributeSettable(&self.0, &name, &mut settable) };
        error == SUCCESS && settable != 0
    }

    pub fn position(&self) -> Result<CGPoint, AxError> {
        let mut point = CGPoint::new(0.0, 0.0);
        self.read_value(POSITION, VALUE_CG_POINT, (&raw mut point).cast())?;
        Ok(point)
    }

    pub fn size(&self) -> Result<CGSize, AxError> {
        let mut size = CGSize::new(0.0, 0.0);
        self.read_value(SIZE, VALUE_CG_SIZE, (&raw mut size).cast())?;
        Ok(size)
    }

    pub fn set_position(&self, point: CGPoint) -> Result<(), AxError> {
        self.write_value(POSITION, VALUE_CG_POINT, (&raw const point).cast())
    }

    pub fn set_size(&self, size: CGSize) -> Result<(), AxError> {
        self.write_value(SIZE, VALUE_CG_SIZE, (&raw const size).cast())
    }

    fn read_value(&self, attribute: &str, kind: u32, out: *mut c_void) -> Result<(), AxError> {
        let value = self.copy(attribute)?;
        // SAFETY: `value` is a CF object; AXValueGetType tolerates any AXValue, and we only
        // read it out when it holds `kind`, into storage of that type (the callers' CGPoint
        // or CGSize).
        unsafe {
            if AXValueGetType(&value) != kind || AXValueGetValue(&value, kind, out) == 0 {
                return Err(CANNOT_COMPLETE);
            }
        }
        Ok(())
    }

    fn write_value(&self, attribute: &str, kind: u32, value: *const c_void) -> Result<(), AxError> {
        // SAFETY: `value` points at a CGPoint or CGSize matching `kind`; the result follows
        // the Create rule.
        let value = unsafe { AXValueCreate(kind, value) }.ok_or(CANNOT_COMPLETE)?;
        let value = unsafe { CFRetained::from_raw(value) };
        self.set(attribute, &value)
    }

    /// The CGWindowID of a window element.
    pub fn window_id(&self) -> Option<u32> {
        let get = get_window_fn()?;
        let mut id = 0u32;
        // SAFETY: valid element and out pointer; see `get_window_fn`.
        let error = unsafe { get(&self.0, &mut id) };
        (error == SUCCESS && id != 0).then_some(id)
    }
}
