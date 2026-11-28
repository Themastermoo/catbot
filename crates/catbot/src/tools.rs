//! The tools Catbot gives bots on a chat API (`api.rs`): reading, listing and searching files,
//! writing and editing them, and running commands, in the bot's own folder and the folders it
//! was given (its memory vault, the Codex if allowed).
//!
//! Reading never asks. Writing and editing ask unless the bot's approval level lets it edit
//! freely; commands ask unless it runs everything without asking; in plan mode neither happens.
//! A path outside the bot's folders is refused, whatever the approval level, and so is one that
//! leads outside through a link. Commands can't be fenced in the same way, which is why they ask.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::claude::PermissionMode;

/// Most of a file shown at once, and of a command's output.
const MOST_TEXT: usize = 100_000;
/// Cap on reading a single file into memory for `read_file` or `edit_file`.
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MOST_LINES: usize = 2_000;
const MOST_ENTRIES: usize = 500;
const MOST_MATCHES: usize = 200;
const COMMAND_TIME: Duration = Duration::from_secs(120);

/// Where a bot may work, and how freely.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Its own folder: relative paths start here, and commands run here.
    pub folder: PathBuf,
    /// Every folder it may read and write (its own first).
    pub roots: Vec<PathBuf>,
    pub mode: PermissionMode,
    /// Catbot's own tools for this turn (`catbot_tools.rs`), if it has any: where, and which.
    pub catbot: Option<(crate::catbot_tools::Access, Vec<Value>)>,
}

/// The tools, as the chat API is told about them: the workspace's own, and Catbot's.
pub fn definitions(ws: &Workspace) -> Value {
    let mut all = base_definitions();
    if let (Some(list), Some((_, tools))) = (all.as_array_mut(), &ws.catbot) {
        list.extend(tools.iter().map(crate::catbot_tools::as_function));
    }
    all
}

fn base_definitions() -> Value {
    let path = json!({ "type": "string", "description": "A file path, relative to your working folder or absolute inside one of your folders." });
    json!([
        tool("read_file", "Read a text file. Long files come in parts: give offset (the first line, from 1) and limit (how many lines).",
            json!({ "path": path, "offset": { "type": "integer" }, "limit": { "type": "integer" } }), &["path"]),
        tool("list_files", "List a folder's files and folders (folders end with /).",
            json!({ "path": { "type": "string", "description": "A folder; your working folder if left out." } }), &[]),
        tool("search_files", "Find lines containing some text (any case) in the files under a folder.",
            json!({ "text": { "type": "string" }, "path": { "type": "string", "description": "A folder; your working folder if left out." } }), &["text"]),
        tool("write_file", "Write a whole text file, making its folders if needed. Replaces the file if it exists.",
            json!({ "path": path, "content": { "type": "string" } }), &["path", "content"]),
        tool("edit_file", "Replace one exact piece of a text file with new text. old_text must appear exactly once.",
            json!({ "path": path, "old_text": { "type": "string" }, "new_text": { "type": "string" } }), &["path", "old_text", "new_text"]),
        tool("run_command", "Run a shell command in your working folder and get what it prints. It stops after two minutes.",
            json!({ "command": { "type": "string" } }), &["command"]),
    ])
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({ "type": "function", "function": { "name": name, "description": description,
        "parameters": { "type": "object", "properties": properties, "required": required } } })
}

/// A call checked and ready: what the person sees (named like Claude's tools, so steps and
/// approval cards read the same), whether it must ask first, and the work itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// "Read", "List", "Search", "Write", "Edit" or "Bash".
    pub shown: &'static str,
    /// What the step shows: `file_path`, `path`, `pattern` or `command`.
    pub input: Value,
    pub ask: bool,
    work: Work,
}

#[derive(Debug, Clone, PartialEq)]
enum Work {
    Read { file: PathBuf, offset: usize, limit: usize },
    List { dir: PathBuf },
    Search { dir: PathBuf, text: String },
    Write { file: PathBuf, content: String },
    Edit { file: PathBuf, old: String, new: String },
    Run { command: String },
    /// One of Catbot's own tools, answered by `catbot serve`.
    Catbot { name: String, args: Value },
}

