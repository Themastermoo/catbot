//! The live frames Catbot itself produces, fanned out to every open event stream.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use tokio::sync::broadcast;

pub struct Hub {
    tx: broadcast::Sender<Value>,
    seq: AtomicU64,
}

impl Default for Hub {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx, seq: AtomicU64::new(1) }
    }
}

impl Hub {
    /// Sends a frame to everyone listening. Message and runtime frames carry a `seq`, as the
    /// reference server's do.
    pub fn send(&self, mut frame: Value) {
        if matches!(frame["kind"].as_str(), Some("message" | "message.patch" | "runtime")) {
            frame["seq"] = json!(self.seq.fetch_add(1, Ordering::Relaxed));
        }
        let _ = self.tx.send(frame);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.tx.subscribe()
    }
}

/// A `runtime` frame: one event of a running turn, in the reference server's shape.
pub fn runtime(thread_id: &str, turn_id: &str, event: Value) -> Value {
    let mut base = json!({
        "eventId": format!("cb-ev-{}", uuid::Uuid::new_v4()),
        "provider": "claudeAgent",
        "providerInstanceId": super::wire::INSTANCE,
        "threadId": thread_id,
        "turnId": turn_id,
        "createdAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    if let (Value::Object(base), Value::Object(event)) = (&mut base, event) {
        base.extend(event);
    }
    json!({ "kind": "runtime", "event": base })
}
