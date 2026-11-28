//! What the app knows that changes while it's open: every bot, and each opened conversation
//! with the turn running in it. One event stream (`/api/events`) keeps it current.
//!
//! The frames are the reference interface's (`message`, `message.patch`, `runtime`, `bot`,
//! `bot.deleted`, `hello`); `apply` turns each into a change here. Frames for the reference
//! server's own bots pass by: this interface shows only Catbot's.

use std::collections::HashMap;

use dioxus::prelude::*;
use serde_json::Value;

use crate::bots::Bot;
use crate::chat::{Body, Held, Msg};

/// Maximum idle threads kept in memory for long-lived sessions.
const MAX_CACHED_THREADS: usize = 24;

/// One conversation as the app holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    pub messages: Vec<Msg>,
    /// Whether there are earlier messages to load.
    pub more: bool,
    /// Whether its messages have been fetched. Until then, saved-message frames wait in `since`
    /// (a running turn is still followed).
    pub loaded: bool,
    /// Messages and changes that came while it wasn't loaded. The fetch may have been answered
    /// before they were saved, so they're laid over it when it comes (a chat used to open with
    /// its newest lines missing).
    pub since: Vec<Msg>,
    /// The turn running now, if one is.
    pub live: Option<Live>,
    /// Monotonic touch stamp for LRU cache eviction.
    pub last_used: u64,
}

/// A running turn: the steps so far and the reply as it streams in. When the turn ends, the
/// saved messages take its place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub steps: Vec<LiveStep>,
    pub text: String,
    /// The bot answering, when the server says (stage 29.12): in a chat with several bots, who
    /// is working.
    pub bot_id: Option<String>,
    /// Whether what the person sends meanwhile can be steered into it (stage 29.14).
    pub steerable: bool,
    /// When it started (ms since the epoch), for how long it has run; 0 when not known.
    pub started: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveStep {
    pub id: String,
    pub name: String,
    /// None while it runs.
    pub ok: Option<bool>,
}

/// Everything live, in one place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub bots: Vec<Bot>,
    pub threads: HashMap<String, Thread>,
    /// The hand-offs running from each chat (stage 32.13), as the server lists them.
    pub running_handoffs: HashMap<String, Vec<Value>>,
    /// Monotonic counter for thread LRU timestamps.
    touch: u64,
    /// Goes up whenever the paired devices or the waiting pairing change (`devices` frames),
    /// so the devices dialog knows to look again.
    /// Counts `engines` frames: an AI's key was saved or forgotten (the bot settings fetch again).
    pub engines_changed: u64,
    /// Bumped when the limits change (a `limits` frame).
    pub limits_changed: u64,
    pub local_ai_changed: u64,
    pub handoffs_changed: u64,
    pub chief_changed: u64,
    pub skills_changed: u64,
    pub email_changed: u64,
    pub pages_changed: u64,
    pub triggers_changed: u64,
    /// The task board changed (stage 30.3).
    pub board_changed: u64,
    pub apps_changed: u64,
    pub computers_changed: u64,
    pub devices_changed: u64,
    /// Counts `inbox` frames: the Inbox's list changed (read it again).
    pub inbox_changed: u64,
    /// Counts `departments` frames.
    pub departments_changed: u64,
    /// Counts `team-map` frames: a hand-off started or ended.
    pub map_changed: u64,
    /// Counts `scheduled` frames: a message was scheduled, changed, cancelled or sent.
    pub scheduled_changed: u64,
    /// Counts `voice` frames: voice settings, keys or downloads changed.
    pub voice_changed: u64,
    /// Counts `update` frames: a download, install or going back moved on (stage 28.4).
    pub update_changed: u64,
    /// Counts `archived-bots` frames: a bot was put away or brought back.
    pub archived_changed: u64,
}

