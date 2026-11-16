//! Bots as the interface shows them: read from the server's `WireBot` JSON (the reference
//! interface's wire format), with the one-line preview the sidebar gives each.
//!
//! The preview rules follow OpenMausBot's `src/components/sidebar-preview.ts` (Apache-2.0, see NOTICE),
//! for the message kinds Catbot sends: text, activity steps and approval cards.

use serde_json::Value;

/// What a channel adds to the bot it's shown as.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelInfo {
    /// Its bots in order: (id, name, mark, colour).
    pub members: Vec<(String, String, String, String)>,
    /// The bot that answers when no one is @named.
    pub lead: String,
    pub folder: String,
}

impl ChannelInfo {
    fn from_wire(wire: &Value) -> Option<ChannelInfo> {
        let members = wire["members"].as_array()?;
        Some(ChannelInfo {
            members: members
                .iter()
                .filter_map(|m| {
                    Some((
                        m["id"].as_str()?.to_string(),
                        m["name"].as_str().unwrap_or_default().to_string(),
                        m["mark"].as_str().unwrap_or("cat-head").to_string(),
                        m["color"].as_str().unwrap_or("blue").to_string(),
                    ))
                })
                .collect(),
            lead: wire["lead"].as_str().unwrap_or_default().to_string(),
            folder: wire["folder"].as_str().unwrap_or_default().to_string(),
        })
    }

    pub fn lead_name(&self) -> String {
        self.members.iter().find(|m| m.0 == self.lead).map(|m| m.1.clone()).unwrap_or_default()
    }
}

/// One of Catbot's bots, as the sidebar needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Bot {
    pub id: String,
    /// The chat shown when the bot is opened (this window's choice once it has one).
    pub thread_id: String,
    /// All its chats, the most recently used first; archived ones are marked.
    pub chats: Vec<Chat>,
    pub name: String,
    /// One of the stylesheet's identity colours (`--identity-<color>`).
    pub color: String,
    /// Which picture it wears (`mascots::MARKS`).
    pub mark: String,
    /// Its instructions.
    pub soul: String,
    /// Which AI runs it: "claude", or a chat API such as "openai" (the server's `engines.rs`).
    pub engine: String,
    /// The model it asks for; empty for its engine's default.
    pub model: String,
    /// The AIs that take over when its own is at a limit, in order: (engine, model).
    pub backups: Vec<(String, String)>,
    /// Set when this "bot" is a channel: a group chat of several bots.
    pub channel: Option<ChannelInfo>,
    /// What it may do without asking: "ask", "edits", "auto" or "full".
    pub approval_mode: String,
    /// Whether it may use the Codex vault.
    pub codex: bool,
    /// Whether it's the Chief of Staff (starred in the sidebar).
    pub chief: bool,
    /// The skills it uses, from the library.
    pub skills: Vec<String>,
    pub activity: Activity,
    pub unread: bool,
    /// Its newest message worth a preview, if it has one.
    pub last: Option<Last>,
}

/// One of a bot's chats (their "task"), as the sidebar lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub activity: Activity,
    /// When it was last used, in ms since the epoch.
    pub updated: f64,
    pub archived: bool,
}

