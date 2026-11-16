//! Team files (stage 15.4): a whole team, in and out of one Markdown file.
//!
//! The file is MausBot's portable format (BotMRR Markdown, `github.com/milind-soni/openmausbot-teams`,
//! MIT): YAML at the top (`agents`, `chiefOfStaff`, `rooms`, `playbooks`, …) and a playbook in
//! Markdown below that any Chief of Staff can follow. Catbot reads v1 and v2 files and MausBot's
//! older `.mausteam.json`, and writes v1.
//!
//! Moo's choice (2026-11-16): import with a review screen first; MausBot's safety rules. A team
//! file never carries chats, keys, AIs or approval levels: imported bots start out asking before
//! they act, on Catbot's default AI. Routines and connected apps aren't imported (Catbot has
//! neither yet); the review says so. Export leaves out anything that looks like a secret.

use std::sync::Arc;

use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Answer, App, bad, channels, fail};
use crate::store::Bot;

/// The most a team file may be.
const MOST_BYTES: usize = 512 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Package {
    id: Option<String>,
    name: String,
    tagline: String,
    summary: String,
    agents: Vec<Agent>,
    chief_of_staff: Value,
    rooms: Vec<Room>,
    routines: Vec<Value>,
    playbooks: Vec<Playbook>,
    requirements: Requirements,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Agent {
    key: String,
    name: String,
    title: String,
    description: String,
    appearance: Appearance,
    playbooks: Vec<String>,
    skills: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Appearance {
    color: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Room {
    key: String,
    name: String,
    members: Vec<String>,
    bulletin: String,
    default_responder: Value,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Playbook {
    key: String,
    name: String,
    summary: String,
    instructions: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Requirements {
    apps: Vec<Value>,
}

/// A team file read: what's in it, and its Markdown playbook.
struct Read {
    package: Package,
    body: String,
}

/// Reads a team file: Markdown with YAML at the top, or MausBot's older JSON.
fn read(text: &str) -> Result<Read, String> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.starts_with('{') {
        return read_json(text);
    }
    let rest = text.strip_prefix("---").ok_or("a team file starts with its YAML between --- lines")?;
    let end = rest.find("\n---").ok_or("the YAML at the top isn't closed with ---")?;
    let yaml = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('-').trim().to_string();
    let value: Value = serde_yaml_ng::from_str(yaml).map_err(|e| format!("the YAML at the top doesn't read: {e}"))?;
    let package: Package = serde_json::from_value(value).map_err(|e| format!("the team's fields don't read: {e}"))?;
    Ok(Read { package, body })
}

/// MausBot's older `team.mausteam.json`: `team.members` and one `team.room`.
fn read_json(text: &str) -> Result<Read, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("it isn't a team file Catbot can read: {e}"))?;
    let team = &value["team"];
    let mut package = Package {
        name: team["name"].as_str().unwrap_or_default().to_string(),
        tagline: team["description"].as_str().unwrap_or_default().to_string(),
        agents: serde_json::from_value(team["members"].clone()).unwrap_or_default(),
        ..Package::default()
    };
    if team["room"].is_object() {
        let mut room: Room = serde_json::from_value(team["room"].clone()).unwrap_or_default();
        room.members = package.agents.iter().map(|a| a.key.clone()).collect();
        package.rooms.push(room);
    }
    Ok(Read { package, body: String::new() })
}

fn key_of(agent: &Agent) -> String {
    if agent.key.trim().is_empty() { agent.name.trim().to_lowercase() } else { agent.key.trim().to_string() }
}

/// The agent key named as Chief of Staff: `chiefOfStaff: ada`, or `{ agent: ada }`.
fn chief_key(package: &Package) -> Option<String> {
    match &package.chief_of_staff {
        Value::String(key) => Some(key.clone()),
        Value::Object(o) => o.get("agent").or(o.get("key")).and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn lead_key(room: &Room) -> Option<String> {
    match &room.default_responder {
        Value::String(key) => Some(key.clone()),
        Value::Object(o) => o.get("agent").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

/// A bot's instructions from its role: what it does, its title on the team, its playbooks.
fn instructions(agent: &Agent, package: &Package) -> String {
    let mut text = agent.description.trim().to_string();
    let team = if package.name.trim().is_empty() { "this" } else { package.name.trim() };
    if !agent.title.trim().is_empty() {
        text.push_str(&format!("\n\nYour role on the {team} team: {}.", agent.title.trim()));
    }
    let used: Vec<&Playbook> = agent.playbooks.iter().filter_map(|k| package.playbooks.iter().find(|p| &p.key == k)).collect();
    if !used.is_empty() {
        text.push_str("\n\nYour playbooks:");
        for p in used {
            text.push_str(&format!("\n\n{}: {}\n{}", p.name.trim(), p.summary.trim(), p.instructions.trim()));
        }
    }
    text.push_str("\n\nThe team's whole playbook is in TEAM.md in your folder.");
    text
}

/// A role's first forty words.
fn about(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 40 { format!("{}…", words[..40].join(" ")) } else { words.join(" ") }
}

/// What importing a file would do, for the review screen.
fn plan(app: &App, read: &Read) -> Result<Value, String> {
    let package = &read.package;
    if package.agents.is_empty() {
        return Err("the file has no bots (agents) in it".into());
    }
    let taken: Vec<String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| b.name.to_lowercase()).collect();
    let name_of = |key: &str| package.agents.iter().find(|a| key_of(a) == key).map(|a| a.name.clone()).unwrap_or_else(|| key.to_string());
    let bots: Vec<Value> = package
        .agents
        .iter()
        .map(|a| {
            let color = a.appearance.color.clone().filter(|c| crate::store::COLORS.contains(&c.as_str()));
            json!({
                "key": key_of(a),
                "name": a.name,
                "title": a.title,
                "about": about(&a.description),
                "color": color,
                "playbooks": a.playbooks.iter().filter_map(|k| package.playbooks.iter().find(|p| &p.key == k)).map(|p| p.name.clone()).collect::<Vec<_>>(),
                "nameTaken": taken.contains(&a.name.trim().to_lowercase()),
            })
        })
        .collect();
    let rooms: Vec<Value> = package
        .rooms
        .iter()
        .map(|r| json!({ "key": r.key, "name": r.name, "members": r.members.iter().map(|m| name_of(m)).collect::<Vec<_>>(), "lead": lead_key(r).map(|k| name_of(&k)) }))
        .collect();
    let mut left_out = Vec::new();
    if !package.routines.is_empty() {
        left_out.push(format!("{} routine{} (Catbot has no scheduled routines yet)", package.routines.len(), if package.routines.len() == 1 { "" } else { "s" }));
    }
    let apps: Vec<String> = package.requirements.apps.iter().filter_map(|a| a["label"].as_str().or(a["slug"].as_str()).map(str::to_string)).collect();
    if !apps.is_empty() {
        left_out.push(format!("connected apps: {} (Catbot can't connect apps yet)", apps.join(", ")));
    }
    let skills: usize = package.agents.iter().map(|a| a.skills.len()).sum();
    if skills > 0 {
        left_out.push(format!("{skills} skill{} (they come as separate files beside a team folder)", if skills == 1 { "" } else { "s" }));
    }
    Ok(json!({
        "name": package.name,
        "tagline": package.tagline,
        "summary": package.summary,
        "bots": bots,
        "channels": rooms,
        "chief": chief_key(package).map(|k| json!({ "key": k, "name": name_of(&k) })),
        "leftOut": left_out,
        "safety": "Imported bots start out asking before they act, on Catbot's default AI. A team file never brings chats, keys or approval levels.",
    }))
}

/// The file itself: given as text, or fetched from a link (a GitHub page is read raw).
async fn source(body: &Value) -> Result<String, String> {
    if let Some(text) = body["text"].as_str().filter(|t| !t.trim().is_empty()) {
        if text.len() > MOST_BYTES {
            return Err("that file is too big for a team file".into());
        }
        return Ok(text.to_string());
    }
    let url = body["url"].as_str().map(str::trim).filter(|u| !u.is_empty()).ok_or("paste a team file, or give its link")?;
    if !url.starts_with("https://") {
        return Err("team links start with https://".into());
    }
    let url = match url.strip_prefix("https://github.com/").and_then(|rest| rest.split_once("/blob/")) {
        Some((repo, path)) => format!("https://raw.githubusercontent.com/{repo}/{path}"),
        None => url.to_string(),
    };
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().map_err(|e| e.to_string())?;
    let answer = client.get(&url).send().await.map_err(|e| format!("couldn't fetch it: {e}"))?;
    if !answer.status().is_success() {
        return Err(format!("couldn't fetch it: {}", answer.status()));
    }
    let bytes = answer.bytes().await.map_err(|e| format!("couldn't fetch it: {e}"))?;
    if bytes.len() > MOST_BYTES {
        return Err("that file is too big for a team file".into());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "that link isn't a text file".into())
}

/// `POST /api/teams/preview` `{ text | url }`: what importing it would do.
pub async fn preview(app: &Arc<App>, body: &Value) -> Answer {
    let text = source(body).await.map_err(bad)?;
    let read = read(&text).map_err(bad)?;
    let plan = plan(app, &read).map_err(bad)?;
    Ok((StatusCode::OK, json!({ "plan": plan, "text": text })))
}

/// `POST /api/teams/import` `{ text, leaveOut: [bot keys], makeChief }`: makes the team.
pub fn import(app: &Arc<App>, body: &Value) -> Answer {
    let text = body["text"].as_str().ok_or_else(|| bad("the team file's text"))?;
    if text.len() > MOST_BYTES {
        return Err(bad("that file is too big for a team file"));
    }
    let read = read(text).map_err(bad)?;
    let package = &read.package;
    let leave: Vec<&str> = body["leaveOut"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut made: Vec<(String, Bot)> = Vec::new();
    for agent in package.agents.iter().filter(|a| !leave.contains(&key_of(a).as_str())) {
        let color = agent.appearance.color.clone().filter(|c| crate::store::COLORS.contains(&c.as_str()));
        let name = if agent.name.trim().is_empty() { key_of(agent) } else { agent.name.trim().to_string() };
        let mut wanted = json!({ "name": name, "soul": instructions(agent, package), "approvalMode": "ask" });
        if let Some(color) = color {
            wanted["color"] = json!(color);
        }
        let bot = super::make_bot(app, &wanted)?;
        // The team's playbook, where the bot can read it.
        if !read.body.trim().is_empty() {
            let _ = std::fs::create_dir_all(&bot.folder);
            if let Err(err) = std::fs::write(bot.folder.join("TEAM.md"), &read.body) {
                eprintln!("catbot: couldn't give {} the team's playbook: {err}", bot.name);
            }
        }
        made.push((key_of(agent), bot));
    }
    let id_of = |key: &str| made.iter().find(|(k, _)| k == key).map(|(_, b)| b.id.clone());
    let mut rooms = Vec::new();
    for room in &package.rooms {
        let members: Vec<String> = room.members.iter().filter_map(|m| id_of(m)).collect();
        if members.is_empty() {
            continue;
        }
        let lead = lead_key(room).and_then(|k| id_of(&k)).filter(|l| members.contains(l));
        let name = if room.name.trim().is_empty() { room.key.clone() } else { room.name.clone() };
        let (_, answer) = channels::create(app, &json!({ "name": name, "members": members, "lead": lead }))?;
        if let Some(id) = answer["bot"]["id"].as_str() {
            if !room.bulletin.trim().is_empty() {
                let channel = app.try_store(|store| store.channel(id)).map_err(fail)?.ok_or_else(|| bad("the new channel vanished"))?;
                channels::change(app, channel, json!({ "soul": room.bulletin }))?;
            }
            if let Some(channel) = app.with_store(|store| store.channel(id)).flatten()
                && !read.body.trim().is_empty()
            {
                let _ = std::fs::create_dir_all(&channel.folder);
                let _ = std::fs::write(channel.folder.join("TEAM.md"), &read.body);
            }
        }
        rooms.push(name);
    }
    let mut chief = None;
    if body["makeChief"] == true
        && let Some(id) = chief_key(package).and_then(|k| id_of(&k))
    {
        super::chief::make_chief(app, &id);
        chief = made.iter().find(|(_, b)| b.id == id).map(|(_, b)| b.name.clone());
    }
    eprintln!("catbot: imported the team {} ({} bots, {} channels)", package.name, made.len(), rooms.len());
    Ok((StatusCode::CREATED, json!({ "bots": made.iter().map(|(_, b)| &b.name).collect::<Vec<_>>(), "channels": rooms, "chief": chief })))
}

/// Takes out anything that looks like a key or token, so a shared file never carries one.
fn scrub(text: &str) -> String {
    const PREFIXES: [&str; 11] = ["sk-", "sk_live_", "sk_test_", "xai-", "ghp_", "gho_", "github_pat_", "AKIA", "AIza", "xoxb-", "xoxp-"];
    text.split_inclusive(char::is_whitespace)
        .map(|piece| {
            let word = piece.trim_end().trim_matches(|c: char| "\"'`()[]{}<>,;".contains(c));
            let looks_secret = word.len() >= 20 && (PREFIXES.iter().any(|p| word.starts_with(p)) || (word.len() >= 32 && word.chars().all(|c| c.is_ascii_hexdigit())));
            if looks_secret { piece.replace(word, "[removed]") } else { piece.to_string() }
        })
        .collect()
}

fn slug(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "team".into() } else { s }
}

/// The team as a file: the chosen bots, the channels among them, and the Chief if it's one of them.
fn write(app: &App, name: &str, tagline: &str, author: &str, ids: Option<Vec<String>>) -> Result<String, String> {
    let (bots, all_channels) = app.with_store(|store| Ok((store.bots()?, store.channels()?))).ok_or("couldn't read the team")?;
    let bots: Vec<Bot> = bots.into_iter().filter(|b| ids.as_ref().is_none_or(|ids| ids.contains(&b.id))).collect();
    if bots.is_empty() {
        return Err("pick at least one bot".into());
    }
    let key = |b: &Bot| slug(&b.name);
    let agents: Vec<Value> = bots
        .iter()
        .map(|b| {
            let (description, title) = role_of(&b.instructions);
            json!({ "key": key(b), "name": b.name, "title": title, "description": scrub(&description), "appearance": { "color": b.color.clone().unwrap_or_else(|| "blue".into()) } })
        })
        .collect();
    let rooms: Vec<Value> = all_channels
        .iter()
        .filter_map(|c| {
            let members: Vec<&Bot> = c.members.iter().filter_map(|m| bots.iter().find(|b| &b.id == m)).collect();
            if members.is_empty() {
                return None;
            }
            let lead = c.lead.as_ref().and_then(|l| bots.iter().find(|b| &b.id == l)).map(key);
            let mut room = json!({ "key": slug(&c.name), "name": c.name, "members": members.iter().map(|b| key(b)).collect::<Vec<_>>(), "bulletin": scrub(c.instructions.trim()) });
            if let Some(lead) = lead {
                room["defaultResponder"] = json!({ "kind": "agent", "agent": lead });
            }
            Some(room)
        })
        .collect();
    let chief = super::chief::chief(app).filter(|c| bots.iter().any(|b| b.id == c.id));
    let name = if name.trim().is_empty() { "My Catbot team" } else { name.trim() };
    let tagline = if tagline.trim().is_empty() { format!("A team of {} bots.", bots.len()) } else { tagline.trim().to_string() };
    let mut head = json!({
        "botmrr": 1,
        "id": slug(name),
        "release": "1.0.0",
        "name": name,
        "tagline": tagline,
        "summary": tagline,
        "category": "Custom",
        "agents": agents,
        "rooms": rooms,
        "routines": [],
        "playbooks": [],
    });
    if !author.trim().is_empty() {
        head["author"] = json!({ "name": author.trim() });
    }
    if let Some(chief) = &chief {
        head["chiefOfStaff"] = json!(key(chief));
    }
    let yaml = serde_yaml_ng::to_string(&in_order(&head)?).map_err(|e| e.to_string())?;
    let mut md = format!("---\n{yaml}---\n\n# {name}\n\n{tagline}\n\n> **Give this file to your Chief of Staff.** It is the complete team blueprint. Any agent system can run it; Catbot and OpenMausBot can also import it directly.\n\n");
    md.push_str("## Activation\n\nYou are the Chief of Staff for this blueprint. Read the whole document before acting. Confirm the person's goal and any missing inputs, then create or delegate to the roles below, keeping their names, ownership and shared-room rules. If your platform cannot create agents, perform the roles one at a time and keep their work clearly separated.\n\nNever ask for pasted passwords or secret keys. Do not send messages, publish, spend money, delete data or start a schedule without the person's explicit approval.\n\n");
    md.push_str(&format!("## Mission\n\n{tagline}\n\n## Outcomes\n\n- The person gets one consolidated answer for each request, checked before it's called done.\n\n## Connections\n\nNone required.\n\n## Team\n\n"));
    for b in &bots {
        md.push_str(&format!("### {}\n\n{}\n\n", b.name, if b.instructions.trim().is_empty() { "(No instructions yet.)".to_string() } else { scrub(b.instructions.trim()) }));
    }
    if !rooms.is_empty() {
        md.push_str("## Shared rooms\n\n");
        for c in all_channels.iter().filter(|c| c.members.iter().any(|m| bots.iter().any(|b| &b.id == m))) {
            let members: Vec<&str> = c.members.iter().filter_map(|m| bots.iter().find(|b| &b.id == m)).map(|b| b.name.as_str()).collect();
            let lead = c.lead.as_ref().and_then(|l| bots.iter().find(|b| &b.id == l)).map(|b| b.name.clone()).unwrap_or_default();
            md.push_str(&format!("### {}\n\nMembers: {}. Lead: {lead}.\n\n{}\n\n", c.name, members.join(", "), scrub(c.instructions.trim())));
        }
    }
    md.push_str(&match &chief {
        Some(c) => format!("## Chief of Staff\n\n{} is the Chief of Staff: the person's one point of contact. It hands work to the role best placed for it, checks what comes back, and returns one answer.\n\n", c.name),
        None => "## Chief of Staff\n\nWhoever runs this file acts as Chief of Staff: the person's one point of contact, handing work to the right role and returning one answer.\n\n".to_string(),
    });
    md.push_str("## Completion rule\n\nA request is done only when the work is finished, checked, and reported to the person in one answer that says what was done and what, if anything, is still open.\n");
    Ok(md)
}

/// Fields in the order MausBot writes them (not alphabetical), all the way down.
fn in_order(value: &Value) -> Result<serde_yaml_ng::Value, String> {
    const ORDER: [&str; 22] = [
        "botmrr", "id", "release", "key", "kind", "agent", "name", "tagline", "summary", "category", "author", "title", "description",
        "appearance", "color", "agents", "chiefOfStaff", "rooms", "members", "bulletin", "defaultResponder", "routines",
    ];
    Ok(match value {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by_key(|k| ORDER.iter().position(|f| f == k).unwrap_or(ORDER.len()));
            let mut map = serde_yaml_ng::Mapping::new();
            for k in keys {
                map.insert(k.as_str().into(), in_order(&o[k])?);
            }
            serde_yaml_ng::Value::Mapping(map)
        }
        Value::Array(items) => serde_yaml_ng::Value::Sequence(items.iter().map(in_order).collect::<Result<_, _>>()?),
        other => serde_yaml_ng::to_value(other).map_err(|e| e.to_string())?,
    })
}

/// A bot's instructions as a team role: without the lines an import added (its title, where the
/// playbook is), and its title back out of them.
fn role_of(instructions: &str) -> (String, String) {
    let mut title = String::new();
    let mut kept = Vec::new();
    for part in instructions.trim().split("\n\n") {
        if part.trim() == "The team's whole playbook is in TEAM.md in your folder." {
            continue;
        }
        if let Some(rest) = part.trim().strip_prefix("Your role on the ")
            && let Some((_, role)) = rest.split_once(" team: ")
        {
            title = role.trim_end_matches('.').to_string();
            continue;
        }
        kept.push(part);
    }
    (kept.join("\n\n").trim().to_string(), title)
}

/// `POST /api/teams/export` `{ name, tagline, author, botIds, save }`: the team file; with `save`,
/// also saved in this computer's Downloads folder.
pub fn export(app: &App, body: &Value) -> Answer {
    let ids = body["botIds"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let name = body["name"].as_str().unwrap_or_default();
    let markdown = write(app, name, body["tagline"].as_str().unwrap_or_default(), body["author"].as_str().unwrap_or_default(), ids).map_err(bad)?;
    let filename = format!("{}.md", slug(if name.trim().is_empty() { "My Catbot team" } else { name }));
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from);
    let folder = home.map(|h| h.join("Downloads")).filter(|d| body["save"] == true && d.is_dir());
    let saved = folder.and_then(|folder| {
        let path = folder.join(&filename);
        std::fs::write(&path, &markdown).ok().map(|_| path.display().to_string())
    });
    Ok((StatusCode::OK, json!({ "markdown": markdown, "filename": filename, "savedTo": saved })))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "---\nbotmrr: 1\nid: garden\nname: Garden Crew\ntagline: Grow things.\nagents:\n  - key: ada\n    name: Ada\n    title: Lead Gardener\n    description: Plans the beds.\n    appearance:\n      color: purple\n    playbooks: [plan]\n  - key: lin\n    name: Lin\n    description: Waters.\nchiefOfStaff: ada\nrooms:\n  - key: shed\n    name: Shed\n    members: [ada, lin]\n    bulletin: Keep it tidy.\n    defaultResponder:\n      kind: agent\n      agent: lin\nroutines:\n  - key: daily\nplaybooks:\n  - key: plan\n    name: Bed Plan\n    summary: Lay out the beds.\n    instructions: Measure first.\n---\n\n# Garden Crew\n\n## Activation\n\nRead it all.\n";

    #[test]
    fn a_team_file_reads() {
        let read = read(FILE).unwrap();
        let p = &read.package;
        assert_eq!((p.name.as_str(), p.agents.len(), p.rooms.len()), ("Garden Crew", 2, 1));
        assert_eq!(chief_key(p).as_deref(), Some("ada"));
        assert_eq!(lead_key(&p.rooms[0]).as_deref(), Some("lin"));
        assert!(read.body.starts_with("# Garden Crew"));
        let ada = instructions(&p.agents[0], p);
        assert!(ada.starts_with("Plans the beds.") && ada.contains("Your role on the Garden Crew team: Lead Gardener.") && ada.contains("Bed Plan: Lay out the beds.\nMeasure first."), "{ada}");
        assert!(super::read("no yaml here").is_err());
    }

    #[test]
    fn an_imported_bot_goes_back_out_as_the_role_it_came_as() {
        let read = read(FILE).unwrap();
        let (description, title) = role_of(&instructions(&read.package.agents[0], &read.package));
        assert_eq!(title, "Lead Gardener");
        assert!(description.starts_with("Plans the beds.") && !description.contains("TEAM.md") && !description.contains("Your role"), "{description}");
    }

    #[test]
    fn keys_and_tokens_are_taken_out() {
        let said = scrub("Use sk-abcdefghijklmnopqrstuvwxyz123 and ghp_ABCDEFGHIJKLMNOPQRSTUV, not skipping words.");
        assert_eq!(said, "Use [removed] and [removed], not skipping words.");
        assert_eq!(slug("Ship Software, Safely!"), "ship-software-safely");
    }
}
