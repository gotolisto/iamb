//! The key queue: listener events on their way from the hook or tap thread to the engine
//! thread (architecture §3.1, §8 rule 1; the 2.8 review's finding 3).
//!
//! Every key the listener sees crosses this queue, also keys typed into a password field, an
//! excluded application or under pause: the listener cannot know that the engine will drop
//! them. So the queue is one of the places typed characters live, and it keeps them the way the
//! keystroke buffer and the hand-off do:
//!
//! - the characters sit in one fixed allocation of [`KEY_QUEUE_CAPACITY`] slots that never
//!   moves or grows, and the worker locks it against swapping ([`region`](KeyQueue::region));
//! - [`pop`](KeyQueue::pop) overwrites the slot it empties, so a character lives only until the
//!   engine thread takes it; [`clear`](KeyQueue::clear) (worker stop) and `Drop` zero them all;
//! - the rest of an event (its kind, sequence number, the window it went to) is kept apart from
//!   the characters, with the character replaced by NUL, so a slot reused by an event of another
//!   kind cannot keep an old character either.
//!
//! The engine's own channel carries only [`EngineMsg::Key`], a notice without content, one per
//! queued event, so keys keep their place among the other messages to the engine. The sink
//! sends the notice and fills the slot under the queue's lock, and the engine thread takes the
//! event under the same lock after it received the notice, so each notice finds its event.
//! Nothing on this path allocates: the queue and the channel are bounded and preallocated.

use std::fmt;
use std::hint::black_box;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use iamb_core::sensitive::Sensitive;
use iamb_platform::{DEFAULT_EVENT_CAPACITY, EventSink, KeyTarget, ListenerEvent, MemoryRegion};

use super::EngineMsg;

/// How many listener events the queue holds: as many as the engine channel.
pub(crate) const KEY_QUEUE_CAPACITY: usize = DEFAULT_EVENT_CAPACITY;

/// An event as it waits in the queue, with its character (if any) replaced by NUL.
type Shape = (ListenerEvent, Option<KeyTarget>);

struct Ring {
    /// Always [`KEY_QUEUE_CAPACITY`] slots: the character of the `Char` event in the same slot
    /// of `shapes`, zero everywhere else.
    chars: Sensitive<Box<[char]>>,
    shapes: Box<[Option<Shape>]>,
    head: usize,
    len: usize,
}

impl Ring {
    fn zero(&mut self) {
        let chars = self.chars.expose_mut();
        chars.fill('\0');
        black_box(&mut **chars);
        self.shapes.fill(None);
        self.head = 0;
        self.len = 0;
    }
}

/// The queue between the listener's sink and the engine thread. See the [module docs](self).
pub(crate) struct KeyQueue {
    ring: Mutex<Ring>,
}

impl KeyQueue {
    /// An empty queue. Create it once and lock [`region`](Self::region).
    pub(crate) fn new() -> Self {
        Self {
            ring: Mutex::new(Ring {
                chars: Sensitive::new(vec!['\0'; KEY_QUEUE_CAPACITY].into_boxed_slice()),
                shapes: vec![None; KEY_QUEUE_CAPACITY].into_boxed_slice(),
                head: 0,
                len: 0,
            }),
        }
    }

