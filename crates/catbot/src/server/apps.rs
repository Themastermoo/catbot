//! Connected apps (stage 16). 16.1 is MCP servers: programs that give bots tools, run on this
//! computer (a command) or reached at a link. Catbot itself talks to each server and hands its
//! tools to the bots the person ticked, through the same road as email and pages, so the same
//! rules hold on every AI.
//!
//! Moo's choices (2026-11-17): MCP servers and Composio both (Composio in 16.2); the person ticks
//! which bots get each app; reading is free and anything that writes asks first. A tool counts as
//! reading only when its server marks it read-only (`readOnlyHint`); any other call shows a card
//! with exactly what would be done, and runs when the person approves it.
//!
//! Kept in the settings as `apps`. A server's keys (header and environment values) never go to a
//! bot, and come back to the app only as their last four characters.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use super::bot_tools::Grant;
use super::{Answer, App, bad};
use crate::store::Role;

const SETTING: &str = "apps";
const PROTOCOL: &str = "2025-06-18";
/// What a bot is shown of a tool's answer.
const MOST_ANSWER: usize = 30_000;

/// A name and value: a header, or an environment variable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub name: String,
    pub value: String,
}

/// One of a server's tools, as it described it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// The server says it only reads.
    pub read_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// "http" (a link) or "command".
    pub transport: String,
    pub url: String,
    pub headers: Vec<Pair>,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<Pair>,
    pub enabled: bool,
    /// The bots that get its tools (none until the person ticks them).
    pub bots: Vec<String>,
    /// What it offered when last asked.
    pub tools: Vec<Tool>,
    pub tested_at: Option<String>,
    /// Why the last try failed, in words.
    pub problem: Option<String>,
}

pub fn servers(app: &App) -> Vec<Server> {
    app.with_store(|store| Ok(store.setting(SETTING)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())).unwrap_or_default()
}

fn keep(app: &App, list: &[Server]) {
    app.with_store(|store| store.set_setting(SETTING, &json!(list)));
    app.hub.send(json!({ "kind": "apps" }));
}

/// A short name for tool names: "GitHub tools" → "github_tools".
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('_') && !s.is_empty() {
            s.push('_');
        }
    }
    let s: String = s.trim_matches('_').chars().take(20).collect();
    if s.is_empty() { "app".into() } else { s }
}

/// The name a bot calls a server's tool by: `<server>__<tool>`, at most 56 characters (Claude
/// adds `mcp__catbot__`), letters, digits, `_` and `-` only.
fn tool_name(server: &Server, tool: &str) -> String {
    let clean: String = tool.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let full = format!("{}__{clean}", slug(&server.name));
    if full.len() <= 56 {
        return full;
    }
    // Too long: cut, with a short mark of the whole so two long names stay apart.
    let mark = format!("{:08x}", full.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))));
    format!("{}_{mark}", &full[..47])
}

// ---- Talking to a server ---------------------------------------------------------------------

/// A server being talked to: a running command, or a link with its session. There's one per
/// server, so the size of the larger kind doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Link {
    Command { child: tokio::process::Child, stdin: tokio::process::ChildStdin, out: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>, next: u64 },
    Http { url: String, headers: Vec<Pair>, session: Option<String>, next: u64 },
}

/// Open links, by server id: a command keeps running between calls.
static LINKS: LazyLock<std::sync::Mutex<HashMap<String, Arc<Mutex<Link>>>>> = LazyLock::new(Default::default);

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(120)).user_agent(concat!("Catbot/", env!("CARGO_PKG_VERSION"))).build().unwrap_or_default()
}

/// Forgets a server's link (it changed, or was turned off or removed): a command stops.
pub fn close(id: &str) {
    LINKS.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
}