/// Checks a call: its arguments, its paths, and whether it needs a yes. An error is told to the bot.
pub fn plan(ws: &Workspace, name: &str, args: &Value) -> Result<Planned, String> {
    let text = |key: &str| args[key].as_str().map(str::to_string).ok_or_else(|| format!("{name} needs {key}"));
    let free_edits = matches!(ws.mode, PermissionMode::AcceptEdits | PermissionMode::Auto | PermissionMode::BypassPermissions);
    let planning = ws.mode == PermissionMode::Plan;
    let refuse_in_plan = || Err("This bot is in plan mode: it may look, but change nothing. Describe the change instead.".to_string());
    Ok(match name {
        "read_file" => {
            let file = ws.resolve(&text("path")?)?;
            let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
            let limit = args["limit"].as_u64().map(|l| l as usize).unwrap_or(MOST_LINES).clamp(1, MOST_LINES);
            Planned { shown: "Read", input: json!({ "file_path": show(&file) }), ask: false, work: Work::Read { file, offset, limit } }
        }
        "list_files" => {
            let dir = ws.resolve(args["path"].as_str().unwrap_or("."))?;
            Planned { shown: "List", input: json!({ "path": show(&dir) }), ask: false, work: Work::List { dir } }
        }
        "search_files" => {
            let dir = ws.resolve(args["path"].as_str().unwrap_or("."))?;
            let text = text("text")?;
            if text.trim().is_empty() {
                return Err("search_files needs some text to look for".into());
            }
            Planned { shown: "Search", input: json!({ "pattern": text, "path": show(&dir) }), ask: false, work: Work::Search { dir, text } }
        }
        "write_file" => {
            if planning {
                return refuse_in_plan();
            }
            let file = ws.resolve(&text("path")?)?;
            let content = text("content")?;
            Planned { shown: "Write", input: json!({ "file_path": show(&file) }), ask: !free_edits, work: Work::Write { file, content } }
        }
        "edit_file" => {
            if planning {
                return refuse_in_plan();
            }
            let file = ws.resolve(&text("path")?)?;
            let (old, new) = (text("old_text")?, text("new_text")?);
            if old.is_empty() {
                return Err("edit_file needs the old_text to replace".into());
            }
            Planned { shown: "Edit", input: json!({ "file_path": show(&file) }), ask: !free_edits, work: Work::Edit { file, old, new } }
        }
        "run_command" => {
            if planning {
                return refuse_in_plan();
            }
            let command = text("command")?;
            let ask = ws.mode != PermissionMode::BypassPermissions;
            Planned { shown: "Bash", input: json!({ "command": command }), ask, work: Work::Run { command } }
        }
        other => match &ws.catbot {
            Some((_, tools)) if tools.iter().any(|t| t["name"] == other) => {
                Planned { shown: "Catbot", input: json!({ "tool": other, "input": args }), ask: false, work: Work::Catbot { name: other.to_string(), args: args.clone() } }
            }
            _ => return Err(format!("There's no tool called {other}.")),
        },
    })
}

fn show(path: &Path) -> String {
    path.display().to_string()
}

