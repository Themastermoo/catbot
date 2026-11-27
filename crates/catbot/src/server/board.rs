//! The task board (stage 30.3): work items given to bots, done in their own chats.
//!
//! Moo's choices (2026-11-26), after OpenMausBot's Team Board:
//! - **Who takes an item:** the bot it's given to, by the person or the Chief of Staff.
//! - **Starting:** by itself, as soon as that bot is free; the person can pause any item.
//! - **Where:** each item gets its own chat with its bot, so the work and what came of it stay
//!   together. The Team Chat hears when it's done, fails, or the bot asks something.
//! - **Order:** an item can wait for others ("after"); it starts once they're done, and is told
//!   what they came to.
//!
//! An item's states: `waiting` (on items before it), `ready`, `working` (its bot's turn is
//! running in its chat), `asking` (the bot asked the person; they answer in the item's chat),
//! `paused`, and the ends `done`, `failed`, `cancelled`. A turn settles its item with
//! `<board-update>failed: why</board-update>` or `<board-update>ask: question</board-update>`;
//! a reply without one is the work done, and is what it came to. A turn that ends with no reply
//! puts the item back to try again, and the third such end fails it. Stopping the turn pauses
//! the item. The Chief adds items with `<board-add>{…}</board-add>`, without a card: it's
//! delegating, which Moo set free (hand-offs), and every item can be paused or cancelled.
//!
//! Kept in the `board` setting. Routes: `GET`/`POST /api/board`, `PATCH`/`DELETE /api/board/:id`.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, turns};
use crate::store::{Bot, Role, Store};

const KEY: &str = "board";
/// Turns that end with no reply before an item fails.
const TRIES: u32 = 3;
const TITLE_MAX: usize = 120;
const TEXT_MAX: usize = 8000;
const RESULT_MAX: usize = 4000;

const UPDATE_OPEN: &str = "<board-update>";
const UPDATE_CLOSE: &str = "</board-update>";
const ADD_OPEN: &str = "<board-add>";
const ADD_CLOSE: &str = "</board-add>";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub text: String,
    /// The bot it's given to.
    pub bot_id: String,
    pub state: String,
    /// Items that have to be done first.
    #[serde(default)]
    pub after: Vec<String>,
    /// Its chat, once it has started.
    #[serde(default)]
    pub thread: Option<String>,
    /// What it came to (done, failed or cancelled), or the bot's question (asking).
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub tries: u32,
    /// "person", or the Chief's name.
    #[serde(default)]
    pub by: String,
    pub created_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
}

impl Item {
    fn open(&self) -> bool {
        !matches!(self.state.as_str(), "done" | "failed" | "cancelled")
    }
}

