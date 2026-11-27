//! The Inbox (stage 19.5): everything waiting on the person, in one list, as MausBot's inbox for a
//! computer of one person (`place-inbox.ts`).
//!
//! * An ask is a card open where it was asked: an approval a running turn waits on, a change a bot
//!   asks to make (the Chief's team changes, a page, an email, an app call, a skill), or a picture
//!   whose AI is still to be chosen. It is read where it was asked, so answering it here answers
//!   that very card. It is listed under "Needs you" until it is answered.
//! * News is a line of its own: a bot's `message_person` (a message to the person, which also lands
//!   in its chat), a trigger's result, or a trigger that failed. News is under "Info" until it's
//!   done with: a result once read, a message once the person writes in that chat, anything once
//!   marked Done.
//! * A chat a bot answered in since the person last looked is unread (`conversations.unread`);
//!   opening it reads it, and reads its news.
//!
//! Kept in the settings under `inbox`: the lines (30 days, 500 at most) and which asks were read.
//!
//! * `GET  /api/inbox`              the rows, newest first (open ones first), and the count
//! * `GET  /api/inbox/count`        how many wait on the person: asks, and news not yet read
//! * `POST /api/inbox/read-all`     every row read (no ask is answered)
//! * `POST /api/inbox/:id/read`     one row read
//! * `POST /api/inbox/:id/done`     a line of news done with (an ask has to be answered)
//!
//! Each change sends an `inbox` frame; a new message also carries its row, for a notification.

use std::collections::HashMap;
use std::sync::Mutex;

use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, wire};
use crate::store::{Conversation, Role, Store};

const KEY: &str = "inbox";
const LINES_KEPT: usize = 500;
const SEEN_KEPT: usize = 1000;
const KEEP_DAYS: i64 = 30;
const ROWS_SHOWN: usize = 200;
const TEXT_MAX: usize = 2000;
/// Messages from one chat in an hour: enough for real news, too few for a loop to bury the person.
pub const MESSAGES_HOURLY: usize = 10;

/// One change at a time to the kept lines.
static CHANGING: Mutex<()> = Mutex::new(());
/// When each chat last sent the person messages, for the hourly limit.
static SENT: Mutex<Option<HashMap<String, Vec<std::time::Instant>>>> = Mutex::new(None);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub id: String,
    /// "message", "result" or "notice".
    pub kind: String,
    pub bot_id: String,
    pub thread_id: String,
    pub text: String,
    /// When, in ms since the epoch.
    pub at: i64,
    #[serde(default)]
    pub seen: bool,
    #[serde(default)]
    pub done: bool,
    /// The trigger a result is news of: its next unread result replaces this line, so one
    /// trigger keeps one row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    #[serde(default)]
    lines: Vec<Line>,
    #[serde(default)]
    seen_asks: Vec<String>,
}

fn load(store: &Store) -> Kept {
    store.setting(KEY).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn keep(store: &Store, mut kept: Kept) {
    let cutoff = now_ms() - KEEP_DAYS * 24 * 3600 * 1000;
    kept.lines.retain(|l| l.at >= cutoff);
    let extra = kept.lines.len().saturating_sub(LINES_KEPT);
    kept.lines.drain(..extra);
    let extra = kept.seen_asks.len().saturating_sub(SEEN_KEPT);
    kept.seen_asks.drain(..extra);
    let _ = store.set_setting(KEY, &json!(kept));
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= TEXT_MAX { text.to_string() } else { format!("{}…", text.chars().take(TEXT_MAX).collect::<String>()) }
}

/// Changes the kept lines under the lock. True if `f` says it changed something.
fn change(app: &App, f: impl FnOnce(&Store, &mut Kept) -> bool) -> bool {
    let _one = CHANGING.lock().expect("inbox lock");
    app.with_store(|store| {
        let mut kept = load(store);
        let changed = f(store, &mut kept);
        if changed {
            keep(store, kept);
        }
        Ok(changed)
    })
    .unwrap_or(false)
}

/// Tells every open interface the inbox changed.
pub fn changed(app: &App) {
    app.hub.send(json!({ "kind": "inbox" }));
}

/// A row of the inbox, as the interface lists it.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: String,
    kind: &'static str,
    tab: &'static str,
    at: i64,
    unread: bool,
    wire: Value,
}

