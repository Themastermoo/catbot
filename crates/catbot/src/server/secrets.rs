//! Secrets from 1Password, handed over unseen (stage 17.3).
//!
//! Moo's choice (2026-11-17): a 1Password service account with read access to a vault made for
//! bots. A bot he ticks can list what's there (names and fields, never values) and name a secret
//! (`op://Vault/Item/field`); Catbot reads it with the 1Password CLI (`op`) and hands it into a
//! command (as an environment variable) or a file, on the bot's own computer. The value isn't
//! kept, isn't shown in the chat, and isn't given to the AI: Catbot blanks it (and its base64)
//! out of whatever comes back. The token is kept here and only its last four characters ever go
//! back. Every use is noted, for Settings › Computers to show.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bot_tools::Grant;
use super::App;

const SETTING: &str = "secrets";
const USES: &str = "secret-uses";
const MOST_USES: usize = 200;
/// The most secrets one command gets.
pub const MOST_AT_ONCE: usize = 20;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Kept {
    token: String,
    /// The bots that may use secrets.
    bots: Vec<String>,
    /// The vaults the account could read when connected (names only).
    vaults: Vec<String>,
}

fn kept(app: &App) -> Kept {
    app.with_store(|store| Ok(store.setting(SETTING)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())).unwrap_or_default()
}

fn keep(app: &App, k: &Kept) {
    app.with_store(|store| store.set_setting(SETTING, &json!(k)));
    app.hub.send(json!({ "kind": "computers" }));
}

/// Whether a bot may use secrets: 1Password is connected and the person ticked it.
pub fn allowed(app: &App, bot_id: &str) -> bool {
    let k = kept(app);
    !k.token.is_empty() && k.bots.iter().any(|b| b == bot_id)
}

/// The 1Password CLI: `CATBOT_OP` if set, else `op` on the PATH.
fn program() -> Option<Vec<String>> {
    if let Ok(set) = std::env::var("CATBOT_OP") {
        let words: Vec<String> = set.split_whitespace().map(String::from).collect();
        if !words.is_empty() {
            return Some(words);
        }
    }
    let found = std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("op").is_file()));
    found.then(|| vec!["op".to_string()])
}

async fn op(token: &str, args: &[&str]) -> Result<String, String> {
    let words = program().ok_or("the 1Password CLI (op) isn't installed on this computer")?;
    let (first, leading) = words.split_first().ok_or("no 1Password CLI")?;
    let mut command = tokio::process::Command::new(first);
    command.args(leading).args(args).env("OP_SERVICE_ACCOUNT_TOKEN", token).env_remove("OP_FORMAT").stdin(std::process::Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output()).await.map_err(|_| "1Password took too long".to_string())?.map_err(|e| format!("couldn't run op: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        let said = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it didn't work").trim();
        // "[ERROR] 2026/11/17 18:00:00 what went wrong": just what went wrong.
        let said = said.trim_start_matches("[ERROR]").trim();
        let mut words = said.splitn(3, ' ');
        let stamped = matches!((words.next(), words.next()), (Some(d), Some(t)) if d.contains('/') && t.contains(':') && d.starts_with(|c: char| c.is_ascii_digit()));
        Err(if stamped { said.splitn(3, ' ').nth(2).unwrap_or(said).to_string() } else { said.to_string() })
    }
}