pub fn list_in(store: &Store) -> Vec<Item> {
    store.setting(KEY).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn list(app: &App) -> Vec<Item> {
    app.with_store(|store| Ok(list_in(store))).unwrap_or_default()
}

/// Changes the board in one go (read, change, keep), and says so to every open interface.
fn change<T>(app: &App, f: impl FnOnce(&mut Vec<Item>, &Store) -> Result<T, String>) -> Result<T, String> {
    let outcome = app.with_store(|store| {
        let mut items = list_in(store);
        let outcome = f(&mut items, store);
        if outcome.is_ok() {
            settle_waiting(&mut items);
            store.set_setting(KEY, &json!(items))?;
        }
        Ok(outcome)
    });
    let outcome = outcome.unwrap_or_else(|| Err("couldn't read the board".into()));
    if outcome.is_ok() {
        app.hub.send(json!({ "kind": "board" }));
    }
    outcome
}

/// Items waiting on others become ready once those are all done; an item whose earlier item
/// failed or was cancelled waits (the person decides).
fn settle_waiting(items: &mut [Item]) {
    let done: Vec<String> = items.iter().filter(|i| i.state == "done").map(|i| i.id.clone()).collect();
    let known: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    for item in items.iter_mut() {
        let blocked = item.after.iter().any(|a| known.contains(a) && !done.contains(a));
        match item.state.as_str() {
            "waiting" if !blocked => item.state = "ready".into(),
            "ready" if blocked => item.state = "waiting".into(),
            _ => {}
        }
    }
}

/// Whether `id` needing `after` closes a loop.
fn loops(items: &[Item], id: &str, after: &[String]) -> bool {
    let mut seen: Vec<String> = Vec::new();
    let mut stack: Vec<String> = after.to_vec();
    while let Some(next) = stack.pop() {
        if next == id {
            return true;
        }
        if seen.contains(&next) {
            continue;
        }
        seen.push(next.clone());
        if let Some(item) = items.iter().find(|i| i.id == next) {
            stack.extend(item.after.iter().cloned());
        }
    }
    false
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() > max { format!("{}…", text.chars().take(max - 1).collect::<String>()) } else { text.to_string() }
}

/// A new item from what was asked: checked against the board and the team.
fn made(items: &[Item], store: &Store, body: &Value, by: &str) -> Result<Item, String> {
    let title = one_line(body["title"].as_str().unwrap_or_default());
    if title.is_empty() {
        return Err("give the item a title".into());
    }
    if title.chars().count() > TITLE_MAX {
        return Err(format!("a title is at most {TITLE_MAX} characters"));
    }
    let text = body["text"].as_str().unwrap_or_default().trim().to_string();
    if text.chars().count() > TEXT_MAX {
        return Err(format!("an item's text is at most {TEXT_MAX} characters"));
    }
    let bot_id = body["botId"].as_str().unwrap_or_default().to_string();
    store.bot_by_id(&bot_id).ok().flatten().ok_or("give it to one of the bots")?;
    let after = ids(&body["after"]);
    if let Some(missing) = after.iter().find(|a| !items.iter().any(|i| &i.id == *a)) {
        return Err(format!("there's no item {missing} to wait for"));
    }
    let id = format!("item-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let state = if body["paused"] == true { "paused" } else { "ready" };
    Ok(Item { id, title, text, bot_id, state: state.into(), after, by: by.into(), created_at: chrono::Utc::now().to_rfc3339(), ..Item::default() })
}

fn ids(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

fn wire(items: &[Item]) -> Value {
    json!({ "items": items })
}

/// `/api/board…`.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or(Value::Null);
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, wire(&list(app)))),
        (&Method::POST, []) => {
            let item = change(app, |items, store| {
                let item = made(items, store, &body, "person")?;
                items.push(item.clone());
                Ok(item)
            })
            .map_err(bad)?;
            tick(app);
            Ok((StatusCode::CREATED, json!({ "item": list(app).into_iter().find(|i| i.id == item.id) })))
        }
        (&Method::PATCH, [id]) => {
            let id = id.to_string();
            let stop = change(app, |items, store| edit(items, store, &id, &body)).map_err(bad)?;
            if let Some(thread) = stop {
                app.turns.interrupt(&thread);
            }
            tick(app);
            Ok((StatusCode::OK, json!({ "item": list(app).into_iter().find(|i| i.id == id) })))
        }
        (&Method::DELETE, [id]) => {
            let id = id.to_string();
            let thread = change(app, |items, _| {
                let at = items.iter().position(|i| i.id == id).ok_or("no such item")?;
                let gone = items.remove(at);
                for item in items.iter_mut() {
                    item.after.retain(|a| *a != id);
                }
                Ok(gone.thread.filter(|_| gone.state == "working"))
            })
            .map_err(bad)?;
            if let Some(thread) = thread {
                app.turns.interrupt(&thread);
            }
            tick(app);
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such board route".into())),
    }
}