/// Fine-grained invalidation signals so domain updates never wake unrelated components or
/// trigger HTTP fetches during streaming tokens.
#[derive(Clone, Copy)]
pub struct Revisions {
    pub engines: Signal<u64>,
    pub limits: Signal<u64>,
    pub local_ai: Signal<u64>,
    pub handoffs: Signal<u64>,
    pub chief: Signal<u64>,
    pub skills: Signal<u64>,
    pub email: Signal<u64>,
    pub pages: Signal<u64>,
    pub triggers: Signal<u64>,
    pub board: Signal<u64>,
    pub apps: Signal<u64>,
    pub computers: Signal<u64>,
    pub devices: Signal<u64>,
    pub inbox: Signal<u64>,
    pub departments: Signal<u64>,
    pub map: Signal<u64>,
    pub scheduled: Signal<u64>,
    pub voice: Signal<u64>,
    pub update: Signal<u64>,
    pub archived: Signal<u64>,
}

impl Revisions {
    pub fn new() -> Self {
        Self {
            engines: Signal::new(0),
            limits: Signal::new(0),
            local_ai: Signal::new(0),
            handoffs: Signal::new(0),
            chief: Signal::new(0),
            skills: Signal::new(0),
            email: Signal::new(0),
            pages: Signal::new(0),
            triggers: Signal::new(0),
            board: Signal::new(0),
            apps: Signal::new(0),
            computers: Signal::new(0),
            devices: Signal::new(0),
            inbox: Signal::new(0),
            departments: Signal::new(0),
            map: Signal::new(0),
            scheduled: Signal::new(0),
            voice: Signal::new(0),
            update: Signal::new(0),
            archived: Signal::new(0),
        }
    }
}

/// Per-thread streaming turn signals, isolated from `Signal<State>` so token deltas only
/// re-render the active thread's `LiveTurn` view.
#[derive(Clone, Copy)]
pub struct LiveTurns(pub Signal<HashMap<String, Signal<Option<Live>>>>);

impl LiveTurns {
    pub fn new() -> Self {
        Self(Signal::new(HashMap::new()))
    }

    pub fn signal_for(&mut self, thread: &str) -> Signal<Option<Live>> {
        if let Some(sig) = self.0.peek().get(thread).copied() {
            return sig;
        }
        let sig = Signal::new(None);
        self.0.write().insert(thread.to_string(), sig);
        sig
    }

    pub fn clear_all(&mut self) {
        let signals: Vec<Signal<Option<Live>>> = self.0.peek().values().copied().collect();
        for mut sig in signals {
            if sig.peek().is_some() {
                sig.set(None);
            }
        }
    }

    pub fn remove(&mut self, thread: &str) {
        if self.0.peek().contains_key(thread) {
            self.0.write().remove(thread);
        }
    }

    pub fn apply_runtime(&mut self, thread: &str, event: &Value) {
        if !runtime_mutates(event) {
            return;
        }
        let mut sig = self.signal_for(thread);
        let mut slot = sig.write();
        apply_runtime_event(&mut slot, event);
    }
}

fn runtime_mutates(event: &Value) -> bool {
    match event["type"].as_str() {
        Some("turn.started" | "turn.completed" | "turn.steerable" | "item.completed" | "runtime.error") => true,
        Some("content.delta") => event["streamKind"] == "assistant_text",
        Some("item.started") => event["itemType"] == "tool",
        _ => false,
    }
}

fn apply_runtime_event(slot: &mut Option<Live>, event: &Value) {
    match event["type"].as_str() {
        Some("turn.started") => {
            *slot = Some(Live {
                bot_id: event["botId"].as_str().map(str::to_string),
                started: crate::chat::js_now(),
                ..Live::default()
            });
        }
        Some("turn.completed") => *slot = None,
        Some("turn.steerable") => {
            let live = slot.get_or_insert_with(Live::default);
            live.steerable = event["steerable"] == true;
        }
        Some("content.delta") if event["streamKind"] == "assistant_text" => {
            let live = slot.get_or_insert_with(Live::default);
            live.text.push_str(event["delta"].as_str().unwrap_or_default());
        }
        Some("item.started") if event["itemType"] == "tool" => {
            let live = slot.get_or_insert_with(Live::default);
            let title = event["title"].as_str().unwrap_or("Tool");
            let summary = event["summary"].as_str().unwrap_or_default();
            live.steps.push(LiveStep {
                id: event["itemId"].as_str().unwrap_or_default().to_string(),
                name: format!("{title} {summary}").trim().to_string(),
                ok: None,
            });
        }
        Some("item.completed") => {
            if let Some(live) = slot.as_mut() {
                let id = event["itemId"].as_str().unwrap_or_default();
                if let Some(step) = live.steps.iter_mut().find(|s| s.id == id) {
                    step.ok = Some(event["ok"] != false);
                }
            }
        }
        Some("runtime.error") => {
            let live = slot.get_or_insert_with(Live::default);
            live.steps.push(LiveStep {
                id: format!("error-{}", live.steps.len()),
                name: event["message"].as_str().unwrap_or("Something went wrong").to_string(),
                ok: Some(false),
            });
        }
        _ => {}
    }
}

