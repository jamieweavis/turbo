//! Global hotkeys (the palette, palette modes, per-app, system action, window, keep awake and
//! colour picker hotkeys) and the app focus-or-hide toggle.
//!
//! Hotkeys go through Carbon's `RegisterEventHotKey`, which is still the supported way to
//! claim a system-wide key combination and, unlike an event tap, needs no Accessibility
//! permission. Carbon delivers the presses to the application event target, i.e. on the
//! main thread through the app's run loop.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::NSString;

use super::{
    LauncherObserver, PICK_COLOR_ID, PaletteMode, Status, audio, failure_reporter, settings_panes,
    shortcuts, system,
};
use crate::config::{Hotkey, LauncherConfig, Modifier, WindowAction};
use crate::error::CoreError;
use crate::keep_awake::{KeepAwake, KeepAwakeCommand};
use crate::media_keys::{MediaCommand, MediaKeys};
use crate::windows::WindowManager;

/// Registers the palette and per-app hotkeys. Hotkeys don't need Accessibility.
/// `new` and `apply` must be called on the main thread.
pub(crate) struct HotkeyManager {
    /// Read by the Carbon handler through a raw pointer, so it outlives the handler.
    inner: Arc<Inner>,
    carbon: Mutex<Carbon>,
}

struct Inner {
    observer: Mutex<Option<Arc<dyn LauncherObserver>>>,
    /// What each registered hotkey id does.
    targets: Mutex<HashMap<u32, Target>>,
    windows: Arc<WindowManager>,
    keep_awake: Arc<KeepAwake>,
    media: Arc<MediaKeys>,
}

/// Carbon's refs: only touched on the main thread (`apply` and `Drop` check), but kept in
/// the manager, which `Core` shares across threads.
struct Carbon {
    handler: EventHandlerRef,
    registered: Vec<EventHotKeyRef>,
    failed: Vec<Hotkey>,
    /// The latest config, re-applied when hotkeys are resumed.
    config: Option<LauncherConfig>,
    /// While a shortcut recorder is listening, nothing is registered, so pressing an
    /// existing hotkey records it instead of firing it.
    paused: bool,
}

// SAFETY: the raw Carbon refs are opaque tokens only dereferenced by Carbon, and every call
// that uses them happens on the main thread.
unsafe impl Send for Carbon {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Palette,
    Mode(PaletteMode),
    App(String),
    System(String),
    Window(WindowAction),
    KeepAwake(KeepAwakeCommand),
    Media(MediaCommand),
    PickColor,
    /// A System Settings pane, by id.
    Settings(String),
    /// Make a sound device (by UID) the default output or input.
    AudioDevice(audio::Direction, String),
    /// Run a shortcut, by its identifier.
    Shortcut(String),
}

impl Target {
    /// What a launcher item's hotkey (`ItemSettings::hotkey`) does, from its result id.
    fn for_item(id: &str) -> Option<Self> {
        let (kind, rest) = id.split_once(':')?;
        match kind {
            "mode" => super::mode_for_id(rest).map(Target::Mode),
            "app" => Some(Target::App(rest.to_owned())),
            "system" => Some(Target::System(rest.to_owned())),
            "settings" => Some(Target::Settings(rest.to_owned())),
            "window" => crate::windows::action_for_id(rest).map(Target::Window),
            "keep-awake" => super::keep_awake_command_for_id(rest).map(Target::KeepAwake),
            "media" => super::media_command_for_id(rest).map(Target::Media),
            "shortcut" => Some(Target::Shortcut(rest.to_owned())),
            _ if id == PICK_COLOR_ID => Some(Target::PickColor),
            _ if let Some(direction) = audio::Direction::from_id_prefix(kind) => {
                Some(Target::AudioDevice(direction, rest.to_owned()))
            }
            _ => None,
        }
    }
}

impl Target {
    /// False when the target's feature is turned off, so its hotkey isn't registered.
    fn is_enabled(&self, config: &LauncherConfig) -> bool {
        match self {
            Target::Mode(PaletteMode::Clipboard) => config.clipboard.enabled,
            Target::Mode(PaletteMode::Files) => config.files.enabled,
            Target::Mode(PaletteMode::Colors) | Target::PickColor => config.color_picker.enabled,
            Target::Window(_) => config.windows.enabled,
            Target::KeepAwake(_) => config.keep_awake.enabled,
            Target::Mode(PaletteMode::Emoji) => config.emoji.enabled,
            Target::Mode(PaletteMode::Camera) => config.camera.enabled,
            Target::Palette
            | Target::Mode(PaletteMode::Search)
            | Target::App(_)
            | Target::System(_)
            | Target::Media(_)
            | Target::Settings(_)
            | Target::AudioDevice(..)
            | Target::Shortcut(_) => true,
        }
    }
}

