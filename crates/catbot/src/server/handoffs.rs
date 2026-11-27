//! Hand-offs (stage 15.2): a bot that ends its reply with `@Name` brings that bot in next, in a
//! channel or in its own chat.
//!
//! Moo's choice (2026-11-16): bots hand work to each other freely. So two bots can't bounce a
//! message back and forth for ever, a chain stops at a depth (3) and a number of bot replies to
//! one message (6), both changeable in Settings, which can also make hand-offs unlimited. Each
//! hand-off shows as a line in the chat ("Scout handed this to Kiwi.").
//!
//! In a channel the bots handed to are its members, working as they do there. In a bot's own chat
//! any other bot can be brought in: it works in its own folder, shown the chat so far, and its
//! reply goes in that chat under its own name. Handed back, the chat's bot carries on in its own
//! session, told what was said.

use std::collections::VecDeque;
use std::sync::Arc;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, channels, turns};
use crate::claude::PermissionMode;
use crate::store::{Bot, Role};

/// The limits on a chain of hand-offs, kept in the settings as `handoffs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rules {
    /// No limits at all: only the person (Stop) or a spend limit ends a chain.
    pub unlimited: bool,
    /// How many hand-offs deep a chain may go: one bot to another is 1.
    pub depth: u32,
    /// How many bot replies one message may bring (the ones the person asked for always answer).
    pub replies: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { unlimited: false, depth: 3, replies: 6 }
    }
}

pub fn rules(app: &App) -> Rules {
    app.with_store(|store| store.setting("handoffs")).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// `GET /api/handoffs` and `PUT /api/handoffs` (`{ unlimited, depth, replies }`).
pub fn route(app: &App, body: Option<Value>) -> Answer {
    let Some(body) = body else {
        return Ok((StatusCode::OK, json!({ "handoffs": rules(app) })));
    };
    let mut wanted: Rules = serde_json::from_value(body).map_err(|e| bad(format!("hand-off limits: {e}")))?;
    wanted.depth = wanted.depth.clamp(1, 50);
    wanted.replies = wanted.replies.clamp(1, 200);
    app.try_store(|store| store.set_setting("handoffs", &json!(wanted))).map_err(super::fail)?;
    app.hub.send(json!({ "kind": "handoffs", "handoffs": wanted }));
    Ok((StatusCode::OK, json!({ "handoffs": wanted })))
}

/// The bots a reply hands to: those it @names in its last paragraph, in the order named, never
/// the bot that wrote it.
pub fn named(reply: &str, candidates: &[Bot], writer: &str) -> Vec<Bot> {
    let last = reply.trim_end().rsplit("\n\n").next().unwrap_or_default();
    let others: Vec<Bot> = candidates.iter().filter(|b| b.id != writer).cloned().collect();
    channels::named(last, &others)
}

/// One bot's turn in a chain.
pub struct Turn {
    pub bot: Bot,
    /// How many hand-offs led here (0: the person asked for it).
    pub depth: u32,
    /// Who handed it on, and what they said.
    pub from: Option<(String, String)>,
    /// The approval level the work was handed on at: the turn runs at this or its own,
    /// whichever gives more freedom (stage 32.4).
    pub level: Option<PermissionMode>,
}

/// The level of the two that gives more freedom (Moo, 2026-11-27: "the highest level from the 2",
/// Full access included).
pub fn freer(own: PermissionMode, handed: Option<PermissionMode>) -> PermissionMode {
    let rank = |mode: PermissionMode| match mode {
        PermissionMode::Plan => 0,
        PermissionMode::Default => 1,
        PermissionMode::AcceptEdits => 2,
        PermissionMode::Auto => 3,
        PermissionMode::BypassPermissions => 4,
    };
    match handed {
        Some(handed) if rank(handed) > rank(own) => handed,
        _ => own,
    }
}

/// An approval level in the interface's words.
fn level_words(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default | PermissionMode::Plan => "Ask for approval",
        PermissionMode::AcceptEdits => "Auto-accept edits",
        PermissionMode::Auto => "Approve for me",
        PermissionMode::BypassPermissions => "Full access",
    }
}

