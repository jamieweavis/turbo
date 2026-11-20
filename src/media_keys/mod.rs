//! Media keys: the keyboard's play/pause, next and previous keys go to the music app you
//! choose (or the one you used last), so they never launch Music by surprise.
//!
//! - `tap`: catches the keys (needs Accessibility) and decides, per press, where it goes.
//! - `players`: the supported apps and sending them commands as Apple Events (needs
//!   Automation consent per app, which macOS asks for the first time).
//! - `now_playing`: tracks the last player used, from the players' own broadcasts.
//!
//! Commands are sent on one worker thread, so a slow app, a launch or the Automation prompt
//! never holds up the tap or the main thread.
//!
//! Headphone buttons and Control Center's Now Playing don't pass through event taps, so
//! they keep macOS's own routing.

mod now_playing;
mod players;
mod tap;

use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;
use parking_lot::Mutex;

use crate::config::{MediaKeysConfig, MediaTarget, WhenNotRunning};
use crate::error::CoreError;
use crate::launcher::running;
pub use players::{AutomationAccess, MediaCommand};
use players::{PLAYERS, Player};
use tap::MediaKeyTap;

/// A supported player that's installed, for the settings picker.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaPlayer {
    pub bundle_id: String,
    pub name: String,
    /// The app bundle, to draw its icon.
    pub path: String,
}

/// How long a launched player gets to finish launching before the key is dropped.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(20);
/// After a player says it has finished launching, it still needs a moment before it acts
/// on commands (Spotify ignores play/pause until its UI has loaded).
const LAUNCH_SETTLE: Duration = Duration::from_millis(1500);

pub(crate) struct MediaKeys {
    shared: Arc<Shared>,
    tap: MediaKeyTap,
}

/// What the tap thread and the API share.
struct Shared {
    /// Read once per key press, which is rare enough for a lock.
    config: Mutex<MediaKeysConfig>,
    /// The worker's queue, started on first use.
    jobs: Mutex<Option<Sender<Job>>>,
}

struct Job {
    player: &'static Player,
    command: MediaCommand,
    launch: bool,
}