impl HotkeyManager {
    pub fn new(
        windows: Arc<WindowManager>,
        keep_awake: Arc<KeepAwake>,
        media: Arc<MediaKeys>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                observer: Mutex::new(None),
                targets: Mutex::new(HashMap::new()),
                windows,
                keep_awake,
                media,
            }),
            carbon: Mutex::new(Carbon {
                handler: ptr::null_mut(),
                registered: Vec::new(),
                failed: Vec::new(),
                config: None,
                paused: false,
            }),
        }
    }

    /// Receives `toggle_palette` when the palette hotkey is pressed.
    pub fn set_observer(&self, observer: Option<Arc<dyn LauncherObserver>>) {
        *self.inner.observer.lock() = observer;
    }

    /// Replaces every registered hotkey with the ones in `config`.
    pub fn apply(&self, config: &LauncherConfig) {
        if !is_main_thread() {
            log::warn!("hotkeys can only be registered on the main thread");
            return;
        }
        let mut carbon = self.carbon.lock();
        carbon.config = Some(config.clone());
        if carbon.handler.is_null() {
            carbon.handler = install_handler(&self.inner);
        }
        unregister_all(&mut carbon);
        if carbon.paused {
            return;
        }

        let plan = plan(config);
        let mut failed = plan.duplicates;
        let mut targets = HashMap::new();
        for planned in plan.hotkeys {
            match register(&planned.hotkey, planned.id) {
                Some(hotkey_ref) => {
                    carbon.registered.push(hotkey_ref);
                    targets.insert(planned.id, planned.target);
                }
                None => failed.push(planned.hotkey),
            }
        }
        log::info!("registered {} hotkeys", carbon.registered.len());
        if !failed.is_empty() {
            log::warn!(
                "{} hotkeys couldn't be registered: {failed:?}",
                failed.len()
            );
        }
        carbon.failed = failed;
        *self.inner.targets.lock() = targets;
    }

    /// Hotkeys from the last `apply` that couldn't be registered: duplicates within the
    /// config, or combinations Carbon refused (usually taken by another app).
    pub fn failed(&self) -> Vec<Hotkey> {
        self.carbon.lock().failed.clone()
    }

    /// Unregisters every hotkey while paused (for shortcut recorders); resuming re-applies
    /// the latest config. Main thread only.
    pub fn set_paused(&self, paused: bool) {
        let config = {
            let mut carbon = self.carbon.lock();
            if carbon.paused == paused {
                return;
            }
            carbon.paused = paused;
            carbon.config.clone()
        };
        if let Some(config) = config {
            self.apply(&config);
        }
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let mut carbon = self.carbon.lock();
        if carbon.handler.is_null() {
            return;
        }
        if !is_main_thread() {
            // Carbon can't be called from here; leave the handler installed and keep its
            // user data alive so a late press can't touch freed memory.
            std::mem::forget(Arc::clone(&self.inner));
            return;
        }
        unregister_all(&mut carbon);
        // SAFETY: the handler was installed by `install_handler` and is removed once.
        unsafe { RemoveEventHandler(carbon.handler) };
        carbon.handler = ptr::null_mut();
    }
}

