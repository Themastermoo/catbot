//! Scheduled messages (stage 21): a message written now that goes out at a set time.
//!
//! Moo's choices (2026-11-23): the time is picked with a clock beside Send; messages waiting
//! to go out are in a Scheduled list, not in the chat; one whose time passed while Catbot (or
//! the computer) was off goes out as soon as Catbot is running again, marked late; in chats with
//! a bot and in channels (not the desktop pet's little chat).
//!
//! The server sends them, so they go out with the app's window closed. Kept in the settings
//! under `scheduled`: the ones waiting, and the last ones sent (a week, 50 at most).
//!
//! * `GET    /api/scheduled?botId=&threadId=`  the waiting ones, soonest first (all, unasked)
//! * `POST   /api/scheduled`                   `{ botId, threadId?, text, at }` (`at` in ms)
//! * `PATCH  /api/scheduled/:id`               `{ text?, at? }` while it waits
//! * `DELETE /api/scheduled/:id`               cancel it
//! * `POST   /api/scheduled/:id/send`          send it now
//!
//! A bot still busy when one is due gets it as soon as it's free (not marked late for that).
//! Each change sends a `scheduled` frame.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};

const KEY: &str = "scheduled";
/// How often due messages are looked for.
const TICK: Duration = Duration::from_secs(15);
/// How late a message may go out before it says so.
const GRACE_MS: i64 = 2 * 60 * 1000;
/// How far ahead a message can be scheduled.
const AHEAD_MS: i64 = 366 * 24 * 3600 * 1000;
const WAITING_MAX: usize = 500;
const DONE_KEPT: usize = 50;
const TEXT_MAX: usize = 100_000;