/// Where a chat is opened from: the channel it belongs to, else its bot.
fn place(conversation: &Conversation) -> String {
    conversation.channel_id.clone().unwrap_or_else(|| conversation.bot_id.clone())
}

fn origin(store: &Store, conversation: &Conversation, message: Option<i64>) -> Value {
    let title = match &conversation.channel_id {
        Some(channel) => store.channel(channel).ok().flatten().map(|c| format!("#{}", c.name)).unwrap_or_default(),
        None => conversation.title.clone().unwrap_or_else(|| crate::titles::UNTITLED.to_string()),
    };
    let mut origin = json!({ "botId": place(conversation), "threadId": conversation.id, "title": title });
    if let Some(id) = message {
        origin["messageId"] = json!(wire::message_id(id));
    }
    origin
}

fn from(store: &Store, bot_id: &str, name: Option<&str>) -> Value {
    let bot = store.bot_by_id(bot_id).ok().flatten();
    let name = name.map(str::to_string).or_else(|| bot.as_ref().map(|b| b.name.clone())).unwrap_or_else(|| "A bot".into());
    let color = bot.as_ref().map(wire::color_of).unwrap_or_else(|| "slate".into());
    json!({ "name": name, "botId": bot_id, "color": color })
}

/// The asks open now: approvals running turns wait on, and cards a bot left asking.
fn asks(app: &App, store: &Store, seen: &[String]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (thread, bot_id, request, message_id) in app.turns.all_open_cards() {
        let (Ok(Some(conversation)), Ok(Some(message))) = (store.conversation(&thread), store.message(message_id)) else { continue };
        let card = message.detail.as_ref().map(|d| d["card"].clone()).unwrap_or(Value::Null);
        let id = wire::message_id(message_id);
        rows.push(Row {
            unread: !seen.contains(&id),
            at: wire::epoch_ms(&message.at),
            kind: "ask",
            tab: "needs-you",
            wire: json!({
                "id": id,
                "from": from(store, &bot_id, None),
                "origin": origin(store, &conversation, Some(message_id)),
                "text": clip(card["subtitle"].as_str().unwrap_or(&message.body)),
                // A question in words is answered in its chat (stage 32.5); an approval, here too.
                "card": if card["requestType"] == "question" {
                    json!({ "type": "question", "requestId": request })
                } else {
                    json!({ "type": "approval", "requestId": request, "tool": card["tool"], "always": card["always"] })
                },
            }),
            id,
        });
    }
    for (thread, message) in store.asking_cards(ROWS_SHOWN).unwrap_or_default() {
        let Ok(Some(conversation)) = store.conversation(&thread) else { continue };
        let Some(detail) = &message.detail else { continue };
        let card = match detail["kind"].as_str() {
            // A review (stage 32.14): "Review Kiwi's answer", the answer's start under it.
            Some("team-change") if detail["state"] == "asking" && detail["action"] == "review" => json!({
                "type": "change",
                "title": detail["heading"],
                "lines": [detail["title"]],
                "warning": null,
            }),
            Some("team-change") if detail["state"] == "asking" => json!({
                "type": "change",
                "title": detail["title"],
                "lines": detail["lines"],
                "warning": detail["warning"],
            }),
            Some("image") if detail["state"] == "choosing" => json!({ "type": "picture", "prompt": detail["prompt"] }),
            _ => continue,
        };
        let asked_by = detail["askedBy"].as_str().filter(|n| *n != "you");
        let bot = asked_by.and_then(|n| store.bot(n).ok().flatten()).map(|b| b.id).unwrap_or_else(|| conversation.bot_id.clone());
        let text = match card["type"].as_str() {
            Some("picture") => format!("Choose who draws: {}", detail["prompt"].as_str().unwrap_or_default()),
            _ => card["title"].as_str().unwrap_or(&message.body).to_string(),
        };
        let id = wire::message_id(message.id);
        rows.push(Row {
            unread: !seen.contains(&id),
            at: wire::epoch_ms(&message.at),
            kind: "ask",
            tab: "needs-you",
            wire: json!({
                "id": id,
                "from": from(store, &bot, asked_by),
                "origin": origin(store, &conversation, Some(message.id)),
                "text": clip(&text),
                "card": card,
            }),
            id,
        });
    }
    rows
}

