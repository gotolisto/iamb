//! The hand-off of the characters typed after a trigger (architecture §4.6, §8 rule 1,
//! ADR-0019, ADR-0024).
//!
//! Between a trigger and the injector's start marker, a fast typist's next keys reach the
//! application, after the abbreviation. The injector deletes them with the abbreviation and
//! types them again after the expansion, so it needs the characters themselves. The engine
//! never hands typed text out; [`TypedHandoff`] is the one other place they may live: at most
//! [`HANDOFF_CAPACITY`] characters, only while an expansion is in flight, zeroed when it ends.
//!
//! # Who writes, who reads
//!
//! | Thread | Calls | When |
//! | --- | --- | --- |
//! | Writer | [`arm`](TypedHandoff::arm) | the engine returned `Expanded { expansion }` |
//! | Writer | [`observe`](TypedHandoff::observe) | for **every** listener event, in stream order, before the engine handles it |
//! | Writer | [`disturb`](TypedHandoff::disturb) | before the engine handles any other event that clears its buffer: `FocusChanged`, `SecureState`, `Paused` |
//! | Injector | [`wait_sealed`](TypedHandoff::wait_sealed), then [`read`](TypedHandoff::read) | after it posted its start marker |
//! | Writer | [`finish`](TypedHandoff::finish) | the injector returned, whatever the outcome (after a clean injection the end marker has already emptied the hand-off; after a cancelled or failed one this does) |
//!
//! **The writer is the thread that feeds the engine**, not the hook callback. The trigger is
//! only known once the engine has matched, on its own thread; by then the hook may have passed
//! on further keys. Those keys are still in the channel, in order, in front of the start
//! marker, so the thread that drains the channel sees every one of them after it armed the
//! hand-off, and none is missed. A hook callback could only cover that gap by copying every key
//! all the time, which is the standing copy that §8 rule 1 forbids (and it would copy keys
//! typed into a password field, which the engine drops unseen). A second benefit: the writer
//! sees exactly the events the engine sees, in the same order, so the hand-off cancels in
//! exactly the cases in which the engine marks the expansion disturbed.
//!
//! # Synchronisation
//!
//! One mutex and one condition variable, no async. The writer never waits: each call takes the
//! mutex for a few instructions. The injector blocks in
//! [`wait_sealed`](TypedHandoff::wait_sealed) until the writer has observed the start marker
//! (the hand-off is then *sealed* and no longer changes), the hand-off is cancelled, or the
//! timeout passes. [`read`](TypedHandoff::read) runs a closure over the characters while it
//! holds the mutex, so the closure must only copy them into the OS input batch and return; it
//! must not call the OS, sleep or block.
//!
//! ```text
//!            arm(e)                 observe(InjectionStarted e)
//!   Idle ───────────▶ Capturing(e) ───────────────────────────▶ Sealed(e) ──┐
//!    ▲                    │ boundary, over-deleting Backspace,              │
//!    │                    │ 65th character, Overflow, disturb, timeout      │
//!    │                    ▼                                                 │
//!    │              Cancelled(e, why) ── characters zeroed at once          │
//!    │                    │                                                 │
//!    └────────────────────┴─────────────────────────────────────────────────┘
//!       observe(InjectionFinished e), finish(e), arm(next), clear(): zeroed
//! ```
//!
//! # Privacy
//!
//! - The characters sit in one heap allocation that never moves or grows;
//!   [`region`](TypedHandoff::region) gives its address range so the service can lock it
//!   against swapping ([`MemoryLock`](crate::MemoryLock)).
//! - They are wrapped in [`Sensitive`]. The type has no `Clone`, and its `Debug` shows only the
//!   status and the count. No method returns the characters; [`read`](TypedHandoff::read) lends
//!   them to the injector's closure.
//! - Every slot is overwritten with zeros when the hand-off is cancelled, cleared, re-armed,
//!   finished or dropped, and a Backspace zeros the slot it frees. The writes go through
//!   [`std::hint::black_box`] so the optimiser cannot drop them as dead stores.
//!
//! ```compile_fail
//! fn needs_clone<T: Clone>() {}
//! needs_clone::<iamb_platform::TypedHandoff>();
//! ```
//!
//! # Known gap
//!
//! Zero-on-drop is not tested: observing freed memory needs `unsafe`, which is forbidden here.
//! `Drop` delegates to the same zeroing routine that `clear` uses, and that routine is tested
//! by reading the storage back (the same recorded exception as the engine's `KeyBuffer`).

