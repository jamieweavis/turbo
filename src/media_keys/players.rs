//! The music apps media keys can go to, and sending them commands as Apple Events.
//!
//! Commands are raw Apple Events (the codes from each app's scripting dictionary, `sdef`)
//! rather than AppleScript: `NSAppleScript` is main-thread only, and an Apple Event can be
//! sent from the media worker thread, where waiting on a slow app or the Automation prompt
//! blocks nothing else. Forwarding the key itself to the app wouldn't work: modern players
//! get media keys through MediaRemote, not as events.

use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::{ClassType, msg_send};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventSendOptions, NSString};

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

/// A supported player: its bundle id and Apple Event codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Player {
    pub bundle_id: &'static str,
    pub name: &'static str,
    /// The event class (suite code) of its playback commands.
    suite: &'static [u8; 4],
    play_pause: &'static [u8; 4],
    next: &'static [u8; 4],
    previous: &'static [u8; 4],
    /// The distributed notification it posts when playback starts, stops or changes track,
    /// with the state under `Player State`.
    pub state_notification: Option<&'static str>,
}

impl Player {
    fn event_id(&self, command: MediaCommand) -> &'static [u8; 4] {
        match command {
            MediaCommand::PlayPause => self.play_pause,
            MediaCommand::Next => self.next,
            MediaCommand::Previous => self.previous,
        }
    }
}

/// In the order they're offered, and tried when nothing has played yet.
pub(crate) const PLAYERS: &[Player] = &[
    Player {
        bundle_id: "com.spotify.client",
        name: "Spotify",
        suite: b"spfy",
        play_pause: b"PlPs",
        next: b"Next",
        previous: b"Prev",
        state_notification: Some("com.spotify.client.PlaybackStateChanged"),
    },
    Player {
        bundle_id: "com.apple.Music",
        name: "Music",
        suite: b"hook",
        play_pause: b"PlPs",
        next: b"Next",
        // `back track`: back to the start of the track, or the previous one if already
        // there, which is what the previous key does in Music itself.
        previous: b"Back",
        state_notification: Some("com.apple.Music.playerInfo"),
    },
    Player {
        bundle_id: "org.videolan.vlc",
        name: "VLC",
        suite: b"VLC#",
        // VLC's `play` toggles.
        play_pause: b"VLC1",
        next: b"VLC4",
        previous: b"VLC3",
        state_notification: None,
    },
];

pub(crate) fn player(bundle_id: &str) -> Option<&'static Player> {
    PLAYERS.iter().find(|p| p.bundle_id == bundle_id)
}

fn four_cc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// Whether Backpack may send Apple Events to an app (System Settings › Privacy & Security ›
/// Automation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AutomationAccess {
    Granted,
    /// Not asked yet: macOS asks the first time a key goes to the app.
    NotDetermined,
    Denied,
    /// macOS can only say while the app is running.
    NotRunning,
}

/// `AEDesc` (AEDataModel.h).
#[repr(C)]
struct AEDesc {
    descriptor_type: u32,
    data_handle: *mut c_void,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn AECreateDesc(type_code: u32, data: *const c_void, size: isize, result: *mut AEDesc) -> i16;
    fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
    fn AEDeterminePermissionToAutomateTarget(
        target: *const AEDesc,
        event_class: u32,
        event_id: u32,
        ask_user_if_needed: u8,
    ) -> i32;
}

const TYPE_KERNEL_PROCESS_ID: u32 = u32::from_be_bytes(*b"kpid");
const NO_ERR: i32 = 0;
const PROC_NOT_FOUND: i32 = -600;
const ERR_AE_EVENT_NOT_PERMITTED: i32 = -1743;
const ERR_AE_EVENT_WOULD_REQUIRE_USER_CONSENT: i32 = -1744;

