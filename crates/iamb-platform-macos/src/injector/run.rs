//! The injector protocol of one expansion (the trait docs of `Injector`; architecture §4.6),
//! independent of how events are posted.
//!
//! [`inject`] drives an [`Output`]: the macOS one posts `CGEvent`s and pastes through the
//! pasteboard; the tests use a recording one. The order:
//!
//! 1. The permission check: without it nothing is posted and the outcome is `Failed`.
//! 2. A hand-off already cancelled for this expansion: `Cancelled`, nothing posted.
//! 3. The plan of the steps after the leading deletion is made now, before the start marker,
//!    so that only posting remains between the hand-off sealing and the output.
//! 4. The output is opened (the private event source), where the keys go is noted
//!    ([`Output::focus`]), and the start marker posted.
//! 5. [`TypedHandoff::wait_sealed`] gives `k`, or the reason to cancel (no end marker then).
//! 6. [`compensate`] gives the deletion, `n + k` Backspaces, posted first; then the bursts;
//!    then the `k` characters, copied out of the hand-off into a stack buffer inside
//!    [`TypedHandoff::read`], chunked and posted outside it, and zeroed.
//! 7. The end marker, and `Finished { retyped: k }`.
//!
//! When the system refuses an event after the start marker, the driver stops, posts no end
//! marker and answers `Failed`, as the trait requires; the service then sends
//! `InjectionCancelled`, which ends the flight in the engine. A paste restores the pasteboard
//! on every path before its error reaches the driver (`clipboard::paste`).
//!
//! # The focus between steps
//!
//! macOS posts every event on its own, so the [`FocusGuard`] is asked before every key
//! operation (the deletion, each run of one key, each text chunk), before every paste and
//! before the retyped characters. When the keys would go elsewhere than at the start marker,
//! the driver stops with `Cancelled(FocusMoved)` and posts no end marker (the 2.8 review's
//! finding 1). A key operation that may move the focus itself (Tab, Return, Escape, Space,
//! an arrow other than Left) ends the comparisons for the expansion: the move it causes
//! cannot be told from another one.
//!
//! # A pasteboard that cannot be saved whole
//!
//! A paste whose snapshot came back incomplete wrote nothing (`clipboard::paste`). Plain text
//! is then typed with keys, as the strategy would type it with the `keys` method; other
//! content stops the expansion with `Cancelled(ClipboardNotRestorable)` (finding 14).

use std::hint::black_box;
use std::time::Duration;

use iamb_platform::{
    CancelReason, ClipboardContent, FocusGuard, HANDOFF_CAPACITY, InjectionOutcome,
    InjectionRequest, Marker, PlatformError, Result, START_MARKER_TIMEOUT, TypedHandoff,
    compensate,
};

use super::chunk::{TextChunk, TextChunks};
use super::plan::{Burst, KeyPress, Op, Planner, keycode};
use super::strategy::{Method, Strategy};
use crate::clipboard::paste::SNAPSHOT_INCOMPLETE;

/// Where the injector's events go.
pub(crate) trait Output {
    /// Where keys go, as the [`FocusGuard`] compares it.
    type Focus: PartialEq;
    /// Where keys go now. Must not wait for any application.
    fn focus(&mut self) -> Self::Focus;
    /// Posts a no-op event carrying `marker`.
    fn marker(&mut self, marker: Marker) -> Result<()>;
    /// Posts one press and release of `key`, marked as output.
    fn press(&mut self, key: KeyPress) -> Result<()>;
    /// Posts one press and release that carries `chunk`, marked as output.
    fn text(&mut self, chunk: &TextChunk) -> Result<()>;
    /// Sleeps.
    fn pause(&mut self, duration: Duration);
    /// Pastes `content`: snapshot, transient write, paste shortcut, `settle`, restore.
    /// [`SNAPSHOT_INCOMPLETE`] when the snapshot could not save everything; nothing was
    /// written then.
    fn paste(&mut self, content: &ClipboardContent, settle: Duration) -> Result<()>;
}

/// True when pressing `key` may move the focus by itself (see the module docs): every key
/// but Backspace, forward Delete and Left.
pub(crate) const fn may_move_focus(key: KeyPress) -> bool {
    !matches!(
        key.code,
        keycode::DELETE | keycode::FORWARD_DELETE | keycode::LEFT
    )
}

