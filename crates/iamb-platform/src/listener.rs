//! The key listener: what it reports, where it sends it, and the trait the OS crates implement
//! (architecture §3.1, §4.1, §5; ADR-0010, ADR-0011).
//!
//! # Why the listener has its own event type
//!
//! The listener reports [`ListenerEvent`], not [`iamb_core::engine::Event`]:
//!
//! - an OS crate can only build what a keyboard hook can see. `FormSubmitted`,
//!   `SnapshotChanged`, `ExpandRequest`, `FocusChanged` or `SecureState` cannot come out of a
//!   hook by mistake, because the type has no such variants;
//! - a hotkey press reaches the listener thread (§4.5) and must keep its place in the key
//!   stream, but it is not an engine event;
//! - [`ListenerEvent`] is `Clone` and comparable, which scripted streams need;
//!   `Event` is neither, because it can carry a whole snapshot.
//!
//! Every key-stream variant converts to the engine event of the same name without loss
//! ([`ListenerEvent::route`]), and the payload types (`Sensitive<char>`, `BoundaryKind`,
//! `ExpansionId`) are the core's own, so there is no second translation step to get wrong.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TrySendError, sync_channel};
use std::time::Duration;

use iamb_core::engine::{BoundaryKind, Event, ExpansionId};
use iamb_core::sensitive::Sensitive;
use iamb_core::template::KeyName;

use crate::error::Result;
use crate::focus::FocusToken;
use crate::hotkeys::HotkeyId;

/// A capacity for the listener's channel that absorbs a burst of key repeats while the engine
/// thread is busy for a few milliseconds, without holding more than a moment of typing.
pub const DEFAULT_EVENT_CAPACITY: usize = 256;

/// One thing the listener observed, in the true order of the key stream.
///
/// `seq` is the listener's sequence number ([`SeqSource`]). `Debug` never shows a typed
/// character.
#[derive(Clone, PartialEq, Eq)]
pub enum ListenerEvent {
    /// A key produced text (after layout and dead-key handling), whatever modifiers were held.
    Char {
        /// The character.
        ch: Sensitive<char>,
        /// Sequence number.
        seq: u64,
    },
    /// Backspace was pressed.
    Backspace {
        /// Sequence number.
        seq: u64,
    },
    /// A key or click that produced no text.
    Boundary {
        /// Which kind.
        kind: BoundaryKind,
        /// Sequence number.
        seq: u64,
    },
    /// An input method started (`true`) or finished composing text.
    Composing {
        /// Whether composition is now active.
        on: bool,
        /// Sequence number.
        seq: u64,
    },
    /// The injector's start marker for an expansion went by.
    InjectionStarted {
        /// The expansion.
        expansion: ExpansionId,
        /// Sequence number.
        seq: u64,
    },
    /// The injector's end marker for an expansion went by.
    InjectionFinished {
        /// The expansion.
        expansion: ExpansionId,
        /// Sequence number.
        seq: u64,
    },
    /// A registered hotkey was pressed ([`Hotkeys`](crate::Hotkeys)).
    Hotkey {
        /// The id it was registered under.
        id: HotkeyId,
        /// Sequence number.
        seq: u64,
    },
    /// Events were lost: the sink was full, or the OS dropped events before the listener saw
    /// them (a disabled event tap, a hook that timed out).
    Overflow,
}

/// Where a [`ListenerEvent`] goes in the service.
#[derive(Debug)]
pub enum Routed {
    /// To the engine, as this event.
    Engine(Event),
    /// A hotkey press: the service decides what it opens (§4.5).
    Hotkey(HotkeyId),
}

impl ListenerEvent {
    /// A `Char` event.
    #[must_use]
    pub const fn char(ch: char, seq: u64) -> Self {
        Self::Char {
            ch: Sensitive::new(ch),
            seq,
        }
    }