/// Where a press (or a launcher command) goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Send it to this running player.
    Send(&'static Player),
    /// Open this player, then send it.
    Launch(&'static Player),
    /// Swallow it.
    Ignore,
    /// Leave the key to macOS.
    PassThrough,
}

impl MediaKeys {
    pub fn new(config: MediaKeysConfig) -> Self {
        now_playing::listen(config.enabled);
        let shared = Arc::new(Shared {
            config: Mutex::new(config),
            jobs: Mutex::new(None),
        });
        Self {
            tap: MediaKeyTap::new(Arc::clone(&shared)),
            shared,
        }
    }

    pub fn set_config(&self, config: MediaKeysConfig) -> Result<(), CoreError> {
        now_playing::listen(config.enabled);
        *self.shared.config.lock() = config;
        self.tap.refresh()
    }

    /// Starts the key tap (while the feature is on). Fails with `PermissionDenied` without
    /// Accessibility.
    pub fn start(&self) -> Result<(), CoreError> {
        self.tap.start()
    }

    pub fn stop(&self) {
        self.tap.stop();
    }

    /// A launcher command or hotkey: like a key press, except that with nothing running it
    /// opens the chosen player whatever `when_not_running` says, since you asked for it.
    pub fn perform(&self, command: MediaCommand) -> Result<(), CoreError> {
        match self.shared.route(false) {
            Route::Send(player) => self.shared.dispatch(player, command, false),
            Route::Launch(player) => self.shared.dispatch(player, command, true),
            Route::Ignore | Route::PassThrough => Err(CoreError::Other {
                message: "no supported music app is installed".into(),
            }),
        }
    }

    /// The supported players that are installed, in menu order.
    pub fn players(&self) -> Vec<MediaPlayer> {
        PLAYERS
            .iter()
            .filter_map(|player| {
                Some(MediaPlayer {
                    bundle_id: player.bundle_id.into(),
                    name: player.name.into(),
                    path: app_path(player.bundle_id)?,
                })
            })
            .collect()
    }

    /// Whether Turbo may control the player, without asking. Can block indefinitely (it waits
    /// on TCC and the player even without a prompt): call it off the main thread.
    pub fn access(&self, bundle_id: &str) -> AutomationAccess {
        players::player(bundle_id).map_or(AutomationAccess::NotDetermined, |player| {
            players::automation_access(player, false)
        })
    }

    /// Shows macOS's Automation prompt for the player if it hasn't been answered (the
    /// player must be running). Blocks until it's answered: call it off the main thread.
    pub fn request_access(&self, bundle_id: &str) -> AutomationAccess {
        players::player(bundle_id).map_or(AutomationAccess::NotDetermined, |player| {
            players::automation_access(player, true)
        })
    }
}

impl Drop for MediaKeys {
    fn drop(&mut self) {
        now_playing::listen(false);
    }
}

impl Shared {
    fn route(&self, from_key: bool) -> Route {
        let config = self.config.lock().clone();
        route(
            &config,
            now_playing::last_active(),
            from_key,
            |id| app_path(id).is_some(),
            running::is_running,
        )
    }

    /// Queues the command for the worker; never blocks.
    fn dispatch(
        &self,
        player: &'static Player,
        command: MediaCommand,
        launch: bool,
    ) -> Result<(), CoreError> {
        let mut jobs = self.jobs.lock();
        let job = Job {
            player,
            command,
            launch,
        };
        let job = match jobs.as_ref() {
            Some(sender) => match sender.send(job) {
                Ok(()) => return Ok(()),
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        // Not started yet (or the worker died): start it and hand it the job.
        let (sender, receiver) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("turbo-media".into())
            .spawn(move || {
                for job in receiver {
                    run(&job);
                }
            })?;
        let _ = sender.send(job);
        *jobs = Some(sender);
        Ok(())
    }
}

/// Decides where a press goes. `installed` and `running` take bundle ids; `installed` is
/// only asked when the chosen player isn't running, since it's the slower check.
fn route(
    config: &MediaKeysConfig,
    last_active: Option<&str>,
    from_key: bool,
    installed: impl Fn(&str) -> bool,
    running: impl Fn(&str) -> bool,
) -> Route {
    let chosen = players::player(&config.player);
    let running_player = |id: Option<&str>| {
        id.and_then(players::player)
            .filter(|player| running(player.bundle_id))
    };
    let target = match config.target {
        MediaTarget::Fixed => running_player(Some(&config.player)),
        MediaTarget::LastActive => running_player(last_active)
            .or_else(|| running_player(Some(&config.player)))
            .or_else(|| PLAYERS.iter().find(|p| running(p.bundle_id))),
    };
    if let Some(player) = target {
        return Route::Send(player);
    }
    // Nothing to send to. The chosen player, or the first installed one if it's gone.
    let launchable = || {
        chosen
            .filter(|p| installed(p.bundle_id))
            .or_else(|| PLAYERS.iter().find(|p| installed(p.bundle_id)))
    };
    let policy = if from_key {
        config.when_not_running
    } else {
        WhenNotRunning::Launch
    };
    match policy {
        WhenNotRunning::Launch => launchable().map_or(
            if from_key {
                Route::PassThrough
            } else {
                Route::Ignore
            },
            Route::Launch,
        ),
        WhenNotRunning::Ignore => Route::Ignore,
        WhenNotRunning::PassThrough => Route::PassThrough,
    }
}

/// The worker's handling of one command: launch if needed, check consent, send.
fn run(job: &Job) {
    let player = job.player;
    if job.launch {
        log::info!("opening {} for a media key", player.name);
        if let Err(error) = launch(player) {
            log::warn!("{error}");
            return;
        }
    }
    match players::automation_access(player, true) {
        AutomationAccess::Denied => {
            log::warn!(
                "not allowed to control {} (System Settings › Privacy & Security › Automation)",
                player.name
            );
            return;
        }
        AutomationAccess::NotRunning => {
            log::info!("{} quit before the media key reached it", player.name);
            return;
        }
        AutomationAccess::Granted | AutomationAccess::NotDetermined => {}
    }
    match players::send(player, job.command) {
        Ok(()) => {
            log::debug!("sent {:?} to {}", job.command, player.name);
            now_playing::mark_active(player);
        }
        Err(error) => log::warn!("{error}"),
    }
}

/// Opens the player in the background and waits until it can take commands.
fn launch(player: &Player) -> Result<(), CoreError> {
    let status = Command::new("/usr/bin/open")
        .args(["-g", "-b", player.bundle_id])
        .status()?;
    if !status.success() {
        return Err(CoreError::Other {
            message: format!("couldn't open {} ({status})", player.name),
        });
    }
    let started = Instant::now();
    while !finished_launching(player.bundle_id) {
        if started.elapsed() > LAUNCH_TIMEOUT {
            return Err(CoreError::Other {
                message: format!("{} didn't finish launching", player.name),
            });
        }
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(LAUNCH_SETTLE);
    Ok(())
}

fn finished_launching(bundle_id: &str) -> bool {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .iter()
        .any(|app| app.isFinishedLaunching() && !app.isTerminated())
}

fn app_path(bundle_id: &str) -> Option<String> {
    let url = NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
    Some(url.path()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY: &str = "com.spotify.client";
    const MUSIC: &str = "com.apple.Music";
    const VLC: &str = "org.videolan.vlc";

    fn config(target: MediaTarget, player: &str, when: WhenNotRunning) -> MediaKeysConfig {
        MediaKeysConfig {
            enabled: true,
            target,
            player: player.into(),
            when_not_running: when,
        }
    }

    fn id(route: Route) -> Option<(&'static str, bool)> {
        match route {
            Route::Send(p) => Some((p.bundle_id, false)),
            Route::Launch(p) => Some((p.bundle_id, true)),
            _ => None,
        }
    }

    fn in_list<'a>(ids: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |id| ids.contains(&id)
    }

    #[test]
    fn last_active_wins_while_running() {
        let c = config(MediaTarget::LastActive, SPOTIFY, WhenNotRunning::Launch);
        let all = [SPOTIFY, MUSIC, VLC];
        let r = route(
            &c,
            Some(MUSIC),
            true,
            in_list(&all),
            in_list(&[SPOTIFY, MUSIC]),
        );
        assert_eq!(id(r), Some((MUSIC, false)));
        // Once it quits, the chosen player takes over.
        let r = route(&c, Some(MUSIC), true, in_list(&all), in_list(&[SPOTIFY]));
        assert_eq!(id(r), Some((SPOTIFY, false)));
        // Then any running player.
        let r = route(&c, Some(MUSIC), true, in_list(&all), in_list(&[VLC]));
        assert_eq!(id(r), Some((VLC, false)));
    }

    #[test]
    fn fixed_ignores_other_running_players() {
        let c = config(MediaTarget::Fixed, SPOTIFY, WhenNotRunning::Ignore);
        let all = [SPOTIFY, MUSIC];
        let r = route(&c, Some(MUSIC), true, in_list(&all), in_list(&[MUSIC]));
        assert_eq!(r, Route::Ignore);
        let r = route(
            &c,
            Some(MUSIC),
            true,
            in_list(&all),
            in_list(&[MUSIC, SPOTIFY]),
        );
        assert_eq!(id(r), Some((SPOTIFY, false)));
    }

    #[test]
    fn nothing_running_follows_the_policy() {
        let all = [SPOTIFY, MUSIC];
        let none = in_list(&[]);
        let launch = config(MediaTarget::LastActive, SPOTIFY, WhenNotRunning::Launch);
        assert_eq!(
            id(route(&launch, None, true, in_list(&all), &none)),
            Some((SPOTIFY, true))
        );
        let ignore = config(MediaTarget::LastActive, SPOTIFY, WhenNotRunning::Ignore);
        assert_eq!(
            route(&ignore, None, true, in_list(&all), &none),
            Route::Ignore
        );
        let pass = config(
            MediaTarget::LastActive,
            SPOTIFY,
            WhenNotRunning::PassThrough,
        );
        assert_eq!(
            route(&pass, None, true, in_list(&all), &none),
            Route::PassThrough
        );
    }

    #[test]
    fn launcher_commands_always_launch() {
        let all = [SPOTIFY, MUSIC];
        let pass = config(MediaTarget::Fixed, SPOTIFY, WhenNotRunning::PassThrough);
        assert_eq!(
            id(route(&pass, None, false, in_list(&all), in_list(&[]))),
            Some((SPOTIFY, true))
        );
    }

    #[test]
    fn uninstalled_choice_falls_back_to_an_installed_player() {
        let c = config(MediaTarget::LastActive, SPOTIFY, WhenNotRunning::Launch);
        let r = route(&c, None, true, in_list(&[MUSIC]), in_list(&[]));
        assert_eq!(id(r), Some((MUSIC, true)));
        // None installed: leave the key to macOS, or do nothing for a launcher command.
        assert_eq!(
            route(&c, None, true, in_list(&[]), in_list(&[])),
            Route::PassThrough
        );
        assert_eq!(
            route(&c, None, false, in_list(&[]), in_list(&[])),
            Route::Ignore
        );
    }

    #[test]
    fn unknown_last_active_is_skipped() {
        let c = config(MediaTarget::LastActive, MUSIC, WhenNotRunning::Ignore);
        let r = route(
            &c,
            Some("com.example.gone"),
            true,
            in_list(&[MUSIC]),
            in_list(&[MUSIC]),
        );
        assert_eq!(id(r), Some((MUSIC, false)));
    }
}
