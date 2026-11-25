//! Catbot's own bookkeeping: bots, their conversations, and what was said. One SQLite file.
//!
//! What a bot *knows* (the person, their projects) belongs in the Codex vault, not here. This
//! is what Catbot needs to run bots and show their history.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::claude::PermissionMode;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The user's home folder (`HOME`, or `USERPROFILE` on Windows).
pub fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where Catbot keeps its data: `$CATBOT_HOME`, else `$XDG_DATA_HOME/catbot`, else
/// `~/.local/share/catbot` (on Windows, `%LOCALAPPDATA%\Catbot`).
pub fn default_home() -> PathBuf {
    if let Some(home) = std::env::var_os("CATBOT_HOME") {
        return PathBuf::from(home);
    }
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty())
    {
        return PathBuf::from(local).join("Catbot");
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(data).join("catbot");
    }
    user_home().join(".local").join("share").join("catbot")
}

/// The Codex vault: `$CATBOT_CODEX`, else `~/VaultCodex`.
pub fn default_codex() -> PathBuf {
    if let Some(codex) = std::env::var_os("CATBOT_CODEX") {
        return PathBuf::from(codex);
    }
    user_home().join("VaultCodex")
}

/// A channel: a group chat of several bots, with its own instructions and folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// `ch-…`, so it can't be mistaken for a bot's id.
    pub id: String,
    pub name: String,
    /// What every member is told about this channel.
    pub instructions: String,
    /// The channel's own working folder, shared by its members while they work here.
    pub folder: PathBuf,
    /// The bot that answers when no one is @named.
    pub lead: Option<String>,
    pub color: String,
    pub created_at: String,
    /// Its bots, in order.
    pub members: Vec<String>,
}

/// What one turn used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageRow {
    pub bot_id: String,
    pub conversation_id: String,
    pub engine: String,
    pub model: String,
    /// "plan" (a subscription covers it), "key" (an API key, paid by use) or "free" (local).
    pub billing: String,
    pub at: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub estimated: bool,
    pub cost_usd: Option<f64>,
}

/// A file attached to a message: kept in its bot's folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub bot_id: String,
    /// The name it came with.
    pub name: String,
    pub path: PathBuf,
    pub mime: String,
    pub size: u64,
    pub created_at: String,
}

/// A chat API's saved key, and its address if it isn't the usual one.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineKey {
    pub key: String,
    pub url: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub instructions: String,
    /// Which AI runs it (`engines.rs`): "claude", or a chat API such as "openai" or "xai".
    pub engine: String,
    /// None: the engine's default model.
    pub model: Option<String>,
    #[serde(serialize_with = "mode_str")]
    pub mode: PermissionMode,
    /// Whether the bot may use the Codex vault.
    pub codex: bool,
    /// The bot's own working folder.
    pub folder: PathBuf,
    pub created_at: String,
    /// Which picture the interface draws for it.
    #[serde(serialize_with = "mark_str")]
    pub mark: Mark,
    /// Its colour, one of `COLORS`. None: picked from its id, so it never changes on its own.
    pub color: Option<String>,
    /// How hard it thinks before answering, in its engine's words ("low", "high", an agent's own
    /// value). None: the engine's (or model's) own default.
    pub effort: Option<String>,
}

/// A bot's profile (stage 23): its role and description, its own picture (a file in `pictures/`)
/// and how it's framed, and when it was put away in the archive.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    pub title: String,
    pub description: String,
    pub picture: Option<String>,
    pub zoom: f64,
    pub focus_x: f64,
    pub focus_y: f64,
    pub archived_at: Option<String>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile { title: String::new(), description: String::new(), picture: None, zoom: 1.0, focus_x: 0.5, focus_y: 0.5, archived_at: None }
    }
}

/// A folder a bot's chats can be put in (stage 23.3, MausBot's projects).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatFolder {
    pub id: String,
    pub name: String,
    pub emoji: String,
}

/// The colours a bot can wear: the interface's identity colours.
pub const COLORS: [&str; 12] = ["green", "blue", "red", "orange", "purple", "cyan", "pink", "yellow", "teal", "coral", "silver", "chrome"];

/// A bot's picture: one of Catbot's three cats, or the first letter of its name in a circle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    CatHead,
    CatSitting,
    CatVisor,
    Letter,
    /// The robot cat's head (stage 25.3), rendered in Blender like the desktop pet.
    RobotHead,
    /// The robot cat sitting, whole.
    RobotFull,
}

impl Mark {
    pub const ALL: [Mark; 6] = [Mark::CatHead, Mark::CatSitting, Mark::CatVisor, Mark::Letter, Mark::RobotHead, Mark::RobotFull];

    pub fn as_str(self) -> &'static str {
        match self {
            Mark::CatHead => "cat-head",
            Mark::CatSitting => "cat-sitting",
            Mark::CatVisor => "cat-visor",
            Mark::Letter => "letter",
            Mark::RobotHead => "robot-head",
            Mark::RobotFull => "robot-full",
        }
    }

    pub fn parse(text: &str) -> Option<Mark> {
        Mark::ALL.into_iter().find(|m| m.as_str() == text)
    }
}

fn mark_str<S: serde::Serializer>(mark: &Mark, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(mark.as_str())
}

fn mode_str<S: serde::Serializer>(mode: &PermissionMode, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(mode.as_str())
}

/// What it takes to make a bot.
#[derive(Debug, Clone)]
pub struct NewBot {
    pub name: String,
    pub instructions: String,
    pub model: Option<String>,
    pub mode: PermissionMode,
    pub codex: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conversation {
    pub id: String,
    pub bot_id: String,
    /// Claude's own id for the conversation, used to continue it.
    pub session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Its name in the chat list: none until the first message.
    pub title: Option<String>,
    /// The person named it (so an automatic name never replaces theirs).
    pub title_by_person: bool,
    /// When it was put away in the bot's archived chats; none while it's in the list.
    pub archived_at: Option<String>,
    /// The channel whose transcript this is; none for a bot's own chat.
    pub channel_id: Option<String>,
    /// A bot answered since the person last looked (stage 19.5).
    pub unread: bool,
}

/// A device paired to reach Catbot from away (`server::remote`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub created_at: String,
    /// When it last asked for anything; none until it does.
    pub last_seen_at: Option<String>,
}

/// A conversation's columns, in the order `conversation_from_row` reads them.
const CONVERSATION: &str = "id, bot_id, session_id, created_at, updated_at, title, title_by_person, archived_at, channel_id, unread";

/// Who a stored message is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The person.
    Person,
    /// The bot's reply.
    Bot,
    /// Something the bot did on the way: a tool, an approval, an error (details in `detail`).
    Step,
    /// Catbot's own note: stopped, a conversation restarted.
    Note,
    /// A card the person answers (an approval), kept in `detail.card` and updated when answered.
    Card,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Person => "person",
            Role::Bot => "bot",
            Role::Step => "step",
            Role::Note => "note",
            Role::Card => "card",
        }
    }
    fn parse(s: &str) -> Role {
        match s {
            "person" => Role::Person,
            "bot" => Role::Bot,
            "step" => Role::Step,
            "card" => Role::Card,
            _ => Role::Note,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    pub id: i64,
    pub role: Role,
    pub body: String,
    pub detail: Option<Value>,
    pub at: String,
}

pub struct Store {
    conn: Connection,
    home: PathBuf,
}

const SCHEMA: &str = "
CREATE TABLE bots (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE COLLATE NOCASE,
    instructions TEXT NOT NULL DEFAULT '',
    model        TEXT,
    mode         TEXT NOT NULL DEFAULT 'default',
    codex        INTEGER NOT NULL DEFAULT 1,
    folder       TEXT NOT NULL,
    created_at   TEXT NOT NULL
);
CREATE TABLE conversations (
    id         TEXT PRIMARY KEY,
    bot_id     TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    session_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX conversations_by_bot ON conversations(bot_id, updated_at);
CREATE TABLE messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    body            TEXT NOT NULL,
    detail          TEXT,
    at              TEXT NOT NULL
);
CREATE INDEX messages_by_conversation ON messages(conversation_id, id);
";

/// The storage format this Catbot writes: the last of `open`'s steps below. A release records
/// it, so going back to an older Catbot knows whether that one can still read the chats
/// (stage 28, updates with rollback). Raise it with each new step.
pub const STORE_VERSION: i64 = 12;

/// How many runs a bot keeps.
pub const RUNS_KEPT: i64 = 200;

/// One turn of a bot's, as the panel's Activity and Inspector show it. `outcome` is "done",
/// "failed" or "stopped"; `events` what happened, trimmed (`server::runs`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Run {
    pub id: i64,
    pub bot_id: String,
    pub conversation_id: String,
    pub turn_id: String,
    pub started_at: String,
    pub ended_at: String,
    pub outcome: String,
    pub engine: String,
    pub model: String,
    pub tools: i64,
    pub errors: i64,
    pub cost_usd: Option<f64>,
    pub events: Value,
}

fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        bot_id: r.get(1)?,
        conversation_id: r.get(2)?,
        turn_id: r.get(3)?,
        started_at: r.get(4)?,
        ended_at: r.get(5)?,
        outcome: r.get(6)?,
        engine: r.get(7)?,
        model: r.get(8)?,
        tools: r.get(9)?,
        errors: r.get(10)?,
        cost_usd: r.get(11)?,
        events: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or(Value::Array(Vec::new())),
    })
}

fn now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

impl Store {
    /// Opens (or creates) the store in `home`, a folder only this user can open.
    pub fn open(home: &Path) -> Result<Self, BoxError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            if !home.exists() {
                std::fs::DirBuilder::new().recursive(true).mode(0o700).create(home)?;
            } else {
                let mut perms = std::fs::metadata(home)?.permissions();
                if perms.mode() & 0o077 != 0 {
                    perms.set_mode(0o700);
                    std::fs::set_permissions(home, perms)?;
                }
            }
        }
        // On Windows, a folder under the user's own AppData is already closed to other users.
        #[cfg(not(unix))]
        std::fs::create_dir_all(home)?;
        let conn = Connection::open(home.join("catbot.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(&format!("BEGIN; {SCHEMA} PRAGMA user_version = 1; COMMIT;"))?;
        }
        if version < 2 {
            // Version 2: a bot's picture and colour. Bots from before keep the defaults (NULL).
            conn.execute_batch(
                "BEGIN; ALTER TABLE bots ADD COLUMN mark TEXT; ALTER TABLE bots ADD COLUMN color TEXT; PRAGMA user_version = 2; COMMIT;",
            )?;
        }
        if version < 3 {
            // Version 3: several chats per bot. Each has a name and can be archived; a bot
            // remembers which chat it opens to. Conversations from before are untitled.
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE conversations ADD COLUMN title TEXT;
                 ALTER TABLE conversations ADD COLUMN title_by_person INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE conversations ADD COLUMN archived_at TEXT;
                 ALTER TABLE bots ADD COLUMN open_conversation TEXT;
                 PRAGMA user_version = 3;
                 COMMIT;",
            )?;
        }
        if version < 4 {
            // Version 4: devices paired to reach Catbot from away (the phone app). Only a hash
            // of each device's key is kept.
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE devices (
                     id           TEXT PRIMARY KEY,
                     name         TEXT NOT NULL,
                     key_hash     TEXT NOT NULL UNIQUE,
                     created_at   TEXT NOT NULL,
                     last_seen_at TEXT
                 );
                 PRAGMA user_version = 4;
                 COMMIT;",
            )?;
        }
        if version < 5 {
            // Version 5: which AI each bot runs on (bots from before run on Claude), and the keys
            // for the chat APIs, kept here in Catbot's private folder and never sent out.
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE bots ADD COLUMN engine TEXT NOT NULL DEFAULT 'claude';
                 CREATE TABLE engine_keys (
                     engine     TEXT PRIMARY KEY,
                     key        TEXT NOT NULL,
                     url        TEXT,
                     updated_at TEXT NOT NULL
                 );
                 PRAGMA user_version = 5;
                 COMMIT;",
            )?;
        }
        if version < 6 {
            // Version 6: files the person attaches to messages. The file lives in the bot's folder
            // (`attachments/`); this keeps what it is and who it belongs to.
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE attachments (
                     id         TEXT PRIMARY KEY,
                     bot_id     TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                     name       TEXT NOT NULL,
                     path       TEXT NOT NULL,
                     mime       TEXT NOT NULL,
                     size       INTEGER NOT NULL,
                     created_at TEXT NOT NULL
                 );
                 PRAGMA user_version = 6;
                 COMMIT;",
            )?;
        }
        if version < 7 {
            // Version 7: what each turn used (for the usage view and limits), and Catbot's own
            // settings (limits, backups) as JSON by name.
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE usage (
                     id              INTEGER PRIMARY KEY,
                     bot_id          TEXT NOT NULL,
                     conversation_id TEXT NOT NULL,
                     engine          TEXT NOT NULL,
                     model           TEXT NOT NULL,
                     billing         TEXT NOT NULL,
                     at              TEXT NOT NULL,
                     input           INTEGER NOT NULL,
                     output          INTEGER NOT NULL,
                     cache_read      INTEGER NOT NULL,
                     cache_write     INTEGER NOT NULL,
                     estimated       INTEGER NOT NULL,
                     cost_usd        REAL
                 );
                 CREATE INDEX usage_at ON usage(at);
                 CREATE TABLE settings (
                     key   TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 PRAGMA user_version = 7;
                 COMMIT;",
            )?;
        }
        if version < 8 {
            // Version 8: channels, group chats of several bots. A channel's transcript is a
            // conversation marked with its channel (held by one of its members, for the key).
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE channels (
                     id           TEXT PRIMARY KEY,
                     name         TEXT NOT NULL,
                     instructions TEXT NOT NULL DEFAULT '',
                     folder       TEXT NOT NULL,
                     lead_bot_id  TEXT,
                     color        TEXT NOT NULL DEFAULT 'blue',
                     created_at   TEXT NOT NULL
                 );
                 CREATE TABLE channel_members (
                     channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
                     bot_id     TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                     position   INTEGER NOT NULL,
                     PRIMARY KEY (channel_id, bot_id)
                 );
                 ALTER TABLE conversations ADD COLUMN channel_id TEXT;
                 PRAGMA user_version = 8;
                 COMMIT;",
            )?;
        }
        if version < 9 {
            // Version 9: a bot's thinking level.
            conn.execute_batch("BEGIN; ALTER TABLE bots ADD COLUMN effort TEXT; PRAGMA user_version = 9; COMMIT;")?;
        }
        if version < 10 {
            // Version 10: each turn's run, with what happened in it (the panel's Activity and Inspector).
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE runs (
                     id              INTEGER PRIMARY KEY AUTOINCREMENT,
                     bot_id          TEXT NOT NULL,
                     conversation_id TEXT NOT NULL,
                     turn_id         TEXT NOT NULL,
                     started_at      TEXT NOT NULL,
                     ended_at        TEXT NOT NULL,
                     outcome         TEXT NOT NULL,
                     engine          TEXT NOT NULL,
                     model           TEXT NOT NULL,
                     tools           INTEGER NOT NULL,
                     errors          INTEGER NOT NULL,
                     cost_usd        REAL,
                     events          TEXT NOT NULL
                 );
                 CREATE INDEX runs_by_bot ON runs(bot_id, id);
                 PRAGMA user_version = 10;
                 COMMIT;",
            )?;
        }
        if version < 11 {
            // Version 11: a chat a bot answered in since the person last looked (the Inbox's unread).
            conn.execute_batch("BEGIN; ALTER TABLE conversations ADD COLUMN unread INTEGER NOT NULL DEFAULT 0; PRAGMA user_version = 11; COMMIT;")?;
        }
        if version < 12 {
            // Version 12 (stage 23): a bot's profile (role, description, its own picture and how it's
            // framed, put away in the archive), its chat folders, and the history of its settings.
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE bot_profiles (
                     bot_id      TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
                     title       TEXT NOT NULL DEFAULT '',
                     description TEXT NOT NULL DEFAULT '',
                     picture     TEXT,
                     zoom        REAL NOT NULL DEFAULT 1,
                     focus_x     REAL NOT NULL DEFAULT 0.5,
                     focus_y     REAL NOT NULL DEFAULT 0.5,
                     archived_at TEXT
                 );
                 CREATE TABLE chat_folders (
                     id       TEXT PRIMARY KEY,
                     bot_id   TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                     name     TEXT NOT NULL,
                     emoji    TEXT NOT NULL DEFAULT '',
                     position INTEGER NOT NULL DEFAULT 0
                 );
                 ALTER TABLE conversations ADD COLUMN folder_id TEXT;
                 CREATE TABLE bot_history (
                     id     INTEGER PRIMARY KEY AUTOINCREMENT,
                     bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                     at     TEXT NOT NULL,
                     what   TEXT NOT NULL,
                     before TEXT NOT NULL
                 );
                 CREATE INDEX bot_history_by_bot ON bot_history(bot_id, id);
                 PRAGMA user_version = 12;
                 COMMIT;",
            )?;
        }
        Ok(Self { conn, home: home.to_path_buf() })
    }

    /// A bot's chat folders (stage 23.3), in their order.
    pub fn chat_folders(&self, bot_id: &str) -> Result<Vec<ChatFolder>, BoxError> {
        let mut stmt = self.conn.prepare("SELECT id, name, emoji FROM chat_folders WHERE bot_id = ?1 ORDER BY position, rowid")?;
        let rows = stmt.query_map([bot_id], |r| Ok(ChatFolder { id: r.get(0)?, name: r.get(1)?, emoji: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_chat_folder(&self, bot_id: &str, name: &str, emoji: &str) -> Result<ChatFolder, BoxError> {
        let id = format!("fold-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let next: i64 = self.conn.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM chat_folders WHERE bot_id = ?1", [bot_id], |r| r.get(0))?;
        self.conn.execute("INSERT INTO chat_folders (id, bot_id, name, emoji, position) VALUES (?1, ?2, ?3, ?4, ?5)", params![id, bot_id, name, emoji, next])?;
        Ok(ChatFolder { id, name: name.to_string(), emoji: emoji.to_string() })
    }

    pub fn change_chat_folder(&self, id: &str, name: &str, emoji: &str) -> Result<(), BoxError> {
        self.conn.execute("UPDATE chat_folders SET name = ?2, emoji = ?3 WHERE id = ?1", params![id, name, emoji])?;
        Ok(())
    }

    /// Takes a folder away; its chats stay, out of any folder.
    pub fn delete_chat_folder(&self, id: &str) -> Result<(), BoxError> {
        self.conn.execute("UPDATE conversations SET folder_id = NULL WHERE folder_id = ?1", [id])?;
        self.conn.execute("DELETE FROM chat_folders WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Puts a bot's folders in this order (ids not listed keep their place after).
    pub fn order_chat_folders(&self, ids: &[String]) -> Result<(), BoxError> {
        for (i, id) in ids.iter().enumerate() {
            self.conn.execute("UPDATE chat_folders SET position = ?2 WHERE id = ?1", params![id, i as i64])?;
        }
        Ok(())
    }

    pub fn set_chat_folder(&self, conversation_id: &str, folder: Option<&str>) -> Result<(), BoxError> {
        self.conn.execute("UPDATE conversations SET folder_id = ?2 WHERE id = ?1", params![conversation_id, folder])?;
        Ok(())
    }

    /// Which folder each of a bot's chats is in.
    pub fn chat_folder_map(&self, bot_id: &str) -> Result<std::collections::HashMap<String, String>, BoxError> {
        let mut stmt = self.conn.prepare("SELECT id, folder_id FROM conversations WHERE bot_id = ?1 AND folder_id IS NOT NULL")?;
        let rows = stmt.query_map([bot_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Keeps a bot's settings as they were before a change (stage 23.4), the newest 100.
    pub fn add_history(&self, bot_id: &str, what: &str, before: &Value) -> Result<(), BoxError> {
        self.conn.execute("INSERT INTO bot_history (bot_id, at, what, before) VALUES (?1, ?2, ?3, ?4)", params![bot_id, now(), what, before.to_string()])?;
        self.conn.execute(
            "DELETE FROM bot_history WHERE bot_id = ?1 AND id NOT IN (SELECT id FROM bot_history WHERE bot_id = ?1 ORDER BY id DESC LIMIT 100)",
            [bot_id],
        )?;
        Ok(())
    }

    /// A bot's changes, newest first: (id, when, what changed).
    pub fn history(&self, bot_id: &str) -> Result<Vec<(i64, String, String)>, BoxError> {
        let mut stmt = self.conn.prepare("SELECT id, at, what FROM bot_history WHERE bot_id = ?1 ORDER BY id DESC")?;
        let rows = stmt.query_map([bot_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One change's settings from before it.
    pub fn history_before(&self, bot_id: &str, id: i64) -> Result<Option<Value>, BoxError> {
        let text: Option<String> = self.conn.query_row("SELECT before FROM bot_history WHERE bot_id = ?1 AND id = ?2", params![bot_id, id], |r| r.get(0)).optional()?;
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// A bot's profile (stage 23): the defaults when nothing was set.
    pub fn profile(&self, bot_id: &str) -> Result<Profile, BoxError> {
        Ok(self
            .conn
            .query_row("SELECT title, description, picture, zoom, focus_x, focus_y, archived_at FROM bot_profiles WHERE bot_id = ?1", [bot_id], |r| {
                Ok(Profile { title: r.get(0)?, description: r.get(1)?, picture: r.get(2)?, zoom: r.get(3)?, focus_x: r.get(4)?, focus_y: r.get(5)?, archived_at: r.get(6)? })
            })
            .optional()?
            .unwrap_or_default())
    }

    pub fn set_profile(&self, bot_id: &str, p: &Profile) -> Result<(), BoxError> {
        self.conn.execute(
            "INSERT INTO bot_profiles (bot_id, title, description, picture, zoom, focus_x, focus_y, archived_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(bot_id) DO UPDATE SET title = excluded.title, description = excluded.description, picture = excluded.picture,
               zoom = excluded.zoom, focus_x = excluded.focus_x, focus_y = excluded.focus_y, archived_at = excluded.archived_at",
            params![bot_id, p.title, p.description, p.picture, p.zoom, p.focus_x, p.focus_y, p.archived_at],
        )?;
        Ok(())
    }

    /// Records what a turn used.
    pub fn add_usage(&self, row: &UsageRow) -> Result<(), BoxError> {
        self.conn.execute(
            "INSERT INTO usage (bot_id, conversation_id, engine, model, billing, at, input, output, cache_read, cache_write, estimated, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                row.bot_id,
                row.conversation_id,
                row.engine,
                row.model,
                row.billing,
                row.at,
                row.input as i64,
                row.output as i64,
                row.cache_read as i64,
                row.cache_write as i64,
                row.estimated,
                row.cost_usd
            ],
        )?;
        Ok(())
    }

    /// Keeps a turn's run, and only a bot's newest `RUNS_KEPT`.
    pub fn add_run(&self, run: &Run) -> Result<i64, BoxError> {
        self.conn.execute(
            "INSERT INTO runs (bot_id, conversation_id, turn_id, started_at, ended_at, outcome, engine, model, tools, errors, cost_usd, events)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![run.bot_id, run.conversation_id, run.turn_id, run.started_at, run.ended_at, run.outcome, run.engine, run.model, run.tools, run.errors, run.cost_usd, run.events.to_string()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM runs WHERE bot_id = ?1 AND id NOT IN (SELECT id FROM runs WHERE bot_id = ?1 ORDER BY id DESC LIMIT ?2)",
            params![run.bot_id, RUNS_KEPT],
        )?;
        Ok(id)
    }

    /// A bot's runs, the newest first, without their events.
    pub fn runs(&self, bot_id: &str, limit: usize) -> Result<Vec<Run>, BoxError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, bot_id, conversation_id, turn_id, started_at, ended_at, outcome, engine, model, tools, errors, cost_usd, '[]'
             FROM runs WHERE bot_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![bot_id, limit as i64], run_row)?.collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// One of a bot's runs, with its events.
    pub fn run(&self, bot_id: &str, id: i64) -> Result<Option<Run>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, bot_id, conversation_id, turn_id, started_at, ended_at, outcome, engine, model, tools, errors, cost_usd, events
                 FROM runs WHERE bot_id = ?1 AND id = ?2",
                params![bot_id, id],
                run_row,
            )
            .optional()?)
    }

    /// Every turn's usage since a time (an RFC 3339 string, as stored), oldest first.
    pub fn usage_since(&self, since: &str) -> Result<Vec<UsageRow>, BoxError> {
        let mut stmt = self.conn.prepare(
            "SELECT bot_id, conversation_id, engine, model, billing, at, input, output, cache_read, cache_write, estimated, cost_usd
             FROM usage WHERE at >= ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([since], |r| {
                Ok(UsageRow {
                    bot_id: r.get(0)?,
                    conversation_id: r.get(1)?,
                    engine: r.get(2)?,
                    model: r.get(3)?,
                    billing: r.get(4)?,
                    at: r.get(5)?,
                    input: r.get::<_, i64>(6)? as u64,
                    output: r.get::<_, i64>(7)? as u64,
                    cache_read: r.get::<_, i64>(8)? as u64,
                    cache_write: r.get::<_, i64>(9)? as u64,
                    estimated: r.get(10)?,
                    cost_usd: r.get(11)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// One of Catbot's settings, or None.
    pub fn setting(&self, key: &str) -> Result<Option<Value>, BoxError> {
        let raw: Option<String> = self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?;
        Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()))
    }

    pub fn set_setting(&self, key: &str, value: &Value) -> Result<(), BoxError> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    /// Keeps a file attached to a bot's message (already saved at `path`).
    pub fn add_attachment(&self, bot_id: &str, name: &str, path: &Path, mime: &str, size: u64) -> Result<Attachment, BoxError> {
        let attachment = Attachment {
            id: uuid::Uuid::new_v4().simple().to_string(),
            bot_id: bot_id.to_string(),
            name: name.to_string(),
            path: path.to_path_buf(),
            mime: mime.to_string(),
            size,
            created_at: now(),
        };
        self.conn.execute(
            "INSERT INTO attachments (id, bot_id, name, path, mime, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![attachment.id, attachment.bot_id, attachment.name, attachment.path.to_string_lossy(), attachment.mime, attachment.size as i64, attachment.created_at],
        )?;
        Ok(attachment)
    }

    pub fn attachment(&self, id: &str) -> Result<Option<Attachment>, BoxError> {
        Ok(self
            .conn
            .query_row("SELECT id, bot_id, name, path, mime, size, created_at FROM attachments WHERE id = ?1", [id], |r| {
                Ok(Attachment {
                    id: r.get(0)?,
                    bot_id: r.get(1)?,
                    name: r.get(2)?,
                    path: PathBuf::from(r.get::<_, String>(3)?),
                    mime: r.get(4)?,
                    size: r.get::<_, i64>(5)? as u64,
                    created_at: r.get(6)?,
                })
            })
            .optional()?)
    }

    /// The key (and any other address) saved for a chat API.
    pub fn engine_key(&self, engine: &str) -> Result<Option<EngineKey>, BoxError> {
        Ok(self
            .conn
            .query_row("SELECT key, url, updated_at FROM engine_keys WHERE engine = ?1", [engine], |r| {
                Ok(EngineKey { key: r.get(0)?, url: r.get(1)?, updated_at: r.get(2)? })
            })
            .optional()?)
    }

    /// Saves a chat API's key, replacing any before it.
    pub fn set_engine_key(&self, engine: &str, key: &str, url: Option<&str>) -> Result<(), BoxError> {
        self.conn.execute(
            "INSERT INTO engine_keys (engine, key, url, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(engine) DO UPDATE SET key = excluded.key, url = excluded.url, updated_at = excluded.updated_at",
            params![engine, key, url, now()],
        )?;
        Ok(())
    }

    /// Forgets a chat API's key. False if there was none.
    pub fn remove_engine_key(&self, engine: &str) -> Result<bool, BoxError> {
        Ok(self.conn.execute("DELETE FROM engine_keys WHERE engine = ?1", [engine])? > 0)
    }

    /// Pairs a device: `key_hash` is the hash of the key it was given.
    pub fn add_device(&self, name: &str, key_hash: &str) -> Result<Device, BoxError> {
        let device = Device { id: uuid::Uuid::new_v4().to_string(), name: name.to_string(), created_at: now(), last_seen_at: None };
        self.conn.execute(
            "INSERT INTO devices (id, name, key_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![device.id, device.name, key_hash, device.created_at],
        )?;
        Ok(device)
    }

    /// The device whose key hashes to this, if it's still paired.
    pub fn device_by_key_hash(&self, key_hash: &str) -> Result<Option<Device>, BoxError> {
        Ok(self
            .conn
            .query_row("SELECT id, name, created_at, last_seen_at FROM devices WHERE key_hash = ?1", [key_hash], device_from_row)
            .optional()?)
    }

    /// Paired devices, the newest first.
    pub fn devices(&self) -> Result<Vec<Device>, BoxError> {
        let mut stmt = self.conn.prepare("SELECT id, name, created_at, last_seen_at FROM devices ORDER BY created_at DESC")?;
        let list = stmt.query_map([], device_from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(list)
    }

    /// Unpairs a device: its key stops working. False if there was no such device.
    pub fn remove_device(&self, id: &str) -> Result<bool, BoxError> {
        Ok(self.conn.execute("DELETE FROM devices WHERE id = ?1", [id])? > 0)
    }

    /// Notes that a device was just heard from.
    pub fn device_seen(&self, id: &str) -> Result<(), BoxError> {
        self.conn.execute("UPDATE devices SET last_seen_at = ?2 WHERE id = ?1", params![id, now()])?;
        Ok(())
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn create_bot(&self, new: NewBot) -> Result<Bot, BoxError> {
        let name = new.name.trim().to_string();
        check_name(&name)?;
        if self.bot(&name)?.is_some() {
            return Err(format!("a bot named {name} already exists").into());
        }
        let folder = self.free_folder(&slug(&name));
        std::fs::create_dir_all(&folder)?;
        let bot = Bot {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            instructions: new.instructions.trim().to_string(),
            engine: crate::engines::CLAUDE.to_string(),
            model: new.model.filter(|m| !m.trim().is_empty()),
            mode: new.mode,
            codex: new.codex,
            folder,
            created_at: now(),
            mark: Mark::default(),
            color: None,
            effort: None,
        };
        self.conn.execute(
            "INSERT INTO bots (id, name, instructions, model, mode, codex, folder, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![bot.id, bot.name, bot.instructions, bot.model, bot.mode.as_str(), bot.codex, bot.folder.to_string_lossy(), bot.created_at],
        )?;
        Ok(bot)
    }

    /// A folder under `home/bots` no other bot has.
    fn free_folder(&self, slug: &str) -> PathBuf {
        let base = self.home.join("bots");
        let mut folder = base.join(slug);
        let mut n = 2;
        while folder.exists() {
            folder = base.join(format!("{slug}-{n}"));
            n += 1;
        }
        folder
    }

    /// The bots, but those put away in the archive (stage 23.5).
    pub fn bots(&self) -> Result<Vec<Bot>, BoxError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, instructions, model, mode, codex, folder, created_at, mark, color, engine, effort FROM bots
             WHERE id NOT IN (SELECT bot_id FROM bot_profiles WHERE archived_at IS NOT NULL) ORDER BY name",
        )?;
        let bots = stmt.query_map([], bot_from_row)?.collect::<Result<_, _>>()?;
        Ok(bots)
    }

    /// Every bot, those in the archive too.
    pub fn all_bots(&self) -> Result<Vec<Bot>, BoxError> {
        let mut stmt = self.conn.prepare("SELECT id, name, instructions, model, mode, codex, folder, created_at, mark, color, engine, effort FROM bots ORDER BY name")?;
        let bots = stmt.query_map([], bot_from_row)?.collect::<Result<_, _>>()?;
        Ok(bots)
    }

    /// A bot by name, ignoring case.
    pub fn bot(&self, name: &str) -> Result<Option<Bot>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, instructions, model, mode, codex, folder, created_at, mark, color, engine, effort FROM bots WHERE name = ?1",
                [name.trim()],
                bot_from_row,
            )
            .optional()?)
    }

    pub fn bot_by_id(&self, id: &str) -> Result<Option<Bot>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, instructions, model, mode, codex, folder, created_at, mark, color, engine, effort FROM bots WHERE id = ?1",
                [id],
                bot_from_row,
            )
            .optional()?)
    }

    /// Saves a bot's name, instructions, model, mode and Codex access. Its folder keeps its name.
    pub fn update_bot(&self, bot: &Bot) -> Result<(), BoxError> {
        let name = bot.name.trim();
        check_name(name)?;
        if let Some(other) = self.bot(name)?.filter(|other| other.id != bot.id) {
            return Err(format!("a bot named {} already exists", other.name).into());
        }
        self.conn.execute(
            "UPDATE bots SET name = ?2, instructions = ?3, model = ?4, mode = ?5, codex = ?6, mark = ?7, color = ?8, engine = ?9, effort = ?10 WHERE id = ?1",
            params![bot.id, name, bot.instructions, bot.model, bot.mode.as_str(), bot.codex, bot.mark.as_str(), bot.color, bot.engine, bot.effort],
        )?;
        Ok(())
    }

    /// `wanted` if no bot has it, else `wanted 2`, `wanted 3`…
    pub fn free_name(&self, wanted: &str) -> Result<String, BoxError> {
        let wanted = wanted.trim();
        let mut name = wanted.to_string();
        let mut n = 2;
        while self.bot(&name)?.is_some() {
            name = format!("{wanted} {n}");
            n += 1;
        }
        Ok(name)
    }

    /// Removes a bot and its conversations. Its folder is left on disk.
    pub fn delete_bot(&self, name: &str) -> Result<Bot, BoxError> {
        let bot = self.bot(name)?.ok_or_else(|| format!("no bot named {name}"))?;
        // A channel transcript held by this bot passes to another member (or the channel goes,
        // if it has no one else), so deleting the bot doesn't delete the channel's talk.
        for channel in self.channels()? {
            let members: Vec<String> = channel.members.iter().filter(|m| **m != bot.id).cloned().collect();
            if channel.members.contains(&bot.id) && members.is_empty() {
                self.delete_channel(&channel.id)?;
                continue;
            }
            if let Some(next) = members.first() {
                self.conn.execute("UPDATE conversations SET bot_id = ?1 WHERE channel_id = ?2 AND bot_id = ?3", params![next, channel.id, bot.id])?;
                if channel.lead.as_deref() == Some(bot.id.as_str()) {
                    self.conn.execute("UPDATE channels SET lead_bot_id = ?1 WHERE id = ?2", params![next, channel.id])?;
                }
            }
        }
        self.conn.execute("DELETE FROM bots WHERE id = ?1", [&bot.id])?;
        Ok(bot)
    }

    /// Makes a channel: its folder, its members (in order) and its transcript.
    pub fn create_channel(&self, name: &str, members: &[String], lead: Option<&str>) -> Result<Channel, BoxError> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a channel needs a name".into());
        }
        if members.is_empty() {
            return Err("a channel needs at least one bot".into());
        }
        let id = format!("ch-{}", uuid::Uuid::new_v4().simple());
        let base = self.home.join("channels").join(slug(name));
        let folder = (1..).map(|n| if n == 1 { base.clone() } else { base.with_file_name(format!("{}-{n}", slug(name))) }).find(|p| !p.exists()).unwrap_or(base);
        std::fs::create_dir_all(&folder)?;
        let lead = lead.filter(|l| members.iter().any(|m| m == l)).map(str::to_string).unwrap_or_else(|| members[0].clone());
        self.conn.execute(
            "INSERT INTO channels (id, name, folder, lead_bot_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, folder.to_string_lossy(), lead, now()],
        )?;
        self.set_channel_members(&id, members)?;
        let at = now();
        self.conn.execute(
            "INSERT INTO conversations (id, bot_id, created_at, updated_at, title, channel_id) VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
            params![uuid::Uuid::new_v4().to_string(), lead, at, name, id],
        )?;
        self.channel(&id)?.ok_or_else(|| "the channel vanished".into())
    }

    fn set_channel_members(&self, channel_id: &str, members: &[String]) -> Result<(), BoxError> {
        self.conn.execute("DELETE FROM channel_members WHERE channel_id = ?1", [channel_id])?;
        for (i, bot) in members.iter().enumerate() {
            self.conn.execute(
                "INSERT OR IGNORE INTO channel_members (channel_id, bot_id, position) VALUES (?1, ?2, ?3)",
                params![channel_id, bot, i as i64],
            )?;
        }
        Ok(())
    }

    pub fn channels(&self) -> Result<Vec<Channel>, BoxError> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT id FROM channels ORDER BY created_at")?;
            stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        Ok(ids.iter().filter_map(|id| self.channel(id).ok().flatten()).collect())
    }

    pub fn channel(&self, id: &str) -> Result<Option<Channel>, BoxError> {
        let found = self
            .conn
            .query_row("SELECT id, name, instructions, folder, lead_bot_id, color, created_at FROM channels WHERE id = ?1", [id], |r| {
                Ok(Channel {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    instructions: r.get(2)?,
                    folder: PathBuf::from(r.get::<_, String>(3)?),
                    lead: r.get(4)?,
                    color: r.get(5)?,
                    created_at: r.get(6)?,
                    members: Vec::new(),
                })
            })
            .optional()?;
        let Some(mut channel) = found else { return Ok(None) };
        let mut stmt = self.conn.prepare("SELECT bot_id FROM channel_members WHERE channel_id = ?1 ORDER BY position")?;
        channel.members = stmt.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(Some(channel))
    }

    /// Saves a channel's name, instructions, colour, members and lead.
    pub fn update_channel(&self, channel: &Channel) -> Result<(), BoxError> {
        if channel.members.is_empty() {
            return Err("a channel needs at least one bot".into());
        }
        let lead = channel.lead.clone().filter(|l| channel.members.contains(l)).unwrap_or_else(|| channel.members[0].clone());
        self.conn.execute(
            "UPDATE channels SET name = ?1, instructions = ?2, color = ?3, lead_bot_id = ?4 WHERE id = ?5",
            params![channel.name.trim(), channel.instructions, channel.color, lead, channel.id],
        )?;
        self.set_channel_members(&channel.id, &channel.members)?;
        // The transcript is held by a member.
        self.conn.execute(
            "UPDATE conversations SET bot_id = ?1, title = ?2 WHERE channel_id = ?3",
            params![lead, channel.name.trim(), channel.id],
        )?;
        Ok(())
    }

    pub fn delete_channel(&self, id: &str) -> Result<(), BoxError> {
        self.conn.execute("DELETE FROM conversations WHERE channel_id = ?1", [id])?;
        self.conn.execute("DELETE FROM channels WHERE id = ?1", [id])?;
        Ok(())
    }

    /// A channel's transcript.
    pub fn channel_conversation(&self, channel_id: &str) -> Result<Option<Conversation>, BoxError> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {CONVERSATION} FROM conversations WHERE channel_id = ?1 LIMIT 1"), [channel_id], conversation_from_row)
            .optional()?)
    }

    /// The chat the bot opens to: the one last opened, else its latest chat in the list,
    /// else its latest archived one.
    pub fn current_conversation(&self, bot_id: &str) -> Result<Option<Conversation>, BoxError> {
        let open: Option<String> = self
            .conn
            .query_row("SELECT open_conversation FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))
            .optional()?
            .flatten();
        if let Some(open) = open
            && let Some(conversation) = self.conversation(&open)?
            && conversation.bot_id == bot_id
            && conversation.channel_id.is_none()
        {
            return Ok(Some(conversation));
        }
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {CONVERSATION} FROM conversations WHERE bot_id = ?1 AND channel_id IS NULL
                     ORDER BY archived_at IS NOT NULL, updated_at DESC, rowid DESC LIMIT 1"
                ),
                [bot_id],
                conversation_from_row,
            )
            .optional()?)
    }

    pub fn conversation(&self, id: &str) -> Result<Option<Conversation>, BoxError> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {CONVERSATION} FROM conversations WHERE id = ?1"), [id], conversation_from_row)
            .optional()?)
    }

    /// All of a bot's chats, archived ones included, the most recently used first.
    pub fn conversations(&self, bot_id: &str) -> Result<Vec<Conversation>, BoxError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {CONVERSATION} FROM conversations WHERE bot_id = ?1 AND channel_id IS NULL ORDER BY updated_at DESC, rowid DESC"))?;
        let list = stmt.query_map([bot_id], conversation_from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(list)
    }

    /// Makes a chat the one its bot opens to.
    pub fn open_conversation(&self, conversation: &Conversation) -> Result<(), BoxError> {
        self.conn.execute(
            "UPDATE bots SET open_conversation = ?2 WHERE id = ?1",
            params![conversation.bot_id, conversation.id],
        )?;
        Ok(())
    }

    /// A fresh chat for the bot, which it then opens to.
    pub fn new_conversation(&self, bot_id: &str) -> Result<Conversation, BoxError> {
        let conversation = self.start_conversation(bot_id, None)?;
        self.open_conversation(&conversation)?;
        Ok(conversation)
    }

    /// A new chat for the same bot holding a copy of this chat's messages up to and including
    /// `upto`, which the bot then opens to. The copy starts without a Claude session.
    pub fn branch_conversation(&self, from: &Conversation, upto: i64, title: &str) -> Result<Conversation, BoxError> {
        let branch = self.new_conversation(&from.bot_id)?;
        self.conn.execute(
            "INSERT INTO messages (conversation_id, role, body, detail, at)
             SELECT ?1, role, body, detail, at FROM messages WHERE conversation_id = ?2 AND id <= ?3 ORDER BY id",
            params![branch.id, from.id, upto],
        )?;
        self.set_title(&branch.id, title, true)?;
        Ok(branch)
    }

    /// Names a chat. `by_person` marks a name the person chose.
    pub fn set_title(&self, id: &str, title: &str, by_person: bool) -> Result<(), BoxError> {
        self.conn.execute(
            "UPDATE conversations SET title = ?2, title_by_person = ?3 WHERE id = ?1",
            params![id, title, by_person],
        )?;
        Ok(())
    }

    /// The first thing the person said in a chat: what an unnamed chat is named after.
    pub fn first_words(&self, conversation_id: &str) -> Result<Option<String>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT body FROM messages WHERE conversation_id = ?1 AND role = 'person' ORDER BY id LIMIT 1",
                [conversation_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Puts a chat in its bot's archived chats (`true`), or back in the list.
    pub fn set_archived(&self, id: &str, archived: bool) -> Result<(), BoxError> {
        let at = archived.then(now);
        self.conn.execute("UPDATE conversations SET archived_at = ?2 WHERE id = ?1", params![id, at])?;
        Ok(())
    }

    /// Archives chats nobody has used since `cutoff`, and returns them. Each bot keeps its
    /// newest chat and the one it opens to, and `busy` chats (working right now) are left.
    pub fn archive_idle(&self, cutoff: chrono::DateTime<chrono::Utc>, busy: &dyn Fn(&str) -> bool) -> Result<Vec<Conversation>, BoxError> {
        let mut archived = Vec::new();
        for bot in self.bots()? {
            let open = self.current_conversation(&bot.id)?.map(|c| c.id);
            // Newest first: the first one is always kept.
            for conversation in self.conversations(&bot.id)?.into_iter().skip(1) {
                let idle_since = chrono::DateTime::parse_from_rfc3339(&conversation.updated_at).map(|t| t.with_timezone(&chrono::Utc));
                let stale = idle_since.is_ok_and(|at| at < cutoff);
                if conversation.archived_at.is_none() && stale && open.as_deref() != Some(&conversation.id) && !busy(&conversation.id) {
                    self.set_archived(&conversation.id, true)?;
                    archived.push(conversation);
                }
            }
        }
        Ok(archived)
    }

    /// Deletes a chat and its messages. If its bot was opening to it, the bot opens to its
    /// latest chat instead.
    pub fn delete_conversation(&self, id: &str) -> Result<(), BoxError> {
        self.conn.execute("UPDATE bots SET open_conversation = NULL WHERE open_conversation = ?1", [id])?;
        self.conn.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The bot's current conversation, started if it has none.
    pub fn ensure_conversation(&self, bot_id: &str) -> Result<Conversation, BoxError> {
        match self.current_conversation(bot_id)? {
            Some(conversation) => Ok(conversation),
            None => self.start_conversation(bot_id, None),
        }
    }

    pub fn start_conversation(&self, bot_id: &str, session_id: Option<&str>) -> Result<Conversation, BoxError> {
        let at = now();
        let conversation = Conversation {
            id: uuid::Uuid::new_v4().to_string(),
            bot_id: bot_id.to_string(),
            session_id: session_id.map(str::to_string),
            created_at: at.clone(),
            updated_at: at,
            title: None,
            title_by_person: false,
            archived_at: None,
            channel_id: None,
            unread: false,
        };
        self.conn.execute(
            "INSERT INTO conversations (id, bot_id, session_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![conversation.id, conversation.bot_id, conversation.session_id, conversation.created_at, conversation.updated_at],
        )?;
        Ok(conversation)
    }

    /// Forgets a conversation's AI session: its next turn starts a new one, shown the chat so far.
    pub fn clear_session(&self, conversation_id: &str) -> Result<(), BoxError> {
        self.conn.execute("UPDATE conversations SET session_id = NULL WHERE id = ?1", [conversation_id])?;
        Ok(())
    }

    pub fn set_session(&self, conversation_id: &str, session_id: &str) -> Result<(), BoxError> {
        self.conn.execute("UPDATE conversations SET session_id = ?2 WHERE id = ?1", params![conversation_id, session_id])?;
        Ok(())
    }

    pub fn add_message(&self, conversation_id: &str, role: Role, body: &str, detail: Option<&Value>) -> Result<i64, BoxError> {
        let at = now();
        let detail = detail.map(Value::to_string);
        self.conn.execute(
            "INSERT INTO messages (conversation_id, role, body, detail, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![conversation_id, role.as_str(), body, detail, at],
        )?;
        let id = self.conn.last_insert_rowid();
        // A new message brings an archived chat back into the list.
        self.conn.execute(
            "UPDATE conversations SET updated_at = ?2, archived_at = NULL WHERE id = ?1",
            params![conversation_id, at],
        )?;
        Ok(id)
    }

    /// The last `limit` messages of a conversation, oldest first.
    /// Steps (tools, errors) and notes saved after message `after`, in every chat, oldest first,
    /// with the chat each is in: what the Chief of Staff's watch looks over.
    pub fn steps_since(&self, after: i64, limit: usize) -> Result<Vec<(String, Message)>, BoxError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, body, detail, at, conversation_id FROM messages WHERE id > ?1 AND role IN ('step', 'note') ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![after, limit as i64], |r| Ok((r.get::<_, String>(5)?, message_from_row(r)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// The newest message's id, in any chat (0 with none).
    pub fn last_message_id(&self) -> Result<i64, BoxError> {
        Ok(self.conn.query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |r| r.get(0))?)
    }

    pub fn messages(&self, conversation_id: &str, limit: usize) -> Result<Vec<Message>, BoxError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, body, detail, at FROM
               (SELECT id, role, body, detail, at FROM messages WHERE conversation_id = ?1 ORDER BY id DESC LIMIT ?2)
             ORDER BY id",
        )?;
        let messages = stmt.query_map(params![conversation_id, limit as i64], message_from_row)?.collect::<Result<_, _>>()?;
        Ok(messages)
    }

    /// Up to `limit` messages older than message `before` (all the latest when None), oldest
    /// first, and whether there are older ones still.
    pub fn page(&self, conversation_id: &str, before: Option<i64>, limit: usize) -> Result<(Vec<Message>, bool), BoxError> {
        let before = before.unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT id, role, body, detail, at FROM messages WHERE conversation_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
        )?;
        let mut messages: Vec<Message> = stmt
            .query_map(params![conversation_id, before, limit as i64 + 1], message_from_row)?
            .collect::<Result<_, _>>()?;
        let more = messages.len() > limit;
        messages.truncate(limit);
        messages.reverse();
        Ok((messages, more))
    }

    /// The message just before `id` in its conversation.
    pub fn previous_message_id(&self, conversation_id: &str, id: i64) -> Result<Option<i64>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT MAX(id) FROM messages WHERE conversation_id = ?1 AND id < ?2",
                params![conversation_id, id],
                |r| r.get::<_, Option<i64>>(0),
            )?)
    }

    /// Replaces a message's detail (a card being answered).
    pub fn update_detail(&self, id: i64, detail: &Value) -> Result<(), BoxError> {
        self.conn.execute("UPDATE messages SET detail = ?2 WHERE id = ?1", params![id, detail.to_string()])?;
        Ok(())
    }

    /// A message, if it's in this conversation.
    pub fn message_in(&self, conversation_id: &str, id: i64) -> Result<Option<Message>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, role, body, detail, at FROM messages WHERE id = ?1 AND conversation_id = ?2",
                params![id, conversation_id],
                message_from_row,
            )
            .optional()?)
    }

    pub fn message(&self, id: i64) -> Result<Option<Message>, BoxError> {
        Ok(self
            .conn
            .query_row("SELECT id, role, body, detail, at FROM messages WHERE id = ?1", [id], message_from_row)
            .optional()?)
    }

    /// Marks a chat read, or answered since the person last looked. True if that changed it.
    pub fn set_unread(&self, conversation_id: &str, unread: bool) -> Result<bool, BoxError> {
        Ok(self.conn.execute("UPDATE conversations SET unread = ?2 WHERE id = ?1 AND unread != ?2", params![conversation_id, unread])? > 0)
    }

    /// Every chat a bot answered in since the person last looked, newest first.
    pub fn unread_conversations(&self) -> Result<Vec<Conversation>, BoxError> {
        let mut stmt = self.conn.prepare(&format!("SELECT {CONVERSATION} FROM conversations WHERE unread = 1 AND archived_at IS NULL ORDER BY updated_at DESC"))?;
        let rows = stmt.query_map([], conversation_from_row)?.collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Cards still waiting on the person in any chat (a change to approve, a picture to choose),
    /// newest first, each with its chat.
    pub fn asking_cards(&self, limit: usize) -> Result<Vec<(String, Message)>, BoxError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, body, detail, at, conversation_id FROM messages
             WHERE detail IS NOT NULL AND json_valid(detail) AND json_extract(detail, '$.state') IN ('asking', 'choosing')
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| Ok((r.get::<_, String>(5)?, message_from_row(r)?)))?.collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// How many conversations a bot has had.
    pub fn conversation_count(&self, bot_id: &str) -> Result<usize, BoxError> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM conversations WHERE bot_id = ?1 AND channel_id IS NULL", [bot_id], |r| r.get(0))?;
        Ok(n as usize)
    }
}

