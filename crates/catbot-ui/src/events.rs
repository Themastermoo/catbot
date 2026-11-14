//! The event stream from `catbot serve` (`/api/events`), as frames of JSON.

use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use serde_json::Value;

/// Follows the stream, handing each frame to `on_frame`, until it closes or fails.
/// The caller decides when to connect again.
pub async fn follow(mut on_frame: impl FnMut(Value)) {
    let Ok(mut source) = EventSource::new("/api/events") else { return };
    let Ok(mut frames) = source.subscribe("message") else { return };
    while let Some(next) = frames.next().await {
        let Ok((_, event)) = next else { break };
        if let Some(frame) = event.data().as_string().and_then(|data| serde_json::from_str::<Value>(&data).ok()) {
            on_frame(frame);
        }
    }
    source.close();
}
