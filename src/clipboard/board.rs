//! Reading copies from, and writing items back to, the general pasteboard.

use std::fs;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypeFileURL,
    NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};

use super::capture::{self, Content};
use super::store::{Source, Stored};
use crate::error::CoreError;

/// The pasteboard's change counter; it increments on every write by any app.
pub(super) fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

/// What was just copied, if it should be recorded. Files win over text (Finder also puts
/// the names there), and text wins over images (Office apps add a picture of the text).
pub(super) fn read(ignored_apps: &[String]) -> Option<(Content, Source)> {
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        let types: Vec<String> = pasteboard
            .types()
            .map(|types| types.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default();
        let source = frontmost_app();
        if types.is_empty()
            || capture::should_skip(&types, source.bundle_id.as_deref(), ignored_apps)
        {
            return None;
        }

        let content = read_files(&pasteboard)
            .or_else(|| {
                // SAFETY: the pasteboard type constants are immutable AppKit statics.
                let text = pasteboard.stringForType(unsafe { NSPasteboardTypeString })?;
                capture::classify_text(&text.to_string())
            })
            .or_else(|| read_image(&pasteboard))?;
        Some((content, source))
    })
}

/// Puts a stored item back on the pasteboard.
pub(super) fn write(stored: &Stored) -> Result<(), CoreError> {
    autoreleasepool(|_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        let written = match stored {
            // SAFETY (all arms): the pasteboard type constants are immutable AppKit statics.
            Stored::Text(text) => pasteboard
                .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString }),
            Stored::Image(path) => {
                let png = fs::read(path)?;
                let data = NSData::with_bytes(&png);
                // TIFF too, for older apps that don't read PNG.
                let tiff = NSBitmapImageRep::imageRepWithData(&data)
                    .and_then(|rep| rep.TIFFRepresentation());
                let mut ok =
                    pasteboard.setData_forType(Some(&data), unsafe { NSPasteboardTypePNG });
                if let Some(tiff) = tiff {
                    ok |= pasteboard.setData_forType(Some(&tiff), unsafe { NSPasteboardTypeTIFF });
                }
                ok
            }
            Stored::Files(paths) => {
                let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
                    .iter()
                    .map(|path| {
                        ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(
                            path,
                        )))
                    })
                    .collect();
                pasteboard.writeObjects(&NSArray::from_retained_slice(&urls))
            }
        };
        if written {
            Ok(())
        } else {
            Err(CoreError::Other {
                message: "couldn't write to the clipboard".into(),
            })
        }
    })
}

fn read_files(pasteboard: &NSPasteboard) -> Option<Content> {
    let items = pasteboard.pasteboardItems()?;
    let paths: Vec<String> = items
        .iter()
        .filter_map(|item| {
            // SAFETY: immutable AppKit constant.
            let url = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
            let url = NSURL::URLWithString(&url)?;
            url.isFileURL()
                .then(|| url.path())?
                .map(|path| path.to_string())
        })
        .collect();
    (!paths.is_empty()).then_some(Content::Files(paths))
}

fn read_image(pasteboard: &NSPasteboard) -> Option<Content> {
    // SAFETY: immutable AppKit constants.
    let (png, tiff) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
    // PNG bytes are kept as they are, so pasting the item back hashes the same; anything
    // else (TIFF) is converted to PNG.
    if let Some(data) = pasteboard.dataForType(png) {
        let rep = NSBitmapImageRep::imageRepWithData(&data)?;
        return Some(image(data.to_vec(), &rep));
    }
    let data = pasteboard.dataForType(tiff)?;
    let rep = NSBitmapImageRep::imageRepWithData(&data)?;
    // SAFETY: an empty properties dictionary is valid for PNG encoding.
    let converted = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(image(converted.to_vec(), &rep))
}

fn image(png: Vec<u8>, rep: &NSBitmapImageRep) -> Content {
    Content::Image {
        png,
        width: rep.pixelsWide() as i64,
        height: rep.pixelsHigh() as i64,
    }
}

/// The app that's frontmost now, i.e. the one the copy came from.
fn frontmost_app() -> Source {
    let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
        return Source::default();
    };
    Source {
        app: app.localizedName().map(|name| name.to_string()),
        bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
    }
}