fn line_tab(line: &Line) -> &'static str {
    if line.done || (line.kind == "result" && line.seen) { "done" } else { "info" }
}

fn line_row(store: &Store, line: &Line) -> Option<Row> {
    let conversation = store.conversation(&line.thread_id).ok().flatten()?;
    let focus = if line.kind == "message" { line.id.strip_prefix("cb-").and_then(|n| n.parse().ok()) } else { None };
    Some(Row {
        id: line.id.clone(),
        kind: match line.kind.as_str() {
            "message" => "message",
            "result" => "result",
            _ => "notice",
        },
        tab: line_tab(line),
        at: line.at,
        unread: !line.seen,
        wire: json!({
            "id": line.id,
            "from": from(store, &line.bot_id, None),
            "origin": origin(store, &conversation, focus),
            "text": line.text,
        }),
    })
}

/// AIs that stopped working (stage 27): one row each, waiting on the person's fix; it goes when
/// the AI works again.
fn troubles(app: &App) -> Vec<Row> {
    super::health::all(app)
        .into_iter()
        .map(|t| {
            let engine = t["engine"].as_str().unwrap_or_default();
            Row {
                id: format!("fix-{engine}"),
                kind: "fix",
                tab: "needs-you",
                at: t["at"].as_i64().unwrap_or(0),
                unread: true,
                wire: json!({
                    "id": format!("fix-{engine}"),
                    "from": { "name": t["name"] },
                    "origin": { "botId": t["botId"], "threadId": t["threadId"] },
                    "text": t["text"],
                    "card": { "type": "fix", "engine": engine, "what": t["what"], "fix": t["fix"], "said": t["said"] },
                }),
            }
        })
        .collect()
}

fn rows(app: &App, store: &Store, kept: &Kept) -> Vec<Row> {
    let mut rows = troubles(app);
    rows.extend(asks(app, store, &kept.seen_asks));
    rows.extend(kept.lines.iter().filter_map(|l| line_row(store, l)));
    rows.sort_by_key(|r| std::cmp::Reverse(r.at));
    // What is still open comes first, so settled rows never push it out of the list.
    let (open, settled): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|r| r.tab != "done");
    open.into_iter().chain(settled).take(ROWS_SHOWN).collect()
}

fn count_of(rows: &[Row]) -> usize {
    rows.iter().filter(|r| r.tab == "needs-you" || (r.tab == "info" && r.unread)).count()
}

/// The inbox as the panel lists it.
pub fn view(app: &App) -> Value {
    let listed = {
        let _one = CHANGING.lock().expect("inbox lock");
        app.with_store(|store| Ok(rows(app, store, &load(store))))
    }
    .unwrap_or_default();
    let count = count_of(&listed);
    let rows: Vec<Value> = listed
        .into_iter()
        .map(|r| {
            let mut wire = r.wire;
            wire["kind"] = json!(r.kind);
            wire["tab"] = json!(r.tab);
            wire["at"] = json!(r.at);
            wire["unread"] = json!(r.unread);
            wire
        })
        .collect();
    json!({ "rows": rows, "count": count })
}

