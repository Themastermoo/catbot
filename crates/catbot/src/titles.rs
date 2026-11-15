//! Names for chats. A new chat is named at once from the first line of its first message,
//! then, in the server, a quick one-shot Claude call on a small model writes a better name
//! that replaces it (unless the person has named the chat in the meantime).
//!
//! After OpenMausBot's thread-title.ts and thread-title-generate.ts (Apache-2.0, see NOTICE):
//! the first-line rule, the prompt and the cleaning are theirs, written again in Rust.

use std::{path::PathBuf, time::Duration};

use crate::claude::{self, Event, PermissionMode, Session, Turn};

/// What a chat is called before it has a message.
pub const UNTITLED: &str = "New chat";

/// The model that names chats, unless `CATBOT_TITLE_MODEL` says otherwise ("off" turns
/// naming off: chats keep their first-line names).
pub const DEFAULT_MODEL: &str = "haiku";

/// How long a name may take before the first-line name is kept.
const PATIENCE: Duration = Duration::from_secs(30);

/// The longest name kept, in characters.
const MAX_CHARS: usize = 60;

/// Most of a message the namer is shown.
const INPUT_MAX_CHARS: usize = 1_500;

/// The model to name chats with, from the environment.
pub fn model_from_env() -> Option<String> {
    match std::env::var("CATBOT_TITLE_MODEL") {
        Ok(model) if model.trim().eq_ignore_ascii_case("off") => None,
        Ok(model) if !model.trim().is_empty() => Some(model.trim().to_string()),
        _ => Some(DEFAULT_MODEL.to_string()),
    }
}

/// The name a chat gets at once: its first message's first line, cut to 48 characters.
pub fn from_message(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return UNTITLED.to_string();
    }
    if line.chars().count() > 48 {
        let cut: String = line.chars().take(47).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_string()
    }
}

/// What the namer is asked.
pub fn prompt(text: &str) -> String {
    let text: String = text.trim().chars().take(INPUT_MAX_CHARS).collect();
    format!(
        "Name the conversation that begins with the message below.\n\
         Reply with only a short title: 3 to 6 words, plain text, no quotes, no trailing period.\n\
         Message:\n{text}"
    )
}

/// The namer's reply as a name, or None if it isn't one.
pub fn clean(reply: &str) -> Option<String> {
    let line = reply.trim().lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.strip_prefix("Title:").or_else(|| line.strip_prefix("title:")).unwrap_or(line).trim();
    let line = line.trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’' | '*' | '`' | '#')).trim();
    let line = line.trim_end_matches(['.', '。']).trim();
    if line.is_empty() || line.chars().count() > MAX_CHARS {
        return None;
    }
    Some(line.to_string())
}

/// Asks Claude for a name for a chat that begins with `text`. `cwd` should be an empty
/// folder of Catbot's own: the call has no tools to speak of and needs no project.
pub async fn generate(cli: Vec<String>, cwd: PathBuf, model: String, text: &str) -> Option<String> {
    std::fs::create_dir_all(&cwd).ok()?;
    let turn = Turn {
        prompt: prompt(text),
        session: Session::New(uuid::Uuid::new_v4().to_string()),
        model: Some(model),
        // No approval helper, so anything that would need a yes is refused.
        permission_mode: PermissionMode::Default,
        cwd,
        system_prompt: Some("You only name conversations. Never use tools; answer with the title alone.".into()),
        cli,
        isolated: true,
        approval_helper: None,
        extra_dirs: Vec::new(),
    };
    let reply = tokio::time::timeout(PATIENCE, async move {
        let mut running = claude::start(&turn).await.ok()?;
        let mut text = String::new();
        while let Some(event) = running.events.recv().await {
            match event {
                Event::Text { text: block } => text = block,
                Event::Finished(outcome) => {
                    if !outcome.ok {
                        return None;
                    }
                    return Some(outcome.result.filter(|r| !r.trim().is_empty()).unwrap_or(text));
                }
                _ => {}
            }
        }
        None
    })
    .await
    .ok()
    .flatten()?;
    clean(&reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_lines_make_names() {
        assert_eq!(from_message("  Plan the garden beds\nwith raised boxes"), "Plan the garden beds");
        assert_eq!(from_message("   \n  "), UNTITLED);
        let long = "a".repeat(60);
        let named = from_message(&long);
        assert_eq!(named.chars().count(), 48);
        assert!(named.ends_with('…'));
    }

    #[test]
    fn replies_are_cleaned_into_names() {
        assert_eq!(clean("\"Plans for the garden.\"").as_deref(), Some("Plans for the garden"));
        assert_eq!(clean("Title: Oak removal quote").as_deref(), Some("Oak removal quote"));
        assert_eq!(clean("\n\n**Tree job invoice**\nmore").as_deref(), Some("Tree job invoice"));
        assert_eq!(clean("   "), None);
        assert_eq!(clean(&"x".repeat(80)), None);
    }

    #[test]
    fn the_prompt_carries_the_message_but_not_all_of_a_long_one() {
        let asked = prompt(&"word ".repeat(1000));
        assert!(asked.starts_with("Name the conversation"));
        assert!(asked.len() < 1_800);
    }
}
