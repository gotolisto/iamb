//! One expansion, start marker to end marker (architecture §4.6; the protocol in the trait
//! docs of [`Injector`](iamb_platform::Injector)).
//!
//! The protocol is written once, against [`Desk`]: the few things it needs from the system.
//! The Windows layer implements `Desk` with `SendInput` and the clipboard; the tests here
//! implement it with a recorder, so every path below runs on the Linux build machine.
//!
//! 1. A hand-off already cancelled for this expansion: [`Cancelled`](InjectionOutcome::Cancelled),
//!    nothing sent.
//! 2. The start marker, in a call of its own. Refused: [`Failed`](InjectionOutcome::Failed),
//!    nothing else is sent.
//! 3. [`TypedHandoff::wait_sealed`]; an error is `Cancelled`, and no end marker follows (the
//!    listener reports nothing more for this expansion).
//! 4. The [`Plan`] of the compensated steps: every batch before the last, the pauses and the
//!    pastes, each batch led by the releases of modifier keys the user still holds.
//! 5. The final batch in one call: its keys, the characters typed after the trigger (copied in
//!    under the hand-off's lock, nothing else done there), the end marker.
//!
//! When the system refuses output after the start marker went out, the injector stops at once
//! and posts no end marker, as the [`Injector`](iamb_platform::Injector) contract says: an end
//! marker would tell the service the expansion finished and count it as used. The outcome is
//! `Failed`, and the service answers the engine with `InjectionCancelled` (§4.6).

use std::time::Duration;

use iamb_platform::{
    ClipboardContent, HandoffStatus, InjectionOutcome, InjectionRequest, Marker, PlatformError,
    TypedHandoff, compensate,
};

use super::plan::{KeyEvent, Op, Plan, release_held};
use super::strategy::Strategy;

/// What the protocol needs from the system.
pub(crate) trait Desk {
    /// Key events being collected for one `SendInput` call.
    type Batch;

    /// An empty batch with room for `capacity` events, so that filling it does not move the
    /// events already in it (they may be retyped characters).
    fn batch(&mut self, capacity: usize) -> Self::Batch;

    /// Appends events to a batch.
    fn extend(&mut self, batch: &mut Self::Batch, events: &[KeyEvent]);

    /// Appends the Unicode events of `chars`. Runs under the hand-off's lock: it may only
    /// convert and copy, never call the system or allocate beyond the batch's capacity.
    fn extend_chars(batch: &mut Self::Batch, chars: &[char]);

    /// Sends a batch in one call and zeroes it. Fewer events taken than sent is
    /// [`InjectionRefused`](PlatformError::InjectionRefused).
    fn send(&mut self, batch: Self::Batch) -> Result<(), PlatformError>;

    /// The modifier keys (from [`MODIFIERS`](super::plan::MODIFIERS)) down right now.
    fn held_modifiers(&mut self) -> Vec<u16>;

    /// Pauses.
    fn sleep(&mut self, ms: u32);

    /// Pastes `content`: snapshot, transient write, paste shortcut, wait, restore.
    fn paste(
        &mut self,
        content: &ClipboardContent,
        strategy: &Strategy,
    ) -> Result<(), PlatformError>;
}

/// Why the output stopped early.
enum Stop {
    Failed(PlatformError),
    Cancelled(iamb_platform::CancelReason),
}