/// Applies one SSE frame with fine-grained signal routing: domain counters bump only their own
/// `Revisions` signal, streaming `runtime` frames update only that thread's `LiveTurns` signal,
/// and no-op frames avoid writing to `Signal<State>` altogether.
pub fn apply_frame(mut state: Signal<State>, mut rev: Revisions, mut live_turns: LiveTurns, frame: &Value) -> After {
    match frame["kind"].as_str() {
        Some("hello") => After::Resync,
        Some("devices") => { *rev.devices.write() += 1; After::Nothing }
        Some("engines") => { *rev.engines.write() += 1; After::Nothing }
        Some("limits") => { *rev.limits.write() += 1; After::Nothing }
        Some("local-ai") => { *rev.local_ai.write() += 1; After::Nothing }
        Some("handoffs") => { *rev.handoffs.write() += 1; After::Nothing }
        Some("chief") => { *rev.chief.write() += 1; After::Nothing }
        Some("skills") => { *rev.skills.write() += 1; After::Nothing }
        Some("email") => { *rev.email.write() += 1; After::Nothing }
        Some("pages") => { *rev.pages.write() += 1; After::Nothing }
        Some("triggers") => { *rev.triggers.write() += 1; After::Nothing }
        Some("board") => { *rev.board.write() += 1; After::Nothing }
        Some("apps") => { *rev.apps.write() += 1; After::Nothing }
        Some("computers") => { *rev.computers.write() += 1; After::Nothing }
        Some("inbox") => { *rev.inbox.write() += 1; After::Nothing }
        Some("departments") => { *rev.departments.write() += 1; After::Nothing }
        Some("team-map") => { *rev.map.write() += 1; After::Nothing }
        Some("scheduled") => { *rev.scheduled.write() += 1; After::Nothing }
        Some("voice") => { *rev.voice.write() += 1; After::Nothing }
        Some("update") => { *rev.update.write() += 1; After::Nothing }
        Some("archived-bots") => { *rev.archived.write() += 1; After::Nothing }
        Some("runtime") => {
            let event = &frame["event"];
            let thread = event["threadId"].as_str().unwrap_or_default();
            if state.peek().owns_thread(thread) {
                live_turns.apply_runtime(thread, event);
            }
            After::Nothing
        }
        Some("bot") => {
            let wire = &frame["bot"];
            if wire["catbot"] != true {
                return After::Nothing;
            }
            let Some(newer) = Bot::from_wire(wire) else { return After::Nothing };
            let mut s = state.write();
            match s.bots.iter_mut().find(|b| b.id == newer.id) {
                Some(bot) => bot.update_from(newer),
                None => s.bots.push(newer),
            }
            After::Nothing
        }
        Some("bot.deleted") => {
            let id = frame["botId"].as_str().unwrap_or_default();
            let removed_threads: Vec<String> = state
                .peek()
                .bots
                .iter()
                .filter(|b| b.id == id)
                .flat_map(|b| std::iter::once(b.thread_id.clone()).chain(b.chats.iter().map(|c| c.id.clone())))
                .collect();
            for t in &removed_threads {
                live_turns.remove(t);
            }
            state.write().apply(frame)
        }
        Some("message" | "message.patch") => {
            let thread = frame["threadId"].as_str().unwrap_or_default();
            if !state.peek().owns_thread(thread) {
                return After::Nothing;
            }
            state.write().apply(frame)
        }
        Some("message.removed" | "handoffs-running") => state.write().apply(frame),
        _ => After::Nothing,
    }
}

