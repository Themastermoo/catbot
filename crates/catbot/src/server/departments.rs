//! Departments and the Team map (stage 19.6), after MausBot 0.1.176's sidebar sections and
//! `TeamMapPage`.
//!
//! Moo's choices (2026-11-23): departments group bots in the sidebar and show as boxes on the
//! Team map; each has shared instructions (shown to its bots every turn, written only by the
//! person), a shared memory note, and may have a chief of its own under the Chief of Staff. A
//! message typed on the map goes to the bots it @tags, every bot of a tagged department, or, for
//! `@everyone`, to the Chief of Staff, who passes it on (to everyone when there's no Chief).
//!
//! Kept in the settings: `departments` (the list, in order, and which bot is in which; a bot in
//! none is in General) and `team-map` (where the map's boxes sit, and who has handed work to whom).
//!
//! * `GET    /api/departments`                  the departments and their bots
//! * `POST   /api/departments`                  `{ name, botIds }`: a new one, bots moved in
//! * `PATCH  /api/departments/:name`            `{ name?, instructions?, chief?, position? }`
//! * `PUT    /api/departments/:name/members`    `{ add: [ids], remove: [ids] }` (General: "General")
//! * `DELETE /api/departments/:name`            its bots go to General; its memory note stays
//! * `GET    /api/team-map`                     hand-offs between bots, and the boxes' places
//! * `PUT    /api/team-map/layout`              the boxes' places
//! * `POST   /api/team-map/message`             `{ text }`: sent to whoever it @tags

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::store::{Bot, Store};

const KEY: &str = "departments";
const MAP: &str = "team-map";
/// What every bot outside a department is in.
pub const GENERAL: &str = "General";
pub const NAME_MAX: usize = 60;
pub const INSTRUCTIONS_MAX: usize = 24_000;
/// Hand-offs the map remembers.
const EDGES_KEPT: usize = 200;

