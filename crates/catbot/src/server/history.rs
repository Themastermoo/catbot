//! A bot's settings history (stage 23.4): each change kept with the settings from before it, and
//! Undo to go back to them. Moo chose it on 2026-11-24, after MausBot's.
//!
//! - `GET  /api/bots/:id/history`: `{ changes: [{ id, at, what }] }`, newest first (100 kept).
//! - `POST /api/bots/:id/history/:change/undo`: the settings from before that change come back
//!   (itself a change, so it can be undone too).
//!
//! Kept: name, instructions, AI and model, thinking, approval level, picture choice and colour,
//! Codex access, role, description, skills, backup AIs and the picture's framing. A picture
//! replaced or removed isn't kept: its file goes.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Map, Value, json};

use super::{Answer, App, fail};
use crate::store::Bot;

/// The settings kept, and what each is called in a change's line.
const KEPT: [(&str, &str); 15] = [
    ("name", "Name"),
    ("soul", "Instructions"),
    ("modelSelection", "AI and model"),
    ("effort", "Thinking"),
    ("approvalMode", "Approval level"),
    ("mark", "Picture"),
    ("color", "Colour"),
    ("codex", "Codex access"),
    ("title", "Role"),
    ("description", "Description"),
    ("skills", "Skills"),
    ("backups", "Backup AIs"),
    ("avatarZoom", "Framing"),
    ("avatarFocusX", "Framing"),
    ("avatarFocusY", "Framing"),
];

/// The bot's kept settings now.
pub fn snapshot(app: &App, bot: &Bot) -> Value {
    let wire = app.with_store(|store| super::wire::bot(store, bot, &|_| "idle", None)).unwrap_or(Value::Null);
    let mut kept = Map::new();
    for (key, _) in KEPT {
        kept.insert(key.to_string(), wire[key].clone());
    }
    Value::Object(kept)
}

/// What changed between two snapshots, in words ("Name, Instructions").
fn changed(before: &Value, after: &Value) -> String {
    let mut names: Vec<&str> = Vec::new();
    for (key, name) in KEPT {
        if before[key] != after[key] && !names.contains(&name) {
            names.push(name);
        }
    }
    names.join(", ")
}

/// Records a change, if there was one: `before` is the bot's snapshot from before it.
pub fn record(app: &App, bot: &Bot, before: &Value, undo: bool) {
    let after = snapshot(app, bot);
    let what = changed(before, &after);
    if what.is_empty() {
        return;
    }
    let what = if undo { format!("Undo: {what}") } else { what };
    app.with_store(|store| store.add_history(&bot.id, &what, before));
}

pub fn route(app: &Arc<App>, bot: Bot, method: &axum::http::Method, rest: &[&str]) -> Answer {
    use axum::http::Method;
    match (method, rest) {
        (&Method::GET, []) => {
            let changes: Vec<Value> = app
                .try_store(|store| store.history(&bot.id))
                .map_err(fail)?
                .into_iter()
                .map(|(id, at, what)| json!({ "id": id, "at": at, "what": what }))
                .collect();
            Ok((StatusCode::OK, json!({ "changes": changes })))
        }
        (&Method::POST, [id, "undo"]) => {
            let id: i64 = id.parse().map_err(|_| (StatusCode::NOT_FOUND, "no such change".to_string()))?;
            let before = app.try_store(|store| store.history_before(&bot.id, id)).map_err(fail)?.ok_or((StatusCode::NOT_FOUND, "no such change".to_string()))?;
            // The framing goes back only while there's still a picture to frame.
            super::patch_bot_as(app, bot, before, true)
        }
        _ => Err((StatusCode::NOT_FOUND, "history doesn't do that".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_named_by_what_it_touched() {
        let before = json!({ "name": "Scout", "soul": "a", "avatarZoom": 1.0, "avatarFocusX": 0.5 });
        let after = json!({ "name": "Scout", "soul": "b", "avatarZoom": 2.0, "avatarFocusX": 0.3 });
        assert_eq!(changed(&before, &after), "Instructions, Framing");
        assert_eq!(changed(&before, &before), "");
    }
}
