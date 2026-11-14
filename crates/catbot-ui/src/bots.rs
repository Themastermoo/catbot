//! Bots as the interface shows them: read from the server's `WireBot` JSON (the reference
//! interface's wire format), with the one-line preview the sidebar gives each.
//!
//! The preview rules follow OpenMausBot's `src/components/sidebar-preview.ts` (Apache-2.0, see NOTICE),
//! for the message kinds Catbot sends: text, activity steps and approval cards.

use serde_json::Value;

/// One of Catbot's bots, as the sidebar needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Bot {
    pub id: String,
    /// Its conversation (Catbot bots have one).
    pub thread_id: String,
    pub name: String,
    /// One of the stylesheet's identity colours (`--identity-<color>`).
    pub color: String,
    pub activity: Activity,
    pub unread: bool,
    /// Its newest message worth a preview, if it has one.
    pub last: Option<Last>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Working,
    WaitingOnYou,
}

/// The newest previewable message: its line and when it was sent (ms since the epoch).
#[derive(Clone, Debug, PartialEq)]
pub struct Last {
    pub line: String,
    pub at: f64,
}

impl Bot {
    /// Catbot's own bots from a `GET /api/bots` answer, in the server's order.
    /// The reference server's bots (MausBot) are left to its own interface.
    pub fn list(answer: &Value) -> Vec<Bot> {
        answer["bots"]
            .as_array()
            .map(|bots| bots.iter().filter(|b| b["catbot"] == true).filter_map(Bot::from_wire).collect())
            .unwrap_or_default()
    }

    pub fn from_wire(wire: &Value) -> Option<Bot> {
        let activity = match wire["activity"].as_str() {
            Some("waiting-on-you") => Activity::WaitingOnYou,
            _ if wire["busy"] == true => Activity::Working,
            Some("working") => Activity::Working,
            _ => Activity::Idle,
        };
        let messages = wire["messages"].as_array().map(Vec::as_slice).unwrap_or_default();
        Some(Bot {
            id: wire["id"].as_str()?.to_string(),
            thread_id: wire["threadId"].as_str().unwrap_or_default().to_string(),
            name: wire["name"].as_str().unwrap_or_default().to_string(),
            color: wire["color"].as_str().unwrap_or("blue").to_string(),
            activity,
            unread: wire["unread"] == true,
            last: messages.iter().rev().find_map(|m| Some(Last { line: line(m)?, at: m["at"].as_f64().unwrap_or(0.0) })),
        })
    }

    /// The row's second line: what the bot is doing, or else its newest line.
    pub fn preview(&self) -> String {
        match self.activity {
            Activity::WaitingOnYou => "Waiting for you…".into(),
            Activity::Working => "Working…".into(),
            Activity::Idle => self.last.as_ref().map(|l| l.line.clone()).unwrap_or_default(),
        }
    }

    /// Whether a search finds this bot: by its name or its preview, ignoring case.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty() || self.name.to_lowercase().contains(&query) || self.preview().to_lowercase().contains(&query)
    }

    /// The letter on its stand-in mark.
    pub fn initial(&self) -> String {
        self.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
    }
}

/// A message as one preview line, or None for one a preview skips.
fn line(message: &Value) -> Option<String> {
    match message["kind"].as_str()? {
        "text" => {
            let text = plain(message["text"].as_str().unwrap_or_default());
            (!text.is_empty()).then_some(text)
        }
        // A tool step, in the full layout, by its name (Catbot's notes start "notice: ").
        "activity" => {
            let name = message["tool"]["name"].as_str()?;
            Some(name.strip_prefix("notice: ").unwrap_or(name).to_string())
        }
        // An approval card by how it ended, or its title while it waits.
        "options" => {
            let card = &message["card"];
            Some(match card["answered"].as_str() {
                Some("allow") => "Allowed".into(),
                Some(_) => "Denied".into(),
                None if card["dismissed"] == true => "Denied".into(),
                None => card["title"].as_str().unwrap_or("Approval needed").to_string(),
            })
        }
        _ => None,
    }
}