/// One change at a time.
static CHANGING: Mutex<()> = Mutex::new(());
/// Hand-offs under way: (from bot, to bot) → the chat.
static RUNNING: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Department {
    pub name: String,
    /// What every bot in it is shown at the start of each turn.
    pub instructions: String,
    /// When the instructions were last saved, in ms.
    pub updated_at: Option<i64>,
    /// The bot leading it, one of its own.
    pub chief: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Departments {
    pub list: Vec<Department>,
    /// Bot id → its department's name.
    pub members: BTreeMap<String, String>,
}

impl Departments {
    pub fn of(&self, bot_id: &str) -> Option<&Department> {
        let name = self.members.get(bot_id)?;
        self.list.iter().find(|d| &d.name == name)
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.list.iter().position(|d| d.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Moves a bot into a department (None: General). It stops leading the one it leaves.
    fn put(&mut self, bot_id: &str, to: Option<&str>) {
        if let Some(old) = self.members.get(bot_id).cloned()
            && Some(old.as_str()) != to
            && let Some(dept) = self.list.iter_mut().find(|d| d.name == old)
            && dept.chief.as_deref() == Some(bot_id)
        {
            dept.chief = None;
        }
        match to {
            Some(name) => self.members.insert(bot_id.to_string(), name.to_string()),
            None => self.members.remove(bot_id),
        };
    }
}

pub fn load(store: &Store) -> Departments {
    let mut kept: Departments = store.setting(KEY).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    // Bots that are gone, and memberships of departments that are gone, are let go.
    let bots: Vec<String> = store.bots().unwrap_or_default().into_iter().map(|b| b.id).collect();
    let names: Vec<String> = kept.list.iter().map(|d| d.name.clone()).collect();
    kept.members.retain(|id, name| bots.contains(id) && names.contains(name));
    for dept in kept.list.iter_mut() {
        if dept.chief.as_ref().is_some_and(|c| kept.members.get(c) != Some(&dept.name)) {
            dept.chief = None;
        }
    }
    kept
}

pub fn departments(app: &App) -> Departments {
    app.with_store(|store| Ok(load(store))).unwrap_or_default()
}

/// What the wire says of a bot: its department ("" for General) and whether it leads it.
pub fn wire_of(store: &Store, bot_id: &str) -> (String, bool) {
    let kept = load(store);
    match kept.of(bot_id) {
        Some(dept) => (dept.name.clone(), dept.chief.as_deref() == Some(bot_id)),
        None => (String::new(), false),
    }
}

/// A department's name as given, or why it can't be one.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("a department needs a name".into());
    }
    if name.chars().count() > NAME_MAX {
        return Err(format!("a department's name is at most {NAME_MAX} characters"));
    }
    if name.eq_ignore_ascii_case(GENERAL) || name.eq_ignore_ascii_case("everyone") {
        return Err(format!("“{name}” is taken; choose another name"));
    }
    if name.contains(['/', '\\', '@', '#']) || name.starts_with('.') {
        return Err("a department's name can't have / \\ @ or #, or start with a dot".into());
    }
    Ok(name)
}

/// Where a department's shared memory note is, in the memory vault.
pub fn memory_note(name: &str) -> String {
    format!("Departments/{name}.md")
}

fn ensure_memory_note(app: &App, dept: &str) {
    let Some(vault) = &app.memory else { return };
    let path = vault.join(memory_note(dept));
    if path.exists() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let today = chrono::Local::now().format("%Y-%m-%d");
    let text = format!(
        "---\ntype: department\ntags: [catbot, department]\ncreated: {today}\n---\n\n# {dept}\n\n\
         The {dept} department's shared notes: what it's for, what it's working on, and what its bots have learned. \
         Every bot in the department reads and adds to this note.\n"
    );
    let _ = std::fs::write(path, text);
}

fn rename_memory_note(app: &App, from: &str, to: &str) {
    let Some(vault) = &app.memory else { return };
    let (old, new) = (vault.join(memory_note(from)), vault.join(memory_note(to)));
    if old.exists() && !new.exists() {
        let _ = std::fs::rename(old, new);
    }
}

/// Saves a change, and shows it: a `departments` frame, and the bots whose department changed.
fn change(app: &App, f: impl FnOnce(&Store, &mut Departments) -> Result<(), (StatusCode, String)>) -> Result<Departments, (StatusCode, String)> {
    let (before, after) = {
        let _one = CHANGING.lock().expect("departments lock");
        let (before, after) = app
            .try_store(|store| {
                let before = load(store);
                let mut after = before.clone();
                Ok((before, f(store, &mut after).map(|_| after)))
            })
            .map_err(fail)?;
        let after = after?;
        app.try_store(|store| store.set_setting(KEY, &json!(after))).map_err(fail)?;
        (before, after)
    };
    let touched: Vec<String> = app
        .with_store(|store| store.bots())
        .unwrap_or_default()
        .into_iter()
        .map(|b| b.id)
        .filter(|id| {
            let was = before.of(id).map(|d| (d.name.clone(), d.chief.as_deref() == Some(id)));
            let is = after.of(id).map(|d| (d.name.clone(), d.chief.as_deref() == Some(id)));
            was != is
        })
        .collect();
    for id in touched {
        app.announce_bot(&id);
    }
    app.hub.send(json!({ "kind": "departments" }));
    // Each department's team chat follows its bots, lead and name (stage 30.4).
    super::team_chat::sync(app);
    Ok(after)
}

/// Sets a department's shared instructions (its team chat's pinned notes, stage 30.4).
pub fn set_instructions(app: &App, name: &str, text: &str) -> Result<(), (StatusCode, String)> {
    if text.len() > INSTRUCTIONS_MAX {
        return Err(bad(format!("shared instructions are at most {INSTRUCTIONS_MAX} bytes")));
    }
    change(app, |_, kept| {
        let at = kept.find(name).ok_or((StatusCode::NOT_FOUND, "no such department".to_string()))?;
        kept.list[at].instructions = text.to_string();
        kept.list[at].updated_at = Some(chrono::Utc::now().timestamp_millis());
        Ok(())
    })
    .map(|_| ())
}

fn wire(app: &App, kept: &Departments) -> Value {
    let bots = app.with_store(|store| store.bots()).unwrap_or_default();
    let list: Vec<Value> = kept
        .list
        .iter()
        .map(|d| {
            let members: Vec<&str> = bots.iter().filter(|b| kept.members.get(&b.id) == Some(&d.name)).map(|b| b.id.as_str()).collect();
            json!({
                "name": d.name,
                "instructions": d.instructions,
                "updatedAt": d.updated_at,
                "chief": d.chief,
                "members": members,
                "memoryNote": app.memory.as_ref().map(|_| memory_note(&d.name)),
            })
        })
        .collect();
    let general: Vec<&str> = bots.iter().filter(|b| !kept.members.contains_key(&b.id)).map(|b| b.id.as_str()).collect();
    json!({ "departments": list, "general": general, "instructionsMax": INSTRUCTIONS_MAX })
}

fn ids(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn known_bots(store: &Store, wanted: &[String]) -> Result<(), (StatusCode, String)> {
    let bots = store.bots().map_err(fail)?;
    match wanted.iter().find(|id| !bots.iter().any(|b| &b.id == *id)) {
        Some(id) => Err((StatusCode::NOT_FOUND, format!("no bot {id}"))),
        None => Ok(()),
    }
}

/// The departments' routes.
pub fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or(Value::Null);
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, wire(app, &departments(app)))),
        (&Method::POST, []) => {
            let name = check_name(body["name"].as_str().unwrap_or_default()).map_err(bad)?;
            let bots = ids(&body["botIds"]);
            let kept = change(app, |store, kept| {
                known_bots(store, &bots)?;
                if kept.find(&name).is_some() {
                    return Err((StatusCode::CONFLICT, format!("there's already a department called {name}")));
                }
                kept.list.push(Department { name: name.clone(), ..Department::default() });
                for id in &bots {
                    kept.put(id, Some(&name));
                }
                Ok(())
            })?;
            ensure_memory_note(app, &name);
            Ok((StatusCode::CREATED, wire(app, &kept)))
        }
        (&Method::PATCH, [name]) => {
            let mut renamed = None;
            let kept = change(app, |store, kept| {
                let at = kept.find(name).ok_or((StatusCode::NOT_FOUND, "no such department".to_string()))?;
                let old = kept.list[at].name.clone();
                if let Some(text) = body["instructions"].as_str() {
                    if text.len() > INSTRUCTIONS_MAX {
                        return Err(bad(format!("shared instructions are at most {INSTRUCTIONS_MAX} bytes")));
                    }
                    kept.list[at].instructions = text.to_string();
                    kept.list[at].updated_at = Some(chrono::Utc::now().timestamp_millis());
                }
                if let Some(chief) = body.get("chief") {
                    match chief.as_str() {
                        None => kept.list[at].chief = None,
                        Some(id) => {
                            known_bots(store, &[id.to_string()])?;
                            if kept.members.get(id) != Some(&old) {
                                return Err(bad("a department's chief has to be one of its bots"));
                            }
                            kept.list[at].chief = Some(id.to_string());
                        }
                    }
                }
                if let Some(position) = body["position"].as_u64() {
                    let dept = kept.list.remove(at);
                    let to = (position as usize).min(kept.list.len());
                    kept.list.insert(to, dept);
                }
                if let Some(new) = body["name"].as_str() {
                    let new = check_name(new).map_err(bad)?;
                    if new != old {
                        if kept.find(&new).is_some_and(|i| !kept.list[i].name.eq_ignore_ascii_case(&old)) {
                            return Err((StatusCode::CONFLICT, format!("there's already a department called {new}")));
                        }
                        if let Some(dept) = kept.list.iter_mut().find(|d| d.name == old) {
                            dept.name = new.clone();
                        }
                        for member in kept.members.values_mut().filter(|m| **m == old) {
                            *member = new.clone();
                        }
                        // Its team chat goes with it (stage 30.4).
                        super::team_chat::renamed_in(store, &old, &new);
                        renamed = Some((old, new));
                    }
                }
                Ok(())
            })?;
            if let Some((old, new)) = renamed {
                rename_memory_note(app, &old, &new);
                rename_layout(app, &old, Some(&new));
                // Every member's wire changes with the name.
                for (id, _) in kept.members.iter().filter(|(_, d)| **d == new) {
                    app.announce_bot(id);
                }
            }
            Ok((StatusCode::OK, wire(app, &kept)))
        }
        (&Method::PUT, [name, "members"]) => {
            let (add, remove) = (ids(&body["add"]), ids(&body["remove"]));
            let general = name.eq_ignore_ascii_case(GENERAL);
            let kept = change(app, |store, kept| {
                known_bots(store, &add)?;
                let target = if general {
                    None
                } else {
                    Some(kept.list[kept.find(name).ok_or((StatusCode::NOT_FOUND, "no such department".to_string()))?].name.clone())
                };
                for id in &add {
                    kept.put(id, target.as_deref());
                }
                for id in &remove {
                    if kept.members.get(id) == target.as_ref() {
                        kept.put(id, None);
                    }
                }
                Ok(())
            })?;
            Ok((StatusCode::OK, wire(app, &kept)))
        }
        (&Method::DELETE, [name]) => {
            let mut gone = String::new();
            let kept = change(app, |_, kept| {
                let at = kept.find(name).ok_or((StatusCode::NOT_FOUND, "no such department".to_string()))?;
                gone = kept.list.remove(at).name;
                kept.members.retain(|_, d| *d != gone);
                Ok(())
            })?;
            rename_layout(app, &gone, None);
            Ok((StatusCode::OK, wire(app, &kept)))
        }
        _ => Err((StatusCode::NOT_FOUND, "departments don't do that".into())),
    }
}

