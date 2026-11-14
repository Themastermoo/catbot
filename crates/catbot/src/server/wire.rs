//! Catbot's bots and messages in the shapes the interface reads (`packages/phone-protocol/src/wire.ts`
//! in the reference repo: `WireBot`, `WireMessage`, `ServerFrame`).

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    claude::PermissionMode,
    store::{Bot, Message, Role, Store},
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

/// The colours the interface knows; each bot gets one from its id, so it never changes.
const COLORS: [&str; 6] = ["blue", "teal", "purple", "orange", "red", "yellow"];

fn color(id: &str) -> &'static str {
    let sum: usize = id.bytes().map(usize::from).sum();
    COLORS[sum % COLORS.len()]
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn model_selection(bot: &Bot) -> Value {
    json!({ "instanceId": INSTANCE, "model": bot.model.clone().unwrap_or_default() })
}

/// A bot as a `WireBot`. With `messages: Some(n)` it carries its latest n messages (the bot
/// list); with None it is an announcement for a `bot` frame, which the interface merges into
/// what it has, so it must not carry messages.
/// `activity` is "idle", "working" or "waiting-on-you".
pub fn bot(store: &Store, bot: &Bot, activity: &str, messages: Option<usize>) -> Result<Value, BoxError> {
    let conversation = store.ensure_conversation(&bot.id)?;
    let busy = activity != "idle";
    let created = epoch_ms(&bot.created_at);
    let updated = epoch_ms(&conversation.updated_at).max(epoch_ms(&conversation.created_at));
    let mut wire = json!({
        "id": bot.id,
        // Marks Catbot's own bots, so Catbot's interface can tell them from the reference server's.
        "catbot": true,
        "threadId": conversation.id,
        "mainThreadId": conversation.id,
        "name": bot.name,
        "title": "",
        "description": "",
        "soul": bot.instructions,
        "soulHash": sha256(&bot.instructions),
        "notifications": true,
        "color": color(&bot.id),
        "mascotBody": "cursor",
        "unread": false,
        "modelSelection": model_selection(bot),
        "createdAt": created,
        "avatarUrl": null,
        "autoApprove": false,
        "approvalMode": approval_mode(bot.mode),
        "activity": activity,
        "busy": busy,
        "waitingForTeammates": false,
        "tasks": [{
            "threadId": conversation.id,
            "title": "Chat",
            "createdAt": epoch_ms(&conversation.created_at),
            "updatedAt": updated,
            "lastThreadOrderAt": updated,
            "unread": false,
            "activity": activity,
            "busy": busy,
            "modelSelection": model_selection(bot),
            "followsBotModel": true,
            "waitingForTeammates": false,
        }],
    });
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
            if let Some(send_id) = detail["sendId"].as_str() {
                fields["sendId"] = json!(send_id);
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
            fields
        }
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
        bot(store, b, "idle", messages).unwrap()
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
