//! System actions listed alongside apps in search: lock, sleep, restart, dark mode, …
//!
//! Each action picks the mechanism that needs the fewest permission prompts:
//! - Lock Screen: `SACLockScreenImmediate` (private login.framework). It needs no permission
//!   and is what the menu bar's Lock Screen item uses; if the symbol ever disappears we fall
//!   back to posting ⌃⌘Q, which needs the Accessibility grant Backpack already asks for.
//! - Sleep / Sleep Displays: `pmset sleepnow` / `displaysleepnow`, which any user may run.
//! - Restart / Shut Down / Log Out: the Apple Events loginwindow handles for the Apple menu
//!   (`aevt/rest`, `aevt/shut`, `aevt/rlgo`). Apps can still cancel to save documents. The
//!   shell confirms first, so these skip loginwindow's own dialog. macOS may ask once for
//!   Automation consent to control loginwindow (unverified: never run while testing).
//! - Toggle Dark Mode: `SLSSetAppearanceThemeLegacy` (private SkyLight). The only public
//!   route is scripting System Events, which needs Automation consent; that's the fallback.
//! - Empty Trash: scripting Finder, which needs Automation consent once. Deleting the trash
//!   ourselves would need Full Disk Access for `~/.Trash`, miss other volumes' trashes, and
//!   trip over locked or in-use files that Finder already knows how to handle.
//! - Start Screen Saver: opening ScreenSaverEngine.app.
//! - Toggle Mute: AppleScript's `set volume`, a Standard Addition that runs inside osascript
//!   and needs no consent.
//! - Volume Up / Down, Toggle Microphone Mute: Core Audio (`audio`), public and promptless.
//! - Toggle Wi-Fi: CoreWLAN's `setPower`, public and promptless unless the admin has set
//!   "Require administrator authorization to turn Wi-Fi on or off".
//! - Eject All Disks: `NSWorkspace.unmountAndEjectDevice`, one volume after another on a
//!   thread; failures (a disk in use) are reported through the observer.
//! - Mission Control / Show Desktop / Show Application Windows / Toggle Dock Auto-Hide:
//!   CoreDock (private, in HIServices), which is what the Dock's own gestures use; no
//!   permission. Mission Control falls back to opening Mission Control.app, auto-hide to
//!   scripting System Events.
//! - Take Screenshot: opening Screenshot.app (its toolbar), which records the screen
//!   itself, so Backpack needs no Screen Recording permission.
//!
//! Nothing here blocks the caller for long: helpers run as child processes, reaped on a
//! thread, and the Core Audio, CoreWLAN and CoreDock calls return within milliseconds.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::CFString;
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use objc2_core_wlan::CWWiFiClient;
use objc2_foundation::{
    NSArray, NSFileManager, NSNumber, NSString, NSURL, NSURLVolumeIsBrowsableKey,
    NSURLVolumeIsEjectableKey, NSURLVolumeIsInternalKey, NSURLVolumeIsLocalKey,
    NSURLVolumeIsRemovableKey, NSURLVolumeIsRootFileSystemKey, NSURLVolumeLocalizedNameKey,
    NSVolumeEnumerationOptions,
};

use super::Status;
use super::audio;
use crate::error::CoreError;

/// A built-in system action, listed alongside apps in search.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SystemAction {
    /// Stable id (`lock-screen`, `sleep`, …); search results use `system:<id>`.
    pub id: &'static str,
    pub title: &'static str,
    /// Extra words it should match ("lock" also finds "Lock Screen" via "secure", …).
    pub keywords: &'static [&'static str],
    /// SF Symbol shown in the palette.
    pub symbol: &'static str,
    pub needs_confirmation: bool,
}

