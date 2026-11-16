//! Reactions: an emoji the person puts on a message.
//!
//! `POST /api/threads/:thread/messages/:id/reactions` with `{ "emoji": "👍", "on": true }` adds
//! one (`on: false` takes it off). They're kept in the message's detail: `reactions` (shown on
//! the message) and `reactionsUntold` (not yet told to the bot). When the person next sends a
//! message, the bot is told about the untold ones along with it.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, bad, fail, wire};
use crate::store::{Message, Role};

/// The most reactions a message keeps.
const MOST: usize = 20;

pub fn react(app: &Arc<App>, thread: &str, message: &str, body: &Value) -> Answer {
    let emoji = body["emoji"].as_str().map(str::trim).unwrap_or_default();
    if !is_emoji(emoji) {
        return Err(bad("that isn't an emoji"));
    }
    let on = body["on"].as_bool().unwrap_or(true);
    let id = wire::parse_message_id(message).ok_or_else(|| bad("which message?"))?;
    let changed = app
        .try_store(|store| {
            let Some(message) = store.message_in(thread, id)? else { return Ok(None) };
            let mut detail = message.detail.clone().filter(Value::is_object).unwrap_or_else(|| json!({}));
            let mut shown = strings(&detail["reactions"]);
            let mut untold = strings(&detail["reactionsUntold"]);
            if on {
                if !shown.iter().any(|e| e == emoji) {
                    if shown.len() >= MOST {
                        return Err("a message keeps at most 20 reactions".into());
                    }
                    shown.push(emoji.to_string());
                    untold.push(emoji.to_string());
                }
            } else {
                shown.retain(|e| e != emoji);
                untold.retain(|e| e != emoji);
            }
            detail["reactions"] = json!(shown);
            detail["reactionsUntold"] = json!(untold);
            store.update_detail(id, &detail)?;
            let parent = store.previous_message_id(thread, id)?;
            let message = store.message(id)?.ok_or("message vanished")?;
            Ok(Some(wire::message(&message, parent)))
        })
        .map_err(|err| if err.to_string().contains("at most") { bad(err) } else { fail(err) })?;
    let wire = changed.ok_or((StatusCode::NOT_FOUND, "no such message in this chat".to_string()))?;
    app.hub.send(json!({ "kind": "message.patch", "threadId": thread, "message": wire }));
    Ok((StatusCode::OK, json!({ "ok": true, "message": wire })))
}

/// What the bot is told about reactions it hasn't heard about yet, and marks them told.
pub fn untold_note(app: &App, thread: &str) -> String {
    let lines = app
        .with_store(|store| {
            let mut lines = Vec::new();
            for message in store.messages(thread, 500)? {
                let Some(detail) = message.detail.as_ref() else { continue };
                let untold = strings(&detail["reactionsUntold"]);
                if untold.is_empty() {
                    continue;
                }
                lines.push(format!("{} on {}", untold.join(" "), what(&message)));
                let mut detail = detail.clone();
                detail["reactionsUntold"] = json!([]);
                store.update_detail(message.id, &detail)?;
            }
            Ok(lines)
        })
        .unwrap_or_default();
    if lines.is_empty() {
        return String::new();
    }
    format!("\n\n[Since your last turn the person reacted: {}.]", lines.join("; "))
}

/// The note saved with a person's message, for the bot.
pub fn note_of(m: &Message) -> String {
    m.detail.as_ref().and_then(|d| d["reactionNote"].as_str()).unwrap_or_default().to_string()
}

/// "your reply “The garden…”", "their message “What's…”".
fn what(m: &Message) -> String {
    let whose = match m.role {
        Role::Person => "their message",
        Role::Bot => "your reply",
        _ => "a step",
    };
    let words: String = m.body.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = words.chars().take(60).collect();
    let more = if words.chars().count() > 60 { "…" } else { "" };
    format!("{whose} “{short}{more}”")
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// An emoji, not words: short, no letters, digits, spaces or controls. (Keycaps and flags are
/// more than one character, so it's a check on what it isn't.)
fn is_emoji(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && !s.is_ascii()
        && s.chars().all(|c| !c.is_whitespace() && !c.is_control() && !c.is_alphabetic() || ('\u{1F1E6}'..='\u{1F1FF}').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_emoji_are_reactions() {
        for ok in ["👍", "❤️", "👍🏽", "🇺🇸", "1️⃣", "👨‍👩‍👧"] {
            assert!(is_emoji(ok), "{ok}");
        }
        for no in ["", "ok", "a👍", "👍 👍", "é", "\n"] {
            assert!(!is_emoji(no), "{no:?}");
        }
    }
}