/// What a frame asks of the app beyond changing `State`.
#[derive(Debug, PartialEq, Eq)]
pub enum After {
    Nothing,
    /// The stream (re)connected: what was missed must be fetched again.
    Resync,
}

impl State {
    fn owns_thread(&self, thread: &str) -> bool {
        self.bots.iter().any(|b| b.has_chat(thread))
    }

    /// Puts a fetched page of messages in place: the newest page replaces what was there,
    /// an earlier page goes in front.
    pub fn loaded(&mut self, thread: &str, page: Vec<Msg>, more: bool, earlier: bool) {
        self.touch += 1;
        let stamp = self.touch;
        let entry = self.threads.entry(thread.to_string()).or_default();
        entry.last_used = stamp;
        let more = if earlier {
            let mut all = page;
            all.append(&mut entry.messages);
            entry.messages = all;
            more
        } else {
            let mut all = page;
            // What came while it loaded: a newer version of a message in the page, or a message
            // newer than any in it. (Older ones the page has, as it stands now.)
            let newest = all.iter().filter_map(|m| order(&m.id)).max().unwrap_or(0);
            for late in std::mem::take(&mut entry.since) {
                match all.iter_mut().find(|m| same_message(m, &late)) {
                    Some(there) => *there = late,
                    None if order(&late.id).is_none_or(|n| n > newest) => all.push(late),
                    None => {}
                }
            }
            // Earlier messages already loaded stay in front of the page: fetching a chat again
            // (after a reconnect) brings only its newest page (Moo, 2026-11-27).
            let oldest = all.iter().filter_map(|m| order(&m.id)).min();
            let mut earlier: Vec<Msg> = Vec::new();
            let mut sending = Vec::new();
            for kept in entry.messages.drain(..) {
                if kept.id.starts_with("optimistic-") {
                    sending.push(kept);
                } else if oldest.is_some_and(|oldest| order(&kept.id).is_some_and(|n| n < oldest)) {
                    earlier.push(kept);
                }
            }
            let more = if earlier.is_empty() { more } else { entry.more };
            // The person's messages still on their way stay in view.
            for sending in sending {
                if !all.iter().any(|m| same_message(m, &sending)) {
                    all.push(sending);
                }
            }
            earlier.append(&mut all);
            entry.messages = earlier;
            more
        };
        entry.more = more;
        if !earlier {
            entry.loaded = true;
        }
        self.evict_idle_threads(thread);
    }

    fn evict_idle_threads(&mut self, keep: &str) {
        if self.threads.len() <= MAX_CACHED_THREADS {
            return;
        }
        let mut victims: Vec<(u64, String)> = self
            .threads
            .iter()
            .filter(|(id, t)| {
                id.as_str() != keep
                    && t.live.is_none()
                    && !t.messages.iter().any(|m| m.id.starts_with("optimistic-"))
            })
            .map(|(id, t)| (t.last_used, id.clone()))
            .collect();
        victims.sort_by_key(|(used, _)| *used);
        let to_drop = self.threads.len().saturating_sub(MAX_CACHED_THREADS);
        for (_, id) in victims.into_iter().take(to_drop) {
            self.threads.remove(&id);
        }
    }

    /// The person's message, shown at once while it's on its way; the saved one replaces it.
    pub fn sending(&mut self, thread: &str, message: Msg) {
        self.threads.entry(thread.to_string()).or_default().messages.push(message);
    }

    /// A message that couldn't be sent is taken back out.
    pub fn unsend(&mut self, thread: &str, send_id: &str) {
        if let Some(entry) = self.threads.get_mut(thread) {
            entry.messages.retain(|m| m.send_id.as_deref() != Some(send_id) || !m.id.starts_with("optimistic-"));
        }
    }