use std::fmt;
use std::hint::black_box;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use iamb_core::engine::ExpansionId;
use iamb_core::sensitive::Sensitive;

use crate::listener::ListenerEvent;
use crate::memory::MemoryRegion;

/// The most characters the hand-off holds (architecture §4.6). A 65th character typed before
/// the start marker cancels the expansion.
pub const HANDOFF_CAPACITY: usize = 64;

/// How long an injector waits for its own start marker before it gives up on an expansion. The
/// marker normally comes back in well under a millisecond; a marker that is this late means the
/// hook is gone, and the self-test (§3.2) will deal with that.
pub const START_MARKER_TIMEOUT: Duration = Duration::from_millis(100);

/// Why an expansion is not sent (the service then delivers `InjectionCancelled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CancelReason {
    /// A boundary key (Enter, an arrow, a click, …) was among the keys typed after the trigger:
    /// the caret may have moved.
    Boundary,
    /// A Backspace deleted more than was typed after the trigger: part of the abbreviation is
    /// gone.
    OverDeleted,
    /// More than [`HANDOFF_CAPACITY`] characters were typed before the start marker.
    TooManyTyped,
    /// Something else cleared the engine's buffer before the start marker: the focus moved,
    /// the focus verdict changed, an input method started composing, or Iamb was paused.
    Disturbed,
    /// The listener lost events, so the count cannot be trusted.
    Overflow,
    /// The start marker did not come back in time.
    StartMarkerTimeout,
    /// The hand-off is not armed for this expansion: the flight already ended.
    NotArmed,
}

/// What the hand-off is doing. Carries no typed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffStatus {
    /// No expansion in flight; the storage is zeroed.
    Idle,
    /// Armed: characters typed after the trigger are being collected.
    Capturing(ExpansionId),
    /// The start marker went by: the characters are final and the injector may read them.
    Sealed(ExpansionId),
    /// The expansion must not be sent; the storage is zeroed.
    Cancelled(ExpansionId, CancelReason),
}

struct State {
    /// Always [`HANDOFF_CAPACITY`] slots (empty only for the instant of an edit); slots at
    /// `len` and beyond are zero.
    chars: Sensitive<Box<[char]>>,
    len: usize,
    status: HandoffStatus,
}

impl State {
    /// Runs `edit` on the storage. `Sensitive` lends no mutable access, so the box is taken
    /// out and put back; only the pointer moves, the characters stay where they are.
    fn edit(&mut self, edit: impl FnOnce(&mut [char])) {
        let mut chars = std::mem::take(&mut self.chars).into_inner();
        edit(&mut chars);
        black_box(&mut chars);
        self.chars = Sensitive::new(chars);
    }

    fn zero(&mut self) {
        self.edit(|chars| chars.fill('\0'));
        self.len = 0;
    }

    fn cancel(&mut self, expansion: ExpansionId, reason: CancelReason) {
        self.zero();
        self.status = HandoffStatus::Cancelled(expansion, reason);
    }

    fn push(&mut self, expansion: ExpansionId, ch: char) {
        if self.len == HANDOFF_CAPACITY {
            self.cancel(expansion, CancelReason::TooManyTyped);
        } else {
            let at = self.len;
            self.edit(|chars| chars[at] = ch);
            self.len += 1;
        }
    }

