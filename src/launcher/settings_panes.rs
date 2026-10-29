//! System Settings panes listed in search, so "privacy" opens Privacy & Security directly.
//!
//! Each pane opens through the `x-apple.systempreferences:` URL scheme, addressed by its
//! settings extension's bundle id (which System Settings resolves on every macOS since
//! Ventura). The privacy sections use the legacy pane id with an anchor, the only way to
//! open one directly.
//!
//! Icons come from the pane's extension bundle in `/System/Library/ExtensionKit/Extensions`:
//! Launch Services hands back the same icon System Settings shows in its sidebar. A pane
//! whose bundle has moved falls back to System Settings' own icon.

use std::path::Path;

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use crate::error::CoreError;

/// A System Settings pane or section, listed alongside apps in search.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SettingsPane {
    /// Stable id (`privacy-security`, …); search results use `settings:<id>`.
    pub id: &'static str,
    pub title: &'static str,
    /// The pane it lives in, for sections ("Privacy & Security"); `None` for panes.
    pub parent: Option<&'static str>,
    pub keywords: &'static [&'static str],
    /// Appended to `x-apple.systempreferences:`.
    pub target: &'static str,
    /// The settings extension bundle the icon comes from.
    pub bundle: &'static str,
}

const EXTENSIONS_DIR: &str = "/System/Library/ExtensionKit/Extensions";
const SYSTEM_SETTINGS: &str = "/System/Applications/System Settings.app";
const PRIVACY: &str = "SecurityPrivacyExtension.appex";

const fn pane(
    id: &'static str,
    title: &'static str,
    keywords: &'static [&'static str],
    target: &'static str,
    bundle: &'static str,
) -> SettingsPane {
    SettingsPane {
        id,
        title,
        parent: None,
        keywords,
        target,
        bundle,
    }
}

const fn privacy(
    id: &'static str,
    title: &'static str,
    keywords: &'static [&'static str],
    anchor: &'static str,
) -> SettingsPane {
    SettingsPane {
        id,
        title,
        parent: Some("Privacy & Security"),
        keywords,
        target: anchor,
        bundle: PRIVACY,
    }
}

