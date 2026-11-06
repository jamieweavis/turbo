//! Camera Preview: which cameras are connected, which one the preview shows, and the
//! camera permission. The shell runs the capture session for the chosen camera and draws
//! it; everything it needs to decide what to show comes from here.
//!
//! The chosen camera is state, not a setting, so it lives in `<data_dir>/camera.json`
//! rather than the config (the shell writes the launcher config back whole, which would
//! undo a camera chosen in between).

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDevicePosition, AVCaptureDeviceType, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureDeviceTypeContinuityCamera, AVCaptureDeviceTypeExternal, AVMediaTypeVideo,
};
use objc2_foundation::NSArray;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, uniffi::Enum)]
pub enum CameraKind {
    /// The Mac's own camera.
    BuiltIn,
    /// A USB or Thunderbolt webcam, or a display's camera.
    External,
    /// An iPhone used as a webcam.
    Continuity,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CameraDevice {
    /// AVFoundation's unique ID; stays the same across reconnections.
    pub id: String,
    pub name: String,
    pub kind: CameraKind,
    /// The camera the preview shows.
    pub selected: bool,
}

/// Whether Jaycast may use the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CameraAccess {
    /// Not asked yet; `request_camera_access` shows the system prompt.
    NotDetermined,
    /// Turned off by a device management profile or parental controls.
    Restricted,
    /// Refused; only System Settings → Privacy & Security → Camera can change it.
    Denied,
    Authorized,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    device: Option<String>,
}

pub(crate) struct Cameras {
    path: PathBuf,
    /// The camera last chosen, kept while it's disconnected so it's chosen again when it
    /// comes back.
    chosen: Mutex<Option<String>>,
}

impl Cameras {
    /// Loads the chosen camera from `data_dir`; a missing or unreadable file means none.
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("camera.json");
        let chosen = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Saved>(&bytes)
                    .unwrap_or_else(|error| {
                        log::warn!("ignoring unreadable camera choice: {error}");
                        Saved::default()
                    })
                    .device
            }
            Err(_) => None,
        };
        Self {
            path,
            chosen: Mutex::new(chosen),
        }
    }

    /// Connected cameras, built-in first, with the one the preview shows marked.
    pub fn list(&self) -> Vec<CameraDevice> {
        let mut devices = connected();
        sort(&mut devices);
        let preferred = system_preferred();
        if let Some(index) = resolve(
            &devices,
            self.chosen.lock().as_deref(),
            preferred.as_deref(),
        ) {
            devices[index].selected = true;
        }
        devices
    }

    /// The camera the preview shows: the one last chosen if it's connected, otherwise the
    /// system's preferred camera, otherwise the first. `None` without any camera.
    pub fn current(&self) -> Option<CameraDevice> {
        self.list().into_iter().find(|device| device.selected)
    }

    /// Makes `id` the camera the preview shows, and remembers it.
    pub fn select(&self, id: &str) -> Result<(), CoreError> {
        let mut chosen = self.chosen.lock();
        if chosen.as_deref() == Some(id) {
            return Ok(());
        }
        *chosen = Some(id.to_owned());
        log::info!("camera preview switched to {id}");
        self.save(chosen.as_deref())
    }

    /// Writes atomically, like the config.
    fn save(&self, device: Option<&str>) -> Result<(), CoreError> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let saved = Saved {
            device: device.map(str::to_owned),
        };
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&saved)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Built-in first, then external, then iPhones, each by name.
fn sort(devices: &mut [CameraDevice]) {
    devices.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
}

/// The index of the camera to show: `chosen` if connected, else `preferred`, else the first.
fn resolve(
    devices: &[CameraDevice],
    chosen: Option<&str>,
    preferred: Option<&str>,
) -> Option<usize> {
    let find = |id: Option<&str>| id.and_then(|id| devices.iter().position(|d| d.id == id));
    find(chosen)
        .or_else(|| find(preferred))
        .or((!devices.is_empty()).then_some(0))
}

/// Video cameras macOS currently has, in the order it lists them. Desk View is left out:
/// it's a second, downward view of a Continuity Camera that's already listed.
fn connected() -> Vec<CameraDevice> {
    // SAFETY: the device type and media type statics are AVFoundation's own constants,
    // which live for the whole process; the discovery session is safe to use from any
    // thread.
    let devices = unsafe {
        let types: Retained<NSArray<AVCaptureDeviceType>> = NSArray::from_slice(&[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeContinuityCamera,
        ]);
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            AVMediaTypeVideo,
            AVCaptureDevicePosition::Unspecified,
        )
        .devices()
    };
    devices.iter().map(|device| describe(&device)).collect()
}

fn describe(device: &AVCaptureDevice) -> CameraDevice {
    // SAFETY: plain property reads on a device the discovery session returned.
    unsafe {
        // An iPhone reports itself as an external camera on macOS; only this tells it apart.
        let kind = if device.isContinuityCamera() {
            CameraKind::Continuity
        } else if *device.deviceType() == *AVCaptureDeviceTypeBuiltInWideAngleCamera {
            CameraKind::BuiltIn
        } else {
            CameraKind::External
        };
        CameraDevice {
            id: device.uniqueID().to_string(),
            name: device.localizedName().to_string(),
            kind,
            selected: false,
        }
    }
}