    fn pop(&mut self, expansion: ExpansionId) {
        if self.len == 0 {
            self.cancel(expansion, CancelReason::OverDeleted);
        } else {
            self.len -= 1;
            let at = self.len;
            self.edit(|chars| chars[at] = '\0');
        }
    }
}

/// The characters typed between a trigger and the injector's start marker. See the
/// [module docs](self).
pub struct TypedHandoff {
    state: Mutex<State>,
    changed: Condvar,
}

impl TypedHandoff {
    /// An idle hand-off. Create it once, where it will live (the service shares it between the
    /// engine and injector threads), and lock [`region`](Self::region).
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                chars: Sensitive::new(vec!['\0'; HANDOFF_CAPACITY].into_boxed_slice()),
                len: 0,
                status: HandoffStatus::Idle,
            }),
            changed: Condvar::new(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // No code panics while holding the mutex, and the state is valid after every statement,
        // so a poisoned mutex is still safe to use.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts collecting for `expansion`: the engine has just returned its `Expanded` action.
    /// Whatever an earlier expansion left is zeroed.
    pub fn arm(&self, expansion: ExpansionId) {
        let mut state = self.state();
        state.zero();
        state.status = HandoffStatus::Capturing(expansion);
        drop(state);
        self.changed.notify_all();
    }

    /// Takes one listener event, in stream order. Call it for every event, before the engine
    /// handles the event; while no expansion is being captured it does nothing but end a
    /// flight.
    ///
    /// | Event | While capturing | Otherwise |
    /// | --- | --- | --- |
    /// | `Char` | stored; the 65th cancels | ignored |
    /// | `Backspace` | removes the last stored character; with none stored, cancels | ignored |
    /// | `Boundary`, `Composing` | cancels | ignored |
    /// | `InjectionStarted` for the armed expansion | seals | ignored |
    /// | `InjectionFinished` for the armed expansion | zeroes and returns to idle (the start marker was lost) | zeroes and returns to idle |
    /// | `Overflow` | cancels | ignored: once sealed, the injector is committed and still needs the characters |
    /// | `Hotkey`, markers of other expansions | ignored | ignored |
    pub fn observe(&self, event: &ListenerEvent) {
        let mut state = self.state();
        let before = state.status;
        match (before, event) {
            (HandoffStatus::Capturing(e), ListenerEvent::Char { ch, .. }) => {
                state.push(e, *ch.expose());
            }
            (HandoffStatus::Capturing(e), ListenerEvent::Backspace { .. }) => state.pop(e),
            (HandoffStatus::Capturing(e), ListenerEvent::Boundary { .. }) => {
                state.cancel(e, CancelReason::Boundary);
            }
            (HandoffStatus::Capturing(e), ListenerEvent::Composing { .. }) => {
                state.cancel(e, CancelReason::Disturbed);
            }
            (HandoffStatus::Capturing(e), ListenerEvent::InjectionStarted { expansion, .. })
                if e == *expansion =>
            {
                state.status = HandoffStatus::Sealed(e);
            }
            (HandoffStatus::Capturing(e), ListenerEvent::Overflow) => {
                state.cancel(e, CancelReason::Overflow);
            }
            (
                HandoffStatus::Capturing(e)
                | HandoffStatus::Sealed(e)
                | HandoffStatus::Cancelled(e, _),
                ListenerEvent::InjectionFinished { expansion, .. },
            ) if e == *expansion => {
                state.zero();
                state.status = HandoffStatus::Idle;
            }
            _ => {}
        }
        let changed = state.status != before;
        drop(state);
        if changed {
            self.changed.notify_all();
        }
    }

    /// Cancels a capture because something other than a listener event is about to clear the
    /// engine's buffer (`FocusChanged`, `SecureState`, `Paused`): call it before the engine
    /// handles that event. Does nothing while idle, and nothing once sealed: the output is
    /// already on its way.
    pub fn disturb(&self) {
        let mut state = self.state();
        if let HandoffStatus::Capturing(e) = state.status {
            state.cancel(e, CancelReason::Disturbed);
            drop(state);
            self.changed.notify_all();
        }
    }

    /// The injector is done with `expansion`, whatever the outcome: zeroes the storage and
    /// returns to idle, unless the hand-off already belongs to a later expansion (the outcome
    /// of one injection can reach the writer after the next trigger).
    pub fn finish(&self, expansion: ExpansionId) {
        let mut state = self.state();
        let ours = match state.status {
            HandoffStatus::Capturing(e)
            | HandoffStatus::Sealed(e)
            | HandoffStatus::Cancelled(e, _) => e == expansion,
            HandoffStatus::Idle => false,
        };
        if ours {
            state.zero();
            state.status = HandoffStatus::Idle;
            drop(state);
            self.changed.notify_all();
        }
    }

    /// Zeroes the storage and returns to idle unconditionally (worker shutdown, screen lock).
    /// An injector still waiting is woken and told [`CancelReason::NotArmed`].
    pub fn clear(&self) {
        let mut state = self.state();
        state.zero();
        state.status = HandoffStatus::Idle;
        drop(state);
        self.changed.notify_all();
    }

    /// Blocks the injector until the start marker of `expansion` has been observed, and
    /// returns how many characters were typed after the trigger (`k` in §4.6).
    ///
    /// # Errors
    ///
    /// The reason the expansion must not be sent. On
    /// [`StartMarkerTimeout`](CancelReason::StartMarkerTimeout) the hand-off is cancelled and
    /// zeroed, so a marker that arrives late cannot seal it.
    pub fn wait_sealed(
        &self,
        expansion: ExpansionId,
        timeout: Duration,
    ) -> Result<usize, CancelReason> {
        let (mut state, _) = self
            .changed
            .wait_timeout_while(self.state(), timeout, |s| {
                s.status == HandoffStatus::Capturing(expansion)
            })
            .unwrap_or_else(PoisonError::into_inner);
        match state.status {
            HandoffStatus::Sealed(e) if e == expansion => Ok(state.len),
            HandoffStatus::Cancelled(e, reason) if e == expansion => Err(reason),
            HandoffStatus::Capturing(e) if e == expansion => {
                state.cancel(e, CancelReason::StartMarkerTimeout);
                drop(state);
                self.changed.notify_all();
                Err(CancelReason::StartMarkerTimeout)
            }
            _ => Err(CancelReason::NotArmed),
        }
    }

    /// Lends the sealed characters of `expansion` to `f`, in typed order.
    ///
    /// `f` runs under the hand-off's mutex: it may only copy the characters into the injector's
    /// input batch (which the injector must zero after sending). It must not call the OS or
    /// block, and must not keep the characters anywhere else.
    ///
    /// # Errors
    ///
    /// [`NotArmed`](CancelReason::NotArmed) unless the hand-off is sealed for `expansion`; the
    /// reason, if it was cancelled.
    pub fn read<R>(
        &self,
        expansion: ExpansionId,
        f: impl FnOnce(&[char]) -> R,
    ) -> Result<R, CancelReason> {
        let state = self.state();
        match state.status {
            HandoffStatus::Sealed(e) if e == expansion => Ok(f(&state.chars.expose()[..state.len])),
            HandoffStatus::Cancelled(e, reason) if e == expansion => Err(reason),
            _ => Err(CancelReason::NotArmed),
        }
    }

    /// What the hand-off is doing.
    #[must_use]
    pub fn status(&self) -> HandoffStatus {
        self.state().status
    }

    /// How many characters are held (never which).
    #[must_use]
    pub fn len(&self) -> usize {
        self.state().len
    }

    /// True when no characters are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The address range of the character storage, for [`MemoryLock`](crate::MemoryLock). It
    /// does not change during the hand-off's life.
    #[must_use]
    pub fn region(&self) -> MemoryRegion {
        MemoryRegion::of_slice(self.state().chars.expose())
    }

    #[cfg(test)]
    fn raw_storage(&self) -> Vec<char> {
        self.state().chars.expose().to_vec()
    }
}

