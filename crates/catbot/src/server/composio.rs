//! Connected apps through Composio (stage 16.2): Gmail, Outlook, Google Calendar, Slack, GitHub
//! and hundreds more, signed in through the person's browser, with Moo's own Composio project key.
//! Composio keeps the apps' sign-ins; Catbot keeps only the key, which never goes to a bot.
//!
//! Moo's choices (2026-11-17) hold here as for MCP servers: he ticks which bots get each app, and
//! reading is free while anything that writes asks first. Bots get three tools of Catbot's own
//! (find, look at, run), so Catbot sees every tool before it runs. A tool counts as reading only
//! when Composio tags it read-only, or its name's verb reads (GET, LIST, FETCH, SEARCH, …), as
//! MausBot decides; anything else shows the person a card first.
//!
//! Kept in the settings as `composio`. `CATBOT_COMPOSIO_API` stands in for Composio in tests.

use std::sync::Arc;
use std::time::Duration;

use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bot_tools::Grant;
use super::{Answer, App, bad};
use crate::store::Role;

const SETTING: &str = "composio";
const READ_VERBS: [&str; 12] = ["GET", "LIST", "FETCH", "SEARCH", "READ", "FIND", "RETRIEVE", "LOOKUP", "DOWNLOAD", "VIEW", "CHECK", "COUNT"];

/// An app connected through Composio, and the bots that get it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComposioApp {
    /// Composio's toolkit slug: "gmail", "outlook", "googlecalendar".
    pub toolkit: String,
    pub name: String,
    pub bots: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Composio {
    pub key: String,
    /// Catbot's user in the Composio project: the apps' accounts belong to it.
    pub user_id: String,
    pub session_id: String,
    pub apps: Vec<ComposioApp>,
}

fn api() -> String {
    std::env::var("CATBOT_COMPOSIO_API").unwrap_or_else(|_| "https://backend.composio.dev".into())
}

pub fn settings(app: &App) -> Composio {
    app.with_store(|store| Ok(store.setting(SETTING)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())).unwrap_or_default()
}

fn keep(app: &App, c: &Composio) {
    app.with_store(|store| store.set_setting(SETTING, &json!(c)));
    app.hub.send(json!({ "kind": "apps" }));
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(120)).user_agent(concat!("Catbot/", env!("CARGO_PKG_VERSION"))).build().unwrap_or_default()
}

/// Composio's answer, or what went wrong in words.
async fn call(key: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let mut req = client().request(method, format!("{}{path}", api())).header("x-api-key", key);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req.send().await.map_err(|e| format!("couldn't reach Composio: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let found: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("Composio didn't accept the key (it's the project key from API Keys, starting ak_)".into());
    }
    if !status.is_success() {
        let said = found["error"]["message"].as_str().or(found["message"].as_str()).map(str::to_string).unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("Composio said {status}: {said}"));
    }
    Ok(found)
}

/// Whether a tool only reads: tagged so, or its verb reads.
pub fn reads(slug: &str, toolkit: &str, tags: &[String]) -> bool {
    if tags.iter().any(|t| t == "readOnlyHint") {
        return true;
    }
    if tags.iter().any(|t| t == "destructiveHint") {
        return false;
    }
    let action = slug.strip_prefix(&format!("{}_", toolkit.to_uppercase())).unwrap_or_else(|| slug.split_once('_').map(|(_, a)| a).unwrap_or(""));
    action.split('_').next().is_some_and(|verb| READ_VERBS.contains(&verb))
}

/// A tool as Composio describes it: its app, whether it reads, its inputs.
struct ToolInfo {
    toolkit: String,
    description: String,
    reads: bool,
    inputs: Value,
}

