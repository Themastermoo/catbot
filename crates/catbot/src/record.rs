//! What a recording holds, how it is written, and what is kept out of it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{io::AsyncWriteExt, sync::mpsc};

/// One line of a recording.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
// JSON kept in its own order (stage 26) made a variant larger; records are few.
#[allow(clippy::large_enum_variant)]
pub enum Record {
    /// A finished request and its response.
    Http {
        id: u64,
        at: String,
        method: String,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        status: u16,
        ms: u128,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_body: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resp_type: Option<String>,
        resp_bytes: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        resp_body: Option<Value>,
    },
    /// A server-sent event stream opened (the interface's live updates).
    SseOpen { id: u64, at: String, path: String, #[serde(skip_serializing_if = "Option::is_none")] query: Option<String>, status: u16 },
    /// One chunk of an event stream, as it arrived.
    Sse { id: u64, at: String, data: Value },
    /// An event stream ended.
    SseClose { id: u64, at: String, bytes: usize },
    /// A websocket was opened (frames are passed through, not recorded).
    Ws { id: u64, at: String, path: String, status: u16 },
}

pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
}

/// Writes records to one file per run, from a background task, so no request waits on the disk.
#[derive(Clone)]
pub struct Recorder {
    tx: mpsc::UnboundedSender<Record>,
}

impl Recorder {
    pub async fn start(dir: &Path) -> std::io::Result<(Self, PathBuf)> {
        tokio::fs::create_dir_all(dir).await?;
        let file = dir.join(format!("{}.jsonl", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        let mut out = tokio::fs::OpenOptions::new().create(true).append(true).open(&file).await?;
        let (tx, mut rx) = mpsc::unbounded_channel::<Record>();
        tokio::spawn(async move {
            while let Some(record) = rx.recv().await {
                let Ok(mut line) = serde_json::to_vec(&record) else { continue };
                line.push(b'\n');
                if out.write_all(&line).await.is_err() {
                    eprintln!("catbot: could not write to the recording; recording stopped");
                    return;
                }
                let _ = out.flush().await;
            }
        });
        Ok((Self { tx }, file))
    }

    pub fn write(&self, record: Record) {
        let _ = self.tx.send(record);
    }
}

/// Field names whose values never go into a recording.
const SECRET_WORDS: &[&str] = &[
    "key", "token", "secret", "password", "passcode", "credential", "authorization", "cookie", "otp", "code",
];

fn is_secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|word| name.contains(word))
}

/// Replaces every string or number held under a secret-looking name with "[redacted]".
fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (name, inner) in map.iter_mut() {
                if is_secret_name(name) && (inner.is_string() || inner.is_number()) {
                    *inner = Value::String("[redacted]".into());
                } else {
                    redact(inner);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

/// The query string with secret-looking parameters' values replaced.
pub fn redact_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    let parts: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if is_secret_name(name) => format!("{name}=[redacted]"),
            _ => pair.to_string(),
        })
        .collect();
    Some(parts.join("&"))
}

/// A body as it goes into a recording: parsed and redacted JSON, readable text, or a size.
pub fn body_value(bytes: &[u8], total: usize, content_type: Option<&str>, cap: usize) -> Option<Value> {
    if total == 0 {
        return None;
    }
    let ty = content_type.unwrap_or("").to_ascii_lowercase();
    let complete = bytes.len() == total;
    let looks_json = ty.contains("json") || bytes.first().is_some_and(|b| *b == b'{' || *b == b'[');
    if complete
        && looks_json
        && let Ok(mut value) = serde_json::from_slice::<Value>(bytes)
    {
        redact(&mut value);
        return Some(value);
    }
    let texty = ty.starts_with("text/") || ty.contains("json") || ty.contains("xml") || ty.contains("javascript");
    match std::str::from_utf8(&bytes[..bytes.len().min(cap)]) {
        Ok(text) if texty || ty.is_empty() => {
            let mut text = text.to_string();
            if total > text.len() {
                text.push_str(&format!(" … [{} more bytes]", total - text.len()));
            }
            Some(Value::String(text))
        }
        _ => Some(Value::String(format!("[{total} bytes of {}]", if ty.is_empty() { "binary" } else { &ty }))),
    }
}

/// One chunk of an event stream: its JSON payloads redacted, everything else as text.
pub fn sse_value(chunk: &[u8], cap: usize) -> Value {
    let text = String::from_utf8_lossy(&chunk[..chunk.len().min(cap)]);
    let mut events = Vec::new();
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            match serde_json::from_str::<Value>(data.trim()) {
                Ok(mut value) => {
                    redact(&mut value);
                    events.push(value);
                }
                Err(_) => events.push(Value::String(line.to_string())),
            }
        } else if !line.is_empty() {
            events.push(Value::String(line.to_string()));
        }
    }
    if chunk.len() > cap {
        events.push(Value::String(format!("… [{} more bytes]", chunk.len() - cap)));
    }
    Value::Array(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_at_any_depth() {
        let mut value = serde_json::json!({
            "name": "Assistant",
            "apiKey": "sk-123",
            "nested": [{ "refreshToken": "abc", "model": "sonnet" }],
            "settings": { "password": 42 }
        });
        redact(&mut value);
        assert_eq!(value["name"], "Assistant");
        assert_eq!(value["apiKey"], "[redacted]");
        assert_eq!(value["nested"][0]["refreshToken"], "[redacted]");
        assert_eq!(value["nested"][0]["model"], "sonnet");
        assert_eq!(value["settings"]["password"], "[redacted]");
    }

    #[test]
    fn query_secrets_are_redacted() {
        assert_eq!(redact_query(Some("since=4&token=abc")).as_deref(), Some("since=4&token=[redacted]"));
        assert_eq!(redact_query(None), None);
    }

    #[test]
    fn bodies_become_json_text_or_a_size() {
        assert_eq!(body_value(b"{\"a\":1}", 7, Some("application/json"), 100).unwrap()["a"], 1);
        assert_eq!(body_value(b"hello", 5, Some("text/plain"), 100).unwrap(), "hello");
        assert_eq!(body_value(&[0, 159, 146, 150], 4, Some("image/png"), 100).unwrap(), "[4 bytes of image/png]");
        assert_eq!(body_value(b"", 0, None, 100), None);
    }
}