/// Runs one expansion. See the module docs.
pub(crate) fn inject<D: Desk>(
    desk: &mut D,
    request: &InjectionRequest,
    typed: &TypedHandoff,
    strategy: &Strategy,
    marker_timeout: Duration,
) -> InjectionOutcome {
    let expansion = request.expansion;
    if let HandoffStatus::Cancelled(e, reason) = typed.status()
        && e == expansion
    {
        return InjectionOutcome::Cancelled(reason);
    }

    if let Err(error) = send_markers(desk, Marker::InjectionStart(expansion)) {
        return InjectionOutcome::Failed(error);
    }
    let k = match typed.wait_sealed(expansion, marker_timeout) {
        Ok(k) => k,
        Err(reason) => return InjectionOutcome::Cancelled(reason),
    };

    let compensated = compensate(&request.steps, k);
    let plan = Plan::new(&compensated, strategy);
    match output(desk, request, typed, &plan, strategy) {
        Ok(()) => InjectionOutcome::Finished { retyped: k },
        // No end marker after a failure: the service answers with `InjectionCancelled`.
        Err(Stop::Failed(error)) => InjectionOutcome::Failed(error),
        Err(Stop::Cancelled(reason)) => InjectionOutcome::Cancelled(reason),
    }
}

fn send_markers<D: Desk>(desk: &mut D, marker: Marker) -> Result<(), PlatformError> {
    let mut batch = desk.batch(1);
    desk.extend(&mut batch, &[KeyEvent::marker(marker)]);
    desk.send(batch)
}

/// Sends one batch, led by the release of held modifiers.
fn send_keys<D: Desk>(desk: &mut D, events: &[KeyEvent]) -> Result<(), PlatformError> {
    let release = release_held(&desk.held_modifiers());
    let mut batch = desk.batch(release.len() + events.len());
    desk.extend(&mut batch, &release);
    desk.extend(&mut batch, events);
    desk.send(batch)
}

