//! `catbot bot …`: make, list, show, change and delete bots.

use std::io::{BufRead, IsTerminal, Write};

use crate::{
    claude::PermissionMode,
    console::Paint,
    store::{Bot, NewBot, Store},
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub fn create(store: &Store, new: NewBot) -> Result<(), BoxError> {
    let bot = store.create_bot(new)?;
    let paint = Paint::stderr();
    eprintln!("Made {}.", paint.bold(&bot.name));
    show_bot(&bot, 0);
    eprintln!("\n{}", paint.dim(&format!("Talk to it: catbot chat {} \"hello\"", shell_name(&bot.name))));
    Ok(())
}

pub fn list(store: &Store) -> Result<(), BoxError> {
    let bots = store.bots()?;
    if bots.is_empty() {
        eprintln!("No bots yet. Make one with: catbot bot new <name> --instructions \"…\"");
        return Ok(());
    }
    let paint = Paint::stderr();
    for bot in bots {
        let model = bot.model.clone().unwrap_or_else(|| "default model".into());
        let codex = if bot.codex { " · Codex" } else { "" };
        let first_line: String = bot.instructions.lines().next().unwrap_or("").chars().take(60).collect();
        println!("{}  {}", paint.bold(&bot.name), paint.dim(&format!("{model} · {}{codex}", mode_words(bot.mode))));
        if !first_line.is_empty() {
            println!("  {}", paint.dim(&first_line));
        }
    }
    Ok(())
}

pub fn show(store: &Store, name: &str) -> Result<(), BoxError> {
    let bot = store.bot(name)?.ok_or_else(|| format!("no bot named {name}"))?;
    let conversations = store.conversation_count(&bot.id)?;
    println!("{}", Paint::stderr().bold(&bot.name));
    show_bot(&bot, conversations);
    Ok(())
}

fn show_bot(bot: &Bot, conversations: usize) {
    let paint = Paint::stderr();
    let row = |label: &str, value: &str| println!("  {} {value}", paint.dim(&format!("{label:<13}")));
    row("model", bot.model.as_deref().unwrap_or("Claude Code's default"));
    row("approvals", mode_words(bot.mode));
    let codex = match (bot.codex, bot.mode) {
        (false, _) => "off",
        (true, PermissionMode::Default | PermissionMode::Plan) => "read and write (writes ask first)",
        (true, _) => "read and write (writes don't ask, because of the approval setting)",
    };
    row("Codex", codex);
    row("folder", &bot.folder.display().to_string());
    if conversations > 0 {
        row("conversations", &conversations.to_string());
    }
    if bot.instructions.is_empty() {
        row("instructions", "(none)");
    } else {
        println!("  {}", paint.dim("instructions"));
        for line in bot.instructions.lines() {
            println!("    {line}");
        }
    }
}

/// What a mode means for the person, in a few words.
pub fn mode_words(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "asks before acting",
        PermissionMode::AcceptEdits => "edits files without asking",
        PermissionMode::Auto => "decides what is safe to run",
        PermissionMode::Plan => "plans only",
        PermissionMode::BypassPermissions => "never asks",
    }
}

pub struct Changes {
    pub instructions: Option<String>,
    /// Some("") goes back to the default model.
    pub model: Option<String>,
    pub mode: Option<PermissionMode>,
    pub codex: Option<bool>,
}

pub fn edit(store: &Store, name: &str, changes: Changes) -> Result<(), BoxError> {
    let mut bot = store.bot(name)?.ok_or_else(|| format!("no bot named {name}"))?;
    if changes.instructions.is_none() && changes.model.is_none() && changes.mode.is_none() && changes.codex.is_none() {
        return Err("nothing to change: give --instructions, --instructions-file, --model, --mode or --codex".into());
    }
    if let Some(instructions) = changes.instructions {
        bot.instructions = instructions.trim().to_string();
    }
    if let Some(model) = changes.model {
        bot.model = Some(model).filter(|m| !m.trim().is_empty());
    }
    if let Some(mode) = changes.mode {
        bot.mode = mode;
    }
    if let Some(codex) = changes.codex {
        bot.codex = codex;
    }
    store.update_bot(&bot)?;
    eprintln!("Saved {}.", Paint::stderr().bold(&bot.name));
    show_bot(&bot, store.conversation_count(&bot.id)?);
    Ok(())
}

/// Deleting loses the bot's history, so it asks the person to type the name (or pass --yes).
pub fn delete(store: &Store, name: &str, yes: bool) -> Result<(), BoxError> {
    let bot = store.bot(name)?.ok_or_else(|| format!("no bot named {name}"))?;
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err("deleting a bot needs --yes when there's no one at the keyboard".into());
        }
        let conversations = store.conversation_count(&bot.id)?;
        eprint!(
            "Delete {} and its {conversations} conversation(s)? Its folder stays. Type its name to confirm: ",
            bot.name
        );
        std::io::stderr().flush()?;
        let mut typed = String::new();
        std::io::stdin().lock().read_line(&mut typed)?;
        if !typed.trim().eq_ignore_ascii_case(&bot.name) {
            eprintln!("Not deleted.");
            return Ok(());
        }
    }
    let bot = store.delete_bot(&bot.name)?;
    eprintln!("Deleted {}. Its folder is still at {}", bot.name, bot.folder.display());
    Ok(())
}

fn shell_name(name: &str) -> String {
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        name.to_string()
    } else {
        format!("\"{name}\"")
    }
}
