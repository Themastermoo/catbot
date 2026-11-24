//! Catbot's bots and messages in the shapes the interface reads (`packages/phone-protocol/src/wire.ts`
//! in the reference repo: `WireBot`, `WireMessage`, `ServerFrame`).

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    claude::PermissionMode,
    store::{Bot, Conversation, Message, Role, Store},
    titles,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The engine instance every Catbot bot runs on.
pub const INSTANCE: &str = "claude";

/// Message ids on the wire: `cb-<row id>`.
pub fn message_id(id: i64) -> String {
    format!("cb-{id}")
}

pub fn parse_message_id(id: &str) -> Option<i64> {
    id.strip_prefix("cb-")?.parse().ok()
}

pub fn epoch_ms(rfc3339: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(rfc3339).map(|t| t.timestamp_millis()).unwrap_or(0)
}

/// The interface's approval levels ("ask", "edits", "auto", "full", "custom").
pub fn approval_mode(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default | PermissionMode::Plan => "ask",
        PermissionMode::AcceptEdits => "edits",
        PermissionMode::Auto => "auto",
        PermissionMode::BypassPermissions => "full",
    }
}

pub fn mode_from_approval(level: &str) -> Option<PermissionMode> {
    match level {
        "ask" | "custom" => Some(PermissionMode::Default),
        "edits" => Some(PermissionMode::AcceptEdits),
        "auto" => Some(PermissionMode::Auto),
        "full" => Some(PermissionMode::BypassPermissions),
        _ => None,
    }
}

/// A bot that hasn't picked a colour gets one of these from its id, so it never changes.
const DEFAULT_COLORS: [&str; 6] = ["blue", "teal", "purple", "orange", "red", "yellow"];

/// A bot's colour, as the interface draws it.
pub fn color_of(bot: &Bot) -> String {
    color(bot)
}