impl Link {
    async fn start(server: &Server) -> Result<Link, String> {
        match server.transport.as_str() {
            "command" => {
                if server.command.trim().is_empty() {
                    return Err("which command runs it?".into());
                }
                let mut run = tokio::process::Command::new(server.command.trim());
                run.args(&server.args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
                for pair in &server.env {
                    run.env(&pair.name, &pair.value);
                }
                let mut child = run.spawn().map_err(|e| format!("couldn't start {}: {e}", server.command.trim()))?;
                let stdin = child.stdin.take().ok_or("no input to the command")?;
                let out = BufReader::new(child.stdout.take().ok_or("no output from the command")?).lines();
                Ok(Link::Command { child, stdin, out, next: 1 })
            }
            _ => {
                let url = server.url.trim();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err("a link starts with https://".into());
                }
                Ok(Link::Http { url: url.to_string(), headers: server.headers.clone(), session: None, next: 1 })
            }
        }
    }

    /// Sends a request and waits for its answer; `None` for a notification, which gets none.
    async fn send(&mut self, method: &str, params: Value, notify: bool) -> Result<Value, String> {
        match self {
            Link::Command { child, stdin, out, next } => {
                let id = *next;
                *next += 1;
                let msg = if notify { json!({ "jsonrpc": "2.0", "method": method, "params": params }) } else { json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }) };
                let mut line = msg.to_string();
                line.push('\n');
                stdin.write_all(line.as_bytes()).await.map_err(|e| format!("the server stopped: {e}"))?;
                stdin.flush().await.map_err(|e| format!("the server stopped: {e}"))?;
                if notify {
                    return Ok(Value::Null);
                }
                let wait = if method == "tools/call" { 300 } else { 60 };
                let answer = tokio::time::timeout(Duration::from_secs(wait), async {
                    loop {
                        let Some(line) = out.next_line().await.map_err(|e| e.to_string())? else {
                            let ended = child.try_wait().ok().flatten().map(|s| format!(" ({s})")).unwrap_or_default();
                            return Err(format!("the server stopped{ended}"));
                        };
                        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                        if msg["id"] == json!(id) && msg.get("method").is_none() {
                            return Ok(msg);
                        }
                        // A request from the server (a ping, asking for roots): answer simply.
                        if msg.get("method").is_some() && !msg["id"].is_null() {
                            let reply = if msg["method"] == "ping" { json!({ "jsonrpc": "2.0", "id": msg["id"], "result": {} }) } else { json!({ "jsonrpc": "2.0", "id": msg["id"], "error": { "code": -32601, "message": "Catbot doesn't offer that" } }) };
                            let _ = stdin.write_all(format!("{reply}\n").as_bytes()).await;
                        }
                    }
                })
                .await
                .map_err(|_| format!("the server didn't answer within {wait} seconds"))??;
                Ok(answer)
            }
            Link::Http { url, headers, session, next } => {
                let id = *next;
                *next += 1;
                let msg = if notify { json!({ "jsonrpc": "2.0", "method": method, "params": params }) } else { json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }) };
                let mut req = http().post(url.as_str()).header("accept", "application/json, text/event-stream").header("content-type", "application/json").header("mcp-protocol-version", PROTOCOL);
                if let Some(s) = session.as_deref() {
                    req = req.header("mcp-session-id", s);
                }
                for pair in headers.iter() {
                    req = req.header(pair.name.as_str(), pair.value.as_str());
                }
                let resp = req.body(msg.to_string()).send().await.map_err(|e| format!("couldn't reach it: {e}"))?;
                let status = resp.status();
                if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
                    *session = Some(s.to_string());
                }
                if status.as_u16() == 401 || status.as_u16() == 403 {
                    return Err(format!("it refused the keys ({status}); check the headers"));
                }
                if !status.is_success() {
                    let text = resp.text().await.unwrap_or_default();
                    return Err(format!("it answered {status}: {}", text.chars().take(200).collect::<String>()));
                }
                if notify {
                    return Ok(Value::Null);
                }
                let streamed = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/event-stream"));
                let text = resp.text().await.map_err(|e| format!("its answer was cut off: {e}"))?;
                if !streamed {
                    return serde_json::from_str(&text).map_err(|_| "its answer wasn't JSON".to_string());
                }
                // Server-sent events: the one that answers this request.
                text.split("\n\n")
                    .filter_map(|event| {
                        let data: Vec<&str> = event.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim_start).collect();
                        serde_json::from_str::<Value>(&data.join("\n")).ok()
                    })
                    .find(|m| m["id"] == json!(id))
                    .ok_or_else(|| "its answer never came".to_string())
            }
        }
    }

    /// A request's result, or its error in words.
    async fn ask(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let answer = self.send(method, params, false).await?;
        if let Some(error) = answer.get("error") {
            return Err(error["message"].as_str().unwrap_or("it refused").to_string());
        }
        Ok(answer["result"].clone())
    }

    async fn handshake(&mut self) -> Result<(), String> {
        self.ask("initialize", json!({ "protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": { "name": "catbot", "version": env!("CARGO_PKG_VERSION") } })).await?;
        self.send("notifications/initialized", json!({}), true).await?;
        Ok(())
    }
}

