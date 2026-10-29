//! What the tap does with each event, given whether the Hyper key is held. Pure, so the
//! rules are unit-tested without a real tap.

use std::time::{Duration, Instant};

use objc2_core_graphics::CGEventFlags;

use crate::config::{HyperKeyConfig, QuickPress};

/// A press longer than this isn't a quick press, even if nothing else was pressed: the
/// user held Hyper, then changed their mind.
pub(super) const QUICK_PRESS_TIMEOUT: Duration = Duration::from_secs(1);

/// The events the machine cares about. The Hyper key itself arrives as the key it's
/// remapped to (see `remap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input {
    HyperDown {
        repeat: bool,
    },
    HyperUp,
    /// Any other key down or mouse button down: it uses the held Hyper key.
    Press,
    /// Key ups, modifier changes and drags: they carry Hyper while it's held, but don't
    /// count as using it.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Let the event through unchanged.
    PassThrough,
    /// Let the event through with these flags added.
    AddFlags(CGEventFlags),
    /// Drop the event; the Hyper key's own presses never reach apps.
    Swallow,
    /// Drop the event, then perform the quick-press action.
    SwallowAndQuickPress(QuickPress),
}

#[derive(Debug, Default)]
pub(super) struct Machine {
    /// When the current Hyper press started, while it's held.
    held_since: Option<Instant>,
    /// Something was pressed during the current Hyper press.
    used: bool,
}

impl Machine {
    pub fn handle(&mut self, input: Input, config: &HyperKeyConfig, now: Instant) -> Outcome {
        match input {
            Input::HyperDown { repeat } => {
                // A repeat whose key-down we missed (tap re-enabled mid-press) starts a press
                // too, but never a quick one.
                if self.held_since.is_none() {
                    log::debug!(
                        "Hyper down{}",
                        if repeat { " (missed key-down)" } else { "" }
                    );
                    self.held_since = Some(now);
                    self.used = repeat;
                }
                Outcome::Swallow
            }
            Input::HyperUp => {
                let quick = match self.held_since.take() {
                    Some(since) => !self.used && now.duration_since(since) < QUICK_PRESS_TIMEOUT,
                    None => false,
                };
                log::debug!("Hyper up (quick: {quick}, used: {})", self.used);
                match config.quick_press {
                    QuickPress::Nothing => Outcome::Swallow,
                    action if quick => Outcome::SwallowAndQuickPress(action),
                    _ => Outcome::Swallow,
                }
            }
            Input::Press | Input::Other if self.held_since.is_some() => {
                if input == Input::Press {
                    self.used = true;
                }
                Outcome::AddFlags(hyper_flags(config))
            }
            Input::Press | Input::Other => Outcome::PassThrough,
        }
    }

    /// Forgets a held press, e.g. after the tap was disabled and may have missed its key-up.
    pub fn reset(&mut self) {
        self.held_since = None;
        self.used = false;
    }
}

