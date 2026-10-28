//! Typing a snippet into the frontmost app: through the pasteboard and ⌘V, with the
//! previous clipboard put back afterwards.
//!
//! Pasting inserts any amount of text in one undoable step and works with every keyboard
//! layout, which typing it key by key wouldn't. The cost is borrowing the clipboard: what
//! was there is copied out first (every item and type) and written back once the target app
//! has had time to read the paste. Both writes carry `org.nspasteboard.TransientType`, so
//! Jaycast's clipboard history and other clipboard managers skip them; for the restore that
//! means the user's original copy isn't recorded a second time (it was recorded when they
//! made it), at the price of the marker staying on it.
//!
//! Insertions run one at a time (a lock spans write → paste → restore), so a second
//! snippet never mistakes the first one's text for the user's clipboard.

use parking_lot::Mutex;
use std::thread;
use std::time::Duration;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
use objc2_core_graphics::{CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGKeyCode};
use objc2_foundation::{NSArray, NSData, NSDate, NSDateFormatter, NSDateFormatterStyle, NSString};

use super::placeholders::{self, CLIPBOARD, DATE, DATETIME, Expanded, TIME};
use crate::input::SYNTHETIC_EVENT_MARKER;

/// nspasteboard.org's marker for short-lived clipboard contents that shouldn't be recorded.
const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

/// How long the target app gets to read the pasteboard after ⌘V before the previous
/// contents go back. Apps read it while handling the key event, but a busy app (or an
/// Electron one) can take a few hundred milliseconds to get to it.
const RESTORE_DELAY: Duration = Duration::from_millis(600);

// Virtual key codes (`kVK_*` from HIToolbox/Events.h).
const KEY_V: CGKeyCode = 0x09;
const KEY_DELETE: CGKeyCode = 0x33;
const KEY_LEFT_ARROW: CGKeyCode = 0x7B;

/// Held from writing the snippet until the clipboard is restored.
static INSERTING: Mutex<()> = Mutex::new(());

/// Erases `erase` characters before the caret (a just-typed keyword), then pastes `text`
/// with its placeholders filled, leaving the caret at `{cursor}`. Waits `delay` first (for
/// the palette to close). Blocks until the clipboard is restored: call it off the main
/// thread and off any event tap's thread.
pub(super) fn run(text: &str, erase: usize, delay: Duration) {
    let _inserting = INSERTING.lock();
    thread::sleep(delay);
    autoreleasepool(|_| {
        // Before anything touches the pasteboard, so `{clipboard}` is the user's copy.
        let expanded = expand_now(text);
        post_key_repeatedly(KEY_DELETE, CGEventFlags::empty(), erase);
        if expanded.text.is_empty() {
            return;
        }

        let pasteboard = NSPasteboard::generalPasteboard();
        let saved = Snapshot::take(&pasteboard);
        let Some(written) = write_transient(&pasteboard, &expanded.text) else {
            saved.restore(&pasteboard);
            eprintln!("jaycast: couldn't put the snippet on the clipboard");
            return;
        };
        post_key_repeatedly(KEY_V, CGEventFlags::MaskCommand, 1);
        // Real arrow keys carry these flags; setting them explicitly also keeps a modifier
        // the user is still holding (⇧ from typing the keyword) from turning it into a
        // selection.
        post_key_repeatedly(
            KEY_LEFT_ARROW,
            CGEventFlags::MaskNumericPad | CGEventFlags::MaskSecondaryFn,
            expanded.caret_left,
        );

        thread::sleep(RESTORE_DELAY);
        // Something new was copied meanwhile: that's the clipboard now, keep it.
        if pasteboard.changeCount() == written {
            saved.restore(&pasteboard);
        }
    });
}

