//! The event stream from `catbot serve` (`/api/events`), as frames of JSON: through the
//! browser's EventSource on the web, and read by hand from a streaming response in the
//! desktop app.

use serde_json::Value;

/// Follows the stream, handing each frame to `on_frame`, until it closes or fails.
/// The caller decides when to connect again.
#[cfg(target_arch = "wasm32")]
pub async fn follow(mut on_frame: impl FnMut(Value)) {
    use futures_util::StreamExt;
    use gloo_net::eventsource::futures::EventSource;
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

#[cfg(not(target_arch = "wasm32"))]
pub async fn follow(mut on_frame: impl FnMut(Value)) {
    use futures_util::StreamExt;
    let request = crate::api::request(reqwest::Method::GET, "/api/events").header("accept", "text/event-stream");
    let Ok(response) = request.send().await else { return };
    crate::api::check_paired(response.status().as_u16());
    if !response.status().is_success() {
        return;
    }
    let mut body = response.bytes_stream();
    let mut reader = SseReader::default();
    while let Some(Ok(chunk)) = body.next().await {
        for frame in reader.push(&chunk) {
            on_frame(frame);
        }
    }
}

/// Server-sent events, as bytes arrive in pieces: a frame is the `data:` lines of one block,
/// and a blank line ends a block. Buffering raw bytes preserves multi-byte UTF-8 split across
/// TCP chunks. (The desktop app's; a browser reads the stream itself.)
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Default)]
pub struct SseReader {
    buffer: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const MAX_SSE_BUFFER: usize = 16 * 1024 * 1024;

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl SseReader {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Value> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_SSE_BUFFER {
            self.buffer.clear();
            return Vec::new();
        }
        let mut frames = Vec::new();
        let mut cursor = 0;
        while let Some((end, sep_len)) = find_event_end(&self.buffer[cursor..]) {
            let block = String::from_utf8_lossy(&self.buffer[cursor..cursor + end]);
            cursor += end + sep_len;
            let data: Vec<&str> = block
                .split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l))
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|l| l.strip_prefix(' ').unwrap_or(l))
                .collect();
            if data.is_empty() {
                continue;
            }
            if let Ok(frame) = serde_json::from_str::<Value>(&data.join("\n")) {
                frames.push(frame);
            }
        }
        if cursor > 0 {
            self.buffer.drain(..cursor);
        }
        frames
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn find_event_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
            return Some((i, 2));
        }
        if bytes[i] == b'\r' && i + 3 < bytes.len() && &bytes[i..i + 4] == b"\r\n\r\n" {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

/// Waits before trying the stream again.
pub async fn pause(ms: u32) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(ms).await;
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(u64::from(ms))).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_across_pieces() {
        let mut reader = SseReader::default();
        assert!(reader.push(b"data: {\"kind\":\"hel").is_empty());
        let frames = reader.push(b"lo\"}\n\n: keepalive\ndata: {\"kind\":\"ping\"}\n\nid: cb1:2\r\ndata: {\"kind\":\"bot\"}\r\n\r\n");
        assert_eq!(frames, vec![json!({ "kind": "hello" }), json!({ "kind": "ping" }), json!({ "kind": "bot" })]);
        assert!(reader.push(b": only a comment\n\n").is_empty());
    }

    #[test]
    fn utf8_split_across_chunks_is_preserved() {
        let mut reader = SseReader::default();
        let raw = "data: {\"delta\":\"🦀…你好\"}\n\n".as_bytes();
        // Split right inside the 4-byte crab emoji.
        let split = raw.iter().position(|&b| b == 0xF0).unwrap() + 2;
        assert!(reader.push(&raw[..split]).is_empty());
        let frames = reader.push(&raw[split..]);
        assert_eq!(frames, vec![json!({ "delta": "🦀…你好" })]);
    }
}
