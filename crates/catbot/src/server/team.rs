//! Running the team (stage 15.3.2): the Chief of Staff makes, changes and deletes bots and
//! channels, asking first.
//!
//! Moo's choice (2026-11-16): nothing is off-limits, deleting bots and changing approval levels
//! included, but every change asks. The Chief writes `<team-change>{…}</team-change>` in its
//! reply; Catbot takes it out and shows a card saying exactly what would change, with Approve and
//! Decline. Nothing happens until the person approves. The Chief hears how each card went with the
//! person's next message.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, bad, channels, fail, turns, wire};
use crate::store::{Bot, Channel, Role, Store};

const OPEN: &str = "<team-change>";
const CLOSE: &str = "</team-change>";

/// What the Chief is told about changing the team.
pub const CHIEF_NOTE: &str = "\n\nYou can change the team, but only with the person's yes. To propose a change, put it in your reply as \
<team-change>{JSON}</team-change>, one change per tag. Catbot shows the person a card with exactly what would change, and does it \
only if they approve; you'll hear how it went with their next message. Say in your reply what you proposed and why. The changes:\n\
- {\"action\":\"create_bot\",\"name\":\"…\",\"instructions\":\"…\",\"ai\":\"claude\",\"approval\":\"ask\"}\n\
- {\"action\":\"change_bot\",\"bot\":\"its name\", and any of \"name\", \"instructions\", \"ai\", \"model\", \"approval\", \"color\", \"codex\": true or false}\n\
- {\"action\":\"delete_bot\",\"bot\":\"its name\"}\n\
- {\"action\":\"create_channel\",\"name\":\"…\",\"members\":[\"bot names\"],\"lead\":\"a member\",\"instructions\":\"…\"}\n\
- {\"action\":\"change_channel\",\"channel\":\"its name\", and any of \"name\", \"instructions\", \"members\", \"lead\"}\n\
- {\"action\":\"delete_channel\",\"channel\":\"its name\"}\n\
Approval levels: \"ask\" (asks before acting), \"edits\" (edits files without asking), \"auto\" (decides what's safe), \
\"full\" (acts without asking). AIs are named by id: claude, and also openai, xai, groq, mistral, deepseek-chat, ollama, gemini, grok-cli and others in Catbot's list.";

/// Takes `<team-change>…</team-change>` out of a reply: the reply without them, and what was in them.
pub fn take(reply: &str) -> (String, Vec<String>) {
    let mut rest = String::new();
    let mut found = Vec::new();
    let mut text = reply;
    while let Some(start) = text.find(OPEN) {
        rest.push_str(&text[..start]);
        let after = &text[start + OPEN.len()..];
        match after.find(CLOSE) {
            Some(end) => {
                found.push(after[..end].trim().to_string());
                text = &after[end + CLOSE.len()..];
            }
            None => text = "",
        }
    }
    rest.push_str(text);
    let rest = rest.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string();
    (rest, found)
}

/// A proposed change made ready to show and to do: what it says, in lines, and the request with
/// names turned into ids.
#[derive(Debug, PartialEq)]
struct Ready {
    title: String,
    lines: Vec<String>,
    /// A warning to show on the card (deleting, or acting without asking).
    warning: Option<String>,
    action: String,
    target: Option<String>,
    body: Value,
}

fn find_bot<'a>(bots: &'a [Bot], name: &str) -> Result<&'a Bot, String> {
    let name = name.trim().trim_start_matches('@');
    bots.iter().find(|b| b.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("there's no bot called {name}"))
}

fn find_channel<'a>(channels: &'a [Channel], name: &str) -> Result<&'a Channel, String> {
    let name = name.trim().trim_start_matches('#');
    channels.iter().find(|c| c.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("there's no channel called {name}"))
}

fn level_words(level: &str) -> Result<&'static str, String> {
    Ok(match level {
        "ask" => "asks before acting",
        "edits" => "edits files without asking",
        "auto" => "decides what's safe to run",
        "full" => "acts without asking",
        other => return Err(format!("{other} isn't an approval level (ask, edits, auto or full)")),
    })
}

fn short(text: &str) -> String {
    let words: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() > 160 { format!("{}…", words.chars().take(159).collect::<String>()) } else { words }
}