impl Inner {
    /// Runs the hotkey's target. Called on the main thread; never holds a lock while calling
    /// out, since the shell may call back into the core.
    fn fire(&self, id: u32) {
        let target = self.targets.lock().get(&id).cloned();
        match target {
            Some(Target::Palette) => {
                let observer = self.observer.lock().clone();
                if let Some(observer) = observer {
                    observer.toggle_palette();
                }
            }
            Some(Target::Mode(mode)) => {
                let observer = self.observer.lock().clone();
                if let Some(observer) = observer {
                    observer.show_mode(mode);
                }
            }
            Some(Target::App(bundle_id)) => {
                if let Err(error) = toggle_app(&bundle_id) {
                    log::error!("app hotkey for {bundle_id} failed: {error}");
                }
            }
            Some(Target::System(action)) => {
                let title = system::actions()
                    .iter()
                    .find(|a| a.id == action)
                    .map_or(action.as_str(), |a| a.title);
                let observer = self.observer.lock().clone();
                match system::perform(&action, failure_reporter(observer, title)) {
                    Ok(Some(status)) => self.show_status(status),
                    Ok(None) => {}
                    Err(error) => log::error!("system action hotkey {action} failed: {error}"),
                }
            }
            Some(Target::AudioDevice(direction, uid)) => {
                match audio::set_default(direction, &uid) {
                    Ok(name) => {
                        let symbol = match direction {
                            audio::Direction::Output => "speaker.wave.2",
                            audio::Direction::Input => "mic",
                        };
                        self.show_status(super::audio_status(direction, &name, symbol));
                    }
                    Err(error) => log::error!("sound device hotkey failed: {error}"),
                }
            }
            Some(Target::Shortcut(id)) => {
                let observer = self.observer.lock().clone();
                if let Err(error) = shortcuts::run(&id, failure_reporter(observer, "Shortcut")) {
                    log::error!("shortcut hotkey failed: {error}");
                }
            }
            Some(Target::Window(action)) => {
                if let Err(error) = self.windows.perform(action) {
                    log::error!("couldn't register window hotkey {action:?}: {error}");
                }
            }
            Some(Target::KeepAwake(command)) => {
                let active = command.resolve(self.keep_awake.is_active());
                if let Err(error) = self.keep_awake.set_active(active) {
                    log::error!("couldn't register keep awake hotkey {command:?}: {error}");
                }
                let observer = self.observer.lock().clone();
                if let Some(observer) = observer {
                    observer.keep_awake_changed(self.keep_awake.is_active());
                }
            }
            Some(Target::Media(command)) => {
                if let Err(error) = self.media.perform(command) {
                    log::error!("media hotkey {command:?} failed: {error}");
                }
            }
            Some(Target::PickColor) => {
                let observer = self.observer.lock().clone();
                if let Some(observer) = observer {
                    observer.pick_color();
                }
            }
            Some(Target::Settings(pane)) => {
                if let Err(error) = settings_panes::open(&pane) {
                    log::error!("settings hotkey {pane} failed: {error}");
                }
            }
            None => {}
        }
    }
}

impl Inner {
    fn show_status(&self, status: Status) {
        let observer = self.observer.lock().clone();
        if let Some(observer) = observer {
            observer.show_status(status);
        }
    }
}

// MARK: Planning (pure)

#[derive(Debug, PartialEq)]
struct Planned {
    id: u32,
    hotkey: Hotkey,
    target: Target,
}

#[derive(Debug, PartialEq)]
struct Plan {
    hotkeys: Vec<Planned>,
    /// Later repeats of a combination already used (the first one wins).
    duplicates: Vec<Hotkey>,
}

/// Numbers the hotkeys to register, palette first, dropping repeated combinations.
fn plan(config: &LauncherConfig) -> Plan {
    let wanted = config
        .hotkey
        .iter()
        .map(|hotkey| (hotkey.clone(), Target::Palette))
        .chain(
            config
                .clipboard
                .hotkey
                .iter()
                .map(|hotkey| (hotkey.clone(), Target::Mode(PaletteMode::Clipboard))),
        )
        .chain(
            config
                .files
                .hotkey
                .iter()
                .map(|hotkey| (hotkey.clone(), Target::Mode(PaletteMode::Files))),
        )
        .chain(
            config
                .app_hotkeys
                .iter()
                .map(|app| (app.hotkey.clone(), Target::App(app.bundle_id.clone()))),
        )
        .chain(
            config
                .action_hotkeys
                .iter()
                .map(|action| (action.hotkey.clone(), Target::System(action.action.clone()))),
        )
        .chain(
            config
                .windows
                .hotkeys
                .iter()
                .filter(|_| config.windows.hotkeys_enabled)
                .map(|binding| (binding.hotkey.clone(), Target::Window(binding.action))),
        )
        .chain(
            [
                (&config.keep_awake.toggle_hotkey, KeepAwakeCommand::Toggle),
                (&config.keep_awake.on_hotkey, KeepAwakeCommand::On),
                (&config.keep_awake.off_hotkey, KeepAwakeCommand::Off),
            ]
            .into_iter()
            .filter_map(|(hotkey, command)| {
                hotkey
                    .clone()
                    .map(|hotkey| (hotkey, Target::KeepAwake(command)))
            }),
        )
        .chain(
            config
                .color_picker
                .hotkey
                .iter()
                .map(|hotkey| (hotkey.clone(), Target::PickColor)),
        )
        .chain(
            config
                .items
                .iter()
                .filter_map(|item| Some((item.hotkey.clone()?, Target::for_item(&item.id)?))),
        )
        .filter(|(_, target)| target.is_enabled(config));

    let mut seen = Vec::new();
    let mut hotkeys = Vec::new();
    let mut duplicates = Vec::new();
    for (hotkey, target) in wanted {
        let key = (hotkey.key_code, carbon_modifiers(&hotkey.modifiers));
        if seen.contains(&key) {
            duplicates.push(hotkey);
            continue;
        }
        seen.push(key);
        hotkeys.push(Planned {
            id: hotkeys.len() as u32 + 1,
            hotkey,
            target,
        });
    }
    Plan {
        hotkeys,
        duplicates,
    }
}

