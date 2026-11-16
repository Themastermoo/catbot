//! The Chief of Staff (stage 15.3): one bot, the person's main point of contact for the team.
//!
//! Moo's choices (2026-11-16), after Grok Bot and MausBot: Catbot has a ready-made Chief it can
//! make, or any working bot can be made the Chief (one at a time, starred in the sidebar). The
//! Chief delegates by handing off (`handoffs.rs`), and runs the team, asking first. Its options
//! have their own page in Settings.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::claude::PermissionMode;
use crate::store::{Bot, NewBot, Store};

/// The Chief's options, kept in the settings as `chief`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    /// Which bot is the Chief, if any.
    pub bot_id: Option<String>,
    /// Catbot watches the team and wakes the Chief when something goes wrong (Moo: on by default).
    pub watch: bool,
    /// The Chief also looks the team over on a timer (Moo: an option, off by default).
    pub checkins: Checkins,
    /// The daily brief (Moo: weekdays by default, changeable).
    pub brief: Brief,
}

impl Default for Options {
    fn default() -> Self {
        Options { bot_id: None, watch: true, checkins: Checkins::default(), brief: Brief::default() }
    }
}

/// The daily brief: the Chief writes it from facts Catbot gathers, at a time, on some days.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Brief {
    pub on: bool,
    /// "HH:MM", this computer's time.
    pub time: String,
    /// 1 Monday … 7 Sunday.
    pub days: Vec<u32>,
}

impl Default for Brief {
    fn default() -> Self {
        Brief { on: true, time: "08:00".into(), days: vec![1, 2, 3, 4, 5] }
    }
}

/// Timed check-ins: every so many hours, between two times, on some days.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Checkins {
    pub on: bool,
    pub every_hours: u32,
    /// "HH:MM", this computer's time.
    pub from: String,
    pub to: String,
    /// 1 Monday … 7 Sunday.
    pub days: Vec<u32>,
}

impl Default for Checkins {
    fn default() -> Self {
        Checkins { on: false, every_hours: 3, from: "09:00".into(), to: "17:00".into(), days: vec![1, 2, 3, 4, 5] }
    }
}