    /// The event for a named key that the layout did not turn into text: Space is a `Char`,
    /// Backspace is `Backspace`, everything else is a `Boundary` of the matching kind. Shared
    /// by the OS listeners and the mock so they classify alike.
    #[must_use]
    pub const fn named_key(key: KeyName, seq: u64) -> Self {
        let kind = match key {
            KeyName::Space => return Self::char(' ', seq),
            KeyName::Backspace => return Self::Backspace { seq },
            KeyName::Enter => BoundaryKind::Enter,
            KeyName::Tab => BoundaryKind::Tab,
            KeyName::Escape => BoundaryKind::Escape,
            KeyName::Up | KeyName::Down | KeyName::Left | KeyName::Right => BoundaryKind::Arrow,
            KeyName::Home | KeyName::End | KeyName::PageUp | KeyName::PageDown => {
                BoundaryKind::Navigation
            }
            KeyName::Delete => BoundaryKind::Other,
        };
        Self::Boundary { kind, seq }
    }

    /// The sequence number; `None` for `Overflow`, which stands for events that have none.
    #[must_use]
    pub const fn seq(&self) -> Option<u64> {
        match self {
            Self::Char { seq, .. }
            | Self::Backspace { seq }
            | Self::Boundary { seq, .. }
            | Self::Composing { seq, .. }
            | Self::InjectionStarted { seq, .. }
            | Self::InjectionFinished { seq, .. }
            | Self::Hotkey { seq, .. } => Some(*seq),
            Self::Overflow => None,
        }
    }

    /// True for the events a person's hands produce in the focused field: `Char`, `Backspace`
    /// and `Boundary`.
    #[must_use]
    pub const fn is_real_key(&self) -> bool {
        matches!(
            self,
            Self::Char { .. } | Self::Backspace { .. } | Self::Boundary { .. }
        )
    }

    /// Converts the event for the service: the engine event of the same name, or the hotkey.
    #[must_use]
    pub fn route(self) -> Routed {
        Routed::Engine(match self {
            Self::Char { ch, seq } => Event::Char { ch, seq },
            Self::Backspace { seq } => Event::Backspace { seq },
            Self::Boundary { kind, seq } => Event::Boundary { kind, seq },
            Self::Composing { on, .. } => Event::Composing(on),
            Self::InjectionStarted { expansion, seq } => Event::InjectionStarted { expansion, seq },
            Self::InjectionFinished { expansion, seq } => {
                Event::InjectionFinished { expansion, seq }
            }
            Self::Overflow => Event::Overflow,
            Self::Hotkey { id, .. } => return Routed::Hotkey(id),
        })
    }
}

impl fmt::Debug for ListenerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char { seq, .. } => f
                .debug_struct("Char")
                .field("seq", seq)
                .finish_non_exhaustive(),
            Self::Backspace { seq } => f.debug_struct("Backspace").field("seq", seq).finish(),
            Self::Boundary { kind, seq } => f
                .debug_struct("Boundary")
                .field("kind", kind)
                .field("seq", seq)
                .finish(),
            Self::Composing { on, seq } => f
                .debug_struct("Composing")
                .field("on", on)
                .field("seq", seq)
                .finish(),
            Self::InjectionStarted { expansion, seq } => f
                .debug_struct("InjectionStarted")
                .field("expansion", expansion)
                .field("seq", seq)
                .finish(),
            Self::InjectionFinished { expansion, seq } => f
                .debug_struct("InjectionFinished")
                .field("expansion", expansion)
                .field("seq", seq)
                .finish(),
            Self::Hotkey { id, seq } => f
                .debug_struct("Hotkey")
                .field("id", id)
                .field("seq", seq)
                .finish(),
            Self::Overflow => f.write_str("Overflow"),
        }
    }
}

/// The source of listener sequence numbers.
///
/// A listener owns one for its whole life, including across [`KeyListener::stop`] and
/// [`KeyListener::start`], and takes one number per event it reports, in the hook callback.
/// Numbers start at 1, only grow, and are never reused; an event dropped by a full sink keeps
/// its number, so a gap marks a loss.
#[derive(Debug, Default)]
pub struct SeqSource(AtomicU64);