/// Runs `bot` at the freer of its level and the one it was handed, saying so in the chat when
/// that's more than its own.
fn at_level(app: &App, thread: &str, mut bot: Bot, handed: Option<PermissionMode>, from: &str) -> Bot {
    let level = freer(bot.mode, handed);
    if level != bot.mode {
        let note = format!("{} works at {} for this, {from}'s level.", bot.name, level_words(level));
        turns::save_and_send(app, thread, Role::Note, &note, None);
        bot.mode = level;
    }
    bot
}

/// Where a chain runs.
pub enum Place {
    Channel(String),
    /// A bot's own chat (its id).
    Chat(String),
}

/// What a bot in its own chat is told about the others it can bring in.
pub fn roster_note(app: &App, bot: &Bot) -> String {
    // The Chief is told about the whole team in its own note (`chief.rs`).
    if app.with_store(|store| Ok(super::chief::is_chief_in(store, &bot.id))).unwrap_or(false) {
        return String::new();
    }
    let others: Vec<String> = app
        .with_store(|store| store.bots())
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.id != bot.id)
        .map(|b| {
            let about: String = b.instructions.split_whitespace().take(14).collect::<Vec<_>>().join(" ");
            if about.is_empty() { b.name } else { format!("{} ({about})", b.name) }
        })
        .collect();
    if others.is_empty() {
        return String::new();
    }
    format!(
        "\n\nOther bots work alongside you: {}. To hand part of the work to one, end your reply with @Name and what \
         you need from them. You'll talk in your department's team chat (or a direct chat, for a bot of \
         another department), not in this chat; then you'll be told what they said, to report back here. \
         Only hand off when another bot is better placed to help.",
        others.join("; ")
    )
}

/// What other bots said in a bot's chat since its own last reply, for a session that didn't see it.
pub fn untold_note(app: &App, thread: &str, owner: &str) -> String {
    let messages = app.with_store(|store| store.messages(thread, 60)).unwrap_or_default();
    let names: std::collections::HashMap<String, String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| (b.id, b.name)).collect();
    let mut said = Vec::new();
    for m in messages.iter().rev() {
        if m.role != Role::Bot || m.body.trim().is_empty() {
            continue;
        }
        let speaker = m.detail.as_ref().and_then(|d| d["botId"].as_str()).unwrap_or(owner);
        if speaker == owner {
            break;
        }
        let name = names.get(speaker).cloned().unwrap_or_else(|| "Another bot".into());
        said.push(format!("[{name}]: {}", m.body.trim()));
    }
    if said.is_empty() {
        return String::new();
    }
    said.reverse();
    format!("\n\n(Said in this chat since your last reply:\n{})", said.join("\n\n"))
}

/// The asker, taking its turn in the other bot's direct chat: in its own folder, told where it is.
fn in_direct(mut bot: Bot, other: &str) -> Bot {
    bot.instructions.push_str(&format!(
        "\n\nYou asked {other} for help in a direct chat between the two of you, on the person's behalf. \
         Everything said there so far is shown to you, {other}'s words marked with its name. Answer as yourself, \
         briefly, without repeating what's been said."
    ));
    bot
}

/// A bot brought into another bot's chat: in its own folder, told whose chat it's in.
fn as_guest(mut bot: Bot, owner: &str, from: &str) -> Bot {
    bot.instructions.push_str(&format!(
        "\n\n{from} has brought you into the person's chat with {owner} to help. Everything said there so far is \
         shown to you, others' words marked with their names. Answer as yourself, briefly, without repeating \
         what's been said. To hand back or on, end your reply with @Name and what you need."
    ));
    bot
}

/// The bots a reply in a bot's own chat hands on to.
pub fn next_after(app: &App, owner: &Bot, reply: &str) -> Vec<Bot> {
    let everyone = app.with_store(|store| store.bots()).unwrap_or_default();
    named(reply, &everyone, &owner.id)
}