/// Files a line of news and tells every open interface (a message with its row, to notify).
fn add(app: &App, line: Line) {
    let notify = line.kind == "message";
    let added = line.clone();
    change(app, |_, kept| {
        if let Some(job) = &line.job {
            kept.lines.retain(|l| l.job.as_ref() != Some(job) || l.seen || l.done);
        }
        kept.lines.push(line);
        true
    });
    if notify {
        let row = app.with_store(|store| Ok(line_row(store, &added))).flatten();
        if let Some(row) = row {
            let mut wire = row.wire;
            wire["kind"] = json!(row.kind);
            app.hub.send(json!({ "kind": "inbox", "line": wire }));
            return;
        }
    }
    changed(app);
}

/// A trigger's run ended: its reply is news, or its failure is.
pub fn trigger_ran(app: &App, trigger_id: &str, name: &str, bot_id: &str, thread: &str, reply: Result<&str, &str>) {
    let (kind, text) = match reply {
        Ok(text) => ("result", format!("{name}: {}", text.trim())),
        Err(why) => ("notice", format!("{name} failed: {why}")),
    };
    add(app, Line {
        id: format!("in-{}", uuid::Uuid::new_v4().simple()),
        kind: kind.into(),
        bot_id: bot_id.into(),
        thread_id: thread.into(),
        text: clip(&text),
        at: now_ms(),
        job: (kind == "result").then(|| trigger_id.to_string()),
        ..Line::default()
    });
}

/// A bot answered in a chat: it's unread until the person looks.
pub fn answered_in(app: &App, thread: &str) {
    let marked = app.with_store(|store| store.set_unread(thread, true)).unwrap_or(false);
    if marked {
        announce_owner(app, thread);
        changed(app);
    }
}

fn announce_owner(app: &App, thread: &str) {
    if let Some(conversation) = app.with_store(|store| store.conversation(thread)).flatten() {
        app.announce_bot(&place(&conversation));
    }
}

/// The person looked at a chat: it's read, and so is its news.
pub fn read_chat(app: &App, thread: &str) {
    let marked = app.with_store(|store| store.set_unread(thread, false)).unwrap_or(false);
    let lines = change(app, |_, kept| {
        let mut any = false;
        for line in kept.lines.iter_mut().filter(|l| l.thread_id == thread && !l.seen) {
            line.seen = true;
            any = true;
        }
        any
    });
    if marked {
        announce_owner(app, thread);
    }
    if marked || lines {
        changed(app);
    }
}

/// The person wrote in a chat: messages it sent them are answered.
pub fn person_spoke(app: &App, thread: &str) {
    let answered = change(app, |_, kept| {
        let mut any = false;
        for line in kept.lines.iter_mut().filter(|l| l.thread_id == thread && l.kind == "message" && !l.done) {
            line.done = true;
            line.seen = true;
            any = true;
        }
        any
    });
    if answered {
        changed(app);
    }
}

/// The tool every bot has for reaching the person.
pub fn tools() -> Vec<Value> {
    vec![json!({
        "name": "message_person",
        "description": "Send the person a message that lands in their Inbox and notifies them. Use it for something they should see even if they aren't watching this chat: what a trigger run found, a question while you work on your own. It's also kept in this chat. Use it sparingly; your normal reply reaches them here anyway. Their answer comes as their next message in this chat.",
        "inputSchema": {
            "type": "object",
            "properties": { "text": { "type": "string", "description": "The message, written to the person." } },
            "required": ["text"],
        },
    })]
}

