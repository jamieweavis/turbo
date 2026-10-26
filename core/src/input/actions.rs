//! Performs remapped actions by posting synthetic events (or poking the Dock).
//!
//! Everything posted here is stamped with [`SYNTHETIC_EVENT_MARKER`] so our own tap lets
//! it through. Events go in at the HID level so they look like real input to the system,
//! including the Dock's symbolic hotkeys (Spaces switching).

use std::ffi::c_int;
use std::process::Command;

use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
};

use super::SYNTHETIC_EVENT_MARKER;
use crate::config::{Action, Modifier};

// Virtual key codes (`kVK_*` from HIToolbox/Events.h).
const KEY_LEFT_BRACKET: CGKeyCode = 0x21;
const KEY_RIGHT_BRACKET: CGKeyCode = 0x1E;
const KEY_EQUAL: CGKeyCode = 0x18;
const KEY_MINUS: CGKeyCode = 0x1B;
const KEY_LEFT_ARROW: CGKeyCode = 0x7B;
const KEY_RIGHT_ARROW: CGKeyCode = 0x7C;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// Private HIServices call the Dock listens to. The second argument is unused.
    fn CoreDockSendNotification(notification: &CFString, unused: c_int);
}

pub(crate) fn perform(action: &Action) {
    match action {
        Action::Back => post_key(KEY_LEFT_BRACKET, &[Modifier::Command]),
        Action::Forward => post_key(KEY_RIGHT_BRACKET, &[Modifier::Command]),
        Action::MissionControl => dock_notification("com.apple.expose.awake"),
        Action::AppExpose => dock_notification("com.apple.expose.front.awake"),
        Action::ShowDesktop => dock_notification("com.apple.showdesktop.awake"),
        // macOS 26 replaced Launchpad with the Apps browser, which has no Dock hook.
        Action::Launchpad => open_app("/System/Applications/Apps.app"),
        Action::PreviousSpace => post_key(KEY_LEFT_ARROW, &[Modifier::Control]),
        Action::NextSpace => post_key(KEY_RIGHT_ARROW, &[Modifier::Control]),
        Action::MiddleClick => post_click(crate::config::BUTTON_MIDDLE),
        Action::KeyboardShortcut {
            key_code,
            modifiers,
        } => post_key(*key_code, modifiers),
    }
}

/// One zoom step: ⌘= or ⌘-. The flags are set explicitly, so whichever modifier the user
/// holds to zoom is replaced by Command rather than combined with it.
pub(crate) fn post_zoom(zoom_in: bool) {
    post_key(
        if zoom_in { KEY_EQUAL } else { KEY_MINUS },
        &[Modifier::Command],
    );
}

/// A full click (down + up) of `button` at the cursor.
pub(crate) fn post_click(button: u32) {
    post_button(button, true);
    post_button(button, false);
}

/// A single down or up of `button` at the cursor.
pub(crate) fn post_button(button: u32, down: bool) {
    let location = CGEvent::new(None)
        .map(|e| CGEvent::location(Some(&e)))
        .unwrap_or_default();
    let event_type = match (button, down) {
        (0, true) => CGEventType::LeftMouseDown,
        (0, false) => CGEventType::LeftMouseUp,
        (1, true) => CGEventType::RightMouseDown,
        (1, false) => CGEventType::RightMouseUp,
        (_, true) => CGEventType::OtherMouseDown,
        (_, false) => CGEventType::OtherMouseUp,
    };
    let Some(event) = CGEvent::new_mouse_event(None, event_type, location, CGMouseButton(button))
    else {
        return;
    };
    CGEvent::set_integer_value_field(
        Some(&event),
        CGEventField::MouseEventButtonNumber,
        button.into(),
    );
    CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventClickState, 1);
    post(&event);
}

fn post_key(key_code: CGKeyCode, modifiers: &[Modifier]) {
    let mut flags = flags_for(modifiers);
    if matches!(key_code, KEY_LEFT_ARROW | KEY_RIGHT_ARROW) {
        // Real arrow keys carry these; the Spaces hotkeys don't match without them.
        flags |= CGEventFlags::MaskNumericPad | CGEventFlags::MaskSecondaryFn;
    }
    for down in [true, false] {
        if let Some(event) = CGEvent::new_keyboard_event(None, key_code, down) {
            CGEvent::set_flags(Some(&event), flags);
            post(&event);
        }
    }
}

pub(crate) fn flags_for(modifiers: &[Modifier]) -> CGEventFlags {
    modifiers
        .iter()
        .fold(CGEventFlags::empty(), |flags, m| flags | modifier_flag(*m))
}

pub(crate) fn modifier_flag(modifier: Modifier) -> CGEventFlags {
    match modifier {
        Modifier::Shift => CGEventFlags::MaskShift,
        Modifier::Control => CGEventFlags::MaskControl,
        Modifier::Option => CGEventFlags::MaskAlternate,
        Modifier::Command => CGEventFlags::MaskCommand,
    }
}

fn post(event: &CFRetained<CGEvent>) {
    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::EventSourceUserData,
        SYNTHETIC_EVENT_MARKER,
    );
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
}

fn dock_notification(name: &'static str) {
    let name = CFString::from_static_str(name);
    // SAFETY: takes a valid CFString; the Dock ignores names it doesn't know.
    unsafe { CoreDockSendNotification(&name, 0) };
}

/// Launches off the tap thread so a slow `open` never stalls input.
fn open_app(path: &'static str) {
    std::thread::spawn(move || {
        let _ = Command::new("/usr/bin/open").arg(path).status();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_combine_modifiers() {
        assert_eq!(flags_for(&[]), CGEventFlags::empty());
        assert_eq!(
            flags_for(&[Modifier::Command, Modifier::Shift]),
            CGEventFlags::MaskCommand | CGEventFlags::MaskShift
        );
    }
}