/// Carbon modifier masks (`Events.h`).
fn carbon_modifiers(modifiers: &[Modifier]) -> u32 {
    modifiers.iter().fold(0, |mask, modifier| {
        mask | match modifier {
            Modifier::Command => CMD_KEY,
            Modifier::Shift => SHIFT_KEY,
            Modifier::Option => OPTION_KEY,
            Modifier::Control => CONTROL_KEY,
        }
    })
}

// MARK: App toggle

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppState {
    NotRunning,
    Frontmost,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToggleAction {
    Hide,
    /// Launch, or for a running app activate it the way a Dock click does (unhiding it and
    /// reopening a window if it has none).
    Open,
}

fn decide(state: AppState) -> ToggleAction {
    match state {
        AppState::Frontmost => ToggleAction::Hide,
        AppState::Background | AppState::NotRunning => ToggleAction::Open,
    }
}

/// Focuses the app (launching it if needed), or hides it if it's already in front.
fn toggle_app(bundle_id: &str) -> Result<(), CoreError> {
    let id = NSString::from_str(bundle_id);
    let running: Vec<_> = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id)
        .to_vec()
        .into_iter()
        .filter(|app| !app.isTerminated())
        .collect();
    let state = if running.is_empty() {
        AppState::NotRunning
    } else if running.iter().any(|app| app.isActive()) {
        AppState::Frontmost
    } else {
        AppState::Background
    };

    match decide(state) {
        ToggleAction::Hide => {
            for app in &running {
                app.hide();
            }
            Ok(())
        }
        ToggleAction::Open => {
            let workspace = NSWorkspace::sharedWorkspace();
            // Prefer the running copy's bundle, in case several copies are installed.
            let url = running
                .iter()
                .find_map(|app| app.bundleURL())
                .or_else(|| workspace.URLForApplicationWithBundleIdentifier(&id))
                .ok_or_else(|| CoreError::Other {
                    message: format!("no app with bundle id {bundle_id} is installed"),
                })?;
            // Opening a running app sends it the reopen event, like clicking its Dock icon.
            let configuration = NSWorkspaceOpenConfiguration::configuration();
            configuration.setActivates(true);
            workspace.openApplicationAtURL_configuration_completionHandler(
                &url,
                &configuration,
                None,
            );
            Ok(())
        }
    }
}

// MARK: Carbon

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerProc = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerProc,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RemoveEventHandler(handler: EventHandlerRef) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hotkey: EventHotKeyRef) -> OSStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
}

unsafe extern "C" {
    fn pthread_main_np() -> i32;
}

const fn four_cc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// Tags our hotkey ids so presses meant for other handlers are ignored.
const SIGNATURE: u32 = four_cc(b"JayC");
const EVENT_CLASS_KEYBOARD: u32 = four_cc(b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const EVENT_PARAM_DIRECT_OBJECT: u32 = four_cc(b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = four_cc(b"hkid");
const NO_ERR: OSStatus = 0;
const EVENT_NOT_HANDLED_ERR: OSStatus = -9874;

const CMD_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;

fn is_main_thread() -> bool {
    // SAFETY: no arguments; reports whether the calling thread is the main thread.
    unsafe { pthread_main_np() != 0 }
}

fn install_handler(inner: &Arc<Inner>) -> EventHandlerRef {
    let spec = EventTypeSpec {
        event_class: EVENT_CLASS_KEYBOARD,
        event_kind: EVENT_HOT_KEY_PRESSED,
    };
    let mut handler = ptr::null_mut();
    // SAFETY: `hotkey_handler` matches Carbon's handler ABI; `inner` is kept alive by the
    // manager until the handler is removed (or forever, if `Drop` can't remove it).
    let status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            hotkey_handler,
            1,
            &spec,
            Arc::as_ptr(inner).cast_mut().cast(),
            &mut handler,
        )
    };
    if status != NO_ERR {
        log::error!("couldn't install the hotkey handler ({status})");
        return ptr::null_mut();
    }
    handler
}

