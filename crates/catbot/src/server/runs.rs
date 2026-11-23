//! Each turn's run, kept for the bot panel's Activity and Inspector (MausBot's `ActivityPanel`
//! and Inspector: Run log, Events, Raw).
//!
//! A turn's live frames all pass through its `emit`; a `Recorder` keeps a trimmed copy as they
//! go (streamed text and thinking run together into one entry, long strings cut, at most
//! `MOST_EVENTS`), and the turn's end stores it as a run (`Store::add_run`, a bot's newest 200).
//!
//! `GET /api/bots/:id/runs` lists them; `GET /api/bots/:id/runs/:run` gives one with its events
//! and the run log made from them.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use hyper::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, fail};
use crate::store::{Bot, Run};

/// Most events a run keeps; past them it only counts.
pub const MOST_EVENTS: usize = 400;
/// Longest string kept in an event, and longest streamed text.
const LONGEST: usize = 1500;
const LONGEST_TEXT: usize = 6000;

/// A turn's events, kept as they're sent.
pub struct Recorder {
    started: Instant,
    started_at: String,
    inner: Mutex<Kept>,
}

#[derive(Default)]
struct Kept {
    events: Vec<Value>,
    dropped: usize,
}

impl Default for Recorder {
    fn default() -> Self {
        Recorder::new()
    }
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder { started: Instant::now(), started_at: chrono::Local::now().to_rfc3339(), inner: Mutex::new(Kept::default()) }
    }

    /// Keeps a copy of a frame of the turn (the event inside the runtime frame).
    pub fn push(&self, event: &Value) {
        let ms = self.started.elapsed().as_millis() as u64;
        let mut kept = self.inner.lock().expect("recorder lock");
        if event["type"] == "content.delta" {
            let kind = if event["streamKind"] == "reasoning_text" { "thinking" } else { "text" };
            let delta = event["delta"].as_str().unwrap_or_default();
            if let Some(last) = kept.events.last_mut().filter(|l| l["type"] == kind) {
                let text = last["text"].as_str().unwrap_or_default();
                if text.chars().count() < LONGEST_TEXT {
                    last["text"] = json!(format!("{text}{delta}"));
                }
                last["chars"] = json!(last["chars"].as_u64().unwrap_or(0) + delta.chars().count() as u64);
                return;
            }
            if kept.events.len() < MOST_EVENTS {
                kept.events.push(json!({ "ms": ms, "type": kind, "text": delta, "chars": delta.chars().count() }));
            } else {
                kept.dropped += 1;
            }
            return;
        }
        if kept.events.len() >= MOST_EVENTS {
            kept.dropped += 1;
            return;
        }
        let mut copy = trimmed(event);
        copy["ms"] = json!(ms);
        kept.events.push(copy);
    }

    /// Stores the run once the turn is over.
    #[allow(clippy::too_many_arguments)]
    pub fn finish(&self, app: &Arc<App>, bot: &Bot, thread: &str, turn_id: &str, outcome: &str, model: &str, tools: usize, errors: usize, cost_usd: Option<f64>) {
        let kept = std::mem::take(&mut *self.inner.lock().expect("recorder lock"));
        let mut events = kept.events;
        if kept.dropped > 0 {
            events.push(json!({ "ms": self.started.elapsed().as_millis() as u64, "type": "catbot.trimmed", "dropped": kept.dropped }));
        }
        let run = Run {
            bot_id: bot.id.clone(),
            conversation_id: thread.to_string(),
            turn_id: turn_id.to_string(),
            started_at: self.started_at.clone(),
            ended_at: chrono::Local::now().to_rfc3339(),
            outcome: outcome.to_string(),
            engine: bot.engine.clone(),
            model: model.to_string(),
            tools: tools as i64,
            errors: errors as i64,
            cost_usd,
            events: Value::Array(events),
            ..Run::default()
        };
        // (A store that fails says so itself.)
        let _ = app.with_store(|store| store.add_run(&run));
    }
}

/// An event with its long strings cut.
fn trimmed(value: &Value) -> Value {
    match value {
        Value::String(s) if s.chars().count() > LONGEST => json!(format!("{}…", s.chars().take(LONGEST).collect::<String>())),
        Value::Array(items) => Value::Array(items.iter().take(50).map(trimmed).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), trimmed(v))).collect()),
        other => other.clone(),
    }
}

/// The run log: a line for each thing that happened, with when (seconds into the turn).
pub fn log(events: &Value) -> Vec<Value> {
    let short = |s: &str, n: usize| {
        let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.chars().count() > n { format!("{}…", line.chars().take(n).collect::<String>()) } else { line }
    };
    let mut lines = Vec::new();
    let mut tools = std::collections::HashMap::new();
    for event in events.as_array().into_iter().flatten() {
        let at = event["ms"].as_u64().unwrap_or(0) as f64 / 1000.0;
        let s = |key: &str| event[key].as_str().unwrap_or_default().to_string();
        let (kind, words) = match event["type"].as_str().unwrap_or_default() {
            "turn.started" => ("start", "Started".to_string()),
            "session.started" => ("start", match event["model"].as_str() { Some(m) => format!("Working on {m}"), None => "Session ready".to_string() }),
            "text" => ("text", format!("Wrote: {}", short(&s("text"), 140))),
            "thinking" => ("thinking", format!("Thought ({} characters)", event["chars"].as_u64().unwrap_or(0))),
            "item.started" => {
                tools.insert(s("itemId"), s("title"));
                let summary = short(&s("summary"), 120);
                ("tool", if summary.is_empty() { format!("Used {}", s("title")) } else { format!("Used {}: {summary}", s("title")) })
            }
            "item.completed" if event["ok"] == false => {
                let name = tools.get(&s("itemId")).cloned().unwrap_or_else(|| "a tool".into());
                ("error", format!("{name} failed: {}", short(s("output").lines().next().unwrap_or_default(), 140)))
            }
            "request.opened" => ("ask", format!("Asked you: {}", short(&s("summary"), 140))),
            "request.resolved" => ("ask", format!("Answer: {}", if event["behavior"] == "allow" { "allowed" } else { "denied" })),
            "runtime.error" => ("error", format!("Error: {}", short(&s("message"), 200))),
            "turn.completed" => ("end", if event["stopReason"] == "interrupted" { "Stopped".to_string() } else if event["ok"] == true { "Finished".to_string() } else { "Finished with a problem".to_string() }),
            "catbot.trimmed" => ("note", format!("…and {} more events not kept", event["dropped"].as_u64().unwrap_or(0))),
            _ => continue,
        };
        lines.push(json!({ "at": at, "kind": kind, "text": words }));
    }
    lines
}