/// A reply in the person's chat with a bot that hands on (to `next`; the chat is marked busy
/// already). Moo's rule (2026-11-27): the bots don't talk in the person's chat. Bots of the same
/// department talk in its team chat; others in a direct chat the asking bot opens with the
/// other, closed once they're done. The person's chat gets a line saying where, then the asking
/// bot's report of what came back.
pub async fn after_reply(app: &Arc<App>, owner: &Bot, thread: &str, request_id: i64, reply: &str, next: Vec<Bot>) {
    let ask = reply.trim_end().rsplit("\n\n").next().unwrap_or(reply).trim().to_string();
    let mut heard: Vec<String> = Vec::new();
    for target in next {
        if !app.channel_busy(thread) {
            break; // Stopped.
        }
        let shared = super::team_chat::shared_room(app, owner, &target);
        let place = match &shared {
            Some((channel, _)) => format!("#{}", app.with_store(|store| store.channel(channel)).flatten().map(|c| c.name).unwrap_or_else(|| "team chat".into())),
            None => "a direct chat".to_string(),
        };
        let run = begin(app, thread, owner, &target, &place);
        let mut answers = match &shared {
            Some((channel, room)) => in_team_chat(app, owner, &target, thread, channel, room, &ask, &run).await,
            None => in_direct_chat(app, owner, &target, thread, &ask, &run).await,
        };
        if stopped(app, thread, &run) {
            answers.push(format!("(The person stopped {}'s part before it was done.)", target.name));
        }
        end(app, thread, &run);
        heard.extend(answers);
    }
    // Back in the person's chat: the asking bot says what came of it.
    if app.channel_busy(thread)
        && !heard.is_empty()
        && let Some(conversation) = app.with_store(|store| store.conversation(thread)).flatten()
    {
        let mut bot = app.with_store(|store| store.bot_by_id(&owner.id)).flatten().unwrap_or_else(|| owner.clone());
        bot.instructions.push_str(&roster_note(app, &bot));
        let cue = format!(
            "{}\n\n(That's what came back on what you asked. Now report to the person here, briefly: what you \
             learned and what happens next. Don't hand off again in this reply.)",
            heard.join("\n\n")
        );
        if let Err(err) = turns::run_through(app, &bot, conversation, request_id, &cue, false).await {
            eprintln!("catbot: {} couldn't report back: {err}", owner.name);
        }
    }
    app.set_channel_busy(thread, false);
    app.announce_bot(&owner.id);
}

/// A hand-off starting from the person's chat (stage 32.13): listed for the row above its
/// message box until it ends. Returns its id.
fn begin(app: &App, thread: &str, owner: &Bot, target: &Bot, place: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let entry = json!({
        "id": id, "from": owner.name, "fromId": owner.id, "to": target.name, "toId": target.id,
        "place": place, "room": null, "chatOf": null, "startedAt": chrono::Utc::now().timestamp_millis(), "stopped": false,
    });
    app.handoffs_now.lock().expect("handoffs lock").entry(thread.to_string()).or_default().push(entry);
    told(app, thread);
    id
}

/// Where a running hand-off talks, once there's a chat for it (for Open).
fn talks_in(app: &App, thread: &str, run: &str, room: &str, chat_of: &str) {
    if let Some(entry) = app.handoffs_now.lock().expect("handoffs lock").get_mut(thread).and_then(|l| l.iter_mut().find(|e| e["id"] == run)) {
        entry["room"] = json!(room);
        entry["chatOf"] = json!(chat_of);
    }
    told(app, thread);
}

fn end(app: &App, thread: &str, run: &str) {
    if let Some(list) = app.handoffs_now.lock().expect("handoffs lock").get_mut(thread) {
        list.retain(|e| e["id"] != run);
    }
    told(app, thread);
}

/// Whether the person stopped this hand-off.
fn stopped(app: &App, thread: &str, run: &str) -> bool {
    app.handoffs_now.lock().expect("handoffs lock").get(thread).and_then(|l| l.iter().find(|e| e["id"] == run)).is_some_and(|e| e["stopped"] == true)
}

/// The hand-offs running from a chat now.
pub fn running(app: &App, thread: &str) -> Vec<Value> {
    app.handoffs_now.lock().expect("handoffs lock").get(thread).cloned().unwrap_or_default()
}

/// Stop on one hand-off: its talk ends where it is (the turn there is interrupted), and the
/// asking bot reports what came of it. False when it isn't running.
pub fn stop(app: &App, thread: &str, run: &str) -> bool {
    let room = {
        let mut all = app.handoffs_now.lock().expect("handoffs lock");
        let Some(entry) = all.get_mut(thread).and_then(|l| l.iter_mut().find(|e| e["id"] == run)) else { return false };
        entry["stopped"] = json!(true);
        (entry["room"].as_str().map(str::to_string), entry["chatOf"].as_str().map(str::to_string))
    };
    if let (Some(room), chat_of) = room {
        app.turns.interrupt(&room);
        // In a team chat, its chain stops too.
        if let Some(channel) = chat_of.filter(|c| app.with_store(|store| store.channel(c)).flatten().is_some()) {
            app.set_channel_busy(&channel, false);
        }
    }
    told(app, thread);
    true
}

