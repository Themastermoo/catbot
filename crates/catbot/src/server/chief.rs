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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    /// Which bot is the Chief, if any.
    pub bot_id: Option<String>,
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
            let wanted = match &body["botId"] {
                Value::Null => None,
                Value::String(id) => {
                    let bot = app.try_store(|store| store.bot_by_id(id)).map_err(fail)?.ok_or_else(|| bad("no such bot"))?;
                    Some(bot.id)
                }
                _ => return Err(bad("botId: a bot's id, or null")),
            };
            set(app, wanted);
            Ok((StatusCode::OK, wire(app)))
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
            set(app, Some(bot.id.clone()));
            Ok((StatusCode::CREATED, wire(app)))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such Chief of Staff route".into())),
    }
}

/// Makes a bot the Chief (or none), and shows the change: the old and new Chief lose and gain their star.
fn set(app: &App, bot_id: Option<String>) {
    let before = options(app);
    let mut next = before.clone();
    next.bot_id = bot_id;
    if app.try_store(|store| store.set_setting("chief", &json!(next))).is_err() {
        return;
    }
    for id in [before.bot_id, next.bot_id.clone()].into_iter().flatten() {
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
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_to_no_chief() {
        assert_eq!(Options::default().bot_id, None);
        let set: Options = serde_json::from_value(json!({ "botId": "b1" })).unwrap();
        assert_eq!(set.bot_id.as_deref(), Some("b1"));
    }
}
