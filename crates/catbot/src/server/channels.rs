//! Channels (stage 15.1): group chats of several bots, each with its own transcript,
//! instructions, working folder, members and lead.
//!
//! Moo's choice (2026-11-16): a message with no @name is answered by the channel's lead;
//! `@Name` (or several, or `@everyone`) picks who answers, one after another, each seeing what
//! the others said. A channel goes over the wire as a bot whose id starts `ch-`, with a
//! `channel` object, so the sidebar and the chat show it like any bot; its routes are the bot
//! routes (`/api/bots/ch-…/…`) and `POST /api/channels` to make one.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{Answer, App, attachments, bad, fail, json_body, turns, wire};
use crate::store::{Bot, Channel, Role, Store};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub fn is_channel(id: &str) -> bool {
    id.starts_with("ch-")
}

/// `POST /api/channels` `{ name, members: [bot ids], lead }`.
pub fn create(app: &Arc<App>, body: &Value) -> Answer {
    let name = body["name"].as_str().unwrap_or_default();
    let members = ids(&body["members"]);
    let channel = app
        .try_store(|store| {
            for id in &members {
                store.bot_by_id(id)?.ok_or_else(|| format!("no bot {id}"))?;
            }
            store.create_channel(name, &members, body["lead"].as_str())
        })
        .map_err(bad)?;
    let wire = app.try_store(|store| channel_wire(app, store, &channel, Some(50))).map_err(fail)?;
    app.hub.send(json!({ "kind": "bot", "bot": wire }));
    Ok((StatusCode::CREATED, json!({ "bot": wire })))
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// A channel as the interface sees a bot.
pub fn channel_wire(app: &App, store: &Store, channel: &Channel, messages: Option<usize>) -> Result<Value, BoxError> {
    let conversation = store.channel_conversation(&channel.id)?.ok_or("the channel has no transcript")?;
    let activity = if app.channel_busy(&channel.id) { "working" } else { app.turns.activity(&conversation.id) };
    let members: Vec<Value> = channel
        .members
        .iter()
        .filter_map(|id| store.bot_by_id(id).ok().flatten())
        .map(|b| json!({ "id": b.id, "name": b.name, "mark": b.mark.as_str(), "color": super::wire::color_of(&b) }))
        .collect();
    let lead = channel.lead.clone().or_else(|| channel.members.first().cloned());
    let lead_bot = lead.as_deref().and_then(|id| store.bot_by_id(id).ok().flatten());
    let mut task = lead_bot.as_ref().map(|b| wire::task(b, &conversation, activity)).unwrap_or(Value::Null);
    task["title"] = json!(channel.name);
    let mut wire = json!({
        "id": channel.id,
        "catbot": true,
        "channel": {
            "members": members,
            "lead": lead,
            "instructions": channel.instructions,
            "folder": channel.folder.to_string_lossy(),
        },
        "threadId": conversation.id,
        "mainThreadId": conversation.id,
        "name": channel.name,
        "title": "",
        "description": "",
        "soul": channel.instructions,
        "color": channel.color,
        "mark": "channel",
        "codex": true,
        "unread": false,
        "modelSelection": { "instanceId": "channel", "model": "" },
        "backups": [],
        "createdAt": super::wire::epoch_ms(&channel.created_at),
        "approvalMode": "ask",
        "activity": activity,
        "busy": activity != "idle",
        "tasks": [task],
    });
    if let Some(limit) = messages {
        let (list, more) = wire::page(store, &conversation.id, None, limit)?;
        wire["messages"] = Value::Array(list);
        wire["hasMore"] = json!(more);
    }
    Ok(wire)
}

pub fn announce(app: &App, channel_id: &str) {
    let wire = app.with_store(|store| {
        let Some(channel) = store.channel(channel_id)? else { return Ok(None) };
        Ok(Some(channel_wire(app, store, &channel, None)?))
    });
    if let Some(Some(wire)) = wire {
        app.hub.send(json!({ "kind": "bot", "bot": wire }));
    }
}

/// The channel's bot routes.
pub async fn route(app: &Arc<App>, id: &str, rest: &[String], req: Request) -> Answer {
    let method = req.method().clone();
    let channel = app.try_store(|store| store.channel(id)).map_err(fail)?.ok_or((StatusCode::NOT_FOUND, "no such channel".into()))?;
    match (method, rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
        (Method::GET, []) | (Method::POST, ["tasks", _]) => {
            let limit = super::query_param(&req, "messages").and_then(|n| n.parse().ok()).unwrap_or(50usize).min(500);
            let wire = app.try_store(|store| channel_wire(app, store, &channel, Some(limit))).map_err(fail)?;
            Ok((StatusCode::OK, json!({ "bot": wire })))
        }
        (Method::PATCH, []) => change(app, channel, json_body(req).await?),
        (Method::DELETE, []) => remove(app, &channel),
        (Method::POST, ["messages"]) => send(app, channel, json_body(req).await?),
        (Method::POST, ["attachments"]) => {
            let lead = lead_bot(app, &channel).map_err(fail)?;
            attachments::upload(app, &lead, req).await
        }
        (Method::POST, ["images"]) => {
            let lead = lead_bot(app, &channel).map_err(fail)?;
            super::images::ask(app, &lead, &json_body(req).await?)
        }
        (Method::POST, ["read"]) => Ok((StatusCode::OK, json!({ "ok": true }))),
        (Method::POST, ["interrupt"]) => {
            if let Some(conversation) = app.with_store(|store| store.channel_conversation(&channel.id)).flatten() {
                app.turns.interrupt(&conversation.id);
            }
            app.stop_channel(&channel.id);
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        _ => Err((StatusCode::NOT_FOUND, "channels don't do that".into())),
    }
}

/// The lead as itself, working in the channel's folder (for files and pictures sent there).
fn lead_bot(app: &App, channel: &Channel) -> Result<Bot, BoxError> {
    let id = channel.lead.clone().or_else(|| channel.members.first().cloned()).ok_or("the channel has no bots")?;
    let mut bot = app.try_store(|store| store.bot_by_id(&id))?.ok_or("the lead is gone")?;
    bot.folder = channel.folder.clone();
    Ok(bot)
}

/// `PATCH`: `name`, `soul` (the instructions), `color`, `members`, `lead`.
/// Deletes a channel: its conversation goes; its members and folder stay.
pub fn remove(app: &Arc<App>, channel: &Channel) -> Answer {
    if let Some(conversation) = app.with_store(|store| store.channel_conversation(&channel.id)).flatten() {
        app.turns.interrupt(&conversation.id);
    }
    app.stop_channel(&channel.id);
    app.try_store(|store| store.delete_channel(&channel.id)).map_err(fail)?;
    app.hub.send(json!({ "kind": "bot.deleted", "botId": channel.id }));
    Ok((StatusCode::OK, json!({ "ok": true })))
}

pub fn change(app: &Arc<App>, mut channel: Channel, body: Value) -> Answer {
    if let Some(name) = body["name"].as_str() {
        if name.trim().is_empty() {
            return Err(bad("a channel needs a name"));
        }
        channel.name = name.trim().to_string();
    }
    if let Some(soul) = body["soul"].as_str().or(body["instructions"].as_str()) {
        channel.instructions = soul.to_string();
    }
    if let Some(color) = body["color"].as_str() {
        channel.color = color.to_string();
    }
    if body.get("members").is_some() {
        channel.members = ids(&body["members"]);
    }
    if let Some(lead) = body["lead"].as_str() {
        channel.lead = Some(lead.to_string());
    }
    app.try_store(|store| {
        for id in &channel.members {
            store.bot_by_id(id)?.ok_or_else(|| format!("no bot {id}"))?;
        }
        store.update_channel(&channel)
    })
    .map_err(bad)?;
    let channel = app.try_store(|store| store.channel(&channel.id)).map_err(fail)?.ok_or_else(|| bad("no such channel"))?;
    let wire = app.try_store(|store| channel_wire(app, store, &channel, None)).map_err(fail)?;
    app.hub.send(json!({ "kind": "bot", "bot": wire }));
    Ok((StatusCode::OK, json!({ "bot": wire })))
}

/// Who answers a message: the members it @names, in the order named (`@everyone`: all of
/// them), or else the lead.
pub fn responders(text: &str, members: &[Bot], lead: Option<&str>) -> Vec<Bot> {
    let lower = text.to_lowercase();
    if lower.contains("@everyone") || lower.contains("@all ") || lower.ends_with("@all") {
        return members.to_vec();
    }
    let named = named(text, members);
    if named.is_empty() {
        let lead = lead.and_then(|l| members.iter().find(|b| b.id == l)).or(members.first());
        return lead.cloned().into_iter().collect();
    }
    named
}

/// The bots a text @names, in the order named.
pub fn named(text: &str, members: &[Bot]) -> Vec<Bot> {
    let lower = text.to_lowercase();
    let mut named: Vec<(usize, Bot)> = Vec::new();
    // Longer names first, so "@Kiwi Two" isn't taken as "@Kiwi".
    let mut by_length: Vec<&Bot> = members.iter().collect();
    by_length.sort_by_key(|b| std::cmp::Reverse(b.name.len()));
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for bot in by_length {
        let tag = format!("@{}", bot.name.to_lowercase());
        let mut from = 0;
        while let Some(found) = lower[from..].find(&tag) {
            let at = from + found;
            let end = at + tag.len();
            let whole = lower[end..].chars().next().is_none_or(|c| !c.is_alphanumeric())
                && lower[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            let overlaps = taken.iter().any(|(s, e)| at < *e && end > *s);
            if whole && !overlaps {
                taken.push((at, end));
                if !named.iter().any(|(_, b)| b.id == bot.id) {
                    named.push((at, bot.clone()));
                }
                break;
            }
            from = end;
        }
    }
    named.sort_by_key(|(at, _)| *at);
    named.into_iter().map(|(_, b)| b).collect()
}

/// A member as it works in this channel: in the channel's folder, told about the channel.
pub fn in_channel(mut bot: Bot, channel: &Channel, members: &[Bot]) -> Bot {
    let others: Vec<String> = members
        .iter()
        .filter(|m| m.id != bot.id)
        .map(|m| if channel.lead.as_deref() == Some(m.id.as_str()) { format!("{} (the lead)", m.name) } else { m.name.clone() })
        .collect();
    let lead_note = if channel.lead.as_deref() == Some(bot.id.as_str()) {
        " You're its lead: you answer whatever isn't addressed to someone by @name."
    } else {
        ""
    };
    let mut note = format!(
        "\n\nYou're talking in the channel \"{}\", a group chat with the person and the bots {}.{lead_note} Others' messages \
         come marked with their names. The person picks who answers with @Name. Answer as yourself, briefly, \
         without repeating what others have said. To hand part of the work to another bot here, end your reply \
         with @Name and what you need from them.",
        channel.name,
        if others.is_empty() { "(none but you)".to_string() } else { others.join(", ") },
    );
    if !channel.instructions.trim().is_empty() {
        note.push_str(&format!("\n\nThe channel's instructions:\n{}", channel.instructions.trim()));
    }
    bot.instructions.push_str(&note);
    bot.folder = channel.folder.clone();
    bot
}

/// `POST …/ch-…/messages`: the person's message, then each responder's turn in order.
fn send(app: &Arc<App>, channel: Channel, body: Value) -> Answer {
    let conversation = app.try_store(|store| store.channel_conversation(&channel.id)).map_err(fail)?.ok_or_else(|| bad("the channel has no transcript"))?;
    if app.channel_busy(&channel.id) || app.turns.busy(&conversation.id) {
        return Err((StatusCode::CONFLICT, "the channel is still answering the last message".into()));
    }
    let members: Vec<Bot> = channel.members.iter().filter_map(|id| app.with_store(|store| store.bot_by_id(id)).flatten()).collect();
    if members.is_empty() {
        return Err(bad("the channel has no bots"));
    }
    let lead = lead_bot(app, &channel).map_err(fail)?;
    let attached = attachments::for_message(app, &lead, &body["attachments"])?;
    let text = body["text"].as_str().map(str::trim).unwrap_or("").to_string();
    if text.is_empty() && attached.is_empty() {
        return Err(bad("nothing to send"));
    }
    let mut detail = json!({});
    if let Some(id) = body["sendId"].as_str() {
        detail["sendId"] = json!(id);
    }
    let described: Vec<Value> = attached.iter().map(attachments::describe).collect();
    if !described.is_empty() {
        detail["attachments"] = json!(described);
    }
    let reacted = super::reactions::untold_note(app, &conversation.id);
    if !reacted.is_empty() {
        detail["reactionNote"] = json!(reacted);
    }
    let message = turns::save_and_send(app, &conversation.id, Role::Person, &text, (detail != json!({})).then_some(&detail))
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "couldn't save the message".to_string()))?;
    let request_id = message["id"].as_str().and_then(wire::parse_message_id).ok_or((StatusCode::INTERNAL_SERVER_ERROR, "no message id".to_string()))?;
    let answering = responders(&text, &members, channel.lead.as_deref());
    app.set_channel_busy(&channel.id, true);
    announce(app, &channel.id);
    let app = app.clone();
    let channel_id = channel.id.clone();
    let thread = conversation.id.clone();
    let asked = answering.len() as u32;
    let queue = answering.into_iter().map(|bot| super::handoffs::Turn { bot, depth: 0, from: None }).collect();
    tokio::spawn(async move {
        super::handoffs::follow(&app, &channel_id, &thread, request_id, queue, super::handoffs::Place::Channel(channel_id.clone()), asked, 0).await;
        app.set_channel_busy(&channel_id, false);
        announce(&app, &channel_id);
    });
    Ok((StatusCode::ACCEPTED, json!({ "ok": true, "message": message })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(id: &str, name: &str) -> Bot {
        Bot {
            id: id.into(),
            name: name.into(),
            instructions: String::new(),
            engine: "claude".into(),
            model: None,
            mode: crate::claude::PermissionMode::Default,
            codex: false,
            folder: std::path::PathBuf::new(),
            created_at: String::new(),
            mark: crate::store::Mark::CatHead,
            color: None,
            effort: None,
        }
    }

    #[test]
    fn at_names_pick_who_answers_and_the_lead_takes_the_rest() {
        let members = vec![bot("a", "Scout"), bot("b", "Kiwi"), bot("c", "Kiwi Two")];
        let names = |text: &str| responders(text, &members, Some("b")).into_iter().map(|b| b.name).collect::<Vec<_>>();
        assert_eq!(names("What do you think?"), vec!["Kiwi"]);
        assert_eq!(names("@kiwi two, then @Scout: plan it"), vec!["Kiwi Two", "Scout"]);
        assert_eq!(names("@Kiwi and @Kiwi again"), vec!["Kiwi"]);
        assert_eq!(names("@everyone hello"), vec!["Scout", "Kiwi", "Kiwi Two"]);
        assert_eq!(names("email me@scoutish.com"), vec!["Kiwi"], "an @ inside a word isn't a name");
        assert_eq!(names("write to me@scout.com"), vec!["Kiwi"]);
    }
}
