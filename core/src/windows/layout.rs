//! Where a window goes for each action: pure geometry, no AppKit or Accessibility.
//!
//! Everything is in Accessibility coordinates: global points with the origin at the top-left
//! of the primary display and y growing downwards. Snapped cells are fractions of the
//! display's visible frame inset by the gap, laid out so neighbouring cells share a single
//! gap. Frames are rounded to whole points by rounding edges (not sizes), so neighbours stay
//! exactly one gap apart.

use crate::config::WindowAction;

/// Within this many points, a window is still where Jaycast put it. Apps nudge frames by a
/// point or so (rounding, backing scale), so exact equality would break cycling.
const SAME_FRAME_TOLERANCE: f64 = 2.0;

/// Widths (or heights) a repeated half cycles through.
const HALF_CYCLE: [f64; 3] = [1.0 / 2.0, 2.0 / 3.0, 1.0 / 3.0];

/// Share of the visible frame an almost-maximised window takes, in each dimension.
const ALMOST_MAXIMIZE: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    fn right(&self) -> f64 {
        self.x + self.w
    }

    fn bottom(&self) -> f64 {
        self.y + self.h
    }

    fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn contains_point(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    fn intersection_area(&self, other: &Rect) -> f64 {
        let w = self.right().min(other.right()) - self.x.max(other.x);
        let h = self.bottom().min(other.bottom()) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }

    /// Shrunk by `by` on every side, never below zero size.
    fn inset(&self, by: f64) -> Rect {
        let dx = by.min(self.w / 2.0);
        let dy = by.min(self.h / 2.0);
        Rect::new(
            self.x + dx,
            self.y + dy,
            self.w - 2.0 * dx,
            self.h - 2.0 * dy,
        )
    }

    /// Edges rounded to whole points.
    fn rounded(&self) -> Rect {
        let (x, y) = (self.x.round(), self.y.round());
        Rect::new(x, y, self.right().round() - x, self.bottom().round() - y)
    }

    pub fn approx_eq(&self, other: &Rect) -> bool {
        (self.x - other.x).abs() <= SAME_FRAME_TOLERANCE
            && (self.y - other.y).abs() <= SAME_FRAME_TOLERANCE
            && (self.w - other.w).abs() <= SAME_FRAME_TOLERANCE
            && (self.h - other.h).abs() <= SAME_FRAME_TOLERANCE
    }
}

/// A display: its whole frame (for deciding which display a window is on) and its visible
/// frame (below the menu bar, beside the Dock: where windows are placed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Screen {
    pub frame: Rect,
    pub visible: Rect,
}

/// Converts an AppKit rect (origin at the bottom-left of the primary display, y up) to
/// Accessibility coordinates.
pub(super) fn flip(rect: Rect, primary_height: f64) -> Rect {
    Rect::new(rect.x, primary_height - rect.y - rect.h, rect.w, rect.h)
}

/// The display showing the largest part of `window`, else the one under its centre, else
/// the primary (index 0).
pub(super) fn screen_for(window: &Rect, screens: &[Screen]) -> usize {
    let best = screens
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.frame.intersection_area(window)))
        .filter(|&(_, area)| area > 0.0)
        // Ties go to the earlier screen, so the result doesn't depend on float noise order.
        .fold(None, |best: Option<(usize, f64)>, (i, area)| match best {
            Some((_, a)) if a >= area => best,
            _ => Some((i, area)),
        });
    if let Some((i, _)) = best {
        return i;
    }
    screens
        .iter()
        .position(|s| s.frame.contains_point(window.center()))
        .unwrap_or(0)
}

/// Screen indices left to right, then top to bottom: the order next/previous display walk.
pub(super) fn display_order(screens: &[Screen]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..screens.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&screens[a].frame, &screens[b].frame);
        a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y))
    });
    order
}

/// A snap Jaycast made: the action, and for halves which step of the 1/2 → 2/3 → 1/3 cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Snap {
    pub action: WindowAction,
    pub step: usize,
}