const ACTIONS: &[SystemAction] = &[
    SystemAction {
        id: "lock-screen",
        title: "Lock Screen",
        keywords: &["secure", "away", "login"],
        symbol: "lock.fill",
        needs_confirmation: false,
    },
    SystemAction {
        id: "sleep",
        title: "Sleep",
        keywords: &["suspend", "standby"],
        symbol: "moon.fill",
        needs_confirmation: false,
    },
    SystemAction {
        id: "sleep-displays",
        title: "Sleep Displays",
        keywords: &["screen", "monitor", "display", "off"],
        symbol: "display",
        needs_confirmation: false,
    },
    SystemAction {
        id: "restart",
        title: "Restart",
        keywords: &["reboot"],
        symbol: "arrow.clockwise",
        needs_confirmation: true,
    },
    SystemAction {
        id: "shut-down",
        title: "Shut Down",
        keywords: &["shutdown", "power off", "turn off"],
        symbol: "power",
        needs_confirmation: true,
    },
    SystemAction {
        id: "log-out",
        title: "Log Out",
        keywords: &["logout", "sign out", "log off"],
        symbol: "rectangle.portrait.and.arrow.right",
        needs_confirmation: true,
    },
    SystemAction {
        id: "toggle-dark-mode",
        title: "Toggle Dark Mode",
        keywords: &["appearance", "theme", "light mode", "night"],
        symbol: "circle.lefthalf.filled",
        needs_confirmation: false,
    },
    SystemAction {
        id: "empty-trash",
        title: "Empty Trash",
        keywords: &["bin", "rubbish", "garbage", "delete"],
        symbol: "trash",
        needs_confirmation: true,
    },
    SystemAction {
        id: "screen-saver",
        title: "Start Screen Saver",
        keywords: &["screensaver"],
        symbol: "sparkles",
        needs_confirmation: false,
    },
    SystemAction {
        id: "toggle-mute",
        title: "Toggle Mute",
        keywords: &["sound", "volume", "audio", "silence", "unmute"],
        symbol: "speaker.slash.fill",
        needs_confirmation: false,
    },
    SystemAction {
        id: "volume-up",
        title: "Volume Up",
        keywords: &["louder", "sound", "increase volume", "raise volume"],
        symbol: "speaker.wave.3.fill",
        needs_confirmation: false,
    },
    SystemAction {
        id: "volume-down",
        title: "Volume Down",
        keywords: &["quieter", "sound", "decrease volume", "lower volume"],
        symbol: "speaker.wave.1.fill",
        needs_confirmation: false,
    },
    SystemAction {
        id: "toggle-mic-mute",
        title: "Toggle Microphone Mute",
        keywords: &[
            "mic",
            "mute microphone",
            "unmute microphone",
            "mute mic",
            "input",
        ],
        symbol: "mic.slash.fill",
        needs_confirmation: false,
    },
    SystemAction {
        id: "toggle-wifi",
        title: "Toggle Wi-Fi",
        keywords: &["wifi", "wireless", "internet", "network", "airport"],
        symbol: "wifi",
        needs_confirmation: false,
    },
    SystemAction {
        id: "eject-all-disks",
        title: "Eject All Disks",
        keywords: &["unmount", "drives", "usb", "external", "volumes", "dmg"],
        symbol: "eject.fill",
        needs_confirmation: false,
    },
    SystemAction {
        id: "mission-control",
        title: "Mission Control",
        keywords: &["expose", "exposé", "spaces", "all windows", "overview"],
        symbol: "rectangle.3.group",
        needs_confirmation: false,
    },
    SystemAction {
        id: "show-desktop",
        title: "Show Desktop",
        keywords: &["desktop", "hide windows", "clear screen"],
        symbol: "menubar.dock.rectangle",
        needs_confirmation: false,
    },
    SystemAction {
        id: "app-windows",
        title: "Show Application Windows",
        keywords: &["app expose", "app exposé", "app windows"],
        symbol: "macwindow.on.rectangle",
        needs_confirmation: false,
    },
    SystemAction {
        id: "toggle-dock-autohide",
        title: "Toggle Dock Auto-Hide",
        keywords: &["dock", "hide dock", "show dock", "autohide"],
        symbol: "dock.rectangle",
        needs_confirmation: false,
    },
    SystemAction {
        id: "screenshot",
        title: "Take Screenshot",
        keywords: &[
            "screen capture",
            "screenshot",
            "screen recording",
            "record screen",
            "capture",
        ],
        symbol: "camera.viewfinder",
        needs_confirmation: false,
    },
];

pub(crate) fn actions() -> &'static [SystemAction] {
    ACTIONS
}