/// In sidebar order, then General's panes, then the privacy sections.
const PANES: &[SettingsPane] = &[
    pane(
        "apple-account",
        "Apple Account",
        &["apple id", "account", "sign in"],
        "com.apple.systempreferences.AppleIDSettings",
        "AppleIDSettings.appex",
    ),
    pane(
        "icloud",
        "iCloud",
        &["icloud drive", "sync"],
        "com.apple.systempreferences.AppleIDSettings:icloud",
        "AppleIDSettings.appex",
    ),
    pane(
        "family",
        "Family",
        &["family sharing", "parental"],
        "com.apple.Family-Settings.extension",
        "FamilySettings.appex",
    ),
    pane(
        "wi-fi",
        "Wi-Fi",
        &["wifi", "wireless", "internet", "network"],
        "com.apple.wifi-settings-extension",
        "Wi-Fi.appex",
    ),
    pane(
        "network",
        "Network",
        &["ethernet", "firewall", "proxy", "dns", "internet"],
        "com.apple.Network-Settings.extension",
        "Network.appex",
    ),
    pane(
        "vpn",
        "VPN",
        &["virtual private network", "tunnel"],
        "com.apple.NetworkExtensionSettingsUI.NESettingsUIExtension",
        "VPN.appex",
    ),
    pane(
        "bluetooth",
        "Bluetooth",
        &["airpods", "headphones", "pair", "devices"],
        "com.apple.BluetoothSettings",
        "Bluetooth.appex",
    ),
    pane(
        "battery",
        "Battery",
        &["power", "energy", "low power mode", "charging"],
        "com.apple.Battery-Settings.extension",
        "PowerPreferences.appex",
    ),
    pane(
        "general",
        "General",
        &[],
        "com.apple.systempreferences.GeneralSettings",
        "GeneralSettings.appex",
    ),
    pane(
        "accessibility",
        "Accessibility",
        &["voiceover", "zoom", "universal access", "reduce motion"],
        "com.apple.Accessibility-Settings.extension",
        "AccessibilitySettingsExtension.appex",
    ),
    pane(
        "appearance",
        "Appearance",
        &[
            "dark mode",
            "light mode",
            "accent colour",
            "accent color",
            "theme",
        ],
        "com.apple.Appearance-Settings.extension",
        "Appearance.appex",
    ),
    pane(
        "siri",
        "Apple Intelligence & Siri",
        &["siri", "assistant", "ai"],
        "com.apple.Siri-Settings.extension",
        "SiriPreferenceExtension.appex",
    ),
    pane(
        "menu-bar",
        "Menu Bar",
        &["control center", "control centre", "status bar"],
        "com.apple.ControlCenter-Settings.extension",
        "ControlCenterSettings.appex",
    ),
    pane(
        "desktop-dock",
        "Desktop & Dock",
        &[
            "dock",
            "stage manager",
            "mission control",
            "hot corners",
            "widgets",
        ],
        "com.apple.Desktop-Settings.extension",
        "DesktopSettings.appex",
    ),
    pane(
        "displays",
        "Displays",
        &[
            "monitor",
            "resolution",
            "night shift",
            "brightness",
            "screen",
        ],
        "com.apple.Displays-Settings.extension",
        "DisplaysExt.appex",
    ),
    pane(
        "spotlight",
        "Spotlight",
        &["search"],
        "com.apple.Spotlight-Settings.extension",
        "SpotlightPreferenceExtension.appex",
    ),
    pane(
        "wallpaper",
        "Wallpaper",
        &["background", "desktop picture", "screen saver"],
        "com.apple.Wallpaper-Settings.extension",
        "Wallpaper.appex",
    ),
    pane(
        "notifications",
        "Notifications",
        &["alerts", "banners"],
        "com.apple.Notifications-Settings.extension",
        "NotificationsSettings.appex",
    ),
    pane(
        "sound",
        "Sound",
        &[
            "audio",
            "volume",
            "output",
            "input",
            "speakers",
            "alert sound",
        ],
        "com.apple.Sound-Settings.extension",
        "Sound.appex",
    ),
    pane(
        "focus",
        "Focus",
        &["do not disturb", "dnd"],
        "com.apple.Focus-Settings.extension",
        "FocusSettingsExtension.appex",
    ),
    pane(
        "screen-time",
        "Screen Time",
        &["app limits", "downtime", "parental"],
        "com.apple.Screen-Time-Settings.extension",
        "ScreenTimePreferencesExtension.appex",
    ),
    pane(
        "lock-screen",
        "Lock Screen",
        &["login window", "require password"],
        "com.apple.Lock-Screen-Settings.extension",
        "LockScreen.appex",
    ),
    pane(
        "privacy-security",
        "Privacy & Security",
        &[
            "privacy",
            "security",
            "permissions",
            "filevault",
            "gatekeeper",
        ],
        "com.apple.settings.PrivacySecurity.extension",
        PRIVACY,
    ),
    pane(
        "touch-id-password",
        "Touch ID & Password",
        &["fingerprint", "password", "apple watch unlock"],
        "com.apple.Touch-ID-Settings.extension",
        "Touch ID & Password.appex",
    ),
    pane(
        "users-groups",
        "Users & Groups",
        &["accounts", "guest", "login"],
        "com.apple.Users-Groups-Settings.extension",
        "UsersGroups.appex",
    ),
    pane(
        "internet-accounts",
        "Internet Accounts",
        &["mail", "email", "google", "exchange", "calendar accounts"],
        "com.apple.Internet-Accounts-Settings.extension",
        "InternetAccountsSettingsExtension.appex",
    ),
    pane(
        "game-center",
        "Game Center",
        &["games"],
        "com.apple.Game-Center-Settings.extension",
        "GameCenterMacOSSettingsExtension.appex",
    ),
    pane(
        "wallet",
        "Wallet & Apple Pay",
        &["apple pay", "cards", "payment"],
        "com.apple.WalletSettingsExtension",
        "WalletSettingsExtension.appex",
    ),
    pane(
        "keyboard",
        "Keyboard",
        &[
            "shortcuts",
            "input sources",
            "dictation",
            "key repeat",
            "text replacement",
        ],
        "com.apple.Keyboard-Settings.extension",
        "KeyboardSettings.appex",
    ),
    pane(
        "mouse",
        "Mouse",
        &["pointer", "tracking speed", "scrolling"],
        "com.apple.Mouse-Settings.extension",
        "MouseExtension.appex",
    ),
    pane(
        "trackpad",
        "Trackpad",
        &["gestures", "tap to click", "force click"],
        "com.apple.Trackpad-Settings.extension",
        "TrackpadExtension.appex",
    ),
    pane(
        "game-controllers",
        "Game Controllers",
        &["gamepad", "joystick", "controller"],
        "com.apple.Game-Controller-Settings.extension",
        "GameControllerMacSettings.appex",
    ),
    pane(
        "printers-scanners",
        "Printers & Scanners",
        &["printer", "scanner", "print"],
        "com.apple.Print-Scan-Settings.extension",
        "PrinterScannerSettings.appex",
    ),
    // General's panes.
    pane(
        "about",
        "About",
        &[
            "about this mac",
            "serial number",
            "macos version",
            "system information",
        ],
        "com.apple.SystemProfiler.AboutExtension",
        "AboutExtension.appex",
    ),
    pane(
        "software-update",
        "Software Update",
        &["update", "upgrade", "macos"],
        "com.apple.Software-Update-Settings.extension",
        "SoftwareUpdateSettingsExtension.appex",
    ),
    pane(
        "storage",
        "Storage",
        &["disk space", "free space"],
        "com.apple.settings.Storage",
        "Storage.appex",
    ),
    pane(
        "applecare",
        "AppleCare & Warranty",
        &["warranty", "coverage"],
        "com.apple.Coverage-Settings.extension",
        "CoverageSettings.appex",
    ),
    pane(
        "airdrop",
        "AirDrop & Continuity",
        &["airdrop", "handoff", "continuity", "airplay receiver"],
        "com.apple.AirDrop-Handoff-Settings.extension",
        "AirDropHandoffExtension.appex",
    ),
    pane(
        "login-items",
        "Login Items & Extensions",
        &[
            "startup",
            "launch at login",
            "background items",
            "extensions",
        ],
        "com.apple.LoginItems-Settings.extension",
        "LoginItems.appex",
    ),
    pane(
        "language-region",
        "Language & Region",
        &["locale", "language", "region", "date format", "temperature"],
        "com.apple.Localization-Settings.extension",
        "Localization.appex",
    ),
    pane(
        "date-time",
        "Date & Time",
        &["clock", "time zone", "timezone"],
        "com.apple.Date-Time-Settings.extension",
        "DateAndTime Extension.appex",
    ),
    pane(
        "sharing",
        "Sharing",
        &[
            "computer name",
            "hostname",
            "file sharing",
            "screen sharing",
            "remote login",
            "ssh",
        ],
        "com.apple.Sharing-Settings.extension",
        "Sharing.appex",
    ),
    pane(
        "time-machine",
        "Time Machine",
        &["backup"],
        "com.apple.Time-Machine-Settings.extension",
        "TimeMachineSettings.appex",
    ),
    pane(
        "device-management",
        "Device Management",
        &["profiles", "mdm"],
        "com.apple.Profiles-Settings.extension",
        "ProfilesSettingsExt.appex",
    ),
    pane(
        "startup-disk",
        "Startup Disk",
        &["boot"],
        "com.apple.Startup-Disk-Settings.extension",
        "StartupDisk.appex",
    ),
    pane(
        "transfer-reset",
        "Transfer or Reset",
        &["erase", "factory reset", "migration"],
        "com.apple.Transfer-Reset-Settings.extension",
        "TransferResetExtension.appex",
    ),
    // Privacy & Security's sections, mostly the permissions apps ask for.
    privacy(
        "privacy-location",
        "Location Services",
        &["location", "gps"],
        "com.apple.preference.security?Privacy_LocationServices",
    ),
    privacy(
        "privacy-camera",
        "Camera",
        &["webcam"],
        "com.apple.preference.security?Privacy_Camera",
    ),
    privacy(
        "privacy-microphone",
        "Microphone",
        &["mic"],
        "com.apple.preference.security?Privacy_Microphone",
    ),
    privacy(
        "privacy-accessibility",
        "Accessibility Permissions",
        &["accessibility access", "control your computer"],
        "com.apple.preference.security?Privacy_Accessibility",
    ),
    privacy(
        "privacy-input-monitoring",
        "Input Monitoring",
        &["keylogging", "keyboard access"],
        "com.apple.preference.security?Privacy_ListenEvent",
    ),
    privacy(
        "privacy-full-disk-access",
        "Full Disk Access",
        &["fda", "disk access", "files"],
        "com.apple.preference.security?Privacy_AllFiles",
    ),
    privacy(
        "privacy-files-folders",
        "Files & Folders",
        &["folder access", "documents access"],
        "com.apple.preference.security?Privacy_FilesAndFolders",
    ),
    privacy(
        "privacy-screen-recording",
        "Screen & System Audio Recording",
        &[
            "screen recording",
            "screen capture",
            "screenshot permission",
        ],
        "com.apple.preference.security?Privacy_ScreenCapture",
    ),
    privacy(
        "privacy-automation",
        "Automation",
        &["apple events", "applescript permission"],
        "com.apple.preference.security?Privacy_Automation",
    ),
];

