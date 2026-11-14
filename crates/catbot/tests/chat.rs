//! `catbot chat` against the stand-in CLI: what reaches Claude, and what gets saved.

use std::path::Path;

use catbot::{
    chat::{self, Options},
    claude::PermissionMode,
    store::{NewBot, Role, Store},
};
use serde_json::Value;

const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.py");

/// The stand-in logs to a path named in an environment variable, which every test in this
/// process shares, so these tests take turns.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Setup {
    dir: tempfile::TempDir,
    store: Store,
    _turn: std::sync::MutexGuard<'static, ()>,
}

impl Setup {
    fn new() -> Self {
        let turn = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("home")).unwrap();
        std::fs::create_dir_all(dir.path().join("Codex")).unwrap();
        store
            .create_bot(NewBot { name: "Scout".into(), instructions: "Find things.".into(), model: Some("haiku".into()), mode: PermissionMode::AcceptEdits, codex: true })
            .unwrap();
        Setup { dir, store, _turn: turn }
    }

    fn log(&self) -> std::path::PathBuf {
        self.dir.path().join("cli.log")
    }

    async fn say(&self, message: &str, new: bool) -> bool {
        // SAFETY: tests in this file hold ONE_AT_A_TIME, so no other thread touches the environment.
        unsafe { std::env::set_var("FAKE_CLAUDE_LOG", self.log()) };
        let options = Options {
            bot: "scout".into(),
            message: message.into(),
            new,
            json: false,
            no_approvals: true,
            cli: vec!["python3".into(), FAKE.into()],
            codex: self.dir.path().join("Codex"),
        };
        chat::chat(&self.store, options).await.unwrap()
    }

    /// Every start of the CLI so far: its arguments and working folder.
    fn starts(&self) -> Vec<Value> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

fn arg_after<'a>(start: &'a Value, flag: &str) -> Option<&'a str> {
    let args = start["args"].as_array()?;
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1)?.as_str()
}

#[tokio::test(flavor = "current_thread")]
async fn a_chat_reaches_claude_as_the_bot_and_is_saved() {
    let s = Setup::new();
    assert!(s.say("Moo", false).await);

    let bot = s.store.bot("Scout").unwrap().unwrap();
    let start = &s.starts()[0];
    assert_eq!(Path::new(start["cwd"].as_str().unwrap()), bot.folder.canonicalize().unwrap());
    assert_eq!(arg_after(start, "--model"), Some("haiku"));
    assert_eq!(arg_after(start, "--permission-mode"), Some("acceptEdits"));
    assert_eq!(arg_after(start, "--add-dir"), Some(s.dir.path().join("Codex").to_str().unwrap()));
    let system = arg_after(start, "--append-system-prompt").unwrap();
    assert!(system.starts_with("You are Scout") && system.contains("Find things.") && system.contains("Codex"));

    let conversation = s.store.current_conversation(&bot.id).unwrap().unwrap();
    assert_eq!(conversation.session_id.as_deref(), arg_after(start, "--session-id"));
    let messages = s.store.messages(&conversation.id, 10).unwrap();
    let said: Vec<(Role, &str)> = messages.iter().map(|m| (m.role, m.body.as_str())).collect();
    assert_eq!(said, [(Role::Person, "Moo"), (Role::Bot, "Hello, Moo")]);
}

#[tokio::test(flavor = "current_thread")]
async fn the_next_message_continues_the_same_conversation() {
    let s = Setup::new();
    s.say("one", false).await;
    s.say("two", false).await;
    let starts = s.starts();
    let first = arg_after(&starts[0], "--session-id").unwrap();
    assert_eq!(arg_after(&starts[1], "--resume"), Some(first));

    let bot = s.store.bot("Scout").unwrap().unwrap();
    assert_eq!(s.store.conversation_count(&bot.id).unwrap(), 1);
    let conversation = s.store.current_conversation(&bot.id).unwrap().unwrap();
    assert_eq!(s.store.messages(&conversation.id, 10).unwrap().len(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn new_starts_a_fresh_conversation() {
    let s = Setup::new();
    s.say("one", false).await;
    s.say("two", true).await;
    let starts = s.starts();
    assert!(arg_after(&starts[1], "--session-id").is_some());
    assert_ne!(arg_after(&starts[1], "--session-id"), arg_after(&starts[0], "--session-id"));
    let bot = s.store.bot("Scout").unwrap().unwrap();
    assert_eq!(s.store.conversation_count(&bot.id).unwrap(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_conversation_claude_lost_carries_on_in_a_new_session() {
    let s = Setup::new();
    s.say("one", false).await;
    let bot = s.store.bot("Scout").unwrap().unwrap();
    let conversation = s.store.current_conversation(&bot.id).unwrap().unwrap();
    s.store.set_session(&conversation.id, "gone-123").unwrap();

    assert!(s.say("still there?", false).await);
    let starts = s.starts();
    assert_eq!(arg_after(&starts[1], "--resume"), Some("gone-123"));
    let fresh = arg_after(&starts[2], "--session-id").unwrap().to_string();
    let after = s.store.current_conversation(&bot.id).unwrap().unwrap();
    assert_eq!(after.id, conversation.id, "same conversation in Catbot");
    assert_eq!(after.session_id.as_deref(), Some(fresh.as_str()));
    let roles: Vec<Role> = s.store.messages(&conversation.id, 10).unwrap().iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::Person, Role::Bot, Role::Person, Role::Note, Role::Bot]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_kept_out_of_codex_never_sees_it() {
    let s = Setup::new();
    let mut bot = s.store.bot("Scout").unwrap().unwrap();
    bot.codex = false;
    s.store.update_bot(&bot).unwrap();
    s.say("hi", false).await;
    let start = &s.starts()[0];
    assert_eq!(arg_after(start, "--add-dir"), None);
    assert!(!arg_after(start, "--append-system-prompt").unwrap().contains("Codex"));
}

#[tokio::test(flavor = "current_thread")]
async fn tool_steps_are_saved_with_the_reply() {
    let s = Setup::new();
    s.say("tool", false).await;
    let bot = s.store.bot("Scout").unwrap().unwrap();
    let conversation = s.store.current_conversation(&bot.id).unwrap().unwrap();
    let messages = s.store.messages(&conversation.id, 10).unwrap();
    let step = messages.iter().find(|m| m.role == Role::Step).expect("a step");
    assert_eq!(step.body, "Bash rm -rf /tmp/x");
    assert_eq!(step.detail.as_ref().unwrap()["ok"], false);
}