impl Default for TypedHandoff {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TypedHandoff {
    fn drop(&mut self) {
        self.state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .zero();
    }
}

impl fmt::Debug for TypedHandoff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        f.debug_struct("TypedHandoff")
            .field("status", &state.status)
            .field("len", &state.len)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use iamb_core::engine::BoundaryKind;

    use super::*;
    use crate::hotkeys::HotkeyId;

    const WAIT: Duration = Duration::from_secs(5);
    const NO_WAIT: Duration = Duration::ZERO;

    fn id(n: u64) -> ExpansionId {
        ExpansionId::from_raw(n)
    }

    fn type_chars(handoff: &TypedHandoff, text: &str) {
        for (seq, ch) in (100_u64..).zip(text.chars()) {
            handoff.observe(&ListenerEvent::char(ch, seq));
        }
    }

    fn start(handoff: &TypedHandoff, n: u64) {
        handoff.observe(&ListenerEvent::InjectionStarted {
            expansion: id(n),
            seq: 900,
        });
    }

    fn finish(handoff: &TypedHandoff, n: u64) {
        handoff.observe(&ListenerEvent::InjectionFinished {
            expansion: id(n),
            seq: 901,
        });
    }

    fn sealed_text(handoff: &TypedHandoff, n: u64) -> Result<String, CancelReason> {
        handoff.read(id(n), |chars| chars.iter().collect())
    }

