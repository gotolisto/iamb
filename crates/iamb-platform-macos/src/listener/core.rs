//! What the tap callback does with an event, free of any system call.
//!
//! The macOS glue in `tap.rs` reads a handful of numbers out of each event ([`RawEvent`]) and
//! hands them to [`ListenerCore::handle`]. Everything the listener decides is decided here,
//! so it is tested on every system with a fake tap switch, fake keyboard layouts and a
//! recording sink.
//!
//! Per event, in this order:
//!
//! 1. A notice that the system disabled the tap: enable it again at once and deliver
//!    `Overflow` ([`recovery`](super::recovery)).
//! 2. An event that carries an Iamb marker is Iamb's own (architecture §5): output is
//!    dropped, the injection markers are reported in stream order, a self-test marker
//!    completes the waiting self-test. None of them is translated.
//! 3. Any other event is real typing, including synthetic events of other software (see
//!    below): a key press is translated ([`translate`](super::translate)), a mouse button
//!    press is `Boundary(Click)`, key releases and modifier changes are not reported.
//!
//! Every reported event takes the next sequence number, in the callback.
//!
//! # Synthetic events of other software
//!
//! An event without an Iamb marker counts as typed by the user even when another program
//! posted it. The focused application cannot tell the difference and puts the text into the
//! document, and the engine's buffer has to mirror the document. Key remappers, on-screen and
//! switch-control keyboards, dictation and remote-desktop tools all type this way; for their
//! users every key is synthetic, and ignoring synthetic events would make Iamb deaf to them.
//! There is also no trustworthy way to tell: the fields that name an event's origin are set by
//! whoever posts it.
//!
//! # Privacy
//!
//! The core keeps no typed text. Between two events it holds the translator's dead-key state
//! and counters. [`RawEvent`] has no `Debug`, and nothing here formats a key or a character.

use std::sync::Arc;
use std::time::Instant;

use iamb_platform::core_types::BoundaryKind;
use iamb_platform::{EventSink, HotkeyId, ListenerEvent, Marker, SeqSource};

use super::inputs::{InputSnapshot, InputSources};
use super::modifiers::Modifiers;
use super::recovery::{DisableReason, Recovery, RecoveryStats, TapCheck, TapHealth, TapSwitch};
use super::selftest::SelfTests;
use super::stats::ListenerStats;
use super::translate::{KeyEffect, Translator};

/// What kind of event the tap delivered.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawKind {
    /// A key went down, or repeats.
    KeyDown,
    /// A key went up.
    KeyUp,
    /// A modifier key changed.
    FlagsChanged,
    /// A mouse button went down.
    MouseDown,
    /// The system's notice that it disabled the tap. The other fields mean nothing.
    TapDisabled(DisableReason),
    /// Anything else. Not reported.
    Other,
}

/// The numbers the listener reads from one event. No `Debug`: it names a key the user
/// pressed.
#[derive(Clone, Copy)]
pub(crate) struct RawEvent {
    /// The kind of event.
    pub(crate) kind: RawKind,
    /// The virtual key code (key events).
    pub(crate) key_code: u16,
    /// Whether a key press is a repeat of a held key.
    pub(crate) repeat: bool,
    /// The event's modifier flags.
    pub(crate) flags: u64,
    /// The type of the keyboard the event came from; 0 when unknown.
    pub(crate) keyboard_type: u32,
    /// The event-source user data, where Iamb's marker lives.
    pub(crate) user_data: i64,
}

impl RawEvent {
    /// An event of `kind` with every other field zero.
    pub(crate) const fn of(kind: RawKind) -> Self {
        Self {
            kind,
            key_code: 0,
            repeat: false,
            flags: 0,
            keyboard_type: 0,
            user_data: 0,
        }
    }
}

/// What a [`ListenerCore`] is built from.
pub(crate) struct CoreParts {
    /// The listener's sequence numbers, which outlive a restart.
    pub(crate) seq: Arc<SeqSource>,
    /// Where events go.
    pub(crate) sink: Box<dyn EventSink>,
    /// The switch of the event tap.
    pub(crate) tap: Box<dyn TapSwitch>,
    /// The keyboard layout and input-method state.
    pub(crate) inputs: Arc<dyn InputSources>,
    /// The rendezvous with `self_test`.
    pub(crate) self_tests: Arc<SelfTests>,
    /// The counters.
    pub(crate) stats: Arc<ListenerStats>,
}

