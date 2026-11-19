//! Moving a Grok Bot here (stage 18.1). Grok Bot keeps a bot's setup in its own cloud account and
//! has no export button, so the bot writes its own: the person pastes `EXPORT_PROMPT` into its
//! chat, and it saves `grok-bot-export-<name>.json` (in Downloads when it can reach the computer)
//! or answers with the JSON. Catbot reads that and makes a bot of each, after a review.
//!
//! The file is the one MausBot reads (`format: "grok-bot-export"`, version 1), so an export works
//! in either. Its shape, the repairs for what a chat window does to it, and refusing an export
//! that holds credential-looking fields follow MausBot's `shared/grok-import.ts` (Apache-2.0),
//! which adapts Nous Research's agent-importers (MIT).
//!
//! Moo's choices (2026-11-19): what the bot knows about the person goes in that bot's own memory
//! note; Catbot looks in Downloads only while the import is open (or the export is pasted or
//! chosen); MausBot's rules: a brought-in bot asks before it acts, on Catbot's default AI, its
//! skills go into the library switched off, and its schedules come in paused. Routines that start
//! on an event, and the apps it used, are written into its memory note, to be set up again.

use std::{path::PathBuf, sync::Arc};

use axum::http::{Method, StatusCode};
use chrono::{Datelike, Local, TimeZone};
use serde::Serialize;
use serde_json::{Value, json};

use super::triggers::Schedule;
use super::{Answer, App, bad};

pub const EXPORT_FORMAT: &str = "grok-bot-export";
const OLD_FORMAT: &str = "openmaus.grok/1";
/// The most an export may be, in characters.
pub const MOST: usize = 4 * 1024 * 1024;
/// The most read from Downloads at once.
const MOST_FILES: usize = 20;
const MOST_ROUTINES: usize = 50;
/// Unbalanced `{` before a reply stops being searched: each one scans to its end.
const MOST_UNBALANCED: usize = 50;

pub const EXPORT_PROMPT: &str = r#"I'm moving you to another app and need a complete, machine-readable copy of your setup. Don't change, pause or delete anything.

Write one JSON document with exactly this shape, then save it as grok-bot-export-<your name>.json (your name in lowercase, spaces as hyphens): in my Downloads folder if you can write to my computer, otherwise in your workspace, and attach it to your reply. If you can't attach files, put the whole document in a single ```json code block instead.

{
  "format": "grok-bot-export",
  "version": 1,
  "bots": [
    {
      "name": "your name",
      "title": "your label or job title, or empty",
      "instructions": "your full role, job description and lasting rules, word for word where you can",
      "memories": ["each thing you remember about the work, one string per fact"],
      "user_facts": ["each fact you know about me (name, time zone, preferences), one per string"],
      "skills": [
        {"name": "skill name", "description": "when to use it",
         "instructions": "the COMPLETE skill text: every step, rule and output format, word for word"}
      ],
      "routines": [
        {"name": "routine name",
         "instruction": "the COMPLETE instruction the routine runs, word for word",
         "trigger": "schedule | slack | github | linear | sentry | pagerduty | email | webhook | other",
         "schedule_cron": "5-field cron expression in the routine's time zone, e.g. 0 8 * * 1-5 (empty when not on a schedule)",
         "schedule_text": "when it runs, in words, as the app shows it",
         "timezone": "IANA time zone, e.g. America/Chicago",
         "paused": false,
         "skills": ["names of the skills this routine uses"],
         "trigger_details": "for event triggers: channel, repository, phrase, senders and so on"}
      ],
      "apps": [{"name": "connected app or MCP server name", "mcp_url": "remote MCP address if any, else empty"}]
    }
  ]
}

