//! The general pasteboard.

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use crate::error::CoreError;

/// Replaces the clipboard's contents with `text`.
pub(crate) fn copy(text: &str) -> Result<(), CoreError> {
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        // SAFETY: `NSPasteboardTypeString` is an immutable AppKit constant.
        let written = pasteboard
            .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        if written {
            Ok(())
        } else {
            Err(CoreError::Other {
                message: "couldn't write to the clipboard".into(),
            })
        }
    })
}
