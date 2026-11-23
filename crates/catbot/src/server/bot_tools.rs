//! Catbot's own tools for bots, the server's end (stage 13.9; `catbot_tools.rs` is the bots'
//! end). Each turn that has any gets a key, good until the turn ends; the key says which bot and
//! which chat a call comes from. Only the home door serves them.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{App, Answer};
use crate::catbot_tools::{Access, KEY_HEADER};
use crate::store::Bot;

/// Who a key speaks for.
#[derive(Debug, Clone)]
pub struct Grant {
    pub bot_id: String,
    pub thread: String,
}

/// A turn's key: closed when the turn is done with it.
pub struct Open {
    app: Arc<App>,
    pub access: Access,
    pub tools: Vec<Value>,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.app.tool_grants.lock().expect("grants lock").remove(&self.access.key);
    }
}

/// The tools a bot has: publishing pages, its triggers, its mailbox if it may use one, the
/// connected apps it's ticked for, and its own computer if it has one.
pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    let mut tools = super::inbox::tools();
    tools.extend(super::pages::tools());
    tools.extend(super::triggers::tools());
    tools.extend(super::email::tools_for(app, bot_id));
    tools.extend(super::apps::tools_for(app, bot_id));
    tools.extend(super::composio::tools_for(app, bot_id));
    tools.extend(super::computers::tools_for(app, bot_id));
    tools.extend(super::secrets::tools_for(app, bot_id));
    tools
}

/// A key for a turn, if the bot has any tools and Catbot knows its own address.
pub fn open(app: &Arc<App>, bot: &Bot, thread: &str) -> Option<Open> {
    let tools = tools_for(app, &bot.id);
    if tools.is_empty() {
        return None;
    }
    let addr = *app.local.get()?;
    let host = if addr.ip().is_unspecified() { "127.0.0.1".to_string() } else { addr.ip().to_string() };
    let host = if addr.is_ipv6() && host != "127.0.0.1" { format!("[{host}]") } else { host };
    let key = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    app.tool_grants.lock().expect("grants lock").insert(key.clone(), Grant { bot_id: bot.id.clone(), thread: thread.to_string() });
    Some(Open { app: app.clone(), access: Access { url: format!("http://{host}:{}", addr.port()), key }, tools })
}

fn grant(app: &App, req: &Request) -> Option<Grant> {
    let key = req.headers().get(KEY_HEADER)?.to_str().ok()?;
    app.tool_grants.lock().expect("grants lock").get(key).cloned()
}

/// `GET /api/bot-tools` and `POST /api/bot-tools/call` `{ name, arguments }`, with a turn's key.
pub async fn route(app: &Arc<App>, call: bool, req: Request) -> Answer {
    let grant = grant(app, &req).ok_or((StatusCode::FORBIDDEN, "no such turn key".to_string()))?;
    if !call {
        return Ok((StatusCode::OK, json!({ "tools": tools_for(app, &grant.bot_id) })));
    }
    let body = super::json_body(req).await?;
    let name = body["name"].as_str().unwrap_or_default();
    let args = if body["arguments"].is_object() { body["arguments"].clone() } else { json!({}) };
    if !tools_for(app, &grant.bot_id).iter().any(|t| t["name"] == name) {
        return Ok((StatusCode::OK, json!({ "text": format!("There's no Catbot tool called {name} for this bot."), "isError": true })));
    }
    // A bot's own computer: free inside its box, and a screenshot comes back as a picture.
    if super::computers::knows(name) {
        return Ok((StatusCode::OK, super::computers::call(app, &grant, name, &args).await));
    }
    let answer = match name {
        "message_person" => super::inbox::message_person(app, &grant, &args),
        "page_publish" => super::pages::propose(app, &grant, &args).await,
        "secrets_list" => super::secrets::call(app, &grant, &args).await,
        "trigger_info" | "trigger" | "delete_trigger" => super::triggers::call(app, &grant, name, &args).await,
        _ if name.starts_with("mail_") => super::email::call(app, &grant, name, &args).await,
        _ if super::apps::knows(app, &grant.bot_id, name) => super::apps::call(app, &grant, name, &args).await,
        _ if super::composio::knows(name) => super::composio::call_tool(app, &grant, name, &args).await,
        _ => Err(format!("unknown tool {name}")),
    };
    Ok((StatusCode::OK, match answer {
        Ok(text) => json!({ "text": text }),
        Err(text) => json!({ "text": text, "isError": true }),
    }))
}

/// The MCP server an agent is given for Catbot's tools (ACP's form): the helper, with no approvals.
pub fn acp_server(app: &App, open: &Open) -> Option<Value> {
    let (program, leading) = app.helper.as_ref()?.split_first()?;
    let mut args: Vec<String> = leading.to_vec();
    args.extend(["approval-mcp".into(), "-".into()]);
    let env: Vec<Value> = open.access.env().into_iter().map(|(name, value)| json!({ "name": name, "value": value })).collect();
    Some(json!({ "name": crate::approval::SERVER_NAME, "command": program, "args": args, "env": env }))
}
