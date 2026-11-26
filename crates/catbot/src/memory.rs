//! The memory vault: a folder of linked Markdown notes the bots keep on this computer.
//!
//! ```text
//! ~/CatbotMemory/
//!   Home.md        what the folder is (written once; the person may edit it)
//!   AGENTS.md      how the bots keep their notes (written once)
//!   Bots/          one note per bot ("Scout.md"), seeded when the bot is made
//!   Projects/      one note per project, shared by the bots that work on it
//!   Journal/       one note per bot per day ("2026-11-15 Scout.md")
//!   Codex -> ~/VaultCodex   a link (not a copy), so notes can point into the Codex
//! ```
//!
//! The bots write the notes themselves: their standing instructions (`prompt`) tell them how.
//! Catbot only makes the folder, seeds each bot's own note, and keeps that note's name in step
//! when a bot is renamed. It's opened as an Obsidian vault in its own right; the phone reads it
//! through its pairing (`server::memory`).
//!
//! A bot with Codex access turned off isn't told about the Codex or given it as a folder, but the
//! vault's `Codex` link is still there: a bot that went looking could follow it. The switch says
//! what a bot is pointed at, not a wall.

use std::path::{Path, PathBuf};

use crate::store::Bot;

/// Where the vault is: `CATBOT_MEMORY`; else `~/CatbotMemory` when it's already there (the
/// first home PC's); else inside Catbot's own data folder, so nothing new appears in a new
/// user's home folder unasked (stage 29.1). `CATBOT_MEMORY=off` turns it off.
pub fn default_dir() -> Option<PathBuf> {
    match std::env::var_os("CATBOT_MEMORY") {
        Some(dir) if dir == "off" => None,
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => {
            let old = crate::store::user_home().join("CatbotMemory");
            Some(if old.is_dir() { old } else { crate::store::default_home().join("memory") })
        }
    }
}

/// The name of the folder inside the vault that leads to the Codex.
pub const CODEX_LINK: &str = "Codex";

/// Makes the vault's folders and starting notes, and its link to the Codex if there is one.
/// Anything already there is left alone, so the person's edits survive.
pub fn prepare(dir: &Path, codex: Option<&Path>) -> std::io::Result<()> {
    for sub in ["Bots", "Projects", "Journal"] {
        std::fs::create_dir_all(dir.join(sub))?;
    }
    let linked = match codex {
        Some(codex) => link_codex(dir, codex),
        None => dir.join(CODEX_LINK).exists(),
    };
    write_new(&dir.join("Home.md"), &home_note(linked))?;
    write_new(&dir.join("AGENTS.md"), AGENTS)?;
    Ok(())
}

/// Links `dir/Codex` to the Codex vault. True if the link is there (made now or before).
fn link_codex(dir: &Path, codex: &Path) -> bool {
    let link = dir.join(CODEX_LINK);
    if link.symlink_metadata().is_ok() {
        return link.is_dir();
    }
    if !codex.is_dir() {
        return false;
    }
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(codex, &link);
    // Windows only makes symbolic links in Developer Mode; a junction needs no permission.
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(codex, &link).or_else(|_| {
        let status = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(codex).output()?.status;
        if status.success() { Ok(()) } else { Err(std::io::Error::other("mklink /J failed")) }
    });
    match made {
        Ok(()) => true,
        Err(err) => {
            eprintln!("catbot: couldn't link the Codex into the memory vault ({err}); notes will name its path instead");
            false
        }
    }
}

/// Whether the vault leads into the Codex.
pub fn has_codex(dir: &Path) -> bool {
    dir.join(CODEX_LINK).is_dir()
}

fn write_new(path: &Path, text: &str) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, text)
}

/// A bot's name as a note name: no characters that file names or `[[links]]` can't hold.
pub fn note_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "/\\:*?\"<>|[]#^".contains(c) || c.is_control() { '-' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() { "Bot".into() } else { cleaned }
}

/// The bot's own note, relative to the vault.
pub fn bot_note(bot: &Bot) -> String {
    format!("Bots/{}.md", note_name(&bot.name))
}