/// Fills placeholders with the current clipboard text and the date and time in the user's
/// locale formats. Only reads what the snippet uses.
pub(super) fn expand_now(text: &str) -> Expanded {
    let date = |date, time| {
        let formatter = NSDateFormatter::new();
        formatter.setDateStyle(date);
        formatter.setTimeStyle(time);
        formatter.stringFromDate(&NSDate::now()).to_string()
    };
    use NSDateFormatterStyle as Style;
    placeholders::expand(text, |name: &str| match name {
        // No text on the clipboard (an image, say) fills in nothing.
        CLIPBOARD => Some(crate::pasteboard::paste().unwrap_or_default()),
        // "26 Oct 2026" / "Oct 26, 2026": unambiguous across locales, unlike 26/10/2026.
        DATE => Some(date(Style::MediumStyle, Style::NoStyle)),
        TIME => Some(date(Style::NoStyle, Style::ShortStyle)),
        DATETIME => Some(date(Style::MediumStyle, Style::ShortStyle)),
        _ => None,
    })
}

/// Replaces the clipboard with `text`, marked transient. Returns the change count after
/// the write, to tell later whether anyone else has written since.
fn write_transient(pasteboard: &NSPasteboard, text: &str) -> Option<isize> {
    pasteboard.clearContents();
    // SAFETY: `NSPasteboardTypeString` is an immutable AppKit constant.
    let written =
        pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
    let marked = pasteboard.setData_forType(
        Some(&NSData::with_bytes(&[])),
        &NSString::from_str(TRANSIENT_TYPE),
    );
    (written && marked).then(|| pasteboard.changeCount())
}

/// Everything on the pasteboard: each item's data for each of its types.
struct Snapshot {
    items: Vec<Vec<(Retained<NSString>, Retained<NSData>)>>,
}

impl Snapshot {
    /// Reading every type makes apps that promised data provide it now, which is what a
    /// paste of that type would do anyway.
    fn take(pasteboard: &NSPasteboard) -> Self {
        let items = pasteboard
            .pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.types()
                            .iter()
                            .filter_map(|kind| {
                                let data = item.dataForType(&kind)?;
                                Some((kind, data))
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|types| !types.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Self { items }
    }

    /// Writes the items back, the first one marked transient so the restore isn't recorded
    /// as a new copy. An empty snapshot leaves the clipboard empty, as it was.
    fn restore(&self, pasteboard: &NSPasteboard) {
        pasteboard.clearContents();
        if self.items.is_empty() {
            return;
        }
        let transient = NSString::from_str(TRANSIENT_TYPE);
        let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, types)| {
                let item = NSPasteboardItem::new();
                for (kind, data) in types {
                    item.setData_forType(data, kind);
                }
                if i == 0 && !types.iter().any(|(kind, _)| **kind == *transient) {
                    item.setData_forType(&NSData::with_bytes(&[]), &transient);
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        if !pasteboard.writeObjects(&NSArray::from_retained_slice(&items)) {
            eprintln!("jaycast: couldn't restore the clipboard after a snippet");
        }
    }
}

/// Presses and releases a key `count` times with exactly `flags` held, stamped as ours so
/// Jaycast's own taps (the expander included) ignore it.
fn post_key_repeatedly(key: CGKeyCode, flags: CGEventFlags, count: usize) {
    for _ in 0..count {
        for down in [true, false] {
            let Some(event) = CGEvent::new_keyboard_event(None, key, down) else {
                return;
            };
            CGEvent::set_flags(Some(&event), flags);
            CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::EventSourceUserData,
                SYNTHETIC_EVENT_MARKER,
            );
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Formats placeholders with this machine's locale; reads the clipboard only if
    /// `{clipboard}` were used (it isn't). `cargo test -- --ignored snippet_dates`.
    #[test]
    #[ignore]
    fn snippet_dates_in_this_locale() {
        let expanded = expand_now("{date} | {time} | {datetime} | {cursor}");
        println!("{:?}", expanded.text);
        assert_eq!(expanded.caret_left, 0);
    }
}