/// What a bot in a department is told every turn: its department and teammates, who leads it,
/// the person's shared instructions, and its shared memory note.
pub fn note(app: &App, bot: &Bot) -> String {
    let kept = departments(app);
    let Some(dept) = kept.of(&bot.id).cloned() else { return String::new() };
    let bots = app.with_store(|store| store.bots()).unwrap_or_default();
    let name_of = |id: &str| bots.iter().find(|b| b.id == id).map(|b| b.name.clone());
    let others: Vec<String> = bots.iter().filter(|b| b.id != bot.id && kept.members.get(&b.id) == Some(&dept.name)).map(|b| b.name.clone()).collect();
    let mut note = format!("\n\nYou're in the {} department.", dept.name);
    if !others.is_empty() {
        note.push_str(&format!(" Its other bots: {}.", others.join(", ")));
    }
    match dept.chief.as_deref() {
        Some(id) if id == bot.id => note.push_str(&format!(
            " You lead it: work that comes to the department is yours to share out. Hand a part to one of its bots by \
             ending your reply with @Name and a clear brief; check their answer before you call the work done. The \
             Chief of Staff{} is over the whole team.",
            super::chief::chief(app).filter(|c| c.id != bot.id).map(|c| format!(", {},", c.name)).unwrap_or_default()
        )),
        Some(id) => {
            if let Some(chief) = name_of(id) {
                note.push_str(&format!(" {chief} leads it; when you aren't sure whose work something is, hand it to @{chief}."));
            }
        }
        None => {}
    }
    if !dept.instructions.trim().is_empty() {
        note.push_str(&format!(
            "\n\nThe person's shared instructions for the {} department (only they can change these):\n{}",
            dept.name,
            dept.instructions.trim()
        ));
    }
    if app.memory.is_some() {
        ensure_memory_note(app, &dept.name);
        note.push_str(&format!(
            "\n\nThe department's shared note is {} in your memory vault. Read it at the start of a conversation, and \
             add what the department's bots should all know (decisions, ongoing work); don't overwrite what others wrote.",
            memory_note(&dept.name)
        ));
    }
    note
}

