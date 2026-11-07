//! Sound devices and volume through Core Audio: switching the default output or input,
//! stepping or setting the output volume, and muting the microphone.
//!
//! All public Core Audio API, so nothing here asks for permission. Volume uses the
//! device's virtual main volume (what the volume keys change); devices without one (many
//! USB interfaces, HDMI and DisplayPort monitors) can't be changed, and say so.
//!
//! Property reads are IPC to coreaudiod and cost about a millisecond each, so the launcher
//! keeps the device list cached and refreshes it from a property listener (`watch`) rather
//! than querying on every keystroke.

use std::ffi::c_void;
use std::ptr;

use objc2_core_foundation::{CFRetained, CFString};

use crate::error::CoreError;

type AudioObjectId = u32;
type OsStatus = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

type ListenerProc = unsafe extern "C" fn(
    object: AudioObjectId,
    count: u32,
    addresses: *const PropertyAddress,
    client_data: *mut c_void,
) -> OsStatus;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectHasProperty(object: AudioObjectId, address: *const PropertyAddress) -> u8;
    fn AudioObjectIsPropertySettable(
        object: AudioObjectId,
        address: *const PropertyAddress,
        settable: *mut u8,
    ) -> OsStatus;
    fn AudioObjectGetPropertyDataSize(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> OsStatus;
    fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectSetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> OsStatus;
    fn AudioObjectAddPropertyListener(
        object: AudioObjectId,
        address: *const PropertyAddress,
        listener: ListenerProc,
        client_data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectRemovePropertyListener(
        object: AudioObjectId,
        address: *const PropertyAddress,
        listener: ListenerProc,
        client_data: *mut c_void,
    ) -> OsStatus;
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const SYSTEM_OBJECT: AudioObjectId = 1;
const UNKNOWN_OBJECT: AudioObjectId = 0;
const ELEMENT_MAIN: u32 = 0;

const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
const SCOPE_INPUT: u32 = fourcc(b"inpt");

const DEVICES: u32 = fourcc(b"dev#");
const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const DEFAULT_INPUT: u32 = fourcc(b"dIn ");
const TRANSLATE_UID: u32 = fourcc(b"uidd");
const NAME: u32 = fourcc(b"lnam");
const UID: u32 = fourcc(b"uid ");
const STREAMS: u32 = fourcc(b"stm#");
const CAN_BE_DEFAULT: u32 = fourcc(b"dflt");
const IS_HIDDEN: u32 = fourcc(b"hidn");
const TRANSPORT: u32 = fourcc(b"tran");
const MUTE: u32 = fourcc(b"mute");
/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`: one volume for every channel,
/// which coreaudiod answers for plain `AudioObject` calls.
const VIRTUAL_MAIN_VOLUME: u32 = fourcc(b"vmvc");

const TRANSPORT_BUILT_IN: u32 = fourcc(b"bltn");
const TRANSPORT_BLUETOOTH: u32 = fourcc(b"blue");
const TRANSPORT_BLUETOOTH_LE: u32 = fourcc(b"blea");
const TRANSPORT_HDMI: u32 = fourcc(b"hdmi");
const TRANSPORT_DISPLAY_PORT: u32 = fourcc(b"dprt");
const TRANSPORT_AIRPLAY: u32 = fourcc(b"airp");
const TRANSPORT_CONTINUITY: u32 = fourcc(b"ccwd");

/// The volume keys' step: a sixteenth.
pub(crate) const VOLUME_STEP: f32 = 1.0 / 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Direction {
    Output,
    Input,
}

impl Direction {
    fn scope(self) -> u32 {
        match self {
            Direction::Output => SCOPE_OUTPUT,
            Direction::Input => SCOPE_INPUT,
        }
    }

    fn default_selector(self) -> u32 {
        match self {
            Direction::Output => DEFAULT_OUTPUT,
            Direction::Input => DEFAULT_INPUT,
        }
    }

    /// The prefix of its launcher result ids.
    pub(crate) fn id_prefix(self) -> &'static str {
        match self {
            Direction::Output => "audio-output",
            Direction::Input => "audio-input",
        }
    }

    pub(crate) fn from_id_prefix(prefix: &str) -> Option<Self> {
        [Direction::Output, Direction::Input]
            .into_iter()
            .find(|d| d.id_prefix() == prefix)
    }

    /// The launcher subtitle and settings category.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Direction::Output => "Sound Output",
            Direction::Input => "Sound Input",
        }
    }
}

/// A device that can be the default output or input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Device {
    /// Stable across reconnects and restarts, unlike the object id.
    pub uid: String,
    pub name: String,
    pub direction: Direction,
    /// SF Symbol for its kind (speaker, headphones, display, microphone, …).
    pub symbol: &'static str,
}

fn address(selector: u32, scope: u32) -> PropertyAddress {
    PropertyAddress {
        selector,
        scope,
        element: ELEMENT_MAIN,
    }
}

fn check(status: OsStatus, what: &str) -> Result<(), CoreError> {
    if status == 0 {
        Ok(())
    } else {
        Err(CoreError::Other {
            message: format!("couldn't {what} (Core Audio error {status})"),
        })
    }
}

/// Reads a fixed-size property.
fn get<T: Copy + Default>(object: AudioObjectId, address: PropertyAddress) -> Option<T> {
    let mut value = T::default();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `value` is a valid, writable `T` of `size` bytes; Core Audio writes at most
    // `size` bytes and reports how many it wrote.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    (status == 0 && size as usize == size_of::<T>()).then_some(value)
}

fn set<T: Copy>(object: AudioObjectId, address: PropertyAddress, value: T) -> OsStatus {
    // SAFETY: `value` is a valid `T` and its size is passed alongside it.
    unsafe {
        AudioObjectSetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            size_of::<T>() as u32,
            (&raw const value).cast(),
        )
    }
}

fn data_size(object: AudioObjectId, address: PropertyAddress) -> u32 {
    let mut size = 0;
    // SAFETY: `size` is a valid out pointer.
    let status =
        unsafe { AudioObjectGetPropertyDataSize(object, &address, 0, ptr::null(), &mut size) };
    if status == 0 { size } else { 0 }
}

fn has(object: AudioObjectId, address: PropertyAddress) -> bool {
    // SAFETY: `address` is a valid property address.
    unsafe { AudioObjectHasProperty(object, &address) != 0 }
}

fn settable(object: AudioObjectId, address: PropertyAddress) -> bool {
    if !has(object, address) {
        return false;
    }
    let mut settable = 0u8;
    // SAFETY: `settable` is a valid out pointer.
    let status = unsafe { AudioObjectIsPropertySettable(object, &address, &mut settable) };
    status == 0 && settable != 0
}

/// Reads a `CFStringRef` property; the caller owns the returned string.
fn get_string(object: AudioObjectId, selector: u32) -> Option<String> {
    let value: *const CFString = get(object, address(selector, SCOPE_GLOBAL))?;
    if value.is_null() {
        return None;
    }
    // SAFETY: Core Audio returns these strings retained (+1), so we take ownership.
    let string = unsafe { CFRetained::from_raw(ptr::NonNull::new(value.cast_mut())?) };
    Some(string.to_string())
}

fn device_ids() -> Vec<AudioObjectId> {
    let address = address(DEVICES, SCOPE_GLOBAL);
    let size = data_size(SYSTEM_OBJECT, address);
    let mut ids = vec![0 as AudioObjectId; size as usize / size_of::<AudioObjectId>()];
    let mut size = (ids.len() * size_of::<AudioObjectId>()) as u32;
    // SAFETY: `ids` has room for `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &address,
            0,
            ptr::null(),
            &mut size,
            ids.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / size_of::<AudioObjectId>());
    ids
}