/// `PATCH`: `title`, `text`, `botId`, `after`, and `state`: "paused", "ready" (go on, or try a
/// failed one again) or "cancelled". The thread to stop, when pausing or cancelling work.
fn edit(items: &mut [Item], store: &Store, id: &str, body: &Value) -> Result<Option<String>, String> {
    let others: Vec<Item> = items.to_vec();
    let item = items.iter_mut().find(|i| i.id == id).ok_or("no such item")?;
    if let Some(title) = body["title"].as_str() {
        let title = one_line(title);
        if title.is_empty() || title.chars().count() > TITLE_MAX {
            return Err(format!("a title needs 1 to {TITLE_MAX} characters"));
        }
        item.title = title;
    }
    if let Some(text) = body["text"].as_str() {
        item.text = cut(text.trim(), TEXT_MAX);
    }
    if let Some(bot) = body["botId"].as_str() {
        if item.state == "working" && bot != item.bot_id {
            return Err("pause it before giving it to another bot".into());
        }
        store.bot_by_id(bot).ok().flatten().ok_or("give it to one of the bots")?;
        if bot != item.bot_id {
            // Another bot starts afresh, in a chat of its own.
            item.thread = None;
            item.bot_id = bot.to_string();
        }
    }
    if body.get("after").is_some() {
        let after = ids(&body["after"]);
        if after.iter().any(|a| !others.iter().any(|i| &i.id == a)) {
            return Err("it can only wait for items on the board".into());
        }
        if loops(&others, id, &after) {
            return Err("that would have items waiting on each other".into());
        }
        item.after = after;
        if item.state == "waiting" {
            item.state = "ready".into();
        }
    }
    let mut stop = None;
    if let Some(state) = body["state"].as_str() {
        match state {
            "paused" if item.open() => {
                if item.state == "working" {
                    stop = item.thread.clone();
                }
                item.state = "paused".into();
            }
            "ready" if matches!(item.state.as_str(), "paused" | "failed" | "cancelled" | "asking") => {
                item.state = "ready".into();
                item.tries = 0;
                item.finished_at = None;
            }
            "cancelled" if item.open() => {
                if item.state == "working" {
                    stop = item.thread.clone();
                }
                item.state = "cancelled".into();
                item.finished_at = Some(chrono::Utc::now().to_rfc3339());
            }
            other => return Err(format!("an item that's {} can't be {other}", item.state)),
        }
    }
    Ok(stop)
}

/// Starts what can start: each ready item whose bot is free, oldest first, one item per bot.
pub fn tick(app: &Arc<App>) {
    let items = list(app);
    let mut taken: Vec<String> = items.iter().filter(|i| i.state == "working").map(|i| i.bot_id.clone()).collect();
    for item in items.iter().filter(|i| i.state == "ready") {
        if taken.contains(&item.bot_id) || !free(app, &item.bot_id) {
            continue;
        }
        taken.push(item.bot_id.clone());
        if let Err(why) = start(app, item, &items) {
            eprintln!("catbot: board item “{}” couldn't start: {why}", item.title);
        }
    }
}

/// Whether a bot is idle in every chat and channel.
fn free(app: &App, bot_id: &str) -> bool {
    let chats = app.with_store(|store| store.conversations(bot_id)).unwrap_or_default();
    if chats.iter().any(|c| app.turns.busy(&c.id) || app.channel_busy(&c.id)) {
        return false;
    }
    let channels = app.with_store(|store| store.channels()).unwrap_or_default();
    !channels.iter().filter(|c| c.members.iter().any(|m| m == bot_id)).any(|c| app.channel_busy(&c.id))
}