/// The department a bot's in, for the Chief of Staff's list of the team.
pub fn label(kept: &Departments, bot_id: &str) -> String {
    match kept.of(bot_id) {
        Some(d) if d.chief.as_deref() == Some(bot_id) => format!(", leads {}", d.name),
        Some(d) => format!(", in {}", d.name),
        None => String::new(),
    }
}

// The Team map.

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Map {
    /// Where the boxes and bots sit: department name (or "General") / bot id → { x, y }.
    layout: Value,
    edges: Vec<Edge>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Edge {
    from: String,
    to: String,
    thread: String,
    last_at: i64,
    count: u64,
}

fn map(store: &Store) -> Map {
    store.setting(MAP).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn keep_map(store: &Store, map: &Map) {
    let _ = store.set_setting(MAP, &json!(map));
}

fn rename_layout(app: &App, from: &str, to: Option<&str>) {
    let _one = CHANGING.lock().expect("departments lock");
    app.with_store(|store| {
        let mut kept = map(store);
        if let Some(boxes) = kept.layout.get_mut("departments").and_then(Value::as_object_mut)
            && let Some(at) = boxes.remove(from)
            && let Some(to) = to
        {
            boxes.insert(to.to_string(), at);
        }
        keep_map(store, &kept);
        Ok(())
    });
}

/// A bot handed work to another: remembered for the map, and shown running until `done`.
pub fn handed(app: &App, from: &str, to: &str, thread: &str) {
    {
        let mut running = RUNNING.lock().expect("running lock");
        running.retain(|(f, t, _)| !(f == from && t == to));
        running.push((from.into(), to.into(), thread.into()));
    }
    {
        let _one = CHANGING.lock().expect("departments lock");
        app.with_store(|store| {
            let mut kept = map(store);
            let now = chrono::Utc::now().timestamp_millis();
            match kept.edges.iter_mut().find(|e| e.from == from && e.to == to) {
                Some(edge) => {
                    edge.last_at = now;
                    edge.count += 1;
                    edge.thread = thread.into();
                }
                None => kept.edges.push(Edge { from: from.into(), to: to.into(), thread: thread.into(), last_at: now, count: 1 }),
            }
            kept.edges.sort_by_key(|e| std::cmp::Reverse(e.last_at));
            kept.edges.truncate(EDGES_KEPT);
            keep_map(store, &kept);
            Ok(())
        });
    }
    app.hub.send(json!({ "kind": "team-map" }));
}

/// A hand-off's turn ended.
pub fn hand_done(app: &App, to: &str, thread: &str) {
    let removed = {
        let mut running = RUNNING.lock().expect("running lock");
        let before = running.len();
        running.retain(|(_, t, th)| !(t == to && th == thread));
        before != running.len()
    };
    if removed {
        app.hub.send(json!({ "kind": "team-map" }));
    }
}

/// The map's routes.
pub async fn map_route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => {
            let kept = app.with_store(|store| Ok(map(store))).unwrap_or_default();
            let running = RUNNING.lock().expect("running lock").clone();
            let edges: Vec<Value> = kept
                .edges
                .iter()
                .map(|e| {
                    let live = running.iter().any(|(f, t, _)| *f == e.from && *t == e.to);
                    json!({ "from": e.from, "to": e.to, "threadId": e.thread, "lastAt": e.last_at, "count": e.count, "state": if live { "running" } else { "connected" } })
                })
                .collect();
            Ok((StatusCode::OK, json!({ "edges": edges, "layout": kept.layout })))
        }
        (&Method::PUT, ["layout"]) => {
            let layout = body.unwrap_or(Value::Null);
            if !layout.is_object() || layout.to_string().len() > 200_000 {
                return Err(bad("the map's layout is an object"));
            }
            {
                let _one = CHANGING.lock().expect("departments lock");
                app.try_store(|store| {
                    let mut kept = map(store);
                    kept.layout = layout.clone();
                    keep_map(store, &kept);
                    Ok(())
                })
                .map_err(fail)?;
            }
            Ok((StatusCode::OK, json!({ "layout": layout })))
        }
        (&Method::POST, ["message"]) => {
            let text = body.as_ref().and_then(|b| b["text"].as_str()).unwrap_or_default().trim().to_string();
            message(app, &text)
        }
        _ => Err((StatusCode::NOT_FOUND, "the team map doesn't do that".into())),
    }
}

