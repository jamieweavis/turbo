//! The worker: finds the window under the cursor and moves or resizes it, off the tap thread
//! so a slow app never holds up the mouse. Drags arrive far faster than an app can follow,
//! so only the newest queued position is applied.
//!
//! Jaycast's own windows are the exception: Accessibility calls on them are handled
//! in-process, by AppKit on the calling thread, and AppKit only allows that on the main
//! thread. So a drag of one of those runs its calls on the main queue instead.

use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFNumberType, CFString, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID,
};

use super::super::ax::{self, Element};
use super::super::layout::Rect;
use super::super::memory::WindowKey;
use super::super::{AX_TIMEOUT_SECONDS, WindowManager, frame};
use super::geometry::{self, Mode};

/// From the tap, in order.
#[derive(Debug)]
pub(super) enum Message {
    Begin { point: (f64, f64), mode: Mode },
    Drag { point: (f64, f64) },
    End,
}

/// How far up from the element under the cursor to look for its window.
const MAX_ANCESTORS: usize = 32;

/// How long to wait for the main thread to move one of Jaycast's own windows. Bounded so
/// stopping input from the main thread, which waits for this worker, can't deadlock.
const MAIN_THREAD_WAIT: Duration = Duration::from_millis(500);

/// Runs until the tap drops its sender, then ends any drag in progress.
pub(super) fn run(inbox: Receiver<Message>, windows: &WindowManager) {
    let mut session: Option<Session> = None;
    while let Ok(mut message) = inbox.recv() {
        // Skip to the newest of a run of drags; anything else is handled in order.
        while let Ok(next) = inbox.try_recv() {
            if matches!(message, Message::Drag { .. }) && matches!(next, Message::Drag { .. }) {
                message = next;
            } else {
                handle(message, &mut session, windows);
                message = next;
            }
        }
        handle(message, &mut session, windows);
    }
    finish(session.take(), windows);
}

fn handle(message: Message, session: &mut Option<Session>, windows: &WindowManager) {
    match message {
        Message::Begin { point, mode } => {
            finish(session.take(), windows);
            *session = if is_own_window_at(point) {
                on_main(move || Session::begin(point, mode, true)).flatten()
            } else {
                Session::begin(point, mode, false)
            };
        }
        Message::Drag { point } => {
            *session = session.take().and_then(|mut session| {
                if session.own {
                    on_main(move || {
                        session.drag(point);
                        session
                    })
                } else {
                    session.drag(point);
                    Some(session)
                }
            });
        }
        Message::End => finish(session.take(), windows),
    }
}

/// Ends the drag, and remembers the frame before it so `Restore` can put the window back.
fn finish(session: Option<Session>, windows: &WindowManager) {
    let Some(session) = session else { return };
    let moved = if session.own {
        on_main(move || session.end()).flatten()
    } else {
        session.end()
    };
    if let Some((key, before, after)) = moved {
        windows.remember_drag(key, before, after);
    }
}

/// Runs `work` on the main thread and waits for its result, or for [`MAIN_THREAD_WAIT`].
fn on_main<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (done, result) = mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        let _ = done.send(work());
    });
    result.recv_timeout(MAIN_THREAD_WAIT).ok()
}

/// Whether one of Jaycast's own windows is at `point`, at or above the frontmost normal
/// window there. Asks the window server, not Accessibility, so it's safe off the main thread.
fn is_own_window_at((x, y): (f64, f64)) -> bool {
    let own = i64::from(std::process::id());
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(windows) = CGWindowListCopyWindowInfo(options, 0) else {
        return false;
    };
    // SAFETY: the keys are immutable CoreGraphics globals.
    let (bounds_key, layer_key, owner_key) =
        unsafe { (kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID) };
    // Front to back.
    for index in 0..windows.count() {
        // SAFETY: the window list holds one dictionary per window, alive while the list is.
        let Some(info) = (unsafe {
            windows
                .value_at_index(index)
                .cast::<CFDictionary>()
                .as_ref()
        }) else {
            continue;
        };
        // SAFETY: a window's bounds are a CGRect dictionary.
        let bounds = unsafe { lookup(info, bounds_key).cast::<CFDictionary>().as_ref() };
        let mut rect = CGRect::default();
        // SAFETY: valid dictionary and out pointer.
        if !unsafe { CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) } {
            continue;
        }
        let inside = x >= rect.origin.x
            && x < rect.origin.x + rect.size.width
            && y >= rect.origin.y
            && y < rect.origin.y + rect.size.height;
        if !inside {
            continue;
        }
        if number(info, owner_key) == Some(own) {
            return true;
        }
        if number(info, layer_key) == Some(0) {
            return false;
        }
    }
    false
}