    pub fn apply(&mut self, frame: &Value) -> After {
        match frame["kind"].as_str() {
            Some("hello") => return After::Resync,
            Some("bot") => {
                let wire = &frame["bot"];
                if wire["catbot"] != true {
                    return After::Nothing;
                }
                let Some(newer) = Bot::from_wire(wire) else { return After::Nothing };
                match self.bots.iter_mut().find(|b| b.id == newer.id) {
                    Some(bot) => bot.update_from(newer),
                    None => self.bots.push(newer),
                }
            }
            Some("devices") => self.devices_changed += 1,
            Some("engines") => self.engines_changed += 1,
            Some("limits") => self.limits_changed += 1,
            Some("local-ai") => self.local_ai_changed += 1,
            Some("handoffs") => self.handoffs_changed += 1,
            Some("handoffs-running") => {
                if let Some(thread) = frame["threadId"].as_str() {
                    let list = frame["running"].as_array().cloned().unwrap_or_default();
                    if list.is_empty() {
                        self.running_handoffs.remove(thread);
                    } else {
                        self.running_handoffs.insert(thread.to_string(), list);
                    }
                }
            }
            Some("chief") => self.chief_changed += 1,
            Some("skills") => self.skills_changed += 1,
            Some("email") => self.email_changed += 1,
            Some("pages") => self.pages_changed += 1,
            Some("triggers") => self.triggers_changed += 1,
            Some("board") => self.board_changed += 1,
            Some("apps") => self.apps_changed += 1,
            Some("computers") => self.computers_changed += 1,
            Some("inbox") => self.inbox_changed += 1,
            Some("departments") => self.departments_changed += 1,
            Some("team-map") => self.map_changed += 1,
            Some("scheduled") => self.scheduled_changed += 1,
            Some("voice") => self.voice_changed += 1,
            Some("update") => self.update_changed += 1,
            Some("archived-bots") => self.archived_changed += 1,
            Some("bot.deleted") => {
                let id = frame["botId"].as_str().unwrap_or_default();
                let mut dead_threads = Vec::new();
                for bot in self.bots.iter().filter(|b| b.id == id) {
                    dead_threads.push(bot.thread_id.clone());
                    for chat in &bot.chats {
                        dead_threads.push(chat.id.clone());
                    }
                }
                self.bots.retain(|b| b.id != id);
                for thread in dead_threads {
                    self.threads.remove(&thread);
                    self.running_handoffs.remove(&thread);
                }
            }
            Some(kind @ ("message" | "message.patch")) => {
                let thread = frame["threadId"].as_str().unwrap_or_default();
                if !self.owns_thread(thread) {
                    return After::Nothing;
                }
                if let Some(bot) = self.bots.iter_mut().find(|b| b.has_chat(thread)) {
                    bot.note_message(&frame["message"]);
                }
                let Some(message) = Msg::from_wire(&frame["message"]) else { return After::Nothing };
                let entry = self.threads.entry(thread.to_string()).or_default();
                // Not loaded (yet): kept for when its fetch comes.
                let list = if entry.loaded { &mut entry.messages } else { &mut entry.since };
                match list.iter_mut().find(|m| same_message(m, &message)) {
                    Some(existing) => *existing = message,
                    None if kind == "message" || !entry.loaded => list.push(message),
                    None => {}
                }
                if !entry.loaded && entry.since.len() > SINCE_MAX {
                    let extra = entry.since.len() - SINCE_MAX;
                    entry.since.drain(..extra);
                }
            }
            // A waiting message taken back before it went (Moo, 2026-11-27).
            Some("message.removed") => {
                let thread = frame["threadId"].as_str().unwrap_or_default();
                let id = frame["messageId"].as_str().unwrap_or_default();
                if let Some(entry) = self.threads.get_mut(thread) {
                    entry.messages.retain(|m| m.id != id);
                    entry.since.retain(|m| m.id != id);
                }
            }
            Some("runtime") => {
                let event = &frame["event"];
                let thread = event["threadId"].as_str().unwrap_or_default();
                if !self.owns_thread(thread) || !runtime_mutates(event) {
                    return After::Nothing;
                }
                let entry = self.threads.entry(thread.to_string()).or_default();
                apply_runtime_event(&mut entry.live, event);
            }
            _ => {}
        }
        After::Nothing
    }
}

/// How many changes a chat that isn't loaded keeps for its fetch.
const SINCE_MAX: usize = 200;