impl SeqSource {
    /// A source whose first number is 1.
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Takes the next number.
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The last number taken; 0 before the first.
    #[must_use]
    pub fn last(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Where a key went: the window (or at least the process) that had the keyboard focus when the
/// listener saw the key, as the OS told the listener at that moment.
///
/// The focus watcher reports a change some time after it happened (an out-of-context window
/// event on Windows, a notification through two run loops on macOS), and the keys typed in
/// that time reach the service first. The service compares a key's target with the last focus
/// change it delivered to the engine ([`is`](Self::is)) and does not let the engine see a key
/// that went somewhere else (the 2.8 review's finding 5). A listener that cannot tell where a
/// key went reports it without a target ([`EventSink::deliver`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyTarget {
    pid: u32,
    window: Option<u64>,
}

impl KeyTarget {
    /// The window of `token` (and its process): what the Windows listener and the mock know.
    #[must_use]
    pub const fn window(token: FocusToken) -> Self {
        Self {
            pid: token.pid(),
            window: Some(token.window()),
        }
    }

    /// Only the process: a key that went to some window of process `pid`.
    #[must_use]
    pub const fn process(pid: u32) -> Self {
        Self { pid, window: None }
    }

    /// The process.
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }

    /// The OS window number, when the listener knows it.
    #[must_use]
    pub const fn window_number(self) -> Option<u64> {
        self.window
    }

    /// True when the key went to the window `token` names: the same process and, when the
    /// window is known, the same window.
    #[must_use]
    pub fn is(self, token: &FocusToken) -> bool {
        self.pid == token.pid() && self.window.is_none_or(|w| w == token.window())
    }

    /// True when `self` and `other` can be the same place: the same process and, when both
    /// name a window, the same window. What the injectors compare before their start marker
    /// ([`FocusGuard::for_request`](crate::FocusGuard::for_request)), where one side may know
    /// only the process (macOS).
    #[must_use]
    pub fn same_place(self, other: Self) -> bool {
        self.pid == other.pid
            && match (self.window, other.window) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

/// Where the listener puts what it observes.
///
/// [`deliver`](Self::deliver) is called on the listener's OS loop thread, from inside the hook
/// or tap callback, so it must return at once: no blocking, no waiting on another thread, no
/// I/O. A sink that cannot take an event drops it and delivers [`ListenerEvent::Overflow`]
/// first the next time it can (ADR-0010); [`OverflowSink`] does exactly that around any
/// non-blocking send.
pub trait EventSink: Send {
    /// Takes one event. Never blocks.
    fn deliver(&mut self, event: ListenerEvent);

    /// Takes one event of the key stream with the window it went to ([`KeyTarget`]). The
    /// listeners call it for `Char`, `Backspace` and `Boundary` when the OS tells them where
    /// the key went. Never blocks. Sinks that have no use for the target need not implement
    /// it: by default the target is dropped and the event delivered.
    fn deliver_to(&mut self, event: ListenerEvent, target: KeyTarget) {
        let _ = target;
        self.deliver(event);
    }
}

impl<F: FnMut(ListenerEvent) + Send> EventSink for F {
    fn deliver(&mut self, event: ListenerEvent) {
        self(event);
    }
}

/// An [`EventSink`] over a non-blocking send, with the overflow rule of ADR-0010: an event
/// that cannot be sent is dropped and a flag is set; the next event that can be sent is
/// preceded by [`ListenerEvent::Overflow`].
///
/// `try_send` returns the event when the queue is full (or gone). The service wraps its own
/// channel this way, so the events of every thread can share one queue to the engine:
///
/// ```
/// use std::sync::mpsc::{TrySendError, sync_channel};
/// use iamb_platform::{EventSink, ListenerEvent, OverflowSink};
///
/// enum ToEngine { Key(ListenerEvent), Tick }
///
/// let (tx, rx) = sync_channel(1);
/// let mut sink = OverflowSink::new(move |event| {
///     tx.try_send(ToEngine::Key(event)).map_err(|e| match e {
///         TrySendError::Full(ToEngine::Key(event))
///         | TrySendError::Disconnected(ToEngine::Key(event)) => event,
///         _ => unreachable!("only keys are sent here"),
///     })
/// });
/// sink.deliver(ListenerEvent::char('a', 1));
/// sink.deliver(ListenerEvent::char('b', 2)); // the queue is full: dropped
/// assert!(sink.overflowed());
/// assert!(matches!(rx.recv(), Ok(ToEngine::Key(ListenerEvent::Char { seq: 1, .. }))));
/// sink.deliver(ListenerEvent::char('c', 3)); // room for one: the overflow notice
/// assert!(matches!(rx.recv(), Ok(ToEngine::Key(ListenerEvent::Overflow))));
/// ```
pub struct OverflowSink<F> {
    try_send: F,
    overflowed: bool,
}

impl<F> OverflowSink<F>
where
    F: FnMut(ListenerEvent) -> std::result::Result<(), ListenerEvent> + Send,
{
    /// A sink over `try_send`.
    pub const fn new(try_send: F) -> Self {
        Self {
            try_send,
            overflowed: false,
        }
    }

    /// True while a dropped event has not been announced yet.
    pub const fn overflowed(&self) -> bool {
        self.overflowed
    }
}

impl<F> EventSink for OverflowSink<F>
where
    F: FnMut(ListenerEvent) -> std::result::Result<(), ListenerEvent> + Send,
{
    fn deliver(&mut self, event: ListenerEvent) {
        if self.overflowed {
            if (self.try_send)(ListenerEvent::Overflow).is_err() {
                // Still full: this event is lost too, and the notice stays owed.
                return;
            }
            self.overflowed = false;
            if event == ListenerEvent::Overflow {
                return;
            }
        }
        if (self.try_send)(event).is_err() {
            self.overflowed = true;
        }
    }
}

impl<F> fmt::Debug for OverflowSink<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverflowSink")
            .field("overflowed", &self.overflowed)
            .finish_non_exhaustive()
    }
}

