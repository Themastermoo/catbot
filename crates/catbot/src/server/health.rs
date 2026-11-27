//! When an AI stops working (stage 27): an expired sign-in, a key turned away, its program gone,
//! or the service down or out of reach.
//!
//! Moo's choices (2026-11-25): no background checks; Catbot finds out when a message fails. An
//! outage is tried once more after a few seconds, then the message goes to the bot's backup AI,
//! as at a usage limit (`backups.rs`). The trouble is told in the Inbox (one row per AI, with
//! its fix), on the AI's row in Settings › AI, once as a desktop notification, and on the phone.
//! It clears itself when that AI answers again, when its key is changed, or on Check again.
//!
//! Stage 32.6 (OpenMausBot 0.1.182): the notification comes at most once a day for the same
//! trouble, naming the bots on that AI; and a message it stopped (no backup could answer) goes
//! again by itself once the AI is fixed: it answers, passes Check again, gets a new key, or its
//! sign-in file changes (someone signed in), looked at every 20 seconds while one waits.
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

/// How long the same trouble goes untold again (Moo's plan: one notice a day).
const TOLD_EVERY_MS: i64 = 24 * 60 * 60 * 1000;

/// The bots that run on an AI, by name.
fn bots_on(app: &App, engine_id: &str) -> Vec<String> {
    let mut names: Vec<String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().filter(|b| b.engine == engine_id).map(|b| b.name).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Files the trouble for an AI. True when it's told (at most once a day for the same trouble).
pub fn broke(app: &App, engine: &Engine, what: &str, said: &str, bot_id: &str, thread: &str) -> bool {
    let bots = bots_on(app, engine.id);
    let text = match bots.as_slice() {
        [] => words(engine, what),
        [one] => format!("{} {one} is on it.", words(engine, what)),
        many => format!("{} Bots on it: {}.", words(engine, what), many.join(", ")),
    };
    let trouble = json!({
        "engine": engine.id,
        "name": engine.name,
        "what": what,
        "text": text,
        "bots": bots,
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
    // Told once a day at most, across restarts too.
    let key = format!("trouble-told:{}:{what}", engine.id);
    let now = chrono::Utc::now().timestamp_millis();
    let last = app.with_store(|store| store.setting(&key)).flatten().and_then(|v| v.as_i64());
    let due = new && last.is_none_or(|at| now - at >= TOLD_EVERY_MS);
    if due {
        app.with_store(|store| store.set_setting(&key, &json!(now)));
    }
    if due {
        eprintln!("catbot: {} stopped working ({what}): {}", engine.name, said.chars().take(200).collect::<String>());
        app.hub.send(json!({ "kind": "ai-trouble", "trouble": trouble }));
    }
    app.hub.send(json!({ "kind": "inbox" }));
    app.hub.send(json!({ "kind": "engines" }));
    due
}

/// An AI answered (or was fixed): its trouble, if any, is over, and what it stopped goes again.
pub fn works(app: &App, engine_id: &str) {
    app.worked.lock().expect("worked lock").insert(engine_id.to_string(), chrono::Utc::now().timestamp_millis());
    let gone = app.troubles.lock().expect("troubles lock").remove(engine_id).is_some();
    if gone {
        app.fixed.notify_one();
    }
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

/// Where messages stopped by an AI wait (a setting, so a restart keeps them).
const STOPPED: &str = "stopped-by-trouble";

/// The person's fix that lets a stopped message go again: signing in, or a key that works.
pub fn waits_for_fix(what: &str) -> bool {
    matches!(what, "sign-in" | "key")
}

/// An AI's sign-in, as its file is now (when it has one Catbot can see): a change means someone
/// signed in.
fn sign_in_mark(engine: &Engine) -> Option<String> {
    let file = match engine.kind {
        Kind::Claude => std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(|| crate::store::user_home().join(".claude")).join(".credentials.json"),
        Kind::Codex => crate::codex::person_home().join("auth.json"),
        _ => return None,
    };
    let meta = file.metadata().ok()?;
    let changed = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    Some(format!("{changed}:{}", meta.len()))
}

/// A message the AI stopped, with no backup to answer: it goes again once the AI is fixed. The
/// chat says so.
pub fn stopped(app: &App, engine: &Engine, what: &str, thread: &str, message: i64, text: &str) {
    let mut all = app.with_store(|store| store.setting(STOPPED)).flatten().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    all.retain(|s| s["thread"] != thread);
    all.push(json!({
        "engine": engine.id,
        "thread": thread,
        "message": message,
        "text": text,
        "at": chrono::Utc::now().timestamp_millis(),
        "mark": sign_in_mark(engine),
    }));
    app.with_store(|store| store.set_setting(STOPPED, &Value::Array(all)));
    let when = if what == "sign-in" { format!("once {} is signed in again", engine.name) } else { format!("once {} has a key that works", engine.name) };
    super::turns::save_and_send(app, thread, crate::store::Role::Note, &format!("Catbot will send this again by itself {when}."), None);
}

/// Sends again the stopped messages whose AI is fixed. Ones older than three days are let go.
pub fn send_again(app: &Arc<App>) {
    let all = app.with_store(|store| store.setting(STOPPED)).flatten().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    if all.is_empty() {
        return;
    }
    let now = chrono::Utc::now().timestamp_millis();
    let worked = app.worked.lock().expect("worked lock").clone();
    let (mut ready, mut waiting) = (Vec::new(), Vec::new());
    for stop in all {
        let at = stop["at"].as_i64().unwrap_or(0);
        if now - at > 3 * TOLD_EVERY_MS {
            continue;
        }
        let Some(engine) = stop["engine"].as_str().and_then(engines::find) else { continue };
        let fixed = worked.get(engine.id).is_some_and(|w| *w > at) || sign_in_mark(engine).is_some_and(|mark| stop["mark"].as_str() != Some(mark.as_str()));
        if fixed { ready.push((engine, stop)) } else { waiting.push(stop) }
    }
    app.with_store(|store| store.set_setting(STOPPED, &Value::Array(waiting)));
    for (engine, stop) in ready {
        let (Some(thread), Some(message), Some(text)) = (stop["thread"].as_str(), stop["message"].as_i64(), stop["text"].as_str()) else { continue };
        if app.with_store(|store| store.conversation(thread)).flatten().is_none() {
            continue;
        }
        eprintln!("catbot: {} is fixed: sending the message it stopped in {thread} again", engine.name);
        super::turns::save_and_send(app, thread, crate::store::Role::Note, &format!("{} works again, so Catbot is sending your message again.", engine.name), None);
        app.held.lock().expect("held lock").entry(thread.to_string()).or_default().push((message, text.to_string()));
        super::release_held(app, thread);
    }
}

/// Looks for fixed AIs while a stopped message waits: when told one is, and every 20 seconds.
pub async fn keep_watch(app: Arc<App>) {
    loop {
        tokio::select! {
            _ = app.fixed.notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(20)) => {}
        }
        send_again(&app);
    }
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