fn color(bot: &Bot) -> String {
    bot.color.clone().unwrap_or_else(|| {
        let sum: usize = bot.id.bytes().map(usize::from).sum();
        DEFAULT_COLORS[sum % DEFAULT_COLORS.len()].to_string()
    })
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn model_selection(bot: &Bot) -> Value {
    json!({ "instanceId": bot.engine, "model": bot.model.clone().unwrap_or_default() })
}

/// What a chat is doing: "idle", "working" or "waiting-on-you", by its id.
pub type Activity<'a> = &'a dyn Fn(&str) -> &'static str;

/// The busiest of several activities: waiting on the person beats working beats idle.
pub fn busiest(activities: impl IntoIterator<Item = &'static str>) -> &'static str {
    let mut most = "idle";
    for activity in activities {
        match activity {
            "waiting-on-you" => return activity,
            "working" => most = activity,
            _ => {}
        }
    }
    most
}

/// One of a bot's chats as a `WireTask` (their name for a thread).
pub fn task(bot: &Bot, conversation: &Conversation, activity: &'static str) -> Value {
    let updated = epoch_ms(&conversation.updated_at).max(epoch_ms(&conversation.created_at));
    let mut wire = json!({
        "threadId": conversation.id,
        "title": conversation.title.clone().unwrap_or_else(|| titles::UNTITLED.to_string()),
        "titleByPerson": conversation.title_by_person,
        "createdAt": epoch_ms(&conversation.created_at),
        "updatedAt": updated,
        "lastThreadOrderAt": updated,
        "unread": conversation.unread,
        "activity": activity,
        "busy": activity != "idle",
        "modelSelection": model_selection(bot),
        "followsBotModel": true,
        "waitingForTeammates": false,
    });
    if let Some(at) = &conversation.archived_at {
        wire["archivedAt"] = json!(epoch_ms(at));
    }
    wire
}

/// A bot as a `WireBot`. With `messages: Some(n)` it carries its latest n messages (the bot
/// list); with None it is an announcement for a `bot` frame, which the interface merges into
/// what it has, so it must not carry messages.
///
/// `threadId` is the chat the bot opens to; `tasks` lists all its chats, archived ones marked
/// with `archivedAt`. The bot's own `activity` is the busiest of its chats'.
pub fn bot(store: &Store, bot: &Bot, activity: Activity<'_>, messages: Option<usize>) -> Result<Value, BoxError> {
    let conversation = store.ensure_conversation(&bot.id)?;
    let conversations = store.conversations(&bot.id)?;
    let tasks: Vec<Value> = conversations.iter().map(|c| task(bot, c, activity(&c.id))).collect();
    let activity = busiest(conversations.iter().map(|c| activity(&c.id)));
    let busy = activity != "idle";
    let created = epoch_ms(&bot.created_at);
    let department = super::departments::wire_of(store, &bot.id);
    let mut wire = json!({
        "id": bot.id,
        // Marks Catbot's own bots, so Catbot's interface can tell them from the reference server's.
        "catbot": true,
        "threadId": conversation.id,
        "mainThreadId": conversation.id,
        "name": bot.name,

        "soul": bot.instructions,
        // Catbot's own: the folder it works in.
        "folder": bot.folder.display().to_string(),
        "soulHash": sha256(&bot.instructions),
        "notifications": true,
        "color": color(bot),
        // Catbot's own: which mascot it wears, and whether it may use the Codex vault.
        "mark": bot.mark.as_str(),
        "codex": bot.codex,
        // Catbot's own: the Chief of Staff wears a star.
        "chief": super::chief::is_chief_in(store, &bot.id),
        // Catbot's own: its department ("" for General), and whether it leads it.
        "department": department.0,
        "departmentChief": department.1,
        // Catbot's own: the skills it uses, from the library.
        "skills": super::skills::of(store, &bot.id),
        // Catbot's own: whether it comes out onto the desktop as a pet when the app's window closes.
        "pet": super::pets::is_pet(store, &bot.id),
        "mascotBody": "cursor",
        "unread": super::inbox::unread_of(&conversations),
        "modelSelection": model_selection(bot),
        // Catbot's own: how hard it thinks, in its engine's words (null: the engine's default).
        "effort": bot.effort,
        // Catbot's own: the AIs that take over when this one is at a limit, in order.
        "backups": super::backups::wire(store, &bot.id),
        "createdAt": created,
        "avatarUrl": null,
        "autoApprove": false,
        "approvalMode": approval_mode(bot.mode),
        "activity": activity,
        "busy": busy,
        "waitingForTeammates": false,
        "tasks": tasks,
    });
    // Its role, description and own picture (stage 23).
    super::profiles::wire_into(store, &bot.id, &mut wire);
    if let Some(limit) = messages {
        let (list, more) = page(store, &conversation.id, None, limit)?;
        let (latest, _) = store.page(&conversation.id, None, 1)?;
        wire["messages"] = Value::Array(list);
        wire["hasMore"] = json!(more);
        wire["activeLeafId"] = latest.first().map(|m| json!(message_id(m.id))).unwrap_or(Value::Null);
    }
    Ok(wire)
}

/// A page of a conversation as `WireMessage`s, each pointing at the one before it.
pub fn page(store: &Store, conversation_id: &str, before: Option<i64>, limit: usize) -> Result<(Vec<Value>, bool), BoxError> {
    let (messages, more) = store.page(conversation_id, before, limit)?;
    let mut parent = match messages.first() {
        Some(first) => store.previous_message_id(conversation_id, first.id)?,
        None => None,
    };
    let mut wire = Vec::with_capacity(messages.len());
    for m in &messages {
        wire.push(message(m, parent));
        parent = Some(m.id);
    }
    Ok((wire, more))
}

/// One stored message as a `WireMessage`.
pub fn message(m: &Message, parent: Option<i64>) -> Value {
    let detail = m.detail.clone().unwrap_or(Value::Null);
    let mut wire = json!({
        "id": message_id(m.id),
        "at": epoch_ms(&m.at),
        "parentId": parent.map(message_id),
    });
    let fields = match m.role {
        Role::Person => {
            let mut fields = json!({ "role": "user", "kind": "text", "text": m.body });
            // Written by Catbot for the Chief of Staff (a problem to look at, a check-in), not by the person.
            if let Some(from) = detail["fromCatbot"].as_str() {
                fields["fromCatbot"] = json!(from);
            }
            if let Some(send_id) = detail["sendId"].as_str() {
                fields["sendId"] = json!(send_id);
            }
            if let Some(attached) = detail.get("attachments").filter(|a| a.is_array()) {
                fields["attachments"] = attached.clone();
            }
            // Written earlier and sent at a set time (stage 21): `{ for, late }`.
            if detail["scheduled"].is_object() {
                fields["scheduled"] = detail["scheduled"].clone();
            }
            fields
        }
        Role::Bot => {
            let mut fields = json!({ "role": "bot", "kind": "text", "text": m.body, "turnSucceeded": true, "turnTerminal": true });
            for (from, to) in [("item_id", "itemId"), ("turn_id", "turnId"), ("request_message_id", "requestMessageId")] {
                if let Some(value) = detail[from].as_str() {
                    fields[to] = json!(value);
                }
            }
            if let Some(attached) = detail.get("attachments").filter(|a| a.is_array()) {
                fields["attachments"] = attached.clone();
            }
            // Which bot said it: in a channel, there are several.
            if let Some(bot) = detail["botId"].as_str() {
                fields["botId"] = json!(bot);
            }
            fields
        }
        Role::Step if detail["kind"] == "image" => json!({
            "role": "bot",
            "kind": "image",
            "image": {
                "prompt": detail["prompt"],
                "askedBy": detail["askedBy"],
                "state": detail["state"],
                "options": detail["options"],
                "choice": detail["choice"],
                "choiceLabel": detail["choiceLabel"],
                "script": detail["script"],
                "error": detail["error"],
                "attachment": detail["attachment"],
            },
        }),
        Role::Step if detail["kind"] == "team-change" => json!({
            "role": "bot",
            "kind": "team-change",
            "teamChange": {
                "title": detail["title"],
                "lines": detail["lines"],
                "warning": detail["warning"],
                "askedBy": detail["askedBy"],
                "heading": detail["heading"],
                "doneText": detail["doneText"],
                "choice": detail["choice"],
                "state": detail["state"],
                "error": detail["error"],
            },
        }),
        Role::Step if detail["kind"] == "trigger" => json!({
            "role": "bot",
            "kind": "trigger",
            "trigger": {
                "said": m.body,
                "action": detail["action"],
                "id": detail["triggerId"],
                "name": detail["name"],
                "when": detail["when"],
                "kind": detail["triggerKind"],
                "before": detail["before"],
                "deleted": detail["deleted"],
            },
        }),
        Role::Step if detail["kind"] == "changes" => json!({
            "role": "bot",
            "kind": "changes",
            "changes": { "summary": m.body, "files": detail["files"], "undone": detail["undone"].as_bool().unwrap_or(false) },
        }),
        Role::Step => {
            let ok = match detail["kind"].as_str() {
                Some("approval") => detail["allowed"].as_bool().unwrap_or(false),
                Some("error") => false,
                // A tool that never reported back (the turn was stopped) didn't succeed.
                _ => detail["ok"].as_bool().unwrap_or(false),
            };
            json!({ "role": "bot", "kind": "activity", "tool": { "name": m.body, "ok": ok } })
        }
        Role::Note => json!({ "role": "bot", "kind": "activity", "tool": { "name": format!("notice: {}", m.body), "ok": true } }),
        Role::Card => json!({ "role": "bot", "kind": "options", "card": detail["card"] }),
    };
    if let (Value::Object(wire), Value::Object(fields)) = (&mut wire, fields) {
        wire.extend(fields);
    }
    if let Some(reactions) = detail.get("reactions").filter(|r| r.as_array().is_some_and(|r| !r.is_empty())) {
        wire["reactions"] = reactions.clone();
    }
    wire
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewBot;

    #[test]
    fn a_bot_on_the_wire() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let bot = store
            .create_bot(NewBot { name: "Scout".into(), instructions: "Find things.".into(), model: Some("claude-opus-5-5".into()), mode: PermissionMode::AcceptEdits, codex: true })
            .unwrap();
        let conversation = store.ensure_conversation(&bot.id).unwrap();
        let first = store.add_message(&conversation.id, Role::Person, "hi", Some(&json!({"sendId": "s-1"}))).unwrap();
        let second = store
            .add_message(&conversation.id, Role::Bot, "hello", Some(&json!({"item_id": "item-t1", "turn_id": "t1", "request_message_id": "cb-1"})))
            .unwrap();

        let wire = bot_value(&store, &bot, Some(50));
        assert_eq!(wire["threadId"], conversation.id);
        assert_eq!(wire["catbot"], true);
        assert_eq!(wire["approvalMode"], "edits");
        assert_eq!(wire["modelSelection"], json!({"instanceId": "claude", "model": "claude-opus-5-5"}));
        assert_eq!(wire["soulHash"].as_str().unwrap().len(), 64);
        assert_eq!(wire["activeLeafId"], message_id(second));
        let messages = wire["messages"].as_array().unwrap();
        assert_eq!(messages[0], json!({"id": message_id(first), "at": messages[0]["at"], "parentId": null, "role": "user", "kind": "text", "text": "hi", "sendId": "s-1"}));
        assert_eq!(messages[1]["parentId"], message_id(first));
        assert_eq!(messages[1]["itemId"], "item-t1");

        let announcement = bot_value(&store, &bot, None);
        assert!(announcement.get("messages").is_none(), "a bot frame must not replace the chat");
    }

    fn bot_value(store: &Store, b: &Bot, messages: Option<usize>) -> Value {
        bot(store, b, &|_| "idle", messages).unwrap()
    }

    #[test]
    fn a_bot_lists_its_chats_and_opens_to_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let kiwi = store
            .create_bot(NewBot { name: "Kiwi".into(), instructions: String::new(), model: None, mode: PermissionMode::Default, codex: false })
            .unwrap();
        let first = store.ensure_conversation(&kiwi.id).unwrap();
        store.add_message(&first.id, Role::Person, "first", None).unwrap();
        store.set_title(&first.id, "Garden plans", false).unwrap();
        let second = store.new_conversation(&kiwi.id).unwrap();
        let old = store.new_conversation(&kiwi.id).unwrap();
        store.set_archived(&old.id, true).unwrap();
        store.open_conversation(&second).unwrap();

        let working = |id: &str| if id == first.id { "working" } else { "idle" };
        let wire = bot(&store, &kiwi, &working, None).unwrap();
        assert_eq!(wire["threadId"], second.id, "it opens to the chat opened last");
        assert_eq!(wire["activity"], "working", "a bot is as busy as its busiest chat");
        let tasks = wire["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 3);
        let named = tasks.iter().find(|t| t["threadId"] == first.id.as_str()).unwrap();
        assert_eq!(named["title"], "Garden plans");
        assert_eq!(named["busy"], true);
        let fresh = tasks.iter().find(|t| t["threadId"] == second.id.as_str()).unwrap();
        assert_eq!(fresh["title"], titles::UNTITLED);
        assert!(fresh.get("archivedAt").is_none());
        let archived = tasks.iter().find(|t| t["threadId"] == old.id.as_str()).unwrap();
        assert!(archived["archivedAt"].as_i64().unwrap() > 0);
        assert_eq!(busiest(["idle", "working", "waiting-on-you", "working"]), "waiting-on-you");
    }

    #[test]
    fn levels_and_ids_round_trip() {
        for mode in [PermissionMode::Default, PermissionMode::AcceptEdits, PermissionMode::Auto, PermissionMode::BypassPermissions] {
            assert_eq!(mode_from_approval(approval_mode(mode)), Some(mode));
        }
        assert_eq!(parse_message_id(&message_id(42)), Some(42));
        assert_eq!(parse_message_id("3dbc0152-de9a"), None);
    }
}