/// Checks (and with `ask`, asks for) permission to send `player` its playback commands.
/// With `ask` it blocks while the prompt is up, so never call it on the main thread.
///
/// The target is the running player's process id. Addressed by bundle id instead,
/// `AEDeterminePermissionToAutomateTarget` never returns (seen on macOS 26 with Spotify running,
/// even without `ask`), and macOS can only answer for a running app anyway.
pub(crate) fn automation_access(player: &Player, ask: bool) -> AutomationAccess {
    let Some(pid) = crate::launcher::running::pid(player.bundle_id) else {
        return AutomationAccess::NotRunning;
    };
    let mut target = AEDesc {
        descriptor_type: 0,
        data_handle: std::ptr::null_mut(),
    };
    // SAFETY: `pid` is valid for its size; `target` is written by AECreateDesc and
    // disposed of below.
    let status = unsafe {
        let created = AECreateDesc(
            TYPE_KERNEL_PROCESS_ID,
            (&raw const pid).cast(),
            size_of::<i32>() as isize,
            &mut target,
        );
        if created != 0 {
            log::warn!(
                "couldn't make an Apple Event target for {}",
                player.bundle_id
            );
            return AutomationAccess::NotDetermined;
        }
        let status = AEDeterminePermissionToAutomateTarget(
            &target,
            four_cc(player.suite),
            four_cc(player.play_pause),
            u8::from(ask),
        );
        AEDisposeDesc(&mut target);
        status
    };
    match status {
        NO_ERR => AutomationAccess::Granted,
        ERR_AE_EVENT_WOULD_REQUIRE_USER_CONSENT => AutomationAccess::NotDetermined,
        ERR_AE_EVENT_NOT_PERMITTED => AutomationAccess::Denied,
        PROC_NOT_FOUND => AutomationAccess::NotRunning,
        other => {
            log::warn!(
                "automation permission for {} failed: {other}",
                player.bundle_id
            );
            AutomationAccess::NotDetermined
        }
    }
}

/// Sends `command` to the running `player`, waiting briefly for it to be handled. Blocks:
/// call it from the media worker.
pub(crate) fn send(player: &Player, command: MediaCommand) -> Result<(), CoreError> {
    const AUTO_GENERATE_RETURN_ID: i16 = -1;
    const ANY_TRANSACTION_ID: i32 = 0;
    const TIMEOUT_SECONDS: f64 = 3.0;

    let target = NSAppleEventDescriptor::descriptorWithBundleIdentifier(&NSString::from_str(
        player.bundle_id,
    ));
    // The typed binding needs objc2-core-services just for the integer typedefs.
    // SAFETY: the selector takes (FourCharCode, FourCharCode, NSAppleEventDescriptor *,
    // AEReturnID, AETransactionID) and returns an autoreleased descriptor.
    let event: Retained<NSAppleEventDescriptor> = unsafe {
        msg_send![
            NSAppleEventDescriptor::class(),
            appleEventWithEventClass: four_cc(player.suite),
            eventID: four_cc(player.event_id(command)),
            targetDescriptor: &*target,
            returnID: AUTO_GENERATE_RETURN_ID,
            transactionID: ANY_TRANSACTION_ID
        ]
    };
    let options = NSAppleEventSendOptions::WaitForReply | NSAppleEventSendOptions::NeverInteract;
    event
        .sendEventWithOptions_timeout_error(options, TIMEOUT_SECONDS)
        .map(|_| ())
        .map_err(|error| CoreError::Other {
            message: format!(
                "couldn't send {command:?} to {}: {}",
                player.name,
                error.localizedDescription()
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn players_are_unique_with_four_char_codes() {
        let mut seen = HashSet::new();
        for p in PLAYERS {
            assert!(seen.insert(p.bundle_id), "duplicate {}", p.bundle_id);
            assert_eq!(player(p.bundle_id), Some(p));
        }
        assert_eq!(player("com.example.nope"), None);
    }

    #[test]
    fn codes_are_big_endian() {
        assert_eq!(four_cc(b"hook"), 0x686F_6F6B);
        assert_eq!(
            four_cc(PLAYERS[0].event_id(MediaCommand::PlayPause)),
            u32::from_be_bytes(*b"PlPs")
        );
    }

    #[test]
    fn music_previous_is_back_track() {
        let music = player("com.apple.Music").unwrap();
        assert_eq!(music.event_id(MediaCommand::Previous), b"Back");
    }

    /// Sends play/pause to Spotify for real (and may show the Automation prompt). Run with
    /// `cargo test send_play_pause -- --ignored --nocapture` while Spotify is open.
    #[test]
    #[ignore]
    fn send_play_pause_to_spotify() {
        let spotify = player("com.spotify.client").unwrap();
        eprintln!("access: {:?}", automation_access(spotify, true));
        eprintln!("send: {:?}", send(spotify, MediaCommand::PlayPause));
    }
}