impl Workspace {
    /// A path the bot gave, made absolute and checked to be inside its folders (links followed).
    pub fn resolve(&self, given: &str) -> Result<PathBuf, String> {
        let given = given.trim();
        if given.is_empty() {
            return Err("give a path".into());
        }
        let raw = Path::new(given);
        let joined = if raw.is_absolute() { raw.to_path_buf() } else { self.folder.join(raw) };
        let path = tidy(&joined);
        let real = real_path(&path);
        let inside = self.roots.iter().filter_map(|root| root.canonicalize().ok()).any(|root| real.starts_with(&root));
        if inside {
            Ok(path)
        } else {
            Err(format!("{given} is outside your folders; you can only use {}.", self.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")))
        }
    }
}

/// `a/./b/../c` → `a/c`, without touching the disk.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Where a path really leads: its nearest existing folder with links followed, then the rest.
fn real_path(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut real = existing.canonicalize().unwrap_or(existing);
    for name in rest.into_iter().rev() {
        real.push(name);
    }
    real
}

/// Does the work. `Ok` is what the bot reads back; `Err` is a failure it's told about.
pub async fn run(planned: &Planned, ws: &Workspace) -> Result<String, String> {
    match &planned.work {
        Work::Catbot { name, args } => match &ws.catbot {
            Some((access, _)) => crate::catbot_tools::call(access, name, args).await,
            None => Err("Catbot's tools aren't open to this turn".into()),
        },
        Work::Read { file, offset, limit } => {
            let meta = tokio::fs::metadata(file).await.map_err(|e| format!("couldn't read {}: {e}", file.display()))?;
            if meta.len() > MAX_FILE_BYTES {
                return Err(format!("{} is too large to read as text ({} bytes)", file.display(), meta.len()));
            }
            let bytes = tokio::fs::read(file).await.map_err(|e| format!("couldn't read {}: {e}", file.display()))?;
            if bytes.contains(&0) {
                return Err(format!("{} isn't a text file", file.display()));
            }
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let start = (offset - 1).min(lines.len());
            let end = start.saturating_add(*limit).min(lines.len());
            let mut out = String::new();
            for (i, line) in lines[start..end].iter().enumerate() {
                out.push_str(&format!("{:>6}\t{line}\n", start + i + 1));
                if out.len() > MOST_TEXT {
                    out.push_str("… (cut short; read on with offset)\n");
                    break;
                }
            }
            if end < lines.len() {
                out.push_str(&format!("({} lines in all; read on with offset {})\n", lines.len(), end + 1));
            }
            Ok(if out.is_empty() { "(empty file)".into() } else { out })
        }
        Work::List { dir } => {
            let mut read = tokio::fs::read_dir(dir).await.map_err(|e| format!("couldn't list {}: {e}", dir.display()))?;
            let mut names = Vec::new();
            while let Ok(Some(entry)) = read.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                let folder = match entry.file_type().await {
                    Ok(t) if t.is_dir() => true,
                    Ok(t) if t.is_symlink() => tokio::fs::metadata(entry.path()).await.is_ok_and(|m| m.is_dir()),
                    _ => false,
                };
                names.push(if folder { format!("{name}/") } else { name });
            }
            names.sort();
            let total = names.len();
            names.truncate(MOST_ENTRIES);
            let mut out = names.join("\n");
            if total > MOST_ENTRIES {
                out.push_str(&format!("\n… and {} more", total - MOST_ENTRIES));
            }
            Ok(if out.is_empty() { "(empty folder)".into() } else { out })
        }
        Work::Search { dir, text } => {
            let (dir, text) = (dir.clone(), text.to_lowercase());
            tokio::task::spawn_blocking(move || search(&dir, &text)).await.map_err(|e| e.to_string())
        }
        Work::Write { file, content } => {
            if let Some(parent) = file.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| format!("couldn't make {}: {e}", parent.display()))?;
            }
            tokio::fs::write(file, content).await.map_err(|e| format!("couldn't write {}: {e}", file.display()))?;
            Ok(format!("Wrote {} ({} bytes).", file.display(), content.len()))
        }
        Work::Edit { file, old, new } => {
            let meta = tokio::fs::metadata(file).await.map_err(|e| format!("couldn't read {}: {e}", file.display()))?;
            if meta.len() > MAX_FILE_BYTES {
                return Err(format!("{} is too large to edit as text ({} bytes)", file.display(), meta.len()));
            }
            let text = tokio::fs::read_to_string(file).await.map_err(|e| format!("couldn't read {}: {e}", file.display()))?;
            match text.matches(old.as_str()).count() {
                0 => Err(format!("old_text isn't in {}; read the file and copy it exactly.", file.display())),
                1 => {
                    tokio::fs::write(file, text.replacen(old.as_str(), new, 1)).await.map_err(|e| format!("couldn't write {}: {e}", file.display()))?;
                    Ok(format!("Edited {}.", file.display()))
                }
                n => Err(format!("old_text is in {} {n} times; include more around it so it's there once.", file.display())),
            }
        }
        Work::Run { command } => run_command(command, &ws.folder).await,
    }
}