    fn all_zero(handoff: &TypedHandoff) -> bool {
        let raw = handoff.raw_storage();
        raw.len() == HANDOFF_CAPACITY && raw.iter().all(|&c| c == '\0')
    }

    #[test]
    fn collects_between_arm_and_start_marker() {
        let handoff = TypedHandoff::new();
        assert_eq!(handoff.status(), HandoffStatus::Idle);
        assert!(handoff.is_empty());

        type_chars(&handoff, "ignored before arming");
        assert!(handoff.is_empty());
        assert!(all_zero(&handoff));

        handoff.arm(id(1));
        assert_eq!(handoff.status(), HandoffStatus::Capturing(id(1)));
        type_chars(&handoff, "om");
        assert_eq!(handoff.len(), 2);
        // Not readable before the start marker.
        assert_eq!(sealed_text(&handoff, 1), Err(CancelReason::NotArmed));

        start(&handoff, 1);
        assert_eq!(handoff.status(), HandoffStatus::Sealed(id(1)));
        assert_eq!(handoff.wait_sealed(id(1), NO_WAIT), Ok(2));
        assert_eq!(sealed_text(&handoff, 1).as_deref(), Ok("om"));

        // Keys after the start marker are between the markers: not collected.
        type_chars(&handoff, "xyz");
        handoff.observe(&ListenerEvent::Backspace { seq: 1 });
        handoff.observe(&ListenerEvent::Boundary {
            kind: BoundaryKind::Click,
            seq: 2,
        });
        handoff.disturb();
        assert_eq!(sealed_text(&handoff, 1).as_deref(), Ok("om"));

        finish(&handoff, 1);
        assert_eq!(handoff.status(), HandoffStatus::Idle);
        assert!(all_zero(&handoff));
        assert_eq!(sealed_text(&handoff, 1), Err(CancelReason::NotArmed));
    }

