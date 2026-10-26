//! System actions listed alongside apps in search: lock, sleep, restart, dark mode, …
//!
//! Each action picks the mechanism that needs the fewest permission prompts:
//! - Lock Screen: `SACLockScreenImmediate` (private login.framework). It needs no permission
//!   and is what the menu bar's Lock Screen item uses; if the symbol ever disappears we fall
//!   back to posting ⌃⌘Q, which needs the Accessibility grant Jaycast already asks for.
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
//!
//! Nothing here blocks the caller: helpers run as child processes, reaped on a thread.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;

use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};

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
];

pub(crate) fn actions() -> &'static [SystemAction] {
    ACTIONS
}

/// Runs an action by id. Must not block for long: the shell calls it from the main thread.
pub(crate) fn perform(id: &str) -> Result<(), CoreError> {
    let mechanism = mechanism(id).ok_or_else(|| CoreError::Other {
        message: format!("unknown system action {id}"),
    })?;
    match mechanism {
        Mechanism::LockScreen => lock_screen(),
        Mechanism::ToggleDarkMode => toggle_dark_mode(id),
        Mechanism::Command(program, args) => spawn(id, program, args),
        Mechanism::AppleScript(script) => spawn(id, OSASCRIPT, &["-e", script]),
    }
}

const OSASCRIPT: &str = "/usr/bin/osascript";
const PMSET: &str = "/usr/bin/pmset";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mechanism {
    LockScreen,
    ToggleDarkMode,
    Command(&'static str, &'static [&'static str]),
    AppleScript(&'static str),
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
            Ok(status) => eprintln!("jaycast: system action {id} failed ({status}): {stderr}"),
            Err(error) => eprintln!("jaycast: system action {id} failed: {error}"),
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
        assert!(perform("definitely-not-an-action").is_err());
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
        assert!(private_symbol(c"/nonexistent.framework/x", c"nope").is_none());
    }
}
