//! Team chats (stage 30.1, one per department since 30.4): a channel for each department, with
//! its bots in it, for the team to work together.
//!
//! Moo's choices (2026-11-26):
//! - **One per team:** each department with bots gets its own team chat, named after it; bots
//!   in no department (General) have none. (30.1 had one room with every bot; that one became an
//!   ordinary channel, its talk kept.)
//! - **Who answers** a message with no @name: the department's lead, or with no lead a bot the
//!   person picks in the chat's settings (kept apart, so it comes back if the lead goes).
//! - **Who's in it:** the department's bots, always: a bot joins or leaves with its department.
//!   A team chat can't lose a member or be deleted; it follows its department's name, and becomes
//!   an ordinary channel if the department is deleted.
//! - **Pinned notes** are the department's shared instructions (read by its bots every turn):
//!   the person edits them; a bot proposes a change with `<team-notes>…</team-notes>`, shown as a
//!   card to approve.
//!
//! An ordinary channel otherwise (`channels.rs`): @names, @everyone, hand-offs, held messages.
//! Kept in `team-chats` (department → channel id) and `team-chat-leads` (department → picked bot).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{App, channels, departments, turns};
use crate::store::{Bot, Channel, Role, Store};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Department → its team chat's channel id.
const KEY: &str = "team-chats";
/// Department → the lead the person picked, for when it has no lead of its own.
const LEADS: &str = "team-chat-leads";
/// 30.1's one room for every bot.
const OLD: &str = "team-chat";

const OPEN: &str = "<team-notes>";
const CLOSE: &str = "</team-notes>";