/// The secure-input part of where keys go, asked through the main thread: an answer at the
/// start marker, then the same question before each step.
///
/// When the main thread did not answer at the start marker (it runs no run loop, as in a test
/// harness), the flag is not asked again: the expansion is not held up by a wait per step,
/// and the flag counts as unchanged. When it answered then but not later, the late question
/// counts as unchanged too: a busy main thread is not a move of the focus. The process and
/// the watcher's notifications are still compared.
#[derive(Debug, Default)]
pub(crate) struct SecureInputReads {
    first: FirstRead,
}

/// What the first question of an injection got.
#[derive(Debug, Clone, Copy, Default)]
enum FirstRead {
    #[default]
    NotAsked,
    Unanswered,
    Answered(bool),
}

impl SecureInputReads {
    /// The flag for this read; `ask` asks the main thread.
    pub(crate) fn read(&mut self, ask: impl FnOnce() -> Option<bool>) -> Option<bool> {
        match self.first {
            FirstRead::NotAsked => {
                let answer = ask();
                self.first = answer.map_or(FirstRead::Unanswered, FirstRead::Answered);
                answer
            }
            FirstRead::Unanswered => None,
            FirstRead::Answered(first) => Some(ask().unwrap_or(first)),
        }
    }
}

/// Runs the protocol for `request`. `permitted` is the permission check; `open` creates the
/// output once the expansion is known to go ahead.
pub(crate) fn inject<O: Output>(
    request: &InjectionRequest,
    typed: &TypedHandoff,
    strategy: &Strategy,
    permitted: impl FnOnce() -> Result<()>,
    open: impl FnOnce() -> Result<O>,
) -> InjectionOutcome {
    inject_with_timeout(
        request,
        typed,
        strategy,
        permitted,
        open,
        START_MARKER_TIMEOUT,
    )
}

/// [`inject`] with the start-marker timeout as a parameter (tests shorten it).
pub(crate) fn inject_with_timeout<O: Output>(
    request: &InjectionRequest,
    typed: &TypedHandoff,
    strategy: &Strategy,
    permitted: impl FnOnce() -> Result<()>,
    open: impl FnOnce() -> Result<O>,
    marker_timeout: Duration,
) -> InjectionOutcome {
    let expansion = request.expansion;
    if let Err(error) = permitted() {
        return InjectionOutcome::Failed(error);
    }
    if let iamb_platform::HandoffStatus::Cancelled(e, reason) = typed.status()
        && e == expansion
    {
        return InjectionOutcome::Cancelled(reason);
    }

    // Everything but the leading deletion is known now.
    let rest = compensate(&request.steps, 0).rest;
    let mut planner = Planner::new(strategy);
    for step in rest {
        planner.step(step);
    }
    let bursts = planner.finish();

    let mut output = match open() {
        Ok(output) => output,
        Err(error) => return InjectionOutcome::Failed(error),
    };
    let mut guard = FocusGuard::new(output.focus());
    if let Err(error) = output.marker(Marker::InjectionStart(expansion)) {
        return InjectionOutcome::Failed(error);
    }
    let k = match typed.wait_sealed(expansion, marker_timeout) {
        Ok(k) => k,
        Err(reason) => return InjectionOutcome::Cancelled(reason),
    };
    let plan = compensate(&request.steps, k);
    match send(
        &mut output,
        request,
        typed,
        strategy,
        plan.delete_first,
        &bursts,
        plan.retype,
        &mut guard,
    ) {
        Ok(()) => {}
        Err(Stop::Failed(error)) => return InjectionOutcome::Failed(error),
        Err(Stop::Cancelled(reason)) => return InjectionOutcome::Cancelled(reason),
    }
    match output.marker(Marker::InjectionEnd(expansion)) {
        Ok(()) => InjectionOutcome::Finished { retyped: k },
        Err(error) => InjectionOutcome::Failed(error),
    }
}

enum Stop {
    Failed(PlatformError),
    Cancelled(CancelReason),
}

impl From<PlatformError> for Stop {
    fn from(error: PlatformError) -> Self {
        Self::Failed(error)
    }
}

