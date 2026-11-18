//! The marker every synthetic event carries (architecture §5).
//!
//! Every event the injector synthesises carries a marker, in the extra-info field on Windows
//! (`dwExtraInfo`) and in the event-source user data on macOS (`kCGEventSourceUserData`). The
//! listener decodes it for each event it sees:
//!
//! | Decoded | The listener |
//! | --- | --- |
//! | `None` | treats the event as a real key |
//! | [`Marker::Output`] | drops it: Iamb never reacts to its own output |
//! | [`Marker::InjectionStart`] / [`Marker::InjectionEnd`] | reports `InjectionStarted` / `InjectionFinished` in stream order (§4.6) |
//! | [`Marker::SelfTest`] | completes the pending `self_test` (§3.2) and reports nothing |
//!
//! # Encoding
//!
//! One 64-bit value: bits 48–63 hold the signature `0x4942`, bits 40–47 the kind, bits 0–39 the
//! payload. The top bit is clear, so the value is the same positive number as an `i64` (macOS)
//! and as a `usize` on the 64-bit Windows targets Iamb supports.
//!
//! The payload of the injection markers is the expansion id, truncated to 40 bits. The engine
//! numbers expansions from 1, so an id only reaches 2⁴⁰ after a trillion expansions in one
//! worker run; [`Marker::fits`] says whether an id survives the round trip.

use iamb_core::engine::ExpansionId;

use crate::listener::ListenerEvent;

/// Number of payload bits in an encoded marker.
pub const PAYLOAD_BITS: u32 = 40;

const SIGNATURE: u64 = 0x4942;
const SIGNATURE_SHIFT: u32 = 48;
const KIND_SHIFT: u32 = PAYLOAD_BITS;
const PAYLOAD_MASK: u64 = (1 << PAYLOAD_BITS) - 1;

const KIND_OUTPUT: u64 = 1;
const KIND_START: u64 = 2;
const KIND_END: u64 = 3;
const KIND_SELF_TEST: u64 = 4;

/// What a synthetic event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Marker {
    /// Ordinary injected output: a deletion, typed text, a key press, the paste shortcut.
    Output,
    /// The no-op event the injector posts before an expansion's output.
    InjectionStart(ExpansionId),
    /// The no-op event the injector posts after an expansion's output.
    InjectionEnd(ExpansionId),
    /// The no-op event of the listener's self-test, with the nonce of that test.
    SelfTest(u32),
}

impl Marker {
    /// The value to put in the event's marker field.
    #[must_use]
    pub const fn encode(self) -> u64 {
        let (kind, payload) = match self {
            Self::Output => (KIND_OUTPUT, 0),
            Self::InjectionStart(e) => (KIND_START, e.raw() & PAYLOAD_MASK),
            Self::InjectionEnd(e) => (KIND_END, e.raw() & PAYLOAD_MASK),
            Self::SelfTest(nonce) => (KIND_SELF_TEST, nonce as u64),
        };
        (SIGNATURE << SIGNATURE_SHIFT) | (kind << KIND_SHIFT) | payload
    }

    /// The marker an event's marker field holds; `None` for a real key (or another program's
    /// synthetic event).
    #[must_use]
    pub const fn decode(raw: u64) -> Option<Self> {
        if raw >> SIGNATURE_SHIFT != SIGNATURE {
            return None;
        }
        let payload = raw & PAYLOAD_MASK;
        match (raw >> KIND_SHIFT) & 0xFF {
            KIND_OUTPUT if payload == 0 => Some(Self::Output),
            KIND_START => Some(Self::InjectionStart(ExpansionId::from_raw(payload))),
            KIND_END => Some(Self::InjectionEnd(ExpansionId::from_raw(payload))),
            // The nonce is 32 bits; anything wider was not written by `encode`.
            KIND_SELF_TEST if payload <= u32::MAX as u64 => Some(Self::SelfTest(low_u32(payload))),
            _ => None,
        }
    }

    /// [`encode`](Self::encode) as the signed value macOS stores. Never negative.
    #[must_use]
    pub const fn encode_i64(self) -> i64 {
        self.encode().cast_signed()
    }