    fn ring(&self) -> MutexGuard<'_, Ring> {
        // Nothing panics while the lock is held, and the ring is valid after every statement.
        self.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `event` and, under the same lock, tells the engine thread with `notify` (a
    /// non-blocking send that answers whether it got through). Gives the event back when the
    /// queue is full or the notice could not be sent; nothing is queued then.
    pub(crate) fn push(
        &self,
        event: ListenerEvent,
        target: Option<KeyTarget>,
        notify: impl FnOnce() -> bool,
    ) -> Result<(), ListenerEvent> {
        let mut ring = self.ring();
        if ring.len == KEY_QUEUE_CAPACITY || !notify() {
            return Err(event);
        }
        let at = (ring.head + ring.len) % KEY_QUEUE_CAPACITY;
        let shape = match event {
            ListenerEvent::Char { ch, seq } => {
                ring.chars.expose_mut()[at] = *ch.expose();
                ListenerEvent::char('\0', seq)
            }
            other => other,
        };
        ring.shapes[at] = Some((shape, target));
        ring.len += 1;
        Ok(())
    }

    /// Takes the oldest event, and zeroes the slot its character was in.
    pub(crate) fn pop(&self) -> Option<Shape> {
        let mut ring = self.ring();
        if ring.len == 0 {
            return None;
        }
        let at = ring.head;
        ring.head = (at + 1) % KEY_QUEUE_CAPACITY;
        ring.len -= 1;
        let (shape, target) = ring.shapes[at].take()?;
        let slot = &mut ring.chars.expose_mut()[at];
        let ch = std::mem::replace(slot, '\0');
        black_box(slot);
        let event = match shape {
            ListenerEvent::Char { seq, .. } => ListenerEvent::char(ch, seq),
            other => other,
        };
        Some((event, target))
    }

    /// Drops every queued event and zeroes every slot (worker stop).
    pub(crate) fn clear(&self) {
        self.ring().zero();
    }

    /// How many events wait.
    pub(crate) fn len(&self) -> usize {
        self.ring().len
    }

    /// The address range of the character storage, for the memory lock. It does not change
    /// during the queue's life.
    pub(crate) fn region(&self) -> MemoryRegion {
        MemoryRegion::of_slice(self.ring().chars.expose())
    }

    #[cfg(test)]
    fn raw_chars(&self) -> Vec<char> {
        self.ring().chars.expose().to_vec()
    }
}

impl Drop for KeyQueue {
    fn drop(&mut self) {
        self.ring
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .zero();
    }
}

impl fmt::Debug for KeyQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyQueue")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

/// The listener's sink: queues each event with the window it went to and sends the engine its
/// notice. Never blocks; a full queue drops the event and announces the loss with `Overflow`
/// first the next time there is room (ADR-0010, the rule of
/// [`OverflowSink`](iamb_platform::OverflowSink)).
pub(crate) struct KeySink {
    queue: Arc<KeyQueue>,
    tx: SyncSender<EngineMsg>,
    overflowed: bool,
}

impl KeySink {
    pub(crate) const fn new(queue: Arc<KeyQueue>, tx: SyncSender<EngineMsg>) -> Self {
        Self {
            queue,
            tx,
            overflowed: false,
        }
    }

    fn send(&self, event: ListenerEvent, target: Option<KeyTarget>) -> bool {
        self.queue
            .push(event, target, || self.tx.try_send(EngineMsg::Key).is_ok())
            .is_ok()
    }

    fn take(&mut self, event: ListenerEvent, target: Option<KeyTarget>) {
        if self.overflowed {
            if !self.send(ListenerEvent::Overflow, None) {
                // Still full: this event is lost too, and the notice stays owed.
                return;
            }
            self.overflowed = false;
            if matches!(event, ListenerEvent::Overflow) {
                return;
            }
        }
        if !self.send(event, target) {
            self.overflowed = true;
        }
    }
}

impl EventSink for KeySink {
    fn deliver(&mut self, event: ListenerEvent) {
        self.take(event, None);
    }

    fn deliver_to(&mut self, event: ListenerEvent, target: KeyTarget) {
        self.take(event, Some(target));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, sync_channel};

    use iamb_core::engine::BoundaryKind;
    use iamb_platform::FocusToken;

    use super::*;

    fn sink(capacity: usize) -> (KeySink, Arc<KeyQueue>, Receiver<EngineMsg>) {
        let queue = Arc::new(KeyQueue::new());
        let (tx, rx) = sync_channel(capacity);
        (KeySink::new(Arc::clone(&queue), tx), queue, rx)
    }

    fn notices(rx: &Receiver<EngineMsg>) -> usize {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|m| matches!(m, EngineMsg::Key))
            .count()
    }

    fn all_zero(queue: &KeyQueue) -> bool {
        let raw = queue.raw_chars();
        raw.len() == KEY_QUEUE_CAPACITY && raw.iter().all(|&c| c == '\0')
    }