/// Who a message typed on the map is for.
#[derive(Debug, Default, PartialEq)]
pub struct Targets {
    /// Every bot it goes to, in the order first tagged.
    pub bots: Vec<String>,
    /// Bots it goes to as one of a tagged department: bot id → department.
    pub via_department: HashMap<String, String>,
    pub everyone: bool,
    /// @words that match nobody.
    pub unknown: Vec<String>,
    /// The message without its tags.
    pub words: String,
}

/// Reads the @tags of a message: a bot's name, a department's, or `everyone`, the longest known
/// name winning, never running on into a longer word (their `mapMessageTargets`).
pub fn targets(text: &str, bots: &[(String, String)], kept: &Departments) -> Targets {
    enum Tag {
        Bot(String),
        Dept(String),
        Everyone,
    }
    let mut names: Vec<(String, Tag)> = bots.iter().map(|(id, name)| (name.clone(), Tag::Bot(id.clone()))).collect();
    names.extend(kept.list.iter().map(|d| (d.name.clone(), Tag::Dept(d.name.clone()))));
    names.push(("everyone".into(), Tag::Everyone));
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.chars().count()));
    let mut out = Targets::default();
    let mut spans = Vec::new();
    let mut at = 0;
    let continues = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
    while let Some(found) = text[at..].find('@') {
        let start = at + found;
        at = start + 1;
        if continues(text[..start].chars().next_back()) {
            continue;
        }
        let rest = &text[start + 1..];
        let hit = names.iter().find(|(name, _)| {
            rest.get(..name.len()).is_some_and(|s| s.eq_ignore_ascii_case(name)) && !continues(rest[name.len()..].chars().next())
        });
        match hit {
            Some((name, tag)) => {
                match tag {
                    Tag::Bot(id) => out.bots.push(id.clone()),
                    Tag::Dept(dept) => {
                        for (id, _) in bots.iter().filter(|(id, _)| kept.members.get(id) == Some(dept)) {
                            out.bots.push(id.clone());
                            out.via_department.entry(id.clone()).or_insert_with(|| dept.clone());
                        }
                    }
                    Tag::Everyone => out.everyone = true,
                }
                spans.push((start, start + 1 + name.len()));
                at = start + 1 + name.len();
            }
            None => {
                let word: String = rest.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
                if !word.is_empty() {
                    out.unknown.push(word);
                }
            }
        }
    }
    let mut words = text.to_string();
    for (from, to) in spans.into_iter().rev() {
        words.replace_range(from..to, "");
    }
    out.words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut seen = std::collections::HashSet::new();
    out.bots.retain(|id| seen.insert(id.clone()));
    out
}

