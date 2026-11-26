//! Backup AIs: when a bot's AI is at a limit (Claude's plan, or Catbot's own token or spend
//! limits, `limits.rs`), its turn goes to the first of its backups that is ready and isn't at a
//! limit too, as MausBot's fallback models do. Each bot keeps its own list, in order
//! (`backups:<bot id>` in the settings).
//!
//! A switch gets a note in the chat. The conversation's AI session is started afresh on each
//! switch (the other AI is shown the chat so far), so each AI knows everything said.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::App;
use crate::engines::{self, Kind};
use crate::store::{Bot, Store};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backup {
    pub engine: String,
    #[serde(default)]
    pub model: Option<String>,
}

fn key(bot_id: &str) -> String {
    format!("backups:{bot_id}")
}

pub fn list(store: &Store, bot_id: &str) -> Vec<Backup> {
    store.setting(&key(bot_id)).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Keeps a bot's backups (from a PATCH's `backups`), checking each AI exists.
pub fn set(store: &Store, bot: &Bot, wire: &Value) -> Result<(), BoxError> {
    let mut chosen: Vec<Backup> = Vec::new();
    for item in wire.as_array().ok_or("backups are a list")? {
        let id = item["engine"].as_str().unwrap_or_default();
        let engine = engines::find(id).ok_or_else(|| format!("Catbot doesn't know the AI {id}"))?;
        let model = item["model"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
        let backup = Backup { engine: engine.id.to_string(), model };
        if (backup.engine != bot.engine || backup.model != bot.model) && !chosen.contains(&backup) {
            chosen.push(backup);
        }
    }
    store.set_setting(&key(&bot.id), &serde_json::to_value(&chosen)?)
}

pub fn wire(store: &Store, bot_id: &str) -> Value {
    json!(list(store, bot_id))
}

/// Whether an AI can take a turn here: a key for an API, the program for an agent.
pub fn ready(app: &App, engine: &str) -> bool {
    let Some(engine) = engines::find(engine) else { return false };
    match engine.kind {
        Kind::Claude => true,
        Kind::Api => engines::access(engine, app.with_store(|store| store.engine_key(engine.id)).flatten().as_ref()).is_some(),
        Kind::Agent => engines::agent_program(engine).is_some() && engines::agent_signed_in(engine) != Some(false),
        Kind::Codex => engines::agent_program(engine).is_some() && crate::codex::signed_in(),
        Kind::Pi => engines::agent_program(engine).is_some(),
        Kind::Plan => engines::agent_program(engine).is_some() && app.chatgpt.signed_in(),
    }
}

/// The bot to run this turn as: None for the bot itself; or, when its AI is at a limit, the bot
/// on its first backup that can go, and why (with when the limit lifts). An error when none can.
pub fn choose(app: &App, bot: &Bot) -> Result<Option<(Bot, String)>, String> {
    let Some((why, until)) = super::limits::blocked(app, bot, &bot.engine) else { return Ok(None) };
    stand_in(app, bot, format!("{why}{}", until_words(until))).map(Some)
}

/// The first of the bot's backups that's ready, not at a limit and not in trouble, to answer
/// instead (`why` says why the bot's own AI isn't), with the note for the chat.
pub fn stand_in(app: &App, bot: &Bot, why: String) -> Result<(Bot, String), String> {
    let backups = app.with_store(|store| Ok(list(store, &bot.id))).unwrap_or_default();
    let mut tried = Vec::new();
    for backup in &backups {
        let name = label(backup);
        if !ready(app, &backup.engine) {
            tried.push(format!("{name} isn't set up"));
            continue;
        }
        if let Some((also, _)) = super::limits::blocked(app, bot, &backup.engine) {
            tried.push(also);
            continue;
        }
        if super::health::is_broken(app, &backup.engine) {
            tried.push(format!("{name} isn't working either"));
            continue;
        }
        let mut stand_in = bot.clone();
        stand_in.engine = backup.engine.clone();
        stand_in.model = backup.model.clone();
        return Ok((stand_in, format!("{why}, so {name} (a backup) is answering.")));
    }
    let tail = if backups.is_empty() {
        format!(". Add a backup AI in {}'s settings, or change the limits in Usage.", bot.name)
    } else {
        format!(", and its backups can't go either ({}).", tried.join("; "))
    };
    Err(format!("{why}{tail}"))
}

/// "Grok (xAI) · grok-4".
pub fn label(backup: &Backup) -> String {
    let name = engines::find(&backup.engine).map(|e| e.name).unwrap_or(backup.engine.as_str());
    match &backup.model {
        Some(model) => format!("{name} · {model}"),
        None => name.to_string(),
    }
}

fn until_words(until: Option<DateTime<Utc>>) -> String {
    until.map(|t| format!(" until {}", t.with_timezone(&chrono::Local).format("%a %-I:%M %p"))).unwrap_or_default()
}

/// Whether a failed Claude turn was turned away for its usage limit (the CLI's words for it).
pub fn says_limit(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["usage limit", "limit reached", "hit your limit", "out of extra usage", "weekly limit", "session limit"].iter().any(|w| lower.contains(w))
}

/// When a limit message says the limit lifts: Claude's `…|1763316000` form, if it's there.
pub fn lifts_at(text: &str) -> Option<DateTime<Utc>> {
    let tail = text.rsplit('|').next()?.trim();
    let secs: i64 = tail.parse().ok()?;
    DateTime::from_timestamp(secs, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_messages_are_recognised() {
        assert!(says_limit("Claude AI usage limit reached|1763316000"));
        assert!(says_limit("5-hour limit reached ∙ resets 3pm"));
        assert!(!says_limit("No conversation found with session ID: x"));
        assert_eq!(lifts_at("Claude AI usage limit reached|1763316000").map(|t| t.timestamp()), Some(1_763_316_000));
        assert_eq!(lifts_at("5-hour limit reached"), None);
        assert_eq!(label(&Backup { engine: "xai".into(), model: Some("grok-4".into()) }), "Grok (xAI) · grok-4");
    }
}