fn output<D: Desk>(
    desk: &mut D,
    request: &InjectionRequest,
    typed: &TypedHandoff,
    plan: &Plan<'_>,
    strategy: &Strategy,
) -> Result<(), Stop> {
    for op in &plan.ops {
        match op {
            Op::Keys(events) => send_keys(desk, events).map_err(Stop::Failed)?,
            Op::Sleep(ms) => desk.sleep(*ms),
            Op::Paste(content) => desk.paste(content, strategy).map_err(Stop::Failed)?,
        }
    }

    // The final batch: keys, retyped characters, end marker, in one call.
    let has_output = !plan.last.is_empty() || plan.retype > 0;
    let release = if has_output {
        release_held(&desk.held_modifiers())
    } else {
        Vec::new()
    };
    // A character is at most two UTF-16 units, each a press and a release.
    let capacity = release.len() + plan.last.len() + plan.retype * 4 + 1;
    let mut batch = desk.batch(capacity);
    desk.extend(&mut batch, &release);
    desk.extend(&mut batch, &plan.last);
    if plan.retype > 0 {
        typed
            .read(request.expansion, |chars| {
                D::extend_chars(&mut batch, chars);
            })
            .map_err(Stop::Cancelled)?;
    }
    desk.extend(
        &mut batch,
        &[KeyEvent::marker(Marker::InjectionEnd(request.expansion))],
    );
    desk.send(batch).map_err(Stop::Failed)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use iamb_platform::core_types::{BoundaryKind, ExpansionId, KeyName, Sensitive};
    use iamb_platform::{CancelReason, InjectionStep, ListenerEvent};

    use super::super::plan::{
        Key, VK_BACK, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_RETURN, VK_SHIFT, VK_UNASSIGNED, push_char,
    };
    use super::*;

    /// What the recorder saw.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Did {
        /// A batch: rendered as text (see `render`).
        Sent(String),
        /// A batch the system refused.
        Refused(String),
        Slept(u32),
        Pasted(ClipboardContent),
        PasteFailed,
    }

    /// A desk that records, and plays the listener: a start marker it sends seals the
    /// hand-off, after first observing the keys "typed" since the trigger.
    struct Recorder<'h> {
        typed: &'h TypedHandoff,
        did: Vec<Did>,
        /// Keys the user typed after the trigger, observed before the start marker.
        typed_before_start: Vec<ListenerEvent>,
        /// Whether the listener sees the start marker.
        hook_alive: bool,
        /// Refuse the send with this index (0 is the start marker).
        refuse_send: Option<usize>,
        sends: usize,
        fail_paste: Option<PlatformError>,
        held: Vec<u16>,
        /// Clear the hand-off after it sealed, before the final batch (worker shutdown).
        clear_after_seal: bool,
    }

    impl<'h> Recorder<'h> {
        fn new(typed: &'h TypedHandoff) -> Self {
            Self {
                typed,
                did: Vec::new(),
                typed_before_start: Vec::new(),
                hook_alive: true,
                refuse_send: None,
                sends: 0,
                fail_paste: None,
                held: Vec::new(),
                clear_after_seal: false,
            }
        }
    }

    /// Renders a batch: `[S]`/`[E]` for the markers, `⌫`, `←`, `⏎`, `⇧`/`⇧↑` for Shift,
    /// `↑<vk>` for other releases at the front, and the typed text.
    fn render(batch: &[KeyEvent]) -> String {
        let mut out = String::new();
        let mut units = Vec::new();
        for event in batch {
            if !matches!(event.key, Key::Unicode(_)) {
                out.push_str(&String::from_utf16_lossy(&units));
                units.clear();
            }
            match (event.marker, event.key, event.up) {
                (Marker::InjectionStart(_), _, _) => out.push_str("[S]"),
                (Marker::InjectionEnd(_), _, _) => out.push_str("[E]"),
                (_, Key::Unicode(u), false) => units.push(u),
                (_, Key::Vk(VK_BACK), false) => out.push('⌫'),
                (_, Key::Vk(VK_LEFT), false) => out.push('←'),
                (_, Key::Vk(VK_RETURN), false) => out.push('⏎'),
                (_, Key::Vk(VK_SHIFT), false) => out.push('⇧'),
                (_, Key::Vk(VK_SHIFT), true) => out.push_str("⇧↑"),
                (_, Key::Vk(VK_UNASSIGNED), false) => out.push_str("mask"),
                (_, Key::Vk(vk), true)
                    if !matches!(vk, VK_BACK | VK_LEFT | VK_RETURN | VK_UNASSIGNED) =>
                {
                    write!(out, "↑{vk:#x}").expect("a string");
                }
                _ => {}
            }
        }
        out.push_str(&String::from_utf16_lossy(&units));
        out
    }

    impl Desk for Recorder<'_> {
        type Batch = Vec<KeyEvent>;

        fn batch(&mut self, capacity: usize) -> Self::Batch {
            Vec::with_capacity(capacity)
        }

        fn extend(&mut self, batch: &mut Self::Batch, events: &[KeyEvent]) {
            batch.extend_from_slice(events);
        }

        fn extend_chars(batch: &mut Self::Batch, chars: &[char]) {
            let capacity = batch.capacity();
            for &ch in chars {
                push_char(batch, ch);
            }
            assert_eq!(batch.capacity(), capacity, "the batch moved under the lock");
        }

        fn send(&mut self, batch: Self::Batch) -> Result<(), PlatformError> {
            let index = self.sends;
            self.sends += 1;
            let shown = render(&batch);
            if self.refuse_send == Some(index) {
                self.did.push(Did::Refused(shown));
                return Err(PlatformError::InjectionRefused);
            }
            self.did.push(Did::Sent(shown));
            for event in &batch {
                match event.marker {
                    Marker::InjectionStart(expansion) if self.hook_alive => {
                        for key in std::mem::take(&mut self.typed_before_start) {
                            self.typed.observe(&key);
                        }
                        self.typed
                            .observe(&ListenerEvent::InjectionStarted { expansion, seq: 1 });
                    }
                    Marker::InjectionEnd(expansion) if self.hook_alive => {
                        self.typed
                            .observe(&ListenerEvent::InjectionFinished { expansion, seq: 2 });
                    }
                    _ => {}
                }
            }
            Ok(())
        }

        fn held_modifiers(&mut self) -> Vec<u16> {
            if self.clear_after_seal {
                self.typed.clear();
            }
            self.held.clone()
        }

        fn sleep(&mut self, ms: u32) {
            self.did.push(Did::Slept(ms));
        }

        fn paste(
            &mut self,
            content: &ClipboardContent,
            _strategy: &Strategy,
        ) -> Result<(), PlatformError> {
            if let Some(error) = self.fail_paste.clone() {
                self.did.push(Did::PasteFailed);
                return Err(error);
            }
            self.did.push(Did::Pasted(content.clone()));
            Ok(())
        }
    }

    const E: ExpansionId = ExpansionId::from_raw(7);

    fn char_event(ch: char) -> ListenerEvent {
        ListenerEvent::Char {
            ch: Sensitive::new(ch),
            seq: 0,
        }
    }

    fn request(steps: Vec<InjectionStep>) -> InjectionRequest {
        InjectionRequest::new(E, steps)
    }

    fn brb() -> InjectionRequest {
        request(vec![
            InjectionStep::DeleteBack(3),
            InjectionStep::TypeText("be right back".to_owned()),
        ])
    }

    fn run(recorder: &mut Recorder<'_>, request: &InjectionRequest) -> InjectionOutcome {
        let typed = recorder.typed;
        inject(
            recorder,
            request,
            typed,
            &Strategy::DEFAULT,
            Duration::from_millis(20),
        )
    }

    fn sent(s: &str) -> Did {
        Did::Sent(s.to_owned())
    }

    #[test]
    fn deletion_text_and_end_marker_go_in_one_call() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Finished { retyped: 0 }
        );
        assert_eq!(desk.did, [sent("[S]"), sent("⌫⌫⌫be right back[E]")]);
        // The end marker emptied the hand-off.
        assert_eq!(typed.status(), HandoffStatus::Idle);
    }

    #[test]
    fn keys_typed_after_the_trigger_are_deleted_and_typed_again() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.typed_before_start = vec![char_event('x'), char_event('é'), char_event('👍')];
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Finished { retyped: 3 }
        );
        assert_eq!(desk.did, [sent("[S]"), sent("⌫⌫⌫⌫⌫⌫be right backxé👍[E]")]);
    }

    #[test]
    fn a_cancelled_hand_off_sends_nothing() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        typed.observe(&ListenerEvent::Boundary {
            kind: BoundaryKind::Enter,
            seq: 0,
        });
        let mut desk = Recorder::new(&typed);
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Cancelled(CancelReason::Boundary)
        );
        assert!(desk.did.is_empty());
    }

    #[test]
    fn a_boundary_before_the_start_marker_cancels_without_an_end_marker() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.typed_before_start = vec![
            char_event('a'),
            ListenerEvent::Boundary {
                kind: BoundaryKind::Arrow,
                seq: 0,
            },
        ];
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Cancelled(CancelReason::Boundary)
        );
        assert_eq!(desk.did, [sent("[S]")]);
    }

    #[test]
    fn a_start_marker_that_never_comes_back_cancels() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.hook_alive = false;
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Cancelled(CancelReason::StartMarkerTimeout)
        );
        assert_eq!(desk.did, [sent("[S]")]);
    }

    #[test]
    fn an_unarmed_hand_off_cancels() {
        let typed = TypedHandoff::new();
        let mut desk = Recorder::new(&typed);
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Cancelled(CancelReason::NotArmed)
        );
        assert_eq!(desk.did, [sent("[S]")]);
    }

    #[test]
    fn a_refused_start_marker_sends_nothing_else() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.refuse_send = Some(0);
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Failed(PlatformError::InjectionRefused)
        );
        assert_eq!(desk.did, [Did::Refused("[S]".to_owned())]);
    }

    #[test]
    fn refused_output_still_posts_the_end_marker() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.refuse_send = Some(1);
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Failed(PlatformError::InjectionRefused)
        );
        assert_eq!(
            desk.did,
            [sent("[S]"), Did::Refused("⌫⌫⌫be right back[E]".to_owned()),]
        );
    }

    #[test]
    fn refused_output_before_a_wait_stops_there() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.refuse_send = Some(1);
        let req = request(vec![
            InjectionStep::TypeText("a".to_owned()),
            InjectionStep::Wait(100),
            InjectionStep::TypeText("b".to_owned()),
        ]);
        assert_eq!(
            run(&mut desk, &req),
            InjectionOutcome::Failed(PlatformError::InjectionRefused)
        );
        assert_eq!(desk.did, [sent("[S]"), Did::Refused("a".to_owned())]);
    }

    #[test]
    fn waits_and_pastes_split_the_output() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.typed_before_start = vec![char_event('q')];
        let long = "L".repeat(250);
        let req = request(vec![
            InjectionStep::DeleteBack(2),
            InjectionStep::TypeText("a".to_owned()),
            InjectionStep::Wait(40),
            InjectionStep::TypeText(long.clone()),
            InjectionStep::PressKey {
                key: KeyName::Enter,
                shift: true,
                times: 1,
            },
            InjectionStep::MoveCaretBack(1),
        ]);
        assert_eq!(
            run(&mut desk, &req),
            InjectionOutcome::Finished { retyped: 1 }
        );
        assert_eq!(
            desk.did,
            [
                sent("[S]"),
                sent("⌫⌫⌫a"),
                Did::Slept(40),
                Did::Pasted(ClipboardContent::text(long)),
                sent("⇧⏎⇧↑←q[E]"),
            ]
        );
    }

    #[test]
    fn a_failed_paste_stops_without_an_end_marker() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.fail_paste = Some(PlatformError::os(5, "OpenClipboard"));
        let req = request(vec![
            InjectionStep::Paste(ClipboardContent::rich("x", "<b>x</b>")),
            InjectionStep::TypeText("after".to_owned()),
        ]);
        assert_eq!(
            run(&mut desk, &req),
            InjectionOutcome::Failed(PlatformError::os(5, "OpenClipboard"))
        );
        assert_eq!(desk.did, [sent("[S]"), Did::PasteFailed]);
    }

    #[test]
    fn an_empty_expansion_still_gets_both_markers() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.held = vec![VK_LSHIFT];
        assert_eq!(
            run(&mut desk, &request(Vec::new())),
            InjectionOutcome::Finished { retyped: 0 }
        );
        // Nothing to send, so no modifier is released either.
        assert_eq!(desk.did, [sent("[S]"), sent("[E]")]);
    }

    #[test]
    fn held_modifiers_are_released_before_each_batch() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.held = vec![VK_LSHIFT, VK_LMENU];
        let req = request(vec![
            InjectionStep::DeleteBack(1),
            InjectionStep::Wait(5),
            InjectionStep::TypeText("x".to_owned()),
        ]);
        assert_eq!(
            run(&mut desk, &req),
            InjectionOutcome::Finished { retyped: 0 }
        );
        assert_eq!(
            desk.did,
            [
                sent("[S]"),
                sent("mask↑0xa0↑0xa4⌫"),
                Did::Slept(5),
                sent("mask↑0xa0↑0xa4x[E]"),
            ]
        );
    }

    #[test]
    fn a_hand_off_cleared_before_the_retype_cancels() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        desk.typed_before_start = vec![char_event('z')];
        desk.clear_after_seal = true;
        assert_eq!(
            run(&mut desk, &brb()),
            InjectionOutcome::Cancelled(CancelReason::NotArmed)
        );
        // Cancelled after the start marker: no end marker, the service answers instead.
        assert_eq!(desk.did, [sent("[S]")]);
    }

    #[test]
    fn markers_carry_the_expansion() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let mut desk = Recorder::new(&typed);
        let mut batch = desk.batch(1);
        desk.extend(&mut batch, &[KeyEvent::marker(Marker::InjectionEnd(E))]);
        assert_eq!(batch[0].marker, Marker::InjectionEnd(E));
        assert_eq!(batch[0].key, Key::Vk(VK_UNASSIGNED));
        assert!(batch[0].up);
    }
}