/// Markdown as one plain line: the marks a person reads past are dropped and the lines joined.
/// A smaller cousin of their `plainPreviewText`; it covers what bots usually write.
pub fn plain(text: &str) -> String {
    let mut words = Vec::new();
    for raw in text.lines() {
        let mut line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") || is_rule(line) {
            continue;
        }
        line = line.trim_start_matches('>').trim_start();
        line = line.trim_start_matches('#').trim_start();
        line = strip_list_marker(line);
        let line: String = line.chars().filter(|c| !matches!(c, '*' | '`')).collect();
        let line = links_to_text(&line.replace("~~", ""));
        words.extend(line.split_whitespace().map(str::to_string));
    }
    words.join(" ")
}

fn is_rule(line: &str) -> bool {
    let marks: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3 && (marks.chars().all(|c| c == '-') || marks.chars().all(|c| c == '*') || marks.chars().all(|c| c == '_') || marks.chars().all(|c| c == '='))
}

fn strip_list_marker(line: &str) -> &str {
    for marker in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return rest;
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits)
        && let Some(rest) = line[digits..].strip_prefix(". ").or_else(|| line[digits..].strip_prefix(") "))
    {
        return rest;
    }
    line
}

/// `[words](address)` → `words`, and `![alt](address)` → `alt`.
fn links_to_text(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(end) = rest[close..].find(')').map(|i| close + i) else { break };
        let before = &rest[..open];
        out.push_str(before.strip_suffix('!').unwrap_or(before));
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire(extra: Value) -> Value {
        let mut bot = json!({ "id": "b1", "threadId": "t1", "catbot": true, "name": "Kiwi", "color": "teal", "activity": "idle", "busy": false, "unread": false, "messages": [] });
        bot.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        bot
    }

    #[test]
    fn only_catbots_bots_are_listed() {
        let answer = json!({ "bots": [wire(json!({})), { "id": "maus", "name": "MausBot" }] });
        let bots = Bot::list(&answer);
        assert_eq!(bots.len(), 1);
        assert_eq!(bots[0].name, "Kiwi");
        assert_eq!(bots[0].color, "teal");
    }

    #[test]
    fn the_preview_is_the_newest_line() {
        let bot = Bot::from_wire(&wire(json!({ "messages": [
            { "kind": "text", "role": "user", "text": "write a poem", "at": 1.0 },
            { "kind": "text", "role": "bot", "text": "## A poem\n\n**Roses** are *red*,\n- see [the vault](obsidian://x)", "at": 2.0 },
        ] })))
        .unwrap();
        assert_eq!(bot.preview(), "A poem Roses are red, see the vault");
        assert_eq!(bot.last.unwrap().at, 2.0);
    }

    #[test]
    fn cards_steps_and_activity() {
        let card = |card: Value| Bot::from_wire(&wire(json!({ "messages": [{ "kind": "options", "card": card, "at": 1.0 }] }))).unwrap().preview();
        assert_eq!(card(json!({ "title": "Approval needed" })), "Approval needed");
        assert_eq!(card(json!({ "title": "Approval needed", "answered": "allow" })), "Allowed");
        assert_eq!(card(json!({ "title": "Approval needed", "answered": "deny" })), "Denied");
        let step = Bot::from_wire(&wire(json!({ "messages": [{ "kind": "activity", "tool": { "name": "notice: Codex is read-only" }, "at": 1.0 }] }))).unwrap();
        assert_eq!(step.preview(), "Codex is read-only");
        assert_eq!(Bot::from_wire(&wire(json!({ "busy": true, "activity": "working" }))).unwrap().preview(), "Working…");
        assert_eq!(Bot::from_wire(&wire(json!({ "busy": true, "activity": "waiting-on-you" }))).unwrap().preview(), "Waiting for you…");
    }

    #[test]
    fn search_and_initials() {
        let bot = Bot::from_wire(&wire(json!({ "messages": [{ "kind": "text", "text": "Hello there", "at": 1.0 }] }))).unwrap();
        assert!(bot.matches("kiw") && bot.matches("THERE") && bot.matches(" ") && !bot.matches("scout"));
        assert_eq!(bot.initial(), "K");
        assert_eq!(Bot { name: "  ·x".into(), ..bot }.initial(), "X");
        assert_eq!(plain("```\ncode\n```\n---\n1. first"), "code first");
    }
}