fn register(hotkey: &Hotkey, id: u32) -> Option<EventHotKeyRef> {
    let mut hotkey_ref = ptr::null_mut();
    // SAFETY: plain values in, one ref out; called on the main thread.
    let status = unsafe {
        RegisterEventHotKey(
            u32::from(hotkey.key_code),
            carbon_modifiers(&hotkey.modifiers),
            EventHotKeyID {
                signature: SIGNATURE,
                id,
            },
            GetApplicationEventTarget(),
            0,
            &mut hotkey_ref,
        )
    };
    (status == NO_ERR && !hotkey_ref.is_null()).then_some(hotkey_ref)
}

fn unregister_all(carbon: &mut Carbon) {
    for hotkey_ref in carbon.registered.drain(..) {
        // SAFETY: each ref came from a successful `RegisterEventHotKey`.
        unsafe { UnregisterEventHotKey(hotkey_ref) };
    }
}

extern "C" fn hotkey_handler(
    _call: EventHandlerCallRef,
    event: EventRef,
    user_data: *mut c_void,
) -> OSStatus {
    // A panic must never unwind into Carbon.
    catch_unwind(AssertUnwindSafe(|| {
        let mut id = EventHotKeyID::default();
        // SAFETY: reads the hotkey id parameter into a correctly sized buffer.
        let status = unsafe {
            GetEventParameter(
                event,
                EVENT_PARAM_DIRECT_OBJECT,
                TYPE_EVENT_HOT_KEY_ID,
                ptr::null_mut(),
                size_of::<EventHotKeyID>(),
                ptr::null_mut(),
                (&raw mut id).cast(),
            )
        };
        if status != NO_ERR || id.signature != SIGNATURE {
            return EVENT_NOT_HANDLED_ERR;
        }
        // SAFETY: `user_data` is the manager's `Inner`, alive while the handler is installed.
        let inner = unsafe { &*user_data.cast::<Inner>() };
        inner.fire(id.id);
        NO_ERR
    }))
    .unwrap_or(EVENT_NOT_HANDLED_ERR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ActionHotkey, AppHotkey, KEY_SPACE, WindowHotkey};

    fn hotkey(key_code: u16, modifiers: &[Modifier]) -> Hotkey {
        Hotkey {
            key_code,
            modifiers: modifiers.to_vec(),
        }
    }

    fn app(bundle_id: &str, hotkey: Hotkey) -> AppHotkey {
        AppHotkey {
            bundle_id: bundle_id.into(),
            name: bundle_id.into(),
            hotkey,
        }
    }

    #[test]
    fn modifiers_map_to_carbon_masks() {
        assert_eq!(carbon_modifiers(&[]), 0);
        assert_eq!(carbon_modifiers(&[Modifier::Option]), 0x0800);
        assert_eq!(
            carbon_modifiers(&[Modifier::Command, Modifier::Shift, Modifier::Control]),
            0x0100 | 0x0200 | 0x1000
        );
    }

    /// The defaults minus the clipboard/file/window hotkeys, so plans only hold what a test
    /// lists.
    fn without_mode_hotkeys() -> LauncherConfig {
        let mut config = LauncherConfig::default();
        config.clipboard.hotkey = None;
        config.files.hotkey = None;
        config.windows.hotkeys.clear();
        config
    }

    #[test]
    fn item_hotkeys_open_modes_and_settings_panes() {
        use crate::config::ItemSettings;
        let mut config = without_mode_hotkeys();
        config.hotkey = None;
        let item = |id: &str, key_code| ItemSettings {
            id: id.into(),
            hotkey: Some(hotkey(key_code, &[Modifier::Option])),
            ..ItemSettings::default()
        };
        config.items = vec![
            item("mode:emoji", 0x0E),
            item("mode:camera", 0x08),
            item("settings:sound", 0x01),
            item("unknown:thing", 0x02),
            ItemSettings {
                id: "mode:colors".into(),
                ..ItemSettings::default()
            },
        ];
        let targets: Vec<Target> = plan(&config)
            .hotkeys
            .into_iter()
            .map(|p| p.target)
            .collect();
        assert_eq!(
            targets,
            vec![
                Target::Mode(PaletteMode::Emoji),
                Target::Mode(PaletteMode::Camera),
                Target::Settings("sound".into())
            ]
        );
        config.camera.enabled = false;
        assert!(
            !plan(&config)
                .hotkeys
                .iter()
                .any(|p| p.target == Target::Mode(PaletteMode::Camera))
        );
        assert_eq!(Target::for_item(PICK_COLOR_ID), Some(Target::PickColor));
        config.emoji.enabled = false;
        assert!(
            !plan(&config)
                .hotkeys
                .iter()
                .any(|p| p.target == Target::Mode(PaletteMode::Emoji))
        );
        assert_eq!(
            Target::for_item("keep-awake:on"),
            Some(Target::KeepAwake(KeepAwakeCommand::On))
        );
    }

    #[test]
    fn window_hotkeys_come_last_and_can_be_switched_off() {
        let mut config = without_mode_hotkeys();
        config.windows.hotkeys = vec![WindowHotkey {
            action: WindowAction::LeftHalf,
            hotkey: hotkey(0x7B, &[Modifier::Control, Modifier::Option]),
        }];
        let targets: Vec<Target> = plan(&config)
            .hotkeys
            .into_iter()
            .map(|p| p.target)
            .collect();
        assert_eq!(
            targets,
            vec![Target::Palette, Target::Window(WindowAction::LeftHalf)]
        );
        config.windows.hotkeys_enabled = false;
        assert_eq!(plan(&config).hotkeys.len(), 1);
    }

    #[test]
    fn keep_awake_hotkeys_come_after_windows() {
        let mut config = without_mode_hotkeys();
        config.keep_awake.toggle_hotkey = Some(hotkey(8, &[Modifier::Control, Modifier::Option]));
        config.keep_awake.off_hotkey = Some(hotkey(8, &[Modifier::Control, Modifier::Command]));
        let targets: Vec<Target> = plan(&config)
            .hotkeys
            .into_iter()
            .map(|p| p.target)
            .collect();
        assert_eq!(
            targets,
            vec![
                Target::Palette,
                Target::KeepAwake(KeepAwakeCommand::Toggle),
                Target::KeepAwake(KeepAwakeCommand::Off),
            ]
        );
    }

    #[test]
    fn color_picker_hotkey_comes_last() {
        let mut config = without_mode_hotkeys();
        config.keep_awake.toggle_hotkey = Some(hotkey(8, &[Modifier::Control, Modifier::Option]));
        config.color_picker.hotkey = Some(hotkey(8, &[Modifier::Control, Modifier::Shift]));
        let targets: Vec<Target> = plan(&config)
            .hotkeys
            .into_iter()
            .map(|p| p.target)
            .collect();
        assert_eq!(
            targets,
            vec![
                Target::Palette,
                Target::KeepAwake(KeepAwakeCommand::Toggle),
                Target::PickColor,
            ]
        );
    }

    #[test]
    fn default_hotkeys_dont_collide() {
        assert!(plan(&LauncherConfig::default()).duplicates.is_empty());
    }

    #[test]
    fn mode_hotkeys_follow_the_palette() {
        let mut config = without_mode_hotkeys();
        config.clipboard.hotkey = Some(hotkey(9, &[Modifier::Option, Modifier::Command]));
        config.files.hotkey = Some(hotkey(3, &[Modifier::Option, Modifier::Command]));
        let targets: Vec<Target> = plan(&config)
            .hotkeys
            .into_iter()
            .map(|p| p.target)
            .collect();
        assert_eq!(
            targets,
            vec![
                Target::Palette,
                Target::Mode(PaletteMode::Clipboard),
                Target::Mode(PaletteMode::Files),
            ]
        );
    }

    #[test]
    fn plan_numbers_palette_first_then_apps() {
        let config = LauncherConfig {
            hotkey: Some(hotkey(KEY_SPACE, &[Modifier::Option])),
            app_hotkeys: vec![
                app("com.apple.Safari", hotkey(1, &[Modifier::Control])),
                app("com.apple.Terminal", hotkey(17, &[Modifier::Control])),
            ],
            ..without_mode_hotkeys()
        };
        let plan = plan(&config);
        assert!(plan.duplicates.is_empty());
        let summary: Vec<_> = plan
            .hotkeys
            .iter()
            .map(|p| (p.id, p.target.clone()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (1, Target::Palette),
                (2, Target::App("com.apple.Safari".into())),
                (3, Target::App("com.apple.Terminal".into())),
            ]
        );
    }

    #[test]
    fn features_turned_off_register_no_hotkeys() {
        let mut config = LauncherConfig::default();
        config.files.hotkey = Some(hotkey(3, &[Modifier::Option, Modifier::Command]));
        config.keep_awake.toggle_hotkey = Some(hotkey(14, &[Modifier::Control]));
        config.clipboard.enabled = false;
        config.files.enabled = false;
        config.windows.enabled = false;
        config.keep_awake.enabled = false;
        let targets: Vec<Target> = plan(&config)
            .hotkeys
            .into_iter()
            .map(|p| p.target)
            .collect();
        assert_eq!(targets, vec![Target::Palette]);
    }

    #[test]
    fn system_action_hotkeys_follow_apps() {
        let config = LauncherConfig {
            hotkey: None,
            app_hotkeys: vec![app("com.apple.Safari", hotkey(1, &[Modifier::Control]))],
            action_hotkeys: vec![ActionHotkey {
                action: "lock-screen".into(),
                hotkey: hotkey(37, &[Modifier::Control, Modifier::Option]),
            }],
            ..without_mode_hotkeys()
        };
        let targets: Vec<Target> = plan(&config)
            .hotkeys
            .into_iter()
            .map(|p| p.target)
            .collect();
        assert_eq!(
            targets,
            vec![
                Target::App("com.apple.Safari".into()),
                Target::System("lock-screen".into()),
            ]
        );
    }

    #[test]
    fn repeated_combinations_are_reported_whatever_the_modifier_order() {
        let config = LauncherConfig {
            hotkey: Some(hotkey(KEY_SPACE, &[Modifier::Command, Modifier::Shift])),
            app_hotkeys: vec![app(
                "com.apple.Safari",
                hotkey(KEY_SPACE, &[Modifier::Shift, Modifier::Command]),
            )],
            ..without_mode_hotkeys()
        };
        let plan = plan(&config);
        assert_eq!(plan.hotkeys.len(), 1);
        assert_eq!(plan.hotkeys[0].target, Target::Palette);
        assert_eq!(plan.duplicates.len(), 1);
    }

    #[test]
    fn no_palette_hotkey_still_registers_apps() {
        let config = LauncherConfig {
            hotkey: None,
            app_hotkeys: vec![app("com.apple.Safari", hotkey(1, &[Modifier::Control]))],
            ..without_mode_hotkeys()
        };
        let plan = plan(&config);
        assert_eq!(plan.hotkeys.len(), 1);
        assert_eq!(plan.hotkeys[0].id, 1);
    }

    #[test]
    fn toggle_hides_frontmost_and_opens_otherwise() {
        assert_eq!(decide(AppState::Frontmost), ToggleAction::Hide);
        assert_eq!(decide(AppState::Background), ToggleAction::Open);
        assert_eq!(decide(AppState::NotRunning), ToggleAction::Open);
    }

    #[test]
    fn signature_is_big_endian_four_cc() {
        assert_eq!(four_cc(b"keyb"), 0x6B65_7962);
        assert_eq!(four_cc(b"----"), 0x2D2D_2D2D);
    }

    #[test]
    fn apply_off_the_main_thread_is_ignored() {
        // Test threads are never the main thread, so this must not touch Carbon.
        let manager = HotkeyManager::new(
            Arc::new(WindowManager::new(Default::default())),
            Arc::new(KeepAwake::new(
                &std::env::temp_dir().join(format!("backpack-test-hotkeys-{}", std::process::id())),
                Default::default(),
            )),
            Arc::new(MediaKeys::new(Default::default())),
        );
        manager.apply(&LauncherConfig::default());
        assert!(manager.failed().is_empty());
        assert!(manager.carbon.lock().handler.is_null());
    }
}