/// What the bot is told when it takes an item: how to settle it, what the items before it came
/// to, then the work itself.
fn brief(item: &Item, items: &[Item]) -> String {
    let mut text = format!(
        "This task board item is yours: “{}”. Do the work, then say what came of it: that reply settles the item. If you \
         can't finish it, end your reply with {UPDATE_OPEN}failed: why{UPDATE_CLOSE}. To ask the person something first, end it \
         with {UPDATE_OPEN}ask: your question{UPDATE_CLOSE}; they answer here.",
        item.title
    );
    let before: Vec<String> = item
        .after
        .iter()
        .filter_map(|a| items.iter().find(|i| &i.id == a))
        .map(|i| format!("- “{}”: {}", i.title, i.result.as_deref().unwrap_or("done")))
        .collect();
    if !before.is_empty() {
        text.push_str(&format!("\n\nWhat the items before it came to:\n{}", before.join("\n")));
    }
    if !item.text.is_empty() {
        text.push_str(&format!("\n\nThe work:\n{}", item.text));
    }
    text
}

/// Opens the item's chat (or goes back to it) and starts its bot's turn there.
fn start(app: &Arc<App>, item: &Item, items: &[Item]) -> Result<(), String> {
    let bot = app.with_store(|store| store.bot_by_id(&item.bot_id)).flatten().ok_or("its bot is gone")?;
    let runner = match super::backups::choose(app, &bot) {
        Ok(Some((stand_in, _))) => stand_in,
        Ok(None) => bot.clone(),
        Err(why) => return Err(why),
    };
    let conversation = match item.thread.as_deref().and_then(|t| app.with_store(|store| store.conversation(t)).flatten()) {
        Some(conversation) => conversation,
        None => {
            let made = app.with_store(|store| store.start_conversation(&bot.id, None)).ok_or("couldn't make its chat")?;
            app.with_store(|store| store.set_title(&made.id, &item.title, true));
            made
        }
    };
    let text = brief(item, items);
    let message = turns::save_and_send(app, &conversation.id, Role::Person, &text, Some(&json!({ "fromCatbot": "board", "boardItem": item.id })))
        .ok_or("couldn't write to its chat")?;
    let request_id = message["id"].as_str().and_then(super::wire::parse_message_id).ok_or("couldn't write to its chat")?;
    let (id, thread) = (item.id.clone(), conversation.id.clone());
    change(app, |items, _| {
        let item = items.iter_mut().find(|i| i.id == id).ok_or("the item is gone")?;
        item.state = "working".into();
        item.thread = Some(thread.clone());
        Ok(())
    })?;
    let mut runner = runner;
    runner.instructions.push_str(&super::handoffs::roster_note(app, &runner));
    turns::start(app.clone(), runner, conversation, request_id, text).map_err(str::to_string)?;
    app.announce_bot(&bot.id);
    Ok(())
}

/// The item whose chat this is, if any (and it's still open).
fn item_of(app: &App, thread: &str) -> Option<Item> {
    list(app).into_iter().find(|i| i.thread.as_deref() == Some(thread) && i.open())
}

/// How a turn settled its item: done (with what it said), failed or asking. None leaves it to
/// the reply.
#[derive(Debug, PartialEq)]
pub enum Settled {
    Failed(String),
    Ask(String),
}

/// Takes the `<board-update>…</board-update>` that ends a reply out of it. Only one at the very
/// end counts, as the bot is told ("end with"): one quoted mid-reply is just words.
pub fn take(reply: &str) -> (String, Option<Settled>) {
    let trimmed = reply.trim_end();
    let Some(start) = trimmed.strip_suffix(UPDATE_CLOSE).and_then(|inner| inner.rfind(UPDATE_OPEN)) else {
        return (reply.trim().to_string(), None);
    };
    let raw = &trimmed[start + UPDATE_OPEN.len()..trimmed.len() - UPDATE_CLOSE.len()];
    let (kind, words) = raw.split_once(':').map(|(k, w)| (k.trim().to_lowercase(), w.trim().to_string())).unwrap_or((raw.trim().to_lowercase(), String::new()));
    let settled = match kind.as_str() {
        "failed" | "fail" => Settled::Failed(words),
        "ask" | "question" => Settled::Ask(words),
        _ => return (reply.trim().to_string(), None),
    };
    (trimmed[..start].trim().to_string(), Some(settled))
}