/// A time of day, "HH:MM", in minutes after midnight.
pub fn minutes(clock: &str) -> Option<u32> {
    let (h, m) = clock.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn options_in(store: &Store) -> Options {
    store.setting("chief").ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn options(app: &App) -> Options {
    app.with_store(|store| Ok(options_in(store))).unwrap_or_default()
}

/// The Chief, if one is set and still there.
pub fn chief_in(store: &Store) -> Option<Bot> {
    let id = options_in(store).bot_id?;
    store.bot_by_id(&id).ok().flatten()
}

pub fn chief(app: &App) -> Option<Bot> {
    app.with_store(|store| Ok(chief_in(store))).flatten()
}

pub fn is_chief_in(store: &Store, bot_id: &str) -> bool {
    options_in(store).bot_id.as_deref() == Some(bot_id)
}

/// The ready-made Chief's name and instructions.
pub const READY_NAME: &str = "Chief";
pub const READY_INSTRUCTIONS: &str = "You're the Chief of Staff: the person's main point of contact for their whole team of bots. \
Take any request. Do what you can yourself, hand parts to the teammate best placed for them, check their work before calling it \
done, and give the person one clear answer. Keep track of what's still open, and follow up on anything that stalls. Be brief.";

/// `GET /api/chief`; at home also `PUT /api/chief` (`{ botId }`, null for none) and
/// `POST /api/chief/ready-made` (make the ready-made Chief and make it the Chief).
pub fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, wire(app))),
        (&Method::PUT, []) => {
            let body = body.unwrap_or_default();
            let mut next = options(app);
            match body.get("botId") {
                None => {}
                Some(Value::Null) => next.bot_id = None,
                Some(Value::String(id)) => {
                    let bot = app.try_store(|store| store.bot_by_id(id)).map_err(fail)?.ok_or_else(|| bad("no such bot"))?;
                    next.bot_id = Some(bot.id);
                }
                Some(_) => return Err(bad("botId: a bot's id, or null")),
            }
            if let Some(watch) = body["watch"].as_bool() {
                next.watch = watch;
            }
            if let Some(checkins) = body.get("checkins") {
                let mut merged = serde_json::to_value(&next.checkins).unwrap_or_default();
                for (key, value) in checkins.as_object().into_iter().flatten() {
                    merged[key] = value.clone();
                }
                let mut checkins: Checkins = serde_json::from_value(merged).map_err(|e| bad(format!("check-ins: {e}")))?;
                if minutes(&checkins.from).is_none() || minutes(&checkins.to).is_none() {
                    return Err(bad("check-in times are HH:MM"));
                }
                checkins.every_hours = checkins.every_hours.clamp(1, 24);
                checkins.days.retain(|d| (1..=7).contains(d));
                next.checkins = checkins;
            }
            if let Some(brief) = body.get("brief") {
                let mut merged = serde_json::to_value(&next.brief).unwrap_or_default();
                for (key, value) in brief.as_object().into_iter().flatten() {
                    merged[key] = value.clone();
                }
                let mut brief: Brief = serde_json::from_value(merged).map_err(|e| bad(format!("brief: {e}")))?;
                if minutes(&brief.time).is_none() {
                    return Err(bad("the brief's time is HH:MM"));
                }
                brief.days.retain(|d| (1..=7).contains(d));
                next.brief = brief;
            }
            set(app, next);
            Ok((StatusCode::OK, wire(app)))
        }
        // Looks over the team now, as the watch does every half hour (for tests, and the impatient).
        (&Method::POST, ["look"]) => {
            let woke = look(app, true);
            Ok((StatusCode::OK, json!({ "woke": woke })))
        }
        (&Method::POST, ["check-in"]) => {
            let woke = check_in(app);
            Ok((StatusCode::OK, json!({ "woke": woke })))
        }
        (&Method::POST, ["brief"]) => {
            let woke = brief(app);
            Ok((StatusCode::OK, json!({ "woke": woke })))
        }
        (&Method::POST, ["ready-made"]) => {
            let bot = app
                .try_store(|store| {
                    let name = store.free_name(READY_NAME)?;
                    let mut bot = store.create_bot(NewBot { name, instructions: READY_INSTRUCTIONS.into(), model: None, mode: PermissionMode::Default, codex: true })?;
                    bot.color = Some("blue".into());
                    store.update_bot(&bot)?;
                    Ok(bot)
                })
                .map_err(bad)?;
            app.seed_memory(&bot);
            eprintln!("catbot: made {}, the ready-made Chief of Staff ({})", bot.name, bot.folder.display());
            let mut next = options(app);
            next.bot_id = Some(bot.id.clone());
            set(app, next);
            Ok((StatusCode::CREATED, wire(app)))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such Chief of Staff route".into())),
    }
}

/// Makes a bot the Chief.
pub fn make_chief(app: &App, bot_id: &str) {
    let mut next = options(app);
    next.bot_id = Some(bot_id.to_string());
    set(app, next);
}

/// Saves the options, and shows the change: an old and new Chief lose and gain their star.
fn set(app: &App, next: Options) {
    let before = options(app);
    if app.try_store(|store| store.set_setting("chief", &json!(next))).is_err() {
        return;
    }
    let moved = before.bot_id != next.bot_id;
    for id in [before.bot_id, next.bot_id.clone()].into_iter().flatten().filter(|_| moved) {
        app.announce_bot(&id);
    }
    app.hub.send(json!({ "kind": "chief", "chief": next }));
}

fn wire(app: &App) -> Value {
    let options = options(app);
    let chief = chief(app);
    json!({ "chief": options, "name": chief.map(|b| b.name), "readyMadeName": READY_NAME })
}