fn ends(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

/// What the settings show.
pub fn shown(app: &App) -> Value {
    let k = kept(app);
    let mut view = json!({ "cli": program().is_some(), "connected": !k.token.is_empty(), "bots": k.bots, "vaults": k.vaults });
    if !k.token.is_empty() {
        view["ends"] = json!(ends(&k.token));
    }
    view
}

/// `PUT /api/computers/secrets {token?, bots?}`: connects (checked with 1Password first),
/// disconnects (`token: null`), or says which bots may use secrets.
pub async fn change(app: &App, body: &Value) -> Result<Value, String> {
    let mut k = kept(app);
    if body.get("token").is_some() {
        match body["token"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
            None => k = Kept { bots: k.bots, ..Kept::default() },
            Some(token) => {
                if !token.starts_with("ops_") {
                    return Err("A service account token starts with ops_ (1Password.com › Developer › Service accounts).".into());
                }
                let listed = op(token, &["vault", "list", "--format", "json"]).await.map_err(|e| format!("1Password didn't take that token: {e}"))?;
                let vaults: Vec<Value> = serde_json::from_str(&listed).unwrap_or_default();
                k.token = token.to_string();
                k.vaults = vaults.iter().filter_map(|v| v["name"].as_str().map(str::to_string)).collect();
            }
        }
    }
    if let Some(bots) = body["bots"].as_array() {
        k.bots = bots.iter().filter_map(|b| b.as_str().map(str::to_string)).collect();
    }
    keep(app, &k);
    Ok(shown(app))
}

/// The last uses, newest first, with the bots' names.
pub fn uses(app: &App) -> Vec<Value> {
    let list: Vec<Value> = app.with_store(|store| store.setting(USES)).flatten().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let names: std::collections::HashMap<String, String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| (b.id, b.name)).collect();
    list.into_iter()
        .rev()
        .take(20)
        .map(|mut u| {
            let name = u["botId"].as_str().and_then(|id| names.get(id)).cloned().unwrap_or_else(|| "A deleted bot".into());
            u["botName"] = json!(name);
            u
        })
        .collect()
}

fn note_use(app: &App, bot_id: &str, reference: &str, how: &str) {
    let mut list: Vec<Value> = app.with_store(|store| store.setting(USES)).flatten().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    list.push(json!({ "at": chrono::Utc::now().to_rfc3339(), "botId": bot_id, "reference": reference, "how": how }));
    let extra = list.len().saturating_sub(MOST_USES);
    list.drain(..extra);
    app.with_store(|store| store.set_setting(USES, &json!(list)));
    app.hub.send(json!({ "kind": "computers" }));
}

/// Whether a name can be an environment variable's.
pub fn good_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') && name.len() <= 64
}

/// Whether text is a secret reference: `op://Vault/Item/field` (or with a section).
pub fn good_reference(reference: &str) -> bool {
    reference.strip_prefix("op://").is_some_and(|rest| rest.split('/').filter(|p| !p.is_empty()).count() >= 3 && !rest.chars().any(char::is_control))
}

/// Reads secrets for a bot, noting each use. `how` says where they go ("command", "file").
pub async fn read(app: &App, bot_id: &str, wanted: &[(String, String)], how: &str) -> Result<Vec<(String, String)>, String> {
    if !allowed(app, bot_id) {
        return Err("You can't use secrets: the person hasn't allowed it for you (Settings › Computers).".into());
    }
    let token = kept(app).token;
    let mut found = Vec::new();
    for (name, reference) in wanted {
        let value = op(&token, &["read", "--no-newline", reference]).await.map_err(|e| format!("Couldn't read {reference}: {e}"))?;
        note_use(app, bot_id, reference, how);
        found.push((name.clone(), value));
    }
    Ok(found)
}

/// Blanks the secrets (and their base64) out of text that goes back to a bot.
pub fn scrub(text: &str, secrets: &[(String, String)]) -> String {
    use base64::Engine;
    let mut out = text.to_string();
    for (name, value) in secrets {
        if value.chars().count() < 3 {
            continue;
        }
        let blank = format!("[secret {name}]");
        let encoded = base64::engine::general_purpose::STANDARD.encode(value);
        for form in [value.as_str(), encoded.as_str(), encoded.trim_end_matches('=')] {
            if !form.is_empty() {
                out = out.replace(form, &blank);
            }
        }
    }
    out
}

// ---- the bot's tools -----------------------------------------------------------------------

pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    if !allowed(app, bot_id) {
        return Vec::new();
    }
    let mut tools = vec![json!({
        "name": "secrets_list",
        "description": "List the secrets the person keeps for bots in 1Password: item names, or one item's fields with the reference that names each (op://Vault/Item/field). Values are never shown. Use a reference with computer_run's secrets, or secret_to_file.",
        "inputSchema": {
            "type": "object",
            "properties": { "item": { "type": "string", "description": "An item's name or id, to list its fields" } },
        },
    })];
    if super::computers::has(app, bot_id) {
        tools.push(json!({
            "name": "secret_to_file",
            "description": "Put a secret into a file on your own computer, readable only by you, without seeing it. Use for keys and certificates a program reads from a file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "reference": { "type": "string", "description": "op://Vault/Item/field, from secrets_list" },
                    "path": { "type": "string", "description": "Where on your computer, such as ~/.ssh/id_ed25519 or keys/token.txt" },
                },
                "required": ["reference", "path"],
            },
        }));
    }
    tools
}

/// `secrets_list`.
pub async fn call(app: &Arc<App>, grant: &Grant, args: &Value) -> Result<String, String> {
    if !allowed(app, &grant.bot_id) {
        return Err("You can't use secrets.".into());
    }
    let token = kept(app).token;
    let items: Vec<Value> = serde_json::from_str(&op(&token, &["item", "list", "--format", "json"]).await?).map_err(|_| "1Password's list didn't read".to_string())?;
    let item = args["item"].as_str().map(str::trim).filter(|i| !i.is_empty());
    let Some(wanted) = item else {
        if items.is_empty() {
            return Ok("There are no secrets for bots yet.".into());
        }
        let mut lines: Vec<String> = items
            .iter()
            .map(|i| format!("{} › {} ({})", i["vault"]["name"].as_str().unwrap_or("?"), i["title"].as_str().unwrap_or("?"), i["category"].as_str().unwrap_or("item").to_lowercase().replace('_', " ")))
            .collect();
        lines.sort();
        return Ok(format!("{}\n\nAsk for one item to see its fields and their references.", lines.join("\n")));
    };
    let found = items
        .iter()
        .find(|i| i["id"].as_str() == Some(wanted) || i["title"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(wanted)))
        .ok_or_else(|| format!("There's no item called {wanted}."))?;
    let id = found["id"].as_str().unwrap_or_default();
    let vault = found["vault"]["id"].as_str().unwrap_or_default();
    let detail: Value = serde_json::from_str(&op(&token, &["item", "get", id, "--vault", vault, "--format", "json"]).await?).map_err(|_| "1Password's item didn't read".to_string())?;
    // Labels, kinds and references only: a value never leaves this function.
    let fields: Vec<String> = detail["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|f| f["reference"].as_str().is_some_and(good_reference))
        .map(|f| format!("{} ({}): {}", f["label"].as_str().unwrap_or("field"), f["type"].as_str().unwrap_or("text").to_lowercase(), f["reference"].as_str().unwrap_or_default()))
        .collect();
    if fields.is_empty() {
        return Ok(format!("{} has no fields to use.", found["title"].as_str().unwrap_or(wanted)));
    }
    Ok(format!("{}:\n{}", found["title"].as_str().unwrap_or(wanted), fields.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_references() {
        assert!(good_name("GH_TOKEN") && good_name("_x1") && !good_name("1X") && !good_name("A-B") && !good_name(""));
        assert!(good_reference("op://Bots/GitHub/password") && good_reference("op://Bots/My Item/section/field"));
        assert!(!good_reference("op://Bots/GitHub") && !good_reference("https://x/y/z") && !good_reference("op://a/b/c\nrm"));
    }

    #[test]
    fn values_are_blanked() {
        let s = vec![("GH".to_string(), "hunter2-secret".to_string()), ("PIN".to_string(), "12".to_string())];
        assert_eq!(scrub("token=hunter2-secret ok 12", &s), "token=[secret GH] ok 12");
        assert_eq!(scrub("aHVudGVyMi1zZWNyZXQ=", &s), "[secret GH]");
        assert_eq!(ends("ops_abcdef"), "cdef");
    }
}
