//! Decoding the keyboard's "aux control" keys (media, volume, brightness), which reach event
//! taps as system-defined events rather than key events. Shared by the brightness key tap
//! and the media key tap.

use objc2_core_graphics::{CGEvent, CGEventField, CGEventType};

/// `NX_SYSDEFINED` (IOLLEvent.h); CoreGraphics has no named `CGEventType` for it.
pub(crate) const SYSTEM_DEFINED: CGEventType = CGEventType(14);
/// Undocumented `CGEventField`s holding a system-defined event's `subtype` and `data1`, the
/// values `NSEvent` reports for them (the round-trip test in `display::keys` pins this).
const FIELD_SUBTYPE: CGEventField = CGEventField(99);
const FIELD_DATA1: CGEventField = CGEventField(149);
/// `NX_SUBTYPE_AUX_CONTROL_BUTTONS` (IOLLEvent.h): media, volume and brightness keys.
pub(crate) const SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;

/// `NX_KEYTYPE_*` (ev_keymap.h).
pub(crate) const KEYTYPE_BRIGHTNESS_UP: isize = 2;
pub(crate) const KEYTYPE_BRIGHTNESS_DOWN: isize = 3;
pub(crate) const KEYTYPE_PLAY: isize = 16;
pub(crate) const KEYTYPE_NEXT: isize = 17;
pub(crate) const KEYTYPE_PREVIOUS: isize = 18;
/// What Apple keyboards send for F9 and F7: a tap means next/previous track.
pub(crate) const KEYTYPE_FAST: isize = 19;
pub(crate) const KEYTYPE_REWIND: isize = 20;

/// Key state in bits 8–15 of `data1`: `NX_KEYDOWN` / `NX_KEYUP` (IOLLEvent.h).
pub(crate) const KEY_STATE_DOWN: isize = 0xA;
pub(crate) const KEY_STATE_UP: isize = 0xB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuxKey {
    /// `NX_KEYTYPE_*`.
    pub key_type: isize,
    /// Key down (including auto-repeat) rather than key up.
    pub pressed: bool,
    pub repeat: bool,
}

/// Decodes an aux-control-button event; `None` for any other system-defined event.
/// `data1` packs the key type in bits 16–31, the key state in bits 8–15 and the repeat
/// flag in bit 0.
pub(crate) fn decode(subtype: i16, data1: isize) -> Option<AuxKey> {
    if subtype != SUBTYPE_AUX_CONTROL_BUTTONS {
        return None;
    }
    let pressed = match (data1 & 0xFF00) >> 8 {
        KEY_STATE_DOWN => true,
        KEY_STATE_UP => false,
        _ => return None,
    };
    Some(AuxKey {
        key_type: (data1 & 0xFFFF_0000) >> 16,
        pressed,
        repeat: data1 & 0x1 != 0,
    })
}

/// `subtype` and `data1` of a system-defined event, read straight from the event's
/// compound-data fields. CoreGraphics names neither field; `NSEvent` would decode them,
/// but converting a caps-lock event off the main thread trips HIToolbox's main-queue
/// assertion and kills the process.
pub(crate) fn aux_fields(event: &CGEvent) -> Option<(i16, isize)> {
    let subtype = CGEvent::integer_value_field(Some(event), FIELD_SUBTYPE);
    let data1 = CGEvent::integer_value_field(Some(event), FIELD_DATA1);
    Some((i16::try_from(subtype).ok()?, data1 as isize))
}

/// The aux key a system-defined event carries, if it is one.
pub(crate) fn key(event: &CGEvent) -> Option<AuxKey> {
    let (subtype, data1) = aux_fields(event)?;
    decode(subtype, data1)
}

#[cfg(test)]
pub(crate) fn data1(key_type: isize, state: isize, repeat: bool) -> isize {
    (key_type << 16) | (state << 8) | isize::from(repeat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_key_type_state_and_repeat() {
        assert_eq!(
            decode(8, data1(KEYTYPE_PLAY, KEY_STATE_DOWN, false)),
            Some(AuxKey {
                key_type: KEYTYPE_PLAY,
                pressed: true,
                repeat: false
            })
        );
        assert_eq!(
            decode(8, data1(KEYTYPE_FAST, KEY_STATE_UP, true)),
            Some(AuxKey {
                key_type: KEYTYPE_FAST,
                pressed: false,
                repeat: true
            })
        );
    }

    #[test]
    fn ignores_other_subtypes_and_states() {
        assert_eq!(decode(7, data1(KEYTYPE_PLAY, KEY_STATE_DOWN, false)), None);
        assert_eq!(decode(8, data1(KEYTYPE_PLAY, 0x3, false)), None);
    }
}