/// The server's open link, started (and greeted) if it isn't.
async fn link(server: &Server) -> Result<Arc<Mutex<Link>>, String> {
    if let Some(open) = LINKS.lock().unwrap_or_else(|p| p.into_inner()).get(&server.id).cloned() {
        return Ok(open);
    }
    let mut fresh = Link::start(server).await?;
    fresh.handshake().await?;
    let open = Arc::new(Mutex::new(fresh));
    LINKS.lock().unwrap_or_else(|p| p.into_inner()).insert(server.id.clone(), open.clone());
    Ok(open)
}

/// A request on the server's link; a link that broke is started again once.
async fn request(server: &Server, method: &str, params: Value) -> Result<Value, String> {
    let open = link(server).await?;
    let first = open.lock().await.ask(method, params.clone()).await;
    match first {
        Err(why) if why.starts_with("the server stopped") || why.contains("session") => {
            close(&server.id);
            let again = link(server).await?;
            again.lock().await.ask(method, params).await
        }
        other => other,
    }
}

/// Every tool the server offers.
async fn list_tools(server: &Server) -> Result<Vec<Tool>, String> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let params = match &cursor {
            Some(c) => json!({ "cursor": c }),
            None => json!({}),
        };
        let page = request(server, "tools/list", params).await?;
        for t in page["tools"].as_array().cloned().unwrap_or_default() {
            tools.push(Tool {
                name: t["name"].as_str().unwrap_or_default().to_string(),
                description: t["description"].as_str().unwrap_or_default().chars().take(1000).collect(),
                input_schema: if t["inputSchema"].is_object() { t["inputSchema"].clone() } else { json!({ "type": "object" }) },
                read_only: t["annotations"]["readOnlyHint"] == true,
            });
        }
        cursor = page["nextCursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

/// A tool's answer as text for a bot: its text, and a note for anything else.
fn answer_text(result: &Value) -> (String, bool) {
    let mut out = Vec::new();
    for item in result["content"].as_array().cloned().unwrap_or_default() {
        match item["type"].as_str() {
            Some("text") => out.push(item["text"].as_str().unwrap_or_default().to_string()),
            Some("resource") => out.push(item["resource"]["text"].as_str().map(str::to_string).unwrap_or_else(|| format!("(a resource: {})", item["resource"]["uri"].as_str().unwrap_or("?")))),
            Some("resource_link") => out.push(format!("(a link: {})", item["uri"].as_str().unwrap_or("?"))),
            Some(other) => out.push(format!("({other} left out)")),
            None => {}
        }
    }
    if out.is_empty() && !result["structuredContent"].is_null() {
        out.push(result["structuredContent"].to_string());
    }
    let mut text = out.join("\n");
    if text.chars().count() > MOST_ANSWER {
        text = text.chars().take(MOST_ANSWER).collect::<String>() + "\n(…cut short)";
    }
    (text, result["isError"] == true)
}

async fn call_tool(server: &Server, tool: &str, args: &Value) -> Result<String, String> {
    let result = request(server, "tools/call", json!({ "name": tool, "arguments": args })).await?;
    let (text, failed) = answer_text(&result);
    if failed { Err(text) } else { Ok(text) }
}

/// Starts a server and asks what it offers: kept on it, with any problem in words.
pub async fn test(app: &App, id: &str) -> Result<Server, String> {
    let server = servers(app).into_iter().find(|s| s.id == id).ok_or("no such app")?;
    close(id);
    let found = list_tools(&server).await;
    if found.is_err() {
        close(id);
    }
    let mut all = servers(app);
    let s = all.iter_mut().find(|s| s.id == id).ok_or("no such app")?;
    s.tested_at = Some(chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false));
    match found {
        Ok(tools) => {
            s.tools = tools;
            s.problem = None;
        }
        Err(why) => s.problem = Some(why),
    }
    let tested = s.clone();
    keep(app, &all);
    Ok(tested)
}

