//! Window management: snapping, maximising, centring, restoring and moving the focused
//! window between displays, through the Accessibility API.
//!
//! - `layout`: where a window goes for each action (pure, unit-tested).
//! - `ax`: the AXUIElement calls that read and set a window's frame.
//! - `memory`: what Jaycast remembers per window, kept bounded.
//!
//! `perform` reads the displays from `NSScreen` (visible frames, flipped to the top-left
//! origin Accessibility uses), finds the frontmost app's focused window and its frame, plans
//! the target and applies it as size, position, size: an app may clamp a size that doesn't
//! fit the display the window is still on, and the second resize lands once it has moved.
//! Windows that won't take the size are aligned within the target instead. Each window's
//! frame before Jaycast first moved it (for `Restore`) and its last snap (for cycling
//! repeated halves, and for keeping a snap when moving to another display) are remembered
//! per pid and CGWindowID.

mod ax;
mod layout;
mod memory;

use std::sync::Mutex;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSScreen, NSWorkspace};
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::NSRect;

use crate::config::{WindowAction, WindowConfig};
use crate::error::CoreError;
use ax::Element;
use layout::{Plan, PlanError, Rect, Screen};
use memory::{WindowKey, WindowMemory};

/// How long an Accessibility call waits for the target app. The default is several
/// seconds, which would freeze the main thread (and the palette) behind a hung app.
const AX_TIMEOUT_SECONDS: f32 = 0.5;

/// A window action as listed in palette search: (action, id, title, keywords, SF Symbol).
/// Search results use `window:<id>`.
pub(crate) type ActionEntry = (
    WindowAction,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
);