static CHANGING: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scheduled {
    pub id: String,
    /// The bot, or the channel, it goes to.
    pub bot_id: String,
    pub thread_id: String,
    pub text: String,
    /// When it's due, ms since the epoch.
    pub at: i64,
    pub created_at: i64,
    /// "waiting", "sent" or "failed".
    pub state: String,
    pub sent_at: Option<i64>,
    pub late: bool,
    pub error: Option<String>,
    /// It was due while its bot was busy: going out after is not late.
    pub held: bool,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn load(app: &App) -> Vec<Scheduled> {
    app.with_store(|store| store.setting(KEY)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn keep(app: &App, mut list: Vec<Scheduled>) -> Result<(), (StatusCode, String)> {
    let week_ago = now_ms() - 7 * 24 * 3600 * 1000;
    let (mut waiting, mut done): (Vec<Scheduled>, Vec<Scheduled>) = list.drain(..).partition(|s| s.state == "waiting");
    done.retain(|s| s.sent_at.unwrap_or(s.at) >= week_ago);
    done.sort_by_key(|s| std::cmp::Reverse(s.sent_at.unwrap_or(s.at)));
    done.truncate(DONE_KEPT);
    waiting.sort_by_key(|s| s.at);
    waiting.extend(done);
    app.try_store(|store| store.set_setting(KEY, &json!(waiting))).map_err(fail)
}

fn changed(app: &App) {
    app.hub.send(json!({ "kind": "scheduled" }));
}

/// The chat a message goes to: the given one if it's the bot's, else its current one; a
/// channel's own transcript. `None` if there's no such bot or channel.
fn place_thread(app: &App, bot_id: &str, thread: Option<&str>) -> Option<String> {
    app.with_store(|store| {
        if let Some(channel) = store.channel(bot_id)? {
            return Ok(store.channel_conversation(&channel.id)?.map(|c| c.id));
        }
        let Some(bot) = store.bot_by_id(bot_id)? else { return Ok(None) };
        if let Some(id) = thread
            && let Some(c) = store.conversation(id)?
            && c.bot_id == bot.id
            && c.channel_id.is_none()
        {
            return Ok(Some(c.id));
        }
        Ok(Some(store.ensure_conversation(&bot.id)?.id))
    })
    .flatten()
}

fn check_at(at: i64) -> Result<i64, (StatusCode, String)> {
    let now = now_ms();
    if at <= now {
        return Err(bad("choose a time that hasn't passed"));
    }
    if at > now + AHEAD_MS {
        return Err(bad("choose a time within a year"));
    }
    Ok(at)
}

fn check_text(text: &str) -> Result<String, (StatusCode, String)> {
    let text = text.trim();
    if text.is_empty() {
        return Err(bad("write the message first"));
    }
    if text.len() > TEXT_MAX {
        return Err(bad("the message is too long to schedule"));
    }
    Ok(text.to_string())
}

/// Sends one: through the same door as a message typed now. Err((true, _)) when its bot is busy.
fn deliver(app: &Arc<App>, item: &Scheduled, late: bool) -> Result<(), (bool, String)> {
    let mut body = json!({ "text": item.text, "threadId": item.thread_id, "scheduled": { "for": item.at, "late": late } });
    body["sendId"] = json!(format!("scheduled-{}", item.id));
    let sent = match app.with_store(|store| store.channel(&item.bot_id)).flatten() {
        Some(channel) => super::channels::send(app, channel, body),
        None => match app.with_store(|store| store.bot_by_id(&item.bot_id)).flatten() {
            Some(bot) => super::send_message(app, bot, body),
            None => return Err((false, "its bot is gone".into())),
        },
    };
    match sent {
        Ok(_) => Ok(()),
        Err((StatusCode::CONFLICT, _)) => Err((true, "busy".into())),
        Err((_, why)) => Err((false, why)),
    }
}

/// Sends what's due (or the one asked for now). True if anything changed.
fn send_due(app: &Arc<App>, only: Option<&str>) -> Result<bool, (StatusCode, String)> {
    let _one = CHANGING.lock().expect("scheduled lock");
    let mut list = load(app);
    let now = now_ms();
    let started = app.started_ms;
    let mut any = false;
    for item in list.iter_mut().filter(|s| s.state == "waiting") {
        let asked = only == Some(item.id.as_str());
        if !(asked || (only.is_none() && item.at <= now)) {
            continue;
        }
        // Late: due before this Catbot started, or long past by now (the computer slept), and not
        // because its bot was busy.
        let late = !asked && !item.held && (item.at < started || now - item.at > GRACE_MS);
        match deliver(app, item, late) {
            Ok(()) => {
                item.state = "sent".into();
                item.sent_at = Some(now);
                item.late = late;
                any = true;
            }
            Err((true, _)) if !asked => {
                if !item.held {
                    item.held = true;
                    any = true;
                }
            }
            Err((true, _)) => return Err((StatusCode::CONFLICT, "the bot is still working; it goes out when the bot is free".into())),
            Err((false, why)) => {
                item.state = "failed".into();
                item.sent_at = Some(now);
                item.error = Some(why);
                any = true;
            }
        }
    }
    if any {
        keep(app, list)?;
        changed(app);
    }
    Ok(any)
}

/// Sends messages as they come due: at once (for those missed while Catbot was off), then every
/// few seconds.
pub fn start(app: Arc<App>) {
    tokio::spawn(async move {
        loop {
            tick(&app);
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Sends whatever is due now (what the ticker does every few seconds; tests call it).
pub fn tick(app: &Arc<App>) {
    if let Err((_, why)) = send_due(app, None) {
        eprintln!("catbot: scheduled messages: {why}");
    }
}

fn wire(item: &Scheduled) -> Value {
    json!(item)
}

pub fn route(app: &Arc<App>, method: &Method, rest: &[&str], bot: Option<String>, thread: Option<String>, body: Value) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => {
            let list: Vec<Value> = load(app)
                .iter()
                .filter(|s| bot.as_ref().is_none_or(|b| *b == s.bot_id) && thread.as_ref().is_none_or(|t| *t == s.thread_id))
                .map(wire)
                .collect();
            Ok((StatusCode::OK, json!({ "messages": list })))
        }
        (&Method::POST, []) => {
            let bot = body["botId"].as_str().ok_or_else(|| bad("which bot (botId)?"))?.to_string();
            let text = check_text(body["text"].as_str().unwrap_or_default())?;
            let at = check_at(body["at"].as_i64().ok_or_else(|| bad("when (at, in ms)?"))?)?;
            let thread = place_thread(app, &bot, body["threadId"].as_str()).ok_or((StatusCode::NOT_FOUND, "no such bot or channel".to_string()))?;
            let item = Scheduled {
                id: format!("sch-{}", uuid::Uuid::new_v4().simple()),
                bot_id: bot,
                thread_id: thread,
                text,
                at,
                created_at: now_ms(),
                state: "waiting".into(),
                ..Scheduled::default()
            };
            {
                let _one = CHANGING.lock().expect("scheduled lock");
                let mut list = load(app);
                if list.iter().filter(|s| s.state == "waiting").count() >= WAITING_MAX {
                    return Err(bad(format!("{WAITING_MAX} messages are already waiting")));
                }
                list.push(item.clone());
                keep(app, list)?;
            }
            changed(app);
            Ok((StatusCode::CREATED, json!({ "message": wire(&item) })))
        }
        (&Method::PATCH, [id]) => {
            let text = body.get("text").map(|t| check_text(t.as_str().unwrap_or_default())).transpose()?;
            let at = body.get("at").map(|a| a.as_i64().ok_or_else(|| bad("at is ms")).and_then(check_at)).transpose()?;
            let item = {
                let _one = CHANGING.lock().expect("scheduled lock");
                let mut list = load(app);
                let item = list.iter_mut().find(|s| s.id == *id).ok_or((StatusCode::NOT_FOUND, "no such scheduled message".to_string()))?;
                if item.state != "waiting" {
                    return Err((StatusCode::CONFLICT, "it has already gone out".into()));
                }
                if let Some(text) = text {
                    item.text = text;
                }
                if let Some(at) = at {
                    item.at = at;
                    item.held = false;
                }
                let item = item.clone();
                keep(app, list)?;
                item
            };
            changed(app);
            Ok((StatusCode::OK, json!({ "message": wire(&item) })))
        }
        (&Method::DELETE, [id]) => {
            {
                let _one = CHANGING.lock().expect("scheduled lock");
                let mut list = load(app);
                let before = list.len();
                list.retain(|s| !(s.id == *id && s.state == "waiting"));
                if list.len() == before {
                    return Err((StatusCode::NOT_FOUND, "nothing waiting by that id".into()));
                }
                keep(app, list)?;
            }
            changed(app);
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        (&Method::POST, [id, "send"]) => {
            // A failed one can be tried again.
            {
                let _one = CHANGING.lock().expect("scheduled lock");
                let mut list = load(app);
                let item = list.iter_mut().find(|s| s.id == *id && s.state != "sent").ok_or((StatusCode::NOT_FOUND, "nothing waiting by that id".to_string()))?;
                if item.state == "failed" {
                    item.state = "waiting".into();
                    item.error = None;
                    item.sent_at = None;
                    keep(app, list)?;
                }
            }
            send_due(app, Some(id))?;
            let sent = load(app).into_iter().find(|s| s.id == *id).map(|s| wire(&s)).unwrap_or(Value::Null);
            if sent["state"] == "failed" {
                return Err((StatusCode::BAD_GATEWAY, sent["error"].as_str().unwrap_or("it couldn't be sent").to_string()));
            }
            Ok((StatusCode::OK, json!({ "message": sent })))
        }
        _ => Err((StatusCode::NOT_FOUND, "scheduled messages don't do that".into())),
    }
}

/// What a bot is told about a message that went out late.
pub fn late_note(body: &Value) -> String {
    if body["scheduled"]["late"] != true {
        return String::new();
    }
    let when = body["scheduled"]["for"].as_i64().and_then(chrono::DateTime::from_timestamp_millis).map(|t| t.with_timezone(&chrono::Local).format("%a %-d %b at %-I:%M %p").to_string()).unwrap_or_default();
    format!("\n\n(The person scheduled this message for {when}; it went out late because Catbot wasn't running then.)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_words_are_checked() {
        let now = now_ms();
        assert!(check_at(now - 1000).is_err());
        assert!(check_at(now + 60_000).is_ok());
        assert!(check_at(now + AHEAD_MS + 60_000).is_err());
        assert_eq!(check_text("  hi  ").unwrap(), "hi");
        assert!(check_text("   ").is_err());
    }

    #[test]
    fn late_ones_say_so_to_the_bot() {
        assert_eq!(late_note(&json!({ "scheduled": { "for": 1, "late": false } })), "");
        assert!(late_note(&json!({ "scheduled": { "for": 1_795_000_000_000i64, "late": true } })).contains("went out late"));
    }
}