/// What the Chief is told on top of its own instructions, every turn: its role, the whole team,
/// and how to delegate.
pub fn note(app: &App, chief: &Bot) -> String {
    let (bots, channels) = app
        .with_store(|store| Ok((store.bots()?, store.channels()?)))
        .unwrap_or_default();
    let team: Vec<String> = bots
        .iter()
        .filter(|b| b.id != chief.id)
        .map(|b| {
            let about: String = b.instructions.split_whitespace().take(24).collect::<Vec<_>>().join(" ");
            let mode = match b.mode {
                PermissionMode::BypassPermissions => "acts without asking",
                PermissionMode::AcceptEdits => "edits files without asking",
                PermissionMode::Auto => "decides what's safe to run",
                PermissionMode::Plan => "plans only",
                _ => "asks before acting",
            };
            let about = if about.is_empty() { "no instructions yet".to_string() } else { about };
            format!("- {} (on {}, {mode}): {about}", b.name, b.engine)
        })
        .collect();
    let names = |ids: &[String]| ids.iter().filter_map(|id| bots.iter().find(|b| &b.id == id)).map(|b| b.name.clone()).collect::<Vec<_>>();
    let rooms: Vec<String> = channels
        .iter()
        .map(|c| {
            let lead = c.lead.as_ref().and_then(|l| bots.iter().find(|b| &b.id == l)).map(|b| b.name.clone()).unwrap_or_default();
            format!("- #{} with {} (lead: {lead})", c.name, names(&c.members).join(", "))
        })
        .collect();
    let mut note = String::from(
        "\n\nYou are the person's Chief of Staff, their one point of contact for the team of bots below. Do the work yourself \
         when it's yours; when a teammate is better placed, hand it to them by ending your reply with @Name and a clear, \
         self-contained brief of what you need. Their answer comes back to you: check it before you call the work done, and \
         never claim a teammate did something before their answer has arrived. Then give the person one consolidated answer.",
    );
    note.push_str("\n\nYour team:\n");
    note.push_str(&if team.is_empty() { "- No other bots yet.".to_string() } else { team.join("\n") });
    if !rooms.is_empty() {
        note.push_str("\n\nChannels (group chats):\n");
        note.push_str(&rooms.join("\n"));
    }
    note.push_str(super::team::CHIEF_NOTE);
    note
}

/// What the watch has looked at, kept in the settings as `chief-watch`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Watched {
    /// The last message looked at.
    seen: i64,
    looked_at: Option<String>,
    checked_in_at: Option<String>,
    /// When the last brief went out, and on which day ("YYYY-MM-DD").
    briefed_at: Option<String>,
    briefed_on: Option<String>,
    /// Approval cards already reported as waiting.
    waiting_told: Vec<i64>,
}

fn watched(app: &App) -> Watched {
    app.with_store(|store| store.setting("chief-watch")).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn keep(app: &App, watched: &Watched) {
    app.with_store(|store| store.set_setting("chief-watch", &json!(watched)));
}

/// How often the watch looks, and how long an approval waits before it's worth mentioning.
const WATCH_EVERY_MIN: i64 = 30;
const WAITING_MIN: i64 = 30;

/// Runs for as long as Catbot does: every minute, the watch (when it's due) and timed check-ins.
pub async fn keep_watch(app: Arc<App>) {
    // Trouble from before Catbot started isn't news.
    let mut w = watched(&app);
    if w.seen == 0 {
        w.seen = app.with_store(|store| store.last_message_id()).unwrap_or(0);
        keep(&app, &w);
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        tick(&app);
    }
}

fn since_minutes(at: &Option<String>) -> i64 {
    at.as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| (chrono::Local::now() - at.with_timezone(&chrono::Local)).num_minutes())
        .unwrap_or(i64::MAX)
}

fn tick(app: &Arc<App>) {
    let options = options(app);
    if chief(app).is_none() {
        return;
    }
    let w = watched(app);
    if options.watch && since_minutes(&w.looked_at) >= WATCH_EVERY_MIN {
        look(app, false);
    }
    if check_in_due(&options.checkins, chrono::Local::now(), since_minutes(&w.checked_in_at)) {
        check_in(app);
    }
    if brief_due(&options.brief, chrono::Local::now(), w.briefed_on.as_deref()) {
        brief(app);
    }
}