async fn tool_info(key: &str, slug: &str) -> Result<ToolInfo, String> {
    let found = call(key, reqwest::Method::GET, &format!("/api/v3.1/tools?tool_slugs={slug}"), None).await?;
    let item = found["items"].as_array().and_then(|i| i.iter().find(|t| t["slug"] == slug)).cloned().ok_or_else(|| format!("Composio has no tool {slug}; find one with app_tools_find"))?;
    let toolkit = item["toolkit"]["slug"].as_str().unwrap_or_default().to_string();
    let tags: Vec<String> = item["tags"].as_array().map(|t| t.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    Ok(ToolInfo { reads: reads(slug, &toolkit, &tags), toolkit, description: item["description"].as_str().unwrap_or_default().to_string(), inputs: item["input_parameters"].clone() })
}

/// The accounts connected to Catbot's Composio user, newest first.
async fn accounts(c: &Composio) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let mut path = format!("/api/v3/connected_accounts?user_ids={}&limit=100", c.user_id);
        if let Some(cur) = &cursor {
            path.push_str(&format!("&cursor={cur}"));
        }
        let page = call(&c.key, reqwest::Method::GET, &path, None).await?;
        out.extend(page["items"].as_array().cloned().unwrap_or_default());
        cursor = page["next_cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

// ---- The bots' side --------------------------------------------------------------------------

/// The apps a bot is ticked for.
pub(super) fn apps_of(c: &Composio, bot_id: &str) -> Vec<ComposioApp> {
    c.apps.iter().filter(|a| a.bots.iter().any(|b| b == bot_id)).cloned().collect()
}

pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    let c = settings(app);
    if c.key.is_empty() {
        return Vec::new();
    }
    let mine = apps_of(&c, bot_id);
    if mine.is_empty() {
        return Vec::new();
    }
    let names = mine.iter().map(|a| format!("{} ({})", a.name, a.toolkit)).collect::<Vec<_>>().join(", ");
    vec![
        json!({
            "name": "app_tools_find",
            "description": format!("Find the tools of a connected app you may use: {names}. Give app (its id in brackets) and what you want to do as query. Each tool says whether it only reads (runs at once) or asks the person first."),
            "inputSchema": { "type": "object", "properties": {
                "app": { "type": "string" },
                "query": { "type": "string", "description": "What you want to do, in a few words." },
            }, "required": ["app"] },
        }),
        json!({
            "name": "app_tool_inputs",
            "description": "The inputs a connected app's tool takes (from app_tools_find), as a JSON schema.",
            "inputSchema": { "type": "object", "properties": { "tool": { "type": "string" } }, "required": ["tool"] },
        }),
        json!({
            "name": "app_events_find",
            "description": "The events a connected app of yours can start a trigger on (a new email, a new Slack message, a new GitHub issue), with the settings each takes. Set one up with the trigger tool (when: \"app\").",
            "inputSchema": { "type": "object", "properties": { "app": { "type": "string" } }, "required": ["app"] },
        }),
        json!({
            "name": "app_tool_run",
            "description": "Run a connected app's tool with its inputs. One that only reads runs at once; anything else shows the person exactly what you asked for, and runs when they approve it (you hear how it went with their next message). If the app has several accounts, name one as account.",
            "inputSchema": { "type": "object", "properties": {
                "tool": { "type": "string" },
                "arguments": { "type": "object" },
                "account": { "type": "string", "description": "Which account, by its label, when the app has several." },
            }, "required": ["tool", "arguments"] },
        }),
    ]
}

pub fn knows(name: &str) -> bool {
    matches!(name, "app_tools_find" | "app_tool_inputs" | "app_tool_run" | "app_events_find")
}

/// The account to run a tool as: the one named, or the app's only active one.
pub(super) async fn account_for(c: &Composio, toolkit: &str, named: Option<&str>) -> Result<String, String> {
    let all = accounts(c).await?;
    let active: Vec<&Value> = all.iter().filter(|a| a["toolkit"]["slug"] == toolkit && a["status"] == "ACTIVE").collect();
    if let Some(named) = named.filter(|n| !n.trim().is_empty()) {
        return active.iter().find(|a| a["alias"] == named || a["id"] == named).and_then(|a| a["id"].as_str()).map(str::to_string).ok_or_else(|| format!("{toolkit} has no working account {named:?}"));
    }
    match active.as_slice() {
        [] => Err(format!("{toolkit} isn't connected (or its sign-in ran out); ask the person to connect it in Settings › Apps")),
        [one] => Ok(one["id"].as_str().unwrap_or_default().to_string()),
        several => Err(format!(
            "{toolkit} has several accounts; name one as account: {}",
            several.iter().map(|a| a["alias"].as_str().unwrap_or_else(|| a["id"].as_str().unwrap_or("?")).to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

async fn execute(c: &Composio, slug: &str, arguments: &Value, account: &str) -> Result<String, String> {
    let done = call(&c.key, reqwest::Method::POST, &format!("/api/v3.1/tools/execute/{slug}"), Some(json!({ "user_id": c.user_id, "connected_account_id": account, "arguments": arguments }))).await?;
    if done["successful"] != true {
        return Err(done["error"].as_str().unwrap_or("it failed").to_string());
    }
    let text = serde_json::to_string_pretty(&done["data"]).unwrap_or_default();
    Ok(if text.chars().count() > 30_000 { text.chars().take(30_000).collect::<String>() + "\n(…cut short)" } else { text })
}

pub async fn call_tool(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Result<String, String> {
    let c = settings(app);
    if c.key.is_empty() {
        return Err("Composio isn't set up".into());
    }
    let mine = apps_of(&c, &grant.bot_id);
    let allowed = |toolkit: &str| mine.iter().any(|a| a.toolkit == toolkit);
    match name {
        "app_tools_find" => {
            let toolkit = args["app"].as_str().unwrap_or_default().trim().to_lowercase();
            if !allowed(&toolkit) {
                return Err(format!("you may not use {toolkit:?}; yours are {}", mine.iter().map(|a| a.toolkit.as_str()).collect::<Vec<_>>().join(", ")));
            }
            let mut path = format!("/api/v3.1/tools?toolkit_slug={toolkit}&limit=25");
            if let Some(q) = args["query"].as_str().filter(|q| !q.trim().is_empty()) {
                path.push_str(&format!("&query={}", q.trim().replace(' ', "+").replace('&', "")));
            }
            let found = call(&c.key, reqwest::Method::GET, &path, None).await?;
            let lines: Vec<String> = found["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|t| {
                    let slug = t["slug"].as_str().unwrap_or_default();
                    let tags: Vec<String> = t["tags"].as_array().map(|x| x.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                    let how = if reads(slug, &toolkit, &tags) { "reads" } else { "asks first" };
                    let what: String = t["description"].as_str().unwrap_or_default().chars().take(200).collect();
                    format!("{slug} ({how}): {what}")
                })
                .collect();
            let accounts = accounts(&c).await.unwrap_or_default();
            let labels: Vec<String> = accounts.iter().filter(|a| a["toolkit"]["slug"] == toolkit.as_str() && a["status"] == "ACTIVE").map(|a| a["alias"].as_str().unwrap_or("(no label)").to_string()).collect();
            let account_line = if labels.len() > 1 { format!("\nAccounts (name one as account): {}", labels.join(", ")) } else { String::new() };
            if lines.is_empty() {
                return Ok(format!("No {toolkit} tools match.{account_line}"));
            }
            Ok(format!("{}{account_line}\napp_tool_inputs shows what one takes.", lines.join("\n")))
        }
        "app_events_find" => {
            let toolkit = args["app"].as_str().unwrap_or_default().trim().to_lowercase();
            if !allowed(&toolkit) {
                return Err(format!("you may not use {toolkit:?}"));
            }
            let events = event_types(&c, &toolkit).await?;
            if events.is_empty() {
                return Ok(format!("{toolkit} has no events to start a trigger on."));
            }
            Ok(events.iter().map(|e| format!("{} ({}): {}\nsettings: {}", e["event"].as_str().unwrap_or_default(), e["name"].as_str().unwrap_or_default(), e["description"].as_str().unwrap_or_default(), e["config"])).collect::<Vec<_>>().join("\n\n"))
        }
        "app_tool_inputs" => {
            let slug = args["tool"].as_str().unwrap_or_default().trim().to_string();
            let info = tool_info(&c.key, &slug).await?;
            if !allowed(&info.toolkit) {
                return Err(format!("you may not use {}", info.toolkit));
            }
            Ok(format!("{slug} ({}): {}\n{}", if info.reads { "reads" } else { "asks first" }, info.description, serde_json::to_string_pretty(&info.inputs).unwrap_or_default()))
        }
        "app_tool_run" => {
            let slug = args["tool"].as_str().unwrap_or_default().trim().to_string();
            let arguments = if args["arguments"].is_object() { args["arguments"].clone() } else { json!({}) };
            let info = tool_info(&c.key, &slug).await?;
            let Some(app_named) = mine.iter().find(|a| a.toolkit == info.toolkit).cloned() else {
                return Err(format!("you may not use {}", info.toolkit));
            };
            let account = account_for(&c, &info.toolkit, args["account"].as_str()).await?;
            if info.reads {
                return execute(&c, &slug, &arguments, &account).await;
            }
            let bot = app.with_store(|store| store.bot_by_id(&grant.bot_id)).flatten().ok_or("the bot is gone")?;
            let shown = serde_json::to_string_pretty(&arguments).unwrap_or_default();
            let shown: String = if shown.chars().count() > 4000 { shown.chars().take(4000).collect::<String>() + "\n…" } else { shown };
            let mut lines = vec![format!("App: {}", app_named.name), format!("Tool: {slug}")];
            if !info.description.is_empty() {
                lines.push(info.description.chars().take(300).collect());
            }
            lines.push(shown);
            let detail = json!({
                "kind": "team-change",
                "heading": format!("{} wants to use {}", bot.name, app_named.name),
                "askedBy": bot.name,
                "title": format!("{} › {slug}", app_named.name),
                "lines": lines,
                "action": "app_call",
                "target": bot.id,
                "body": { "composio": true, "tool": slug, "arguments": arguments, "account": account },
                "state": "asking",
            });
            super::turns::save_and_send(app, &grant.thread, Role::Step, &format!("{}: {slug}", app_named.name), Some(&detail));
            Ok("It's waiting for the person's approval in the chat; they see exactly what you asked for. You'll hear how it went with their next message. Don't ask again.".into())
        }
        other => Err(format!("unknown tool {other}")),
    }
}

/// Runs an approved Composio call (`apps::run_approved` sends it here).
pub async fn run_approved(app: &App, body: &Value) -> Result<String, String> {
    let c = settings(app);
    if c.key.is_empty() {
        return Err("Composio isn't set up any more".into());
    }
    let answer = execute(&c, body["tool"].as_str().unwrap_or_default(), &body["arguments"], body["account"].as_str().unwrap_or_default()).await?;
    let short: String = answer.chars().take(1500).collect();
    Ok(format!("Done. It answered: {short}"))
}

// ---- App events (stage 16.3) ------------------------------------------------------------------

/// The events an app can start a trigger on: (slug, name, description, its settings as a JSON schema).
pub async fn event_types(c: &Composio, toolkit: &str) -> Result<Vec<Value>, String> {
    let found = call(&c.key, reqwest::Method::GET, &format!("/api/v3.1/triggers_types?toolkit_slugs={toolkit}&limit=100"), None).await?;
    Ok(found["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| json!({ "event": t["slug"], "name": t["name"], "description": t["description"].as_str().map(|d| d.chars().take(300).collect::<String>()), "config": t["config"] }))
        .collect())
}

/// Starts (or changes) Composio's watch for an event, sending each one to `deliver`: its id.
pub async fn upsert_event(c: &Composio, event: &str, account: &str, config: &Value, deliver: &str) -> Result<String, String> {
    let made = call(&c.key, reqwest::Method::POST, &format!("/api/v3.1/trigger_instances/{event}/upsert"), Some(json!({ "connected_account_id": account, "trigger_config": config, "egress_url": deliver }))).await?;
    made["trigger_id"].as_str().map(str::to_string).ok_or_else(|| "Composio didn't say which watch it made".into())
}

pub async fn switch_event(c: &Composio, instance: &str, on: bool) -> Result<(), String> {
    call(&c.key, reqwest::Method::PATCH, &format!("/api/v3.1/trigger_instances/manage/{instance}"), Some(json!({ "status": if on { "enable" } else { "disable" } }))).await.map(|_| ())
}

pub async fn delete_event(c: &Composio, instance: &str) -> Result<(), String> {
    match call(&c.key, reqwest::Method::DELETE, &format!("/api/v3.1/trigger_instances/manage/{instance}"), None).await {
        Err(why) if why.contains("404") => Ok(()),
        other => other.map(|_| ()),
    }
}

// ---- The person's side: Settings › Apps › Composio -------------------------------------------

async fn shown(app: &App, c: &Composio) -> Value {
    if c.key.is_empty() {
        return json!({ "connected": false });
    }
    let (list, problem) = match accounts(c).await {
        Ok(list) => (list, None),
        Err(why) => (Vec::new(), Some(why)),
    };
    let apps: Vec<Value> = c
        .apps
        .iter()
        .map(|a| {
            let mine: Vec<Value> = list.iter().filter(|x| x["toolkit"]["slug"] == a.toolkit.as_str()).map(|x| json!({ "id": x["id"], "alias": x["alias"], "status": x["status"] })).collect();
            let names: Vec<String> = a.bots.iter().filter_map(|id| app.with_store(|store| store.bot_by_id(id)).flatten().map(|b| b.name)).collect();
            json!({ "toolkit": a.toolkit, "name": a.name, "bots": a.bots, "botNames": names, "accounts": mine })
        })
        .collect();
    let tail: String = c.key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    json!({ "connected": true, "keyEnds": tail, "apps": apps, "problem": problem })
}

/// A Tool Router session for Catbot's user: checks the key, and keeps the session for app events.
async fn session(key: &str, user_id: &str) -> Result<String, String> {
    let made = call(key, reqwest::Method::POST, "/api/v3/tool_router/session", Some(json!({ "user_id": user_id, "manage_connections": { "enable": false }, "workbench": { "enable": false } }))).await?;
    made["session_id"].as_str().map(str::to_string).ok_or_else(|| "Composio didn't make a session".into())
}

/// `GET`/`PUT`/`DELETE /api/composio`, `GET /api/composio/toolkits?search=` (`query` is the
/// search), `POST
/// /api/composio/connect` `{toolkit, name, alias}`, `PUT /api/composio/apps/:toolkit` `{bots}`,
/// `DELETE /api/composio/apps/:toolkit`, `DELETE /api/composio/accounts/:id`, `GET
/// /api/composio/events?search=<toolkit>` (what an app's triggers can start on).
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>, query: Option<String>) -> Answer {
    let body = body.unwrap_or_default();
    let mut c = settings(app);
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, shown(app, &c).await)),
        (&Method::PUT, []) => {
            let key = body["key"].as_str().map(str::trim).filter(|k| !k.is_empty()).ok_or_else(|| bad("the Composio project key is missing"))?;
            if key.starts_with("ck_") {
                return Err(bad("that's a personal key (ck_); Catbot needs the project key from your project's API Keys page, starting ak_"));
            }
            let user_id = if c.user_id.is_empty() { format!("catbot-{}", uuid::Uuid::new_v4().simple()) } else { c.user_id.clone() };
            let session_id = session(key, &user_id).await.map_err(bad)?;
            c.key = key.to_string();
            c.user_id = user_id;
            c.session_id = session_id;
            keep(app, &c);
            Ok((StatusCode::OK, shown(app, &c).await))
        }
        (&Method::DELETE, []) => {
            // The apps' accounts stay in the Composio project, for if the key comes back.
            c.key.clear();
            c.session_id.clear();
            keep(app, &c);
            Ok((StatusCode::OK, json!({ "connected": false })))
        }
        (&Method::GET, ["toolkits"]) => {
            if c.key.is_empty() {
                return Err(bad("add the Composio key first"));
            }
            // Only plain words go on to Composio.
            let search: String = query.unwrap_or_default().chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect::<String>().trim().replace(' ', "+");
            let found = call(&c.key, reqwest::Method::GET, &format!("/api/v3/toolkits?search={search}&limit=30&sort_by=usage"), None).await.map_err(bad)?;
            let list: Vec<Value> = found["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|t| json!({ "toolkit": t["slug"], "name": t["name"], "description": t["meta"]["description"].as_str().map(|d| d.chars().take(160).collect::<String>()), "noAuth": t["no_auth"] }))
                .collect();
            Ok((StatusCode::OK, json!({ "toolkits": list })))
        }
        (&Method::GET, ["events"]) => {
            if c.key.is_empty() {
                return Err(bad("add the Composio key first"));
            }
            let toolkit: String = query.unwrap_or_default().chars().filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_').collect();
            Ok((StatusCode::OK, json!({ "events": event_types(&c, &toolkit).await.map_err(bad)? })))
        }
        (&Method::POST, ["connect"]) => {
            if c.key.is_empty() {
                return Err(bad("add the Composio key first"));
            }
            let toolkit = body["toolkit"].as_str().map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).ok_or_else(|| bad("which app?"))?;
            let name = body["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&toolkit).to_string();
            let mut ask = json!({ "toolkit": toolkit });
            if let Some(alias) = body["alias"].as_str().map(str::trim).filter(|a| !a.is_empty()) {
                ask["alias"] = json!(alias);
            }
            if c.session_id.is_empty() {
                c.session_id = session(&c.key, &c.user_id).await.map_err(bad)?;
            }
            let link = call(&c.key, reqwest::Method::POST, &format!("/api/v3/tool_router/session/{}/link", c.session_id), Some(ask)).await.map_err(bad)?;
            if !c.apps.iter().any(|a| a.toolkit == toolkit) {
                c.apps.push(ComposioApp { toolkit: toolkit.clone(), name, bots: Vec::new() });
            }
            keep(app, &c);
            Ok((StatusCode::OK, json!({ "redirectUrl": link["redirect_url"], "account": link["connected_account_id"] })))
        }
        (&Method::PUT, ["apps", toolkit]) => {
            let a = c.apps.iter_mut().find(|a| a.toolkit == *toolkit).ok_or_else(|| bad("no such app"))?;
            if let Some(bots) = body["bots"].as_array() {
                a.bots = bots.iter().filter_map(|b| b.as_str().map(str::to_string)).collect();
            }
            keep(app, &c);
            Ok((StatusCode::OK, shown(app, &c).await))
        }
        (&Method::DELETE, ["apps", toolkit]) => {
            // Its accounts are disconnected at Composio too.
            for account in accounts(&c).await.map_err(bad)?.iter().filter(|a| a["toolkit"]["slug"] == *toolkit) {
                if let Some(id) = account["id"].as_str() {
                    call(&c.key, reqwest::Method::DELETE, &format!("/api/v3/connected_accounts/{id}"), None).await.map_err(bad)?;
                }
            }
            c.apps.retain(|a| a.toolkit != *toolkit);
            keep(app, &c);
            Ok((StatusCode::OK, shown(app, &c).await))
        }
        (&Method::DELETE, ["accounts", id]) => {
            let mine = accounts(&c).await.map_err(bad)?;
            if !mine.iter().any(|a| a["id"] == *id) {
                return Err(bad("no such account"));
            }
            call(&c.key, reqwest::Method::DELETE, &format!("/api/v3/connected_accounts/{id}"), None).await.map_err(bad)?;
            Ok((StatusCode::OK, shown(app, &c).await))
        }
        _ => Err(bad("no such Composio request")),
    }
}

#[cfg(test)]
mod tests {
    use super::reads;

    #[test]
    fn reading_tools_are_told_from_writing_ones() {
        assert!(reads("GMAIL_FETCH_EMAILS", "gmail", &[]));
        assert!(reads("GOOGLECALENDAR_EVENTS_LIST", "googlecalendar", &["readOnlyHint".into()]));
        assert!(!reads("GOOGLECALENDAR_EVENTS_LIST", "googlecalendar", &[]), "EVENTS isn't a verb");
        assert!(!reads("GMAIL_SEND_EMAIL", "gmail", &[]));
        assert!(!reads("GMAIL_CREATE_EMAIL_DRAFT", "gmail", &[]), "a draft writes");
        assert!(!reads("OUTLOOK_LIST_MESSAGES", "outlook", &["destructiveHint".into()]));
        assert!(reads("OUTLOOK_LIST_MESSAGES", "outlook", &[]));
    }
}
