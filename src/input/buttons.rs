//! Button remapping as a pure state machine: no CoreGraphics, no clock.
//!
//! The tap feeds it button downs/ups/drags with timestamps and asks it when the next
//! deadline is (hold threshold, double-click window). It answers with what to do with the
//! event in hand and which side effects to perform. Timing is injected, so every path is
//! unit-testable.

use std::time::{Duration, Instant};

use crate::config::{Action, ButtonBinding, Trigger};

/// How long a button must stay down before a Hold binding fires.
pub(crate) const HOLD_THRESHOLD: Duration = Duration::from_millis(300);
/// How long after a release a second press still counts as a double click.
pub(crate) const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(300);
/// Moving further than this (in points) before anything fires turns the press back into
/// an ordinary drag, so middle-drag (autoscroll, panning) keeps working.
pub(crate) const DRAG_THRESHOLD: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    PassThrough,
    Swallow,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Effect {
    Perform(Action),
    /// Post a full click of this button, because its real down/up were swallowed.
    ReplayClick(u32),
    /// Post this button's down, handing the rest of the press back to the system.
    ReplayDown(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decision {
    pub disposition: Disposition,
    pub effects: Vec<Effect>,
}

impl Decision {
    fn pass() -> Self {
        Self {
            disposition: Disposition::PassThrough,
            effects: Vec::new(),
        }
    }

    fn swallow() -> Self {
        Self {
            disposition: Disposition::Swallow,
            effects: Vec::new(),
        }
    }

    fn swallow_with(effects: Vec<Effect>) -> Self {
        Self {
            disposition: Disposition::Swallow,
            effects,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Down swallowed; waiting for the up, the hold deadline or a drag.
    Pressed { at: Instant, origin: (f64, f64) },
    /// Released once; waiting to see whether a second press follows.
    AwaitingSecond { released_at: Instant },
    /// An action already fired for this press; swallow the rest of it.
    Consumed,
    /// Handed back to the system after a drag; pass everything until the up.
    Passing,
}

#[derive(Debug)]
struct Press {
    button: u32,
    phase: Phase,
}

pub(crate) struct ButtonMachine {
    bindings: Vec<ButtonBinding>,
    presses: Vec<Press>,
}

impl ButtonMachine {
    pub fn new(bindings: Vec<ButtonBinding>) -> Self {
        Self {
            bindings,
            presses: Vec::new(),
        }
    }

    pub fn has_same_bindings(&self, bindings: &[ButtonBinding]) -> bool {
        self.bindings == bindings
    }

    /// Forgets all in-flight presses, e.g. after the tap was disabled and events were missed.
    pub fn reset(&mut self) {
        self.presses.clear();
    }

    pub fn down(&mut self, button: u32, at: Instant, location: (f64, f64)) -> Decision {
        if !self.has_bindings(button) {
            return Decision::pass();
        }
        let second_press = matches!(self.phase(button), Some(Phase::AwaitingSecond { .. }));
        if second_press && let Some(action) = self.action(button, Trigger::DoubleClick) {
            let effects = vec![Effect::Perform(action.clone())];
            self.set_phase(button, Phase::Consumed);
            return Decision::swallow_with(effects);
        }
        // A press still in flight here means we missed its up; start over.
        self.set_phase(
            button,
            Phase::Pressed {
                at,
                origin: location,
            },
        );
        Decision::swallow()
    }

    pub fn up(&mut self, button: u32, at: Instant) -> Decision {
        match self.phase(button) {
            None => Decision::pass(),
            Some(Phase::Pressed { .. }) => {
                if self.action(button, Trigger::DoubleClick).is_some() {
                    self.set_phase(button, Phase::AwaitingSecond { released_at: at });
                    Decision::swallow()
                } else {
                    self.remove(button);
                    Decision::swallow_with(vec![self.click_effect(button)])
                }
            }
            Some(Phase::Consumed) => {
                self.remove(button);
                Decision::swallow()
            }
            Some(Phase::Passing) | Some(Phase::AwaitingSecond { .. }) => {
                self.remove(button);
                Decision::pass()
            }
        }
    }

    pub fn dragged(&mut self, button: u32, location: (f64, f64)) -> Decision {
        match self.phase(button) {
            Some(Phase::Pressed { origin, .. }) => {
                let distance = (location.0 - origin.0).hypot(location.1 - origin.1);
                if distance > DRAG_THRESHOLD {
                    // Swallow this drag; the replayed down is queued ahead of the next one.
                    self.set_phase(button, Phase::Passing);
                    Decision::swallow_with(vec![Effect::ReplayDown(button)])
                } else {
                    Decision::swallow()
                }
            }
            Some(Phase::Consumed) => Decision::swallow(),
            _ => Decision::pass(),
        }
    }

    /// Fires every deadline that has passed by `now`.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        let buttons: Vec<u32> = self.presses.iter().map(|p| p.button).collect();
        for button in buttons {
            match self.phase(button) {
                Some(Phase::Pressed { at, .. }) if now >= at + HOLD_THRESHOLD => {
                    if let Some(action) = self.action(button, Trigger::Hold) {
                        effects.push(Effect::Perform(action.clone()));
                        self.set_phase(button, Phase::Consumed);
                    }
                }
                Some(Phase::AwaitingSecond { released_at })
                    if now >= released_at + DOUBLE_CLICK_INTERVAL =>
                {
                    self.remove(button);
                    effects.push(self.click_effect(button));
                }
                _ => {}
            }
        }
        effects
    }

    /// The earliest time [`tick`](Self::tick) has something to do, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.presses
            .iter()
            .filter_map(|press| match press.phase {
                Phase::Pressed { at, .. } if self.action(press.button, Trigger::Hold).is_some() => {
                    Some(at + HOLD_THRESHOLD)
                }
                Phase::AwaitingSecond { released_at } => Some(released_at + DOUBLE_CLICK_INTERVAL),
                _ => None,
            })
            .min()
    }

    /// A completed single click: the Click binding, or the original click if only other
    /// triggers are bound (its down/up were swallowed while we waited to find out).
    fn click_effect(&self, button: u32) -> Effect {
        match self.action(button, Trigger::Click) {
            Some(action) => Effect::Perform(action.clone()),
            None => Effect::ReplayClick(button),
        }
    }

    fn has_bindings(&self, button: u32) -> bool {
        self.bindings.iter().any(|b| b.button == button)
    }

    fn action(&self, button: u32, trigger: Trigger) -> Option<&Action> {
        self.bindings
            .iter()
            .find(|b| b.button == button && b.trigger == trigger)
            .map(|b| &b.action)
    }

    fn phase(&self, button: u32) -> Option<Phase> {
        self.presses
            .iter()
            .find(|p| p.button == button)
            .map(|p| p.phase)
    }

    fn set_phase(&mut self, button: u32, phase: Phase) {
        match self.presses.iter_mut().find(|p| p.button == button) {
            Some(press) => press.phase = phase,
            None => self.presses.push(Press { button, phase }),
        }
    }

    fn remove(&mut self, button: u32) {
        self.presses.retain(|p| p.button != button);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BUTTON_BACK, BUTTON_MIDDLE};

    const ORIGIN: (f64, f64) = (100.0, 100.0);

    fn bind(button: u32, trigger: Trigger, action: Action) -> ButtonBinding {
        ButtonBinding {
            button,
            trigger,
            action,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn unbound_button_passes_through() {
        let mut m = ButtonMachine::new(vec![bind(BUTTON_BACK, Trigger::Click, Action::Back)]);
        let t = Instant::now();
        assert_eq!(m.down(5, t, ORIGIN), Decision::pass());
        assert_eq!(m.dragged(5, (200.0, 200.0)), Decision::pass());
        assert_eq!(m.up(5, t + ms(50)), Decision::pass());
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn click_binding_fires_on_release() {
        let mut m = ButtonMachine::new(vec![bind(BUTTON_BACK, Trigger::Click, Action::Back)]);
        let t = Instant::now();
        assert_eq!(m.down(BUTTON_BACK, t, ORIGIN), Decision::swallow());
        assert_eq!(
            m.next_deadline(),
            None,
            "no hold or double binding, nothing to wait for"
        );
        assert_eq!(
            m.up(BUTTON_BACK, t + ms(80)),
            Decision::swallow_with(vec![Effect::Perform(Action::Back)])
        );
    }

    #[test]
    fn click_fires_even_after_a_long_press_when_hold_is_unbound() {
        let mut m = ButtonMachine::new(vec![bind(BUTTON_BACK, Trigger::Click, Action::Back)]);
        let t = Instant::now();
        m.down(BUTTON_BACK, t, ORIGIN);
        assert!(m.tick(t + ms(2000)).is_empty());
        assert_eq!(
            m.up(BUTTON_BACK, t + ms(2000)).effects,
            vec![Effect::Perform(Action::Back)]
        );
    }

    #[test]
    fn hold_fires_at_threshold_and_swallows_the_release() {
        let mut m = ButtonMachine::new(vec![bind(
            BUTTON_MIDDLE,
            Trigger::Hold,
            Action::MissionControl,
        )]);
        let t = Instant::now();
        m.down(BUTTON_MIDDLE, t, ORIGIN);
        assert_eq!(m.next_deadline(), Some(t + HOLD_THRESHOLD));
        assert!(m.tick(t + ms(299)).is_empty());
        assert_eq!(
            m.tick(t + HOLD_THRESHOLD),
            vec![Effect::Perform(Action::MissionControl)]
        );
        assert_eq!(m.next_deadline(), None);
        assert_eq!(
            m.dragged(BUTTON_MIDDLE, (300.0, 300.0)),
            Decision::swallow()
        );
        assert_eq!(m.up(BUTTON_MIDDLE, t + ms(500)), Decision::swallow());
    }

    #[test]
    fn short_press_with_only_hold_bound_replays_the_click() {
        let mut m = ButtonMachine::new(vec![bind(
            BUTTON_MIDDLE,
            Trigger::Hold,
            Action::MissionControl,
        )]);
        let t = Instant::now();
        m.down(BUTTON_MIDDLE, t, ORIGIN);
        assert_eq!(
            m.up(BUTTON_MIDDLE, t + ms(100)),
            Decision::swallow_with(vec![Effect::ReplayClick(BUTTON_MIDDLE)])
        );
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn click_and_hold_on_the_same_button() {
        let mut m = ButtonMachine::new(vec![
            bind(BUTTON_MIDDLE, Trigger::Click, Action::ShowDesktop),
            bind(BUTTON_MIDDLE, Trigger::Hold, Action::MissionControl),
        ]);
        let t = Instant::now();
        m.down(BUTTON_MIDDLE, t, ORIGIN);
        assert_eq!(
            m.up(BUTTON_MIDDLE, t + ms(100)).effects,
            vec![Effect::Perform(Action::ShowDesktop)]
        );
        m.down(BUTTON_MIDDLE, t + ms(1000), ORIGIN);
        assert_eq!(
            m.tick(t + ms(1400)),
            vec![Effect::Perform(Action::MissionControl)]
        );
        assert!(m.up(BUTTON_MIDDLE, t + ms(1500)).effects.is_empty());
    }

    #[test]
    fn double_click_fires_on_second_press() {
        let mut m = ButtonMachine::new(vec![
            bind(BUTTON_BACK, Trigger::Click, Action::Back),
            bind(BUTTON_BACK, Trigger::DoubleClick, Action::AppExpose),
        ]);
        let t = Instant::now();
        m.down(BUTTON_BACK, t, ORIGIN);
        assert_eq!(
            m.up(BUTTON_BACK, t + ms(60)),
            Decision::swallow(),
            "waits for a second click"
        );
        assert_eq!(m.next_deadline(), Some(t + ms(60) + DOUBLE_CLICK_INTERVAL));
        assert_eq!(
            m.down(BUTTON_BACK, t + ms(150), ORIGIN),
            Decision::swallow_with(vec![Effect::Perform(Action::AppExpose)])
        );
        assert_eq!(m.up(BUTTON_BACK, t + ms(200)), Decision::swallow());
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn single_click_fires_after_double_click_window() {
        let mut m = ButtonMachine::new(vec![
            bind(BUTTON_BACK, Trigger::Click, Action::Back),
            bind(BUTTON_BACK, Trigger::DoubleClick, Action::AppExpose),
        ]);
        let t = Instant::now();
        m.down(BUTTON_BACK, t, ORIGIN);
        m.up(BUTTON_BACK, t + ms(60));
        assert!(m.tick(t + ms(200)).is_empty());
        assert_eq!(
            m.tick(t + ms(60) + DOUBLE_CLICK_INTERVAL),
            vec![Effect::Perform(Action::Back)]
        );
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn single_click_with_only_double_bound_replays_after_window() {
        let mut m = ButtonMachine::new(vec![bind(
            BUTTON_BACK,
            Trigger::DoubleClick,
            Action::AppExpose,
        )]);
        let t = Instant::now();
        m.down(BUTTON_BACK, t, ORIGIN);
        m.up(BUTTON_BACK, t + ms(60));
        assert_eq!(m.tick(t + ms(500)), vec![Effect::ReplayClick(BUTTON_BACK)]);
    }

    #[test]
    fn dragging_past_threshold_hands_the_press_back() {
        let mut m = ButtonMachine::new(vec![bind(
            BUTTON_MIDDLE,
            Trigger::Hold,
            Action::MissionControl,
        )]);
        let t = Instant::now();
        m.down(BUTTON_MIDDLE, t, ORIGIN);
        assert_eq!(
            m.dragged(BUTTON_MIDDLE, (102.0, 101.0)),
            Decision::swallow(),
            "jitter"
        );
        assert_eq!(
            m.dragged(BUTTON_MIDDLE, (120.0, 100.0)),
            Decision::swallow_with(vec![Effect::ReplayDown(BUTTON_MIDDLE)])
        );
        assert_eq!(m.next_deadline(), None, "hold no longer pending");
        assert!(m.tick(t + ms(1000)).is_empty());
        assert_eq!(m.dragged(BUTTON_MIDDLE, (150.0, 100.0)), Decision::pass());
        assert_eq!(m.up(BUTTON_MIDDLE, t + ms(1000)), Decision::pass());
    }

    #[test]
    fn up_without_a_tracked_press_passes_through() {
        let mut m = ButtonMachine::new(vec![bind(BUTTON_BACK, Trigger::Click, Action::Back)]);
        assert_eq!(m.up(BUTTON_BACK, Instant::now()), Decision::pass());
    }

    #[test]
    fn reset_forgets_in_flight_presses() {
        let mut m = ButtonMachine::new(vec![bind(
            BUTTON_MIDDLE,
            Trigger::Hold,
            Action::MissionControl,
        )]);
        let t = Instant::now();
        m.down(BUTTON_MIDDLE, t, ORIGIN);
        m.reset();
        assert_eq!(m.next_deadline(), None);
        assert_eq!(m.up(BUTTON_MIDDLE, t + ms(50)), Decision::pass());
    }

    #[test]
    fn buttons_are_tracked_independently() {
        let mut m = ButtonMachine::new(vec![
            bind(BUTTON_MIDDLE, Trigger::Hold, Action::MissionControl),
            bind(BUTTON_BACK, Trigger::Click, Action::Back),
        ]);
        let t = Instant::now();
        m.down(BUTTON_MIDDLE, t, ORIGIN);
        m.down(BUTTON_BACK, t + ms(10), ORIGIN);
        assert_eq!(
            m.up(BUTTON_BACK, t + ms(50)).effects,
            vec![Effect::Perform(Action::Back)]
        );
        assert_eq!(
            m.tick(t + HOLD_THRESHOLD),
            vec![Effect::Perform(Action::MissionControl)]
        );
    }
}