/// What Jaycast remembers about one window.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Memory {
    /// The frame before Jaycast first moved it, for `Restore`.
    pub original: Option<Rect>,
    /// The last snap and the frame the window actually ended up with.
    pub last: Option<(Snap, Rect)>,
}

impl Memory {
    /// The last snap, if the window hasn't been moved or resized since.
    fn still_snapped(&self, current: &Rect) -> Option<Snap> {
        self.last
            .filter(|(_, frame)| frame.approx_eq(current))
            .map(|(snap, _)| snap)
    }

    /// Records a move from `before` to `after`.
    pub fn record(&mut self, before: Rect, snap: Option<Snap>, after: Rect) {
        self.original.get_or_insert(before);
        self.last = snap.map(|snap| (snap, after));
    }
}

/// Which side of the target a window that can't be resized to fit is aligned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Start,
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Plan {
    pub frame: Rect,
    /// Remembered for cycling and for re-snapping on another display. `None` for restores
    /// and free moves between displays.
    pub snap: Option<Snap>,
    /// Restore forgets the window afterwards.
    pub forget: bool,
    /// Where a window that refuses `frame`'s size is kept (the visible frame, inset by the
    /// gap), and how it's aligned within `frame`.
    pub bounds: Rect,
    pub align: (Align, Align),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlanError {
    NothingToRestore,
    OneDisplay,
}

/// The target for `action` on a window at `window`.
pub(super) fn plan(
    action: WindowAction,
    window: Rect,
    screens: &[Screen],
    memory: &Memory,
    gap: f64,
) -> Result<Plan, PlanError> {
    let current = screen_for(&window, screens);
    match action {
        WindowAction::Restore => {
            let frame = memory
                .original
                .ok_or(PlanError::NothingToRestore)?
                .rounded();
            Ok(Plan {
                frame,
                snap: None,
                forget: true,
                bounds: frame,
                align: (Align::Start, Align::Start),
            })
        }
        WindowAction::NextDisplay | WindowAction::PreviousDisplay => {
            let order = display_order(screens);
            if order.len() < 2 {
                return Err(PlanError::OneDisplay);
            }
            let at = order.iter().position(|&i| i == current).unwrap_or(0);
            let next = if action == WindowAction::NextDisplay {
                (at + 1) % order.len()
            } else {
                (at + order.len() - 1) % order.len()
            };
            let target = &screens[order[next]];
            // A window still where Jaycast snapped it is snapped the same way over there;
            // anything else keeps its relative position and size.
            match memory.still_snapped(&window) {
                Some(snap) => Ok(snap_plan(snap, &window, target, gap)),
                None => {
                    let frame = relative_move(&window, &screens[current].visible, &target.visible);
                    Ok(Plan {
                        frame,
                        snap: None,
                        forget: false,
                        bounds: target.visible,
                        align: (Align::Middle, Align::Middle),
                    })
                }
            }
        }
        _ => {
            let step = match memory.still_snapped(&window) {
                Some(last) if last.action == action && is_cycling_half(action) => {
                    (last.step + 1) % HALF_CYCLE.len()
                }
                _ => 0,
            };
            Ok(snap_plan(
                Snap { action, step },
                &window,
                &screens[current],
                gap,
            ))
        }
    }
}

fn is_cycling_half(action: WindowAction) -> bool {
    matches!(
        action,
        WindowAction::LeftHalf
            | WindowAction::RightHalf
            | WindowAction::TopHalf
            | WindowAction::BottomHalf
    )
}

/// The frame for a positional action on `screen`.
fn snap_plan(snap: Snap, window: &Rect, screen: &Screen, gap: f64) -> Plan {
    use Align::*;
    use WindowAction::*;
    let area = screen.visible.inset(gap);
    let half = HALF_CYCLE[snap.step % HALF_CYCLE.len()];
    let third = 1.0 / 3.0;
    // Horizontal and vertical extents as fractions of the area: (start, end).
    let full = (0.0, 1.0);
    let (columns, rows, align) = match snap.action {
        LeftHalf => ((0.0, half), full, (Start, Middle)),
        RightHalf => ((1.0 - half, 1.0), full, (End, Middle)),
        TopHalf => (full, (0.0, half), (Middle, Start)),
        BottomHalf => (full, (1.0 - half, 1.0), (Middle, End)),
        TopLeft => ((0.0, 0.5), (0.0, 0.5), (Start, Start)),
        TopRight => ((0.5, 1.0), (0.0, 0.5), (End, Start)),
        BottomLeft => ((0.0, 0.5), (0.5, 1.0), (Start, End)),
        BottomRight => ((0.5, 1.0), (0.5, 1.0), (End, End)),
        LeftThird => ((0.0, third), full, (Start, Middle)),
        CenterThird => ((third, 2.0 * third), full, (Middle, Middle)),
        RightThird => ((2.0 * third, 1.0), full, (End, Middle)),
        LeftTwoThirds => ((0.0, 2.0 * third), full, (Start, Middle)),
        RightTwoThirds => ((third, 1.0), full, (End, Middle)),
        TopLeftSixth => ((0.0, third), (0.0, 0.5), (Start, Start)),
        TopCenterSixth => ((third, 2.0 * third), (0.0, 0.5), (Middle, Start)),
        TopRightSixth => ((2.0 * third, 1.0), (0.0, 0.5), (End, Start)),
        BottomLeftSixth => ((0.0, third), (0.5, 1.0), (Start, End)),
        BottomCenterSixth => ((third, 2.0 * third), (0.5, 1.0), (Middle, End)),
        BottomRightSixth => ((2.0 * third, 1.0), (0.5, 1.0), (End, End)),
        Maximize => (full, full, (Middle, Middle)),
        AlmostMaximize => {
            let margin = (1.0 - ALMOST_MAXIMIZE) / 2.0;
            let inner = Rect::new(
                area.x + area.w * margin,
                area.y + area.h * margin,
                area.w * ALMOST_MAXIMIZE,
                area.h * ALMOST_MAXIMIZE,
            );
            return Plan {
                frame: inner.rounded(),
                snap: Some(snap),
                forget: false,
                bounds: area,
                align: (Middle, Middle),
            };
        }
        Center => {
            let w = window.w.min(area.w);
            let h = window.h.min(area.h);
            let frame = Rect::new(
                area.x + (area.w - w) / 2.0,
                area.y + (area.h - h) / 2.0,
                w,
                h,
            );
            return Plan {
                frame: frame.rounded(),
                snap: Some(snap),
                forget: false,
                bounds: area,
                align: (Middle, Middle),
            };
        }
        Restore | NextDisplay | PreviousDisplay => unreachable!("handled by plan"),
    };
    Plan {
        frame: cell(&area, gap, columns, rows).rounded(),
        snap: Some(snap),
        forget: false,
        bounds: area,
        align,
    }
}

/// The part of `area` between fractional edges, with one `gap` between neighbouring cells:
/// fractions are of `area + gap`, and each cell gives up a gap at its far edge.
fn cell(area: &Rect, gap: f64, (x0, x1): (f64, f64), (y0, y1): (f64, f64)) -> Rect {
    let (w, h) = (area.w + gap, area.h + gap);
    let left = area.x + x0 * w;
    let top = area.y + y0 * h;
    let right = area.x + x1 * w - gap;
    let bottom = area.y + y1 * h - gap;
    Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
}

/// Same position and size relative to the visible frame, clamped to fit the new one.
fn relative_move(window: &Rect, from: &Rect, to: &Rect) -> Rect {
    let sx = if from.w > 0.0 { to.w / from.w } else { 1.0 };
    let sy = if from.h > 0.0 { to.h / from.h } else { 1.0 };
    let frame = Rect::new(
        to.x + (window.x - from.x) * sx,
        to.y + (window.y - from.y) * sy,
        window.w * sx,
        window.h * sy,
    );
    clamp(&frame, to).rounded()
}

/// Shrunk to fit `bounds`, then moved inside it.
fn clamp(frame: &Rect, bounds: &Rect) -> Rect {
    let w = frame.w.min(bounds.w);
    let h = frame.h.min(bounds.h);
    let x = frame.x.min(bounds.right() - w).max(bounds.x);
    let y = frame.y.min(bounds.bottom() - h).max(bounds.y);
    Rect::new(x, y, w, h)
}

/// Where a window of `size` goes when it can't be resized to `plan.frame`: aligned within
/// the target by the plan's anchors, then kept inside the visible frame where it fits.
pub(super) fn place_fixed_size(plan: &Plan, (w, h): (f64, f64)) -> Rect {
    let along = |start: f64, length: f64, size: f64, align: Align| match align {
        Align::Start => start,
        Align::Middle => start + (length - size) / 2.0,
        Align::End => start + length - size,
    };
    let frame = Rect::new(
        along(plan.frame.x, plan.frame.w, w, plan.align.0),
        along(plan.frame.y, plan.frame.h, h, plan.align.1),
        w,
        h,
    );
    // Clamp the position only: the window's size is what it is.
    let b = &plan.bounds;
    let x = if w <= b.w {
        frame.x.min(b.right() - w).max(b.x)
    } else {
        b.x
    };
    let y = if h <= b.h {
        frame.y.min(b.bottom() - h).max(b.y)
    } else {
        b.y
    };
    Rect::new(x.round(), y.round(), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use WindowAction::*;

    /// A 1440×900 laptop display with a 25 pt menu bar and no Dock on screen.
    const LAPTOP: Screen = Screen {
        frame: Rect::new(0.0, 0.0, 1440.0, 900.0),
        visible: Rect::new(0.0, 25.0, 1440.0, 875.0),
    };
    /// A 2560×1440 display to the right of the laptop, top-aligned.
    const EXTERNAL: Screen = Screen {
        frame: Rect::new(1440.0, 0.0, 2560.0, 1440.0),
        visible: Rect::new(1440.0, 25.0, 2560.0, 1415.0),
    };
    /// A 1920×1080 display to the left of the laptop, lower down.
    const LEFT: Screen = Screen {
        frame: Rect::new(-1920.0, 200.0, 1920.0, 1080.0),
        visible: Rect::new(-1920.0, 200.0, 1920.0, 1080.0),
    };

    const WINDOW: Rect = Rect::new(100.0, 100.0, 800.0, 600.0);

    fn frame(action: WindowAction, window: Rect, gap: f64) -> Rect {
        plan(action, window, &[LAPTOP], &Memory::default(), gap)
            .unwrap()
            .frame
    }

    /// Plans `action`, then remembers it as `WindowManager` does (assuming the window took
    /// the frame exactly), returning the new frame.
    fn apply(action: WindowAction, window: Rect, screens: &[Screen], memory: &mut Memory) -> Rect {
        let plan = plan(action, window, screens, memory, 0.0).unwrap();
        if plan.forget {
            *memory = Memory::default();
        } else {
            memory.record(window, plan.snap, plan.frame);
        }
        plan.frame
    }

    #[test]
    fn halves_split_the_visible_frame() {
        assert_eq!(
            frame(LeftHalf, WINDOW, 0.0),
            Rect::new(0.0, 25.0, 720.0, 875.0)
        );
        assert_eq!(
            frame(RightHalf, WINDOW, 0.0),
            Rect::new(720.0, 25.0, 720.0, 875.0)
        );
        assert_eq!(
            frame(TopHalf, WINDOW, 0.0),
            Rect::new(0.0, 25.0, 1440.0, 438.0)
        );
        assert_eq!(
            frame(BottomHalf, WINDOW, 0.0),
            Rect::new(0.0, 463.0, 1440.0, 437.0)
        );
    }

    #[test]
    fn quarters_and_maximize() {
        assert_eq!(
            frame(TopLeft, WINDOW, 0.0),
            Rect::new(0.0, 25.0, 720.0, 438.0)
        );
        assert_eq!(
            frame(TopRight, WINDOW, 0.0),
            Rect::new(720.0, 25.0, 720.0, 438.0)
        );
        assert_eq!(
            frame(BottomLeft, WINDOW, 0.0),
            Rect::new(0.0, 463.0, 720.0, 437.0)
        );
        assert_eq!(
            frame(BottomRight, WINDOW, 0.0),
            Rect::new(720.0, 463.0, 720.0, 437.0)
        );
        assert_eq!(frame(Maximize, WINDOW, 0.0), LAPTOP.visible);
    }

    #[test]
    fn thirds_tile_without_overlap() {
        assert_eq!(
            frame(LeftThird, WINDOW, 0.0),
            Rect::new(0.0, 25.0, 480.0, 875.0)
        );
        assert_eq!(
            frame(CenterThird, WINDOW, 0.0),
            Rect::new(480.0, 25.0, 480.0, 875.0)
        );
        assert_eq!(
            frame(RightThird, WINDOW, 0.0),
            Rect::new(960.0, 25.0, 480.0, 875.0)
        );
        assert_eq!(
            frame(LeftTwoThirds, WINDOW, 0.0),
            Rect::new(0.0, 25.0, 960.0, 875.0)
        );
        assert_eq!(
            frame(RightTwoThirds, WINDOW, 0.0),
            Rect::new(480.0, 25.0, 960.0, 875.0)
        );
    }

    #[test]
    fn sixths_tile_two_rows_of_three() {
        assert_eq!(
            frame(TopLeftSixth, WINDOW, 0.0),
            Rect::new(0.0, 25.0, 480.0, 438.0)
        );
        assert_eq!(
            frame(TopCenterSixth, WINDOW, 0.0),
            Rect::new(480.0, 25.0, 480.0, 438.0)
        );
        assert_eq!(
            frame(TopRightSixth, WINDOW, 0.0),
            Rect::new(960.0, 25.0, 480.0, 438.0)
        );
        assert_eq!(
            frame(BottomLeftSixth, WINDOW, 0.0),
            Rect::new(0.0, 463.0, 480.0, 437.0)
        );
        assert_eq!(
            frame(BottomCenterSixth, WINDOW, 0.0),
            Rect::new(480.0, 463.0, 480.0, 437.0)
        );
        assert_eq!(
            frame(BottomRightSixth, WINDOW, 0.0),
            Rect::new(960.0, 463.0, 480.0, 437.0)
        );

        // With a gap, neighbours in a row and across rows sit one gap apart.
        let gap = 10.0;
        let top = [TopLeftSixth, TopCenterSixth, TopRightSixth].map(|a| frame(a, WINDOW, gap));
        let bottom_left = frame(BottomLeftSixth, WINDOW, gap);
        assert_eq!(top[0].x, gap);
        assert!((top[1].x - top[0].right() - gap).abs() <= 1.0);
        assert!((top[2].x - top[1].right() - gap).abs() <= 1.0);
        assert_eq!(1440.0 - top[2].right(), gap);
        assert_eq!(bottom_left.y - top[0].bottom(), gap);
        assert_eq!(900.0 - bottom_left.bottom(), gap);
    }

    #[test]
    fn gap_surrounds_windows_and_separates_neighbours_once() {
        let gap = 10.0;
        let left = frame(LeftHalf, WINDOW, gap);
        let right = frame(RightHalf, WINDOW, gap);
        assert_eq!(left, Rect::new(10.0, 35.0, 705.0, 855.0));
        assert_eq!(right, Rect::new(725.0, 35.0, 705.0, 855.0));
        assert_eq!(right.x - left.right(), gap);
        assert_eq!(1440.0 - right.right(), gap);

        assert_eq!(
            frame(Maximize, WINDOW, gap),
            Rect::new(10.0, 35.0, 1420.0, 855.0)
        );

        let top_left = frame(TopLeft, WINDOW, gap);
        let bottom_left = frame(BottomLeft, WINDOW, gap);
        assert_eq!(bottom_left.y - top_left.bottom(), gap);
        assert_eq!(900.0 - bottom_left.bottom(), gap);

        // Thirds: each neighbour exactly one gap apart, outer edges one gap from the screen.
        let thirds = [LeftThird, CenterThird, RightThird].map(|a| frame(a, WINDOW, gap));
        assert_eq!(thirds[0].x, gap);
        assert!((thirds[1].x - thirds[0].right() - gap).abs() <= 1.0);
        assert!((thirds[2].x - thirds[1].right() - gap).abs() <= 1.0);
        assert_eq!(1440.0 - thirds[2].right(), gap);
        // Two thirds meets the opposite third with a single gap.
        let two = frame(LeftTwoThirds, WINDOW, gap);
        assert!((thirds[2].x - two.right() - gap).abs() <= 1.0);
    }

    #[test]
    fn frames_are_whole_points() {
        let screen = Screen {
            frame: Rect::new(0.0, 0.0, 1001.0, 701.0),
            visible: Rect::new(0.0, 23.0, 1001.0, 678.0),
        };
        for &(action, ..) in super::super::ACTIONS {
            if matches!(action, Restore | NextDisplay | PreviousDisplay) {
                continue;
            }
            let window = Rect::new(13.3, 47.7, 333.3, 222.2);
            let f = plan(action, window, &[screen], &Memory::default(), 7.0)
                .unwrap()
                .frame;
            for v in [f.x, f.y, f.w, f.h] {
                assert_eq!(v, v.round(), "{action:?} gave {f:?}");
            }
        }
    }

    #[test]
    fn repeated_halves_cycle_widths() {
        let mut memory = Memory::default();
        let mut window = WINDOW;
        let mut widths = Vec::new();
        for _ in 0..4 {
            window = apply(LeftHalf, window, &[LAPTOP], &mut memory);
            assert_eq!(window.x, 0.0);
            widths.push(window.w);
        }
        assert_eq!(widths, [720.0, 960.0, 480.0, 720.0]);

        let mut memory = Memory::default();
        let mut window = WINDOW;
        let mut edges = Vec::new();
        for _ in 0..3 {
            window = apply(RightHalf, window, &[LAPTOP], &mut memory);
            edges.push((window.x, window.right()));
        }
        assert_eq!(edges, [(720.0, 1440.0), (480.0, 1440.0), (960.0, 1440.0)]);
    }

    #[test]
    fn repeated_vertical_halves_cycle_heights() {
        let mut memory = Memory::default();
        let mut window = WINDOW;
        let mut heights = Vec::new();
        for _ in 0..3 {
            window = apply(BottomHalf, window, &[LAPTOP], &mut memory);
            assert_eq!(window.bottom(), 900.0);
            heights.push(window.h);
        }
        assert_eq!(heights, [437.0, 583.0, 292.0]);
    }

    #[test]
    fn cycling_needs_the_same_action_and_an_untouched_window() {
        let mut memory = Memory::default();
        let left = apply(LeftHalf, WINDOW, &[LAPTOP], &mut memory);
        // A different half starts at 1/2.
        let right = apply(RightHalf, left, &[LAPTOP], &mut memory);
        assert_eq!(right.w, 720.0);
        // The user nudged the window: start over at 1/2.
        let moved = Rect::new(right.x - 50.0, right.y, right.w, right.h);
        assert_eq!(apply(RightHalf, moved, &[LAPTOP], &mut memory).w, 720.0);
        // Within the tolerance it still cycles (apps round frames).
        let nudged = Rect::new(720.0 + 1.0, 25.0, 719.0, 875.0);
        assert_eq!(apply(RightHalf, nudged, &[LAPTOP], &mut memory).w, 960.0);
    }

    #[test]
    fn non_halves_do_not_cycle() {
        let mut memory = Memory::default();
        let first = apply(LeftThird, WINDOW, &[LAPTOP], &mut memory);
        let second = apply(LeftThird, first, &[LAPTOP], &mut memory);
        assert_eq!(first, second);
    }

    #[test]
    fn almost_maximize_is_centred_and_inset() {
        let f = frame(AlmostMaximize, WINDOW, 0.0);
        assert_eq!(f, Rect::new(72.0, 69.0, 1296.0, 787.0));
        assert_eq!(f.x, 1440.0 - f.right());
    }

    #[test]
    fn center_keeps_size_and_clamps_to_the_visible_frame() {
        assert_eq!(
            frame(Center, WINDOW, 0.0),
            Rect::new(320.0, 163.0, 800.0, 600.0)
        );
        let huge = Rect::new(-50.0, 0.0, 2000.0, 1000.0);
        assert_eq!(frame(Center, huge, 0.0), LAPTOP.visible);
        assert_eq!(
            frame(Center, huge, 10.0),
            Rect::new(10.0, 35.0, 1420.0, 855.0)
        );
    }

    #[test]
    fn restore_returns_to_the_frame_before_jaycast_and_forgets() {
        let mut memory = Memory::default();
        let snapped = apply(LeftHalf, WINDOW, &[LAPTOP], &mut memory);
        let snapped = apply(TopRight, snapped, &[LAPTOP], &mut memory);
        let snapped = apply(Maximize, snapped, &[LAPTOP], &mut memory);
        assert_eq!(apply(Restore, snapped, &[LAPTOP], &mut memory), WINDOW);
        assert_eq!(memory, Memory::default());
        assert_eq!(
            plan(Restore, WINDOW, &[LAPTOP], &memory, 0.0),
            Err(PlanError::NothingToRestore)
        );
    }

    #[test]
    fn flips_appkit_coordinates() {
        // The external display's top is level with the laptop's, so in AppKit its origin is
        // 540 below the laptop's (1440 - 900).
        let appkit = Rect::new(1440.0, -540.0, 2560.0, 1440.0);
        assert_eq!(flip(appkit, 900.0), EXTERNAL.frame);
        // The laptop's visible frame: the menu bar's 25 points are cut from the top.
        assert_eq!(
            flip(Rect::new(0.0, 0.0, 1440.0, 875.0), 900.0),
            LAPTOP.visible
        );
        // A display above the primary has negative y.
        assert_eq!(
            flip(Rect::new(0.0, 900.0, 1920.0, 1080.0), 900.0),
            Rect::new(0.0, -1080.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn window_is_on_the_screen_with_most_of_it() {
        let screens = [LAPTOP, EXTERNAL];
        assert_eq!(screen_for(&WINDOW, &screens), 0);
        // Straddling, mostly on the external display.
        assert_eq!(
            screen_for(&Rect::new(1340.0, 100.0, 800.0, 600.0), &screens),
            1
        );
        // Mostly on the laptop.
        assert_eq!(
            screen_for(&Rect::new(1000.0, 100.0, 800.0, 600.0), &screens),
            0
        );
        // Off every screen: the primary.
        assert_eq!(
            screen_for(&Rect::new(9000.0, 9000.0, 10.0, 10.0), &screens),
            0
        );
        // Zero-sized, but its centre is on the external display.
        assert_eq!(screen_for(&Rect::new(2000.0, 500.0, 0.0, 0.0), &screens), 1);
    }

    #[test]
    fn snaps_on_the_window_s_display() {
        let window = Rect::new(2000.0, 300.0, 800.0, 600.0);
        let f = plan(
            LeftHalf,
            window,
            &[LAPTOP, EXTERNAL],
            &Memory::default(),
            0.0,
        )
        .unwrap()
        .frame;
        assert_eq!(f, Rect::new(1440.0, 25.0, 1280.0, 1415.0));
    }

    #[test]
    fn displays_are_ordered_left_to_right_then_top_to_bottom() {
        assert_eq!(display_order(&[LAPTOP, EXTERNAL, LEFT]), [2, 0, 1]);
        let below = Screen {
            frame: Rect::new(0.0, 900.0, 1440.0, 900.0),
            visible: Rect::new(0.0, 900.0, 1440.0, 900.0),
        };
        assert_eq!(display_order(&[below, LAPTOP]), [1, 0]);
    }

    #[test]
    fn next_and_previous_display_cycle_through_all() {
        let screens = [LAPTOP, EXTERNAL, LEFT];
        let memory = Memory::default();
        let mut window = WINDOW;
        let mut visited = Vec::new();
        for _ in 0..3 {
            window = plan(NextDisplay, window, &screens, &memory, 0.0)
                .unwrap()
                .frame;
            visited.push(screen_for(&window, &screens));
        }
        assert_eq!(visited, [1, 2, 0]);
        let back = plan(PreviousDisplay, WINDOW, &screens, &memory, 0.0)
            .unwrap()
            .frame;
        assert_eq!(screen_for(&back, &screens), 2);
        assert_eq!(
            plan(NextDisplay, WINDOW, &[LAPTOP], &memory, 0.0),
            Err(PlanError::OneDisplay)
        );
    }

    #[test]
    fn moving_displays_keeps_relative_position_and_size() {
        // Window occupies x 10%..60%, y 20%..60% of the laptop's visible frame.
        let window = Rect::new(144.0, 25.0 + 175.0, 720.0, 350.0);
        let p = plan(
            NextDisplay,
            window,
            &[LAPTOP, EXTERNAL],
            &Memory::default(),
            0.0,
        )
        .unwrap();
        assert_eq!(
            p.frame,
            Rect::new(1440.0 + 256.0, 25.0 + 283.0, 1280.0, 566.0)
        );
        assert_eq!(p.snap, None);
    }

    #[test]
    fn moving_to_a_smaller_display_clamps_to_fit() {
        // Hanging off the bottom-right of the external display.
        let window = Rect::new(3500.0, 1200.0, 900.0, 400.0);
        let f = plan(
            PreviousDisplay,
            window,
            &[LAPTOP, EXTERNAL],
            &Memory::default(),
            0.0,
        )
        .unwrap()
        .frame;
        assert!(f.x >= 0.0 && f.right() <= 1440.0, "{f:?}");
        assert!(f.y >= 25.0 && f.bottom() <= 900.0, "{f:?}");
    }

    #[test]
    fn snapped_windows_stay_snapped_on_the_next_display() {
        let screens = [LAPTOP, EXTERNAL];
        let mut memory = Memory::default();
        let left = apply(LeftHalf, WINDOW, &screens, &mut memory);
        let left = apply(LeftHalf, left, &screens, &mut memory); // 2/3
        let moved = apply(NextDisplay, left, &screens, &mut memory);
        assert_eq!(moved, Rect::new(1440.0, 25.0, 1707.0, 1415.0));
        // Still cycling from where it was: 2/3 → 1/3.
        let next = apply(LeftHalf, moved, &screens, &mut memory);
        assert_eq!(next.w, 853.0);
        // And Restore still knows the very first frame.
        assert_eq!(apply(Restore, next, &screens, &mut memory), WINDOW);
    }

    #[test]
    fn fixed_size_windows_are_aligned_within_the_target() {
        let p = plan(LeftHalf, WINDOW, &[LAPTOP], &Memory::default(), 0.0).unwrap();
        // Narrower than the half: left edge, centred vertically.
        assert_eq!(
            place_fixed_size(&p, (400.0, 300.0)),
            Rect::new(0.0, 313.0, 400.0, 300.0)
        );
        let p = plan(RightHalf, WINDOW, &[LAPTOP], &Memory::default(), 0.0).unwrap();
        // Wider than the half: right edge on the screen edge.
        assert_eq!(
            place_fixed_size(&p, (1000.0, 300.0)),
            Rect::new(440.0, 313.0, 1000.0, 300.0)
        );
        let p = plan(BottomRight, WINDOW, &[LAPTOP], &Memory::default(), 0.0).unwrap();
        assert_eq!(
            place_fixed_size(&p, (300.0, 200.0)),
            Rect::new(1140.0, 700.0, 300.0, 200.0)
        );
        // Bigger than the screen: pinned to the top-left of the visible frame.
        assert_eq!(
            place_fixed_size(&p, (2000.0, 1000.0)),
            Rect::new(0.0, 25.0, 2000.0, 1000.0)
        );
    }

    #[test]
    fn approx_eq_tolerates_a_couple_of_points() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!(a.approx_eq(&Rect::new(2.0, -2.0, 98.0, 102.0)));
        assert!(!a.approx_eq(&Rect::new(3.0, 0.0, 100.0, 100.0)));
    }
}
