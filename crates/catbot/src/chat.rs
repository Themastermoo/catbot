//! `catbot chat`: talk to a bot. Its instructions, model, folder and Codex access come from
//! the store, the conversation continues where it left off, and everything said is saved.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::{
    ask,
    claude::{self, Session, Turn},
    console::{self, Paint, Step, Transcript},
    store::{Bot, Role, Store},
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct Options {
    pub bot: String,
    pub message: String,
    /// Start a fresh conversation instead of continuing the last one.
    pub new: bool,
    pub json: bool,
    pub no_approvals: bool,
    /// The Claude Code program (and leading arguments).
    pub cli: Vec<String>,
    /// The Codex vault, for bots allowed to use it.
    pub codex: PathBuf,
    /// The memory vault (`memory.rs`), if it's on.
    pub memory: Option<PathBuf>,
}

/// The bot's standing instructions: who it is, where it works, and how to use Codex and the memory vault.
pub fn system_prompt(bot: &Bot, codex: Option<&Path>, memory: Option<&Path>) -> String {
    let mut prompt = format!("You are {}, one of the person's bots in Catbot, a chat app where they talk to a team of AI bots.", bot.name);
    if !bot.instructions.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&bot.instructions);
    }
    prompt.push_str(&format!(
        "\n\nYour own working folder is {}. Keep your files there; other bots have their own folders.",
        bot.folder.display()
    ));
    if let Some(codex) = codex {
        prompt.push_str(&format!(
            "\n\nCodex, the shared memory of all the person's bots, is an Obsidian vault of Markdown notes at {path}. \
             Look there before answering questions about the person, their projects or earlier decisions (start from \
             any index, home or README note). When you learn something durable that future bots should know, add or \
             update a note there: follow the vault's existing structure and naming, and link related notes with \
             [[wikilinks]]. Say briefly what you're saving there and why. \
             Never put passwords, keys or other secrets in it. If the vault has an AGENTS.md, read it first and \
             follow it.",
            path = codex.display()
        ));
    }
    if let Some(memory) = memory {
        prompt.push_str(&crate::memory::prompt(memory, bot, codex.is_some() && crate::memory::has_codex(memory)));
    }
    prompt.push_str(crate::server::images::BOT_NOTE);
    prompt.push_str(crate::server::skills::BOT_NOTE);
    prompt
}

/// The turn that runs a bot: its model, approval level, folder, instructions, Codex and memory vault.
pub fn bot_turn(
    bot: &Bot,
    prompt: String,
    session: Session,
    cli: Vec<String>,
    approval_helper: Option<Vec<String>>,
    codex: Option<PathBuf>,
    memory: Option<PathBuf>,
) -> Turn {
    Turn {
        prompt,
        session,
        model: bot.model.clone(),
        permission_mode: bot.mode,
        cwd: bot.folder.clone(),
        system_prompt: Some(system_prompt(bot, codex.as_deref(), memory.as_deref())),
        cli,
        isolated: true,
        approval_helper,
        extra_dirs: memory.into_iter().chain(codex).collect(),
        catbot_tools: None,
        effort: bot.effort.clone().filter(|e| crate::engines::find(&bot.engine).and_then(|engine| crate::engines::effort_levels(engine, "")).is_some_and(|l| l.contains(&e.as_str()))),
    }
}

/// The vault, if this bot may use it and it exists.
pub fn codex_dir(bot: &Bot, codex: &Path) -> Option<PathBuf> {
    (bot.codex && codex.is_dir()).then(|| codex.to_path_buf())
}

/// The CLI's answer when asked to resume a conversation it doesn't have.
pub fn says_lost(text: &str) -> bool {
    text.contains("No conversation found")
}

