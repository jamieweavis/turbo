//! Where a dragged window goes: pure, so it's unit-tested without moving anything.
//!
//! A move keeps the grab point under the cursor. A resize always moves the bottom-right
//! corner, wherever the window was grabbed, and keeps the top-left corner where it was.

use super::super::layout::Rect;

/// Smaller than this and a window is hard to grab again; apps usually stop sooner.
pub(super) const MIN_SIZE: (f64, f64) = (100.0, 60.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Move,
    Resize,
}

/// `frame` moved by the cursor's travel `(dx, dy)` since the grab.
pub(super) fn moved(frame: Rect, (dx, dy): (f64, f64)) -> Rect {
    Rect::new(frame.x + dx, frame.y + dy, frame.w, frame.h)
}

/// `frame` with its bottom-right corner moved by the cursor's travel `(dx, dy)` since the
/// grab, never below [`MIN_SIZE`]. A window already smaller than that isn't made to jump up
/// to it.
pub(super) fn resized(frame: Rect, (dx, dy): (f64, f64)) -> Rect {
    let w = (frame.w + dx).max(MIN_SIZE.0.min(frame.w));
    let h = (frame.h + dy).max(MIN_SIZE.1.min(frame.h));
    Rect::new(frame.x, frame.y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect::new(100.0, 100.0, 300.0, 300.0);

    #[test]
    fn move_follows_the_cursor() {
        assert_eq!(
            moved(FRAME, (-40.0, 25.0)),
            Rect::new(60.0, 125.0, 300.0, 300.0)
        );
    }

    #[test]
    fn resize_moves_the_bottom_right_and_keeps_the_top_left() {
        let after = resized(FRAME, (30.0, -40.0));
        assert_eq!(after, Rect::new(100.0, 100.0, 330.0, 260.0));
    }

    #[test]
    fn never_below_the_minimum_and_the_top_left_holds() {
        let after = resized(FRAME, (-1000.0, -1000.0));
        assert_eq!(after, Rect::new(100.0, 100.0, MIN_SIZE.0, MIN_SIZE.1));
    }

    #[test]
    fn a_tiny_window_isnt_grown_to_the_minimum() {
        let tiny = Rect::new(0.0, 0.0, 50.0, 40.0);
        assert_eq!(resized(tiny, (-10.0, -10.0)), tiny);
    }
}