/// A bot's fields from a change, as the bot routes take them, with a line for each.
fn bot_fields(change: &Value, lines: &mut Vec<String>, warning: &mut Option<String>, name_of: Option<&str>) -> Result<Value, String> {
    let mut body = json!({});
    if let Some(name) = change["name"].as_str().filter(|n| !n.trim().is_empty()) {
        if name_of.is_some() {
            lines.push(format!("Name: {}", name.trim()));
        }
        body["name"] = json!(name.trim());
    }
    if let Some(text) = change["instructions"].as_str() {
        lines.push(format!("Instructions: {}", short(text)));
        body["soul"] = json!(text.trim());
    }
    if let Some(ai) = change["ai"].as_str().filter(|a| !a.trim().is_empty()) {
        let engine = crate::engines::find(ai.trim()).ok_or_else(|| format!("Catbot doesn't know the AI {ai}"))?;
        let model = change["model"].as_str().filter(|m| !m.trim().is_empty());
        lines.push(match model {
            Some(model) => format!("AI: {} ({model})", engine.name),
            None => format!("AI: {}", engine.name),
        });
        body["modelSelection"] = json!({ "instanceId": engine.id, "model": model });
    } else if let Some(model) = change["model"].as_str().filter(|m| !m.trim().is_empty()) {
        lines.push(format!("Model: {model}"));
        body["modelSelection"] = json!({ "model": model });
    }
    if let Some(level) = change["approval"].as_str() {
        let words = level_words(level)?;
        lines.push(format!("Approval level: {words}"));
        if level == "full" {
            *warning = Some(format!("{} would act without asking, anywhere it can reach.", name_of.unwrap_or("The bot")));
        }
        body["approvalMode"] = json!(level);
    }
    if let Some(color) = change["color"].as_str() {
        if !crate::store::COLORS.contains(&color) {
            return Err(format!("{color} isn't one of Catbot's colours"));
        }
        lines.push(format!("Colour: {color}"));
        body["color"] = json!(color);
    }
    if let Some(codex) = change["codex"].as_bool() {
        lines.push(if codex { "Codex vault: may read and write it".into() } else { "Codex vault: no access".into() });
        body["codex"] = json!(codex);
    }
    Ok(body)
}

fn members(change: &Value, bots: &[Bot]) -> Result<Option<Vec<Bot>>, String> {
    let Some(list) = change["members"].as_array() else { return Ok(None) };
    let found: Result<Vec<Bot>, String> = list.iter().filter_map(Value::as_str).map(|n| find_bot(bots, n).cloned()).collect();
    let found = found?;
    if found.is_empty() {
        return Err("a channel needs at least one bot".into());
    }
    Ok(Some(found))
}