/// Sends one message to a bot and saves the exchange. Returns whether the turn went well.
pub async fn chat(store: &Store, options: Options) -> Result<bool, BoxError> {
    let paint = Paint::stderr();
    let bot = find(store, &options.bot)?;
    if bot.engine != crate::engines::CLAUDE {
        let name = crate::engines::find(&bot.engine).map(|e| e.name).unwrap_or(bot.engine.as_str());
        return Err(format!("{} runs on {name}; talk to it in Catbot's window (catbot chat drives Claude bots only, for now)", bot.name).into());
    }
    if !bot.folder.is_dir() {
        std::fs::create_dir_all(&bot.folder)?;
    }
    let codex = codex_for(&bot, &options.codex, &paint);
    let memory = memory_for(&bot, options.memory.as_deref(), &options.codex, &paint);
    let approvals_on = !options.no_approvals && console::can_ask(options.json);

    let current = if options.new { None } else { store.current_conversation(&bot.id)? };
    let (conversation, mut session) = match current {
        Some(conversation) => {
            let session = match &conversation.session_id {
                Some(id) => Session::Resume(id.clone()),
                None => Session::New(uuid::Uuid::new_v4().to_string()),
            };
            (conversation, session)
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let conversation = store.start_conversation(&bot.id, Some(&id))?;
            store.open_conversation(&conversation)?;
            (conversation, Session::New(id))
        }
    };
    store.add_message(&conversation.id, Role::Person, &options.message, None)?;
    if conversation.title.is_none() {
        store.set_title(&conversation.id, &crate::titles::from_message(&options.message), false)?;
    }

    let helper = approvals_on.then(ask::helper).transpose()?;
    let mut turn = bot_turn(&bot, options.message.clone(), session.clone(), options.cli.clone(), helper, codex, memory);
    if let Session::New(id) = &session {
        store.set_session(&conversation.id, id)?;
    }

    let mut transcript = run(&mut turn, approvals_on, options.json).await?;
    if matches!(session, Session::Resume(_)) && lost_session(&transcript) {
        // Claude no longer has this conversation (it forgets very old ones). Carry on in a new one.
        let id = uuid::Uuid::new_v4().to_string();
        let note = "Claude no longer had this conversation, so it continued in a fresh one.";
        eprintln!("{}", paint.dim(&format!("  {note}")));
        store.add_message(&conversation.id, Role::Note, note, None)?;
        store.set_session(&conversation.id, &id)?;
        session = Session::New(id);
        turn.session = session.clone();
        transcript = run(&mut turn, approvals_on, options.json).await?;
    }

    save(store, &conversation.id, &transcript)?;
    if !options.json {
        eprintln!("{}", paint.dim(&format!("  saved · catbot history {}", quote(&bot.name))));
    }
    Ok(transcript.ok())
}

async fn run(turn: &mut Turn, approvals_on: bool, json: bool) -> Result<Transcript, BoxError> {
    let mut running = claude::start(turn).await?;
    console::drive(&mut running, approvals_on, json).await
}

/// The CLI's answer when asked to resume a conversation it doesn't have.
fn lost_session(transcript: &Transcript) -> bool {
    let says = says_lost;
    transcript.outcome.as_ref().is_some_and(|o| !o.ok && o.result.as_deref().is_some_and(says))
        || transcript.steps.iter().any(|s| matches!(s, Step::Error { message } if says(message)))
}

/// Keeps what happened: each step, then the reply (or a note if there was none).
fn save(store: &Store, conversation_id: &str, transcript: &Transcript) -> Result<(), BoxError> {
    for step in &transcript.steps {
        let body = match step {
            Step::Tool { name, detail, .. } => format!("{name} {detail}").trim().to_string(),
            Step::Approval { tool, detail, allowed, .. } => {
                format!("{} {tool} {detail}", if *allowed { "allowed" } else { "refused" }).trim().to_string()
            }
            Step::Error { message } => message.clone(),
        };
        store.add_message(conversation_id, Role::Step, &body, Some(&serde_json::to_value(step)?))?;
    }
    if !transcript.reply.is_empty() {
        let outcome = transcript.outcome.as_ref();
        let detail = json!({
            "model": transcript.model,
            "cost_usd": outcome.and_then(|o| o.cost_usd),
            "duration_ms": outcome.and_then(|o| o.duration_ms),
        });
        store.add_message(conversation_id, Role::Bot, &transcript.reply, Some(&detail))?;
    }
    if transcript.stopped {
        store.add_message(conversation_id, Role::Note, "Stopped by the person.", None)?;
    }
    Ok(())
}