#[allow(clippy::too_many_arguments)] // The protocol's state, passed down once.
fn send<O: Output>(
    output: &mut O,
    request: &InjectionRequest,
    typed: &TypedHandoff,
    strategy: &Strategy,
    delete_first: usize,
    bursts: &[Burst],
    retype: usize,
    guard: &mut FocusGuard<O::Focus>,
) -> std::result::Result<(), Stop> {
    let gap = strategy.key_delay();
    let mut paced = Paced::new(output, gap, guard);
    if delete_first > 0 {
        paced.check()?;
        paced.press(KeyPress::DELETE, delete_first)?;
    }
    for burst in bursts {
        match burst {
            Burst::Events { ops, gap } => {
                paced.gap = *gap;
                paced.ops(ops)?;
            }
            Burst::Wait(duration) => {
                paced.output.pause(*duration);
                paced.first = true;
            }
            Burst::Paste { content, settle } => {
                paced.check()?;
                match paced.output.paste(content, *settle) {
                    Ok(()) => {}
                    Err(error) if error == SNAPSHOT_INCOMPLETE => {
                        let ops = typed_instead(content, strategy)
                            .ok_or(Stop::Cancelled(CancelReason::ClipboardNotRestorable))?;
                        paced.gap = gap;
                        paced.ops(&ops)?;
                    }
                    Err(error) => return Err(Stop::Failed(error)),
                }
                paced.first = true;
            }
        }
    }
    if retype > 0 {
        paced.check()?;
        paced.gap = gap;
        retype_typed(&mut paced, request, typed, strategy)?;
    }
    Ok(())
}

/// The key operations that type `content` instead of pasting it: its plain text as the
/// strategy types text with the `keys` method. `None` for content that is more than plain
/// text (HTML, files), which keys cannot carry.
fn typed_instead(content: &ClipboardContent, strategy: &Strategy) -> Option<Vec<Op>> {
    let ClipboardContent {
        text: Some(text),
        html: None,
        files,
    } = content
    else {
        return None;
    };
    if !files.is_empty() {
        return None;
    }
    let keys = Strategy {
        method: Method::Keys,
        ..*strategy
    };
    let mut planner = Planner::new(&keys);
    planner.step(&iamb_platform::InjectionStep::TypeText(text.clone()));
    Some(
        planner
            .finish()
            .into_iter()
            .flat_map(|burst| match burst {
                Burst::Events { ops, .. } => ops,
                Burst::Wait(_) | Burst::Paste { .. } => Vec::new(),
            })
            .collect(),
    )
}

/// Copies the characters typed after the trigger out of the hand-off, types them, and zeroes
/// every copy.
fn retype_typed<O: Output>(
    paced: &mut Paced<'_, O>,
    request: &InjectionRequest,
    typed: &TypedHandoff,
    strategy: &Strategy,
) -> std::result::Result<(), Stop> {
    let mut batch = ['\0'; HANDOFF_CAPACITY];
    // Inside the closure: a copy and nothing else (the hand-off's rule).
    let len = typed
        .read(request.expansion, |chars| {
            let len = chars.len().min(HANDOFF_CAPACITY);
            batch[..len].copy_from_slice(&chars[..len]);
            len
        })
        .map_err(Stop::Cancelled)?;
    let mut chunks = Vec::with_capacity(len);
    let mut packer = TextChunks::new(usize::from(strategy.chunk_units));
    let mut emit = |chunk| chunks.push(chunk);
    for &c in &batch[..len] {
        packer.push(c, &mut emit);
    }
    packer.finish(&mut emit);
    batch.fill('\0');
    black_box(&mut batch);
    let mut result = Ok(());
    for chunk in &chunks {
        if let Err(error) = paced.text(chunk) {
            result = Err(Stop::Failed(error));
            break;
        }
    }
    // Each chunk zeroes itself when dropped.
    drop(chunks);
    result
}

/// Posts key operations with the strategy's delay between two of them, asking the focus
/// guard before each.
struct Paced<'o, O: Output> {
    output: &'o mut O,
    gap: Duration,
    /// True until the first event of a run is posted: no delay before it.
    first: bool,
    guard: &'o mut FocusGuard<O::Focus>,
}