fn shown(run: &Run, title: Option<&str>) -> Value {
    let seconds = match (chrono::DateTime::parse_from_rfc3339(&run.started_at), chrono::DateTime::parse_from_rfc3339(&run.ended_at)) {
        (Ok(a), Ok(b)) => (b - a).num_milliseconds() as f64 / 1000.0,
        _ => 0.0,
    };
    json!({
        "id": run.id,
        "threadId": run.conversation_id,
        "chat": title,
        "turnId": run.turn_id,
        "startedAt": run.started_at,
        "endedAt": run.ended_at,
        "seconds": seconds,
        "outcome": run.outcome,
        "engine": run.engine,
        "model": run.model,
        "tools": run.tools,
        "errors": run.errors,
        "costUsd": run.cost_usd,
    })
}

/// `GET /api/bots/:id/runs` and `/runs/:run`.
pub fn route(app: &Arc<App>, bot: &Bot, rest: &[&str]) -> Answer {
    let titles: std::collections::HashMap<String, String> = app.with_store(|store| store.conversations(&bot.id)).unwrap_or_default().into_iter().filter_map(|c| Some((c.id, c.title?))).collect();
    match rest {
        [] => {
            let runs = app.try_store(|store| store.runs(&bot.id, crate::store::RUNS_KEPT as usize)).map_err(fail)?;
            Ok((StatusCode::OK, json!({ "runs": runs.iter().map(|r| shown(r, titles.get(&r.conversation_id).map(String::as_str))).collect::<Vec<_>>() })))
        }
        [id] => {
            let id: i64 = id.parse().map_err(|_| (StatusCode::NOT_FOUND, "no such run".to_string()))?;
            let run = app.try_store(|store| store.run(&bot.id, id)).map_err(fail)?.ok_or((StatusCode::NOT_FOUND, "no such run".to_string()))?;
            let mut out = shown(&run, titles.get(&run.conversation_id).map(String::as_str));
            out["log"] = json!(log(&run.events));
            out["events"] = run.events;
            Ok((StatusCode::OK, json!({ "run": out })))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such run".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_kept_short_and_told_plainly() {
        let record = Recorder::new();
        record.push(&json!({ "type": "turn.started" }));
        record.push(&json!({ "type": "session.started", "model": "claude-opus" }));
        record.push(&json!({ "type": "content.delta", "streamKind": "reasoning_text", "delta": "Let me " }));
        record.push(&json!({ "type": "content.delta", "streamKind": "reasoning_text", "delta": "look." }));
        record.push(&json!({ "type": "item.started", "itemType": "tool", "itemId": "t1", "title": "Bash", "summary": "ls -la" }));
        record.push(&json!({ "type": "item.completed", "itemType": "tool", "itemId": "t1", "ok": false, "output": "ls: no such file\nmore" }));
        record.push(&json!({ "type": "content.delta", "streamKind": "assistant_text", "delta": "Hello " }));
        record.push(&json!({ "type": "content.delta", "streamKind": "assistant_text", "delta": "there" }));
        record.push(&json!({ "type": "runtime.error", "message": "x".repeat(5000) }));
        record.push(&json!({ "type": "turn.completed", "ok": true }));
        let events = Value::Array(record.inner.lock().unwrap().events.clone());
        let list = events.as_array().unwrap();
        assert_eq!(list.len(), 8, "the streamed text ran together");
        assert_eq!(list[2]["text"], "Let me look.");
        assert_eq!(list[5]["text"], "Hello there");
        assert!(list[6]["message"].as_str().unwrap().chars().count() <= LONGEST + 1, "long strings are cut");
        let lines: Vec<String> = log(&events).iter().map(|l| l["text"].as_str().unwrap().to_string()).collect();
        assert_eq!(lines[1], "Working on claude-opus");
        assert_eq!(lines[2], "Thought (12 characters)");
        assert_eq!(lines[3], "Used Bash: ls -la");
        assert_eq!(lines[4], "Bash failed: ls: no such file");
        assert_eq!(lines[5], "Wrote: Hello there");
        assert_eq!(lines.last().unwrap(), "Finished");
    }

    #[test]
    fn a_long_turn_keeps_its_first_events_and_counts_the_rest() {
        let record = Recorder::new();
        for n in 0..MOST_EVENTS + 10 {
            record.push(&json!({ "type": "item.started", "itemId": n.to_string(), "title": "Read" }));
        }
        let kept = record.inner.lock().unwrap();
        assert_eq!((kept.events.len(), kept.dropped), (MOST_EVENTS, 10));
    }
}