fn bot_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Bot> {
    let mode: String = r.get(4)?;
    Ok(Bot {
        id: r.get(0)?,
        name: r.get(1)?,
        instructions: r.get(2)?,
        model: r.get(3)?,
        mode: parse_mode(&mode),
        codex: r.get(5)?,
        folder: PathBuf::from(r.get::<_, String>(6)?),
        created_at: r.get(7)?,
        mark: r.get::<_, Option<String>>(8)?.as_deref().and_then(Mark::parse).unwrap_or_default(),
        color: r.get::<_, Option<String>>(9)?.filter(|c| COLORS.contains(&c.as_str())),
        engine: r.get(10)?,
        effort: r.get::<_, Option<String>>(11)?.filter(|e| !e.is_empty()),
    })
}

fn message_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let detail: Option<String> = r.get(3)?;
    Ok(Message {
        id: r.get(0)?,
        role: Role::parse(&r.get::<_, String>(1)?),
        body: r.get(2)?,
        detail: detail.and_then(|d| serde_json::from_str(&d).ok()),
        at: r.get(4)?,
    })
}

fn device_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, last_seen_at: r.get(3)? })
}

fn conversation_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: r.get(0)?,
        bot_id: r.get(1)?,
        session_id: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
        title: r.get(5)?,
        title_by_person: r.get(6)?,
        archived_at: r.get(7)?,
        channel_id: r.get(8)?,
        unread: r.get(9)?,
    })
}