/// The listener's logic and the little state it keeps. Lives on the listener thread.
pub(crate) struct ListenerCore {
    seq: Arc<SeqSource>,
    sink: Box<dyn EventSink>,
    tap: Box<dyn TapSwitch>,
    inputs: Arc<dyn InputSources>,
    self_tests: Arc<SelfTests>,
    stats: Arc<ListenerStats>,
    translator: Translator,
    recovery: Recovery,
    snapshot: Option<Arc<InputSnapshot>>,
    seen_generation: Option<u64>,
    reported_composing: Option<bool>,
    activity: bool,
}

impl ListenerCore {
    /// A core for a tap that was just enabled.
    pub(crate) fn new(parts: CoreParts) -> Self {
        Self {
            seq: parts.seq,
            sink: parts.sink,
            tap: parts.tap,
            inputs: parts.inputs,
            self_tests: parts.self_tests,
            stats: parts.stats,
            translator: Translator::new(),
            recovery: Recovery::new(),
            snapshot: None,
            seen_generation: None,
            reported_composing: None,
            activity: false,
        }
    }

    /// Called once when the tap is installed: takes the first input snapshot and reports
    /// whether an input method is selected, so the engine's view is right after a restart
    /// too.
    pub(crate) fn started(&mut self) {
        self.sync_inputs();
    }

    /// Handles one event from the tap. `entered` is when the callback was entered; the time
    /// from then until this returns is recorded as the callback's duration.
    pub(crate) fn handle(&mut self, raw: &RawEvent, entered: Instant) {
        self.dispatch(raw, entered);
        self.stats.record_callback(entered.elapsed());
    }

    fn dispatch(&mut self, raw: &RawEvent, now: Instant) {
        if let RawKind::TapDisabled(reason) = raw.kind {
            self.recovery.disabled(reason, now, self.tap.as_mut());
            self.lost_events();
            return;
        }
        if let Some(marker) = Marker::decode_i64(raw.user_data) {
            self.own_event(marker);
            return;
        }
        self.activity = true;
        self.sync_inputs();
        match raw.kind {
            RawKind::KeyDown => self.key_down(raw),
            RawKind::MouseDown => {
                self.translator.reset();
                let seq = self.seq.next();
                self.sink.deliver(ListenerEvent::Boundary {
                    kind: BoundaryKind::Click,
                    seq,
                });
            }
            RawKind::KeyUp | RawKind::FlagsChanged | RawKind::Other | RawKind::TapDisabled(_) => {}
        }
    }

    /// An event Iamb posted itself.
    fn own_event(&mut self, marker: Marker) {
        self.stats.count_own_event();
        match marker {
            Marker::Output => {}
            Marker::SelfTest(nonce) => self.self_tests.complete(nonce),
            Marker::InjectionStart(_) | Marker::InjectionEnd(_) => {
                if matches!(marker, Marker::InjectionStart(_)) {
                    // Iamb's output is about to go through the application's text input: a
                    // pending accent does not survive that.
                    self.translator.reset();
                }
                if let Some(event) = marker.listener_event(self.seq.next()) {
                    self.sink.deliver(event);
                }
            }
        }
    }

    fn key_down(&mut self, raw: &RawEvent) {
        self.stats.count_key_down();
        // While an input method owns the keys, no layout applies: nothing is text.
        let layout = self
            .snapshot
            .as_ref()
            .filter(|snapshot| !snapshot.input_method)
            .and_then(|snapshot| snapshot.layout.as_deref());
        let output = self.translator.key_down(
            raw.key_code,
            Modifiers::from_event_flags(raw.flags),
            raw.repeat,
            raw.keyboard_type,
            layout,
        );
        for ch in output.text() {
            let seq = self.seq.next();
            self.sink.deliver(ListenerEvent::Char { ch, seq });
        }
        match output.key_effect() {
            KeyEffect::None => {}
            KeyEffect::Named(name) => {
                let seq = self.seq.next();
                self.sink.deliver(ListenerEvent::named_key(name, seq));
            }
            KeyEffect::Boundary(kind) => {
                let seq = self.seq.next();
                self.sink.deliver(ListenerEvent::Boundary { kind, seq });
            }
        }
    }