pub(crate) fn panes() -> &'static [SettingsPane] {
    PANES
}

impl SettingsPane {
    /// The bundle to draw the pane's icon from: its settings extension, or System Settings
    /// if that has moved.
    pub fn icon_path(&self) -> String {
        let path = Path::new(EXTENSIONS_DIR).join(self.bundle);
        if path.exists() {
            path.to_string_lossy().into_owned()
        } else {
            SYSTEM_SETTINGS.to_owned()
        }
    }

    fn url(&self) -> String {
        format!("x-apple.systempreferences:{}", self.target)
    }
}

/// Opens a pane by id in System Settings.
pub(crate) fn open(id: &str) -> Result<(), CoreError> {
    let pane = PANES
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| CoreError::Other {
            message: format!("unknown settings pane {id}"),
        })?;
    let url = pane.url();
    autoreleasepool(|_| {
        let opened = NSURL::URLWithString(&NSString::from_str(&url))
            .is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
        if opened {
            Ok(())
        } else {
            Err(CoreError::Other {
                message: format!("Couldn't open {}", pane.title),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_kebab_case() {
        let mut seen = HashSet::new();
        for pane in panes() {
            assert!(seen.insert(pane.id), "duplicate id {}", pane.id);
            assert!(
                pane.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{} isn't kebab-case",
                pane.id
            );
            assert!(!pane.title.is_empty() && !pane.target.is_empty());
        }
    }

    #[test]
    fn urls_parse() {
        for pane in panes() {
            let url = pane.url();
            assert!(
                autoreleasepool(|_| NSURL::URLWithString(&NSString::from_str(&url)).is_some()),
                "{url}"
            );
        }
    }

    /// Bundles move between macOS versions (the app falls back to System Settings' icon),
    /// so this only holds on the macOS the list was written against.
    /// Run with `cargo test -p jaycast -- --ignored icon_bundles_exist_on_this_macos`.
    #[test]
    #[ignore = "depends on the macOS version"]
    fn icon_bundles_exist_on_this_macos() {
        let missing: Vec<_> = panes()
            .iter()
            .filter(|pane| !Path::new(EXTENSIONS_DIR).join(pane.bundle).exists())
            .map(|pane| format!("{} ({})", pane.id, pane.bundle))
            .collect();
        assert!(missing.is_empty(), "no bundle for: {}", missing.join(", "));
    }

    #[test]
    fn unknown_pane_is_an_error() {
        assert!(open("definitely-not-a-pane").is_err());
    }
}
