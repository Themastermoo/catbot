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

/// Where Catbot keeps its data: `$CATBOT_HOME`, else `$XDG_DATA_HOME/catbot`, else
/// `~/.local/share/catbot`.
pub fn default_home() -> PathBuf {
    if let Some(home) = std::env::var_os("CATBOT_HOME") {
        return PathBuf::from(home);
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(data).join("catbot");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join("catbot")
}

/// The Codex vault: `$CATBOT_CODEX`, else `~/VaultCodex`.
pub fn default_codex() -> PathBuf {
    if let Some(codex) = std::env::var_os("CATBOT_CODEX") {
        return PathBuf::from(codex);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("VaultCodex")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub instructions: String,
    /// None: Claude Code's default model.
    pub model: Option<String>,
    #[serde(serialize_with = "mode_str")]
    pub mode: PermissionMode,
    /// Whether the bot may use the Codex vault.
    pub codex: bool,
    /// The bot's own working folder.
    pub folder: PathBuf,
    pub created_at: String,
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
}

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

fn now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

impl Store {
    /// Opens (or creates) the store in `home`, a folder only this user can open.
    pub fn open(home: &Path) -> Result<Self, BoxError> {
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
        let conn = Connection::open(home.join("catbot.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(&format!("BEGIN; {SCHEMA} PRAGMA user_version = 1; COMMIT;"))?;
        }
        Ok(Self { conn, home: home.to_path_buf() })
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
            model: new.model.filter(|m| !m.trim().is_empty()),
            mode: new.mode,
            codex: new.codex,
            folder,
            created_at: now(),
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

    pub fn bots(&self) -> Result<Vec<Bot>, BoxError> {
        let mut stmt = self.conn.prepare("SELECT id, name, instructions, model, mode, codex, folder, created_at FROM bots ORDER BY name")?;
        let bots = stmt.query_map([], bot_from_row)?.collect::<Result<_, _>>()?;
        Ok(bots)
    }

    /// A bot by name, ignoring case.
    pub fn bot(&self, name: &str) -> Result<Option<Bot>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, instructions, model, mode, codex, folder, created_at FROM bots WHERE name = ?1",
                [name.trim()],
                bot_from_row,
            )
            .optional()?)
    }

    pub fn bot_by_id(&self, id: &str) -> Result<Option<Bot>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, instructions, model, mode, codex, folder, created_at FROM bots WHERE id = ?1",
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
            "UPDATE bots SET name = ?2, instructions = ?3, model = ?4, mode = ?5, codex = ?6 WHERE id = ?1",
            params![bot.id, name, bot.instructions, bot.model, bot.mode.as_str(), bot.codex],
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
        self.conn.execute("DELETE FROM bots WHERE id = ?1", [&bot.id])?;
        Ok(bot)
    }

    /// The bot's most recent conversation.
    pub fn current_conversation(&self, bot_id: &str) -> Result<Option<Conversation>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, bot_id, session_id, created_at, updated_at FROM conversations
                 WHERE bot_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT 1",
                [bot_id],
                conversation_from_row,
            )
            .optional()?)
    }

    pub fn conversation(&self, id: &str) -> Result<Option<Conversation>, BoxError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, bot_id, session_id, created_at, updated_at FROM conversations WHERE id = ?1",
                [id],
                conversation_from_row,
            )
            .optional()?)
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
        };
        self.conn.execute(
            "INSERT INTO conversations (id, bot_id, session_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![conversation.id, conversation.bot_id, conversation.session_id, conversation.created_at, conversation.updated_at],
        )?;
        Ok(conversation)
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
        self.conn.execute("UPDATE conversations SET updated_at = ?2 WHERE id = ?1", params![conversation_id, at])?;
        Ok(id)
    }

    /// The last `limit` messages of a conversation, oldest first.
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

    pub fn message(&self, id: i64) -> Result<Option<Message>, BoxError> {
        Ok(self
            .conn
            .query_row("SELECT id, role, body, detail, at FROM messages WHERE id = ?1", [id], message_from_row)
            .optional()?)
    }

    /// How many conversations a bot has had.
    pub fn conversation_count(&self, bot_id: &str) -> Result<usize, BoxError> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM conversations WHERE bot_id = ?1", [bot_id], |r| r.get(0))?;
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

fn conversation_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation { id: r.get(0)?, bot_id: r.get(1)?, session_id: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)? })
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