fn search(dir: &Path, text: &str) -> String {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut files = 0;
    while let Some(here) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&here) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            files += 1;
            if files > 20_000 || entry.metadata().is_ok_and(|m| m.len() > 2_000_000) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            for (n, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(text) {
                    let line: String = line.trim().chars().take(200).collect();
                    found.push(format!("{}:{}: {line}", path.display(), n + 1));
                    if found.len() >= MOST_MATCHES {
                        found.push("… (more; narrow the search)".into());
                        return found.join("\n");
                    }
                }
            }
        }
    }
    if found.is_empty() { "No matches.".into() } else { found.join("\n") }
}

/// Kills a child's entire process group on Unix so backgrounded/grandchild processes do not survive
/// timeout or cancellation.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    if pid > 1 && pid <= i32::MAX as u32 {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // SAFETY: `pid > 1`, so `-(pid as i32)` targets only the child's own process group (SIGKILL = 9).
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
}

struct ChildGroupGuard {
    child: tokio::process::Child,
    #[cfg(unix)]
    pgid: Option<u32>,
}

impl ChildGroupGuard {
    fn kill_all(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pgid.take() {
            kill_process_group(pid);
        }
        let _ = self.child.start_kill();
    }
}

impl Drop for ChildGroupGuard {
    fn drop(&mut self) {
        self.kill_all();
    }
}