/// The camera macOS picks by default (the user's choice in apps like FaceTime, or the one
/// it judges best, such as a nearby iPhone), if any.
fn system_preferred() -> Option<String> {
    // SAFETY: a class property read with no preconditions.
    unsafe { AVCaptureDevice::systemPreferredCamera() }.map(|device| {
        // SAFETY: a plain property read.
        unsafe { device.uniqueID() }.to_string()
    })
}

pub(crate) fn access() -> CameraAccess {
    let Some(video) = (unsafe { AVMediaTypeVideo }) else {
        return CameraAccess::Restricted;
    };
    // SAFETY: a class method with no preconditions; `video` is AVFoundation's constant.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) } {
        AVAuthorizationStatus::Authorized => CameraAccess::Authorized,
        AVAuthorizationStatus::Denied => CameraAccess::Denied,
        AVAuthorizationStatus::Restricted => CameraAccess::Restricted,
        _ => CameraAccess::NotDetermined,
    }
}

/// Shows the system's camera prompt if it hasn't been answered, and resolves to whether
/// access is granted (straight away when it was answered before). The app must have
/// `NSCameraUsageDescription` in its Info.plist, or macOS ends the process instead.
pub(crate) fn request_access() -> AccessRequest {
    let shared = Arc::new(Mutex::new(Answer::default()));
    let Some(video) = (unsafe { AVMediaTypeVideo }) else {
        shared.lock().granted = Some(false);
        return AccessRequest(shared);
    };
    let answer = Arc::clone(&shared);
    let handler = RcBlock::new(move |granted: Bool| {
        let waker = {
            let mut answer = answer.lock();
            answer.granted = Some(granted.as_bool());
            answer.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    });
    // SAFETY: AVFoundation copies the block and calls it once, on a queue of its own.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(video, &handler) };
    AccessRequest(shared)
}

#[derive(Default)]
struct Answer {
    granted: Option<bool>,
    waker: Option<Waker>,
}

/// Resolves once the camera prompt is answered.
pub(crate) struct AccessRequest(Arc<Mutex<Answer>>);

impl Future for AccessRequest {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<bool> {
        let mut answer = self.0.lock();
        match answer.granted {
            Some(granted) => Poll::Ready(granted),
            None => {
                answer.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(id: &str, name: &str, kind: CameraKind) -> CameraDevice {
        CameraDevice {
            id: id.into(),
            name: name.into(),
            kind,
            selected: false,
        }
    }

    fn cameras() -> Vec<CameraDevice> {
        let mut devices = vec![
            camera("iphone", "Jamie’s iPhone Camera", CameraKind::Continuity),
            camera("studio", "Studio Display Camera", CameraKind::External),
            camera("facetime", "FaceTime HD Camera", CameraKind::BuiltIn),
            camera("brio", "Logitech BRIO", CameraKind::External),
        ];
        sort(&mut devices);
        devices
    }

    fn ids(devices: &[CameraDevice]) -> Vec<&str> {
        devices.iter().map(|d| d.id.as_str()).collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jaycast-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn built_in_comes_first_then_external_then_iphones() {
        assert_eq!(ids(&cameras()), ["facetime", "brio", "studio", "iphone"]);
    }

    #[test]
    fn chosen_camera_wins_when_connected() {
        let devices = cameras();
        assert_eq!(resolve(&devices, Some("studio"), Some("iphone")), Some(2));
    }

    #[test]
    fn falls_back_to_the_system_choice_then_the_first() {
        let devices = cameras();
        assert_eq!(
            resolve(&devices, Some("unplugged"), Some("iphone")),
            Some(3)
        );
        assert_eq!(resolve(&devices, Some("unplugged"), None), Some(0));
        assert_eq!(resolve(&devices, None, Some("gone")), Some(0));
        assert_eq!(resolve(&[], Some("studio"), Some("iphone")), None);
    }

    #[test]
    fn choice_survives_a_restart() {
        let dir = temp_dir("camera-choice");
        let first = Cameras::new(&dir);
        assert_eq!(*first.chosen.lock(), None);
        first.select("brio").unwrap();
        assert_eq!(*Cameras::new(&dir).chosen.lock(), Some("brio".into()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreadable_choice_starts_empty() {
        let dir = temp_dir("camera-unreadable");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("camera.json"), "not json").unwrap();
        assert_eq!(*Cameras::new(&dir).chosen.lock(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Touches the real machine: lists whatever cameras are connected.
    #[test]
    #[ignore]
    fn lists_connected_cameras() {
        let dir = temp_dir("camera-list");
        let cameras = Cameras::new(&dir);
        let list = cameras.list();
        println!("{list:#?}\naccess: {:?}", access());
        assert!(list.iter().filter(|d| d.selected).count() <= 1);
    }

    #[test]
    fn access_request_resolves_once_answered() {
        let answer = Arc::new(Mutex::new(Answer::default()));
        let mut request = AccessRequest(Arc::clone(&answer));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert_eq!(Pin::new(&mut request).poll(&mut cx), Poll::Pending);
        assert!(answer.lock().waker.is_some());
        answer.lock().granted = Some(true);
        assert_eq!(Pin::new(&mut request).poll(&mut cx), Poll::Ready(true));
    }
}