/// Sends a message from the map to whoever it tags, each in its own chat with the person.
fn message(app: &Arc<App>, text: &str) -> Answer {
    if text.is_empty() {
        return Err(bad("nothing to send"));
    }
    let bots: Vec<Bot> = app.try_store(|store| store.bots()).map_err(fail)?;
    let kept = departments(app);
    let named: Vec<(String, String)> = bots.iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    let found = targets(text, &named, &kept);
    if !found.unknown.is_empty() && found.bots.is_empty() && !found.everyone {
        return Err(bad(format!("nobody here is called @{}", found.unknown[0])));
    }
    if found.bots.is_empty() && !found.everyone {
        return Err(bad("tag a bot, a department or @everyone with @"));
    }
    if found.words.is_empty() {
        return Err(bad("say something besides the tags"));
    }
    // @everyone goes to the Chief of Staff, who passes it on; with no Chief, to every bot.
    let chief = super::chief::chief(app);
    let mut sends: Vec<(Bot, String)> = Vec::new();
    let mut via = None;
    if found.everyone {
        match &chief {
            Some(chief) => {
                via = Some(chief.name.clone());
                sends.push((chief.clone(), format!(
                    "The person wrote to the whole team on the Team map: “{}”\n\nPass it on to the bots it concerns by handing off (@Name, with what they need), and answer what's yours.",
                    found.words
                )));
            }
            None => sends.extend(bots.iter().map(|b| (b.clone(), format!("(To everyone on the team.) {}", found.words)))),
        }
    }
    for id in &found.bots {
        if sends.iter().any(|(b, _)| &b.id == id) {
            continue;
        }
        let Some(bot) = bots.iter().find(|b| &b.id == id) else { continue };
        let said = match found.via_department.get(id) {
            Some(dept) => format!("(To the {dept} department.) {}", found.words),
            None => found.words.clone(),
        };
        sends.push((bot.clone(), said));
    }
    let mut sent = Vec::new();
    let mut failed = Vec::new();
    for (bot, said) in sends {
        // A bot still working gets it in a new chat, so nothing waits.
        let busy = app.with_store(|store| store.current_conversation(&bot.id)).flatten().is_some_and(|c| app.turns.busy(&c.id) || app.channel_busy(&c.id));
        let thread = if busy { app.with_store(|store| store.new_conversation(&bot.id)).map(|c| c.id) } else { None };
        let mut body = json!({ "text": said });
        if let Some(thread) = &thread {
            body["threadId"] = json!(thread);
        }
        match super::send_message(app, bot.clone(), body) {
            Ok(_) => sent.push(json!({ "botId": bot.id, "name": bot.name })),
            Err((_, why)) => failed.push(json!({ "botId": bot.id, "name": bot.name, "error": why })),
        }
    }
    Ok((StatusCode::OK, json!({ "sent": sent, "failed": failed, "via": via, "unknown": found.unknown })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(id: &str, name: &str) -> (String, String) {
        (id.into(), name.into())
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name("  Garden   crew "), Ok("Garden crew".into()));
        assert!(check_name("").is_err());
        assert!(check_name("general").is_err());
        assert!(check_name("Everyone").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name(&"x".repeat(NAME_MAX + 1)).is_err());
    }

    #[test]
    fn tags_find_bots_departments_and_everyone() {
        let bots = [bot("1", "Kiwi"), bot("2", "Scout"), bot("3", "Scout Two"), bot("4", "Quill")];
        let mut kept = Departments { list: vec![Department { name: "Garden".into(), ..Department::default() }], ..Departments::default() };
        kept.members.insert("2".into(), "Garden".into());
        kept.members.insert("4".into(), "Garden".into());
        let t = targets("@Scout Two and @garden: water the beds, @nobody", &bots, &kept);
        assert_eq!(t.bots, ["3", "2", "4"], "the longest name wins; a department brings its bots");
        assert_eq!(t.via_department.get("2").map(String::as_str), Some("Garden"));
        assert_eq!(t.unknown, ["nobody"]);
        assert_eq!(t.words, "and : water the beds, @nobody", "an unknown tag stays in the words");
        let t = targets("@everyone stand-up at 9, mail@kiwi.com", &bots, &kept);
        assert!(t.everyone && t.bots.is_empty(), "an address isn't a tag");
        assert_eq!(targets("@Kiwis hi", &bots, &kept).unknown, ["Kiwis"], "a name doesn't run on into a longer word");
    }

    #[test]
    fn moving_a_chief_out_clears_its_lead() {
        let mut kept = Departments { list: vec![Department { name: "Garden".into(), chief: Some("2".into()), ..Department::default() }], ..Departments::default() };
        kept.members.insert("2".into(), "Garden".into());
        kept.put("2", Some("Garden"));
        assert_eq!(kept.list[0].chief.as_deref(), Some("2"), "staying put keeps it");
        kept.put("2", None);
        assert_eq!(kept.list[0].chief, None);
    }
}