impl<'o, O: Output> Paced<'o, O> {
    const fn new(output: &'o mut O, gap: Duration, guard: &'o mut FocusGuard<O::Focus>) -> Self {
        Self {
            output,
            gap,
            first: true,
            guard,
        }
    }

    /// `Cancelled(FocusMoved)` when the keys would go elsewhere now.
    fn check(&mut self) -> std::result::Result<(), Stop> {
        let output = &mut *self.output;
        self.guard.check(|| output.focus()).map_err(Stop::Cancelled)
    }

    /// Posts `ops`, each after a look at the focus.
    fn ops(&mut self, ops: &[Op]) -> std::result::Result<(), Stop> {
        for op in ops {
            self.check()?;
            match op {
                Op::Press { key, times } => {
                    self.press(*key, *times)?;
                    if may_move_focus(*key) {
                        self.guard.own_move();
                    }
                }
                Op::Text(chunk) => self.text(chunk)?,
            }
        }
        Ok(())
    }

    fn pace(&mut self) {
        if !self.first && !self.gap.is_zero() {
            self.output.pause(self.gap);
        }
        self.first = false;
    }

    fn press(&mut self, key: KeyPress, times: usize) -> Result<()> {
        for _ in 0..times {
            self.pace();
            self.output.press(key)?;
        }
        Ok(())
    }

    fn text(&mut self, chunk: &TextChunk) -> Result<()> {
        self.pace();
        self.output.text(chunk)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use iamb_platform::core_types::{ExpansionId, KeyName};
    use iamb_platform::{CancelReason, InjectionStep, ListenerEvent, PermissionKind, TypedHandoff};

    use super::*;
    use crate::injector::plan::keycode;
    use crate::injector::strategy::Mods;

    /// What the recording output saw.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Seen {
        Marker(Marker),
        Key(u16, Mods),
        Text(String),
        Pause(u128),
        Paste(Option<String>, u128),
    }

    /// An output that records, and fails the `fail_at`-th call when asked.
    #[derive(Default)]
    struct Recorder {
        seen: Vec<Seen>,
        fail_at: Option<usize>,
        calls: usize,
        /// Where keys go.
        focus: u64,
        /// After this many recorded entries the focus is `.1`.
        move_focus: Option<(usize, u64)>,
        focus_reads: usize,
        /// Pastes find the pasteboard impossible to save whole.
        paste_incomplete: bool,
    }

    impl Recorder {
        fn call(&mut self) -> Result<()> {
            self.calls += 1;
            if self.fail_at == Some(self.calls) {
                Err(PlatformError::os(0, "CGEventCreateKeyboardEvent"))
            } else {
                Ok(())
            }
        }
    }

    impl Output for &mut Recorder {
        type Focus = u64;

        fn focus(&mut self) -> u64 {
            self.focus_reads += 1;
            if let Some((after, to)) = self.move_focus
                && self.seen.len() >= after
            {
                self.focus = to;
            }
            self.focus
        }

        fn marker(&mut self, marker: Marker) -> Result<()> {
            self.call()?;
            self.seen.push(Seen::Marker(marker));
            Ok(())
        }
        fn press(&mut self, key: KeyPress) -> Result<()> {
            self.call()?;
            self.seen.push(Seen::Key(key.code, key.mods));
            Ok(())
        }
        fn text(&mut self, chunk: &TextChunk) -> Result<()> {
            self.call()?;
            self.seen.push(Seen::Text(chunk.to_text()));
            Ok(())
        }
        fn pause(&mut self, duration: Duration) {
            self.seen.push(Seen::Pause(duration.as_millis()));
        }
        fn paste(&mut self, content: &ClipboardContent, settle: Duration) -> Result<()> {
            self.call()?;
            if self.paste_incomplete {
                return Err(SNAPSHOT_INCOMPLETE);
            }
            self.seen
                .push(Seen::Paste(content.text.clone(), settle.as_millis()));
            Ok(())
        }
    }

    const E: ExpansionId = ExpansionId::from_raw(7);

    fn sealed(typed_after: &[char]) -> TypedHandoff {
        let typed = TypedHandoff::new();
        typed.arm(E);
        for (i, &c) in typed_after.iter().enumerate() {
            typed.observe(&ListenerEvent::char(c, i as u64));
        }
        typed.observe(&ListenerEvent::InjectionStarted {
            expansion: E,
            seq: 100,
        });
        typed
    }