/// Runs an action by id. Must not block for long: the shell calls it from the main thread.
/// Returns what to confirm to the user, for actions whose effect isn't visible by itself
/// (a pill reading "Wi-Fi off"). `on_failure` gets a message if work the action left
/// running fails later (a disk that couldn't be ejected), on a background thread.
pub(crate) fn perform(
    id: &str,
    on_failure: impl FnOnce(String) + Send + 'static,
) -> Result<Option<Status>, CoreError> {
    let mechanism = mechanism(id).ok_or_else(|| CoreError::Other {
        message: format!("unknown system action {id}"),
    })?;
    match mechanism {
        Mechanism::LockScreen => lock_screen(),
        Mechanism::ToggleDarkMode => toggle_dark_mode(id),
        Mechanism::Command(program, args) => spawn(id, program, args),
        Mechanism::AppleScript(script) => spawn(id, OSASCRIPT, &["-e", script]),
        Mechanism::StepVolume { up } => {
            return audio::step_output_volume(up).map(|volume| Some(volume_status(volume)));
        }
        Mechanism::ToggleMicMute => {
            let (muted, name) = audio::toggle_input_mute()?;
            return Ok(Some(if muted {
                Status::new("mic.slash.fill", format!("{name} muted"))
            } else {
                Status::new("mic.fill", format!("{name} on"))
            }));
        }
        Mechanism::ToggleWifi => return toggle_wifi().map(Some),
        Mechanism::EjectAll => return eject_all(on_failure).map(Some),
        Mechanism::Dock {
            notification,
            fallback,
        } => dock_notification(id, notification, fallback),
        Mechanism::ToggleDockAutoHide => return toggle_dock_autohide(id),
    }
    .map(|()| None)
}

/// "Volume 56%", with the speaker icon for the level.
pub(crate) fn volume_status(volume: f32) -> Status {
    let percent = (volume * 100.0).round() as u32;
    let symbol = match percent {
        0 => "speaker.fill",
        1..=33 => "speaker.wave.1.fill",
        34..=66 => "speaker.wave.2.fill",
        _ => "speaker.wave.3.fill",
    };
    Status::new(symbol, format!("Volume {percent}%"))
}