/// Takes every `open…close` out of a text: the rest, and what was inside each.
fn take_tags(text: &str, open: &str, close: &str) -> (String, Vec<String>) {
    let mut rest = String::new();
    let mut found = Vec::new();
    let mut text = text;
    while let Some(start) = text.find(open) {
        rest.push_str(&text[..start]);
        let after = &text[start + open.len()..];
        match after.find(close) {
            Some(end) => {
                found.push(after[..end].trim().to_string());
                text = &after[end + close.len()..];
            }
            None => text = "",
        }
    }
    rest.push_str(text);
    (rest.trim().to_string(), found)
}

/// Whether a chat is an open item's.
pub fn is_item_thread(app: &App, thread: &str) -> bool {
    item_of(app, thread).is_some()
}

/// A turn in an item's chat ended (`how`: done, failed or stopped): the item is settled, tried
/// again, or paused; the Team Chat hears how it went.
pub fn turn_ended(app: &App, bot: &Bot, thread: &str, how: &str, reply: &str, settled: Option<Settled>) {
    let Some(item) = item_of(app, thread) else { return };
    if !matches!(item.state.as_str(), "working" | "asking") {
        return;
    }
    let now = chrono::Utc::now().to_rfc3339();
    let (state, result, tries) = match (how, settled) {
        ("stopped", _) => ("paused", item.result.clone(), item.tries),
        (_, Some(Settled::Failed(why))) => ("failed", Some(if why.is_empty() { "It couldn't be done.".into() } else { why }), item.tries),
        (_, Some(Settled::Ask(question))) => ("asking", Some(question), item.tries),
        _ if !reply.trim().is_empty() => ("done", Some(cut(reply.trim(), RESULT_MAX)), item.tries),
        _ if item.tries + 1 >= TRIES => ("failed", Some(format!("Its turn ended without an answer {TRIES} times."), ), item.tries + 1),
        _ => ("ready", item.result.clone(), item.tries + 1),
    };
    let id = item.id.clone();
    let finished = matches!(state, "done" | "failed").then(|| now.clone());
    let kept = change(app, |items, _| {
        let it = items.iter_mut().find(|i| i.id == id).ok_or("the item is gone")?;
        it.state = state.into();
        it.result = result.clone();
        it.tries = tries;
        it.finished_at = finished.clone();
        Ok(())
    });
    if kept.is_err() {
        return;
    }
    let title = &item.title;
    match state {
        "done" => super::team_chat::post(app, bot, "finished", thread, &format!("{} finished the board item “{title}”.", bot.name)),
        "failed" => super::team_chat::post(app, bot, "failed", thread, &format!("{} couldn't finish the board item “{title}”: {}", bot.name, cut(&one_line(result.as_deref().unwrap_or_default()), 100))),
        "asking" => super::team_chat::post(app, bot, "waiting", thread, &format!("{} asks you about the board item “{title}”: {}", bot.name, cut(&one_line(result.as_deref().unwrap_or_default()), 100))),
        _ => {}
    }
}

/// What the Chief is told about the board.
pub fn chief_note(app: &App) -> String {
    let items = list(app);
    let bots = app.with_store(|store| store.bots()).unwrap_or_default();
    let name = |id: &str| bots.iter().find(|b| b.id == id).map(|b| b.name.clone()).unwrap_or_else(|| "a bot that's gone".into());
    let open: Vec<String> = items.iter().filter(|i| i.open()).map(|i| format!("- “{}” for {} ({})", i.title, name(&i.bot_id), i.state)).collect();
    format!(
        "\n\nThe task board holds work given to bots; each item runs in its own chat with its bot, when that bot is free, and \
         its answer is what it came to. To put work on it, write {ADD_OPEN}{{\"title\":\"…\",\"text\":\"what to do and what done \
         looks like\",\"bot\":\"its name\",\"after\":[\"titles of items to finish first\"]}}{ADD_CLOSE} in your reply, one per tag; \
         it goes on at once (the person can pause or cancel it). Open items now:\n{}",
        if open.is_empty() { "- none".to_string() } else { open.join("\n") }
    )
}

