//! Skills (stage 13.8): instructions for doing one kind of job well, in the open Agent Skills
//! format (a folder with a `SKILL.md`: YAML `name` and `description`, then the instructions), the
//! same as Claude Code and MausBot use.
//!
//! Catbot keeps one library (`CATBOT_HOME/skills/<name>/`), and each bot picks the skills it uses.
//! Before a turn, a bot's skills are put in its folder's `.claude/skills/`, where Claude Code
//! finds them on its own; other AIs are told where they are. `/name …` in the composer runs one
//! for that message. A bot can write a new skill; it's saved only if the person approves the card.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::store::{Bot, Role, Store};

pub const NAME_MAX: usize = 64;
pub const DESCRIPTION_MAX: usize = 1024;
pub const FILE_MAX: usize = 256 * 1024;
/// Commands the composer already has, which no skill may be called.
const TAKEN: [&str; 1] = ["image"];
/// Marks a skill folder Catbot put in a bot's folder, so Catbot only ever removes its own.
const PLACED: &str = ".catbot-skill";

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// The spec's rule: lowercase letters and digits with single hyphens. It's also what keeps a
/// name from reaching outside the library (no dots, no slashes).
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name.split('-').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

/// Reads a SKILL.md.
pub fn parse(text: &str) -> Result<Skill, String> {
    if text.len() > FILE_MAX {
        return Err("that SKILL.md is too big".into());
    }
    let text = text.trim_start_matches('\u{feff}').trim();
    let rest = text.strip_prefix("---").ok_or("a SKILL.md starts with its name and description between --- lines")?;
    let end = rest.find("\n---").ok_or("the YAML at the top isn't closed with ---")?;
    let head: Value = serde_yaml_ng::from_str(&rest[..end]).map_err(|e| format!("the YAML at the top doesn't read: {e}"))?;
    let name = head["name"].as_str().map(str::trim).unwrap_or_default().to_string();
    let description = head["description"].as_str().map(|d| d.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    if !is_name(&name) {
        return Err(format!("“{name}” isn't a skill name: lowercase letters, digits and single hyphens, at most {NAME_MAX}"));
    }
    if TAKEN.contains(&name.as_str()) {
        return Err(format!("/{name} is already one of Catbot's own commands"));
    }
    if description.is_empty() || description.len() > DESCRIPTION_MAX {
        return Err(format!("a skill needs a description of at most {DESCRIPTION_MAX} characters"));
    }
    let body = rest[end + 4..].trim_start_matches('-').trim().to_string();
    Ok(Skill { name, description, body })
}

fn library_dir(store: &Store) -> PathBuf {
    store.home().join("skills")
}

/// Every skill in the library, by name.
pub fn library(store: &Store) -> Vec<Skill> {
    let mut skills: Vec<Skill> = std::fs::read_dir(library_dir(store))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("SKILL.md")).ok())
        .filter_map(|text| parse(&text).ok())
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// The skills a bot uses (settings key `skills:<bot>`), as still in the library.
pub fn of(store: &Store, bot_id: &str) -> Vec<String> {
    let wanted: Vec<String> = store.setting(&format!("skills:{bot_id}")).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let have: Vec<String> = library(store).into_iter().map(|s| s.name).collect();
    wanted.into_iter().filter(|n| have.contains(n)).collect()
}

/// Sets the skills a bot uses (from a bot PATCH's `skills`).
pub fn set(store: &Store, bot_id: &str, list: &Value) -> Result<(), String> {
    let names: Vec<String> = list.as_array().ok_or("skills: a list of names")?.iter().filter_map(Value::as_str).map(str::to_string).collect();
    let have: Vec<String> = library(store).into_iter().map(|s| s.name).collect();
    if let Some(missing) = names.iter().find(|n| !have.contains(n)) {
        return Err(format!("there's no skill called {missing}"));
    }
    store.set_setting(&format!("skills:{bot_id}"), &json!(names)).map_err(|e| e.to_string())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Before a turn: the bot's skills in `<folder>/.claude/skills/`, fresh from the library, and
/// any Catbot put there before that the bot no longer uses taken away. Returns what the bot is
/// told about them (nothing for Claude, which finds them itself).
pub fn place(app: &App, bot: &Bot, folder: &Path) -> String {
    let Some((library_dir, names, skills)) = app.with_store(|store| Ok((library_dir(store), of(store, &bot.id), library(store)))) else { return String::new() };
    let target = folder.join(".claude").join("skills");
    if let Ok(entries) = std::fs::read_dir(&target) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().join(PLACED).is_file() && !names.contains(&name) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let mut placed = Vec::new();
    for name in &names {
        let to = target.join(name);
        // A skill of the bot's own making with the same name is the bot's: left alone.
        if to.exists() && !to.join(PLACED).is_file() {
            continue;
        }
        let _ = std::fs::remove_dir_all(&to);
        if copy_dir(&library_dir.join(name), &to).and_then(|_| std::fs::write(to.join(PLACED), "Put here by Catbot from its skills library; changes here are replaced.\n")).is_ok() {
            placed.push(name.clone());
        }
    }
    if placed.is_empty() || bot.engine == crate::engines::CLAUDE {
        return String::new();
    }
    let lines: Vec<String> = skills
        .iter()
        .filter(|s| placed.contains(&s.name))
        .map(|s| format!("- {}: {} (instructions: .claude/skills/{}/SKILL.md)", s.name, s.description, s.name))
        .collect();
    format!(
        "\n\nYou have skills: instructions for doing certain jobs well. When a job matches one, read its SKILL.md first and follow it.\n{}",
        lines.join("\n")
    )
}

/// `/name request` at the start of a message runs one of the bot's skills: the words the bot
/// reads name the skill and where its instructions are. Anything else is left as it is.
pub fn command(app: &App, bot: &Bot, text: &str) -> Option<String> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (name, request) = match rest.split_once(char::is_whitespace) {
        Some((name, request)) => (name, request.trim()),
        None => (rest, ""),
    };
    let name = name.trim_end_matches(|c: char| ".,;:!?".contains(c));
    if !is_name(name) || !app.with_store(|store| Ok(of(store, &bot.id))).unwrap_or_default().iter().any(|n| n == name) {
        return None;
    }
    let ask = if request.is_empty() { "Run it now.".to_string() } else { format!("The request: {request}") };
    Some(format!("Use your skill “{name}” for this: read .claude/skills/{name}/SKILL.md in your folder and follow it. {ask}"))
}