/// Seeds the bot's own note if it has none yet.
pub fn ensure_bot_note(dir: &Path, bot: &Bot) -> std::io::Result<()> {
    let path = dir.join(bot_note(bot));
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let created = bot.created_at.get(..10).unwrap_or(&bot.created_at);
    let instructions = if bot.instructions.trim().is_empty() {
        "No standing instructions yet.".to_string()
    } else {
        bot.instructions.trim().to_string()
    };
    let text = format!(
        "---\ntype: bot\nbot: \"{name}\"\ncreated: {created}\ntags: [catbot, bot]\n---\n\n# {name}\n\n\
         One of Catbot's bots. Back to [[Home]].\n\n\
         ## Who I am\n\n{instructions}\n\n\
         ## Working on\n\n\
         ## What I've learned\n\n\
         ## Journal\n",
        name = bot.name.replace('"', "'"),
    );
    std::fs::write(path, text)
}

/// Keeps the bot's note with the bot when it's renamed: moves the file and updates its name
/// inside. Links to the old name in other notes stay as they were.
pub fn rename_bot(dir: &Path, old: &Bot, new: &Bot) -> std::io::Result<()> {
    let from = dir.join(bot_note(old));
    let to = dir.join(bot_note(new));
    if from == to || !from.exists() || to.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&from)?;
    let text = text
        .replacen(&format!("bot: \"{}\"", old.name.replace('"', "'")), &format!("bot: \"{}\"", new.name.replace('"', "'")), 1)
        .replacen(&format!("\n# {}\n", old.name), &format!("\n# {}\n", new.name), 1);
    std::fs::write(&to, text)?;
    std::fs::remove_file(&from)
}

/// What a bot is told about the vault, for its standing instructions.
pub fn prompt(dir: &Path, bot: &Bot, codex: bool) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d");
    let me = note_name(&bot.name);
    let mut text = format!(
        "\n\nYour memory vault is {path}: Markdown notes that Catbot's bots keep on this computer, linked to one another \
         with [[wikilinks]]. The person reads them in Obsidian. At the start of a conversation, read your own note, \
         Bots/{me}.md (and the vault's AGENTS.md if you haven't yet). Keep these up to date as you work:\n\
         - Bots/{me}.md, about you: who you are, what you're working on, what you've learned. Rewrite it so it stays \
         current rather than growing forever.\n\
         - Projects/<project name>.md, one note per project you work on, shared with the other bots: what it is, where \
         it lives, decisions, current state. Read it before working on a project; add to it, don't overwrite what other \
         bots wrote.\n\
         - Journal/{today} {me}.md, today's journal: a few lines per working session (what was asked, what you did, \
         what's left), linking the projects and notes involved.\n\
         Link each note to the others it relates to. Save what will matter later, not every detail, and say briefly \
         what you saved. Never put passwords, keys or other secrets in these notes.",
        path = dir.display(),
    );
    if codex {
        text.push_str(
            "\nThe vault's Codex folder is the person's Codex itself, not a copy. When a project or subject has a Codex \
             note, link to it from your notes, like [[Codex/20 Projects/Catbot|Catbot in the Codex]], rather than \
             copying what it says.",
        );
    }
    text
}

fn home_note(codex: bool) -> String {
    let codex = if codex {
        "- **Codex/** — your Codex vault, linked in (not a copy). Notes here point into it, like [[Codex/00 Home|the Codex home]].\n"
    } else {
        ""
    };
    format!(
        "---\ntype: home\ntags: [catbot]\n---\n\n# Catbot memory\n\n\
         The notes Catbot's bots keep on this computer: what each bot is, the projects they work on, and a journal of \
         their working sessions. The bots write them as they work; you can read and edit them too (in Obsidian, open \
         this folder as a vault).\n\n\
         - **Bots/** — one note per bot, named after it.\n\
         - **Projects/** — one note per project, shared by every bot that works on it.\n\
         - **Journal/** — one note per bot per day, like `2026-11-15 Scout`.\n\
         {codex}\n\
         How the bots keep these notes: [[AGENTS]].\n"
    )
}

