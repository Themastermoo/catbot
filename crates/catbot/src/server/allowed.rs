//! "Always allow": what a bot may do without asking, besides what its approval level lets it.
//!
//! Pressing "Always allow" on an approval card remembers that kind of action for that bot,
//! wherever it works: its own chat, a channel, or another bot's chat. A file write or edit
//! allows writing and editing files; a command allows its family (`cargo test …`, not every
//! `cargo`); any other tool allows that tool. The rules are kept per bot and listed (and
//! removed) in the bot's settings.
//!
//! The bot's approval level is read again when a question comes in, so switching a bot to Full
//! access while it works (or while its teammates wait their turn) stops the asking at once.

use serde_json::{Value, json};

use super::App;
use crate::claude::PermissionMode;

fn key_for(bot_id: &str) -> String {
    format!("allowed:{bot_id}")
}

/// The tools that write or edit a file, across the AIs Catbot drives.
fn writes_files(tool: &str) -> bool {
    matches!(tool, "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "write_file" | "edit_file" | "apply_patch" | "Patch")
}

fn runs_commands(tool: &str) -> bool {
    matches!(tool, "Bash" | "run_command" | "shell" | "Shell")
}

/// The pieces of a command line, split where one command ends and another begins, so that an
/// allowed `cd` can't carry an `rm` in with it.
fn pieces(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let (mut single, mut double) = (false, false);
    while let Some(c) = chars.next() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '&' | '|' | ';' | '\n' if !single && !double => {
                if matches!(chars.peek(), Some('&') | Some('|')) {
                    chars.next();
                }
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    out.push(current);
    out.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// Programs whose first word is a subcommand that changes what they do (`git push` is not `git
/// status`).
const WITH_SUBCOMMANDS: &[&str] = &[
    "cargo", "git", "gh", "npm", "npx", "pnpm", "yarn", "bun", "deno", "docker", "podman", "kubectl", "go", "pip", "pip3", "uv",
    "apt", "apt-get", "dnf", "pacman", "snap", "flatpak", "brew", "systemctl", "rustup", "dx", "make", "just", "dotnet", "gradle", "flutter",
];

/// A command's family: its program, with its subcommand for programs that have them (`cargo
/// test`, `git status`, `ls`, `echo`). Leading `VAR=value` settings are skipped.
fn family(piece: &str) -> String {
    let words: Vec<&str> = piece.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
    let Some(program) = words.first() else { return String::new() };
    let program = program.rsplit('/').next().unwrap_or(program);
    match words.get(1) {
        Some(sub) if WITH_SUBCOMMANDS.contains(&program) && !sub.starts_with('-') && sub.chars().all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c)) => format!("{program} {sub}"),
        _ => program.to_string(),
    }
}

/// Whether a command line could hide another command inside it ($(…), backticks).
fn nests(command: &str) -> bool {
    command.contains("$(") || command.contains('`')
}

/// The rules an "Always allow" on this question would add: one for files, the command families
/// in a command line, or the tool itself. Empty when nothing safe can be remembered (a command
/// that runs other commands inside it).
pub fn rules_for(tool: &str, input: &Value) -> Vec<String> {
    if writes_files(tool) {
        return vec!["files".into()];
    }
    if runs_commands(tool) {
        let command = input["command"].as_str().unwrap_or_default();
        if nests(command) {
            return Vec::new();
        }
        let mut families: Vec<String> = Vec::new();
        for piece in pieces(command) {
            let f = family(&piece);
            let rule = format!("command:{f}");
            if !f.is_empty() && !families.contains(&rule) {
                families.push(rule);
            }
        }
        return families;
    }
    vec![format!("tool:{tool}")]
}

/// How a rule reads in the bot's settings.
pub fn label(rule: &str) -> String {
    if rule == "files" {
        "Write and edit files".into()
    } else if let Some(family) = rule.strip_prefix("command:") {
        format!("Run `{family}` commands")
    } else if let Some(tool) = rule.strip_prefix("tool:") {
        format!("Use {}", tool.strip_prefix("mcp__").map(|t| t.replace("__", " · ").replace('_', " ")).unwrap_or_else(|| tool.to_string()))
    } else {
        rule.to_string()
    }
}

/// The card's "Always allow" button text, or None when it can't be offered.
pub fn offer(tool: &str, input: &Value) -> Option<String> {
    let rules = rules_for(tool, input);
    if rules.is_empty() {
        return None;
    }
    Some(match rules.as_slice() {
        [one] if one == "files" => "Always allow file edits".into(),
        [one] if one.starts_with("command:") => format!("Always allow `{}`", &one["command:".len()..]),
        [_] => "Always allow this tool".into(),
        _ => "Always allow these commands".into(),
    })
}

/// The bot's remembered rules.
pub fn rules(app: &App, bot_id: &str) -> Vec<String> {
    app.with_store(|store| Ok(rules_in(store, bot_id))).unwrap_or_default()
}

/// The same, with the store already open.
pub fn rules_in(store: &crate::store::Store, bot_id: &str) -> Vec<String> {
    store
        .setting(&key_for(bot_id))
        .ok()
        .flatten()
        .and_then(|v| v.as_array().cloned())
        .map(|list| list.iter().filter_map(|r| r.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn keep(app: &App, bot_id: &str, rules: &[String]) {
    app.with_store(|store| store.set_setting(&key_for(bot_id), &json!(rules)));
}

/// Remembers rules for a bot.
pub fn add(app: &App, bot_id: &str, new: &[String]) {
    let mut all = rules(app, bot_id);
    for rule in new {
        if !all.contains(rule) {
            all.push(rule.clone());
        }
    }
    keep(app, bot_id, &all);
}

/// Forgets one rule. False if the bot didn't have it.
pub fn forget(app: &App, bot_id: &str, rule: &str) -> bool {
    let mut all = rules(app, bot_id);
    let before = all.len();
    all.retain(|r| r != rule);
    keep(app, bot_id, &all);
    all.len() != before
}

/// The rules for the bot's settings: each with how it reads.
pub fn wire(rules: &[String]) -> Value {
    Value::Array(rules.iter().map(|r| json!({ "rule": r, "label": label(r) })).collect())
}

/// Whether the bot may do this now without asking: its approval level as it is now (not as it
/// was when the turn began), or a rule it was given.
pub fn covers(app: &App, bot_id: &str, tool: &str, input: &Value) -> bool {
    let mode = app.with_store(|store| store.bot_by_id(bot_id)).flatten().map(|b| b.mode);
    let files = writes_files(tool);
    match mode {
        Some(PermissionMode::BypassPermissions) => return true,
        Some(PermissionMode::AcceptEdits | PermissionMode::Auto) if files => return true,
        _ => {}
    }
    let wanted = rules_for(tool, input);
    if wanted.is_empty() {
        return false;
    }
    let have = rules(app, bot_id);
    wanted.iter().all(|w| have.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(command: &str) -> Vec<String> {
        rules_for("Bash", &json!({ "command": command }))
    }

    #[test]
    fn a_command_is_remembered_by_its_family() {
        assert_eq!(bash("cargo test -q -p catbot"), ["command:cargo test"]);
        assert_eq!(bash("ls -la /tmp"), ["command:ls"]);
        assert_eq!(bash("RUST_LOG=debug /usr/bin/git status"), ["command:git status"]);
        assert_eq!(bash("python3 script.py"), ["command:python3"]);
        assert_eq!(bash("echo hi there"), ["command:echo"]);
        assert_eq!(bash("git -C x push"), ["command:git"], "an option first: the whole program");
    }

    #[test]
    fn a_chain_needs_every_link_allowed() {
        assert_eq!(bash("cd crates && cargo build; echo 'a && b' | wc -l"), ["command:cd", "command:cargo build", "command:echo", "command:wc"]);
        assert!(bash("echo $(rm -rf ~)").is_empty(), "nothing hidden inside is remembered");
        assert!(bash("echo `whoami`").is_empty());
    }

    #[test]
    fn files_and_tools_read_plainly() {
        assert_eq!(rules_for("Write", &json!({ "file_path": "/x" })), ["files"]);
        assert_eq!(rules_for("mcp__blender__render", &json!({})), ["tool:mcp__blender__render"]);
        assert_eq!(label("files"), "Write and edit files");
        assert_eq!(label("command:cargo test"), "Run `cargo test` commands");
        assert_eq!(label("tool:mcp__blender__render"), "Use blender · render");
        assert_eq!(offer("Edit", &json!({})).as_deref(), Some("Always allow file edits"));
        assert_eq!(offer("Bash", &json!({ "command": "cargo test" })).as_deref(), Some("Always allow `cargo test`"));
        assert_eq!(offer("Bash", &json!({ "command": "echo $(id)" })), None);
    }
}