/// The sending end of [`event_channel`]: an [`EventSink`] with the overflow rule built in.
pub struct ChannelSink {
    inner: OverflowSink<TrySend>,
}

type TrySend = Box<dyn FnMut(ListenerEvent) -> std::result::Result<(), ListenerEvent> + Send>;

impl ChannelSink {
    /// True while a dropped event has not been announced yet.
    #[must_use]
    pub const fn overflowed(&self) -> bool {
        self.inner.overflowed()
    }
}

impl EventSink for ChannelSink {
    fn deliver(&mut self, event: ListenerEvent) {
        self.inner.deliver(event);
    }
}

impl fmt::Debug for ChannelSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelSink")
            .field("overflowed", &self.overflowed())
            .finish_non_exhaustive()
    }
}

/// A bounded queue of listener events: the sink for [`KeyListener::start`] and the receiving
/// end. A full queue drops events and announces the loss with [`ListenerEvent::Overflow`].
#[must_use]
pub fn event_channel(capacity: usize) -> (ChannelSink, Receiver<ListenerEvent>) {
    let (tx, rx) = sync_channel(capacity);
    let inner = OverflowSink::new(Box::new(move |event| {
        tx.try_send(event).map_err(|e| match e {
            TrySendError::Full(event) | TrySendError::Disconnected(event) => event,
        })
    }) as TrySend);
    (ChannelSink { inner }, rx)
}