const AGENTS: &str = "---\ntype: meta\ntags: [catbot]\n---\n\n# How the bots keep this vault\n\n\
Every note starts with a little frontmatter (`type`, plus `bot` or `project` as fits, `tags`) and a `# Title`.\n\n\
- **Your note** (`Bots/<your name>.md`): who you are, what you're working on, what you've learned. Keep it short and \
current; rewrite sections instead of piling on.\n\
- **Project notes** (`Projects/<project>.md`): shared. Read before you work on a project; add to it rather than \
overwrite what another bot wrote, and date what you add. Say where the project lives (folders, repos).\n\
- **Journal** (`Journal/YYYY-MM-DD <your name>.md`): a few lines per working session — what was asked, what you did, \
what's left — with links to the notes involved.\n\
- **Links:** link related notes with `[[wikilinks]]` (`[[Projects/Garden]]`, `[[Bots/Scout]]`). If the vault has a \
`Codex` folder, it is the person's Codex itself: link into it (`[[Codex/20 Projects/Catbot|Catbot]]`) rather than copying \
it.\n\
- **Never** store passwords, keys, tokens or other secrets here.\n\
- Save what will matter in a later conversation; skip the moment-to-moment.\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{claude::PermissionMode, store::{NewBot, Store}};

    fn bot(store: &Store, name: &str) -> Bot {
        store
            .create_bot(NewBot { name: name.into(), instructions: "Find things.".into(), model: None, mode: PermissionMode::Default, codex: true })
            .unwrap()
    }

    #[test]
    fn the_vault_is_made_once_and_linked_to_the_codex() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("00 Home.md"), "# Home").unwrap();
        let vault = dir.path().join("memory");
        prepare(&vault, Some(&codex)).unwrap();
        if cfg!(windows) && !has_codex(&vault) {
            eprintln!("skipped: this Windows can't make links (Wine can't; Windows makes junctions)");
            return;
        }
        for sub in ["Bots", "Projects", "Journal"] {
            assert!(vault.join(sub).is_dir());
        }
        assert!(has_codex(&vault));
        assert_eq!(std::fs::read_to_string(vault.join("Codex/00 Home.md")).unwrap(), "# Home");
        assert!(std::fs::read_to_string(vault.join("Home.md")).unwrap().contains("[[Codex/00 Home"));
        // The person's edits survive the next start.
        std::fs::write(vault.join("Home.md"), "mine").unwrap();
        prepare(&vault, Some(&codex)).unwrap();
        assert_eq!(std::fs::read_to_string(vault.join("Home.md")).unwrap(), "mine");
    }

    #[test]
    fn without_a_codex_the_vault_stands_alone() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("memory");
        prepare(&vault, Some(&dir.path().join("nowhere"))).unwrap();
        assert!(!has_codex(&vault));
        assert!(!std::fs::read_to_string(vault.join("Home.md")).unwrap().contains("Codex"));
    }

    #[test]
    fn each_bot_gets_a_note_that_follows_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("home")).unwrap();
        let vault = dir.path().join("memory");
        prepare(&vault, None).unwrap();
        let scout = bot(&store, "Scout");
        ensure_bot_note(&vault, &scout).unwrap();
        let text = std::fs::read_to_string(vault.join("Bots/Scout.md")).unwrap();
        assert!(text.contains("# Scout") && text.contains("Find things.") && text.contains("bot: \"Scout\""));
        // Seeding again keeps what the bot wrote.
        std::fs::write(vault.join("Bots/Scout.md"), format!("{text}\nLearned: soil is clay.\n")).unwrap();
        ensure_bot_note(&vault, &scout).unwrap();
        assert!(std::fs::read_to_string(vault.join("Bots/Scout.md")).unwrap().contains("soil is clay"));
        let mut ranger = scout.clone();
        ranger.name = "Ranger".into();
        rename_bot(&vault, &scout, &ranger).unwrap();
        assert!(!vault.join("Bots/Scout.md").exists());
        let text = std::fs::read_to_string(vault.join("Bots/Ranger.md")).unwrap();
        assert!(text.contains("# Ranger") && text.contains("bot: \"Ranger\"") && text.contains("soil is clay"));
    }

    #[test]
    fn names_become_safe_note_names() {
        assert_eq!(note_name("Scout"), "Scout");
        assert_eq!(note_name("Tree/Helper: #2"), "Tree-Helper- -2");
        assert_eq!(note_name(" .. "), "Bot");
    }

    #[test]
    fn the_prompt_says_where_and_how() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("home")).unwrap();
        let scout = bot(&store, "Scout");
        let text = prompt(Path::new("/home/moo/CatbotMemory"), &scout, true);
        assert!(text.contains("/home/moo/CatbotMemory") && text.contains("Bots/Scout.md"));
        assert!(text.contains(&format!("Journal/{} Scout.md", chrono::Local::now().format("%Y-%m-%d"))));
        assert!(text.contains("[[Codex/"));
        assert!(!prompt(Path::new("/m"), &scout, false).contains("Codex"));
    }
}