    fn run(
        request: &InjectionRequest,
        typed: &TypedHandoff,
        strategy: &Strategy,
        recorder: &mut Recorder,
    ) -> InjectionOutcome {
        inject_with_timeout(
            request,
            typed,
            strategy,
            || Ok(()),
            || Ok(recorder),
            Duration::from_millis(50),
        )
    }

    fn text(s: &str) -> InjectionStep {
        InjectionStep::TypeText(s.to_owned())
    }

    fn delete() -> Seen {
        Seen::Key(keycode::DELETE, Mods::NONE)
    }

    #[test]
    fn markers_bracket_the_output() {
        let request = InjectionRequest::new(E, vec![InjectionStep::DeleteBack(2), text("hi")]);
        let mut recorder = Recorder::default();
        let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
        assert_eq!(
            recorder.seen,
            [
                Seen::Marker(Marker::InjectionStart(E)),
                delete(),
                delete(),
                Seen::Text("hi".to_owned()),
                Seen::Marker(Marker::InjectionEnd(E)),
            ]
        );
    }

    #[test]
    fn an_empty_expansion_still_gets_both_markers() {
        let request = InjectionRequest::new(E, Vec::new());
        let mut recorder = Recorder::default();
        let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
        assert_eq!(
            recorder.seen,
            [
                Seen::Marker(Marker::InjectionStart(E)),
                Seen::Marker(Marker::InjectionEnd(E))
            ]
        );
    }