/// Takes `<board-add>…</board-add>` out of the Chief's reply and puts each on the board.
pub fn take_added(reply: &str) -> (String, Vec<String>) {
    take_tags(reply, ADD_OPEN, ADD_CLOSE)
}

/// Puts what the Chief asked for on the board, saying so in its chat.
pub fn add_from_chief(app: &Arc<App>, thread: &str, chief: &Bot, found: Vec<String>) {
    for raw in found {
        let outcome = serde_json::from_str::<Value>(&raw).map_err(|_| "it isn't JSON".to_string()).and_then(|asked| {
            change(app, |items, store| {
                let bots = store.bots().map_err(|e| e.to_string())?;
                let named = asked["bot"].as_str().unwrap_or_default();
                let bot = bots.iter().find(|b| b.name.eq_ignore_ascii_case(named.trim())).ok_or_else(|| format!("there's no bot called {named}"))?;
                let mut after = Vec::new();
                for title in asked["after"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    let earlier = items.iter().rev().find(|i| i.title.eq_ignore_ascii_case(title.trim())).ok_or_else(|| format!("there's no item “{title}” to wait for"))?;
                    after.push(earlier.id.clone());
                }
                let body = json!({ "title": asked["title"], "text": asked["text"], "botId": bot.id, "after": after });
                let item = made(items, store, &body, &chief.name)?;
                items.push(item.clone());
                Ok((item.title, bot.name.clone()))
            })
        });
        let note = match outcome {
            Ok((title, bot)) => format!("{} put “{title}” on the task board for {bot}.", chief.name),
            Err(why) => format!("{} asked for a task board item Catbot couldn't use: {why}.", chief.name),
        };
        turns::save_and_send(app, thread, Role::Note, &note, None);
    }
    tick(app);
}

/// Looks again every so often: a bot that was busy may be free now. Started with the server.
pub async fn keep_going(app: Arc<App>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        tick(&app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, state: &str, after: &[&str]) -> Item {
        Item { id: id.into(), title: id.into(), state: state.into(), after: after.iter().map(|a| a.to_string()).collect(), ..Item::default() }
    }

    #[test]
    fn items_wait_for_the_ones_before_them() {
        let mut items = vec![item("a", "working", &[]), item("b", "ready", &["a"]), item("c", "waiting", &["b"])];
        settle_waiting(&mut items);
        assert_eq!(items.iter().map(|i| i.state.as_str()).collect::<Vec<_>>(), ["working", "waiting", "waiting"]);
        items[0].state = "done".into();
        settle_waiting(&mut items);
        assert_eq!(items[1].state, "ready");
        assert!(loops(&items, "a", &["c".into()]), "a after c after b after a");
        assert!(!loops(&items, "c", &["a".into()]));
    }

    #[test]
    fn a_reply_settles_its_item() {
        assert_eq!(take("Couldn't.\n<board-update>failed: no key</board-update>\n"), ("Couldn't.".to_string(), Some(Settled::Failed("no key".into()))));
        assert_eq!(take("Hmm.\n<board-update>ask: which beds?</board-update>").1, Some(Settled::Ask("which beds?".into())));
        assert_eq!(take("All done."), ("All done.".to_string(), None));
        // Quoted mid-reply, it's just words.
        assert_eq!(take("I could end with <board-update>ask: x</board-update>, but it's done.").1, None);
        let (rest, added) = take_added("On it.\n<board-add>{\"title\":\"Seeds\"}</board-add>");
        assert_eq!((rest.as_str(), added), ("On it.", vec!["{\"title\":\"Seeds\"}".to_string()]));
    }
}