/// Observes the keyboard (and mouse clicks) of the whole session.
///
/// One implementation per OS: low-level keyboard and mouse hooks on a message-loop thread on
/// Windows, an event tap on a run-loop thread on macOS. The listener owns that thread;
/// [`start`](Self::start) creates it and returns once the hook or tap is installed.
///
/// # Rules every implementation keeps
///
/// - **Observe only** (ADR-0010). It never blocks, delays or swallows a key. The work in the
///   callback is bounded: translate, take a sequence number, [`deliver`](EventSink::deliver),
///   return (under 1 ms at the 99th percentile).
/// - **Text is a `Char`** (§4.1). A key is a `Char` whenever translation yields text, whatever
///   modifiers are held, so `AltGr` and Option combinations are characters. A key that yields no
///   text is `Backspace` or a `Boundary`; a chord that yields no text is
///   `Boundary(Shortcut)`; a mouse button press is `Boundary(Click)`.
/// - **Translation leaves the target application alone** (§4.1). It uses the foreground
///   thread's layout without touching the kernel's dead-key state, and tracks dead keys itself.
/// - **Composition** by an input method is reported as `Composing`.
/// - **Its own output is invisible** (§5). Each event's marker is decoded with
///   [`Marker::decode`](crate::Marker::decode): output is dropped, injection markers are
///   reported, a self-test marker completes [`self_test`](Self::self_test).
/// - **Losses are announced.** When the OS reports that it dropped events or disabled the tap,
///   the listener recovers (re-enables the tap) and delivers `Overflow`.
/// - **Hotkeys** registered through [`Hotkeys`](crate::Hotkeys) are served by the same thread
///   and delivered as `Hotkey` through the same sink.
///
/// All methods take `&self` and the trait is `Send + Sync`: the worker shares one listener
/// between the thread that starts it and the supervision timer that tests it.
pub trait KeyListener: Send + Sync {
    /// Installs the hook or tap and starts delivering to `sink`.
    ///
    /// # Errors
    ///
    /// [`AlreadyRunning`](crate::PlatformError::AlreadyRunning) when started twice;
    /// [`PermissionDenied`](crate::PlatformError::PermissionDenied) when the OS refuses the tap
    /// for lack of Input Monitoring or Accessibility; [`Os`](crate::PlatformError::Os) when
    /// installation fails.
    fn start(&self, sink: Box<dyn EventSink>) -> Result<()>;

    /// Removes the hook or tap, stops the loop thread and drops the sink. Stopping a stopped
    /// listener is not an error. To reinstall after a failed self-test, stop and start again.
    ///
    /// # Errors
    ///
    /// [`Os`](crate::PlatformError::Os) when the OS refuses to remove the hook.
    fn stop(&self) -> Result<()>;

    /// Posts a marked no-op key event through the OS and waits until the listener's own hook
    /// sees it (§3.2: every 30 seconds, with a 1-second limit). Nothing is delivered to the
    /// sink.
    ///
    /// # Errors
    ///
    /// [`Timeout`](crate::PlatformError::Timeout) when the event was not seen in `timeout`: the
    /// OS removed or disabled the hook. [`NotRunning`](crate::PlatformError::NotRunning) when
    /// the listener is stopped.
    fn self_test(&self, timeout: Duration) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_character() {
        let shown = format!("{:?}", ListenerEvent::char('Q', 7));
        assert_eq!(shown, "Char { seq: 7, .. }");
        assert!(!shown.contains('Q'));
        assert_eq!(
            format!(
                "{:?}",
                ListenerEvent::Boundary {
                    kind: BoundaryKind::Click,
                    seq: 2
                }
            ),
            "Boundary { kind: Click, seq: 2 }"
        );
        assert_eq!(format!("{:?}", ListenerEvent::Overflow), "Overflow");
        for event in [
            ListenerEvent::Backspace { seq: 1 },
            ListenerEvent::Composing { on: true, seq: 1 },
            ListenerEvent::InjectionStarted {
                expansion: ExpansionId::from_raw(1),
                seq: 1,
            },
            ListenerEvent::InjectionFinished {
                expansion: ExpansionId::from_raw(1),
                seq: 1,
            },
            ListenerEvent::Hotkey {
                id: HotkeyId(1),
                seq: 1,
            },
        ] {
            assert!(format!("{event:?}").contains("seq: 1"));
        }
    }

