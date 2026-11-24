//! A bot's chat folders (stage 23.3): its chats grouped under a name and an emoji, as MausBot's
//! projects. Moo chose them on 2026-11-24.
//!
//! - The bot's wire form carries `folders` (`{id, name, emoji}` in order) and each chat its
//!   `folderId`.
//! - `POST   /api/bots/:id/folders {name, emoji?}`: a new folder, last.
//! - `PATCH  /api/bots/:id/folders/:folder {name?, emoji?, move?: -1|1}`: rename it, or move it.
//! - `DELETE /api/bots/:id/folders/:folder`: its chats stay, out of any folder.
//! - `PATCH  /api/bots/:id/tasks/:thread {folderId}`: puts a chat in a folder (null: none).

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::store::{Bot, ChatFolder};

const NAME_MAX: usize = 60;
const FOLDERS_MAX: usize = 50;

fn name_of(body: &Value) -> Result<Option<String>, (StatusCode, String)> {
    match body.get("name") {
        None => Ok(None),
        Some(name) => {
            let name = name.as_str().unwrap_or_default().trim();
            if name.is_empty() {
                return Err(bad("a folder needs a name"));
            }
            if name.chars().count() > NAME_MAX {
                return Err(bad(format!("a folder's name is at most {NAME_MAX} letters")));
            }
            Ok(Some(name.to_string()))
        }
    }
}

fn emoji_of(body: &Value) -> Result<Option<String>, (StatusCode, String)> {
    match body.get("emoji") {
        None => Ok(None),
        Some(e) => {
            let e = e.as_str().unwrap_or_default().trim();
            // One emoji (a few code points at most: skin tones, joiners).
            if e.chars().count() > 8 || e.chars().any(|c| c.is_ascii_alphanumeric()) {
                return Err(bad("a folder's emoji is one emoji"));
            }
            Ok(Some(e.to_string()))
        }
    }
}

fn find(app: &App, bot: &Bot, id: &str) -> Result<(Vec<ChatFolder>, ChatFolder), (StatusCode, String)> {
    let all = app.try_store(|store| store.chat_folders(&bot.id)).map_err(fail)?;
    let folder = all.iter().find(|f| f.id == id).cloned().ok_or((StatusCode::NOT_FOUND, "no such folder".to_string()))?;
    Ok((all, folder))
}

pub fn route(app: &Arc<App>, bot: &Bot, method: &axum::http::Method, rest: &[&str], body: Value) -> Answer {
    use axum::http::Method;
    match (method, rest) {
        (&Method::POST, []) => {
            let name = name_of(&body)?.ok_or_else(|| bad("a folder needs a name"))?;
            let emoji = emoji_of(&body)?.unwrap_or_default();
            if app.try_store(|store| store.chat_folders(&bot.id)).map_err(fail)?.len() >= FOLDERS_MAX {
                return Err(bad(format!("{FOLDERS_MAX} folders is the most a bot can have")));
            }
            let folder = app.try_store(|store| store.add_chat_folder(&bot.id, &name, &emoji)).map_err(fail)?;
            app.announce_bot(&bot.id);
            Ok((StatusCode::CREATED, json!({ "folder": folder })))
        }
        (&Method::PATCH, [id]) => {
            let (mut all, mut folder) = find(app, bot, id)?;
            if let Some(name) = name_of(&body)? {
                folder.name = name;
            }
            if let Some(emoji) = emoji_of(&body)? {
                folder.emoji = emoji;
            }
            app.try_store(|store| store.change_chat_folder(&folder.id, &folder.name, &folder.emoji)).map_err(fail)?;
            if let Some(step) = body["move"].as_i64() {
                let at = all.iter().position(|f| f.id == folder.id).unwrap_or(0);
                let to = (at as i64 + step.signum()).clamp(0, all.len() as i64 - 1) as usize;
                let moved = all.remove(at);
                all.insert(to, moved);
                let ids: Vec<String> = all.iter().map(|f| f.id.clone()).collect();
                app.try_store(|store| store.order_chat_folders(&ids)).map_err(fail)?;
            }
            app.announce_bot(&bot.id);
            Ok((StatusCode::OK, json!({ "folder": folder })))
        }
        (&Method::DELETE, [id]) => {
            let (_, folder) = find(app, bot, id)?;
            app.try_store(|store| store.delete_chat_folder(&folder.id)).map_err(fail)?;
            app.announce_bot(&bot.id);
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        _ => Err((StatusCode::NOT_FOUND, "folders don't do that".into())),
    }
}

/// `folderId` on `PATCH …/tasks/:thread`: puts the chat in one of the bot's folders, or none.
pub fn put(app: &App, bot: &Bot, conversation_id: &str, folder: &Value) -> Result<(), (StatusCode, String)> {
    let folder = match folder {
        Value::Null => None,
        Value::String(id) => {
            find(app, bot, id)?;
            Some(id.as_str())
        }
        _ => return Err(bad("folderId is a folder's id, or null")),
    };
    app.try_store(|store| store.set_chat_folder(conversation_id, folder)).map_err(fail)
}