const OSASCRIPT: &str = "/usr/bin/osascript";
const PMSET: &str = "/usr/bin/pmset";
const OPEN: &str = "/usr/bin/open";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mechanism {
    LockScreen,
    ToggleDarkMode,
    Command(&'static str, &'static [&'static str]),
    AppleScript(&'static str),
    StepVolume {
        up: bool,
    },
    ToggleMicMute,
    ToggleWifi,
    EjectAll,
    /// A CoreDock notification, with an app to open if CoreDock is missing.
    Dock {
        notification: &'static str,
        fallback: Option<&'static str>,
    },
    ToggleDockAutoHide,
}

fn mechanism(id: &str) -> Option<Mechanism> {
    Some(match id {
        "lock-screen" => Mechanism::LockScreen,
        "sleep" => Mechanism::Command(PMSET, &["sleepnow"]),
        "sleep-displays" => Mechanism::Command(PMSET, &["displaysleepnow"]),
        "restart" => {
            Mechanism::AppleScript(r#"tell application "loginwindow" to «event aevtrest»"#)
        }
        "shut-down" => {
            Mechanism::AppleScript(r#"tell application "loginwindow" to «event aevtshut»"#)
        }
        "log-out" => {
            Mechanism::AppleScript(r#"tell application "loginwindow" to «event aevtrlgo»"#)
        }
        "toggle-dark-mode" => Mechanism::ToggleDarkMode,
        "empty-trash" => Mechanism::AppleScript(r#"tell application "Finder" to empty trash"#),
        "screen-saver" => Mechanism::Command(
            "/usr/bin/open",
            &["/System/Library/CoreServices/ScreenSaverEngine.app"],
        ),
        "toggle-mute" => Mechanism::AppleScript(
            "set volume output muted not (output muted of (get volume settings))",
        ),
        "volume-up" => Mechanism::StepVolume { up: true },
        "volume-down" => Mechanism::StepVolume { up: false },
        "toggle-mic-mute" => Mechanism::ToggleMicMute,
        "toggle-wifi" => Mechanism::ToggleWifi,
        "eject-all-disks" => Mechanism::EjectAll,
        "mission-control" => Mechanism::Dock {
            notification: "com.apple.expose.awake",
            fallback: Some("/System/Applications/Mission Control.app"),
        },
        "show-desktop" => Mechanism::Dock {
            notification: "com.apple.showdesktop.awake",
            fallback: None,
        },
        "app-windows" => Mechanism::Dock {
            notification: "com.apple.expose.front.awake",
            fallback: None,
        },
        "toggle-dock-autohide" => Mechanism::ToggleDockAutoHide,
        "screenshot" => {
            Mechanism::Command(OPEN, &["/System/Applications/Utilities/Screenshot.app"])
        }
        _ => return None,
    })
}

/// Starts a helper process and reaps it on a thread, logging failures (e.g. a declined
/// Automation prompt) since nobody is waiting for the result.
fn spawn(id: &str, program: &str, args: &[&str]) -> Result<(), CoreError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CoreError::Other {
            message: format!("couldn't run {program}: {error}"),
        })?;
    let id = id.to_owned();
    thread::spawn(move || {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => log::warn!("system action {id} failed ({status}): {stderr}"),
            Err(error) => log::warn!("system action {id} failed: {error}"),
        }
    });
    Ok(())
}

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
const RTLD_LAZY: c_int = 0x1;

/// Looks up a function in a (private) framework at runtime, so a missing symbol degrades to
/// a fallback instead of failing to launch.
fn private_symbol(framework: &CStr, symbol: &CStr) -> Option<*mut c_void> {
    // SAFETY: both strings are NUL-terminated; dlopen of an already-loaded or missing
    // framework is harmless, and the handle is intentionally never closed.
    let pointer = unsafe {
        let handle = dlopen(framework.as_ptr(), RTLD_LAZY);
        if handle.is_null() {
            return None;
        }
        dlsym(handle, symbol.as_ptr())
    };
    (!pointer.is_null()).then_some(pointer)
}

fn lock_screen() -> Result<(), CoreError> {
    if let Some(symbol) = private_symbol(
        c"/System/Library/PrivateFrameworks/login.framework/login",
        c"SACLockScreenImmediate",
    ) {
        // SAFETY: `SACLockScreenImmediate` takes no arguments and returns an int status.
        let lock = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn() -> c_int>(symbol) };
        lock();
        return Ok(());
    }
    // ⌃⌘Q, the system's Lock Screen shortcut.
    const KEY_Q: CGKeyCode = 0x0C;
    for down in [true, false] {
        let event =
            CGEvent::new_keyboard_event(None, KEY_Q, down).ok_or_else(|| CoreError::Other {
                message: "couldn't create the lock screen keystroke".into(),
            })?;
        CGEvent::set_flags(
            Some(&event),
            CGEventFlags::MaskCommand | CGEventFlags::MaskControl,
        );
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
    Ok(())
}

fn toggle_dark_mode(id: &str) -> Result<(), CoreError> {
    const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
    let get = private_symbol(SKYLIGHT, c"SLSGetAppearanceThemeLegacy");
    let set = private_symbol(SKYLIGHT, c"SLSSetAppearanceThemeLegacy");
    if let (Some(get), Some(set)) = (get, set) {
        // SAFETY: SkyLight's getter returns a bool (dark) and the setter takes one.
        let get = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn() -> bool>(get) };
        let set = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn(bool)>(set) };
        set(!get());
        return Ok(());
    }
    spawn(
        id,
        OSASCRIPT,
        &[
            "-e",
            r#"tell application "System Events" to tell appearance preferences to set dark mode to not dark mode"#,
        ],
    )
}

fn toggle_wifi() -> Result<Status, CoreError> {
    autoreleasepool(|_| {
        // SAFETY: CoreWLAN's client and interface are safe to use from any thread.
        let interface =
            unsafe { CWWiFiClient::sharedWiFiClient().interface() }.ok_or_else(|| {
                CoreError::Other {
                    message: "this Mac has no Wi-Fi".into(),
                }
            })?;
        // SAFETY: as above; `setPower` reports failure through the error.
        let on = !unsafe { interface.powerOn() };
        unsafe { interface.setPower_error(on) }.map_err(|error| CoreError::Other {
            message: format!(
                "couldn't turn Wi-Fi {}: {}",
                if on { "on" } else { "off" },
                error.localizedDescription()
            ),
        })?;
        Ok(if on {
            Status::new("wifi", "Wi-Fi on")
        } else {
            Status::new("wifi.slash", "Wi-Fi off")
        })
    })
}

/// A mounted volume worth ejecting: (path, name).
fn ejectable_volumes() -> Vec<(String, String)> {
    autoreleasepool(|_| {
        // SAFETY: the keys are immutable framework constants.
        let keys = unsafe {
            [
                NSURLVolumeIsEjectableKey,
                NSURLVolumeIsRemovableKey,
                NSURLVolumeIsInternalKey,
                NSURLVolumeIsLocalKey,
                NSURLVolumeIsBrowsableKey,
                NSURLVolumeIsRootFileSystemKey,
                NSURLVolumeLocalizedNameKey,
            ]
        };
        let [ejectable, removable, internal, local, browsable, root, name] = keys;
        let key_array = NSArray::from_slice(&keys);
        let Some(urls) = NSFileManager::defaultManager()
            .mountedVolumeURLsIncludingResourceValuesForKeys_options(
                Some(&key_array),
                NSVolumeEnumerationOptions::SkipHiddenVolumes,
            )
        else {
            return Vec::new();
        };
        urls.iter()
            .filter_map(|url| {
                let values = url.resourceValuesForKeys_error(&key_array).ok()?;
                let flag = |key| {
                    values
                        .objectForKey(key)
                        .and_then(|value| value.downcast::<NSNumber>().ok())
                        .is_some_and(|number| number.boolValue())
                };
                let path = url.path()?.to_string();
                // Disks Finder would offer to eject: local, visible, not the startup disk,
                // and external or removable (a USB drive, an SD card, a mounted disk image).
                let wanted = path.starts_with("/Volumes/")
                    && flag(local)
                    && flag(browsable)
                    && !flag(root)
                    && (flag(ejectable) || flag(removable) || !flag(internal));
                if !wanted {
                    return None;
                }
                let name = values
                    .objectForKey(name)
                    .and_then(|value| value.downcast::<NSString>().ok())
                    .map_or_else(
                        || path.rsplit('/').next().unwrap_or("").to_owned(),
                        |n| n.to_string(),
                    );
                Some((path, name))
            })
            .collect()
    })
}

/// Ejects every external disk on a thread, since a busy disk can take seconds to refuse.
fn eject_all(on_failure: impl FnOnce(String) + Send + 'static) -> Result<Status, CoreError> {
    let volumes = ejectable_volumes();
    if volumes.is_empty() {
        return Ok(Status::new("eject", "No disks to eject"));
    }
    let count = volumes.len();
    thread::Builder::new()
        .name("backpack-eject".into())
        .spawn(move || {
            let failures: Vec<String> = volumes
                .into_iter()
                .filter_map(|(path, name)| {
                    autoreleasepool(|_| {
                        let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
                        NSWorkspace::sharedWorkspace()
                            .unmountAndEjectDeviceAtURL_error(&url)
                            .err()
                            .map(|error| format!("{name}: {}", error.localizedDescription()))
                    })
                })
                .collect();
            if failures.is_empty() {
                log::info!("ejected {count} disks");
            } else {
                log::warn!("{} of {count} disks couldn't be ejected", failures.len());
                on_failure(failures.join("\n"));
            }
        })
        .map_err(|error| CoreError::Other {
            message: format!("couldn't start ejecting: {error}"),
        })?;
    Ok(Status::new(
        "eject.fill",
        if count == 1 {
            "Ejecting 1 disk".to_owned()
        } else {
            format!("Ejecting {count} disks")
        },
    ))
}

const HISERVICES: &CStr = c"/System/Library/Frameworks/ApplicationServices.framework/Frameworks/HIServices.framework/HIServices";

fn dock_notification(
    id: &str,
    notification: &str,
    fallback: Option<&'static str>,
) -> Result<(), CoreError> {
    if let Some(symbol) = private_symbol(HISERVICES, c"CoreDockSendNotification") {
        // SAFETY: `CoreDockSendNotification(CFStringRef, int)`; the string outlives the call.
        let send = unsafe {
            std::mem::transmute::<*mut c_void, extern "C" fn(*const CFString, c_int)>(symbol)
        };
        let notification = CFString::from_str(notification);
        send(&*notification, 0);
        return Ok(());
    }
    match fallback {
        Some(app) => spawn(id, OPEN, &[app]),
        None => Err(CoreError::Other {
            message: "this version of macOS doesn't support that".into(),
        }),
    }
}

fn toggle_dock_autohide(id: &str) -> Result<Option<Status>, CoreError> {
    let get = private_symbol(HISERVICES, c"CoreDockGetAutoHideEnabled");
    let set = private_symbol(HISERVICES, c"CoreDockSetAutoHideEnabled");
    if let (Some(get), Some(set)) = (get, set) {
        // SAFETY: CoreDock's getter returns a bool and the setter takes one.
        let get = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn() -> bool>(get) };
        let set = unsafe { std::mem::transmute::<*mut c_void, extern "C" fn(bool)>(set) };
        let hide = !get();
        set(hide);
        return Ok(Some(if hide {
            Status::new("dock.arrow.down.rectangle", "Dock auto-hide on")
        } else {
            Status::new("dock.rectangle", "Dock auto-hide off")
        }));
    }
    spawn(
        id,
        OSASCRIPT,
        &[
            "-e",
            r#"tell application "System Events" to tell dock preferences to set autohide to not autohide"#,
        ],
    )?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_kebab_case() {
        let mut seen = HashSet::new();
        for action in actions() {
            assert!(seen.insert(action.id), "duplicate id {}", action.id);
            assert!(
                action
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-'),
                "{} isn't kebab-case",
                action.id
            );
            assert!(!action.title.is_empty() && !action.symbol.is_empty());
        }
    }

    #[test]
    fn every_action_has_a_mechanism() {
        for action in actions() {
            assert!(mechanism(action.id).is_some(), "{} can't run", action.id);
        }
    }

    #[test]
    fn disruptive_actions_confirm() {
        for id in ["restart", "shut-down", "log-out", "empty-trash"] {
            let action = actions().iter().find(|a| a.id == id).unwrap();
            assert!(action.needs_confirmation, "{id} should confirm");
        }
        let lock = actions().iter().find(|a| a.id == "lock-screen").unwrap();
        assert!(!lock.needs_confirmation);
    }

    #[test]
    fn unknown_action_is_an_error() {
        assert!(perform("definitely-not-an-action", |_| {}).is_err());
    }

    #[test]
    fn volume_status_rounds_and_picks_an_icon() {
        assert_eq!(volume_status(0.5625).text, "Volume 56%");
        assert_eq!(volume_status(0.0).symbol, "speaker.fill");
        assert_eq!(volume_status(1.0).symbol, "speaker.wave.3.fill");
    }

    /// Lists what Eject All Disks would eject; ejects nothing.
    #[test]
    #[ignore = "reads the real mounted volumes"]
    fn lists_ejectable_volumes() {
        println!("{:#?}", ejectable_volumes());
    }

    #[test]
    fn private_symbols_resolve_on_this_macos() {
        assert!(
            private_symbol(
                c"/System/Library/PrivateFrameworks/login.framework/login",
                c"SACLockScreenImmediate"
            )
            .is_some()
        );
        assert!(
            private_symbol(
                c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
                c"SLSSetAppearanceThemeLegacy"
            )
            .is_some()
        );
        for symbol in [
            c"CoreDockSendNotification",
            c"CoreDockGetAutoHideEnabled",
            c"CoreDockSetAutoHideEnabled",
        ] {
            assert!(private_symbol(HISERVICES, symbol).is_some(), "{symbol:?}");
        }
        assert!(private_symbol(c"/nonexistent.framework/x", c"nope").is_none());
    }
}