    #[test]
    fn named_keys_classify_like_the_engine_expects() {
        assert_eq!(
            ListenerEvent::named_key(KeyName::Space, 1),
            ListenerEvent::char(' ', 1)
        );
        assert_eq!(
            ListenerEvent::named_key(KeyName::Backspace, 2),
            ListenerEvent::Backspace { seq: 2 }
        );
        let kind_of = |key| match ListenerEvent::named_key(key, 0) {
            ListenerEvent::Boundary { kind, .. } => Some(kind),
            _ => None,
        };
        assert_eq!(kind_of(KeyName::Enter), Some(BoundaryKind::Enter));
        assert_eq!(kind_of(KeyName::Tab), Some(BoundaryKind::Tab));
        assert_eq!(kind_of(KeyName::Escape), Some(BoundaryKind::Escape));
        assert_eq!(kind_of(KeyName::Delete), Some(BoundaryKind::Other));
        for key in [KeyName::Up, KeyName::Down, KeyName::Left, KeyName::Right] {
            assert_eq!(kind_of(key), Some(BoundaryKind::Arrow));
        }
        for key in [
            KeyName::Home,
            KeyName::End,
            KeyName::PageUp,
            KeyName::PageDown,
        ] {
            assert_eq!(kind_of(key), Some(BoundaryKind::Navigation));
        }
        // Every named key is classified.
        assert_eq!(KeyName::ALL.len(), 14);
    }

    #[test]
    fn seq_and_real_key() {
        assert_eq!(ListenerEvent::char('a', 9).seq(), Some(9));
        assert_eq!(ListenerEvent::Overflow.seq(), None);
        assert!(ListenerEvent::char('a', 1).is_real_key());
        assert!(ListenerEvent::Backspace { seq: 1 }.is_real_key());
        assert!(
            ListenerEvent::Boundary {
                kind: BoundaryKind::Arrow,
                seq: 1
            }
            .is_real_key()
        );
        assert!(!ListenerEvent::Composing { on: true, seq: 1 }.is_real_key());
        assert!(!ListenerEvent::Overflow.is_real_key());
        assert_eq!(
            ListenerEvent::Hotkey {
                id: HotkeyId(3),
                seq: 4
            }
            .seq(),
            Some(4)
        );
    }

    #[test]
    fn route_keeps_every_field() {
        let e = ExpansionId::from_raw(5);
        let routed = |event: ListenerEvent| match event.route() {
            Routed::Engine(event) => format!("{event:?}"),
            Routed::Hotkey(id) => format!("hotkey {}", id.0),
        };
        assert_eq!(
            routed(ListenerEvent::char('x', 3)),
            format!("{:?}", Event::char('x', 3))
        );
        assert!(matches!(
            ListenerEvent::char('x', 3).route(),
            Routed::Engine(Event::Char { ch, seq: 3 }) if *ch.expose() == 'x'
        ));
        assert_eq!(
            routed(ListenerEvent::Backspace { seq: 4 }),
            "Backspace { seq: 4 }"
        );
        assert_eq!(
            routed(ListenerEvent::Boundary {
                kind: BoundaryKind::Tab,
                seq: 5
            }),
            "Boundary { kind: Tab, seq: 5 }"
        );
        assert_eq!(
            routed(ListenerEvent::Composing { on: true, seq: 6 }),
            "Composing(true)"
        );
        assert_eq!(
            routed(ListenerEvent::InjectionStarted {
                expansion: e,
                seq: 7
            }),
            format!(
                "{:?}",
                Event::InjectionStarted {
                    expansion: e,
                    seq: 7
                }
            )
        );
        assert_eq!(
            routed(ListenerEvent::InjectionFinished {
                expansion: e,
                seq: 8
            }),
            format!(
                "{:?}",
                Event::InjectionFinished {
                    expansion: e,
                    seq: 8
                }
            )
        );
        assert_eq!(routed(ListenerEvent::Overflow), "Overflow");
        assert_eq!(
            routed(ListenerEvent::Hotkey {
                id: HotkeyId(2),
                seq: 9
            }),
            "hotkey 2"
        );
    }