fn lookup(info: &CFDictionary, key: &CFString) -> *const c_void {
    // SAFETY: window info dictionaries are keyed by CFStrings.
    unsafe { info.value((key as *const CFString).cast()) }
}

fn number(info: &CFDictionary, key: &CFString) -> Option<i64> {
    // SAFETY: the window list's layer and owner pid values are CFNumbers.
    let value = unsafe { lookup(info, key).cast::<CFNumber>().as_ref() }?;
    let mut out = 0i64;
    // SAFETY: `out` is an i64, matching SInt64Type.
    unsafe { value.value(CFNumberType::SInt64Type, (&raw mut out).cast()) }.then_some(out)
}

/// One drag of one window.
struct Session {
    pid: i32,
    app: Element,
    window: Element,
    mode: Mode,
    /// The cursor and the window's frame at the grab.
    cursor: (f64, f64),
    start: Rect,
    /// The frame it was last given.
    last: Rect,
    /// The app had `AXEnhancedUserInterface` on, which is off for the drag.
    enhanced: bool,
    /// One of Jaycast's own windows, so only touched on the main thread.
    own: bool,
}

// SAFETY: the AXUIElements are CF objects, safe to retain, release and message from any
// thread. A session on Jaycast's own window is only ever used on the main thread (`own`).
unsafe impl Send for Session {}

impl Session {
    /// Grabs and raises the window under `point`, if it can be moved (and, to resize,
    /// resized). Nothing under the cursor, or a full-screen window, is no drag.
    fn begin(point: (f64, f64), mode: Mode, own: bool) -> Option<Self> {
        let system = Element::system_wide()?;
        system.set_timeout(AX_TIMEOUT_SECONDS);
        let window = window_at(&system, point)?;
        window.set_timeout(AX_TIMEOUT_SECONDS);
        if window.bool(ax::FULL_SCREEN) == Some(true) || !window.is_settable(ax::POSITION) {
            return None;
        }
        if mode == Mode::Resize && !window.is_settable(ax::SIZE) {
            return None;
        }
        let pid = window.pid()?;
        let app = Element::application(pid)?;
        app.set_timeout(AX_TIMEOUT_SECONDS);
        let start = frame(&window).ok()?;

        let _ = app.set_bool(ax::FRONTMOST, true);
        let _ = window.set_bool(ax::MAIN, true);
        let _ = window.perform(ax::RAISE);

        let enhanced = app.bool(ax::ENHANCED_USER_INTERFACE) == Some(true);
        if enhanced {
            let _ = app.set_bool(ax::ENHANCED_USER_INTERFACE, false);
        }
        Some(Self {
            pid,
            app,
            window,
            mode,
            cursor: point,
            start,
            last: start,
            enhanced,
            own,
        })
    }

    fn drag(&mut self, (x, y): (f64, f64)) {
        let delta = (x - self.cursor.0, y - self.cursor.1);
        match self.mode {
            Mode::Move => {
                let target = geometry::moved(self.start, delta);
                if self
                    .window
                    .set_position(CGPoint::new(target.x, target.y))
                    .is_ok()
                {
                    self.last = target;
                }
            }
            Mode::Resize => self.resize(geometry::resized(self.start, delta)),
        }
    }

    /// Only the size changes, so the top-left corner stays where it is.
    fn resize(&mut self, target: Rect) {
        if self
            .window
            .set_size(CGSize::new(target.w, target.h))
            .is_ok()
        {
            self.last = target;
        }
    }

    /// Gives the app back its enhanced UI. Returns the window and its frames before and
    /// after, if it moved.
    fn end(self) -> Option<(WindowKey, Rect, Rect)> {
        if self.enhanced {
            let _ = self.app.set_bool(ax::ENHANCED_USER_INTERFACE, true);
        }
        if self.last.approx_eq(&self.start) {
            return None;
        }
        let id = self.window.window_id()?;
        Some(((self.pid, id), self.start, self.last))
    }
}

/// The window containing the element under `point`.
fn window_at(system: &Element, (x, y): (f64, f64)) -> Option<Element> {
    let mut element = system.element_at(x, y).ok()?;
    if let Ok(window) = element.element(ax::WINDOW) {
        return Some(window);
    }
    for _ in 0..MAX_ANCESTORS {
        if element.string(ax::ROLE).as_deref() == Some(ax::WINDOW_ROLE) {
            return Some(element);
        }
        element = element.element(ax::PARENT).ok()?;
    }
    None
}