fn symbol(transport: u32, direction: Direction) -> &'static str {
    match (direction, transport) {
        (Direction::Input, TRANSPORT_CONTINUITY) => "iphone",
        (Direction::Input, _) => "mic",
        (_, TRANSPORT_BUILT_IN) => "speaker.wave.2",
        (_, TRANSPORT_BLUETOOTH | TRANSPORT_BLUETOOTH_LE) => "headphones",
        (_, TRANSPORT_HDMI | TRANSPORT_DISPLAY_PORT) => "display",
        (_, TRANSPORT_AIRPLAY) => "airplayaudio",
        _ => "hifispeaker",
    }
}

/// Every visible device that can be the default output or input, in Core Audio's order; a
/// device with both (a USB headset) is listed once per direction.
pub(crate) fn devices() -> Vec<Device> {
    let mut devices = Vec::new();
    for id in device_ids() {
        if get::<u32>(id, address(IS_HIDDEN, SCOPE_GLOBAL)).unwrap_or(0) != 0 {
            continue;
        }
        let (Some(uid), Some(name)) = (get_string(id, UID), get_string(id, NAME)) else {
            continue;
        };
        let transport = get::<u32>(id, address(TRANSPORT, SCOPE_GLOBAL)).unwrap_or(0);
        for direction in [Direction::Output, Direction::Input] {
            let scope = direction.scope();
            let has_streams = data_size(id, address(STREAMS, scope)) > 0;
            let can_be_default = get::<u32>(id, address(CAN_BE_DEFAULT, scope)).unwrap_or(0) != 0;
            if has_streams && can_be_default {
                devices.push(Device {
                    uid: uid.clone(),
                    name: name.clone(),
                    direction,
                    symbol: symbol(transport, direction),
                });
            }
        }
    }
    devices
}