fn names(bots: &[Bot]) -> String {
    bots.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// Reads a proposed change against the team as it is.
fn ready(store: &Store, raw: &str) -> Result<Ready, String> {
    let change: Value = serde_json::from_str(raw).map_err(|e| format!("it isn't JSON Catbot can read ({e})"))?;
    let bots = store.bots().map_err(|e| e.to_string())?;
    let channels = store.channels().map_err(|e| e.to_string())?;
    let action = change["action"].as_str().unwrap_or_default().to_string();
    let mut lines = Vec::new();
    let mut warning = None;
    let (title, target, body) = match action.as_str() {
        "create_bot" => {
            let name = change["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or("a new bot needs a name")?;
            let body = bot_fields(&change, &mut lines, &mut warning, None)?;
            (format!("Make a new bot, {name}"), None, body)
        }
        "change_bot" => {
            let bot = find_bot(&bots, change["bot"].as_str().unwrap_or_default())?;
            let body = bot_fields(&change, &mut lines, &mut warning, Some(&bot.name))?;
            if lines.is_empty() {
                return Err(format!("it doesn't say what to change about {}", bot.name));
            }
            (format!("Change {}", bot.name), Some(bot.id.clone()), body)
        }
        "delete_bot" => {
            let bot = find_bot(&bots, change["bot"].as_str().unwrap_or_default())?;
            warning = Some(format!("{}'s chats are deleted for good. Its folder of files stays.", bot.name));
            (format!("Delete {}", bot.name), Some(bot.id.clone()), json!({}))
        }
        "create_channel" => {
            let name = change["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or("a new channel needs a name")?;
            let members = members(&change, &bots)?.ok_or("a new channel needs its members")?;
            let lead = match change["lead"].as_str() {
                Some(lead) => {
                    let lead = find_bot(&bots, lead)?;
                    if !members.iter().any(|m| m.id == lead.id) {
                        return Err(format!("{} isn't one of the channel's members", lead.name));
                    }
                    lead.clone()
                }
                None => members[0].clone(),
            };
            lines.push(format!("Members: {}", names(&members)));
            lines.push(format!("Lead: {}", lead.name));
            let mut body = json!({ "name": name, "members": members.iter().map(|b| &b.id).collect::<Vec<_>>(), "lead": lead.id });
            if let Some(text) = change["instructions"].as_str() {
                lines.push(format!("Instructions: {}", short(text)));
                body["soul"] = json!(text);
            }
            (format!("Make a channel, #{name}"), None, body)
        }
        "change_channel" => {
            let channel = find_channel(&channels, change["channel"].as_str().unwrap_or_default())?;
            let mut body = json!({});
            if let Some(name) = change["name"].as_str().filter(|n| !n.trim().is_empty()) {
                lines.push(format!("Name: #{}", name.trim()));
                body["name"] = json!(name.trim());
            }
            if let Some(text) = change["instructions"].as_str() {
                lines.push(format!("Instructions: {}", short(text)));
                body["soul"] = json!(text);
            }
            let new_members = members(&change, &bots)?;
            if let Some(list) = &new_members {
                lines.push(format!("Members: {}", names(list)));
                body["members"] = json!(list.iter().map(|b| &b.id).collect::<Vec<_>>());
            }
            if let Some(lead) = change["lead"].as_str() {
                let lead = find_bot(&bots, lead)?;
                let in_it = new_members.as_ref().map(|l| l.iter().any(|m| m.id == lead.id)).unwrap_or(channel.members.contains(&lead.id));
                if !in_it {
                    return Err(format!("{} isn't one of #{}'s members", lead.name, channel.name));
                }
                lines.push(format!("Lead: {}", lead.name));
                body["lead"] = json!(lead.id);
            }
            if lines.is_empty() {
                return Err(format!("it doesn't say what to change about #{}", channel.name));
            }
            (format!("Change #{}", channel.name), Some(channel.id.clone()), body)
        }
        "delete_channel" => {
            let channel = find_channel(&channels, change["channel"].as_str().unwrap_or_default())?;
            warning = Some(format!("#{}'s conversation is deleted for good. Its bots and folder stay.", channel.name));
            (format!("Delete #{}", channel.name), Some(channel.id.clone()), json!({}))
        }
        "" => return Err("it doesn't say which change (\"action\")".into()),
        other => return Err(format!("{other} isn't a change Catbot knows")),
    };
    Ok(Ready { title, lines, warning, action, target, body })
}

/// Shows a card for each change the Chief proposed in a reply (or a note when one can't be read).
pub fn propose(app: &App, thread: &str, chief: &Bot, found: Vec<String>) {
    for raw in found {
        match app.with_store(|store| Ok(ready(store, &raw))) {
            Some(Ok(ready)) => {
                let detail = json!({
                    "kind": "team-change",
                    "askedBy": chief.name,
                    "title": ready.title,
                    "lines": ready.lines,
                    "warning": ready.warning,
                    "action": ready.action,
                    "target": ready.target,
                    "body": ready.body,
                    "state": "asking",
                });
                turns::save_and_send(app, thread, Role::Step, &format!("Team change: {}", ready.title), Some(&detail));
            }
            Some(Err(why)) => {
                let note = format!("{} proposed a team change Catbot couldn't use: {why}.", chief.name);
                turns::save_and_send(app, thread, Role::Note, &note, None);
                remember(app, thread, &format!("Your proposed change couldn't be used: {why}."));
            }
            None => {}
        }
    }
}

/// `POST /api/threads/:thread/team/:message` `{ approve: true | false }`.
pub async fn answer(app: &Arc<App>, thread: &str, message: &str, body: &Value) -> Answer {
    let id = wire::parse_message_id(message).ok_or_else(|| bad("which change?"))?;
    let row = app
        .try_store(|store| store.message_in(thread, id))
        .map_err(fail)?
        .filter(|m| m.detail.as_ref().is_some_and(|d| d["kind"] == "team-change"))
        .ok_or((StatusCode::NOT_FOUND, "no such team change in this chat".to_string()))?;
    let mut detail = row.detail.clone().unwrap_or(Value::Null);
    if detail["state"] != "asking" {
        return Err((StatusCode::CONFLICT, "this change has been answered already".into()));
    }
    let title = detail["title"].as_str().unwrap_or_default().to_string();
    if body["approve"] != true {
        detail["state"] = json!("declined");
        update(app, thread, id, &detail);
        remember(app, thread, &format!("The person declined: {title}."));
        return Ok((StatusCode::OK, json!({ "ok": true, "state": "declined" })));
    }
    // A card with a choice (where a page goes): the person's pick, if it's one of the card's own.
    if detail["choice"].is_object() {
        let options = detail["choice"]["options"].as_array().cloned().unwrap_or_default();
        let picked = body["choice"].as_str().or(detail["choice"]["value"].as_str()).unwrap_or_default().to_string();
        if !options.iter().any(|o| o["id"] == picked.as_str()) {
            return Err(bad("that isn't one of the card's choices"));
        }
        detail["choice"]["value"] = json!(picked);
        detail["body"]["choice"] = json!(picked);
    }
    // Sending an email or publishing a page takes a moment: the card says so meanwhile.
    let slow = match detail["action"].as_str() {
        Some("send_mail") => Some("sending"),
        Some("publish_page") => Some("publishing"),
        Some("app_call") => Some("working"),
        _ => None,
    };
    if let Some(state) = slow {
        let mut working = detail.clone();
        working["state"] = json!(state);
        update(app, thread, id, &working);
    }
    let outcome = apply(app, &detail).await;
    match &outcome {
        Ok(note) => {
            detail["state"] = json!("done");
            // What came of it (a page's link), on the card and for the bot.
            if let Some(note) = note {
                detail["doneText"] = json!(note);
            }
            let more = note.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
            remember(app, thread, &format!("The person approved, and it's done: {title}.{more}"));
        }
        Err(why) => {
            detail["state"] = json!("failed");
            detail["error"] = json!(why);
            remember(app, thread, &format!("The person approved {title}, but it failed: {why}."));
        }
    }
    update(app, thread, id, &detail);
    Ok((StatusCode::OK, json!({ "ok": outcome.is_ok(), "state": detail["state"], "error": detail["error"] })))
}

/// Does an approved change.
async fn apply(app: &Arc<App>, detail: &Value) -> Result<Option<String>, String> {
    let target = detail["target"].as_str().unwrap_or_default();
    let body = detail["body"].clone();
    let refused = |(_, why): (StatusCode, String)| why;
    let bot = || app.with_store(|store| store.bot_by_id(target)).flatten().ok_or_else(|| "that bot is gone".to_string());
    let channel = || app.with_store(|store| store.channel(target)).flatten().ok_or_else(|| "that channel is gone".to_string());
    let done = match detail["action"].as_str().unwrap_or_default() {
        "publish_page" => return super::pages::publish(app, &body).await.map(Some),
        "app_call" => return super::apps::run_approved(app, &body).await.map(Some),
        "create_bot" => super::make_bot(app, &body).map(|_| ()).map_err(refused),
        "change_bot" => super::patch_bot(app, bot()?, body).map(|_| ()).map_err(refused),
        "delete_bot" => super::remove_bot(app, &bot()?).map(|_| ()).map_err(refused),
        "create_channel" => channels::create(app, &body).map(|_| ()).map_err(refused),
        "change_channel" => channels::change(app, channel()?, body).map(|_| ()).map_err(refused),
        "delete_channel" => channels::remove(app, &channel()?).map(|_| ()).map_err(refused),
        "save_skill" => super::skills::save_approved(app, target, body["text"].as_str().unwrap_or_default()),
        "send_mail" => super::email::send(app, &body).await,
        other => Err(format!("{other} isn't a change Catbot knows")),
    };
    done.map(|_| None)
}

fn update(app: &App, thread: &str, id: i64, detail: &Value) {
    let wire = app.with_store(|store| {
        store.update_detail(id, detail)?;
        let parent = store.previous_message_id(thread, id)?;
        Ok(wire::message(&store.message(id)?.ok_or("message vanished")?, parent))
    });
    if let Some(wire) = wire {
        app.hub.send(json!({ "kind": "message.patch", "threadId": thread, "message": wire }));
    }
}

/// Keeps a line for the Chief to hear with the person's next message in this chat.
fn remember(app: &App, thread: &str, line: &str) {
    let key = format!("team-untold:{thread}");
    let mut lines: Vec<String> = app.with_store(|store| store.setting(&key)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    lines.push(line.to_string());
    app.with_store(|store| store.set_setting(&key, &json!(lines)));
}

/// What happened to the Chief's proposed changes since it last heard, once.
pub fn untold_note(app: &App, thread: &str) -> String {
    let key = format!("team-untold:{thread}");
    let lines: Vec<String> = app.with_store(|store| store.setting(&key)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    if lines.is_empty() {
        return String::new();
    }
    app.with_store(|store| store.set_setting(&key, &json!([])));
    format!("\n\n(Your team changes: {})", lines.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_come_out_of_the_reply() {
        let (rest, found) = take("I'll add a researcher.\n<team-change>{\"action\":\"create_bot\",\"name\":\"Moss\"}</team-change>\nOK?");
        assert_eq!(rest, "I'll add a researcher.\n\nOK?");
        assert_eq!(found, ["{\"action\":\"create_bot\",\"name\":\"Moss\"}"]);
        assert_eq!(take("no tags").1.len(), 0);
    }

    #[test]
    fn approval_levels_are_worded_and_checked() {
        assert_eq!(level_words("full"), Ok("acts without asking"));
        assert!(level_words("sometimes").is_err());
    }
}
