//! When an AI stops working (stage 27): an expired sign-in, a key turned away, its program gone,
//! or the service down or out of reach.
//!
//! Moo's choices (2026-11-25): no background checks; Catbot finds out when a message fails. An
//! outage is tried once more after a few seconds, then the message goes to the bot's backup AI,
//! as at a usage limit (`backups.rs`). The trouble is told in the Inbox (one row per AI, with
//! its fix), on the AI's row in Settings › AI, once as a desktop notification, and on the phone.
//! It clears itself when that AI answers again, when its key is changed, or on Check again.
//!
//! - `POST /api/ai-trouble/:engine/check`: a free check (an API's model list; whether a program
//!   is there and signed in); clears the trouble when it passes.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{Answer, App};
use crate::engines::{self, Engine, Kind};

/// What went wrong, from the words the AI or its program failed with: "sign-in", "key",
/// "missing" or "down". None when it isn't one of these (a wrong model, say).
pub fn classify(engine: &Engine, said: &str) -> Option<&'static str> {
    let l = said.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| l.contains(w));
    if any(&["isn't installed", "not installed", "no such file", "couldn't start", "command not found", "program isn't"]) {
        return Some("missing");
    }
    if engine.kind == Kind::Api {
        if l.contains("turned the key away") {
            return Some(if engine.needs_key { "key" } else { "down" });
        }
    } else if any(&["/login", "please log in", "not logged in", "log in again", "sign in", "signed out", "not signed in", "oauth", "token has expired", "token expired", "authentication", "unauthorized", "invalid api key", "invalid x-api-key", "credentials", " 401"]) {
        return Some("sign-in");
    }
    if any(&[
        "couldn't reach",
        "stopped answering",
        "answered 500",
        "answered 502",
        "answered 503",
        "answered 504",
        "answered 529",
        "overloaded",
        "timed out",
        "timeout",
        "connection refused",
        "connection reset",
        "service unavailable",
        "internal server error",
        "bad gateway",
        "error sending request",
        "api error: 5",
    ]) {
        return Some("down");
    }
    None
}

/// The service's status page, for "down".
pub fn status_page(engine_id: &str) -> Option<&'static str> {
    Some(match engine_id {
        "claude" | "anthropic" => "https://status.claude.com",
        "codex" | "openai" => "https://status.openai.com",
        "xai" | "grok-cli" => "https://status.x.ai",
        "google" | "gemini" => "https://aistudio.google.com/status",
        "openrouter" => "https://status.openrouter.ai",
        "groq" => "https://groqstatus.com",
        "mistral" => "https://status.mistral.ai",
        "deepseek" => "https://status.deepseek.com",
        "cursor" => "https://status.cursor.com",
        "copilot" => "https://www.githubstatus.com",
        _ => return None,
    })
}

/// What the person reads, by what went wrong.
fn words(engine: &Engine, what: &str) -> String {
    let name = engine.name;
    match what {
        "sign-in" => format!("{name} needs you to sign in again."),
        "key" => format!("{name} turned its key away. It may have been revoked or run out of credit."),
        "missing" => format!("{name}'s program isn't on this computer any more."),
        _ => format!("{name} isn't answering. The service may be down, or this computer can't reach it."),
    }
}

/// The note in the chat when a message goes to a backup instead.
pub fn why(engine: &Engine, what: &str) -> String {
    let all = words(engine, what);
    all.split(". ").next().unwrap_or(&all).trim_end_matches('.').to_string()
}

/// The fix: what the button does ("sign-in", "key", "install" or "status") and, for the last,
/// where it goes.
fn fix(engine: &Engine, what: &str) -> Value {
    match what {
        "sign-in" => json!({ "how": "sign-in", "label": "Sign in" }),
        "key" => json!({ "how": "key", "label": "New key" }),
        "missing" => json!({ "how": "install", "label": "Install" }),
        _ => json!({ "how": "status", "label": "Status page", "url": status_page(engine.id) }),
    }
}

/// Files the trouble for an AI. True when it's new (so it's told once).
pub fn broke(app: &App, engine: &Engine, what: &str, said: &str, bot_id: &str, thread: &str) -> bool {
    let trouble = json!({
        "engine": engine.id,
        "name": engine.name,
        "what": what,
        "text": words(engine, what),
        "said": said.chars().take(400).collect::<String>(),
        "fix": fix(engine, what),
        "botId": bot_id,
        "threadId": thread,
        "at": chrono::Utc::now().timestamp_millis(),
    });
    let new = {
        let mut all = app.troubles.lock().expect("troubles lock");
        let new = all.get(engine.id).is_none_or(|old| old["what"] != what);
        all.insert(engine.id.to_string(), trouble.clone());
        new
    };
    if new {
        eprintln!("catbot: {} stopped working ({what}): {}", engine.name, said.chars().take(200).collect::<String>());
        app.hub.send(json!({ "kind": "ai-trouble", "trouble": trouble }));
    }
    app.hub.send(json!({ "kind": "inbox" }));
    app.hub.send(json!({ "kind": "engines" }));
    new
}