/// Saves a skill to the library (`replace` to write over one with the same name).
pub(super) fn save(store: &Store, text: &str, replace: bool) -> Result<Skill, String> {
    let skill = parse(text)?;
    let dir = library_dir(store).join(&skill.name);
    if dir.join("SKILL.md").exists() && !replace {
        return Err(format!("there's already a skill called {}", skill.name));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("SKILL.md"), text.trim_start_matches('\u{feff}').trim().to_string() + "\n").map_err(|e| e.to_string())?;
    Ok(skill)
}

fn wire(store: &Store) -> Value {
    let bots = store.bots().unwrap_or_default();
    let skills: Vec<Value> = library(store)
        .into_iter()
        .map(|s| {
            let users: Vec<&str> = bots.iter().filter(|b| of(store, &b.id).contains(&s.name)).map(|b| b.name.as_str()).collect();
            json!({ "name": s.name, "description": s.description, "body": s.body, "bots": users })
        })
        .collect();
    json!({ "skills": skills })
}

/// A GitHub page of a SKILL.md, or of the folder it's in, as the raw file.
fn raw_url(url: &str) -> String {
    let url = url.trim();
    let Some((repo, path)) = url.strip_prefix("https://github.com/").and_then(|rest| rest.split_once("/blob/").or_else(|| rest.split_once("/tree/"))) else {
        return url.to_string();
    };
    let path = path.trim_end_matches('/');
    let path = if path.ends_with(".md") { path.to_string() } else { format!("{path}/SKILL.md") };
    format!("https://raw.githubusercontent.com/{repo}/{path}")
}

async fn fetch(url: &str) -> Result<String, String> {
    if !url.trim().starts_with("https://") {
        return Err("skill links start with https://".into());
    }
    let answer = crate::http::client()
        .get(raw_url(url))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("couldn't fetch it: {e}"))?;
    if !answer.status().is_success() {
        return Err(format!("couldn't fetch it: {}", answer.status()));
    }
    let bytes = answer.bytes().await.map_err(|e| format!("couldn't fetch it: {e}"))?;
    if bytes.len() > FILE_MAX {
        return Err("that SKILL.md is too big".into());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "that link isn't a text file".into())
}

/// `GET /api/skills`; at home `POST /api/skills` `{ text | url, replace }` adds one,
/// `PUT /api/skills/:name` `{ text }` changes one, `DELETE /api/skills/:name` removes one.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or_default();
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, app.try_store(|store| Ok(wire(store))).map_err(fail)?)),
        (&Method::POST, []) => {
            let text = match body["text"].as_str().filter(|t| !t.trim().is_empty()) {
                Some(text) => text.to_string(),
                None => fetch(body["url"].as_str().ok_or_else(|| bad("paste a SKILL.md, or give its link"))?).await.map_err(bad)?,
            };
            let replace = body["replace"] == true;
            let skill = app.try_store(|store| save(store, &text, replace).map_err(Into::into)).map_err(bad)?;
            changed(app);
            Ok((StatusCode::CREATED, json!({ "name": skill.name })))
        }
        (&Method::PUT, [name]) => {
            let text = body["text"].as_str().ok_or_else(|| bad("the skill's SKILL.md"))?;
            let skill = parse(text).map_err(bad)?;
            if skill.name != *name {
                return Err(bad("a skill's name can't change; add it as a new skill instead"));
            }
            app.try_store(|store| save(store, text, true).map_err(Into::into)).map_err(bad)?;
            changed(app);
            Ok((StatusCode::OK, json!({ "name": skill.name })))
        }
        (&Method::DELETE, [name]) => {
            if !is_name(name) {
                return Err(bad("no such skill"));
            }
            app.try_store(|store| {
                let dir = library_dir(store).join(name);
                if !dir.join("SKILL.md").is_file() {
                    return Err("no such skill".into());
                }
                std::fs::remove_dir_all(dir)?;
                Ok(())
            })
            .map_err(bad)?;
            changed(app);
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such skills route".into())),
    }
}