/// A stored mode name back to a mode; anything unknown becomes the safest one.
pub fn parse_mode(s: &str) -> PermissionMode {
    match s {
        "acceptEdits" => PermissionMode::AcceptEdits,
        "auto" => PermissionMode::Auto,
        "plan" => PermissionMode::Plan,
        "bypassPermissions" => PermissionMode::BypassPermissions,
        _ => PermissionMode::Default,
    }
}

fn check_name(name: &str) -> Result<(), BoxError> {
    if name.is_empty() {
        return Err("a bot needs a name".into());
    }
    if name.chars().count() > 40 {
        return Err("a bot's name can be at most 40 characters".into());
    }
    if name.chars().any(char::is_control) || name.starts_with('-') {
        return Err(format!("{name:?} can't be a bot's name").into());
    }
    Ok(())
}

/// A folder name for a bot: lowercase letters, digits and dashes.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() { "bot".into() } else { slug }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("catbot")).unwrap();
        (dir, store)
    }

    fn new(name: &str) -> NewBot {
        NewBot { name: name.into(), instructions: "Be brief.".into(), model: None, mode: PermissionMode::Default, codex: true }
    }

    #[test]
    fn store_version_is_the_last_step() {
        let (_dir, store) = store();
        let written: i64 = store.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(written, STORE_VERSION, "raise STORE_VERSION with each new step in open");
    }

    #[test]
    fn bots_are_made_found_and_kept() {
        let (_dir, store) = store();
        let bot = store.create_bot(NewBot { model: Some("opus".into()), mode: PermissionMode::AcceptEdits, ..new("Tree Helper") }).unwrap();
        assert!(bot.folder.ends_with("bots/tree-helper"));
        assert!(bot.folder.is_dir());
        let found = store.bot("tree helper").unwrap().expect("found ignoring case");
        assert_eq!(found, bot);
        assert_eq!(found.mode, PermissionMode::AcceptEdits);
        assert_eq!(store.bots().unwrap().len(), 1);
    }

    #[test]
    fn names_are_unique_and_checked() {
        let (_dir, store) = store();
        store.create_bot(new("Scout")).unwrap();
        assert!(store.create_bot(new("SCOUT")).unwrap_err().to_string().contains("already exists"));
        assert!(store.create_bot(new("  ")).is_err());
        assert!(store.create_bot(new("--help")).is_err());
        assert!(store.create_bot(new(&"x".repeat(41))).is_err());
    }

    #[test]
    fn folders_never_collide() {
        let (_dir, store) = store();
        let a = store.create_bot(new("Scout!")).unwrap();
        let b = store.create_bot(new("Scout?")).unwrap();
        assert_ne!(a.folder, b.folder);
        assert!(b.folder.ends_with("bots/scout-2"));
        assert_eq!(slug("Ünïcode only"), "n-code-only");
        assert_eq!(slug("!!!"), "bot");
    }

    #[test]
    fn updates_are_saved() {
        let (_dir, store) = store();
        let mut bot = store.create_bot(new("Scout")).unwrap();
        bot.instructions = "Be thorough.".into();
        bot.model = Some("haiku".into());
        bot.codex = false;
        store.update_bot(&bot).unwrap();
        assert_eq!(store.bot("Scout").unwrap().unwrap(), bot);
    }

    #[test]
    fn conversations_and_messages() {
        let (_dir, store) = store();
        let bot = store.create_bot(new("Scout")).unwrap();
        assert_eq!(store.current_conversation(&bot.id).unwrap(), None);
        let first = store.start_conversation(&bot.id, Some("s1")).unwrap();
        for i in 0..5 {
            store.add_message(&first.id, Role::Person, &format!("m{i}"), None).unwrap();
        }
        store.add_message(&first.id, Role::Step, "Bash", Some(&serde_json::json!({"ok": true}))).unwrap();
        let last3 = store.messages(&first.id, 3).unwrap();
        assert_eq!(last3.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["m3", "m4", "Bash"]);
        assert_eq!(last3[2].role, Role::Step);
        assert_eq!(last3[2].detail.as_ref().unwrap()["ok"], true);

        let second = store.start_conversation(&bot.id, None).unwrap();
        assert_eq!(store.current_conversation(&bot.id).unwrap().unwrap().id, second.id);
        store.set_session(&second.id, "s2").unwrap();
        assert_eq!(store.current_conversation(&bot.id).unwrap().unwrap().session_id.as_deref(), Some("s2"));
        assert_eq!(store.conversation_count(&bot.id).unwrap(), 2);
    }

    #[test]
    fn renaming_and_paging() {
        let (_dir, store) = store();
        let mut bot = store.create_bot(new("Scout")).unwrap();
        store.create_bot(new("Other")).unwrap();
        bot.name = "other".into();
        assert!(store.update_bot(&bot).is_err(), "names stay unique");
        bot.name = "Ranger".into();
        store.update_bot(&bot).unwrap();
        assert_eq!(store.bot_by_id(&bot.id).unwrap().unwrap().name, "Ranger");
        assert_eq!(store.free_name("Ranger").unwrap(), "Ranger 2");

        let conv = store.ensure_conversation(&bot.id).unwrap();
        assert_eq!(store.ensure_conversation(&bot.id).unwrap().id, conv.id);
        let ids: Vec<i64> = (0..5).map(|i| store.add_message(&conv.id, Role::Person, &format!("m{i}"), None).unwrap()).collect();
        let (latest, more) = store.page(&conv.id, None, 2).unwrap();
        assert_eq!(latest.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["m3", "m4"]);
        assert!(more);
        let (older, more) = store.page(&conv.id, Some(ids[3]), 10).unwrap();
        assert_eq!(older.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["m0", "m1", "m2"]);
        assert!(!more);
        assert_eq!(store.previous_message_id(&conv.id, ids[2]).unwrap(), Some(ids[1]));
        assert_eq!(store.previous_message_id(&conv.id, ids[0]).unwrap(), None);
        assert_eq!(store.message(ids[4]).unwrap().unwrap().body, "m4");
    }

    #[test]
    fn deleting_a_bot_removes_its_history_but_not_its_folder() {
        let (_dir, store) = store();
        let bot = store.create_bot(new("Scout")).unwrap();
        let conv = store.start_conversation(&bot.id, Some("s1")).unwrap();
        store.add_message(&conv.id, Role::Person, "hi", None).unwrap();
        store.delete_bot("scout").unwrap();
        assert!(store.bot("Scout").unwrap().is_none());
        assert!(store.messages(&conv.id, 10).unwrap().is_empty());
        assert!(bot.folder.is_dir());
        assert!(store.delete_bot("Scout").is_err());
    }

    #[test]
    fn a_bots_picture_and_colour() {
        let (_dir, store) = store();
        let mut bot = store.create_bot(new("Kiwi")).unwrap();
        assert_eq!((bot.mark, bot.color.as_deref()), (Mark::CatHead, None));
        bot.mark = Mark::CatVisor;
        bot.color = Some("teal".into());
        store.update_bot(&bot).unwrap();
        let again = store.bot("Kiwi").unwrap().unwrap();
        assert_eq!((again.mark, again.color.as_deref()), (Mark::CatVisor, Some("teal")));
        assert_eq!(Mark::parse("cat-sitting"), Some(Mark::CatSitting));
        assert_eq!(Mark::parse("robot-full"), Some(Mark::RobotFull));
        assert_eq!(Mark::parse("mouse"), None);
    }

    #[test]
    fn a_version_1_store_gains_pictures_and_chats() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("catbot");
        std::fs::create_dir_all(&home).unwrap();
        {
            let conn = Connection::open(home.join("catbot.db")).unwrap();
            conn.execute_batch(&format!("{SCHEMA} PRAGMA user_version = 1;")).unwrap();
            conn.execute(
                "INSERT INTO bots (id, name, folder, created_at) VALUES ('b1', 'Old', '/tmp/old', '2026-11-14T00:00:00Z')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&home).unwrap();
        let bot = store.bot("Old").unwrap().unwrap();
        assert_eq!((bot.mark, bot.color), (Mark::CatHead, None));
        assert_eq!(bot.engine, "claude", "bots from before run on Claude");
        let version: i64 = store.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, 12);
        assert_eq!(bot.effort, None, "and think at their engine's own level");
        // And keys for the chat APIs.
        assert_eq!(store.engine_key("xai").unwrap(), None);
        store.set_engine_key("xai", "xai-1", None).unwrap();
        store.set_engine_key("xai", "xai-2", Some("https://example.com/v1")).unwrap();
        let saved = store.engine_key("xai").unwrap().unwrap();
        assert_eq!((saved.key.as_str(), saved.url.as_deref()), ("xai-2", Some("https://example.com/v1")));
        assert!(store.remove_engine_key("xai").unwrap());
        assert!(!store.remove_engine_key("xai").unwrap());
        // And chats: an old conversation is kept, unnamed, and opened to.
        let conv = store.start_conversation("b1", Some("s1")).unwrap();
        assert_eq!(store.current_conversation("b1").unwrap().unwrap().title, None);
        assert_eq!(store.current_conversation("b1").unwrap().unwrap().id, conv.id);
    }

    #[test]
    fn idle_chats_are_archived_but_not_the_newest_open_or_busy() {
        let (_dir, store) = store();
        let bot = store.create_bot(new("Kiwi")).unwrap();
        let chats: Vec<Conversation> = (0..5).map(|_| store.new_conversation(&bot.id).unwrap()).collect();
        let age = |c: &Conversation, days: i64| {
            let at = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
            store.conn.execute("UPDATE conversations SET updated_at = ?2 WHERE id = ?1", params![c.id, at]).unwrap();
        };
        // 0: recent. 1: old but open. 2: old and working. 3, 4: old and idle (4 is older).
        age(&chats[0], 2);
        age(&chats[1], 40);
        age(&chats[2], 40);
        age(&chats[3], 31);
        age(&chats[4], 90);
        store.open_conversation(&chats[1]).unwrap();
        let working = chats[2].id.clone();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
        let archived = store.archive_idle(cutoff, &|id| id == working).unwrap();
        let mut ids: Vec<&str> = archived.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        let mut want = vec![chats[3].id.as_str(), chats[4].id.as_str()];
        want.sort();
        assert_eq!(ids, want);
        assert!(store.archive_idle(cutoff, &|_| false).unwrap().iter().all(|c| c.id == chats[2].id), "only the chat that was busy is left to archive");

        // A bot whose every chat is old keeps its newest.
        let other = store.create_bot(new("Fern")).unwrap();
        let only = store.ensure_conversation(&other.id).unwrap();
        age(&only, 100);
        assert!(store.archive_idle(cutoff, &|_| false).unwrap().is_empty());
    }

    #[test]
    fn devices_pair_and_unpair() {
        let (_dir, store) = store();
        let phone = store.add_device("Moo's phone", "hash-1").unwrap();
        assert_eq!(store.device_by_key_hash("hash-1").unwrap().unwrap().id, phone.id);
        assert!(store.device_by_key_hash("hash-2").unwrap().is_none());
        assert!(store.add_device("Copy", "hash-1").is_err(), "two devices can't share a key");
        store.device_seen(&phone.id).unwrap();
        assert!(store.devices().unwrap()[0].last_seen_at.is_some());
        assert!(store.remove_device(&phone.id).unwrap());
        assert!(!store.remove_device(&phone.id).unwrap());
        assert!(store.device_by_key_hash("hash-1").unwrap().is_none(), "a removed device's key stops working");
    }

    #[test]
    fn several_chats_per_bot() {
        let (_dir, store) = store();
        let bot = store.create_bot(new("Kiwi")).unwrap();
        let first = store.ensure_conversation(&bot.id).unwrap();
        store.add_message(&first.id, Role::Person, "plan the garden", None).unwrap();
        store.add_message(&first.id, Role::Person, "and the shed", None).unwrap();
        assert_eq!(store.first_words(&first.id).unwrap().as_deref(), Some("plan the garden"));

        // A new chat is opened to; the first stays where it was.
        let second = store.new_conversation(&bot.id).unwrap();
        assert_eq!(store.current_conversation(&bot.id).unwrap().unwrap().id, second.id);
        store.open_conversation(&first).unwrap();
        assert_eq!(store.current_conversation(&bot.id).unwrap().unwrap().id, first.id);
        assert_eq!(store.conversations(&bot.id).unwrap().len(), 2);

        // Names: automatic, then the person's.
        store.set_title(&second.id, "Shed plans", false).unwrap();
        store.set_title(&second.id, "My shed", true).unwrap();
        let named = store.conversation(&second.id).unwrap().unwrap();
        assert_eq!((named.title.as_deref(), named.title_by_person), (Some("My shed"), true));

        // Archived, then back when a message arrives.
        store.set_archived(&second.id, true).unwrap();
        assert!(store.conversation(&second.id).unwrap().unwrap().archived_at.is_some());
        store.add_message(&second.id, Role::Person, "back again", None).unwrap();
        assert!(store.conversation(&second.id).unwrap().unwrap().archived_at.is_none());

        // Deleting the open chat: the bot opens to what's left.
        store.delete_conversation(&first.id).unwrap();
        assert!(store.conversation(&first.id).unwrap().is_none());
        assert!(store.messages(&first.id, 10).unwrap().is_empty());
        assert_eq!(store.current_conversation(&bot.id).unwrap().unwrap().id, second.id);

        // An archived chat is opened to only when nothing else is left.
        let third = store.new_conversation(&bot.id).unwrap();
        store.set_archived(&second.id, true).unwrap();
        store.delete_conversation(&third.id).unwrap();
        assert_eq!(store.current_conversation(&bot.id).unwrap().unwrap().id, second.id);
    }

    #[cfg(unix)]
    #[test]
    fn the_store_survives_reopening_and_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("catbot");
        Store::open(&home).unwrap().create_bot(new("Scout")).unwrap();
        let store = Store::open(&home).unwrap();
        assert_eq!(store.bots().unwrap().len(), 1);
        assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
    }
}