fn default_device(direction: Direction) -> Result<AudioObjectId, CoreError> {
    match get::<AudioObjectId>(
        SYSTEM_OBJECT,
        address(direction.default_selector(), SCOPE_GLOBAL),
    ) {
        Some(id) if id != UNKNOWN_OBJECT => Ok(id),
        _ => Err(CoreError::Other {
            message: format!("there's no {}", direction.label().to_lowercase()),
        }),
    }
}

fn device_name(id: AudioObjectId) -> String {
    get_string(id, NAME).unwrap_or_else(|| "This device".to_owned())
}

/// Makes the device with `uid` the default for `direction`; returns its name.
pub(crate) fn set_default(direction: Direction, uid: &str) -> Result<String, CoreError> {
    let cf_uid = CFString::from_str(uid);
    let qualifier: *const CFString = &*cf_uid;
    let mut id = UNKNOWN_OBJECT;
    let mut size = size_of::<AudioObjectId>() as u32;
    let translate = address(TRANSLATE_UID, SCOPE_GLOBAL);
    // SAFETY: the qualifier is a `CFStringRef` (pointer-sized) kept alive by `cf_uid`, and
    // `id` is a valid out pointer of `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &translate,
            size_of::<*const CFString>() as u32,
            (&raw const qualifier).cast(),
            &mut size,
            (&raw mut id).cast(),
        )
    };
    if status != 0 || id == UNKNOWN_OBJECT {
        return Err(CoreError::Other {
            message: "that sound device isn't connected".into(),
        });
    }
    check(
        set(
            SYSTEM_OBJECT,
            address(direction.default_selector(), SCOPE_GLOBAL),
            id,
        ),
        "switch the sound device",
    )?;
    Ok(device_name(id))
}

fn no_volume(id: AudioObjectId) -> CoreError {
    CoreError::Other {
        message: format!("{} has no volume control", device_name(id)),
    }
}

/// The default output's volume, 0…1.
fn output_volume(id: AudioObjectId) -> Result<f32, CoreError> {
    get::<f32>(id, address(VIRTUAL_MAIN_VOLUME, SCOPE_OUTPUT)).ok_or_else(|| no_volume(id))
}

/// Sets the default output's volume (clamped to 0…1), unmuting it like the volume keys do
/// when raising it. Returns the volume it ended up at.
pub(crate) fn set_output_volume(volume: f32) -> Result<f32, CoreError> {
    let id = default_device(Direction::Output)?;
    let volume_address = address(VIRTUAL_MAIN_VOLUME, SCOPE_OUTPUT);
    if !settable(id, volume_address) {
        return Err(no_volume(id));
    }
    let volume = volume.clamp(0.0, 1.0);
    check(set(id, volume_address, volume), "set the volume")?;
    let mute = address(MUTE, SCOPE_OUTPUT);
    if volume > 0.0 && settable(id, mute) {
        // Not fatal: the volume itself changed.
        let _ = set(id, mute, 0u32);
    }
    // The device may round to its own steps.
    Ok(output_volume(id).unwrap_or(volume))
}

/// Raises or lowers the default output's volume by a step; returns the new volume.
pub(crate) fn step_output_volume(up: bool) -> Result<f32, CoreError> {
    let id = default_device(Direction::Output)?;
    let current = output_volume(id)?;
    // Snap to the step grid, as the volume keys do, so repeated steps land on round numbers.
    let steps = (current / VOLUME_STEP).round() + if up { 1.0 } else { -1.0 };
    set_output_volume(steps * VOLUME_STEP)
}

/// Mutes or unmutes the default input; returns whether it's now muted and its name.
pub(crate) fn toggle_input_mute() -> Result<(bool, String), CoreError> {
    let id = default_device(Direction::Input)?;
    let mute = address(MUTE, SCOPE_INPUT);
    if !settable(id, mute) {
        return Err(CoreError::Other {
            message: format!("{} can't be muted", device_name(id)),
        });
    }
    let muted = get::<u32>(id, mute).unwrap_or(0) == 0;
    check(set(id, mute, u32::from(muted)), "mute the microphone")?;
    Ok((muted, device_name(id)))
}