Rules: include EVERY routine you have (paused ones too) and EVERY skill you use, with their full text, not summaries. If you can't read something (a routine's schedule, a skill's text), leave it out and tell me in your reply what you left out; never guess or fill it in from memory. Never include passwords, API keys, tokens, webhook keys, cookies or card numbers: leave them out entirely. Write valid JSON only (no comments).
If it's too long for one message or file, split it: send several documents, each complete and valid in this same shape with the same bot name, each holding what the others don't. They're brought in together.
"#;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub instructions: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Routine {
    pub name: String,
    pub instruction: String,
    pub trigger: String,
    pub cron: String,
    pub when: String,
    pub zone: String,
    pub paused: bool,
    pub skills: Vec<String>,
    pub details: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GrokApp {
    pub name: String,
    pub mcp_url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GrokBot {
    pub name: String,
    pub title: String,
    pub description: String,
    pub instructions: String,
    pub memories: Vec<String>,
    pub user_facts: Vec<String>,
    pub skills: Vec<Skill>,
    pub routines: Vec<Routine>,
    pub apps: Vec<GrokApp>,
}

// ---- Reading an export ----

fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn first(values: &[&Value]) -> String {
    values.iter().map(|v| text_of(v)).find(|s| !s.is_empty()).unwrap_or_default()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().map(|item| if item.is_object() { first(&[&item["text"], &item["content"], &item["name"]]) } else { text_of(item) }).filter(|s| !s.is_empty()).collect()
}

/// One line of plain text: control and separator characters become spaces.
fn one_line(text: &str) -> String {
    let plain: String = text.chars().map(|c| if c.is_control() || c == '\u{2028}' || c == '\u{2029}' { ' ' } else { c }).collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, most: usize) -> String {
    if text.chars().count() <= most { text.to_string() } else { format!("{}…", text.chars().take(most - 1).collect::<String>().trim_end()) }
}

/// A key that names a credential: a document with one is refused whole.
fn credential_key(key: &str) -> bool {
    let mut snake = String::new();
    let mut last_lower = false;
    for c in key.chars() {
        if c.is_ascii_uppercase() && last_lower {
            snake.push('_');
        }
        last_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        snake.push(if c == '-' || c.is_whitespace() { '_' } else { c.to_ascii_lowercase() });
    }
    if snake.contains("password") || snake.ends_with("key") {
        return true;
    }
    let parts: Vec<&str> = snake.split('_').collect();
    const WORDS: [&str; 12] = ["apikey", "token", "secret", "passwd", "passphrase", "credential", "credentials", "auth", "authorization", "bearer", "cookie", "cookies"];
    const PAIRS: [(&str, &str); 6] = [("api", "key"), ("private", "key"), ("access", "key"), ("session", "id"), ("session", "key"), ("session", "token")];
    parts.iter().any(|p| WORDS.contains(p)) || parts.windows(2).any(|w| PAIRS.contains(&(w[0], w[1])))
}

/// The credential-looking keys in a document. An app's named value slots (`headers`, `env`) are
/// never read, so their names don't count.
fn credential_keys(value: &Value, path: &str, in_apps: bool) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().enumerate().flat_map(|(i, item)| credential_keys(item, &format!("{path}[{i}]"), in_apps)).collect(),
        Value::Object(map) => map
            .iter()
            .flat_map(|(key, child)| {
                let here = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                if in_apps && ["headers", "env", "environment"].contains(&key.as_str()) {
                    return Vec::new();
                }
                let mut found = if credential_key(key) { vec![here.clone()] } else { Vec::new() };
                found.extend(credential_keys(child, &here, key == "apps" || key == "plugins"));
                found
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Commas just before a `}` or `]`, outside strings: what a chat app or phone leaves behind.
fn drop_trailing_commas(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let (mut quoted, mut escaped) = (false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
        } else if b == b',' && bytes[i + 1..].iter().find(|c| !c.is_ascii_whitespace()).is_some_and(|c| *c == b'}' || *c == b']') {
            continue;
        }
        out.push(b);
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

fn loose_json(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok().or_else(|| serde_json::from_str(&drop_trailing_commas(text)).ok())
}

/// Every top-level `{…}` in a reply, by brace matching that minds JSON strings.
fn braced_objects(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut unbalanced = 0;
    let mut at = text.find('{');
    while let Some(start) = at {
        if unbalanced >= MOST_UNBALANCED {
            break;
        }
        let (mut depth, mut quoted, mut end, mut i) = (0usize, false, None, start);
        while i < bytes.len() && end.is_none() {
            match bytes[i] {
                b'\\' if quoted => i += 1,
                b'"' => quoted = !quoted,
                b'{' if !quoted => depth += 1,
                b'}' if !quoted => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        match end {
            Some(end) => {
                found.push(&text[start..=end]);
                at = text[end + 1..].find('{').map(|n| n + end + 1);
            }
            None => {
                unbalanced += 1;
                at = text[start + 1..].find('{').map(|n| n + start + 1);
            }
        }
    }
    found
}

fn is_export(value: &Value) -> bool {
    value.is_object() && (value["format"] == EXPORT_FORMAT || value["format"] == OLD_FORMAT || value["bots"].is_array())
}

fn looks_like_bot(value: &Value) -> bool {
    value.is_object() && !text_of(&value["name"]).is_empty() && ["instructions", "memories", "user_facts", "skills", "routines", "apps"].iter().any(|k| value.get(*k).is_some())
}

/// The export documents in a reply or file. Inside a reply only export-shaped objects count (an
/// example routine isn't a bot); a file that's nothing but JSON may also be a bare bot or a list
/// of them. The repairs run only once the text as written has found nothing.
fn documents_in(text: &str) -> Vec<Value> {
    let repaired = text.replace(['\u{201c}', '\u{201d}'], "\"").replace(['\u{2018}', '\u{2019}'], "'");
    let versions: Vec<&str> = if repaired == text { vec![text] } else { vec![text, &repaired] };
    for version in versions {
        if let Some(whole) = loose_json(version.trim()) {
            if is_export(&whole) {
                return vec![whole];
            }
            if looks_like_bot(&whole) {
                return vec![json!({ "bots": [whole] })];
            }
            if let Some(list) = whole.as_array().filter(|l| !l.is_empty() && l.iter().all(looks_like_bot)) {
                return vec![json!({ "bots": list })];
            }
        }
        let found: Vec<Value> = braced_objects(version).into_iter().filter_map(loose_json).filter(is_export).collect();
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

fn read_bot(bot: &Value) -> GrokBot {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    GrokBot {
        name: one_line(&first(&[&bot["name"]])),
        title: one_line(&first(&[&bot["title"]])),
        description: first(&[&bot["description"]]),
        instructions: first(&[&bot["instructions"], &bot["soul"], &bot["systemPrompt"]]),
        memories: strings(&bot["memories"]),
        user_facts: strings(if bot["user_facts"].is_null() { &bot["userFacts"] } else { &bot["user_facts"] }),
        skills: list(&bot["skills"])
            .iter()
            .filter_map(|s| match s {
                Value::String(name) => Some(Skill { name: name.trim().to_string(), ..Skill::default() }),
                Value::Object(_) => Some(Skill {
                    name: first(&[&s["name"], &s["title"]]),
                    description: first(&[&s["description"], &s["summary"]]),
                    instructions: first(&[&s["instructions"], &s["body"], &s["content"], &s["text"]]),
                }),
                _ => None,
            })
            .collect(),
        routines: list(&bot["routines"])
            .iter()
            .filter(|r| r.is_object())
            .map(|r| Routine {
                name: first(&[&r["name"], &r["title"]]),
                instruction: first(&[&r["instruction"], &r["prompt"], &r["instructions"], &r["task"]]),
                trigger: first(&[&r["trigger"]]).to_lowercase(),
                cron: first(&[&r["schedule_cron"], &r["cron"], &r["cronExpression"]]),
                when: first(&[&r["schedule_text"], &r["schedule"], &r["when"]]),
                zone: first(&[&r["timezone"], &r["timeZone"], &r["tz"]]),
                paused: r["paused"] == true,
                skills: strings(&r["skills"]),
                details: first(&[&r["trigger_details"], &r["details"]]),
            })
            .collect(),
        apps: [&bot["apps"], &bot["plugins"], &bot["integrations"]]
            .into_iter()
            .find(|v| v.is_array())
            .map(list)
            .unwrap_or_default()
            .iter()
            .filter_map(|a| {
                let name = if let Value::String(s) = a { s.trim().to_string() } else { first(&[&a["name"], &a["label"], &a["id"], &a["slug"]]) };
                (!name.is_empty()).then(|| GrokApp { name: one_line(&name), mcp_url: first(&[&a["mcp_url"], &a["mcpUrl"], &a["url"]]) })
            })
            .collect(),
    }
}

/// Every string, with anything that looks like a key taken out.
fn scrubbed(bot: GrokBot) -> GrokBot {
    let s = |t: &str| super::teams::scrub(t);
    let all = |l: &[String]| l.iter().map(|t| s(t)).collect::<Vec<_>>();
    GrokBot {
        name: s(&bot.name),
        title: s(&bot.title),
        description: s(&bot.description),
        instructions: s(&bot.instructions),
        memories: all(&bot.memories),
        user_facts: all(&bot.user_facts),
        skills: bot.skills.iter().map(|k| Skill { name: s(&k.name), description: s(&k.description), instructions: s(&k.instructions) }).collect(),
        routines: bot
            .routines
            .iter()
            .map(|r| Routine { name: s(&r.name), instruction: s(&r.instruction), when: s(&r.when), details: s(&r.details), skills: all(&r.skills), ..r.clone() })
            .collect(),
        apps: bot.apps.iter().map(|a| GrokApp { name: s(&a.name), mcp_url: a.mcp_url.clone() }).collect(),
    }
}

/// One bot per name: lists are joined dropping exact repeats, and other fields keep the last
/// one given, so a split or repeated export brings each thing in once.
pub fn merge(bots: Vec<GrokBot>) -> Vec<GrokBot> {
    fn join<T: PartialEq + Clone>(into: &mut Vec<T>, more: &[T]) {
        for item in more {
            if !into.contains(item) {
                into.push(item.clone());
            }
        }
    }
    let mut merged: Vec<GrokBot> = Vec::new();
    for bot in bots {
        match merged.iter_mut().find(|m| m.name.to_lowercase() == bot.name.to_lowercase()) {
            None => merged.push(bot),
            Some(into) => {
                for (field, more) in [(&mut into.title, bot.title), (&mut into.description, bot.description), (&mut into.instructions, bot.instructions)] {
                    if !more.is_empty() {
                        *field = more;
                    }
                }
                join(&mut into.memories, &bot.memories);
                join(&mut into.user_facts, &bot.user_facts);
                join(&mut into.skills, &bot.skills);
                join(&mut into.routines, &bot.routines);
                join(&mut into.apps, &bot.apps);
            }
        }
    }
    merged
}

/// The bots in a reply or file: a bare document, a fenced one, or several with chat around them.
pub fn parse(text: &str) -> Result<Vec<GrokBot>, String> {
    if text.chars().count() > MOST {
        return Err("that's bigger than a Grok Bot export can be (4 MB)".into());
    }
    let text = text.trim_start_matches('\u{feff}');
    let documents = documents_in(text);
    if documents.is_empty() {
        return Err(if text.matches('{').count() > text.matches('}').count() {
            "the export looks cut off: ask the bot to split it into several documents, as the prompt says".into()
        } else {
            "that isn't a Grok Bot export: paste the prompt into the bot's chat and bring back what it writes".into()
        });
    }
    let refused: Vec<String> = documents.iter().flat_map(|d| credential_keys(d, "", false)).collect();
    if !refused.is_empty() {
        let shown: Vec<String> = refused.into_iter().take(5).collect();
        return Err(format!("the export has fields that look like keys or passwords ({}), so none of it was read: ask the bot to write it again without them", shown.join(", ")));
    }
    if documents.iter().any(|d| d["version"].as_u64().is_some_and(|v| v > 1)) {
        return Err("that export is a newer version than Catbot reads".into());
    }
    let bots: Vec<GrokBot> = documents
        .iter()
        .flat_map(|d| match d["bots"].as_array() {
            Some(list) => list.iter().filter(|b| b.is_object()).cloned().collect::<Vec<_>>(),
            None => vec![d.clone()],
        })
        .map(|b| read_bot(&b))
        .filter(|b| !b.name.is_empty())
        .map(scrubbed)
        .collect();
    let bots = merge(bots);
    if bots.is_empty() {
        return Err("that isn't a Grok Bot export: it has no bot with a name".into());
    }
    Ok(bots)
}

// ---- Schedules ----

const CLOCK_TRIGGERS: [&str; 6] = ["", "schedule", "scheduled", "time", "cron", "interval"];

/// A cron day-of-week field as Catbot's days (Monday 1 … Sunday 7).
fn cron_days(field: &str) -> Option<Vec<u32>> {
    const NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
    let day = |t: &str| -> Option<u32> {
        let t = t.trim().to_lowercase();
        let n = match t.parse::<u32>() {
            Ok(n) if n <= 7 => n,
            Ok(_) => return None,
            Err(_) => NAMES.iter().position(|n| t.starts_with(n))? as u32,
        };
        Some(if n == 0 { 7 } else { n })
    };
    if field == "*" || field == "?" {
        return Some((1..=7).collect());
    }
    let mut days = Vec::new();
    for part in field.split(',') {
        match part.split_once('-') {
            Some((a, b)) => {
                // Cron counts Sunday as 0, so a range runs in cron's order.
                let (a, b) = (day(a)? % 7, day(b)?);
                let b = if b == 7 && a != 0 { 7 } else { b % 7 };
                if a > b {
                    return None;
                }
                days.extend((a..=b).map(|d| if d == 0 { 7 } else { d }));
            }
            None => days.push(day(part)?),
        }
    }
    days.sort_unstable();
    days.dedup();
    (!days.is_empty()).then_some(days)
}

/// A cron expression as a Catbot schedule, its times moved from `zone` to this computer's
/// clock. None for what Catbot's schedules can't say (days of the month, several hours…).
pub fn from_cron(expression: &str, zone: Option<chrono_tz::Tz>) -> Option<Schedule> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else { return None };
    if !["*", "?"].contains(dom) || !["*", "?"].contains(month) {
        return None;
    }
    let days = cron_days(dow)?;
    let all_days = days.len() == 7;
    let today = Local::now().date_naive();
    let start = |m: u32| format!("{}T00:{m:02}", today.format("%Y-%m-%d"));
    let every = |m: u32, days: &[u32]| -> Option<Schedule> {
        let schedule = Schedule::Interval { every_minutes: m, start: start(0), days: (!all_days).then(|| days.to_vec()), from: None, to: None };
        schedule.check().ok().map(|_| schedule)
    };
    match (minute.parse::<u32>().ok(), hour.parse::<u32>().ok()) {
        (Some(m), Some(h)) if m < 60 && h < 24 => {
            let (mut h, mut m, mut days) = (h, m, days);
            if let Some(zone) = zone {
                let there = zone.with_ymd_and_hms(today.year(), today.month(), today.day(), h, m, 0).single()?;
                let here = there.with_timezone(&Local);
                let shift = (here.date_naive() - there.date_naive()).num_days();
                (h, m) = (chrono::Timelike::hour(&here), chrono::Timelike::minute(&here));
                days = days.iter().map(|d| ((*d as i64 - 1 + shift).rem_euclid(7) + 1) as u32).collect();
                days.sort_unstable();
            }
            Some(Schedule::Weekly { time: format!("{h:02}:{m:02}"), days })
        }
        (Some(m), None) if m < 60 && *hour == "*" => {
            let schedule = Schedule::Interval { every_minutes: 60, start: start(m), days: (!all_days).then_some(days), from: None, to: None };
            Some(schedule)
        }
        (Some(m), None) if m < 60 => {
            let n: u32 = hour.strip_prefix("*/")?.parse().ok()?;
            let schedule = Schedule::Interval { every_minutes: n * 60, start: start(m), days: (!all_days).then_some(days), from: None, to: None };
            schedule.check().ok().map(|_| schedule)
        }
        (None, None) if *hour == "*" => every(minute.strip_prefix("*/")?.parse().ok()?, &days),
        _ => None,
    }
}

/// A routine's schedule here: from its cron, or its words when those are a cron. None for a
/// routine that starts on an event, or whose clock Catbot can't read.
pub fn schedule_of(routine: &Routine) -> Option<Schedule> {
    if !CLOCK_TRIGGERS.contains(&routine.trigger.as_str()) {
        return None;
    }
    let zone = routine.zone.parse::<chrono_tz::Tz>().ok();
    [&routine.cron, &routine.when].into_iter().filter(|e| !e.is_empty()).find_map(|e| from_cron(e, zone))
}

/// A schedule in words, for the review.
pub fn in_words(schedule: &Schedule) -> String {
    const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let days = |d: &[u32]| match d {
        [1, 2, 3, 4, 5, 6, 7] => "every day".to_string(),
        [1, 2, 3, 4, 5] => "weekdays".to_string(),
        [6, 7] => "weekends".to_string(),
        _ => d.iter().filter_map(|n| NAMES.get(*n as usize - 1)).copied().collect::<Vec<_>>().join(", "),
    };
    match schedule {
        Schedule::Weekly { time, days: d } => format!("{} at {time}", days(d)),
        Schedule::Interval { every_minutes, days: d, .. } => {
            let every = if every_minutes % 60 == 0 { format!("every {} hour{}", every_minutes / 60, if *every_minutes == 60 { "" } else { "s" }) } else { format!("every {every_minutes} minutes") };
            match d {
                Some(d) => format!("{every}, {}", days(d)),
                None => every,
            }
        }
        Schedule::Once { at } => format!("once, at {at}"),
    }
}

// ---- Skills ----

fn skill_slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').chars().take(super::skills::NAME_MAX - 3).collect::<String>().trim_end_matches('-').to_string()
}

/// A skill as a SKILL.md under `name`; None when it has no text or won't make a valid one.
fn skill_md(skill: &Skill, name: &str) -> Option<String> {
    let given = skill.instructions.trim_start_matches('\u{feff}');
    let (front, body) = match given.strip_prefix("---").and_then(|rest| rest.find("\n---").map(|end| (&rest[..end], &rest[end + 4..]))) {
        Some((front, body)) => (Some(front), body.trim_start_matches('-')),
        None => (None, given),
    };
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let from_front = front.and_then(|f| serde_yaml_ng::from_str::<Value>(f).ok()).map(|v| text_of(&v["description"])).unwrap_or_default();
    let first_line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim_start_matches('#').trim().to_string();
    let description = clip(&one_line([&skill.description, &from_front, &first_line].into_iter().find(|d| !d.trim().is_empty())?), super::skills::DESCRIPTION_MAX);
    let text = format!("---\nname: {name}\ndescription: {}\n---\n\n{body}\n", serde_json::to_string(&description).ok()?);
    (text.len() <= super::skills::FILE_MAX && super::skills::parse(&text).is_ok()).then_some(text)
}

// ---- Downloads ----

fn downloads() -> PathBuf {
    std::env::var_os("CATBOT_DOWNLOADS").map(PathBuf::from).unwrap_or_else(|| crate::store::user_home().join("Downloads"))
}

/// Whether a file name is an export's: `grok-bot-export….json|md|txt`, no folders.
pub fn is_export_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("grok-bot-export") && [".json", ".md", ".txt"].iter().any(|e| lower.ends_with(e)) && !name.contains(['/', '\\']) && !name.contains("..")
}

/// The exports in Downloads, newest first: real files only (no links), not too big.
pub fn found() -> Vec<(String, std::time::SystemTime, u64)> {
    let mut files: Vec<(String, std::time::SystemTime, u64)> = std::fs::read_dir(downloads())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = std::fs::symlink_metadata(entry.path()).ok()?;
            (is_export_name(&name) && meta.is_file() && meta.len() as usize <= MOST).then(|| (name, meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len()))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files.truncate(MOST_FILES);
    files
}

/// What a request brings: pasted text, and files from Downloads by name, each read on its own.
fn sources(body: &Value) -> Result<Vec<String>, String> {
    let mut texts = Vec::new();
    if let Some(text) = body["text"].as_str().filter(|t| !t.trim().is_empty()) {
        texts.push(text.to_string());
    }
    let listed: Vec<String> = found().into_iter().map(|(name, ..)| name).collect();
    for name in body["files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !listed.iter().any(|n| n == name) {
            return Err(format!("{name} isn't a Grok Bot export in Downloads"));
        }
        let text = std::fs::read_to_string(downloads().join(name)).map_err(|e| format!("couldn't read {name}: {e}"))?;
        texts.push(text);
    }
    if texts.is_empty() {
        return Err("paste the export, or pick a file from Downloads".into());
    }
    Ok(texts)
}

fn read_all(body: &Value) -> Result<Vec<GrokBot>, String> {
    let mut bots = Vec::new();
    for text in sources(body)? {
        bots.extend(parse(&text)?);
    }
    Ok(merge(bots))
}

// ---- The review and the import ----

fn about(bot: &GrokBot) -> String {
    let text = if bot.description.trim().is_empty() { &bot.instructions } else { &bot.description };
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 40 { format!("{}…", words[..40].join(" ")) } else { words.join(" ") }
}

fn color_of(bot: &GrokBot) -> &'static str {
    let hash = bot.name.bytes().chain(bot.instructions.bytes()).fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x01000193));
    crate::store::COLORS[hash as usize % crate::store::COLORS.len()]
}

fn plan(app: &App, bots: &[GrokBot]) -> Value {
    let taken: Vec<String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| b.name.to_lowercase()).collect();
    let shown: Vec<Value> = bots
        .iter()
        .map(|bot| {
            let skills: Vec<&str> = bot.skills.iter().filter(|s| !s.instructions.trim().is_empty()).map(|s| s.name.as_str()).collect();
            let (timed, events): (Vec<&Routine>, Vec<&Routine>) = bot.routines.iter().filter(|r| !r.instruction.is_empty()).partition(|r| schedule_of(r).is_some());
            json!({
                "key": bot.name.to_lowercase(),
                "name": bot.name,
                "title": bot.title,
                "about": about(bot),
                "color": color_of(bot),
                "nameTaken": taken.contains(&bot.name.to_lowercase()),
                "skills": skills,
                "schedules": timed.iter().take(MOST_ROUTINES).map(|r| json!({ "name": r.name, "when": schedule_of(r).map(|s| in_words(&s)) })).collect::<Vec<_>>(),
                "events": events.iter().map(|r| if r.name.is_empty() { "a routine".to_string() } else { r.name.clone() }).collect::<Vec<_>>(),
                "memories": bot.memories.len(),
                "facts": bot.user_facts.len(),
                "apps": bot.apps.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "bots": shown,
        "safety": "Brought-in bots ask before they act, on Catbot's default AI. Their skills go into your skills library switched off, and their schedules come in paused: switch on what you want once you've looked. Routines that start on an event, and the apps they used, are written into each bot's memory note to set up again here. Keys and passwords never come over.",
    })
}

/// The bot's notes from Grok Bot: what it remembered, what it knew about the person, the routines
/// that start on an event, and the apps it used.
fn notes(bot: &GrokBot, events: &[&Routine]) -> Option<String> {
    let mut seen = Vec::new();
    let mut fresh = |list: &[String]| -> Vec<String> {
        list.iter()
            .map(|l| one_line(l))
            .filter(|l| {
                let key = l.to_lowercase();
                let new = !seen.contains(&key);
                seen.push(key);
                new
            })
            .collect()
    };
    let (remembered, about) = (fresh(&bot.memories), fresh(&bot.user_facts));
    let mut text = String::new();
    let mut part = |heading: &str, lines: Vec<String>| {
        if !lines.is_empty() {
            text.push_str(&format!("### {heading}\n\n{}\n\n", lines.join("\n")));
        }
    };
    part("What I remember", remembered.iter().map(|l| format!("- {l}")).collect());
    part("About the person", about.iter().map(|l| format!("- {l}")).collect());
    part(
        "Routines that start on an event (set them up again in Triggers)",
        events
            .iter()
            .map(|r| {
                let mut line = format!("- **{}** ({}{}): {}", if r.name.is_empty() { "routine" } else { &r.name }, if r.trigger.is_empty() { "schedule Catbot couldn't read" } else { &r.trigger }, if r.details.is_empty() && r.when.is_empty() { String::new() } else { format!("; {}", if r.details.is_empty() { &r.when } else { &r.details }) }, one_line(&r.instruction));
                if r.paused {
                    line.push_str(" (paused in Grok Bot)");
                }
                line
            })
            .collect(),
    );
    part(
        "Apps I used (connect them again in Settings › Apps)",
        bot.apps.iter().map(|a| if a.mcp_url.starts_with("https://") && super::teams::scrub(&a.mcp_url) == a.mcp_url && !a.mcp_url.contains('?') { format!("- {} ({})", a.name, a.mcp_url) } else { format!("- {}", a.name) }).collect(),
    );
    (!text.is_empty()).then(|| format!("\n## Brought over from Grok Bot ({})\n\n{}", Local::now().format("%Y-%m-%d"), text.trim_end()))
}

/// Makes the bots, leaving out the ones named; returns what was made.
fn import(app: &Arc<App>, bots: &[GrokBot], leave: &[&str]) -> Result<Value, (StatusCode, String)> {
    let mut made = Vec::new();
    for grok in bots.iter().filter(|b| !leave.contains(&b.name.to_lowercase().as_str())) {
        let mut soul = if grok.instructions.trim().is_empty() { grok.description.trim().to_string() } else { grok.instructions.trim().to_string() };
        if !grok.title.is_empty() {
            soul.push_str(&format!("\n\nYour role: {}.", grok.title));
        }
        let bot = super::make_bot(app, &json!({ "name": grok.name, "soul": soul, "approvalMode": "ask", "color": color_of(grok) }))?;

        // Skills: into the library, switched off for the bot.
        let mut skills = Vec::new();
        // Each skill's name in Grok Bot, and its name here.
        let mut named: Vec<(String, String)> = Vec::new();
        let library: Vec<super::skills::Skill> = app.with_store(|store| Ok(super::skills::library(store))).unwrap_or_default();
        for (i, skill) in grok.skills.iter().enumerate() {
            let base = match skill_slug(&skill.name) {
                s if s.is_empty() => format!("skill-{}", i + 1),
                s => s,
            };
            let Some(text) = skill_md(skill, &base) else { continue };
            let wanted = super::skills::parse(&text).ok();
            // The same skill already in the library is shared; another of that name gets a number.
            let mut name = base.clone();
            let mut n = 2;
            loop {
                match library.iter().find(|s| s.name == name) {
                    Some(have) if wanted.as_ref().is_some_and(|w| w.body == have.body && w.description == have.description) => break,
                    Some(_) => {
                        name = format!("{base}-{n}");
                        n += 1;
                    }
                    None => break,
                }
            }
            if !library.iter().any(|s| s.name == name) {
                let Some(text) = skill_md(skill, &name) else { continue };
                if let Err(err) = app.try_store(|store| super::skills::save(store, &text, false).map_err(Into::into)) {
                    eprintln!("catbot: couldn't add {}'s skill {name}: {err}", bot.name);
                    continue;
                }
            }
            named.push((skill.name.trim().to_lowercase(), name.clone()));
            skills.push(name);
        }

        // Routines on a clock: paused triggers. Those on an event go in its notes.
        let mut schedules = 0;
        let mut events = Vec::new();
        for routine in grok.routines.iter().filter(|r| !r.instruction.is_empty()) {
            let Some(schedule) = schedule_of(routine).filter(|_| schedules < MOST_ROUTINES) else {
                events.push(routine);
                continue;
            };
            let uses: Vec<&String> = routine.skills.iter().filter_map(|s| named.iter().find(|(given, _)| *given == s.trim().to_lowercase()).map(|(_, here)| here)).collect();
            let mut prompt = routine.instruction.clone();
            if !uses.is_empty() {
                prompt.push_str(&format!("\n\nUse these skills: {}.", uses.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            }
            let name = clip(&one_line(if routine.name.is_empty() { "Routine from Grok Bot" } else { &routine.name }), 80);
            let body = json!({ "botId": bot.id, "kind": "schedule", "name": name, "prompt": prompt.chars().take(20_000).collect::<String>(), "schedule": schedule, "enabled": false });
            match super::triggers::create(app, &body, "grok-import") {
                Ok(_) => schedules += 1,
                Err(err) => {
                    eprintln!("catbot: couldn't bring over {}'s routine {name}: {err}", bot.name);
                    events.push(routine);
                }
            }
        }

        // Its notes, in its memory note (or its folder without a memory vault).
        let mut noted = false;
        if let Some(text) = notes(grok, &events) {
            let path = match &app.memory {
                Some(memory) => memory.join(crate::memory::bot_note(&bot)),
                None => bot.folder.join("GROK-NOTES.md"),
            };
            let written = std::fs::create_dir_all(path.parent().unwrap_or(&bot.folder)).and_then(|_| {
                use std::io::Write;
                std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(format!("{text}\n").as_bytes())
            });
            match written {
                Ok(()) => noted = true,
                Err(err) => eprintln!("catbot: couldn't write {}'s notes from Grok Bot: {err}", bot.name),
            }
        }
        made.push(json!({ "id": bot.id, "name": bot.name, "skills": skills, "schedules": schedules, "events": events.len(), "noted": noted }));
    }
    if !made.is_empty() {
        app.hub.send(json!({ "kind": "skills" }));
        app.hub.send(json!({ "kind": "triggers" }));
    }
    eprintln!("catbot: brought over {} bot(s) from Grok Bot", made.len());
    Ok(json!({ "bots": made }))
}

/// `GET /api/grok-move`: the prompt, and the exports in Downloads.
/// `POST /api/grok-move/preview` `{ text?, files? }`: what bringing them in would do.
/// `POST /api/grok-move/import` `{ text?, files?, leaveOut: [names] }`: makes the bots.
pub fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or(Value::Null);
    match (method, rest) {
        (&Method::GET, []) => {
            let files: Vec<Value> = found()
                .into_iter()
                .map(|(name, modified, size)| json!({ "name": name, "modified": chrono::DateTime::<Local>::from(modified).to_rfc3339(), "size": size }))
                .collect();
            Ok((StatusCode::OK, json!({ "prompt": EXPORT_PROMPT, "files": files, "folder": downloads().display().to_string() })))
        }
        (&Method::POST, ["preview"]) => {
            let bots = read_all(&body).map_err(bad)?;
            Ok((StatusCode::OK, json!({ "plan": plan(app, &bots) })))
        }
        (&Method::POST, ["import"]) => {
            let bots = read_all(&body).map_err(bad)?;
            let leave: Vec<String> = body["leaveOut"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_lowercase).collect();
            let leave: Vec<&str> = leave.iter().map(String::as_str).collect();
            if bots.iter().all(|b| leave.contains(&b.name.to_lowercase().as_str())) {
                return Err(bad("every bot is left out"));
            }
            import(app, &bots, &leave).map(|made| (StatusCode::CREATED, made))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such Grok Bot route".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORT: &str = r#"Here it is!
```json
{
  "format": "grok-bot-export",
  "version": 1,
  "bots": [{
    "name": "Scout",
    "title": "Researcher",
    "instructions": "Find things out. Cite sources.",
    "memories": ["Moo likes short answers", "The garden is in zone 8a"],
    "user_facts": ["Name: Moo", "Time zone: America/Chicago"],
    "skills": [{"name": "Weekly Digest", "description": "Summarise the week", "instructions": "Collect the week's notes and write five bullets."}],
    "routines": [
      {"name": "Morning brief", "instruction": "Write the morning brief.", "trigger": "schedule", "schedule_cron": "0 8 * * 1-5", "timezone": "America/Chicago", "skills": ["Weekly Digest"]},
      {"name": "PR watch", "instruction": "Review new pull requests.", "trigger": "github", "trigger_details": "repo moo/catbot"}
    ],
    "apps": [{"name": "Notion", "mcp_url": "https://mcp.notion.com/mcp"}],
  }]
}
```
Tell me if anything's missing."#;

    #[test]
    fn a_reply_is_read_with_its_repairs() {
        let bots = parse(EXPORT).unwrap();
        assert_eq!(bots.len(), 1);
        let scout = &bots[0];
        assert_eq!((scout.name.as_str(), scout.title.as_str(), scout.memories.len(), scout.user_facts.len()), ("Scout", "Researcher", 2, 2));
        assert_eq!(scout.skills[0].name, "Weekly Digest");
        assert_eq!(scout.routines[1].trigger, "github");
        assert_eq!(scout.apps[0].mcp_url, "https://mcp.notion.com/mcp");
        // Curly quotes from a phone, and a bare bot.
        let bare = parse("{\u{201c}name\u{201d}: \u{201c}Echo\u{201d}, \u{201c}instructions\u{201d}: \u{201c}Repeat.\u{201d}}").unwrap();
        assert_eq!(bare[0].instructions, "Repeat.");
    }

    #[test]
    fn credentials_refuse_the_export_and_keys_are_taken_out() {
        let with_key = r#"{"format":"grok-bot-export","version":1,"bots":[{"name":"A","instructions":"x","apiKey":"abc"}]}"#;
        assert!(parse(with_key).unwrap_err().contains("bots[0].apiKey"));
        let header_names = r#"{"format":"grok-bot-export","version":1,"bots":[{"name":"A","instructions":"x","apps":[{"name":"S","headers":{"Authorization":""}}]}]}"#;
        assert!(parse(header_names).is_ok(), "an app's header names are only names");
        let token_in_text = r#"{"format":"grok-bot-export","version":1,"bots":[{"name":"A","instructions":"use sk-abcdefghijklmnopqrstuvwxyz123456 for it"}]}"#;
        assert!(!parse(token_in_text).unwrap()[0].instructions.contains("sk-abc"));
        assert!(parse(r#"{"format":"grok-bot-export","version":2,"bots":[{"name":"A"}]}"#).unwrap_err().contains("newer"));
        assert!(parse("```json\n{\"format\": \"grok-bot-export\", \"bots\": [{\"name\": \"A\"").unwrap_err().contains("cut off"));
        assert!(parse("hello there").unwrap_err().contains("isn't a Grok Bot export"));
    }

    #[test]
    fn split_exports_merge_by_name() {
        let one = r#"{"format":"grok-bot-export","version":1,"bots":[{"name":"Scout","instructions":"A","memories":["m1"]}]}"#;
        let two = r#"{"format":"grok-bot-export","version":1,"bots":[{"name":"scout","memories":["m1","m2"],"skills":[{"name":"s","instructions":"do"}]}]}"#;
        let bots = parse(&format!("{one}\n\nand the rest:\n{two}")).unwrap();
        assert_eq!(bots.len(), 1);
        assert_eq!((bots[0].instructions.as_str(), bots[0].memories.len(), bots[0].skills.len()), ("A", 2, 1));
    }

    #[test]
    fn crons_become_catbot_schedules() {
        assert_eq!(from_cron("30 8 * * 1-5", None), Some(Schedule::Weekly { time: "08:30".into(), days: vec![1, 2, 3, 4, 5] }));
        assert_eq!(from_cron("0 9 * * SUN,sat", None), Some(Schedule::Weekly { time: "09:00".into(), days: vec![6, 7] }));
        assert_eq!(from_cron("0 9 * * 0-6", None), Some(Schedule::Weekly { time: "09:00".into(), days: vec![1, 2, 3, 4, 5, 6, 7] }));
        assert!(matches!(from_cron("*/15 * * * *", None), Some(Schedule::Interval { every_minutes: 15, days: None, .. })));
        assert!(matches!(from_cron("5 */2 * * 1-5", None), Some(Schedule::Interval { every_minutes: 120, days: Some(_), .. })));
        assert_eq!(from_cron("0 9 1 * *", None), None, "days of the month aren't Catbot's");
        assert_eq!(from_cron("0 8,17 * * *", None), None);
        assert_eq!(from_cron("*/2 * * * *", None), None, "every 2 minutes is too often");
        // A zone: 08:00 in Tokyo is the evening before, here in UTC.
        let tokyo: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
        if Local::now().offset().local_minus_utc() == 0 {
            assert_eq!(from_cron("0 8 * * 1", Some(tokyo)), Some(Schedule::Weekly { time: "23:00".into(), days: vec![7] }));
        }
        let routine = Routine { trigger: "github".into(), cron: "0 8 * * *".into(), ..Routine::default() };
        assert_eq!(schedule_of(&routine), None, "an event routine isn't on a clock");
        assert_eq!(in_words(&Schedule::Weekly { time: "08:00".into(), days: vec![1, 2, 3, 4, 5] }), "weekdays at 08:00");
    }

    #[test]
    fn skills_become_skill_files() {
        let skill = Skill { name: "Weekly Digest!".into(), description: String::new(), instructions: "---\nname: whatever\ndescription: Sum up the week\n---\n# Digest\nFive bullets.".into() };
        let name = skill_slug(&skill.name);
        assert_eq!(name, "weekly-digest");
        let text = skill_md(&skill, &name).unwrap();
        let parsed = super::super::skills::parse(&text).unwrap();
        assert_eq!((parsed.name.as_str(), parsed.description.as_str(), parsed.body.as_str()), ("weekly-digest", "Sum up the week", "# Digest\nFive bullets."));
        assert_eq!(skill_md(&Skill { name: "x".into(), ..Skill::default() }, "x"), None);
        assert!(is_export_name("grok-bot-export-scout.json") && !is_export_name("notes.json") && !is_export_name("grok-bot-export/../x.json"));
    }
}
