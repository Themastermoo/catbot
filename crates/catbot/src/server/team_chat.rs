//! The Team Chat (stage 30.1): one channel with every bot in it, for the team to work together.
//!
//! Moo's choices (2026-11-26):
//! - **Who answers** a message with no @name: the Chief of Staff, or with no Chief a lead the
//!   person picks (kept apart, so it comes back if the Chief goes).
//! - **Who's in it:** every bot, always. A new bot joins, a deleted or archived one leaves; it
//!   can't be taken out, and the Team Chat can't be deleted.
//! - **Pinned notes** (the channel's instructions, read by every bot there): the person edits
//!   them; a bot proposes a change with `<team-notes>…</team-notes>`, shown as a card to approve.
//!
//! It's an ordinary channel otherwise (`channels.rs`): @names, @everyone, hand-offs, held
//! messages. Its id is kept in the `team-chat` setting.

use serde_json::{Value, json};

use super::{App, channels, turns};
use crate::store::{Bot, Channel, Role, Store};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub const NAME: &str = "Team Chat";
/// The setting holding its channel's id.
const KEY: &str = "team-chat";
/// The lead the person picked, for when there's no Chief.
const LEAD: &str = "team-chat-lead";

const OPEN: &str = "<team-notes>";
const CLOSE: &str = "</team-notes>";

/// The Team Chat's channel id, if it exists.
pub fn id_in(store: &Store) -> Option<String> {
    let id = store.setting(KEY).ok().flatten()?.as_str()?.to_string();
    store.channel(&id).ok().flatten().map(|c| c.id)
}

pub fn is_team_chat(app: &App, channel_id: &str) -> bool {
    app.with_store(|store| Ok(id_in(store).is_some_and(|id| id == channel_id))).unwrap_or(false)
}

/// Whether a conversation is the Team Chat's.
pub fn is_thread(app: &App, thread: &str) -> bool {
    app.with_store(|store| {
        let Some(id) = id_in(store) else { return Ok(false) };
        Ok(store.channel_conversation(&id)?.is_some_and(|c| c.id == thread))
    })
    .unwrap_or(false)
}

/// Keeps the person's pick of lead, for when there's no Chief.
pub fn pick_lead(app: &App, bot_id: &str) {
    app.with_store(|store| store.set_setting(LEAD, &json!(bot_id)));
}

/// Who it's made of now: every bot not put away, in the sidebar's order (by name); and who
/// answers.
fn wanted(store: &Store) -> Result<(Vec<String>, Option<String>), BoxError> {
    let members: Vec<String> = store.bots()?.into_iter().map(|b| b.id).collect();
    let chief = super::chief::options_in(store).bot_id.filter(|id| members.contains(id));
    let picked = store.setting(LEAD)?.and_then(|v| v.as_str().map(str::to_string)).filter(|id| members.contains(id));
    let lead = chief.or(picked).or_else(|| members.first().cloned());
    Ok((members, lead))
}

/// Brings the Team Chat up to date with the team: made once there's a bot, every bot in it, the
/// Chief (or the person's pick) as its lead. Called at start and whenever the team changes.
pub fn sync(app: &App) {
    let changed = app.with_store(|store| {
        let (members, lead) = wanted(store)?;
        if members.is_empty() {
            return Ok(None);
        }
        let channel = match id_in(store).and_then(|id| store.channel(&id).ok().flatten()) {
            Some(channel) => channel,
            None => {
                let made = store.create_channel(NAME, &members, lead.as_deref())?;
                store.set_setting(KEY, &json!(made.id))?;
                return Ok(Some(made.id));
            }
        };
        if channel.members == members && channel.lead == lead {
            return Ok(None);
        }
        store.update_channel(&Channel { members, lead, ..channel.clone() })?;
        Ok(Some(channel.id))
    });
    if let Some(Some(id)) = changed {
        channels::announce(app, &id);
    }
}

/// What a bot in the Team Chat is told, on top of what any channel member is.
pub fn note(channel: &Channel) -> String {
    let notes = if channel.instructions.trim().is_empty() { "There are no pinned notes yet." } else { "The pinned notes are the channel's instructions above." };
    format!(
        "\n\nThis channel is the {NAME}: the whole team is here, to coordinate. {notes} To change the pinned notes (who \
         does what, goals, a checklist), put the whole new text in your reply as {OPEN}…{CLOSE}; Catbot asks the person, \
         and changes them only if they approve."
    )
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

/// A card asking the person to approve new pinned notes. It's a team change to the channel, so
/// the card and its answer are the Chief's (`team.rs`).
pub fn propose(app: &App, thread: &str, bot: &Bot, text: &str) {
    let Some(channel) = app.with_store(|store| Ok(id_in(store))).flatten() else { return };
    let mut lines = vec!["New pinned notes:".to_string()];
    lines.extend(text.lines().map(str::to_string));
    let detail = json!({
        "kind": "team-change",
        "askedBy": bot.name,
        "title": format!("Change the {NAME}'s pinned notes"),
        "lines": lines,
        "warning": Value::Null,
        "action": "change_channel",
        "target": channel,
        "body": { "soul": text },
        "state": "asking",
    });
    turns::save_and_send(app, thread, Role::Step, &format!("Team change: change the {NAME}'s pinned notes"), Some(&detail));
}

// --- Team updates (stage 30.2) -------------------------------------------------------------
//
// Moo's choice: hand-offs, finished jobs and bots waiting on him show up in the Team Chat as
// short lines, so the whole team can be followed in one place. Each says which chat it's about
// and opens it. Kept short on purpose: a hand-off made elsewhere; a job that took two minutes or
// more, a trigger's run, or a turn that failed; a bot waiting for an approval (once per chat in
// ten minutes). Nothing from the Team Chat itself. Bots aren't shown these lines.

/// How long a turn runs before its end is news.
const LONG_TURN: std::time::Duration = std::time::Duration::from_secs(120);
/// How often one chat's "waiting for you" is said.
const WAITING_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// When each chat last said it was waiting.
static WAITING: std::sync::Mutex<Option<std::collections::HashMap<String, std::time::Instant>>> = std::sync::Mutex::new(None);

/// The Team Chat's own conversation, unless `thread` is it.
fn room_for(app: &App, thread: &str) -> Option<String> {
    app.with_store(|store| {
        let Some(id) = id_in(store) else { return Ok(None) };
        let room = store.channel_conversation(&id)?.map(|c| c.id);
        Ok(room.filter(|room| room != thread))
    })
    .flatten()
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
    let Some(room) = room_for(app, thread) else { return };
    let owner = app.with_store(|store| store.conversation(thread)).flatten().map(|c| match c.channel_id {
        Some(channel) => channel,
        None => c.bot_id,
    });
    let detail = json!({ "teamUpdate": { "botId": bot.id, "kind": kind, "threadId": thread, "chatOf": owner } });
    turns::save_and_send(app, &room, Role::Note, text, Some(&detail));
    if let Some(id) = app.with_store(|store| Ok(id_in(store))).flatten() {
        channels::announce(app, &id);
    }
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