pub(crate) const ACTIONS: &[ActionEntry] = &[
    (
        WindowAction::LeftHalf,
        "left-half",
        "Left Half",
        &["snap", "window", "split"],
        "rectangle.lefthalf.inset.filled",
    ),
    (
        WindowAction::RightHalf,
        "right-half",
        "Right Half",
        &["snap", "window", "split"],
        "rectangle.righthalf.inset.filled",
    ),
    (
        WindowAction::TopHalf,
        "top-half",
        "Top Half",
        &["snap", "window"],
        "rectangle.tophalf.inset.filled",
    ),
    (
        WindowAction::BottomHalf,
        "bottom-half",
        "Bottom Half",
        &["snap", "window"],
        "rectangle.bottomhalf.inset.filled",
    ),
    (
        WindowAction::TopLeft,
        "top-left",
        "Top Left Quarter",
        &["snap", "window", "corner"],
        "rectangle.inset.topleft.filled",
    ),
    (
        WindowAction::TopRight,
        "top-right",
        "Top Right Quarter",
        &["snap", "window", "corner"],
        "rectangle.inset.topright.filled",
    ),
    (
        WindowAction::BottomLeft,
        "bottom-left",
        "Bottom Left Quarter",
        &["snap", "window", "corner"],
        "rectangle.inset.bottomleft.filled",
    ),
    (
        WindowAction::BottomRight,
        "bottom-right",
        "Bottom Right Quarter",
        &["snap", "window", "corner"],
        "rectangle.inset.bottomright.filled",
    ),
    (
        WindowAction::LeftThird,
        "left-third",
        "Left Third",
        &["snap", "window"],
        "rectangle.leftthird.inset.filled",
    ),
    (
        WindowAction::CenterThird,
        "center-third",
        "Center Third",
        &["snap", "window", "middle"],
        "rectangle.center.inset.filled",
    ),
    (
        WindowAction::RightThird,
        "right-third",
        "Right Third",
        &["snap", "window"],
        "rectangle.rightthird.inset.filled",
    ),
    (
        WindowAction::LeftTwoThirds,
        "left-two-thirds",
        "Left Two Thirds",
        &["snap", "window"],
        "rectangle.leftthird.inset.filled",
    ),
    (
        WindowAction::RightTwoThirds,
        "right-two-thirds",
        "Right Two Thirds",
        &["snap", "window"],
        "rectangle.rightthird.inset.filled",
    ),
    (
        WindowAction::TopLeftSixth,
        "top-left-sixth",
        "Top Left Sixth",
        &["snap", "window", "grid"],
        "square.grid.3x3.topleft.filled",
    ),
    (
        WindowAction::TopCenterSixth,
        "top-center-sixth",
        "Top Center Sixth",
        &["snap", "window", "grid"],
        "square.grid.3x3.topmiddle.filled",
    ),
    (
        WindowAction::TopRightSixth,
        "top-right-sixth",
        "Top Right Sixth",
        &["snap", "window", "grid"],
        "square.grid.3x3.topright.filled",
    ),
    (
        WindowAction::BottomLeftSixth,
        "bottom-left-sixth",
        "Bottom Left Sixth",
        &["snap", "window", "grid"],
        "square.grid.3x3.bottomleft.filled",
    ),
    (
        WindowAction::BottomCenterSixth,
        "bottom-center-sixth",
        "Bottom Center Sixth",
        &["snap", "window", "grid"],
        "square.grid.3x3.bottommiddle.filled",
    ),
    (
        WindowAction::BottomRightSixth,
        "bottom-right-sixth",
        "Bottom Right Sixth",
        &["snap", "window", "grid"],
        "square.grid.3x3.bottomright.filled",
    ),
    (
        WindowAction::Maximize,
        "maximize",
        "Maximize Window",
        &["fill", "fullscreen", "zoom", "window"],
        "rectangle.inset.filled",
    ),
    (
        WindowAction::AlmostMaximize,
        "almost-maximize",
        "Almost Maximize Window",
        &["fill", "window"],
        "rectangle.center.inset.filled",
    ),
    (
        WindowAction::Center,
        "center",
        "Center Window",
        &["middle", "window"],
        "rectangle.center.inset.filled",
    ),
    (
        WindowAction::Restore,
        "restore",
        "Restore Window",
        &["undo", "reset", "window"],
        "arrow.uturn.backward",
    ),
    (
        WindowAction::NextDisplay,
        "next-display",
        "Move to Next Display",
        &["monitor", "screen", "window"],
        "arrow.right.square",
    ),
    (
        WindowAction::PreviousDisplay,
        "previous-display",
        "Move to Previous Display",
        &["monitor", "screen", "window"],
        "arrow.left.square",
    ),
];

pub(crate) fn action_for_id(id: &str) -> Option<WindowAction> {
    ACTIONS.iter().find(|a| a.1 == id).map(|a| a.0)
}

/// The window action a palette result (`window:<id>`) applies, for the shell's icons.
#[uniffi::export]
pub fn window_action_for_result(result_id: &str) -> Option<WindowAction> {
    action_for_id(result_id.strip_prefix("window:")?)
}

/// Moves and resizes the frontmost app's focused window. Remembers frames per window (for
/// `Restore` and for cycling repeated halves), so one instance is shared by the hotkeys and
/// the launcher.
pub(crate) struct WindowManager {
    config: Mutex<WindowConfig>,
    memory: Mutex<WindowMemory>,
}

impl WindowManager {
    pub fn new(config: WindowConfig) -> Self {
        Self {
            config: Mutex::new(config),
            memory: Mutex::new(WindowMemory::default()),
        }
    }