/// An AI answered (or was fixed): its trouble, if any, is over.
pub fn works(app: &App, engine_id: &str) {
    let gone = app.troubles.lock().expect("troubles lock").remove(engine_id).is_some();
    if gone {
        eprintln!("catbot: {engine_id} works again");
        app.hub.send(json!({ "kind": "inbox" }));
        app.hub.send(json!({ "kind": "engines" }));
    }
}

pub fn of(app: &App, engine_id: &str) -> Option<Value> {
    app.troubles.lock().expect("troubles lock").get(engine_id).cloned()
}

pub fn is_broken(app: &App, engine_id: &str) -> bool {
    app.troubles.lock().expect("troubles lock").contains_key(engine_id)
}

/// Every AI in trouble, newest first.
pub fn all(app: &App) -> Vec<Value> {
    let mut list: Vec<Value> = app.troubles.lock().expect("troubles lock").values().cloned().collect();
    list.sort_by_key(|t| std::cmp::Reverse(t["at"].as_i64().unwrap_or(0)));
    list
}

/// A free check: an API lists its models with the key; a program is there (and signed in, where
/// that can be told without asking it anything).
async fn check(app: &App, engine: &Engine) -> Result<(), String> {
    match engine.kind {
        Kind::Api => {
            let saved = app.with_store(|store| store.engine_key(engine.id)).flatten();
            let access = engines::access(engine, saved.as_ref()).ok_or("there's no key")?;
            crate::api::models(&access).await.map(|_| ())
        }
        Kind::Claude => {
            if !crate::claude::installed(&app.cli) {
                return Err("Claude Code isn't installed on this computer".into());
            }
            if engines::claude_signed_in() == Some(false) { Err("Claude Code isn't signed in yet".into()) } else { Ok(()) }
        }
        Kind::Plan => {
            engines::agent_program(engine).ok_or("Codex isn't installed")?;
            if app.chatgpt.signed_in() { Ok(()) } else { Err("ChatGPT isn't signed in".into()) }
        }
        Kind::Codex => {
            engines::agent_program(engine).ok_or("Codex isn't installed")?;
            if crate::codex::signed_in() { Ok(()) } else { Err("Codex isn't signed in".into()) }
        }
        _ => {
            engines::agent_program(engine).ok_or("its program isn't installed")?;
            if engines::agent_signed_in(engine) == Some(false) { Err("it isn't signed in".into()) } else { Ok(()) }
        }
    }
}

pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str]) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, json!({ "troubles": all(app) }))),
        (&Method::POST, [id, "check"]) => {
            let engine = engines::find(id).ok_or((StatusCode::NOT_FOUND, format!("Catbot doesn't know the AI {id}")))?;
            match check(app, engine).await {
                Ok(()) => {
                    works(app, engine.id);
                    Ok((StatusCode::OK, json!({ "ok": true })))
                }
                Err(why) => Ok((StatusCode::OK, json!({ "ok": false, "problem": why }))),
            }
        }
        _ => Err((StatusCode::NOT_FOUND, "no such trouble route".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_told_apart() {
        let claude = engines::find("claude").unwrap();
        let openai = engines::find("openai").unwrap();
        let ollama = engines::find("ollama").unwrap();
        assert_eq!(classify(claude, "Invalid API key · Please run /login"), Some("sign-in"));
        assert_eq!(classify(claude, "OAuth token has expired"), Some("sign-in"));
        assert_eq!(classify(claude, "API Error: 529 Overloaded"), Some("down"));
        assert_eq!(classify(openai, "OpenAI turned the key away (401 Unauthorized): Incorrect API key Check the key in the bot's settings."), Some("key"));
        assert_eq!(classify(openai, "Couldn't reach OpenAI at https://api.openai.com/v1: error sending request"), Some("down"));
        assert_eq!(classify(openai, "OpenAI answered 503 Service Unavailable: busy"), Some("down"));
        assert_eq!(classify(openai, "OpenAI doesn't know the model gpt-9 (nope). Pick another in the bot's settings."), None);
        assert_eq!(classify(ollama, "Couldn't reach Ollama (this computer) at http://127.0.0.1:11434/v1. Is it running on this computer?"), Some("down"));
        assert_eq!(classify(claude, "claude isn't installed on this computer"), Some("missing"));
        assert_eq!(status_page("anthropic"), Some("https://status.claude.com"));
        assert_eq!(fix(openai, "key")["how"], "key");
        assert_eq!(fix(claude, "down")["url"], "https://status.claude.com");
    }
}
