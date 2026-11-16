//! A turn's file changes, from its checkpoints (`checkpoints.rs`): the row after the turn, the
//! patch behind it, and undoing (and redoing) it.
//!
//! - `GET  /api/threads/:thread/changes/:message`: `{ patch }`.
//! - `POST /api/threads/:thread/changes/:message/undo`: the bot's folder goes back to how it was
//!   before that turn (later turns' changes go too). The bot is told on its next turn.
//! - `POST /api/threads/:thread/changes/:message/redo`: back to how it was before the undo.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, bad, fail, turns, wire};
use crate::checkpoints::{self, Checkpoints};
use crate::store::{Bot, Role};

/// A bot's checkpoints, unless they're turned off (`CATBOT_CHECKPOINTS=off`).
pub fn of(app: &App, bot: &Bot) -> Option<Checkpoints> {
    if std::env::var("CATBOT_CHECKPOINTS").is_ok_and(|v| v == "off") {
        return None;
    }
    let home = app.with_store(|store| Ok(store.home().to_path_buf()))?;
    Some(Checkpoints::new(&home, &key(app, bot), &bot.folder))
}

/// Which private repository a folder's checkpoints live in: the bot's own for its own folder,
/// one per folder otherwise (a channel's), so one folder's snapshots never land in another.
fn key(app: &App, bot: &Bot) -> String {
    let own = app.with_store(|store| store.bot_by_id(&bot.id)).flatten().map(|b| b.folder);
    if own.as_ref() == Some(&bot.folder) {
        return bot.id.clone();
    }
    // FNV-1a of the folder's path: short, and the same every time.
    let hash = bot.folder.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("folder-{hash:016x}")
}

/// Snapshots the bot's folder (off the async threads); None when there's no checkpoint.
pub async fn snapshot(points: Option<Checkpoints>, label: &'static str) -> Option<String> {
    let points = points?;
    match tokio::task::spawn_blocking(move || points.snapshot(label)).await {
        Ok(Ok(id)) => Some(id),
        Ok(Err(err)) => {
            eprintln!("catbot: no checkpoint this turn ({err})");
            None
        }
        Err(_) => None,
    }
}

/// After a turn: if it changed the folder, the row saying so.
pub async fn after_turn(app: &Arc<App>, thread: &str, bot_id: &str, points: Option<Checkpoints>, before: Option<String>) {
    let (Some(points), Some(before)) = (points, before) else { return };
    let Some(after) = snapshot(Some(points.clone()), "after a turn").await else { return };
    if after == before {
        return;
    }
    let (b, a) = (before.clone(), after.clone());
    let Ok(Ok(files)) = tokio::task::spawn_blocking(move || points.changes(&b, &a)).await else { return };
    if files.is_empty() {
        return;
    }
    let mut detail = checkpoints::detail(&before, &after, &files);
    // Whose folder it was: another bot's, when one was brought into this chat.
    detail["botId"] = json!(bot_id);
    turns::save_and_send(app, thread, Role::Step, &checkpoints::summary(&files), Some(&detail));
}

pub async fn route(app: &Arc<App>, thread: &str, message: &str, action: Option<&str>) -> Answer {
    let id = wire::parse_message_id(message).ok_or_else(|| bad("which turn?"))?;
    let conversation = app.try_store(|store| store.conversation(thread)).map_err(fail)?.ok_or_else(|| bad("no such chat"))?;
    let mut bot = app.try_store(|store| store.bot_by_id(&conversation.bot_id)).map_err(fail)?.ok_or_else(|| bad("no such bot"))?;
    let row = app.try_store(|store| store.message_in(thread, id)).map_err(fail)?;
    // A bot brought into this chat changed its own folder.
    if let Some(other) = row.as_ref().and_then(|m| m.detail.as_ref()).and_then(|d| d["botId"].as_str()).filter(|b| *b != bot.id)
        && let Some(found) = app.with_store(|store| store.bot_by_id(other)).flatten()
    {
        bot = found;
    }
    // A channel's turns changed the channel's folder.
    if let Some(channel) = conversation.channel_id.as_deref().and_then(|id| app.with_store(|store| store.channel(id)).flatten()) {
        bot.folder = channel.folder;
    }
    let row = row
        .filter(|m| m.detail.as_ref().is_some_and(|d| d["kind"] == "changes"))
        .ok_or((StatusCode::NOT_FOUND, "no such changes in this chat".to_string()))?;
    let mut detail = row.detail.clone().unwrap_or(Value::Null);
    let points = of(app, &bot).ok_or((StatusCode::CONFLICT, "checkpoints are turned off".to_string()))?;
    let (before, after) = (detail["before"].as_str().unwrap_or_default().to_string(), detail["after"].as_str().unwrap_or_default().to_string());
    match action {
        None => {
            let patch = tokio::task::spawn_blocking(move || points.patch(&before, &after)).await.map_err(fail)?.map_err(fail)?;
            Ok((StatusCode::OK, json!({ "patch": patch })))
        }
        Some(which @ ("undo" | "redo")) => {
            if app.turns.busy(thread) {
                return Err((StatusCode::CONFLICT, "wait for the bot to finish first".into()));
            }
            let undo = which == "undo";
            if detail["undone"].as_bool().unwrap_or(false) == undo {
                return Err((StatusCode::CONFLICT, if undo { "already undone" } else { "not undone" }.into()));
            }
            let target = if undo { before } else { detail["undoPoint"].as_str().unwrap_or_default().to_string() };
            let point = tokio::task::spawn_blocking(move || points.restore(&target)).await.map_err(fail)?.map_err(fail)?;
            detail["undone"] = json!(undo);
            if undo {
                detail["undoPoint"] = json!(point);
            }
            // The bot hears about it with the next message (an undo then a redo cancels out).
            detail["untold"] = json!(!detail["untold"].as_bool().unwrap_or(false));
            let wire = app
                .try_store(|store| {
                    store.update_detail(id, &detail)?;
                    let parent = store.previous_message_id(thread, id)?;
                    Ok(wire::message(&store.message(id)?.ok_or("message vanished")?, parent))
                })
                .map_err(fail)?;
            app.hub.send(json!({ "kind": "message.patch", "threadId": thread, "message": wire }));
            Ok((StatusCode::OK, json!({ "ok": true, "message": wire })))
        }
        Some(other) => Err(bad(format!("can't {other} changes"))),
    }
}

/// What the bot is told about undone (or redone) turns it hasn't heard about, marking them told.
pub fn untold_note(app: &App, thread: &str) -> String {
    let lines = app
        .with_store(|store| {
            let mut lines = Vec::new();
            for message in store.messages(thread, 500)? {
                let Some(detail) = message.detail.as_ref().filter(|d| d["kind"] == "changes" && d["untold"] == true) else { continue };
                lines.push(if detail["undone"] == true {
                    format!("the person undid your changes to your folder ({}); it is back as it was before that turn", message.body)
                } else {
                    format!("the person put back your changes to your folder ({})", message.body)
                });
                let mut detail = detail.clone();
                detail["untold"] = json!(false);
                store.update_detail(message.id, &detail)?;
            }
            Ok(lines)
        })
        .unwrap_or_default();
    if lines.is_empty() {
        return String::new();
    }
    format!("\n\n[Since your last turn {}.]", lines.join("; and "))
}