    #[test]
    fn characters_typed_after_the_trigger_are_deleted_and_typed_again() {
        let request =
            InjectionRequest::new(E, vec![InjectionStep::DeleteBack(3), text("be right back")]);
        let mut recorder = Recorder::default();
        let typed = sealed(&['x', 'é', '👋']);
        let outcome = run(&request, &typed, &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 3 });
        let deletes = recorder.seen.iter().filter(|s| **s == delete()).count();
        assert_eq!(deletes, 6, "n + k");
        assert_eq!(
            &recorder.seen[7..],
            [
                Seen::Text("be right back".to_owned()),
                Seen::Text("xé👋".to_owned()),
                Seen::Marker(Marker::InjectionEnd(E)),
            ]
        );
    }

    #[test]
    fn a_palette_pick_deletes_only_the_typed_characters() {
        let request = InjectionRequest::new(E, vec![text("hello")]);
        let mut recorder = Recorder::default();
        let outcome = run(
            &request,
            &sealed(&['a', 'b']),
            &Strategy::DEFAULT,
            &mut recorder,
        );
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 2 });
        assert_eq!(
            recorder.seen,
            [
                Seen::Marker(Marker::InjectionStart(E)),
                delete(),
                delete(),
                Seen::Text("hello".to_owned()),
                Seen::Text("ab".to_owned()),
                Seen::Marker(Marker::InjectionEnd(E)),
            ]
        );
    }

    #[test]
    fn a_cancelled_hand_off_sends_nothing() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        typed.observe(&ListenerEvent::Boundary {
            kind: iamb_platform::core_types::BoundaryKind::Enter,
            seq: 1,
        });
        let request = InjectionRequest::new(E, vec![text("x")]);
        let mut recorder = Recorder::default();
        let outcome = run(&request, &typed, &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Cancelled(CancelReason::Boundary));
        assert!(recorder.seen.is_empty());
    }

    #[test]
    fn a_missing_start_marker_cancels_after_the_start_marker_only() {
        let typed = TypedHandoff::new();
        typed.arm(E);
        let request = InjectionRequest::new(E, vec![text("x")]);
        let mut recorder = Recorder::default();
        let outcome = run(&request, &typed, &Strategy::DEFAULT, &mut recorder);
        assert_eq!(
            outcome,
            InjectionOutcome::Cancelled(CancelReason::StartMarkerTimeout)
        );
        assert_eq!(recorder.seen, [Seen::Marker(Marker::InjectionStart(E))]);
    }

    #[test]
    fn the_hand_off_seals_while_the_injector_waits() {
        let typed = Arc::new(TypedHandoff::new());
        typed.arm(E);
        let writer = {
            let typed = Arc::clone(&typed);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(10));
                typed.observe(&ListenerEvent::char('z', 1));
                typed.observe(&ListenerEvent::InjectionStarted {
                    expansion: E,
                    seq: 2,
                });
            })
        };
        let request = InjectionRequest::new(E, vec![InjectionStep::DeleteBack(1), text("y")]);
        let mut recorder = Recorder::default();
        let outcome = inject_with_timeout(
            &request,
            &typed,
            &Strategy::DEFAULT,
            || Ok(()),
            || Ok(&mut recorder),
            Duration::from_secs(5),
        );
        writer.join().expect("the writer thread");
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 1 });
        assert_eq!(
            recorder.seen.last(),
            Some(&Seen::Marker(Marker::InjectionEnd(E)))
        );
    }

    #[test]
    fn without_permission_nothing_is_posted() {
        let request = InjectionRequest::new(E, vec![text("x")]);
        let mut opened = false;
        let outcome = inject_with_timeout(
            &request,
            &sealed(&[]),
            &Strategy::DEFAULT,
            || {
                Err(PlatformError::PermissionDenied(
                    PermissionKind::Accessibility,
                ))
            },
            || {
                opened = true;
                Err::<&mut Recorder, _>(PlatformError::Unsupported)
            },
            Duration::from_millis(10),
        );
        assert_eq!(
            outcome,
            InjectionOutcome::Failed(PlatformError::PermissionDenied(
                PermissionKind::Accessibility
            ))
        );
        assert!(!opened);
    }

    #[test]
    fn a_source_that_cannot_be_created_fails_before_the_start_marker() {
        let request = InjectionRequest::new(E, vec![text("x")]);
        let error = PlatformError::os(0, "CGEventSourceCreate");
        let outcome = inject_with_timeout(
            &request,
            &sealed(&[]),
            &Strategy::DEFAULT,
            || Ok(()),
            || Err::<&mut Recorder, _>(error.clone()),
            Duration::from_millis(10),
        );
        assert_eq!(outcome, InjectionOutcome::Failed(error));
    }

    #[test]
    fn a_refused_event_stops_the_output_without_an_end_marker() {
        let request = InjectionRequest::new(
            E,
            vec![
                InjectionStep::DeleteBack(1),
                text("a"),
                text("b"),
                text("c"),
            ],
        );
        // Calls: start marker, delete, «a», then «b» fails.
        for (fail_at, posted) in [(1, 0), (2, 1), (4, 3)] {
            let mut recorder = Recorder {
                fail_at: Some(fail_at),
                ..Recorder::default()
            };
            let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
            assert_eq!(
                outcome,
                InjectionOutcome::Failed(PlatformError::os(0, "CGEventCreateKeyboardEvent"))
            );
            assert_eq!(recorder.seen.len(), posted);
            assert!(
                !recorder
                    .seen
                    .contains(&Seen::Marker(Marker::InjectionEnd(E)))
            );
        }
        // A refused end marker is a failure too.
        let mut recorder = Recorder {
            fail_at: Some(6),
            ..Recorder::default()
        };
        let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
        assert!(matches!(outcome, InjectionOutcome::Failed(_)));
    }

    #[test]
    fn waits_pastes_and_key_delays() {
        let strategy = Strategy {
            key_delay_ms: 2,
            ..Strategy::DEFAULT
        };
        let request = InjectionRequest::new(
            E,
            vec![
                InjectionStep::DeleteBack(2),
                text("a"),
                InjectionStep::Wait(40),
                InjectionStep::Paste(ClipboardContent::text("pasted")),
                InjectionStep::PressKey {
                    key: KeyName::Enter,
                    shift: true,
                    times: 1,
                },
                InjectionStep::MoveCaretBack(1),
            ],
        );
        let mut recorder = Recorder::default();
        let outcome = run(&request, &sealed(&['q']), &strategy, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 1 });
        assert_eq!(
            recorder.seen,
            [
                Seen::Marker(Marker::InjectionStart(E)),
                delete(),
                Seen::Pause(2),
                delete(),
                Seen::Pause(2),
                delete(),
                Seen::Pause(2),
                Seen::Text("a".to_owned()),
                Seen::Pause(40),
                Seen::Paste(Some("pasted".to_owned()), 250),
                Seen::Key(keycode::RETURN, Mods::SHIFT),
                Seen::Pause(2),
                Seen::Key(keycode::LEFT, Mods::NONE),
                Seen::Pause(2),
                Seen::Text("q".to_owned()),
                Seen::Marker(Marker::InjectionEnd(E)),
            ]
        );
    }

    #[test]
    fn long_text_goes_by_paste_and_the_retyped_characters_by_keys() {
        let long = "word ".repeat(100);
        let request = InjectionRequest::new(E, vec![InjectionStep::DeleteBack(1), text(&long)]);
        let mut recorder = Recorder::default();
        let outcome = run(&request, &sealed(&['!']), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 1 });
        assert_eq!(
            recorder.seen,
            [
                Seen::Marker(Marker::InjectionStart(E)),
                delete(),
                delete(),
                Seen::Paste(Some(long), 250),
                Seen::Text("!".to_owned()),
                Seen::Marker(Marker::InjectionEnd(E)),
            ]
        );
    }

    #[test]
    fn a_hand_off_cleared_before_the_retype_cancels() {
        // The worker shut down mid-injection: the characters are gone, so they cannot be typed
        // again.
        struct Clearing<'a> {
            inner: &'a mut Recorder,
            typed: &'a TypedHandoff,
        }
        impl Output for Clearing<'_> {
            type Focus = u64;
            fn focus(&mut self) -> u64 {
                (&mut *self.inner).focus()
            }
            fn marker(&mut self, marker: Marker) -> Result<()> {
                (&mut *self.inner).marker(marker)
            }
            fn press(&mut self, key: KeyPress) -> Result<()> {
                (&mut *self.inner).press(key)
            }
            fn text(&mut self, chunk: &TextChunk) -> Result<()> {
                self.typed.clear();
                (&mut *self.inner).text(chunk)
            }
            fn pause(&mut self, duration: Duration) {
                (&mut *self.inner).pause(duration);
            }
            fn paste(&mut self, content: &ClipboardContent, settle: Duration) -> Result<()> {
                (&mut *self.inner).paste(content, settle)
            }
        }
        let typed = sealed(&['k']);
        let request = InjectionRequest::new(E, vec![text("x")]);
        let mut recorder = Recorder::default();
        let outcome = inject_with_timeout(
            &request,
            &typed,
            &Strategy::DEFAULT,
            || Ok(()),
            || {
                Ok(Clearing {
                    inner: &mut recorder,
                    typed: &typed,
                })
            },
            Duration::from_millis(10),
        );
        assert_eq!(outcome, InjectionOutcome::Cancelled(CancelReason::NotArmed));
        assert!(
            !recorder
                .seen
                .contains(&Seen::Marker(Marker::InjectionEnd(E)))
        );
    }

    /// Review 2.8, finding 1: the trigger key moved the focus while the start marker
    /// travelled.
    #[test]
    fn a_focus_moved_after_the_start_marker_gets_nothing() {
        let request =
            InjectionRequest::new(E, vec![InjectionStep::DeleteBack(3), text("be right back")]);
        let mut recorder = Recorder {
            move_focus: Some((1, 2)),
            ..Recorder::default()
        };
        let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(
            outcome,
            InjectionOutcome::Cancelled(CancelReason::FocusMoved)
        );
        assert_eq!(recorder.seen, [Seen::Marker(Marker::InjectionStart(E))]);
    }

    #[test]
    fn a_focus_moved_between_two_steps_gets_nothing_after_it() {
        let request = InjectionRequest::new(
            E,
            vec![
                InjectionStep::DeleteBack(1),
                text("a"),
                InjectionStep::Wait(40),
                text("b"),
            ],
        );
        // The start marker, the deletion, «a» and the pause are recorded; then it moves.
        let mut recorder = Recorder {
            move_focus: Some((4, 2)),
            ..Recorder::default()
        };
        let outcome = run(&request, &sealed(&['q']), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(
            outcome,
            InjectionOutcome::Cancelled(CancelReason::FocusMoved)
        );
        assert_eq!(
            recorder.seen,
            [
                Seen::Marker(Marker::InjectionStart(E)),
                delete(),
                delete(),
                Seen::Text("a".to_owned()),
                Seen::Pause(40),
            ]
        );
    }

    #[test]
    fn the_snippets_own_tab_ends_the_focus_check() {
        let request = InjectionRequest::new(
            E,
            vec![
                text("ann"),
                InjectionStep::PressKey {
                    key: KeyName::Tab,
                    shift: false,
                    times: 1,
                },
                text("next"),
            ],
        );
        // The Tab moves to the next field, as it should.
        let mut recorder = Recorder {
            move_focus: Some((3, 2)),
            ..Recorder::default()
        };
        let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
        assert_eq!(
            recorder.seen.last(),
            Some(&Seen::Marker(Marker::InjectionEnd(E)))
        );
        assert!(may_move_focus(KeyPress::plain(keycode::RETURN)));
        assert!(may_move_focus(KeyPress::plain(keycode::ESCAPE)));
        assert!(!may_move_focus(KeyPress::DELETE));
        assert!(!may_move_focus(KeyPress::plain(keycode::LEFT)));
    }

    #[test]
    fn a_plain_expansion_reads_the_focus_once_per_operation() {
        let request =
            InjectionRequest::new(E, vec![InjectionStep::DeleteBack(3), text("be right back")]);
        let mut recorder = Recorder::default();
        let outcome = run(&request, &sealed(&['x']), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 1 });
        // Start marker, the deletion, the text chunk, the retyped characters.
        assert_eq!(recorder.focus_reads, 4);
    }

    #[test]
    fn a_main_thread_that_does_not_answer_counts_as_unchanged() {
        let mut reads = SecureInputReads::default();
        assert_eq!(reads.read(|| None), None);
        assert_eq!(
            reads.read(|| unreachable!("not asked again without a first answer")),
            None
        );
        let mut reads = SecureInputReads::default();
        assert_eq!(reads.read(|| Some(false)), Some(false));
        assert_eq!(reads.read(|| None), Some(false), "a busy main thread");
        assert_eq!(reads.read(|| Some(true)), Some(true), "a password field");
    }

    /// Review 2.8, finding 14.
    #[test]
    fn text_is_typed_when_the_pasteboard_cannot_be_saved_whole() {
        let long = "word ".repeat(100);
        let request = InjectionRequest::new(E, vec![InjectionStep::DeleteBack(1), text(&long)]);
        let mut recorder = Recorder {
            paste_incomplete: true,
            ..Recorder::default()
        };
        let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 0 });
        let typed: String = recorder
            .seen
            .iter()
            .filter_map(|s| match s {
                Seen::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(typed, long);
        assert!(!recorder.seen.iter().any(|s| matches!(s, Seen::Paste(..))));
    }

    #[test]
    fn rich_content_is_not_pasted_when_the_pasteboard_cannot_be_saved_whole() {
        let request = InjectionRequest::new(
            E,
            vec![
                InjectionStep::Paste(ClipboardContent::rich("x", "<b>x</b>")),
                text("after"),
            ],
        );
        let mut recorder = Recorder {
            paste_incomplete: true,
            ..Recorder::default()
        };
        let outcome = run(&request, &sealed(&[]), &Strategy::DEFAULT, &mut recorder);
        assert_eq!(
            outcome,
            InjectionOutcome::Cancelled(CancelReason::ClipboardNotRestorable)
        );
        assert_eq!(recorder.seen, [Seen::Marker(Marker::InjectionStart(E))]);
    }

    #[test]
    fn a_long_burst_of_typed_characters_is_chunked() {
        let typed_after: Vec<char> = "abcdefghijklmnopqrstuvwxy".chars().collect();
        let request = InjectionRequest::new(E, Vec::new());
        let mut recorder = Recorder::default();
        let outcome = run(
            &request,
            &sealed(&typed_after),
            &Strategy::DEFAULT,
            &mut recorder,
        );
        assert_eq!(outcome, InjectionOutcome::Finished { retyped: 25 });
        let texts: Vec<&Seen> = recorder
            .seen
            .iter()
            .filter(|s| matches!(s, Seen::Text(_)))
            .collect();
        assert_eq!(
            texts,
            [
                &Seen::Text("abcdefghijklmnopqrst".to_owned()),
                &Seen::Text("uvwxy".to_owned())
            ]
        );
    }
}