    #[test]
    fn events_come_out_in_order_with_their_target() {
        let (mut sink, queue, rx) = sink(8);
        let target = KeyTarget::window(FocusToken::from_raw(7, 70));
        sink.deliver_to(ListenerEvent::char('q', 1), target);
        sink.deliver(ListenerEvent::Backspace { seq: 2 });
        sink.deliver_to(
            ListenerEvent::Boundary {
                kind: BoundaryKind::Click,
                seq: 3,
            },
            target,
        );
        assert_eq!(notices(&rx), 3);
        assert_eq!(queue.len(), 3);
        assert_eq!(
            queue.pop(),
            Some((ListenerEvent::char('q', 1), Some(target)))
        );
        assert_eq!(
            queue.pop(),
            Some((ListenerEvent::Backspace { seq: 2 }, None))
        );
        assert!(matches!(
            queue.pop(),
            Some((ListenerEvent::Boundary { seq: 3, .. }, Some(t))) if t == target
        ));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn a_taken_character_is_zeroed_at_once() {
        let (mut sink, queue, _rx) = sink(KEY_QUEUE_CAPACITY);
        for (seq, ch) in (1..).zip("secret".chars()) {
            sink.deliver(ListenerEvent::char(ch, seq));
        }
        assert_eq!(
            queue.raw_chars()[..6].iter().collect::<String>(),
            "secret",
            "held only while queued"
        );
        for expected in "secret".chars() {
            let (event, _) = queue.pop().expect("queued");
            assert!(matches!(event, ListenerEvent::Char { ch, .. } if *ch.expose() == expected));
        }
        assert!(all_zero(&queue), "every slot is zero once taken");
        // Around the end of the ring as well.
        for round in 0..3_u64 {
            for seq in 0..200 {
                sink.deliver(ListenerEvent::char('x', round * 1000 + seq));
            }
            while queue.pop().is_some() {}
            assert!(all_zero(&queue));
        }
    }

    #[test]
    fn other_kinds_never_hold_a_character() {
        let (mut sink, queue, _rx) = sink(KEY_QUEUE_CAPACITY);
        sink.deliver(ListenerEvent::char('z', 1));
        let _ = queue.pop();
        sink.deliver(ListenerEvent::Backspace { seq: 2 });
        assert!(all_zero(&queue));
        let ring = queue.ring();
        for (shape, _) in ring.shapes.iter().flatten() {
            assert!(!matches!(shape, ListenerEvent::Char { ch, .. } if *ch.expose() != '\0'));
        }
    }

    #[test]
    fn clear_and_the_full_queue() {
        let (mut sink, queue, rx) = sink(KEY_QUEUE_CAPACITY + 8);
        for seq in 0..KEY_QUEUE_CAPACITY as u64 + 2 {
            sink.deliver(ListenerEvent::char('k', seq));
        }
        // The queue held its capacity; the rest was dropped and the loss is owed.
        assert_eq!(queue.len(), KEY_QUEUE_CAPACITY);
        assert_eq!(notices(&rx), KEY_QUEUE_CAPACITY);
        assert!(sink.overflowed);
        let _ = queue.pop();
        sink.deliver(ListenerEvent::char('m', 900));
        // One slot: the notice took it, the event behind it is lost again.
        assert!(sink.overflowed);
        queue.clear();
        assert!(all_zero(&queue));
        assert_eq!(queue.len(), 0);
        sink.deliver(ListenerEvent::char('n', 901));
        assert_eq!(queue.pop(), Some((ListenerEvent::Overflow, None)));
        assert_eq!(queue.pop(), Some((ListenerEvent::char('n', 901), None)));
        assert!(!sink.overflowed);
    }

    #[test]
    fn a_full_channel_queues_nothing() {
        // The engine channel is full of other messages: the event is not queued either, so no
        // event waits without its notice.
        let (mut sink, queue, rx) = sink(1);
        sink.deliver(ListenerEvent::char('a', 1));
        sink.deliver(ListenerEvent::char('b', 2));
        assert_eq!(queue.len(), 1);
        assert!(sink.overflowed);
        assert_eq!(notices(&rx), 1);
        assert!(queue.raw_chars()[1..].iter().all(|&c| c == '\0'));
    }

    #[test]
    fn the_region_is_stable_and_debug_shows_a_count() {
        let (mut sink, queue, _rx) = sink(4);
        let region = queue.region();
        assert_eq!(region.len(), KEY_QUEUE_CAPACITY * size_of::<char>());
        sink.deliver(ListenerEvent::char('Q', 1));
        assert_eq!(format!("{queue:?}"), "KeyQueue { len: 1, .. }");
        let _ = queue.pop();
        queue.clear();
        assert_eq!(queue.region(), region);
    }
}