    pub fn set_config(&self, config: WindowConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Applies `action` to the focused window of the frontmost app. Fails with
    /// `PermissionDenied` without Accessibility. Main thread (hotkeys and palette both call
    /// from there).
    pub fn perform(&self, action: WindowAction) -> Result<(), CoreError> {
        // SAFETY: no arguments; reads the process's TCC state.
        if unsafe { ax::AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| other("window actions run on the main thread"))?;
        let screens = screens(mtm);
        if screens.is_empty() {
            return Err(other("no displays found"));
        }
        let (gap, almost_maximize) = {
            let config = self.config.lock().unwrap();
            (
                f64::from(config.gap),
                f64::from(config.almost_maximize_percent.clamp(10, 100)) / 100.0,
            )
        };

        let (pid, app, window) = focused_window()?;
        let before = frame(&window).map_err(ax_error)?;
        // Without a window number the window isn't remembered: cycling and Restore then
        // just don't apply to it.
        let key = window.window_id().map(|id| (pid, id));
        let memory = key
            .map(|key| self.memory.lock().unwrap().get(key))
            .unwrap_or_default();

        let plan = layout::plan(action, before, &screens, &memory, gap, almost_maximize).map_err(
            |error| {
                other(match error {
                    PlanError::NothingToRestore => "Jaycast hasn't moved this window",
                    PlanError::OneDisplay => "there's only one display",
                })
            },
        )?;

        // The memory lock isn't held across these calls: a slow app shouldn't block the
        // other caller.
        let after = with_enhanced_ui_off(&app, || apply(&window, &plan))?;

        if let Some(key) = key {
            let mut memory = memory;
            let memory = if plan.forget {
                None
            } else {
                memory.record(before, plan.snap, after);
                Some(memory)
            };
            self.memory.lock().unwrap().put(key, memory, is_open);
        }
        Ok(())
    }
}

fn other(message: &str) -> CoreError {
    CoreError::Other {
        message: message.into(),
    }
}

fn ax_error(error: ax::AxError) -> CoreError {
    match error {
        ax::API_DISABLED => CoreError::PermissionDenied,
        ax::CANNOT_COMPLETE => other("the app isn't responding"),
        error => CoreError::Other {
            message: format!("the window couldn't be moved (AXError {error})"),
        },
    }
}

/// The displays, primary first, in Accessibility coordinates.
fn screens(mtm: MainThreadMarker) -> Vec<Screen> {
    let screens: Vec<_> = NSScreen::screens(mtm).iter().collect();
    // The primary display (the one with the menu bar) sits at the AppKit origin; AppKit
    // lists it first, which is the fallback.
    let primary = screens
        .iter()
        .position(|s| {
            let f = s.frame();
            f.origin.x == 0.0 && f.origin.y == 0.0
        })
        .unwrap_or(0);
    let Some(primary_height) = screens.get(primary).map(|s| s.frame().size.height) else {
        return Vec::new();
    };
    let convert = |r: NSRect| {
        layout::flip(
            Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height),
            primary_height,
        )
    };
    let mut result: Vec<Screen> = screens
        .iter()
        .map(|s| Screen {
            frame: convert(s.frame()),
            visible: convert(s.visibleFrame()),
        })
        .collect();
    result.swap(0, primary);
    result
}

/// The frontmost app's pid, its element, and its focused (else main) window, if that can
/// be moved.
fn focused_window() -> Result<(i32, Element, Element), CoreError> {
    let pid = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
        .ok_or_else(|| other("no app is frontmost"))?;
    let app = Element::application(pid).ok_or_else(|| other("couldn't reach the frontmost app"))?;
    app.set_timeout(AX_TIMEOUT_SECONDS);
    let window = app
        .element(ax::FOCUSED_WINDOW)
        .or_else(|_| app.element(ax::MAIN_WINDOW))
        .map_err(|error| match error {
            ax::API_DISABLED => CoreError::PermissionDenied,
            ax::CANNOT_COMPLETE => other("the frontmost app isn't responding"),
            _ => other("the frontmost app has no focused window"),
        })?;
    window.set_timeout(AX_TIMEOUT_SECONDS);
    if window.bool(ax::FULL_SCREEN) == Some(true) {
        return Err(other("the window is full screen"));
    }
    if !window.is_settable(ax::POSITION) {
        return Err(other("the window can't be moved"));
    }
    Ok((pid, app, window))
}

fn frame(window: &Element) -> Result<Rect, ax::AxError> {
    let position = window.position()?;
    let size = window.size()?;
    Ok(Rect::new(position.x, position.y, size.width, size.height))
}