/// `message_person`: one message in the chat that sent it, one line in the Inbox.
pub fn message_person(app: &App, grant: &super::bot_tools::Grant, args: &Value) -> Result<String, String> {
    let text = args["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("say something (\"text\")")?;
    {
        let mut sent = SENT.lock().expect("sent lock");
        let recent = sent.get_or_insert_with(HashMap::new).entry(grant.thread.clone()).or_default();
        recent.retain(|at| at.elapsed() < std::time::Duration::from_secs(3600));
        if recent.len() >= MESSAGES_HOURLY {
            return Err(format!("You have sent {MESSAGES_HOURLY} messages from this chat in the last hour, the most allowed. Don't retry: put it in your reply."));
        }
        recent.push(std::time::Instant::now());
    }
    let detail = json!({ "botId": grant.bot_id, "inbox": true });
    let message = super::turns::save_and_send(app, &grant.thread, Role::Bot, text, Some(&detail)).ok_or("couldn't save the message")?;
    let id = message["id"].as_str().unwrap_or_default().to_string();
    add(app, Line {
        id,
        kind: "message".into(),
        bot_id: grant.bot_id.clone(),
        thread_id: grant.thread.clone(),
        text: clip(text),
        at: now_ms(),
        ..Line::default()
    });
    answered_in(app, &grant.thread);
    Ok("It's in their Inbox and they were notified. Their answer comes as their next message in this chat.".into())
}

/// The inbox's routes.
pub fn route(app: &App, method: &Method, rest: &[&str]) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, view(app))),
        (&Method::GET, ["count"]) => Ok((StatusCode::OK, json!({ "count": view(app)["count"] }))),
        (&Method::POST, ["read-all"]) => {
            let listed: Vec<String> = view(app)["rows"].as_array().into_iter().flatten().filter(|r| r["kind"] == "ask" && r["unread"] == true).filter_map(|r| r["id"].as_str().map(str::to_string)).collect();
            change(app, |_, kept| {
                let mut any = false;
                for line in kept.lines.iter_mut().filter(|l| !l.seen) {
                    line.seen = true;
                    any = true;
                }
                for id in listed {
                    kept.seen_asks.push(id);
                    any = true;
                }
                any
            });
            changed(app);
            Ok((StatusCode::OK, view(app)))
        }
        (&Method::POST, [id, what @ ("read" | "done")]) => {
            let id = id.to_string();
            let is_ask = view(app)["rows"].as_array().into_iter().flatten().any(|r| r["kind"] == "ask" && r["id"] == id.as_str());
            if is_ask && *what == "done" {
                return Err((StatusCode::CONFLICT, "this one waits on your answer".into()));
            }
            let mut found = is_ask;
            change(app, |_, kept| {
                if is_ask {
                    if kept.seen_asks.contains(&id) {
                        return false;
                    }
                    kept.seen_asks.push(id.clone());
                    return true;
                }
                let Some(line) = kept.lines.iter_mut().find(|l| l.id == id) else { return false };
                found = true;
                let before = (line.seen, line.done);
                line.seen = true;
                if *what == "done" {
                    line.done = true;
                }
                before != (line.seen, line.done)
            });
            if !found {
                return Err((StatusCode::NOT_FOUND, "no such line in the inbox".into()));
            }
            changed(app);
            Ok((StatusCode::OK, view(app)))
        }
        _ => Err((StatusCode::NOT_FOUND, "the inbox doesn't do that".into())),
    }
}

/// A bot's or a channel's chats, unread or not, as the store has them (for the bot list).
pub fn unread_of(conversations: &[Conversation]) -> bool {
    conversations.iter().any(|c| c.unread && c.archived_at.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_settle_once_read_and_messages_once_done() {
        let mut line = Line { kind: "result".into(), ..Line::default() };
        assert_eq!(line_tab(&line), "info");
        line.seen = true;
        assert_eq!(line_tab(&line), "done");
        let mut message = Line { kind: "message".into(), seen: true, ..Line::default() };
        assert_eq!(line_tab(&message), "info", "a message read but not answered still waits");
        message.done = true;
        assert_eq!(line_tab(&message), "done");
    }

    #[test]
    fn the_count_is_asks_and_unread_news() {
        let row = |tab, unread| Row { id: String::new(), kind: "notice", tab, at: 0, unread, wire: Value::Null };
        assert_eq!(count_of(&[row("needs-you", false), row("info", true), row("info", false), row("done", true)]), 2);
    }

    #[test]
    fn long_text_is_clipped() {
        assert_eq!(clip(&"a".repeat(TEXT_MAX + 5)).chars().count(), TEXT_MAX + 1);
        assert_eq!(clip("  hi "), "hi");
    }
}