    /// The watchdog's periodic look, on the listener thread, outside the callback.
    pub(crate) fn tick(&mut self, now: Instant) {
        self.check_tap(now);
        if self.activity {
            // Something was typed or clicked since the last look: make sure the layout is
            // still the one in use, in case the change notification did not arrive.
            self.activity = false;
            self.inputs.request_refresh();
        }
        self.sync_inputs();
    }

    /// Looks at the tap and enables it when it is disabled (the watchdog, and `self_test`).
    pub(crate) fn check_tap(&mut self, now: Instant) -> TapCheck {
        let check = self.recovery.check(now, self.tap.as_mut());
        if check == TapCheck::Reenabled {
            self.lost_events();
        }
        check
    }

    /// A registered hotkey was pressed: report it at this point of the stream.
    pub(crate) fn hotkey(&mut self, id: HotkeyId) {
        let seq = self.seq.next();
        self.sink.deliver(ListenerEvent::Hotkey { id, seq });
    }

    /// The input sources published a new snapshot.
    pub(crate) fn inputs_changed(&mut self) {
        self.sync_inputs();
    }

    /// The state of the tap and the recovery counters.
    pub(crate) const fn tap_health(&self) -> (TapHealth, RecoveryStats) {
        (self.recovery.health(), self.recovery.stats())
    }

    /// The tap was down: keys were lost, and what the application composed is unknown.
    fn lost_events(&mut self) {
        self.translator.reset();
        self.sink.deliver(ListenerEvent::Overflow);
    }