/// Parses "volume 40", "vol 40%" or "set volume 40" into a percentage.
pub(crate) fn parse_volume_query(query: &str) -> Option<u8> {
    let query = query.trim().to_lowercase();
    let rest = query.strip_prefix("set ").unwrap_or(&query);
    let rest = rest
        .strip_prefix("volume")
        .or_else(|| rest.strip_prefix("vol"))?;
    // A separator is required, so "volume2" isn't read as a level.
    let rest = rest.strip_prefix([' ', ':'])?.trim();
    let digits = rest.strip_suffix('%').unwrap_or(rest).trim_end();
    if digits.is_empty() || digits.len() > 3 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let percent: u8 = digits.parse().ok()?;
    (percent <= 100).then_some(percent)
}

/// Calls `on_change` (on a Core Audio thread) whenever devices come or go, until dropped.
pub(crate) struct Watcher {
    /// Leaked into the listener's client data; reclaimed on drop after removing the
    /// listeners.
    callback: *mut Box<dyn Fn() + Send + Sync>,
}

// SAFETY: the pointer is only dereferenced by Core Audio's listener (which requires the
// callback to be `Send + Sync`) and freed once in `drop`.
unsafe impl Send for Watcher {}
unsafe impl Sync for Watcher {}

const WATCHED: [u32; 1] = [DEVICES];

unsafe extern "C" fn listener(
    _object: AudioObjectId,
    _count: u32,
    _addresses: *const PropertyAddress,
    client_data: *mut c_void,
) -> OsStatus {
    // A panic must not unwind into Core Audio.
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `client_data` is the `Box` leaked by `watch`, alive until the listeners
        // are removed in `Watcher::drop`.
        let callback = unsafe { &*client_data.cast::<Box<dyn Fn() + Send + Sync>>() };
        callback();
    });
    0
}

pub(crate) fn watch(on_change: impl Fn() + Send + Sync + 'static) -> Watcher {
    let callback: *mut Box<dyn Fn() + Send + Sync> = Box::into_raw(Box::new(Box::new(on_change)));
    for selector in WATCHED {
        // SAFETY: `callback` stays valid until `drop` removes this listener.
        let status = unsafe {
            AudioObjectAddPropertyListener(
                SYSTEM_OBJECT,
                &address(selector, SCOPE_GLOBAL),
                listener,
                callback.cast(),
            )
        };
        if status != 0 {
            log::warn!("couldn't watch sound devices (Core Audio error {status})");
        }
    }
    Watcher { callback }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        for selector in WATCHED {
            // SAFETY: removes the listener added in `watch` with the same arguments.
            unsafe {
                AudioObjectRemovePropertyListener(
                    SYSTEM_OBJECT,
                    &address(selector, SCOPE_GLOBAL),
                    listener,
                    self.callback.cast(),
                );
            }
        }
        // SAFETY: the listeners are gone (removal waits for a running callback), so nothing
        // else refers to the box.
        drop(unsafe { Box::from_raw(self.callback) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_volume_queries() {
        assert_eq!(parse_volume_query("volume 40"), Some(40));
        assert_eq!(parse_volume_query("Volume 40%"), Some(40));
        assert_eq!(parse_volume_query("vol 0"), Some(0));
        assert_eq!(parse_volume_query("set volume 100"), Some(100));
        assert_eq!(parse_volume_query("volume: 7"), Some(7));
        assert_eq!(parse_volume_query("volume 101"), None);
        assert_eq!(parse_volume_query("volume"), None);
        assert_eq!(parse_volume_query("volume up"), None);
        assert_eq!(parse_volume_query("volume2"), None);
        assert_eq!(parse_volume_query("volumes 4"), None);
        assert_eq!(parse_volume_query("volume -4"), None);
    }

    #[test]
    fn direction_ids_round_trip() {
        for direction in [Direction::Output, Direction::Input] {
            assert_eq!(
                Direction::from_id_prefix(direction.id_prefix()),
                Some(direction)
            );
        }
    }

    /// Lists this Mac's devices; changes nothing.
    #[test]
    #[ignore = "reads the real sound devices"]
    fn lists_devices() {
        let devices = devices();
        println!("{devices:#?}");
        assert!(!devices.is_empty());
        assert!(
            devices
                .iter()
                .all(|d| !d.uid.is_empty() && !d.name.is_empty())
        );
    }
}