/// Runs `body` with the app's `AXEnhancedUserInterface` off, if it was on, and turns it back
/// on afterwards.
fn with_enhanced_ui_off<T>(app: &Element, body: impl FnOnce() -> T) -> T {
    let enhanced = app.bool(ax::ENHANCED_USER_INTERFACE) == Some(true);
    if enhanced {
        let _ = app.set_bool(ax::ENHANCED_USER_INTERFACE, false);
    }
    let result = body();
    if enhanced {
        let _ = app.set_bool(ax::ENHANCED_USER_INTERFACE, true);
    }
    result
}

/// Moves `window` to the plan's frame and returns the frame it ended up with.
fn apply(window: &Element, plan: &Plan) -> Result<Rect, CoreError> {
    let target = plan.frame;
    let size = CGSize::new(target.w, target.h);
    // Size first so the window fits where it's going; then position; then size again, since
    // an app can clamp the first resize to the display the window was still on. A window
    // that can't be resized just keeps its size; that's handled below, not an error.
    let resizable = window.is_settable(ax::SIZE);
    if resizable {
        let _ = window.set_size(size);
    }
    window
        .set_position(CGPoint::new(target.x, target.y))
        .map_err(ax_error)?;
    if resizable {
        let _ = window.set_size(size);
    }

    let actual = frame(window).map_err(ax_error)?;
    if (actual.w - target.w).abs() <= 1.0 && (actual.h - target.h).abs() <= 1.0 {
        return Ok(actual);
    }
    // It kept (some of) its own size: place what it has within the target.
    let placed = layout::place_fixed_size(plan, (actual.w, actual.h));
    if !placed.approx_eq(&actual) {
        window
            .set_position(CGPoint::new(placed.x, placed.y))
            .map_err(ax_error)?;
    }
    frame(window).map_err(ax_error)
}

/// Whether a remembered window still exists: its app is running and the window server still
/// knows its number.
fn is_open((pid, id): WindowKey) -> bool {
    if NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_none() {
        return false;
    }
    match CGWindowListCopyWindowInfo(CGWindowListOption::OptionIncludingWindow, id) {
        Some(windows) => windows.count() > 0,
        // Can't tell: keep it (the size cap still bounds memory).
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_round_trip() {
        for &(action, id, ..) in ACTIONS {
            assert_eq!(action_for_id(id), Some(action));
        }
        assert_eq!(action_for_id("nope"), None);
    }

    /// Read-only: prints the displays and the frontmost app's focused window. Nothing is
    /// moved. `cargo test -p jaycast-core windows::tests::probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe() {
        // SAFETY: not really the main thread (libtest runs every test on its own thread), but
        // nothing else in this process touches AppKit and only screen geometry is read. A
        // manual diagnostic only; never relied on by the app.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let screens = screens(mtm);
        for (i, screen) in screens.iter().enumerate() {
            println!(
                "screen {i}: frame {:?}, visible {:?}",
                screen.frame, screen.visible
            );
        }
        println!("display order: {:?}", layout::display_order(&screens));
        // SAFETY: no arguments; reads the process's TCC state.
        if unsafe { ax::AXIsProcessTrusted() } == 0 {
            println!("not trusted for Accessibility; grant it to the terminal running this");
            return;
        }
        match focused_window() {
            Ok((pid, _, window)) => {
                let frame = frame(&window);
                println!(
                    "pid {pid}, window {:?}, frame {frame:?}",
                    window.window_id()
                );
                if let Ok(frame) = frame {
                    println!("on screen {}", layout::screen_for(&frame, &screens));
                    let plan = layout::plan(
                        WindowAction::LeftHalf,
                        frame,
                        &screens,
                        &layout::Memory::default(),
                        0.0,
                        0.9,
                    );
                    println!("left half would be {plan:?}");
                }
            }
            Err(error) => println!("no focused window: {error}"),
        }
    }
}
