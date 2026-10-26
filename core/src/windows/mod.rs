//! STUB — to be implemented: window management (snap, maximise, centre, restore, move
//! between displays) through the Accessibility API.

#![allow(dead_code)]

use std::sync::Mutex;

use crate::config::{WindowAction, WindowConfig};
use crate::error::CoreError;

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
        "rectangle.dashed",
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

/// Moves and resizes the frontmost app's focused window. Remembers frames per window (for
/// `Restore` and for cycling repeated halves), so one instance is shared by the hotkeys and
/// the launcher.
pub(crate) struct WindowManager {
    config: Mutex<WindowConfig>,
}

impl WindowManager {
    pub fn new(config: WindowConfig) -> Self {
        Self {
            config: Mutex::new(config),
        }
    }

    pub fn set_config(&self, config: WindowConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Applies `action` to the focused window of the frontmost app. Fails with
    /// `PermissionDenied` without Accessibility. Main thread (hotkeys and palette both call
    /// from there).
    pub fn perform(&self, action: WindowAction) -> Result<(), CoreError> {
        let _ = action;
        Err(CoreError::Other {
            message: "window management is not implemented yet".into(),
        })
    }
}
