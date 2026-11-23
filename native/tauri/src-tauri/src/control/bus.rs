//! MC-9: the app's one event bus. The control socket's `events` stream, its waits, the status hooks and the webhooks all read it;
//! MC-32's triggers will too. A slow reader loses the oldest events (it is told how many), never stalls the app.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use mighty_core::control::events::{ControlEvent, EventBody, PaneRef};
use tokio::sync::broadcast;

/// How many events a reader may fall behind before it loses the oldest.
const CAPACITY: usize = 512;

/// What the bus carries.
#[derive(Debug, Clone)]
pub enum BusEvent {
    /// A pane's state, attention or run end: what scripts, hooks and webhooks hear.
    Control(Arc<ControlEvent>),
    /// New output of a pane, in whole lines (a terminal's, without escapes; an agent's log entry), for `pane wait-output` only:
    /// never forwarded to the `events` stream, a hook or a webhook.
    Output { pane: String, text: Arc<str> },
}

pub struct Bus {
    sender: broadcast::Sender<BusEvent>,
    seq: AtomicU64,
}

impl Default for Bus {
    fn default() -> Bus {
        Bus { sender: broadcast::channel(CAPACITY).0, seq: AtomicU64::new(0) }
    }
}

impl Bus {
    /// A pane event, numbered and timed now, sent to every reader; the event as sent.
    pub fn publish(&self, pane: PaneRef, body: EventBody) -> Arc<ControlEvent> {
        let event = Arc::new(ControlEvent {
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            pane,
            body,
        });
        let _ = self.sender.send(BusEvent::Control(event.clone()));
        event
    }

    /// New output of `pane`, when someone waits for output.
    pub fn output(&self, pane: &str, text: &str) {
        if self.sender.receiver_count() > 0 && !text.is_empty() {
            let _ = self.sender.send(BusEvent::Output { pane: pane.to_string(), text: Arc::from(text) });
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.sender.subscribe()
    }

    /// Whether anyone reads the bus (output is only decoded and sent then).
    pub fn heard(&self) -> bool {
        self.sender.receiver_count() > 0
    }
}

/// The app's bus.
pub fn global() -> &'static Bus {
    static BUS: OnceLock<Bus> = OnceLock::new();
    BUS.get_or_init(Bus::default)
}

#[cfg(test)]
mod tests {
    use mighty_core::control::events::PaneState;

    use super::*;

    #[tokio::test]
    async fn every_reader_hears_each_event_in_order_and_output_only_while_heard() {
        let bus = Bus::default();
        bus.output("p1", "nobody listens");
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        let body = EventBody::State { from: PaneState::Idle, to: PaneState::Running, event: None, reason: None };
        let a = bus.publish(PaneRef { pane: "p1".into(), ..PaneRef::default() }, body.clone());
        let b = bus.publish(PaneRef { pane: "p2".into(), ..PaneRef::default() }, body);
        bus.output("p1", "line");
        assert_eq!((a.seq, b.seq), (1, 2));
        for reader in [&mut first, &mut second] {
            let BusEvent::Control(one) = reader.recv().await.unwrap() else { panic!() };
            let BusEvent::Control(two) = reader.recv().await.unwrap() else { panic!() };
            assert_eq!((one.pane.pane.as_str(), two.pane.pane.as_str()), ("p1", "p2"));
            let BusEvent::Output { pane, text } = reader.recv().await.unwrap() else { panic!() };
            assert_eq!((pane.as_str(), &*text), ("p1", "line"));
        }
        assert!(chrono::DateTime::parse_from_rfc3339(&a.at).is_ok());
    }
}