// ---- The bots' side --------------------------------------------------------------------------

/// The tools a bot gets from the apps it's ticked for.
pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for server in servers(app).into_iter().filter(|s| s.enabled && s.problem.is_none() && s.bots.iter().any(|b| b == bot_id)) {
        for tool in &server.tools {
            let asks = if tool.read_only { "" } else { " (The person approves each use of this tool first.)" };
            out.push(json!({
                "name": tool_name(&server, &tool.name),
                "description": format!("[{}] {}{asks}", server.name, tool.description),
                "inputSchema": tool.input_schema,
            }));
        }
    }
    out
}

/// The server and tool a bot's tool name stands for, if the bot may use it.
fn find(app: &App, bot_id: &str, name: &str) -> Option<(Server, Tool)> {
    servers(app).into_iter().filter(|s| s.enabled && s.bots.iter().any(|b| b == bot_id)).find_map(|s| {
        let tool = s.tools.iter().find(|t| tool_name(&s, &t.name) == name)?.clone();
        Some((s, tool))
    })
}

pub fn knows(app: &App, bot_id: &str, name: &str) -> bool {
    find(app, bot_id, name).is_some()
}

/// A bot's call to an app's tool: a read runs now; anything else waits for the person's yes.
pub async fn call(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Result<String, String> {
    let (server, tool) = find(app, &grant.bot_id, name).ok_or_else(|| format!("there's no app tool {name} for you"))?;
    if tool.read_only {
        return call_tool(&server, &tool.name, args).await;
    }
    let bot = app.with_store(|store| store.bot_by_id(&grant.bot_id)).flatten().ok_or("the bot is gone")?;
    let shown = serde_json::to_string_pretty(args).unwrap_or_default();
    let shown: String = if shown.chars().count() > 4000 { shown.chars().take(4000).collect::<String>() + "\n…" } else { shown };
    let mut lines = vec![format!("App: {}", server.name), format!("Tool: {}", tool.name)];
    if !tool.description.is_empty() {
        lines.push(tool.description.chars().take(300).collect());
    }
    lines.push(shown);
    let detail = json!({
        "kind": "team-change",
        "heading": format!("{} wants to use {}", bot.name, server.name),
        "askedBy": bot.name,
        "title": format!("{} › {}", server.name, tool.name),
        "lines": lines,
        "action": "app_call",
        "target": bot.id,
        "body": { "server": server.id, "tool": tool.name, "arguments": args },
        "state": "asking",
    });
    super::turns::save_and_send(app, &grant.thread, Role::Step, &format!("{}: {}", server.name, tool.name), Some(&detail));
    Ok("It's waiting for the person's approval in the chat; they see exactly what you asked for. You'll hear how it went with their next message. Don't ask again.".into())
}

/// Runs an approved app call (`team.rs`); what it answered, shortened, becomes the card's note.
pub async fn run_approved(app: &App, body: &Value) -> Result<String, String> {
    if body["composio"] == true {
        return super::composio::run_approved(app, body).await;
    }
    if body["blender"] == true {
        return super::blender::run_approved(app, body).await;
    }
    let id = body["server"].as_str().unwrap_or_default();
    let server = servers(app).into_iter().find(|s| s.id == id).ok_or("that app was removed")?;
    if !server.enabled {
        return Err(format!("{} is turned off", server.name));
    }
    let tool = body["tool"].as_str().unwrap_or_default();
    let answer = call_tool(&server, tool, &body["arguments"]).await?;
    let short: String = answer.chars().take(1500).collect();
    Ok(if short.is_empty() { "Done.".into() } else { format!("Done. It answered: {short}") })
}

// ---- The person's side: Settings › Apps ------------------------------------------------------

/// A secret value as the app sees it: its last four characters.
fn masked(value: &str) -> String {
    if value.chars().count() <= 4 {
        return "••••".into();
    }
    let tail: String = value.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("••••{tail}")
}

fn shown(app: &App, s: &Server) -> Value {
    let names: Vec<String> = s.bots.iter().filter_map(|id| app.with_store(|store| store.bot_by_id(id)).flatten().map(|b| b.name)).collect();
    json!({
        "id": s.id,
        "name": s.name,
        "transport": s.transport,
        "url": s.url,
        "headers": s.headers.iter().map(|p| json!({ "name": p.name, "value": masked(&p.value) })).collect::<Vec<_>>(),
        "command": s.command,
        "args": s.args,
        "env": s.env.iter().map(|p| json!({ "name": p.name, "value": masked(&p.value) })).collect::<Vec<_>>(),
        "enabled": s.enabled,
        "bots": s.bots,
        "botNames": names,
        "tools": s.tools.iter().map(|t| json!({ "name": t.name, "description": t.description, "readOnly": t.read_only })).collect::<Vec<_>>(),
        "testedAt": s.tested_at,
        "problem": s.problem,
    })
}

/// Name/value rows from the app. A value still shown masked keeps what was saved under that name.
fn pairs(given: &Value, before: &[Pair]) -> Result<Vec<Pair>, String> {
    let mut out = Vec::new();
    for row in given.as_array().cloned().unwrap_or_default() {
        let name = row["name"].as_str().unwrap_or_default().trim().to_string();
        let value = row["value"].as_str().unwrap_or_default().to_string();
        if name.is_empty() {
            continue;
        }
        if name.chars().any(|c| c.is_whitespace() || c == ':' || c == '=') {
            return Err(format!("{name:?} isn't a name a header or variable can have"));
        }
        let value = if value.starts_with("••••") { before.iter().find(|p| p.name == name).map(|p| p.value.clone()).unwrap_or_default() } else { value };
        out.push(Pair { name, value });
    }
    Ok(out)
}

/// Takes what `body` gives: name, how it's reached, its keys.
fn fill(s: &mut Server, body: &Value) -> Result<(), String> {
    if let Some(name) = body["name"].as_str() {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 {
            return Err("a name of 1 to 40 characters".into());
        }
        s.name = name.to_string();
    }
    if let Some(t) = body["transport"].as_str() {
        if t != "http" && t != "command" {
            return Err("an app is reached at a link (http) or run as a command".into());
        }
        s.transport = t.to_string();
    }
    if let Some(url) = body["url"].as_str() {
        s.url = url.trim().to_string();
    }
    if !body["headers"].is_null() {
        s.headers = pairs(&body["headers"], &s.headers)?;
    }
    if let Some(command) = body["command"].as_str() {
        s.command = command.trim().to_string();
    }
    if let Some(args) = body["args"].as_array() {
        s.args = args.iter().filter_map(|a| a.as_str().map(str::to_string)).filter(|a| !a.is_empty()).collect();
    }
    if !body["env"].is_null() {
        s.env = pairs(&body["env"], &s.env)?;
    }
    if let Some(bots) = body["bots"].as_array() {
        s.bots = bots.iter().filter_map(|b| b.as_str().map(str::to_string)).collect();
    }
    if let Some(on) = body["enabled"].as_bool() {
        s.enabled = on;
    }
    if s.name.is_empty() {
        return Err("a name for it".into());
    }
    match s.transport.as_str() {
        "command" if s.command.is_empty() => Err("the command that runs it".into()),
        "http" if !(s.url.starts_with("https://") || s.url.starts_with("http://")) => Err("its link, starting https://".into()),
        _ => Ok(()),
    }
}

/// `GET /api/apps`, `POST /api/apps` (add, then test), `PUT`/`DELETE /api/apps/:id`,
/// `POST /api/apps/:id/test`.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or_default();
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, json!({ "servers": servers(app).iter().map(|s| shown(app, s)).collect::<Vec<_>>() }))),
        (&Method::POST, []) => {
            let mut s = Server { id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(), transport: "http".into(), ..Server::default() };
            fill(&mut s, &body).map_err(bad)?;
            if servers(app).iter().any(|other| slug(&other.name) == slug(&s.name)) {
                return Err(bad("there's an app by that name already"));
            }
            let mut all = servers(app);
            all.push(s.clone());
            keep(app, &all);
            // Tested at once: on when it answers, off with the reason when it doesn't.
            let tested = test(app, &s.id).await.map_err(bad)?;
            let on = tested.problem.is_none();
            let mut all = servers(app);
            if let Some(x) = all.iter_mut().find(|x| x.id == s.id) {
                x.enabled = on;
            }
            keep(app, &all);
            let s = servers(app).into_iter().find(|x| x.id == s.id).ok_or_else(|| bad("it vanished"))?;
            Ok((StatusCode::CREATED, json!({ "server": shown(app, &s) })))
        }
        (&Method::PUT, [id]) => {
            let mut all = servers(app);
            let s = all.iter_mut().find(|s| s.id == *id).ok_or_else(|| bad("no such app"))?;
            let before = s.clone();
            let mut next = s.clone();
            fill(&mut next, &body).map_err(bad)?;
            *s = next.clone();
            keep(app, &all);
            // How it's reached changed: start it afresh and ask again.
            let reached = |x: &Server| (x.transport.clone(), x.url.clone(), x.headers.clone(), x.command.clone(), x.args.clone(), x.env.clone());
            if reached(&before) != reached(&next) {
                test(app, id).await.map_err(bad)?;
            } else if !next.enabled {
                close(id);
            }
            let s = servers(app).into_iter().find(|x| x.id == *id).ok_or_else(|| bad("it vanished"))?;
            Ok((StatusCode::OK, json!({ "server": shown(app, &s) })))
        }
        (&Method::POST, [id, "test"]) => {
            let s = test(app, id).await.map_err(bad)?;
            Ok((StatusCode::OK, json!({ "server": shown(app, &s) })))
        }
        (&Method::DELETE, [id]) => {
            let mut all = servers(app);
            let before = all.len();
            all.retain(|s| s.id != *id);
            if all.len() == before {
                return Err(bad("no such app"));
            }
            close(id);
            keep(app, &all);
            Ok((StatusCode::OK, json!({ "deleted": id })))
        }
        _ => Err(bad("no such apps request")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_safe_and_stay_apart() {
        let s = Server { name: "GitHub (work)".into(), ..Server::default() };
        assert_eq!(tool_name(&s, "create_issue"), "github_work__create_issue");
        assert_eq!(tool_name(&s, "get.thing/x"), "github_work__get_thing_x");
        let long_a = tool_name(&s, &"a".repeat(80));
        let long_b = tool_name(&s, &format!("{}b", "a".repeat(79)));
        assert!(long_a.len() <= 56 && long_b.len() <= 56 && long_a != long_b);
    }

    #[test]
    fn keys_go_back_masked_and_masked_values_keep_the_saved_one() {
        assert_eq!(masked("Bearer abcdef1234"), "••••1234");
        let before = vec![Pair { name: "Authorization".into(), value: "Bearer secret9876".into() }];
        let kept = pairs(&json!([{ "name": "Authorization", "value": "••••9876" }, { "name": "X-New", "value": "v" }]), &before).unwrap();
        assert_eq!(kept[0].value, "Bearer secret9876");
        assert_eq!(kept[1].value, "v");
        assert!(pairs(&json!([{ "name": "Bad Name", "value": "x" }]), &[]).is_err());
    }

    #[test]
    fn answers_become_text() {
        let (text, failed) = answer_text(&json!({ "content": [{ "type": "text", "text": "hi" }, { "type": "image", "data": "..." }], "isError": false }));
        assert_eq!((text.as_str(), failed), ("hi\n(image left out)", false));
        assert!(answer_text(&json!({ "content": [], "isError": true })).1);
    }
}