/// The vault, if this bot may use it and it exists.
/// The memory vault, made ready with the bot's own note; None if it's off or can't be made.
pub fn memory_ready(bot: &Bot, memory: Option<&Path>, codex: &Path) -> Result<Option<PathBuf>, std::io::Error> {
    let Some(memory) = memory else { return Ok(None) };
    crate::memory::prepare(memory, Some(codex))?;
    crate::memory::ensure_bot_note(memory, bot)?;
    Ok(Some(memory.to_path_buf()))
}

fn memory_for(bot: &Bot, memory: Option<&Path>, codex: &Path, paint: &Paint) -> Option<PathBuf> {
    memory_ready(bot, memory, codex).unwrap_or_else(|err| {
        eprintln!("{}", paint.dim(&format!("  (the memory vault couldn't be made ready: {err}; this turn runs without it)")));
        None
    })
}

fn codex_for(bot: &Bot, codex: &Path, paint: &Paint) -> Option<PathBuf> {
    if !bot.codex {
        return None;
    }
    if codex.is_dir() {
        Some(codex.to_path_buf())
    } else {
        eprintln!("{}", paint.dim(&format!("  (Codex not found at {}; this turn runs without it)", codex.display())));
        None
    }
}

fn find(store: &Store, name: &str) -> Result<Bot, BoxError> {
    if let Some(bot) = store.bot(name)? {
        return Ok(bot);
    }
    let names: Vec<String> = store.bots()?.into_iter().map(|b| b.name).collect();
    Err(if names.is_empty() {
        format!("no bot named {name}. Make one with: catbot bot new {}", quote(name)).into()
    } else {
        format!("no bot named {name}. Your bots: {}", names.join(", ")).into()
    })
}

/// A name as it would be typed in a shell.
fn quote(name: &str) -> String {
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\\\""))
    }
}

/// `catbot history`: the bot's current conversation.
pub fn history(store: &Store, name: &str, limit: usize, json: bool) -> Result<(), BoxError> {
    let bot = find(store, name)?;
    let Some(conversation) = store.current_conversation(&bot.id)? else {
        eprintln!("{} has no conversations yet.", bot.name);
        return Ok(());
    };
    let messages = store.messages(&conversation.id, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&messages)?);
        return Ok(());
    }
    let paint = Paint::stderr();
    for message in messages {
        let time = message.at.get(11..16).unwrap_or("");
        match message.role {
            Role::Person => println!("\n{} {}\n{}", paint.bold("You"), paint.dim(time), message.body),
            Role::Bot => println!("\n{} {}\n{}", paint.bold(&bot.name), paint.dim(time), message.body),
            Role::Step => println!("{}", paint.dim(&format!("  · {}", message.body))),
            Role::Note => println!("{}", paint.dim(&format!("  ({})", message.body))),
            Role::Card => {
                let card = message.detail.as_ref().map(|d| d["card"].clone()).unwrap_or_default();
                let answer = card["answered"].as_str().unwrap_or("unanswered");
                println!("{}", paint.dim(&format!("  ? {} → {answer}", message.body)));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{claude::PermissionMode, store::NewBot};

    #[test]
    fn the_system_prompt_names_the_bot_its_folder_and_codex() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let bot = store
            .create_bot(NewBot { name: "Scout".into(), instructions: "Find things.".into(), model: None, mode: PermissionMode::Default, codex: true })
            .unwrap();
        let with = system_prompt(&bot, Some(Path::new("/home/moo/VaultCodex")), None);
        assert!(with.starts_with("You are Scout"));
        assert!(with.contains("Find things."));
        assert!(with.contains(&bot.folder.display().to_string()));
        assert!(with.contains("/home/moo/VaultCodex"));
        let without = system_prompt(&bot, None, None);
        assert!(!without.contains("Codex"));
        let remembering = system_prompt(&bot, None, Some(Path::new("/home/moo/CatbotMemory")));
        assert!(remembering.contains("/home/moo/CatbotMemory") && remembering.contains("Bots/Scout.md"));
        assert!(!remembering.contains("Codex"));
    }

    #[test]
    fn names_are_quoted_for_the_shell() {
        assert_eq!(quote("Scout"), "Scout");
        assert_eq!(quote("Tree Helper"), "\"Tree Helper\"");
    }
}