impl Chat {
    fn from_wire(wire: &Value) -> Option<Chat> {
        Some(Chat {
            id: wire["threadId"].as_str()?.to_string(),
            title: wire["title"].as_str().filter(|t| !t.trim().is_empty()).unwrap_or("New chat").to_string(),
            activity: Activity::from_wire(wire),
            updated: wire["updatedAt"].as_f64().unwrap_or(0.0),
            archived: wire.get("archivedAt").is_some_and(|a| !a.is_null()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Working,
    WaitingOnYou,
}

impl Activity {
    /// From a bot's or a chat's `activity` and `busy`.
    fn from_wire(wire: &Value) -> Activity {
        match wire["activity"].as_str() {
            Some("waiting-on-you") => Activity::WaitingOnYou,
            _ if wire["busy"] == true => Activity::Working,
            Some("working") => Activity::Working,
            _ => Activity::Idle,
        }
    }

    /// The name the bot pictures know this by (`BotMark`'s `state`).
    pub fn mood(self) -> &'static str {
        match self {
            Activity::Idle => "idle",
            Activity::Working => "working",
            Activity::WaitingOnYou => "waiting",
        }
    }
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
        let activity = Activity::from_wire(wire);
        let messages = wire["messages"].as_array().map(Vec::as_slice).unwrap_or_default();
        let chats = wire["tasks"].as_array().map(|tasks| tasks.iter().filter_map(Chat::from_wire).collect()).unwrap_or_default();
        Some(Bot {
            id: wire["id"].as_str()?.to_string(),
            thread_id: wire["threadId"].as_str().unwrap_or_default().to_string(),
            chats,
            name: wire["name"].as_str().unwrap_or_default().to_string(),
            color: wire["color"].as_str().unwrap_or("blue").to_string(),
            mark: wire["mark"].as_str().unwrap_or("cat-head").to_string(),
            soul: wire["soul"].as_str().unwrap_or_default().to_string(),
            engine: wire["modelSelection"]["instanceId"].as_str().filter(|e| !e.is_empty()).unwrap_or("claude").to_string(),
            model: wire["modelSelection"]["model"].as_str().unwrap_or_default().to_string(),
            channel: ChannelInfo::from_wire(&wire["channel"]),
            backups: wire["backups"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|b| Some((b["engine"].as_str()?.to_string(), b["model"].as_str().unwrap_or_default().to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            approval_mode: wire["approvalMode"].as_str().unwrap_or("ask").to_string(),
            codex: wire["codex"].as_bool().unwrap_or(true),
            chief: wire["chief"].as_bool().unwrap_or(false),
            skills: wire["skills"].as_array().map(|s| s.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
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

    /// Whether a search finds this bot: by its name, its preview or one of its chats' names,
    /// ignoring case.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.name.to_lowercase().contains(&query)
            || self.preview().to_lowercase().contains(&query)
            || self.chats.iter().any(|c| c.title.to_lowercase().contains(&query))
    }

    /// Whether this chat is one of the bot's.
    pub fn has_chat(&self, thread: &str) -> bool {
        self.thread_id == thread || self.chats.iter().any(|c| c.id == thread)
    }

    /// The open chat, if the bot's list has it.
    pub fn open_chat(&self) -> Option<&Chat> {
        self.chats.iter().find(|c| c.id == self.thread_id)
    }

    /// The chats in the list (not archived).
    pub fn listed_chats(&self) -> impl Iterator<Item = &Chat> {
        self.chats.iter().filter(|c| !c.archived)
    }

    /// The bot as its open chat sees it: busy only if that chat is.
    pub fn in_open_chat(&self) -> Bot {
        let mut bot = self.clone();
        if let Some(chat) = self.open_chat() {
            bot.activity = chat.activity;
        }
        bot
    }

    /// Whether a chat other than the open one is working or waiting.
    pub fn busy_elsewhere(&self) -> bool {
        self.chats.iter().any(|c| c.id != self.thread_id && c.activity != Activity::Idle)
    }

    /// A `bot` frame's news: its name, colour, chats and what it's doing. A frame carries no
    /// messages, so the preview is kept from what's known; and this window stays on the chat
    /// it has open, as long as that chat still exists.
    pub fn update_from(&mut self, newer: Bot) {
        let last = self.last.take();
        let thread_id = std::mem::take(&mut self.thread_id);
        *self = newer;
        if self.last.is_none() {
            self.last = last;
        }
        if !thread_id.is_empty() && (self.chats.is_empty() || self.chats.iter().any(|c| c.id == thread_id)) {
            self.thread_id = thread_id;
        }
    }

    /// A new or changed message in this bot's conversation, for the preview.
    pub fn note_message(&mut self, message: &Value) {
        if let Some(line) = line(message) {
            let at = message["at"].as_f64().unwrap_or(0.0);
            if self.last.as_ref().is_none_or(|last| at >= last.at) {
                self.last = Some(Last { line, at });
            }
        }
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
    fn frames_keep_what_they_dont_carry() {
        let mut bot = Bot::from_wire(&wire(json!({ "messages": [{ "kind": "text", "text": "old", "at": 1.0 }] }))).unwrap();
        let frame = Bot::from_wire(&json!({ "id": "b1", "catbot": true, "name": "Kiwi 2", "color": "red", "activity": "working", "busy": true })).unwrap();
        bot.update_from(frame);
        assert_eq!((bot.name.as_str(), bot.activity, bot.thread_id.as_str()), ("Kiwi 2", Activity::Working, "t1"));
        assert_eq!(bot.last.as_ref().unwrap().line, "old");
        bot.note_message(&json!({ "kind": "text", "text": "**new**", "at": 2.0 }));
        assert_eq!(bot.last.as_ref().unwrap().line, "new");
        bot.note_message(&json!({ "kind": "text", "text": "older", "at": 0.5 }));
        assert_eq!(bot.last.as_ref().unwrap().line, "new");
    }

    #[test]
    fn chats_their_activity_and_which_is_open() {
        let tasks = json!([
            { "threadId": "t2", "title": "Shed plans", "activity": "working", "busy": true, "updatedAt": 5.0 },
            { "threadId": "t1", "title": "Garden", "activity": "idle", "busy": false, "updatedAt": 3.0 },
            { "threadId": "t0", "title": "", "activity": "idle", "busy": false, "updatedAt": 1.0, "archivedAt": 2.0 },
        ]);
        let mut bot = Bot::from_wire(&wire(json!({ "tasks": tasks, "activity": "working", "busy": true }))).unwrap();
        assert_eq!(bot.chats.len(), 3);
        assert_eq!(bot.listed_chats().count(), 2);
        assert_eq!(bot.chats[2].title, "New chat");
        assert!(bot.chats[2].archived);
        // The open chat (t1) is idle though another chat works: the chat view sees it idle.
        assert_eq!(bot.open_chat().unwrap().title, "Garden");
        assert_eq!(bot.in_open_chat().activity, Activity::Idle);
        assert!(bot.busy_elsewhere());
        assert!(bot.matches("shed") && bot.has_chat("t0") && !bot.has_chat("t9"));

        // A frame from the server opening another chat doesn't move this window off its chat…
        let frame = Bot::from_wire(&json!({ "id": "b1", "catbot": true, "threadId": "t2", "tasks": tasks })).unwrap();
        bot.update_from(frame);
        assert_eq!(bot.thread_id, "t1");
        // …until that chat is gone.
        let gone = json!([{ "threadId": "t2", "title": "Shed plans", "activity": "idle", "busy": false }]);
        bot.update_from(Bot::from_wire(&json!({ "id": "b1", "catbot": true, "threadId": "t2", "tasks": gone })).unwrap());
        assert_eq!(bot.thread_id, "t2");
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