/// The row's frame: `{ kind: "handoffs-running", threadId, running }`.
fn told(app: &App, thread: &str) {
    app.hub.send(json!({ "kind": "handoffs-running", "threadId": thread, "running": running(app, thread) }));
}

/// The line in the person's chat that says where the bots are talking, with Open.
fn say_where(app: &App, owner: &Bot, thread: &str, text: &str, room: &str, chat_of: &str) {
    let detail = json!({ "teamUpdate": { "botId": owner.id, "kind": "handoff", "threadId": room, "chatOf": chat_of } });
    turns::save_and_send(app, thread, Role::Note, text, Some(&detail));
}

/// Waits (up to a quarter of an hour) for a chat to be free.
async fn free(app: &App, key: &str, room: &str, thread: &str, run: &str) {
    for _ in 0..450 {
        if (!app.channel_busy(key) && !app.turns.busy(room)) || stopped(app, thread, run) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

/// Same department: the asking bot's words go in the team chat, and the other answers there (and
/// may bring in its teammates there, within the limits). What they said comes back.
#[allow(clippy::too_many_arguments)]
async fn in_team_chat(app: &Arc<App>, owner: &Bot, target: &Bot, thread: &str, channel: &str, room: &str, ask: &str, run: &str) -> Vec<String> {
    let name = app.with_store(|store| store.channel(channel)).flatten().map(|c| c.name).unwrap_or_else(|| "the team chat".into());
    say_where(app, owner, thread, &format!("{} asked {} in #{name}.", owner.name, target.name), room, channel);
    free(app, channel, room, thread, run).await;
    if stopped(app, thread, run) {
        return Vec::new();
    }
    talks_in(app, thread, run, room, channel);
    let Some(asked) = turns::save_and_send(app, room, Role::Bot, ask, Some(&json!({ "botId": owner.id }))) else { return Vec::new() };
    let Some(asked_id) = asked["id"].as_str().and_then(super::wire::parse_message_id) else { return Vec::new() };
    let mut queue = VecDeque::new();
    hand(app, room, &rules(app), &mut queue, 0, 1, Turn { bot: target.clone(), depth: 1, from: Some((owner.name.clone(), ask.to_string())), level: Some(owner.mode) });
    app.set_channel_busy(channel, true);
    channels::announce(app, channel);
    follow(app, channel, room, asked_id, queue, Place::Channel(channel.to_string()), 1, 0).await;
    app.set_channel_busy(channel, false);
    channels::announce(app, channel);
    channels::release_held(app, channel);
    // Everything the others said there after the question.
    let names: std::collections::HashMap<String, String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| (b.id, b.name)).collect();
    let said = app.with_store(|store| store.messages_after(room, asked_id)).unwrap_or_default();
    said.into_iter()
        .filter(|m| m.role == Role::Bot && !m.body.trim().is_empty())
        .filter_map(|m| {
            let who = m.detail.as_ref().and_then(|d| d["botId"].as_str().map(str::to_string))?;
            (who != owner.id).then(|| format!("[{}, in #{name}]: {}", names.get(&who).cloned().unwrap_or_else(|| "A teammate".into()), m.body.trim()))
        })
        .collect()
}

/// How many replies a direct chat between two bots may run to (hand-offs unlimited: no end).
const DIRECT_REPLIES: u32 = 20;

/// Whether a reply ends with a line saying just DONE: its bot thinks the task is done.
pub fn says_done(reply: &str) -> bool {
    reply.trim_end().lines().last().is_some_and(|line| line.trim().trim_matches(|c: char| !c.is_alphanumeric()).eq_ignore_ascii_case("done"))
}

/// Another department (or none): a direct chat of the other bot's, opened for this, where the two
/// talk it through, taking turns, until both say it's done (Moo, 2026-11-27: "an agreed upon
/// done", not one answer). Then it's closed (archived). At the limit it's left open to look in.
async fn in_direct_chat(app: &Arc<App>, owner: &Bot, target: &Bot, thread: &str, ask: &str, run: &str) -> Vec<String> {
    let Some(conversation) = app.with_store(|store| store.new_conversation(&target.id)) else { return Vec::new() };
    talks_in(app, thread, run, &conversation.id, &target.id);
    let title: String = format!("{} asks: {}", owner.name, ask.split_whitespace().collect::<Vec<_>>().join(" "));
    let title: String = if title.chars().count() > 60 { format!("{}…", title.chars().take(59).collect::<String>()) } else { title };
    let _ = app.with_store(|store| store.set_title(&conversation.id, &title, false));
    say_where(app, owner, thread, &format!("{} asked {} in a direct chat.", owner.name, target.name), &conversation.id, &target.id);
    let Some(asked) = turns::save_and_send(app, &conversation.id, Role::Bot, ask, Some(&json!({ "botId": owner.id }))) else { return Vec::new() };
    let Some(mut last_id) = asked["id"].as_str().and_then(super::wire::parse_message_id) else { return Vec::new() };
    super::departments::handed(app, &owner.id, &target.id, &conversation.id);
    let rules = rules(app);
    let limit = if rules.unlimited { u32::MAX } else { DIRECT_REPLIES };
    // The work runs at the freer of the two bots' levels (stage 32.4).
    let level = freer(target.mode, Some(owner.mode));
    if level != target.mode {
        let note = format!("{} works at {} for this, {}'s level.", target.name, level_words(level), owner.name);
        turns::save_and_send(app, &conversation.id, Role::Note, &note, None);
    }
    let how = "Talk it through here, taking turns, until the task is done. When you think it is, end your reply \
               with a line saying just DONE; the chat closes once you've both said so. Don't hand off from here.";
    // Taking turns: the one asked first, then the asker (a guest in this chat), and so on.
    let mut said: Vec<(String, String)> = vec![(owner.name.clone(), ask.to_string())];
    let mut done_by: Option<String> = None;
    let mut agreed = false;
    let mut replies = 0;
    while replies < limit && app.channel_busy(thread) && !stopped(app, thread, run) {
        let (speaker, other) = if replies % 2 == 0 { (target, owner) } else { (owner, target) };
        let now = app.with_store(|store| store.bot_by_id(&speaker.id)).flatten().unwrap_or_else(|| speaker.clone());
        let bot = match super::backups::choose(app, &now) {
            Ok(Some((stand_in, _))) => stand_in,
            Ok(None) => now,
            Err(why) => {
                turns::save_and_send(app, &conversation.id, Role::Note, &format!("{} can't answer: {why}", speaker.name), None);
                break;
            }
        };
        let (last_name, last_said) = said.last().cloned().unwrap_or_default();
        let agreeing = if done_by.as_deref() == Some(other.name.as_str()) { format!(" {} thinks it's done: say DONE too if you agree.", other.name) } else { String::new() };
        let cue = format!("[{last_name}]: {last_said}\n\n({} is another bot; you're in a direct chat on the person's behalf. {how}{agreeing})", other.name);
        let mut bot = bot;
        bot.mode = freer(bot.mode, Some(level));
        let (runner, guest) = if speaker.id == target.id { (bot, false) } else { (in_direct(bot, &target.name), true) };
        app.announce_bot(&target.id);
        let reply = turns::run_through(app, &runner, conversation.clone(), last_id, &cue, guest).await.ok().flatten();
        replies += 1;
        let Some(reply) = reply.filter(|r| !r.trim().is_empty()) else { break };
        if let Some(id) = app.with_store(|store| store.messages(&conversation.id, 1)).and_then(|m| m.last().map(|m| m.id)) {
            last_id = id;
        }
        let done = says_done(&reply);
        said.push((speaker.name.clone(), reply));
        if done && done_by.as_deref() == Some(other.name.as_str()) {
            agreed = true;
            break;
        }
        done_by = done.then(|| speaker.name.clone());
    }
    super::departments::hand_done(app, &target.id, &conversation.id);
    if stopped(app, thread, run) {
        turns::save_and_send(app, &conversation.id, Role::Note, "The person stopped this here; it's left open to look in.", None);
    } else if agreed {
        turns::save_and_send(app, &conversation.id, Role::Note, "Both said it's done: this direct chat is closed.", None);
        let _ = app.with_store(|store| store.set_archived(&conversation.id, true));
    } else if replies >= limit {
        let note = format!("Stopped after {limit} replies without both saying it's done; left open (Settings › Hand-offs).");
        turns::save_and_send(app, &conversation.id, Role::Note, &note, None);
    }
    app.announce_bot(&target.id);
    // What the other said, for the asker's report (the latest, if it was a long talk).
    let theirs: Vec<String> = said.iter().skip(1).filter(|(who, _)| *who == target.name).map(|(who, words)| format!("[{who}, in a direct chat]: {}", words.trim())).collect();
    let keep = theirs.len().saturating_sub(4);
    let mut heard: Vec<String> = theirs.into_iter().skip(keep).collect();
    if heard.is_empty() {
        heard.push(format!("[{} gave no answer in the direct chat]", target.name));
    }
    if !agreed {
        heard.push("(The direct chat ended without you both agreeing it's done.)".to_string());
    }
    heard
}

/// Queues a hand-off if the limits allow, noting it in the chat either way.
fn hand(app: &App, thread: &str, rules: &Rules, queue: &mut VecDeque<Turn>, done: u32, allowed: u32, turn: Turn) {
    if queue.iter().any(|t| t.bot.id == turn.bot.id) {
        return; // Already going to answer.
    }
    let from = turn.from.as_ref().map(|(name, _)| name.clone()).unwrap_or_default();
    if !rules.unlimited && turn.depth > rules.depth {
        let note = format!("{from} asked {} to help, but hand-offs go only {} deep (Settings › Hand-offs).", turn.bot.name, rules.depth);
        turns::save_and_send(app, thread, Role::Note, &note, None);
        return;
    }
    if !rules.unlimited && done + queue.len() as u32 >= allowed.max(rules.replies) {
        let note = format!(
            "{from} asked {} to help, but one message brings at most {} bot replies (Settings › Hand-offs).",
            turn.bot.name, rules.replies
        );
        turns::save_and_send(app, thread, Role::Note, &note, None);
        return;
    }
    turns::save_and_send(app, thread, Role::Note, &format!("{from} handed this to {}.", turn.bot.name), None);
    // The Team map draws who hands work to whom.
    if let Some(giver) = app.with_store(|store| store.bot(&from)).flatten() {
        super::departments::handed(app, &giver.id, &turn.bot.id, thread);
        // And the Team Chat hears of it (stage 30.2).
        super::team_chat::handed(app, thread, &giver, &turn.bot.name);
    }
    queue.push_back(turn);
}

/// Runs a chain's turns one after another, queueing the hand-offs each reply makes. `key` is
/// what marks it running (a channel's id, or the chat's): Stop clears it. `asked` is how many the
/// person asked for, who always answer; `done` how many have answered already.
#[allow(clippy::too_many_arguments)]
pub async fn follow(app: &Arc<App>, key: &str, thread: &str, request_id: i64, mut queue: VecDeque<Turn>, place: Place, asked: u32, mut done: u32) {
    let rules = rules(app);
    let mut first = true;
    while let Some(turn) = queue.pop_front() {
        if !app.channel_busy(key) {
            break; // Stopped.
        }
        let Some(conversation) = app.with_store(|store| store.conversation(thread)).flatten() else { break };
        // The bot as it is now, not as it was when it was handed the work: a change made while
        // it waited (Full access, say) holds for its turn.
        let now = app.with_store(|store| store.bot_by_id(&turn.bot.id)).flatten().unwrap_or_else(|| turn.bot.clone());
        // A backup takes the turn if the bot's AI is at a limit; a bot that can't go is noted.
        let bot = match super::backups::choose(app, &now) {
            Ok(Some((stand_in, why))) => {
                turns::save_and_send(app, thread, Role::Note, &format!("{}: {why}", turn.bot.name), None);
                stand_in
            }
            Ok(None) => now,
            Err(why) => {
                turns::save_and_send(app, thread, Role::Note, &format!("{} can't answer: {why}", turn.bot.name), None);
                continue;
            }
        };
        let from = turn.from.as_ref().map(|(name, _)| name.as_str());
        let bot = at_level(app, thread, bot, turn.level, from.unwrap_or_default());
        let ran_at = bot.mode;
        let (runner, guest, cue, candidates) = match &place {
            Place::Channel(id) => {
                let Some(channel) = app.with_store(|store| store.channel(id)).flatten() else { break };
                let members: Vec<Bot> = channel.members.iter().filter_map(|m| app.with_store(|store| store.bot_by_id(m)).flatten()).collect();
                let cue = match from {
                    Some(from) => format!("{from} handed this to you. It's your turn now, as {}.", bot.name),
                    None if first => format!("It's your turn to answer the person's last message, as {}.", bot.name),
                    None => format!("It's your turn now, as {}.", bot.name),
                };
                let mut runner = channels::in_channel(bot, &channel, &members);
                if let Some(dept) = super::team_chat::department_of_thread(app, thread) {
                    runner.instructions.push_str(&super::team_chat::note(&channel, &dept));
                }
                (runner, false, cue, members)
            }
            Place::Chat(owner_id) => {
                let everyone = app.with_store(|store| store.bots()).unwrap_or_default();
                let Some(owner) = everyone.iter().find(|b| &b.id == owner_id).cloned() else { break };
                let (from_name, from_said) = turn.from.clone().unwrap_or_default();
                if bot.id == owner.id {
                    // Handed back: the chat's own bot, in its own session, told what was said.
                    let mut bot = bot;
                    bot.instructions.push_str(&roster_note(app, &bot));
                    let cue = format!("[{from_name}]: {}\n\n({from_name} handed this back to you.)", from_said.trim());
                    (bot, false, cue, everyone)
                } else {
                    let cue = format!("{from_name} handed this to you. It's your turn now, as {}.", bot.name);
                    (as_guest(bot, &owner.name, &from_name), true, cue, everyone)
                }
            }
        };
        first = false;
        let reply = match turns::run_through(app, &runner, conversation, request_id, &cue, guest).await {
            Ok(reply) => reply,
            Err(err) => {
                eprintln!("catbot: {} couldn't answer: {err}", turn.bot.name);
                None
            }
        };
        done += 1;
        super::departments::hand_done(app, &turn.bot.id, thread);
        app.announce_bot(match &place {
            Place::Channel(id) => id,
            Place::Chat(owner) => owner,
        });
        let Some(reply) = reply else { continue };
        for next in named(&reply, &candidates, &turn.bot.id) {
            let handed = Turn { bot: next, depth: turn.depth + 1, from: Some((turn.bot.name.clone(), reply.clone())), level: Some(ran_at) };
            hand(app, thread, &rules, &mut queue, done, asked, handed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_offs_run_at_the_freer_level_of_the_two() {
        use PermissionMode::*;
        assert_eq!(freer(Default, Some(BypassPermissions)), BypassPermissions);
        assert_eq!(freer(Auto, Some(AcceptEdits)), Auto);
        assert_eq!(freer(Plan, Some(Default)), Default);
        assert_eq!(freer(AcceptEdits, None), AcceptEdits);
    }

    #[test]
    fn a_bot_says_done_on_a_line_of_its_own() {
        assert!(says_done("Beds planned.\n\nDONE"));
        assert!(says_done("All set.\n**Done.**\n"));
        assert!(!says_done("Done with the first bed; the second next?"));
        assert!(!says_done("DONE\n\nOne more thing: the shed."));
        assert!(!says_done(""));
    }

    #[test]
    fn only_the_last_paragraph_hands_on() {
        let bot = |id: &str, name: &str| Bot {
            id: id.into(),
            name: name.into(),
            instructions: String::new(),
            engine: "claude".into(),
            model: None,
            mode: crate::claude::PermissionMode::Default,
            codex: false,
            folder: std::path::PathBuf::new(),
            created_at: String::new(),
            mark: crate::store::Mark::CatHead,
            color: None,
            effort: None,
        };
        let all = [bot("a", "Scout"), bot("b", "Kiwi"), bot("c", "Moss")];
        assert!(named("As @Kiwi said, beans.\n\nThat's all.", &all, "a").is_empty());
        let next: Vec<String> = named("Beans.\n\n@Moss and @Kiwi, can you check the soil?", &all, "a").into_iter().map(|b| b.name).collect();
        assert_eq!(next, ["Moss", "Kiwi"]);
        assert!(named("Over to me, @Scout", &all, "a").is_empty(), "never hands to itself");
    }

    #[test]
    fn the_limits_have_defaults_and_can_be_lifted() {
        assert_eq!(Rules::default(), Rules { unlimited: false, depth: 3, replies: 6 });
        let lifted: Rules = serde_json::from_value(json!({ "unlimited": true })).unwrap();
        assert!(lifted.unlimited && lifted.depth == 3);
    }
}