/// Whether the day's brief is due: on, on one of its days, its time passed, not yet sent today.
/// (A computer asleep at the time gets it when Catbot next runs that day.)
fn brief_due(brief: &Brief, now: chrono::DateTime<chrono::Local>, briefed_on: Option<&str>) -> bool {
    use chrono::{Datelike, Timelike};
    let Some(at) = minutes(&brief.time) else { return false };
    let today = now.format("%Y-%m-%d").to_string();
    brief.on && brief.days.contains(&now.weekday().number_from_monday()) && now.hour() * 60 + now.minute() >= at && briefed_on != Some(today.as_str())
}

/// Messages after a time, in a chat.
fn since_in(store: &Store, thread: &str, since: chrono::DateTime<chrono::FixedOffset>) -> Vec<crate::store::Message> {
    store
        .messages(thread, 400)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| chrono::DateTime::parse_from_rfc3339(&m.at).is_ok_and(|at| at > since))
        .collect()
}

/// The facts the brief is written from: what each bot did, what waits on the person, what went
/// wrong, and spending.
fn brief_facts(app: &App, chief: &Bot, since: chrono::DateTime<chrono::FixedOffset>) -> String {
    use crate::store::Role;
    let mut done = Vec::new();
    let mut waiting = Vec::new();
    let mut wrong = Vec::new();
    app.with_store(|store| {
        for bot in store.bots()? {
            let mut replies = 0;
            let mut latest: Option<crate::store::Message> = None;
            for c in store.conversations(&bot.id)? {
                if app.turns.activity(&c.id) == "waiting-on-you" {
                    waiting.push(format!("{} is waiting for your approval ({})", bot.name, place(store, &c.id)));
                }
                for m in since_in(store, &c.id, since) {
                    let detail = m.detail.clone().unwrap_or_default();
                    match m.role {
                        Role::Bot if !m.body.trim().is_empty() && detail["botId"].as_str().is_none_or(|b| b == bot.id) => {
                            replies += 1;
                            latest = Some(m);
                        }
                        Role::Step if detail["kind"] == "error" => wrong.push(format!("{}: {}", place(store, &c.id), short(&m.body, 160))),
                        Role::Step if detail["kind"] == "team-change" && detail["state"] == "asking" => {
                            waiting.push(format!("A team change to approve: {} ({})", detail["title"].as_str().unwrap_or_default(), place(store, &c.id)))
                        }
                        Role::Step if detail["kind"] == "image" && detail["state"] == "choosing" => {
                            waiting.push(format!("A picture waiting for you to pick who draws it ({})", place(store, &c.id)))
                        }
                        _ => {}
                    }
                }
            }
            if bot.id == chief.id {
                continue;
            }
            match latest {
                Some(m) => done.push(format!("{}: {replies} repl{} — latest at {}: “{}”", bot.name, if replies == 1 { "y" } else { "ies" }, clock(&m.at), short(&m.body, 200))),
                None => done.push(format!("{}: nothing new", bot.name)),
            }
        }
        for channel in store.channels()? {
            let Some(c) = store.channel_conversation(&channel.id)? else { continue };
            let said = since_in(store, &c.id, since).iter().filter(|m| m.role == Role::Bot).count();
            if said > 0 {
                done.push(format!("#{}: {said} bot repl{}", channel.name, if said == 1 { "y" } else { "ies" }));
            }
        }
        Ok(())
    });
    let status = super::limits::status(app);
    let spend = &status["spend"];
    let mut money = match spend["capUsd"].as_f64() {
        Some(cap) => format!("This month: ${:.2} of the ${cap:.2} limit.", spend["usedUsd"].as_f64().unwrap_or(0.0)),
        None => format!("This month: ${:.2} (no monthly limit set).", spend["usedUsd"].as_f64().unwrap_or(0.0)),
    };
    for engine in status["engines"].as_array().into_iter().flatten() {
        let (used, limit) = (engine["week"]["used"].as_u64().unwrap_or(0), engine["week"]["limit"].as_u64());
        if let Some(limit) = limit.filter(|l| *l > 0) {
            money.push_str(&format!(" {}: {}% of this week's tokens.", engine["name"].as_str().unwrap_or_default(), used * 100 / limit));
        }
    }
    let list = |items: &[String], none: &str| if items.is_empty() { format!("- {none}") } else { items.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n") };
    format!(
        "Waiting on the person:\n{}\n\nWhat each bot did:\n{}\n\nWhat went wrong:\n{}\n\nSpending:\n- {money}",
        list(&waiting, "nothing"),
        list(&done, "no other bots yet"),
        list(&wrong, "nothing"),
    )
}

/// The daily brief: Catbot gathers the facts, the Chief writes them up in its main chat.
pub fn brief(app: &Arc<App>) -> bool {
    let Some(chief) = chief(app) else { return false };
    let now = chrono::Local::now();
    let mut w = watched(app);
    let since = w
        .briefed_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .unwrap_or_else(|| (now - chrono::Duration::hours(24)).fixed_offset());
    let facts = brief_facts(app, &chief, since);
    let text = format!(
        "Daily brief from Catbot. Write the person's brief for {} from the facts below, as a chief of staff would: \
         lead with what needs them, then what got done, what's stuck, and spending. A few short lines, no filler, \
         nothing the facts don't say. If something needs following up, hand it on with @Name after the brief.\n\n\
         Since {}:\n\n{facts}",
        now.format("%A, %B %-d"),
        since.with_timezone(&chrono::Local).format("%A %-I:%M %p"),
    );
    if !wake(app, &chief, "brief", &text) {
        return false;
    }
    w.briefed_at = Some(now.to_rfc3339());
    w.briefed_on = Some(now.format("%Y-%m-%d").to_string());
    keep(app, &w);
    true
}

/// Whether a timed check-in is due: on, on one of its days, between its times, and long enough after the last.
fn check_in_due(checkins: &Checkins, now: chrono::DateTime<chrono::Local>, minutes_since: i64) -> bool {
    use chrono::{Datelike, Timelike};
    let (Some(from), Some(to)) = (minutes(&checkins.from), minutes(&checkins.to)) else { return false };
    let at = now.hour() * 60 + now.minute();
    checkins.on
        && checkins.days.contains(&now.weekday().number_from_monday())
        && at >= from
        && at < to
        && minutes_since >= i64::from(checkins.every_hours) * 60
}

/// Where a chat is, in words: "Scout's chat “Garden plans”", "#Greenhouse".
fn place(store: &Store, thread: &str) -> String {
    let Ok(Some(c)) = store.conversation(thread) else { return "a chat".into() };
    if let Some(channel) = c.channel_id.as_deref().and_then(|id| store.channel(id).ok().flatten()) {
        return format!("#{}", channel.name);
    }
    let name = store.bot_by_id(&c.bot_id).ok().flatten().map(|b| b.name).unwrap_or_else(|| "A bot".into());
    match c.title {
        Some(title) => format!("{name}'s chat “{title}”"),
        None => format!("{name}'s chat"),
    }
}

fn clock(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at).map(|at| at.with_timezone(&chrono::Local).format("%-I:%M %p").to_string()).unwrap_or_default()
}