fn map(store: &Store, key: &str) -> BTreeMap<String, String> {
    store.setting(key).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// The department whose team chat this channel is.
pub fn department_of(store: &Store, channel_id: &str) -> Option<String> {
    map(store, KEY).into_iter().find(|(_, id)| id == channel_id).map(|(name, _)| name)
}

pub fn is_team_chat(app: &App, channel_id: &str) -> bool {
    app.with_store(|store| Ok(department_of(store, channel_id).is_some())).unwrap_or(false)
}

/// Every team chat's channel id.
pub fn ids_in(store: &Store) -> Vec<String> {
    map(store, KEY).into_values().collect()
}

/// The team chat a conversation is, if it is one: its channel and department.
fn of_thread(store: &Store, thread: &str) -> Option<(String, String)> {
    let channel = store.conversation(thread).ok().flatten()?.channel_id?;
    department_of(store, &channel).map(|dept| (channel, dept))
}

pub fn is_thread(app: &App, thread: &str) -> bool {
    app.with_store(|store| Ok(of_thread(store, thread).is_some())).unwrap_or(false)
}

/// Keeps the person's pick of lead for a team chat, for when its department has no lead.
pub fn pick_lead(app: &App, channel_id: &str, bot_id: &str) {
    app.with_store(|store| {
        let Some(dept) = department_of(store, channel_id) else { return Ok(()) };
        let mut leads = map(store, LEADS);
        leads.insert(dept, bot_id.to_string());
        store.set_setting(LEADS, &json!(leads))
    });
}

/// A department renamed: its team chat and pick go with it (called inside the change).
pub fn renamed_in(store: &Store, old: &str, new: &str) {
    for key in [KEY, LEADS] {
        let mut kept = map(store, key);
        if let Some(value) = kept.remove(old) {
            kept.insert(new.to_string(), value);
            let _ = store.set_setting(key, &json!(kept));
        }
    }
}

/// The department's shared instructions: the team chat's pinned notes.
pub fn notes_in(store: &Store, channel_id: &str) -> Option<String> {
    let dept = department_of(store, channel_id)?;
    departments::load(store).list.into_iter().find(|d| d.name == dept).map(|d| d.instructions)
}

/// Brings every team chat up to date with the departments: one for each department with bots,
/// its bots in it, its lead (or the person's pick) answering; one whose department is gone
/// becomes an ordinary channel. Called at start and whenever bots or departments change.
pub fn sync(app: &App) {
    let changed = app.with_store(|store| {
        let mut changed: Vec<String> = Vec::new();
        // 30.1's room for every bot: an ordinary channel from now on.
        if let Some(old) = store.setting(OLD)?.and_then(|v| v.as_str().map(str::to_string)) {
            store.set_setting(OLD, &Value::Null)?;
            changed.push(old);
        }
        let kept = departments::load(store);
        let bots = store.bots()?;
        let mut chats = map(store, KEY);
        let before = chats.clone();
        let leads = map(store, LEADS);
        // Departments that are gone: their chats are ordinary channels now.
        let gone: Vec<String> = chats.keys().filter(|name| !kept.list.iter().any(|d| &d.name == *name)).cloned().collect();
        for name in gone {
            if let Some(id) = chats.remove(&name) {
                changed.push(id);
            }
        }
        for dept in &kept.list {
            let members: Vec<String> = bots.iter().filter(|b| kept.members.get(&b.id) == Some(&dept.name)).map(|b| b.id.clone()).collect();
            if members.is_empty() {
                if let Some(id) = chats.remove(&dept.name) {
                    changed.push(id);
                }
                continue;
            }
            let chief = dept.chief.clone().filter(|id| members.contains(id));
            let picked = leads.get(&dept.name).cloned().filter(|id| members.contains(id));
            let lead = chief.or(picked).or_else(|| members.first().cloned());
            match chats.get(&dept.name).and_then(|id| store.channel(id).ok().flatten()) {
                Some(channel) => {
                    if channel.members != members || channel.lead != lead || channel.name != dept.name {
                        store.update_channel(&Channel { members, lead, name: dept.name.clone(), ..channel.clone() })?;
                        changed.push(channel.id);
                    }
                }
                None => {
                    let made = store.create_channel(&dept.name, &members, lead.as_deref())?;
                    chats.insert(dept.name.clone(), made.id.clone());
                    changed.push(made.id);
                }
            }
        }
        if chats != before {
            store.set_setting(KEY, &json!(chats))?;
        }
        Ok::<_, BoxError>(changed)
    });
    for id in changed.into_iter().flatten() {
        channels::announce(app, &id);
    }
}

/// What a bot in a team chat is told, on top of what any channel member is.
pub fn note(channel: &Channel, dept: &str) -> String {
    let _ = channel;
    format!(
        "\n\nThis channel is the {dept} department's team chat: its bots are here, to coordinate. Its pinned notes are the \
         department's shared instructions. To change them (who does what, goals, a checklist), put the whole new text in \
         your reply as {OPEN}…{CLOSE}; Catbot asks the person, and changes them only if they approve."
    )
}

/// The department of the team chat a thread is, for the note above.
pub fn department_of_thread(app: &App, thread: &str) -> Option<String> {
    app.with_store(|store| Ok(of_thread(store, thread).map(|(_, d)| d))).flatten()
}

/// Takes `<team-notes>…</team-notes>` out of a reply: the reply without it, and the last
/// proposed text (one change at a time; the last is the one meant).
pub fn take(reply: &str) -> (String, Option<String>) {
    let mut rest = String::new();
    let mut found = None;
    let mut text = reply;
    while let Some(start) = text.find(OPEN) {
        rest.push_str(&text[..start]);
        let after = &text[start + OPEN.len()..];
        match after.find(CLOSE) {
            Some(end) => {
                found = Some(after[..end].trim().to_string());
                text = &after[end + CLOSE.len()..];
            }
            None => text = "",
        }
    }
    rest.push_str(text);
    (rest.trim().to_string(), found)
}

/// A card asking the person to approve new pinned notes. It's a team change to the channel (its
/// notes are the department's instructions), so the card and its answer are the Chief's (`team.rs`).
pub fn propose(app: &App, thread: &str, bot: &Bot, text: &str) {
    let Some((channel, dept)) = app.with_store(|store| Ok(of_thread(store, thread))).flatten() else { return };
    let mut lines = vec!["New pinned notes (the department's shared instructions):".to_string()];
    lines.extend(text.lines().map(str::to_string));
    let detail = json!({
        "kind": "team-change",
        "askedBy": bot.name,
        "title": format!("Change {dept}'s pinned notes"),
        "lines": lines,
        "warning": Value::Null,
        "action": "change_channel",
        "target": channel,
        "body": { "soul": text },
        "state": "asking",
    });
    turns::save_and_send(app, thread, Role::Step, &format!("Team change: change {dept}'s pinned notes"), Some(&detail));
}

// --- Team updates (stage 30.2) -------------------------------------------------------------
//
// Moo's choice: hand-offs, finished jobs and bots waiting on him show up in the bot's team chat
// (its department's; none for General) as short lines, so each team can be followed in one place. Each says which chat it's about
// and opens it. Kept short on purpose: a hand-off made elsewhere; a job that took two minutes or
// more, a trigger's run, or a turn that failed; a bot waiting for an approval (once per chat in
// ten minutes). Nothing from the Team Chat itself. Bots aren't shown these lines.

/// How long a turn runs before its end is news.
const LONG_TURN: std::time::Duration = std::time::Duration::from_secs(120);
/// How often one chat's "waiting for you" is said.
const WAITING_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// When each chat last said it was waiting.
static WAITING: std::sync::Mutex<Option<std::collections::HashMap<String, std::time::Instant>>> = std::sync::Mutex::new(None);

/// The team chat of the bot's department (its channel, and its conversation), unless `thread`
/// is it. None for a bot in General.
fn room_for(app: &App, bot: &Bot, thread: &str) -> Option<(String, String)> {
    app.with_store(|store| {
        let Some(dept) = departments::load(store).of(&bot.id).map(|d| d.name.clone()) else { return Ok(None) };
        let Some(id) = map(store, KEY).get(&dept).cloned() else { return Ok(None) };
        let room = store.channel_conversation(&id)?.map(|c| c.id);
        Ok(room.filter(|room| room != thread).map(|room| (id, room)))
    })
    .flatten()
}

/// The team chat two bots share, if they're in the same department and it has one: its channel
/// and its conversation.
pub fn shared_room(app: &App, one: &Bot, other: &Bot) -> Option<(String, String)> {
    let same = app.with_store(|store| {
        let kept = departments::load(store);
        Ok(kept.of(&one.id).zip(kept.of(&other.id)).is_some_and(|(a, b)| a.name == b.name))
    });
    if same != Some(true) {
        return None;
    }
    room_for(app, one, "")
}

/// Where a chat is, in words: "in #Garden", "in Palette's chat", "in its chat “Seeds”".
fn place(app: &App, thread: &str, bot: &Bot) -> String {
    app.with_store(|store| {
        let Some(conversation) = store.conversation(thread)? else { return Ok(String::new()) };
        if let Some(channel) = conversation.channel_id.as_deref().and_then(|id| store.channel(id).ok().flatten()) {
            return Ok(format!(" in #{}", channel.name));
        }
        let title = conversation.title.filter(|t| !t.trim().is_empty());
        if conversation.bot_id == bot.id {
            return Ok(match title {
                Some(title) => format!(" in its chat “{}”", title.trim()),
                None => " in its chat".to_string(),
            });
        }
        let owner = store.bot_by_id(&conversation.bot_id)?.map(|b| b.name).unwrap_or_else(|| "another bot".into());
        Ok(format!(" in {owner}'s chat"))
    })
    .unwrap_or_default()
}

/// Puts one line in the Team Chat about `bot`, opening `thread`.
pub fn post(app: &App, bot: &Bot, kind: &str, thread: &str, text: &str) {
    let Some((channel, room)) = room_for(app, bot, thread) else { return };
    let owner = app.with_store(|store| store.conversation(thread)).flatten().map(|c| match c.channel_id {
        Some(channel) => channel,
        None => c.bot_id,
    });
    let detail = json!({ "teamUpdate": { "botId": bot.id, "kind": kind, "threadId": thread, "chatOf": owner } });
    turns::save_and_send(app, &room, Role::Note, text, Some(&detail));
    channels::announce(app, &channel);
}

/// A bot handed work to another, somewhere other than the Team Chat.
pub fn handed(app: &App, thread: &str, from: &Bot, to: &str) {
    let text = format!("{} handed work to {to}{}.", from.name, place(app, thread, from));
    post(app, from, "handoff", thread, &text);
}

/// A turn ended: news if it failed, or took a while.
pub fn after_turn(app: &App, bot: &Bot, thread: &str, how: &str, took: std::time::Duration, why: Option<&str>) {
    let text = match how {
        "failed" => format!("{} couldn't finish{}{}.", bot.name, place(app, thread, bot), why.map(|w| format!(": {}", short(w))).unwrap_or_default()),
        "done" if took >= LONG_TURN => format!("{} finished{} (after {} min).", bot.name, place(app, thread, bot), took.as_secs().div_ceil(60)),
        _ => return,
    };
    post(app, bot, if how == "failed" { "failed" } else { "finished" }, thread, &text);
}

/// A trigger's run ended.
pub fn trigger_ran(app: &App, bot: &Bot, thread: &str, name: &str, ok: bool) {
    let text = if ok { format!("{} ran the trigger “{name}”{}.", bot.name, place(app, thread, bot)) } else { format!("{}'s trigger “{name}” failed{}.", bot.name, place(app, thread, bot)) };
    post(app, bot, if ok { "finished" } else { "failed" }, thread, &text);
}

/// A bot asks the person's approval: said once per chat in a while.
pub fn waiting(app: &App, bot: &Bot, thread: &str, what: &str) {
    {
        let mut said = WAITING.lock().unwrap_or_else(|p| p.into_inner());
        let said = said.get_or_insert_with(Default::default);
        if said.get(thread).is_some_and(|at| at.elapsed() < WAITING_EVERY) {
            return;
        }
        said.insert(thread.to_string(), std::time::Instant::now());
    }
    let text = format!("{} is waiting for you{}: {}.", bot.name, place(app, thread, bot), short(what));
    post(app, bot, "waiting", thread, &text);
}

/// The start of a long text, on one line.
fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 100 { format!("{}…", line.chars().take(99).collect::<String>()) } else { line }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposed_notes_come_out_of_the_reply() {
        let (rest, notes) = take("Updated the plan.\n<team-notes>\n- Kiwi: seeds\n- Scout: beds\n</team-notes>\nShout if not.");
        assert_eq!(rest, "Updated the plan.\n\nShout if not.");
        assert_eq!(notes.as_deref(), Some("- Kiwi: seeds\n- Scout: beds"));
        assert_eq!(take("No notes here."), ("No notes here.".to_string(), None));
    }

    #[test]
    fn long_texts_are_cut_short() {
        assert_eq!(short("Run\n  the tests"), "Run the tests");
        assert_eq!(short(&"a".repeat(150)).chars().count(), 100);
    }
}