    #[test]
    fn a_key_target_names_a_window_or_a_process() {
        let token = FocusToken::from_raw(42, 7);
        let exact = KeyTarget::window(token);
        assert_eq!((exact.pid(), exact.window_number()), (42, Some(7)));
        assert!(exact.is(&token));
        assert!(!exact.is(&FocusToken::from_raw(42, 8)), "another window");
        assert!(!exact.is(&FocusToken::from_raw(43, 7)), "another process");
        let process = KeyTarget::process(42);
        assert_eq!(process.window_number(), None);
        assert!(process.is(&token) && process.is(&FocusToken::from_raw(42, 8)));
        assert!(!process.is(&FocusToken::from_raw(43, 7)));

        // A sink without a use for the target gets the event.
        let mut seen = Vec::new();
        {
            let mut sink = |event: ListenerEvent| seen.push(event);
            sink.deliver_to(ListenerEvent::char('a', 1), exact);
        }
        assert_eq!(seen, [ListenerEvent::char('a', 1)]);
    }

    #[test]
    fn seq_source_counts_from_one() {
        let seq = SeqSource::new();
        assert_eq!(seq.last(), 0);
        assert_eq!(seq.next(), 1);
        assert_eq!(seq.next(), 2);
        assert_eq!(seq.last(), 2);
        assert_eq!(SeqSource::default().next(), 1);
    }

    #[test]
    fn closures_are_sinks() {
        let mut seen = Vec::new();
        {
            let mut sink = |event: ListenerEvent| seen.push(event);
            sink.deliver(ListenerEvent::char('a', 1));
            EventSink::deliver(&mut sink, ListenerEvent::Overflow);
        }
        assert_eq!(seen, [ListenerEvent::char('a', 1), ListenerEvent::Overflow]);
    }

    #[test]
    fn a_full_channel_drops_and_announces_overflow_next() {
        let (mut sink, rx) = event_channel(2);
        sink.deliver(ListenerEvent::char('a', 1));
        sink.deliver(ListenerEvent::char('b', 2));
        assert!(!sink.overflowed());
        sink.deliver(ListenerEvent::char('c', 3)); // dropped
        sink.deliver(ListenerEvent::char('d', 4)); // still full: dropped, notice still owed
        assert!(sink.overflowed());
        assert_eq!(format!("{sink:?}"), "ChannelSink { overflowed: true, .. }");
        assert_eq!(
            format!("{:?}", OverflowSink::new(|_| Ok(()))),
            "OverflowSink { overflowed: false, .. }"
        );

        assert_eq!(rx.try_recv(), Ok(ListenerEvent::char('a', 1)));
        // One free slot: the notice takes it, and the event behind it is dropped again.
        sink.deliver(ListenerEvent::char('e', 5));
        assert!(sink.overflowed());
        assert_eq!(rx.try_recv(), Ok(ListenerEvent::char('b', 2)));
        assert_eq!(rx.try_recv(), Ok(ListenerEvent::Overflow));
        assert!(rx.try_recv().is_err());

        // Room again: the notice for 'e' comes first, then the stream continues.
        sink.deliver(ListenerEvent::char('f', 6));
        assert!(!sink.overflowed());
        assert_eq!(rx.try_recv(), Ok(ListenerEvent::Overflow));
        assert_eq!(rx.try_recv(), Ok(ListenerEvent::char('f', 6)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn an_overflow_from_the_os_is_not_announced_twice() {
        let (mut sink, rx) = event_channel(1);
        sink.deliver(ListenerEvent::char('a', 1));
        sink.deliver(ListenerEvent::char('b', 2)); // dropped
        assert_eq!(rx.try_recv(), Ok(ListenerEvent::char('a', 1)));
        sink.deliver(ListenerEvent::Overflow); // the OS also lost events
        assert_eq!(rx.try_recv(), Ok(ListenerEvent::Overflow));
        assert!(rx.try_recv().is_err());
        assert!(!sink.overflowed());
    }

    #[test]
    fn a_sink_without_a_receiver_keeps_dropping() {
        let (mut sink, rx) = event_channel(4);
        drop(rx);
        sink.deliver(ListenerEvent::char('a', 1));
        sink.deliver(ListenerEvent::char('b', 2));
        assert!(sink.overflowed());
    }
}
