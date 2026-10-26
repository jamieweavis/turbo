//! Running apps, for the palette's actions menu: whether an app is running, hide and quit.

use objc2::rc::Retained;
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::NSString;

use crate::error::CoreError;

pub(crate) fn is_running(bundle_id: &str) -> bool {
    !instances(bundle_id).is_empty()
}

pub(crate) fn hide(bundle_id: &str) -> Result<(), CoreError> {
    for app in running(bundle_id)? {
        app.hide();
    }
    Ok(())
}

/// A normal quit lets the app ask to save (it may refuse); `force` kills it outright.
pub(crate) fn quit(bundle_id: &str, force: bool) -> Result<(), CoreError> {
    for app in running(bundle_id)? {
        let asked = if force {
            app.forceTerminate()
        } else {
            app.terminate()
        };
        if !asked && !app.isTerminated() {
            return Err(CoreError::Other {
                message: format!("couldn't quit {bundle_id}"),
            });
        }
    }
    Ok(())
}

fn running(bundle_id: &str) -> Result<Vec<Retained<NSRunningApplication>>, CoreError> {
    let apps = instances(bundle_id);
    if apps.is_empty() {
        return Err(CoreError::Other {
            message: format!("{bundle_id} isn't running"),
        });
    }
    Ok(apps)
}

fn instances(bundle_id: &str) -> Vec<Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .to_vec()
        .into_iter()
        .filter(|app| !app.isTerminated())
        .collect()
}