fn changed(app: &App) {
    app.hub.send(json!({ "kind": "skills" }));
}

/// Takes `<save-skill>…</save-skill>` (a whole SKILL.md) out of a reply.
pub fn take(reply: &str) -> (String, Vec<String>) {
    let (open, close) = ("<save-skill>", "</save-skill>");
    let mut rest = String::new();
    let mut found = Vec::new();
    let mut text = reply;
    while let Some(start) = text.find(open) {
        rest.push_str(&text[..start]);
        let after = &text[start + open.len()..];
        match after.find(close) {
            Some(end) => {
                found.push(after[..end].trim().to_string());
                text = &after[end + close.len()..];
            }
            None => text = "",
        }
    }
    rest.push_str(text);
    (rest.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string(), found)
}

/// What every bot is told about writing skills.
pub const BOT_NOTE: &str = "\n\nWhen you've worked out a way of doing a job that will come up again, you can offer it as a skill: put a whole \
SKILL.md in your reply between <save-skill> and </save-skill> (YAML at the top between --- lines with a lowercase-hyphenated \
`name` and a one-sentence `description` of when to use it, then the instructions). The person sees a card and the skill is \
saved only if they approve. Offer one only when it's clearly worth keeping.";

/// A card for each skill a bot offered (or a note when one can't be read).
pub fn propose(app: &App, thread: &str, bot: &Bot, found: Vec<String>) {
    for text in found {
        match parse(&text) {
            Ok(skill) => {
                let exists = app.with_store(|store| Ok(library(store).iter().any(|s| s.name == skill.name))).unwrap_or(false);
                let mut lines = vec![format!("When: {}", skill.description)];
                let first: String = skill.body.split_whitespace().take(40).collect::<Vec<_>>().join(" ");
                if !first.is_empty() {
                    lines.push(format!("Starts: {first}…"));
                }
                let detail = json!({
                    "kind": "team-change",
                    "heading": format!("{} wants to save a skill", bot.name),
                    "askedBy": bot.name,
                    "title": if exists { format!("Replace the skill “{}”", skill.name) } else { format!("Save the skill “{}”", skill.name) },
                    "lines": lines,
                    "warning": exists.then(|| "A skill with this name is in your library; approving replaces it.".to_string()),
                    "action": "save_skill",
                    "doneText": "Saved to your skills, and turned on for this bot.",
                    "target": bot.id,
                    "body": { "text": text },
                    "state": "asking",
                });
                super::turns::save_and_send(app, thread, Role::Step, &format!("Skill: {}", skill.name), Some(&detail));
            }
            Err(why) => {
                super::turns::save_and_send(app, thread, Role::Note, &format!("{} offered a skill Catbot couldn't read: {why}.", bot.name), None);
            }
        }
    }
}

/// An approved skill: into the library, and used by the bot that wrote it.
pub fn save_approved(app: &App, bot_id: &str, text: &str) -> Result<(), String> {
    app.with_store(|store| {
        let skill = save(store, text, true)?;
        let mut names = of(store, bot_id);
        if !names.contains(&skill.name) {
            names.push(skill.name);
        }
        store.set_setting(&format!("skills:{bot_id}"), &json!(names))?;
        Ok(())
    })
    .ok_or_else(|| "couldn't save the skill".to_string())?;
    changed(app);
    app.announce_bot(bot_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: bed-plan\ndescription: Lay out garden beds\n  from a sketch.\n---\n\n# Bed plan\n\nMeasure first.\n";

    #[test]
    fn a_skill_reads_and_names_are_checked() {
        let skill = parse(SKILL).unwrap();
        assert_eq!((skill.name.as_str(), skill.description.as_str()), ("bed-plan", "Lay out garden beds from a sketch."));
        assert!(skill.body.starts_with("# Bed plan"));
        assert!(is_name("pdf") && is_name("a-b2") && !is_name("A") && !is_name("a--b") && !is_name("../x") && !is_name("-a"));
        assert!(parse("---\nname: image\ndescription: x\n---\nbody").is_err(), "/image is Catbot's");
        assert!(parse("no yaml").is_err());
    }

    #[test]
    fn github_pages_are_read_raw() {
        assert_eq!(raw_url("https://github.com/o/r/tree/main/skills/pdf"), "https://raw.githubusercontent.com/o/r/main/skills/pdf/SKILL.md");
        assert_eq!(raw_url("https://github.com/o/r/blob/main/skills/pdf/SKILL.md"), "https://raw.githubusercontent.com/o/r/main/skills/pdf/SKILL.md");
    }

    #[test]
    fn offered_skills_come_out_of_the_reply() {
        let (rest, found) = take(&format!("Here's one.\n<save-skill>{SKILL}</save-skill>"));
        assert_eq!(rest, "Here's one.");
        assert_eq!(parse(&found[0]).unwrap().name, "bed-plan");
    }
}