fn short(text: &str, most: usize) -> String {
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() > most { format!("{}…", words.chars().take(most - 1).collect::<String>()) } else { words }
}

/// Looks over everything since the last look; wakes the Chief if something went wrong. `asked`:
/// the person asked for this look, so it runs even with the watch off. Returns whether it woke the Chief.
pub fn look(app: &Arc<App>, asked: bool) -> bool {
    let Some(chief) = chief(app) else { return false };
    if !asked && !options(app).watch {
        return false;
    }
    let mut w = watched(app);
    let found = app.with_store(|store| {
        let own: Vec<String> = store.conversations(&chief.id)?.into_iter().map(|c| c.id).collect();
        let mut problems = Vec::new();
        let mut last = w.seen;
        for (thread, m) in store.steps_since(w.seen, 500)? {
            last = m.id;
            if own.contains(&thread) {
                continue;
            }
            let detail = m.detail.clone().unwrap_or_default();
            let is_error = m.role == crate::store::Role::Step && detail["kind"] == "error";
            let cant = m.role == crate::store::Role::Note && m.body.contains("can't answer");
            if is_error || cant {
                problems.push(format!("In {}, at {}: {}", place(store, &thread), clock(&m.at), short(&m.body, 300)));
            }
        }
        // Approvals waiting on the person for a while.
        let mut waiting = Vec::new();
        for bot in store.bots()?.into_iter().filter(|b| b.id != chief.id) {
            for c in store.conversations(&bot.id)? {
                if app.turns.activity(&c.id) != "waiting-on-you" {
                    continue;
                }
                let Some(card) = store.messages(&c.id, 20)?.into_iter().rev().find(|m| m.role == crate::store::Role::Card) else { continue };
                if w.waiting_told.contains(&card.id) || since_minutes(&Some(card.at.clone())) < WAITING_MIN {
                    continue;
                }
                problems.push(format!("{} has been waiting for the person's approval since {} ({}): {}", bot.name, clock(&card.at), place(store, &c.id), short(&card.body, 160)));
                waiting.push(card.id);
            }
        }
        Ok((problems, last, waiting))
    });
    let Some((problems, last, waiting)) = found else { return false };
    w.looked_at = Some(chrono::Local::now().to_rfc3339());
    if problems.is_empty() {
        w.seen = last;
        keep(app, &w);
        return false;
    }
    let report = format!(
        "Catbot's watch found something for you to look at:\n- {}\n\nFor each one: if a teammate can fix it, hand it back to them with @Name and a corrected brief (retry the same thing at most twice). If only the person can fix it (a sign-in, a key, an approval, a setting), tell them plainly what to do. Keep it short.",
        problems.join("\n- ")
    );
    if !wake(app, &chief, "watch", &report) {
        // The Chief is busy: the same things are looked at again next time.
        keep(app, &Watched { looked_at: None, ..w });
        return false;
    }
    w.seen = last;
    w.waiting_told.extend(waiting);
    let keep_from = w.waiting_told.len().saturating_sub(200);
    w.waiting_told.drain(..keep_from);
    keep(app, &w);
    true
}

