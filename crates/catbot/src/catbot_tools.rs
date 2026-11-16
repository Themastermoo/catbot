//! Catbot's own tools for bots (stage 13.9): things only Catbot can do for a bot, such as
//! reading the person's mailbox. `catbot serve` serves them at `/api/bot-tools`, reached with a
//! key made for one turn. Claude Code and agents get them through the MCP helper; bots on a
//! chat API, through their own tool list.

use serde_json::{Value, json};

/// The header that carries a turn's key.
pub const KEY_HEADER: &str = "x-catbot-tools";

/// Where a turn reaches its tools, and its key.
#[derive(Debug, Clone, PartialEq)]
pub struct Access {
    pub url: String,
    pub key: String,
}

impl Access {
    /// From the helper's environment (`CATBOT_TOOLS_URL`, `CATBOT_TOOLS_KEY`).
    pub fn from_env() -> Option<Access> {
        let url = std::env::var("CATBOT_TOOLS_URL").ok().filter(|u| !u.is_empty())?;
        let key = std::env::var("CATBOT_TOOLS_KEY").ok().filter(|k| !k.is_empty())?;
        Some(Access { url, key })
    }

    /// The environment that hands the access to the helper.
    pub fn env(&self) -> [(&'static str, String); 2] {
        [("CATBOT_TOOLS_URL", self.url.clone()), ("CATBOT_TOOLS_KEY", self.key.clone())]
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(180)).build().unwrap_or_default()
}

/// The tools, as MCP lists them: `{ name, description, inputSchema }`.
pub async fn list(access: &Access) -> Vec<Value> {
    let Ok(answer) = client().get(format!("{}/api/bot-tools", access.url)).header(KEY_HEADER, &access.key).send().await else { return Vec::new() };
    let Ok(body) = answer.json::<Value>().await else { return Vec::new() };
    body["tools"].as_array().cloned().unwrap_or_default()
}

/// Calls one: its text answer, or what went wrong (told to the bot).
pub async fn call(access: &Access, name: &str, arguments: &Value) -> Result<String, String> {
    let answer = client()
        .post(format!("{}/api/bot-tools/call", access.url))
        .header(KEY_HEADER, &access.key)
        .json(&json!({ "name": name, "arguments": arguments }))
        .send()
        .await
        .map_err(|e| format!("Catbot didn't answer: {e}"))?;
    let body: Value = answer.json().await.map_err(|e| format!("Catbot's answer didn't read: {e}"))?;
    let text = body["text"].as_str().or(body["error"].as_str()).unwrap_or_default().to_string();
    if body["isError"] == true || body.get("error").is_some() { Err(text) } else { Ok(text) }
}

/// An MCP tool as a chat API's function.
pub fn as_function(tool: &Value) -> Value {
    json!({ "type": "function", "function": { "name": tool["name"], "description": tool["description"], "parameters": tool["inputSchema"] } })
}