pub(super) fn hyper_flags(config: &HyperKeyConfig) -> CGEventFlags {
    let flags = CGEventFlags::MaskControl | CGEventFlags::MaskAlternate | CGEventFlags::MaskCommand;
    if config.include_shift {
        flags | CGEventFlags::MaskShift
    } else {
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(quick_press: QuickPress) -> HyperKeyConfig {
        HyperKeyConfig {
            enabled: true,
            key: crate::config::HyperKeySource::CapsLock,
            include_shift: true,
            quick_press,
        }
    }

    const ALL_FOUR: CGEventFlags = CGEventFlags::MaskControl
        .union(CGEventFlags::MaskAlternate)
        .union(CGEventFlags::MaskShift)
        .union(CGEventFlags::MaskCommand);

    #[test]
    fn held_hyper_adds_all_four_modifiers() {
        let config = config(QuickPress::Nothing);
        let mut machine = Machine::default();
        let now = Instant::now();
        assert_eq!(
            machine.handle(Input::Press, &config, now),
            Outcome::PassThrough
        );
        assert_eq!(
            machine.handle(Input::HyperDown { repeat: false }, &config, now),
            Outcome::Swallow
        );
        assert_eq!(
            machine.handle(Input::Press, &config, now),
            Outcome::AddFlags(ALL_FOUR)
        );
        assert_eq!(
            machine.handle(Input::Other, &config, now),
            Outcome::AddFlags(ALL_FOUR)
        );
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now),
            Outcome::Swallow
        );
        assert_eq!(
            machine.handle(Input::Press, &config, now),
            Outcome::PassThrough
        );
        assert_eq!(
            machine.handle(Input::Other, &config, now),
            Outcome::PassThrough
        );
    }

    #[test]
    fn shift_can_be_left_out() {
        let mut config = config(QuickPress::Nothing);
        config.include_shift = false;
        assert_eq!(
            hyper_flags(&config),
            CGEventFlags::MaskControl | CGEventFlags::MaskAlternate | CGEventFlags::MaskCommand
        );
    }

    #[test]
    fn quick_press_fires_when_nothing_else_was_pressed() {
        let config = config(QuickPress::Escape);
        let mut machine = Machine::default();
        let now = Instant::now();
        machine.handle(Input::HyperDown { repeat: false }, &config, now);
        // Auto-repeats of the Hyper key itself don't count as using it.
        machine.handle(Input::HyperDown { repeat: true }, &config, now);
        // Neither do modifier changes or key-ups.
        machine.handle(Input::Other, &config, now);
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now + Duration::from_millis(200)),
            Outcome::SwallowAndQuickPress(QuickPress::Escape)
        );
    }

    #[test]
    fn no_quick_press_after_using_hyper() {
        let config = config(QuickPress::ToggleCapsLock);
        let mut machine = Machine::default();
        let now = Instant::now();
        machine.handle(Input::HyperDown { repeat: false }, &config, now);
        machine.handle(Input::Press, &config, now);
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now),
            Outcome::Swallow
        );
        // The next press starts fresh.
        machine.handle(Input::HyperDown { repeat: false }, &config, now);
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now),
            Outcome::SwallowAndQuickPress(QuickPress::ToggleCapsLock)
        );
    }

    #[test]
    fn no_quick_press_after_a_long_hold() {
        let config = config(QuickPress::Escape);
        let mut machine = Machine::default();
        let now = Instant::now();
        machine.handle(Input::HyperDown { repeat: false }, &config, now);
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now + QUICK_PRESS_TIMEOUT),
            Outcome::Swallow
        );
    }

    #[test]
    fn no_quick_press_when_set_to_nothing() {
        let config = config(QuickPress::Nothing);
        let mut machine = Machine::default();
        let now = Instant::now();
        machine.handle(Input::HyperDown { repeat: false }, &config, now);
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now),
            Outcome::Swallow
        );
    }

    #[test]
    fn missed_key_down_never_quick_presses() {
        let config = config(QuickPress::Escape);
        let mut machine = Machine::default();
        let now = Instant::now();
        // A key-up with no press seen, then a repeat with no key-down seen.
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now),
            Outcome::Swallow
        );
        machine.handle(Input::HyperDown { repeat: true }, &config, now);
        assert_eq!(
            machine.handle(Input::Press, &config, now),
            Outcome::AddFlags(ALL_FOUR)
        );
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now),
            Outcome::Swallow
        );
    }

    #[test]
    fn reset_releases_hyper() {
        let config = config(QuickPress::Escape);
        let mut machine = Machine::default();
        let now = Instant::now();
        machine.handle(Input::HyperDown { repeat: false }, &config, now);
        machine.reset();
        assert_eq!(
            machine.handle(Input::Press, &config, now),
            Outcome::PassThrough
        );
        assert_eq!(
            machine.handle(Input::HyperUp, &config, now),
            Outcome::Swallow
        );
    }
}