/// A timed check-in: the Chief is shown where the team stands and follows up as it sees fit.
pub fn check_in(app: &Arc<App>) -> bool {
    let Some(chief) = chief(app) else { return false };
    let lines = app
        .with_store(|store| {
            let mut lines = Vec::new();
            for bot in store.bots()?.into_iter().filter(|b| b.id != chief.id) {
                let chats = store.conversations(&bot.id)?;
                let doing = chats.iter().map(|c| app.chat_activity(&c.id)).find(|a| *a != "idle").unwrap_or("idle");
                let doing = match doing {
                    "waiting-on-you" => "waiting for the person's approval",
                    "working" => "working",
                    _ => "idle",
                };
                let last = chats
                    .iter()
                    .filter_map(|c| store.messages(&c.id, 30).ok()?.into_iter().rev().find(|m| m.role == crate::store::Role::Bot && !m.body.trim().is_empty()))
                    .max_by_key(|m| m.id);
                lines.push(match last {
                    Some(m) => format!("{}: {doing}; last said at {}: “{}”", bot.name, clock(&m.at), short(&m.body, 140)),
                    None => format!("{}: {doing}; hasn't said anything yet", bot.name),
                });
            }
            Ok(lines)
        })
        .unwrap_or_default();
    let report = format!(
        "Timed check-in from Catbot. Where the team stands:\n- {}\n\nLook it over. Follow up with anyone who needs it by handing off with @Name. If nothing needs the person's attention, reply only: Nothing to report.",
        if lines.is_empty() { "No other bots yet.".to_string() } else { lines.join("\n- ") }
    );
    if !wake(app, &chief, "check-in", &report) {
        return false;
    }
    let mut w = watched(app);
    w.checked_in_at = Some(chrono::Local::now().to_rfc3339());
    keep(app, &w);
    true
}