    fn sync_inputs(&mut self) {
        let generation = self.inputs.generation();
        if self.seen_generation == Some(generation) {
            return;
        }
        self.seen_generation = Some(generation);
        self.snapshot = self.inputs.current();
        // A pending accent belongs to the layout that was replaced.
        self.translator.reset();
        let composing = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.input_method);
        if self.reported_composing != Some(composing) {
            self.reported_composing = Some(composing);
            let seq = self.seq.next();
            self.sink
                .deliver(ListenerEvent::Composing { on: composing, seq });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iamb_platform::core_types::ExpansionId;

    use super::super::keycodes::vk;
    use super::super::modifiers::{FLAG_COMMAND, FLAG_OPTION, FLAG_SHIFT};
    use super::super::recovery::WATCHDOG_INTERVAL;
    use super::super::testing::{FakeInputs, FakeLayout, Rig, shown};
    use super::*;

    fn key(code: u16) -> RawEvent {
        RawEvent {
            key_code: code,
            ..RawEvent::of(RawKind::KeyDown)
        }
    }

    fn key_with(code: u16, flags: u64) -> RawEvent {
        RawEvent { flags, ..key(code) }
    }

    fn marked(kind: RawKind, marker: Marker) -> RawEvent {
        RawEvent {
            user_data: marker.encode_i64(),
            ..RawEvent::of(kind)
        }
    }

    /// A started core on the fake US layout; the initial `Composing(false)` is consumed.
    fn us_rig() -> Rig {
        let mut rig = Rig::new(FakeInputs::layout(FakeLayout::us()));
        rig.core.started();
        assert_eq!(rig.take(), ["composing:false#1"]);
        rig
    }

    #[test]
    fn typed_keys_become_characters_with_rising_sequence_numbers() {
        let mut rig = us_rig();
        for code in [vk::ANSI_H, vk::ANSI_I] {
            rig.send(&key(code));
        }
        rig.send(&key_with(vk::ANSI_1, FLAG_SHIFT));
        rig.send(&key(vk::SPACE));
        assert_eq!(rig.take(), ["h#2", "i#3", "!#4", " #5"]);
        assert_eq!(rig.stats.snapshot().key_downs, 4);
        assert_eq!(rig.stats.snapshot().callbacks, 4);
    }

    #[test]
    fn named_keys_clicks_and_shortcuts_are_classified() {
        let mut rig = us_rig();
        rig.send(&key(vk::DELETE));
        rig.send(&key(vk::RETURN));
        rig.send(&key(vk::TAB));
        rig.send(&key(vk::ESCAPE));
        rig.send(&key(vk::UP_ARROW));
        rig.send(&key(vk::HOME));
        rig.send(&key(vk::FORWARD_DELETE));
        rig.send(&key_with(vk::ANSI_C, FLAG_COMMAND));
        rig.send(&RawEvent::of(RawKind::MouseDown));
        rig.send(&key(vk::F5));
        assert_eq!(
            rig.take(),
            [
                "backspace#2",
                "boundary:Enter#3",
                "boundary:Tab#4",
                "boundary:Escape#5",
                "boundary:Arrow#6",
                "boundary:Navigation#7",
                "boundary:Other#8",
                "boundary:Shortcut#9",
                "boundary:Click#10",
                "boundary:Other#11",
            ]
        );
    }

    #[test]
    fn key_releases_and_modifier_changes_are_not_reported() {
        let mut rig = us_rig();
        rig.send(&RawEvent {
            key_code: vk::ANSI_A,
            ..RawEvent::of(RawKind::KeyUp)
        });
        rig.send(&RawEvent {
            key_code: vk::SHIFT,
            flags: FLAG_SHIFT,
            ..RawEvent::of(RawKind::FlagsChanged)
        });
        rig.send(&RawEvent::of(RawKind::Other));
        assert!(rig.take().is_empty());
        assert_eq!(rig.seq.last(), 1, "no sequence number was spent");
    }

    #[test]
    fn a_key_repeat_counts_as_a_key() {
        let mut rig = us_rig();
        rig.send(&key(vk::ANSI_A));
        for _ in 0..3 {
            rig.send(&RawEvent {
                repeat: true,
                ..key(vk::ANSI_A)
            });
        }
        assert_eq!(rig.take(), ["a#2", "a#3", "a#4", "a#5"]);
    }

    #[test]
    fn dead_keys_combine_across_events() {
        let mut rig = Rig::new(FakeInputs::layout(FakeLayout::spanish()));
        rig.core.started();
        rig.take();
        rig.send(&key(vk::ANSI_C));
        rig.send(&key(FakeLayout::SPANISH_ACUTE));
        // A key release and a modifier change between the two halves change nothing.
        rig.send(&RawEvent::of(RawKind::KeyUp));
        rig.send(&RawEvent::of(RawKind::FlagsChanged));
        rig.send(&key(vk::ANSI_E));
        rig.send(&key(FakeLayout::SPANISH_ACUTE));
        rig.send(&key(vk::ANSI_X));
        assert_eq!(rig.take(), ["c#2", "é#3", "´#4", "x#5"]);
    }

    #[test]
    fn a_click_ends_a_pending_dead_key() {
        let mut rig = Rig::new(FakeInputs::layout(FakeLayout::spanish()));
        rig.core.started();
        rig.take();
        rig.send(&key(FakeLayout::SPANISH_ACUTE));
        rig.send(&RawEvent::of(RawKind::MouseDown));
        rig.send(&key(vk::ANSI_E));
        assert_eq!(rig.take(), ["boundary:Click#2", "e#3"]);
    }

    #[test]
    fn iambs_own_output_is_dropped() {
        let mut rig = us_rig();
        for kind in [RawKind::KeyDown, RawKind::KeyUp, RawKind::MouseDown] {
            rig.send(&RawEvent {
                key_code: vk::ANSI_A,
                ..marked(kind, Marker::Output)
            });
        }
        assert!(rig.take().is_empty());
        assert_eq!(rig.seq.last(), 1);
        let stats = rig.stats.snapshot();
        assert_eq!((stats.own_events, stats.key_downs), (3, 0));
    }

    #[test]
    fn injection_markers_are_reported_in_stream_order() {
        let mut rig = us_rig();
        let expansion = ExpansionId::from_raw(41);
        rig.send(&key(vk::ANSI_A));
        rig.send(&marked(RawKind::KeyUp, Marker::InjectionStart(expansion)));
        rig.send(&RawEvent {
            key_code: vk::ANSI_X,
            ..marked(RawKind::KeyDown, Marker::Output)
        });
        // A real key typed during the injection keeps its place.
        rig.send(&key(vk::ANSI_B));
        rig.send(&marked(RawKind::KeyUp, Marker::InjectionEnd(expansion)));
        assert_eq!(rig.take(), ["a#2", "started:41#3", "b#4", "finished:41#5"]);
    }

    #[test]
    fn a_marker_is_honoured_on_any_kind_of_event() {
        let mut rig = us_rig();
        let expansion = ExpansionId::from_raw(2);
        for kind in [
            RawKind::KeyDown,
            RawKind::KeyUp,
            RawKind::FlagsChanged,
            RawKind::Other,
        ] {
            rig.send(&marked(kind, Marker::InjectionStart(expansion)));
        }
        assert_eq!(
            rig.take(),
            ["started:2#2", "started:2#3", "started:2#4", "started:2#5"]
        );
    }

    #[test]
    fn an_injection_start_ends_a_pending_dead_key() {
        let mut rig = Rig::new(FakeInputs::layout(FakeLayout::spanish()));
        rig.core.started();
        rig.take();
        rig.send(&key(FakeLayout::SPANISH_ACUTE));
        rig.send(&marked(
            RawKind::KeyUp,
            Marker::InjectionStart(ExpansionId::from_raw(1)),
        ));
        rig.send(&key(vk::ANSI_E));
        assert_eq!(rig.take(), ["started:1#2", "e#3"]);
    }

    #[test]
    fn a_self_test_marker_completes_the_test_and_reports_nothing() {
        let mut rig = us_rig();
        let nonce = rig.self_tests.begin();
        rig.send(&marked(RawKind::KeyUp, Marker::SelfTest(nonce)));
        assert!(rig.self_tests.wait_until(nonce, Instant::now()));
        assert!(rig.take().is_empty());
        assert_eq!(rig.seq.last(), 1);
        // Another process's self-test is not ours and is not typing either.
        rig.self_tests.end();
        let mine = rig.self_tests.begin();
        rig.send(&marked(
            RawKind::KeyUp,
            Marker::SelfTest(mine.wrapping_add(9)),
        ));
        assert!(!rig.self_tests.wait_until(mine, Instant::now()));
        assert!(rig.take().is_empty());
    }

    #[test]
    fn other_softwares_synthetic_events_count_as_typing() {
        let mut rig = us_rig();
        // User data that is not an Iamb marker: some other program's tag.
        for user_data in [1, 0x7E57, i64::MAX, -1, 0x4942] {
            rig.send(&RawEvent {
                user_data,
                ..key(vk::ANSI_A)
            });
        }
        assert_eq!(rig.take(), ["a#2", "a#3", "a#4", "a#5", "a#6"]);
    }

    #[test]
    fn a_disabled_tap_is_enabled_in_the_same_callback_and_the_loss_announced() {
        for reason in [DisableReason::Timeout, DisableReason::UserInput] {
            let mut rig = us_rig();
            rig.send(&key(vk::ANSI_A));
            rig.tap.system_disables();
            rig.send(&RawEvent::of(RawKind::TapDisabled(reason)));
            assert!(
                rig.tap.is_enabled(),
                "enabled again before the callback returned"
            );
            assert_eq!(rig.tap.enable_calls(), 1);
            rig.send(&key(vk::ANSI_B));
            assert_eq!(rig.take(), ["a#2", "overflow", "b#3"]);

            let (health, stats) = rig.core.tap_health();
            assert_eq!(health, TapHealth::Working);
            assert_eq!(stats.recoveries, 1);
            assert!(stats.longest_recovery.expect("one recovery") < Duration::from_secs(1));
        }
    }

    #[test]
    fn a_disable_notice_is_not_mistaken_for_a_marked_event() {
        let mut rig = us_rig();
        rig.tap.system_disables();
        // Whatever the fields of the notice hold, it is a notice.
        rig.send(&RawEvent {
            user_data: Marker::Output.encode_i64(),
            ..RawEvent::of(RawKind::TapDisabled(DisableReason::Timeout))
        });
        assert_eq!(rig.take(), ["overflow"]);
        assert!(rig.tap.is_enabled());
    }

    #[test]
    fn the_watchdog_finds_a_silently_disabled_tap_within_one_interval() {
        let mut rig = us_rig();
        let start = Instant::now();
        rig.core.tick(start);
        assert!(rig.take().is_empty());

        rig.tap.system_disables();
        rig.core.tick(start + WATCHDOG_INTERVAL);
        assert!(rig.tap.is_enabled());
        assert_eq!(rig.take(), ["overflow"]);
        rig.core.tick(start + WATCHDOG_INTERVAL * 2);
        assert!(rig.take().is_empty());
        assert_eq!(rig.core.tap_health().1.unannounced, 1);
        assert!(WATCHDOG_INTERVAL < Duration::from_secs(1));
    }

    #[test]
    fn a_lost_tap_announces_the_loss_once_until_it_returns() {
        let mut rig = us_rig();
        let start = Instant::now();
        rig.tap.system_disables();
        rig.tap.refuse_to_enable(true);
        rig.send(&RawEvent::of(RawKind::TapDisabled(
            DisableReason::UserInput,
        )));
        assert_eq!(rig.take(), ["overflow"]);
        for n in 1..=4 {
            assert_eq!(
                rig.core.check_tap(start + WATCHDOG_INTERVAL * n),
                TapCheck::StillDisabled
            );
        }
        assert!(rig.take().is_empty());
        assert!(matches!(rig.core.tap_health().0, TapHealth::Lost { .. }));

        rig.tap.refuse_to_enable(false);
        assert_eq!(
            rig.core.check_tap(start + WATCHDOG_INTERVAL * 5),
            TapCheck::Reenabled
        );
        assert_eq!(rig.take(), ["overflow"]);
        assert_eq!(rig.core.tap_health().0, TapHealth::Working);
    }

    #[test]
    fn losing_events_ends_a_pending_dead_key() {
        let mut rig = Rig::new(FakeInputs::layout(FakeLayout::spanish()));
        rig.core.started();
        rig.take();
        rig.send(&key(FakeLayout::SPANISH_ACUTE));
        rig.tap.system_disables();
        rig.send(&RawEvent::of(RawKind::TapDisabled(DisableReason::Timeout)));
        rig.send(&key(vk::ANSI_E));
        assert_eq!(rig.take(), ["overflow", "e#2"]);
    }

    #[test]
    fn hotkeys_take_their_place_in_the_stream() {
        let mut rig = us_rig();
        rig.send(&key(vk::ANSI_A));
        rig.core.hotkey(HotkeyId(7));
        rig.send(&key(vk::ANSI_B));
        assert_eq!(rig.take(), ["a#2", "hotkey:7#3", "b#4"]);
    }

    #[test]
    fn an_input_method_is_reported_as_composing_and_types_no_text() {
        let mut rig = us_rig();
        rig.send(&key(vk::ANSI_A));
        rig.inputs.publish(Some(FakeLayout::us()), true);
        // The change is reported before the first key that follows it.
        rig.send(&key(vk::ANSI_N));
        rig.send(&key(vk::ANSI_I));
        rig.send(&key(vk::RETURN));
        rig.send(&key(vk::DELETE));
        assert_eq!(
            rig.take(),
            [
                "a#2",
                "composing:true#3",
                "boundary:Other#4",
                "boundary:Other#5",
                "boundary:Enter#6",
                "backspace#7",
            ]
        );

        rig.inputs.publish(Some(FakeLayout::us()), false);
        rig.core.inputs_changed();
        assert_eq!(rig.take(), ["composing:false#8"]);
        rig.send(&key(vk::ANSI_A));
        assert_eq!(rig.take(), ["a#9"]);
    }

    #[test]
    fn the_composing_state_is_reported_at_start_whatever_it_is() {
        let mut rig = Rig::new(FakeInputs::input_method(FakeLayout::us()));
        rig.core.started();
        assert_eq!(rig.take(), ["composing:true#1"]);

        // No snapshot yet: not composing as far as anyone knows, and no text either.
        let mut rig = Rig::new(FakeInputs::unknown());
        rig.core.started();
        assert_eq!(rig.take(), ["composing:false#1"]);
        rig.send(&key(vk::ANSI_A));
        rig.send(&key_with(vk::ANSI_A, FLAG_OPTION));
        assert_eq!(rig.take(), ["boundary:Other#2", "boundary:Shortcut#3"]);
        // The layout arrives: the next key is text.
        rig.inputs.publish(Some(FakeLayout::us()), false);
        rig.send(&key(vk::ANSI_A));
        assert_eq!(rig.take(), ["a#4"]);
    }

    #[test]
    fn a_new_layout_is_used_from_the_next_key_and_drops_a_pending_accent() {
        let mut rig = Rig::new(FakeInputs::layout(FakeLayout::spanish()));
        rig.core.started();
        rig.take();
        rig.send(&key(vk::ANSI_Y));
        rig.send(&key(FakeLayout::SPANISH_ACUTE));
        rig.inputs.publish(Some(FakeLayout::german()), false);
        rig.send(&key(vk::ANSI_E));
        rig.send(&key(vk::ANSI_Y));
        rig.send(&key_with(vk::ANSI_L, FLAG_OPTION));
        // Same input-method state, so no `Composing` event; QWERTZ from here on.
        assert_eq!(rig.take(), ["y#2", "e#3", "z#4", "@#5"]);
    }

    #[test]
    fn the_watchdog_refreshes_the_layout_only_after_activity() {
        let mut rig = us_rig();
        let now = Instant::now();
        rig.core.tick(now);
        assert_eq!(rig.inputs.refresh_requests(), 0);
        rig.send(&key(vk::ANSI_A));
        rig.send(&key(vk::ANSI_B));
        rig.core.tick(now);
        assert_eq!(rig.inputs.refresh_requests(), 1);
        rig.core.tick(now);
        assert_eq!(rig.inputs.refresh_requests(), 1);
        // A modifier change is activity too: it may be the input-source shortcut.
        rig.send(&RawEvent::of(RawKind::FlagsChanged));
        rig.core.tick(now);
        assert_eq!(rig.inputs.refresh_requests(), 2);
        // Iamb's own events are not.
        rig.send(&marked(RawKind::KeyDown, Marker::Output));
        rig.core.tick(now);
        assert_eq!(rig.inputs.refresh_requests(), 2);
    }

    #[test]
    fn the_watchdog_picks_up_a_new_snapshot_without_a_key() {
        let mut rig = us_rig();
        rig.inputs.publish(Some(FakeLayout::us()), true);
        rig.core.tick(Instant::now());
        assert_eq!(rig.take(), ["composing:true#2"]);
    }

    #[test]
    fn a_full_sink_is_the_sinks_business() {
        // The core delivers every event; the overflow rule lives in the sink (ADR-0010).
        let (sink, receiver) = iamb_platform::event_channel(2);
        let mut rig = Rig::with_sink(FakeInputs::layout(FakeLayout::us()), Box::new(sink));
        rig.core.started();
        for code in [vk::ANSI_A, vk::ANSI_B, vk::ANSI_C] {
            rig.send(&key(code));
        }
        let first: Vec<ListenerEvent> = receiver.try_iter().collect();
        assert_eq!(shown(&first), ["composing:false#1", "a#2"]);
        rig.send(&key(vk::ANSI_D));
        let next: Vec<ListenerEvent> = receiver.try_iter().collect();
        assert_eq!(shown(&next), ["overflow", "d#5"]);
    }

    #[test]
    fn ten_thousand_keys_none_dropped_none_doubled() {
        let mut rig = us_rig();
        let letters = [vk::ANSI_A, vk::ANSI_B, vk::ANSI_C, vk::ANSI_D, vk::ANSI_E];
        for i in 0..10_000 {
            rig.send(&key(letters[i % letters.len()]));
        }
        let events = rig.take();
        assert_eq!(events.len(), 10_000);
        for (i, event) in events.iter().enumerate() {
            let letter = ['a', 'b', 'c', 'd', 'e'][i % 5];
            assert_eq!(*event, format!("{letter}#{}", i + 2));
        }
        assert_eq!(rig.stats.snapshot().key_downs, 10_000);
    }
}