/// The same message: by id, or the saved one for a message still on its way (by its sendId).
fn same_message(m: &Msg, other: &Msg) -> bool {
    m.id == other.id
        || (m.id.starts_with("optimistic-") && other.send_id.is_some() && m.send_id == other.send_id)
        || (other.id.starts_with("optimistic-") && m.send_id.is_some() && m.send_id == other.send_id)
}

/// A saved message's place in its chat ("cb-12" is 12); none for one still on its way.
fn order(id: &str) -> Option<i64> {
    id.strip_prefix("cb-")?.parse().ok()
}

/// A sendId for a message: a version 4 UUID, as the reference server wants. Made from
/// Math.random so it works on any address, where crypto.randomUUID needs a secure page.
pub fn send_id() -> String {
    let mut bytes = [0u8; 16];
    for b in &mut bytes {
        *b = (random() * 256.0) as u8;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(target_arch = "wasm32")]
fn random() -> f64 {
    js_sys::Math::random()
}

/// In the desktop app: splitmix64 over a counter that starts from the clock, so each run of
/// the app makes different ids. (They only need to be unique, not secret.)
#[cfg(not(target_arch = "wasm32"))]
fn random() -> f64 {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: OnceLock<AtomicU64> = OnceLock::new();
    let state = STATE.get_or_init(|| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
        AtomicU64::new(now ^ u64::from(std::process::id()).rotate_left(32))
    });
    let mut z = state.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// The person's message as it's shown before the server has it.
pub fn optimistic(text: &str, send_id: &str, now: f64, attachments: Vec<crate::attach::Attached>) -> Msg {
    Msg { id: format!("optimistic-{send_id}"), at: now, send_id: Some(send_id.to_string()), body: Body::Person(text.to_string()), attachments, reactions: Vec::new(), bot_id: None, scheduled: None, held: Held::No }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> State {
        let bot = Bot::from_wire(&json!({ "id": "b1", "threadId": "t1", "catbot": true, "name": "Kiwi", "messages": [] })).unwrap();
        let mut state = State { bots: vec![bot], ..Default::default() };
        state.loaded("t1", vec![], false, false);
        state
    }

    fn message(id: &str, extra: Value) -> Value {
        let mut m = json!({ "id": id, "at": 5.0, "role": "bot", "kind": "text", "text": "hi" });
        m.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({ "kind": "message", "threadId": "t1", "message": m })
    }

    #[test]
    fn a_sent_message_is_replaced_by_the_saved_one() {
        let mut s = state();
        s.sending("t1", optimistic("hello", "s-1", 1.0, Vec::new()));
        s.apply(&message("cb-9", json!({ "role": "user", "text": "hello", "sendId": "s-1" })));
        let messages = &s.threads["t1"].messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "cb-9");
        assert_eq!(s.bots[0].last.as_ref().unwrap().line, "hello");
    }

    #[test]
    fn messages_and_patches() {
        let mut s = state();
        s.apply(&message("cb-1", json!({})));
        s.apply(&message("cb-1", json!({})));
        assert_eq!(s.threads["t1"].messages.len(), 1, "the same message twice is one message");
        let mut patch = message("cb-1", json!({ "text": "edited" }));
        patch["kind"] = json!("message.patch");
        s.apply(&patch);
        assert_eq!(s.threads["t1"].messages[0].body, Body::Reply("edited".into()));
        let mut stray = message("cb-2", json!({}));
        stray["kind"] = json!("message.patch");
        s.apply(&stray);
        assert_eq!(s.threads["t1"].messages.len(), 1, "a patch never adds a message");
    }

    #[test]
    fn other_servers_bots_pass_by() {
        let mut s = state();
        s.apply(&json!({ "kind": "bot", "bot": { "id": "maus", "name": "MausBot" } }));
        let mut theirs = message("x", json!({}));
        theirs["threadId"] = json!("their-thread");
        s.apply(&theirs);
        s.apply(&json!({ "kind": "runtime", "event": { "type": "turn.started", "threadId": "their-thread" } }));
        assert_eq!(s.bots.len(), 1);
        assert!(!s.threads.contains_key("their-thread"));
    }

    #[test]
    fn what_comes_while_a_chat_loads_isnt_lost() {
        let mut s = State::default();
        s.bots.push(Bot::from_wire(&json!({ "id": "b1", "threadId": "t1", "catbot": true, "name": "Kiwi", "messages": [] })).unwrap());
        let said = |id: &str, text: &str| json!({ "kind": "message", "threadId": "t1", "message": { "id": id, "at": 1.0, "role": "bot", "kind": "text", "text": text } });
        // The chat is being fetched: its newest line, and a change to one it'll have, come meanwhile.
        s.apply(&said("cb-3", "third"));
        s.apply(&json!({ "kind": "message.patch", "threadId": "t1", "message": { "id": "cb-2", "at": 1.0, "role": "bot", "kind": "text", "text": "second, changed" } }));
        // The fetch was answered before they were saved.
        let page = |id: &str, text: &str| crate::chat::Msg::from_wire(&json!({ "id": id, "at": 1.0, "role": "bot", "kind": "text", "text": text })).unwrap();
        s.loaded("t1", vec![page("cb-1", "first"), page("cb-2", "second")], false, false);
        let texts: Vec<String> = s.threads["t1"].messages.iter().map(|m| match &m.body { crate::chat::Body::Reply(t) => t.clone(), _ => String::new() }).collect();
        assert_eq!(texts, ["first", "second, changed", "third"]);
        assert!(s.threads["t1"].since.is_empty());
        // Loaded: frames go straight in.
        s.apply(&said("cb-4", "fourth"));
        assert_eq!(s.threads["t1"].messages.len(), 4);
    }

    #[test]
    fn a_message_taken_back_leaves_the_chat() {
        let mut s = state();
        let page = |id: &str, text: &str| crate::chat::Msg::from_wire(&json!({ "id": id, "at": 1.0, "role": "user", "kind": "text", "text": text })).unwrap();
        s.loaded("t1", vec![page("cb-1", "keep"), page("cb-2", "take back")], false, false);
        s.apply(&json!({ "kind": "message.removed", "threadId": "t1", "messageId": "cb-2" }));
        assert_eq!(s.threads["t1"].messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["cb-1"]);
    }

    #[test]
    fn fetching_a_chat_again_keeps_its_earlier_messages() {
        let mut s = state();
        let page = |id: &str, text: &str| crate::chat::Msg::from_wire(&json!({ "id": id, "at": 1.0, "role": "bot", "kind": "text", "text": text })).unwrap();
        s.loaded("t1", vec![page("cb-5", "five"), page("cb-6", "six")], true, false);
        s.loaded("t1", vec![page("cb-3", "three"), page("cb-4", "four")], false, true);
        // A reconnect fetches the newest page again; what was scrolled back to stays, and so
        // does whether there's more before it.
        s.threads.get_mut("t1").unwrap().loaded = false;
        s.loaded("t1", vec![page("cb-6", "six"), page("cb-7", "seven")], true, false);
        let t = &s.threads["t1"];
        assert_eq!(t.messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["cb-3", "cb-4", "cb-5", "cb-6", "cb-7"]);
        assert!(!t.more, "nothing before cb-3, as its page said");
    }

    #[test]
    fn a_turn_streams_then_gives_way() {
        let mut s = state();
        let ev = |e: Value| {
            let mut e = e;
            e["threadId"] = json!("t1");
            json!({ "kind": "runtime", "event": e })
        };
        s.apply(&ev(json!({ "type": "turn.started", "botId": "b1" })));
        assert_eq!(s.threads["t1"].live.as_ref().unwrap().bot_id.as_deref(), Some("b1"));
        s.apply(&ev(json!({ "type": "item.started", "itemType": "tool", "itemId": "tu1", "title": "Read", "summary": "notes.txt" })));
        s.apply(&ev(json!({ "type": "content.delta", "streamKind": "assistant_text", "delta": "Hel", "itemId": "i" })));
        s.apply(&ev(json!({ "type": "content.delta", "streamKind": "reasoning_text", "delta": "(thinking)", "itemId": "i-t" })));
        s.apply(&ev(json!({ "type": "content.delta", "streamKind": "assistant_text", "delta": "lo", "itemId": "i" })));
        s.apply(&ev(json!({ "type": "item.completed", "itemId": "tu1", "ok": true })));
        let live = s.threads["t1"].live.clone().unwrap();
        assert_eq!(live.text, "Hello");
        assert_eq!(live.steps, vec![LiveStep { id: "tu1".into(), name: "Read notes.txt".into(), ok: Some(true) }]);
        s.apply(&ev(json!({ "type": "turn.completed", "ok": true })));
        assert!(s.threads["t1"].live.is_none());
    }

    #[test]
    fn bots_come_and_go() {
        let mut s = state();
        s.apply(&json!({ "kind": "bot", "bot": { "id": "b1", "catbot": true, "name": "Kiwi", "activity": "waiting-on-you", "busy": true } }));
        assert_eq!(s.bots[0].activity, crate::bots::Activity::WaitingOnYou);
        assert_eq!(s.bots[0].thread_id, "t1");
        s.apply(&json!({ "kind": "bot", "bot": { "id": "b2", "catbot": true, "name": "Fern", "threadId": "t2" } }));
        assert_eq!(s.bots.len(), 2);
        s.apply(&json!({ "kind": "bot.deleted", "botId": "b1" }));
        assert_eq!(s.bots.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Fern"]);
        assert_eq!(s.apply(&json!({ "kind": "hello" })), After::Resync);
    }

    #[test]
    fn earlier_pages_go_in_front() {
        let mut s = state();
        let m = |id: &str| Msg { id: id.into(), at: 0.0, send_id: None, body: Body::Reply(id.into()), attachments: Vec::new(), reactions: Vec::new(), bot_id: None, scheduled: None, held: Held::No };
        s.loaded("t1", vec![m("c"), m("d")], true, false);
        s.loaded("t1", vec![m("a"), m("b")], false, true);
        let ids: Vec<_> = s.threads["t1"].messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"]);
        assert!(!s.threads["t1"].more);
    }

    #[test]
    fn running_hand_offs_are_kept_per_chat() {
        let mut state = State::default();
        state.apply(&json!({ "kind": "handoffs-running", "threadId": "t1", "running": [{ "id": "h1", "from": "Scout", "to": "Kiwi" }] }));
        assert_eq!(state.running_handoffs["t1"][0]["to"], "Kiwi");
        state.apply(&json!({ "kind": "handoffs-running", "threadId": "t1", "running": [] }));
        assert!(!state.running_handoffs.contains_key("t1"), "ended");
        assert_eq!(state.handoffs_changed, 0, "not the Hand-offs settings' frame");
    }

    #[test]
    fn stray_runtime_events_do_not_stick_live_working() {
        let mut s = state();
        s.apply(&json!({ "kind": "runtime", "event": { "type": "request.opened", "threadId": "t1" } }));
        s.apply(&json!({ "kind": "runtime", "event": { "type": "item.completed", "threadId": "t1", "itemId": "x", "ok": true } }));
        assert!(s.threads["t1"].live.is_none());
    }

    #[test]
    fn earlier_load_does_not_mark_unloaded_thread_loaded() {
        let mut s = state();
        let m = |id: &str| Msg { id: id.into(), at: 0.0, send_id: None, body: Body::Reply(id.into()), attachments: Vec::new(), reactions: Vec::new(), bot_id: None, scheduled: None, held: Held::No };
        s.threads.get_mut("t1").unwrap().loaded = false;
        s.loaded("t1", vec![m("cb-1")], false, true);
        assert!(!s.threads["t1"].loaded, "reconnect reload must still fetch the newest page");
    }

    #[test]
    fn deleting_a_bot_cleans_up_its_threads() {
        let mut s = state();
        s.apply(&json!({ "kind": "handoffs-running", "threadId": "t1", "running": [{ "id": "h1" }] }));
        s.apply(&json!({ "kind": "bot.deleted", "botId": "b1" }));
        assert!(!s.threads.contains_key("t1"));
        assert!(!s.running_handoffs.contains_key("t1"));
    }

    #[test]
    fn send_ids_are_uuids() {
        let id = send_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(id, send_id());
    }
}