    #[test]
    fn nothing_typed_seals_empty() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(4));
        start(&handoff, 4);
        assert_eq!(handoff.wait_sealed(id(4), NO_WAIT), Ok(0));
        assert_eq!(sealed_text(&handoff, 4).as_deref(), Ok(""));
    }

    #[test]
    fn backspace_undoes_typed_characters_and_zeroes_the_slot() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(1));
        type_chars(&handoff, "oxm");
        handoff.observe(&ListenerEvent::Backspace { seq: 1 });
        handoff.observe(&ListenerEvent::Backspace { seq: 2 });
        assert_eq!(handoff.len(), 1);
        let raw = handoff.raw_storage();
        assert_eq!(raw[0], 'o');
        assert!(raw[1..].iter().all(|&c| c == '\0'), "freed slots are zero");
        type_chars(&handoff, "m");
        start(&handoff, 1);
        assert_eq!(sealed_text(&handoff, 1).as_deref(), Ok("om"));
    }

    #[test]
    fn backspace_past_the_trigger_cancels() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(1));
        type_chars(&handoff, "x");
        handoff.observe(&ListenerEvent::Backspace { seq: 1 });
        assert_eq!(handoff.status(), HandoffStatus::Capturing(id(1)));
        handoff.observe(&ListenerEvent::Backspace { seq: 2 });
        assert_eq!(
            handoff.status(),
            HandoffStatus::Cancelled(id(1), CancelReason::OverDeleted)
        );
        // Later keys and even the start marker change nothing.
        type_chars(&handoff, "q");
        start(&handoff, 1);
        assert!(all_zero(&handoff));
        assert_eq!(
            handoff.wait_sealed(id(1), NO_WAIT),
            Err(CancelReason::OverDeleted)
        );
        assert_eq!(sealed_text(&handoff, 1), Err(CancelReason::OverDeleted));
    }

    #[test]
    fn a_boundary_cancels_and_zeroes() {
        for kind in [
            BoundaryKind::Enter,
            BoundaryKind::Tab,
            BoundaryKind::Arrow,
            BoundaryKind::Click,
            BoundaryKind::Shortcut,
        ] {
            let handoff = TypedHandoff::new();
            handoff.arm(id(2));
            type_chars(&handoff, "secret");
            handoff.observe(&ListenerEvent::Boundary { kind, seq: 1 });
            assert_eq!(
                handoff.status(),
                HandoffStatus::Cancelled(id(2), CancelReason::Boundary)
            );
            assert!(all_zero(&handoff));
            assert!(handoff.is_empty());
        }
    }

    #[test]
    fn composition_disturb_and_overflow_cancel() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(1));
        type_chars(&handoff, "ab");
        handoff.observe(&ListenerEvent::Composing { on: true, seq: 1 });
        assert_eq!(
            handoff.status(),
            HandoffStatus::Cancelled(id(1), CancelReason::Disturbed)
        );
        assert!(all_zero(&handoff));

        handoff.arm(id(2));
        type_chars(&handoff, "ab");
        handoff.disturb();
        assert_eq!(
            handoff.status(),
            HandoffStatus::Cancelled(id(2), CancelReason::Disturbed)
        );
        assert!(all_zero(&handoff));

        handoff.arm(id(3));
        type_chars(&handoff, "ab");
        handoff.observe(&ListenerEvent::Overflow);
        assert_eq!(
            handoff.status(),
            HandoffStatus::Cancelled(id(3), CancelReason::Overflow)
        );
        assert!(all_zero(&handoff));

        // The end marker of a cancelled expansion still returns the hand-off to idle.
        finish(&handoff, 3);
        assert_eq!(handoff.status(), HandoffStatus::Idle);
    }

    #[test]
    fn an_overflow_after_the_start_marker_keeps_the_sealed_characters() {
        // The injector has deleted them from the field by now and must type them again.
        let handoff = TypedHandoff::new();
        handoff.arm(id(4));
        type_chars(&handoff, "ab");
        start(&handoff, 4);
        handoff.observe(&ListenerEvent::Overflow);
        assert_eq!(sealed_text(&handoff, 4).as_deref(), Ok("ab"));
        // The end marker may have been lost with the overflow: `finish` empties the hand-off.
        handoff.finish(id(4));
        assert_eq!(handoff.status(), HandoffStatus::Idle);
        assert!(all_zero(&handoff));
    }

    #[test]
    fn finish_ends_only_its_own_expansion() {
        let handoff = TypedHandoff::new();
        handoff.finish(id(1));
        assert_eq!(handoff.status(), HandoffStatus::Idle);

        for sealed in [false, true] {
            handoff.arm(id(1));
            type_chars(&handoff, "abc");
            if sealed {
                start(&handoff, 1);
            }
            // The outcome of an older injection arrives late: nothing changes.
            handoff.finish(id(7));
            assert_eq!(handoff.len(), 3);
            handoff.finish(id(1));
            assert_eq!(handoff.status(), HandoffStatus::Idle);
            assert!(all_zero(&handoff));
        }

        handoff.arm(id(2));
        handoff.disturb();
        handoff.finish(id(2));
        assert_eq!(handoff.status(), HandoffStatus::Idle);

        // A waiting injector is woken.
        let handoff = Arc::new(TypedHandoff::new());
        handoff.arm(id(3));
        let injector = {
            let handoff = Arc::clone(&handoff);
            thread::spawn(move || handoff.wait_sealed(id(3), WAIT))
        };
        handoff.finish(id(3));
        assert_eq!(
            injector.join().expect("no panic"),
            Err(CancelReason::NotArmed)
        );
    }

    #[test]
    fn holds_sixty_four_characters_and_cancels_on_the_next() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(1));
        let text: String = ('a'..='z').cycle().take(HANDOFF_CAPACITY).collect();
        type_chars(&handoff, &text);
        assert_eq!(handoff.len(), HANDOFF_CAPACITY);
        assert_eq!(handoff.status(), HandoffStatus::Capturing(id(1)));
        start(&handoff, 1);
        assert_eq!(handoff.wait_sealed(id(1), NO_WAIT), Ok(HANDOFF_CAPACITY));
        assert_eq!(sealed_text(&handoff, 1).as_deref(), Ok(text.as_str()));

        handoff.arm(id(2));
        type_chars(&handoff, &text);
        type_chars(&handoff, "!");
        assert_eq!(
            handoff.status(),
            HandoffStatus::Cancelled(id(2), CancelReason::TooManyTyped)
        );
        assert!(all_zero(&handoff));
        assert_eq!(handoff.raw_storage().len(), HANDOFF_CAPACITY);
    }

    #[test]
    fn clear_zeroes_every_slot() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(1));
        type_chars(&handoff, "typed after the trigger");
        assert!(!all_zero(&handoff));
        handoff.clear();
        assert!(all_zero(&handoff));
        assert_eq!(handoff.status(), HandoffStatus::Idle);
        assert!(handoff.is_empty());

        handoff.arm(id(2));
        type_chars(&handoff, "sealed text");
        start(&handoff, 2);
        handoff.clear();
        assert!(all_zero(&handoff));
        assert_eq!(sealed_text(&handoff, 2), Err(CancelReason::NotArmed));
    }

    #[test]
    fn arming_again_zeroes_what_the_last_expansion_left() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(1));
        type_chars(&handoff, "old characters");
        start(&handoff, 1);
        handoff.arm(id(2));
        assert!(all_zero(&handoff));
        assert_eq!(handoff.status(), HandoffStatus::Capturing(id(2)));
        // The old expansion's markers and readers are refused.
        finish(&handoff, 1);
        assert_eq!(handoff.status(), HandoffStatus::Capturing(id(2)));
        assert_eq!(sealed_text(&handoff, 1), Err(CancelReason::NotArmed));
        assert_eq!(
            handoff.wait_sealed(id(1), NO_WAIT),
            Err(CancelReason::NotArmed)
        );
        assert_eq!(handoff.status(), HandoffStatus::Capturing(id(2)));
    }

    #[test]
    fn markers_of_other_expansions_and_hotkeys_are_ignored() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(5));
        type_chars(&handoff, "a");
        start(&handoff, 4);
        finish(&handoff, 4);
        handoff.observe(&ListenerEvent::Hotkey {
            id: HotkeyId(1),
            seq: 3,
        });
        assert_eq!(handoff.status(), HandoffStatus::Capturing(id(5)));
        assert_eq!(handoff.len(), 1);

        // An end marker without a start marker (the start marker was lost) ends the flight.
        finish(&handoff, 5);
        assert_eq!(handoff.status(), HandoffStatus::Idle);
        assert!(all_zero(&handoff));

        // While idle, nothing arms it by accident.
        let idle = TypedHandoff::default();
        start(&idle, 1);
        idle.observe(&ListenerEvent::Overflow);
        idle.disturb();
        assert_eq!(idle.status(), HandoffStatus::Idle);
        assert_eq!(
            idle.wait_sealed(id(1), NO_WAIT),
            Err(CancelReason::NotArmed)
        );
    }

    #[test]
    fn a_missing_start_marker_times_out_and_cancels() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(1));
        type_chars(&handoff, "ab");
        assert_eq!(
            handoff.wait_sealed(id(1), Duration::from_millis(5)),
            Err(CancelReason::StartMarkerTimeout)
        );
        assert!(all_zero(&handoff));
        // The late marker cannot seal it any more.
        start(&handoff, 1);
        assert_eq!(
            handoff.status(),
            HandoffStatus::Cancelled(id(1), CancelReason::StartMarkerTimeout)
        );
    }

    #[test]
    fn the_injector_thread_wakes_when_the_writer_sees_the_marker() {
        let handoff = Arc::new(TypedHandoff::new());
        handoff.arm(id(9));
        let injector = {
            let handoff = Arc::clone(&handoff);
            thread::spawn(move || {
                let k = handoff.wait_sealed(id(9), WAIT)?;
                let text = handoff.read(id(9), |chars| chars.iter().collect::<String>())?;
                Ok::<_, CancelReason>((k, text))
            })
        };
        type_chars(&handoff, "hi");
        start(&handoff, 9);
        assert_eq!(injector.join().expect("no panic"), Ok((2, "hi".to_owned())));
    }

    #[test]
    fn a_waiting_injector_is_woken_by_cancel_and_clear() {
        for cancel in [true, false] {
            let handoff = Arc::new(TypedHandoff::new());
            handoff.arm(id(1));
            let injector = {
                let handoff = Arc::clone(&handoff);
                thread::spawn(move || handoff.wait_sealed(id(1), WAIT))
            };
            // Whether the wait starts before or after the change, the result is the same.
            let expected = if cancel {
                handoff.disturb();
                CancelReason::Disturbed
            } else {
                handoff.clear();
                CancelReason::NotArmed
            };
            assert_eq!(injector.join().expect("no panic"), Err(expected));
        }
    }

    #[test]
    fn the_region_is_stable_and_covers_the_storage() {
        let handoff = TypedHandoff::new();
        let region = handoff.region();
        assert_eq!(region.len(), HANDOFF_CAPACITY * size_of::<char>());
        assert!(!region.is_empty());
        handoff.arm(id(1));
        type_chars(&handoff, "abc");
        handoff.observe(&ListenerEvent::Backspace { seq: 1 });
        start(&handoff, 1);
        handoff.clear();
        assert_eq!(handoff.region(), region, "edits never move the allocation");
    }

    #[test]
    fn debug_shows_the_status_and_the_count_only() {
        let handoff = TypedHandoff::new();
        handoff.arm(id(3));
        type_chars(&handoff, "Zq");
        let shown = format!("{handoff:?}");
        assert_eq!(
            shown,
            "TypedHandoff { status: Capturing(ExpansionId(3)), len: 2, .. }"
        );
        assert!(!shown.contains('Z') && !shown.contains('q'));
    }
}