/// Starts a Chief turn on something Catbot wrote for it, in its main chat. False when the Chief is busy or can't go.
fn wake(app: &Arc<App>, chief: &Bot, from: &str, text: &str) -> bool {
    let Some(conversation) = app.with_store(|store| store.ensure_conversation(&chief.id)) else { return false };
    if app.turns.busy(&conversation.id) || app.channel_busy(&conversation.id) {
        return false;
    }
    let runner = match super::backups::choose(app, chief) {
        Ok(Some((stand_in, _))) => stand_in,
        Ok(None) => chief.clone(),
        Err(_) => return false,
    };
    let Some(message) = super::turns::save_and_send(app, &conversation.id, crate::store::Role::Person, text, Some(&json!({ "fromCatbot": from }))) else { return false };
    let Some(request_id) = message["id"].as_str().and_then(super::wire::parse_message_id) else { return false };
    let said = format!("{text}{}{}", super::handoffs::untold_note(app, &conversation.id, &chief.id), super::team::untold_note(app, &conversation.id));
    super::turns::start(app.clone(), runner, conversation, request_id, said).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_ins_come_on_their_days_between_their_times() {
        use chrono::TimeZone;
        let on = Checkins { on: true, ..Checkins::default() };
        // Monday 2026-11-16.
        let at = |h, m| chrono::Local.with_ymd_and_hms(2026, 11, 16, h, m, 0).unwrap();
        assert!(check_in_due(&on, at(10, 0), i64::MAX));
        assert!(!check_in_due(&on, at(10, 0), 120), "three hours apart");
        assert!(check_in_due(&on, at(10, 0), 180));
        assert!(!check_in_due(&on, at(8, 59), i64::MAX) && !check_in_due(&on, at(17, 0), i64::MAX));
        let sunday = chrono::Local.with_ymd_and_hms(2026, 11, 15, 10, 0, 0).unwrap();
        assert!(!check_in_due(&on, sunday, i64::MAX));
        assert!(!check_in_due(&Checkins::default(), at(10, 0), i64::MAX), "off by default");
        assert_eq!((minutes("08:30"), minutes("24:00")), (Some(510), None));
    }

    #[test]
    fn the_brief_comes_once_on_its_days_after_its_time() {
        use chrono::TimeZone;
        let brief = Brief::default();
        let monday = |h, m| chrono::Local.with_ymd_and_hms(2026, 11, 16, h, m, 0).unwrap();
        assert!(!brief_due(&brief, monday(7, 59), None));
        assert!(brief_due(&brief, monday(8, 0), None));
        assert!(brief_due(&brief, monday(13, 0), Some("2026-11-13")), "late, if Catbot wasn't running at 8");
        assert!(!brief_due(&brief, monday(9, 0), Some("2026-11-16")), "once a day");
        let saturday = chrono::Local.with_ymd_and_hms(2026, 11, 21, 9, 0, 0).unwrap();
        assert!(!brief_due(&brief, saturday, None), "weekdays by default");
        assert!(!brief_due(&Brief { on: false, ..Brief::default() }, monday(9, 0), None));
    }

    #[test]
    fn options_default_to_no_chief() {
        assert!(Options::default().watch);
        assert_eq!(Options::default().bot_id, None);
        let set: Options = serde_json::from_value(json!({ "botId": "b1" })).unwrap();
        assert_eq!(set.bot_id.as_deref(), Some("b1"));
    }
}