/// Reads an output pipe up to `cap` bytes, draining any remaining output so the child cannot block
/// on a full pipe buffer while preventing unbounded memory growth.
async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(mut reader: R, cap: usize) -> (Vec<u8>, bool) {
    use tokio::io::AsyncReadExt;
    let mut out = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        if out.len() < cap {
            let take = (cap - out.len()).min(n);
            out.extend_from_slice(&chunk[..take]);
            if take < n {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    (out, truncated)
}

async fn run_command(command: &str, folder: &Path) -> Result<String, String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        c.creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c.process_group(0);
        c
    };
    let mut child = cmd
        .current_dir(folder)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("couldn't run it: {err}"))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    #[cfg(unix)]
    let pgid = child.id();
    let mut guard = ChildGroupGuard {
        child,
        #[cfg(unix)]
        pgid,
    };

    let cap = MOST_TEXT + 4096;
    let run_fut = async {
        let (out_res, err_res, status_res) = tokio::join!(read_bounded(stdout, cap), read_bounded(stderr, cap), guard.child.wait());
        status_res.map(|status| (status, out_res, err_res))
    };

    let (status, (out_bytes, out_cut), (err_bytes, err_cut)) = match tokio::time::timeout(COMMAND_TIME, run_fut).await {
        Ok(Ok(done)) => {
            #[cfg(unix)]
            {
                guard.pgid = None;
            }
            done
        }
        Ok(Err(err)) => return Err(format!("couldn't run it: {err}")),
        Err(_) => {
            guard.kill_all();
            return Err("It ran for two minutes without finishing, so it was stopped.".into());
        }
    };

    let mut text = String::from_utf8_lossy(&out_bytes).into_owned();
    let errors = String::from_utf8_lossy(&err_bytes);
    if !errors.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&errors);
    }
    let mut truncated = out_cut || err_cut;
    if text.len() > MOST_TEXT {
        let mut cut = MOST_TEXT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        truncated = true;
    }
    if truncated {
        text.push_str("\n… (output cut short)");
    }
    let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "a signal".into());
    if status.success() {
        Ok(if text.trim().is_empty() { "(done, nothing printed)".into() } else { text })
    } else {
        Err(format!("It stopped with {code}.\n{text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catbot_tools_join_the_list_and_need_no_yes() {
        let (_dir, mut ws) = workspace(PermissionMode::Default);
        let mail = json!({ "name": "mail_search", "description": "Search mail", "inputSchema": { "type": "object", "properties": {} } });
        ws.catbot = Some((crate::catbot_tools::Access { url: "http://127.0.0.1:1".into(), key: "k".into() }, vec![mail]));
        let names: Vec<String> = definitions(&ws).as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"read_file".to_string()) && names.contains(&"mail_search".to_string()));
        let planned = plan(&ws, "mail_search", &json!({ "query": "seeds" })).unwrap();
        assert_eq!((planned.shown, planned.ask), ("Catbot", false));
        assert!(plan(&ws, "mail_send", &json!({})).is_err(), "only the tools it was given");
    }

    fn workspace(mode: PermissionMode) -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("bot");
        let memory = dir.path().join("memory");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&memory).unwrap();
        std::fs::write(dir.path().join("secret.txt"), "no").unwrap();
        let ws = Workspace { folder: folder.clone(), roots: vec![folder, memory], mode, catbot: None };
        (dir, ws)
    }

    #[test]
    fn paths_stay_inside_the_bots_folders() {
        let (dir, ws) = workspace(PermissionMode::Default);
        assert_eq!(ws.resolve("notes/a.md").unwrap(), dir.path().join("bot/notes/a.md"));
        assert!(ws.resolve(dir.path().join("memory/Bots/x.md").to_str().unwrap()).is_ok());
        for outside in ["../secret.txt", "/etc/passwd", "notes/../../secret.txt"] {
            assert!(ws.resolve(outside).is_err(), "{outside}");
        }
        #[cfg(unix)]
        {
            // A link inside that leads outside is outside.
            std::os::unix::fs::symlink(dir.path(), dir.path().join("bot/way-out")).unwrap();
            assert!(ws.resolve("way-out/secret.txt").is_err());
        }
    }

    #[test]
    fn what_asks_depends_on_the_approval_level() {
        let args = json!({ "path": "a.txt", "content": "x", "command": "ls", "old_text": "x", "new_text": "y" });
        let asks = |mode, tool: &str| {
            let (_dir, ws) = workspace(mode);
            plan(&ws, tool, &args).map(|p| p.ask)
        };
        assert_eq!(asks(PermissionMode::Default, "read_file"), Ok(false));
        assert_eq!(asks(PermissionMode::Default, "write_file"), Ok(true));
        assert_eq!(asks(PermissionMode::AcceptEdits, "edit_file"), Ok(false));
        assert_eq!(asks(PermissionMode::AcceptEdits, "run_command"), Ok(true));
        assert_eq!(asks(PermissionMode::BypassPermissions, "run_command"), Ok(false));
        assert!(asks(PermissionMode::Plan, "write_file").is_err());
        assert!(asks(PermissionMode::Default, "format_disk").is_err());
        let (_dir, ws) = workspace(PermissionMode::Default);
        let planned = plan(&ws, "run_command", &args).unwrap();
        assert_eq!((planned.shown, planned.input["command"].as_str()), ("Bash", Some("ls")));
    }

    #[tokio::test]
    async fn the_tools_do_their_work() {
        let (_dir, ws) = workspace(PermissionMode::BypassPermissions);
        let go = |name: &str, args: Value| {
            let planned = plan(&ws, name, &args).unwrap();
            let ws = ws.clone();
            async move { run(&planned, &ws).await }
        };
        go("write_file", json!({ "path": "garden/plan.md", "content": "carrots\nbeans\n" })).await.unwrap();
        assert!(go("read_file", json!({ "path": "garden/plan.md" })).await.unwrap().contains("2\tbeans"));
        assert_eq!(go("list_files", json!({})).await.unwrap(), "garden/");
        go("edit_file", json!({ "path": "garden/plan.md", "old_text": "beans", "new_text": "peas" })).await.unwrap();
        assert!(go("search_files", json!({ "text": "PEAS" })).await.unwrap().contains("plan.md:2: peas"));
        assert!(go("edit_file", json!({ "path": "garden/plan.md", "old_text": "kale", "new_text": "x" })).await.is_err());
        #[cfg(unix)]
        {
            assert_eq!(go("run_command", json!({ "command": "cat garden/plan.md | wc -l" })).await.unwrap().trim(), "2");
            assert!(go("run_command", json!({ "command": "exit 3" })).await.unwrap_err().contains("stopped with 3"));
        }
    }
}