    /// [`decode`](Self::decode) from the signed value macOS stores.
    #[must_use]
    pub const fn decode_i64(raw: i64) -> Option<Self> {
        Self::decode(raw.cast_unsigned())
    }

    /// True when `expansion` survives encoding (it is below 2⁴⁰).
    #[must_use]
    pub const fn fits(expansion: ExpansionId) -> bool {
        expansion.raw() <= PAYLOAD_MASK
    }

    /// The event the listener reports for this marker, stamped with `seq`; `None` for output
    /// and self-test markers, which are not reported.
    #[must_use]
    pub const fn listener_event(self, seq: u64) -> Option<ListenerEvent> {
        match self {
            Self::InjectionStart(expansion) => {
                Some(ListenerEvent::InjectionStarted { expansion, seq })
            }
            Self::InjectionEnd(expansion) => {
                Some(ListenerEvent::InjectionFinished { expansion, seq })
            }
            Self::Output | Self::SelfTest(_) => None,
        }
    }
}

/// The low 32 bits of `value`.
const fn low_u32(value: u64) -> u32 {
    (value & 0xFFFF_FFFF) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> [Marker; 6] {
        [
            Marker::Output,
            Marker::InjectionStart(ExpansionId::from_raw(1)),
            Marker::InjectionStart(ExpansionId::from_raw(PAYLOAD_MASK)),
            Marker::InjectionEnd(ExpansionId::from_raw(77)),
            Marker::SelfTest(0),
            Marker::SelfTest(u32::MAX),
        ]
    }

    #[test]
    fn round_trips_through_both_widths() {
        for m in all() {
            assert_eq!(Marker::decode(m.encode()), Some(m));
            assert_eq!(Marker::decode_i64(m.encode_i64()), Some(m));
            assert!(m.encode_i64() > 0, "the top bit is clear");
        }
    }

    #[test]
    fn distinct_markers_encode_differently() {
        let encoded: Vec<u64> = all().iter().map(|m| m.encode()).collect();
        for (i, a) in encoded.iter().enumerate() {
            for b in &encoded[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn real_keys_and_foreign_values_decode_to_none() {
        for raw in [0, 1, u64::MAX, 0xFFFF_FFFF, 0x4942, 0x4943 << 48] {
            assert_eq!(Marker::decode(raw), None, "{raw:#x}");
        }
        assert_eq!(Marker::decode_i64(-1), None);
        // Our signature with an unknown kind, an output marker with a payload, and a self-test
        // nonce wider than 32 bits were not written by `encode`.
        let sig = SIGNATURE << SIGNATURE_SHIFT;
        assert_eq!(Marker::decode(sig), None);
        assert_eq!(Marker::decode(sig | (9 << KIND_SHIFT)), None);
        assert_eq!(Marker::decode(sig | (KIND_OUTPUT << KIND_SHIFT) | 1), None);
        assert_eq!(
            Marker::decode(sig | (KIND_SELF_TEST << KIND_SHIFT) | (1 << 32)),
            None
        );
    }

    #[test]
    fn expansion_ids_are_truncated_to_the_payload() {
        let big = ExpansionId::from_raw((1 << PAYLOAD_BITS) + 5);
        assert!(!Marker::fits(big));
        assert!(Marker::fits(ExpansionId::from_raw(PAYLOAD_MASK)));
        assert_eq!(
            Marker::decode(Marker::InjectionStart(big).encode()),
            Some(Marker::InjectionStart(ExpansionId::from_raw(5)))
        );
    }

    #[test]
    fn only_injection_markers_are_reported() {
        let e = ExpansionId::from_raw(3);
        assert_eq!(
            Marker::InjectionStart(e).listener_event(10),
            Some(ListenerEvent::InjectionStarted {
                expansion: e,
                seq: 10
            })
        );
        assert_eq!(
            Marker::InjectionEnd(e).listener_event(11),
            Some(ListenerEvent::InjectionFinished {
                expansion: e,
                seq: 11
            })
        );
        assert_eq!(Marker::Output.listener_event(1), None);
        assert_eq!(Marker::SelfTest(4).listener_event(1), None);
    }
}
