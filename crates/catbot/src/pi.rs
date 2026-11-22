//! A bot's turn on Pi, the open pi coding agent (stage 12.4.4), with the person's own models and
//! keys (Pi signs in to its providers itself: `pi auth login`).
//!
//! Pi runs as `pi --mode rpc`, JSON records a line on its stdin and stdout (its docs/rpc.md):
//!
//! ```text
//! catbot → { type: "prompt", message, images? }
//! pi     → the prompt's response, then session events (message_update deltas, message_end,
//!          tool_execution_start/end, …) until agent_settled; and extension_ui_request records
//! ```
//!
//! **Approvals on cards** (Moo's choice, 2026-11-18). Pi asks nothing on its own, so Catbot loads an
//! extension of its own (`catbot-pi.ts`, written into Catbot's folder) that stops each step that
//! would change something and asks through Pi's `ctx.ui.confirm`, which RPC mode hands to Catbot:
//! the card. The bot's level decides: "Ask" asks for edits and commands, "Auto-accept edits" asks
//! for commands, "Full access" asks nothing, plan mode refuses changes. The same extension gives Pi
//! Catbot's own tools, as an MCP server it registers. Pi runs with `--no-approve`, so files a bot
//! leaves in its own folder (`.pi/`) are never loaded as extensions of their own.
//!
//! Sessions: Catbot names each conversation's Pi session (`--session-id`) in a folder per bot, so
//! the next message carries on; a new session is given the conversation so far.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
};

use crate::approval::{Answerer, Decision};
use crate::claude::{Event, Outcome, PermissionMode, Running, Tokens};
use crate::tools::Workspace;

/// Catbot's extension for Pi: approvals, and Catbot's own tools.
pub const EXTENSION: &str = include_str!("catbot-pi.ts");
/// The title Catbot's extension asks under, so other extensions' questions aren't taken for it.
const ASKS: &str = "catbot-approval";

pub struct PiTurn {
    pub program: PathBuf,
    /// Catbot's folder for this bot's Pi sessions and instructions.
    pub folder: PathBuf,
    /// The extension's file.
    pub extension: PathBuf,
    pub model: String,
    /// The Pi session: an id Catbot chose, carried on when it exists.
    pub session: String,
    /// Whether the session is new (and needs the conversation so far).
    pub new: bool,
    pub instructions: String,
    pub earlier: Vec<(bool, String)>,
    pub text: String,
    /// Pictures with the message: (type, bytes).
    pub pictures: Vec<(String, Vec<u8>)>,
    pub workspace: Workspace,
    /// Catbot's tools as an MCP server: `{ name, command, args, env: [{ name, value }] }`.
    pub mcp: Option<Value>,
    /// `--thinking`: Pi clamps it to what the model can do. None, the model's own level.
    pub effort: Option<String>,
}

/// The extension's word for an approval level.
pub fn mode_word(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::BypassPermissions => "full",
        PermissionMode::AcceptEdits | PermissionMode::Auto => "edits",
        PermissionMode::Plan => "plan",
        PermissionMode::Default => "ask",
    }
}

/// Writes Catbot's extension for Pi where `start` loads it (when it isn't already there as is).
pub fn write_extension(path: &Path) -> std::io::Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(EXTENSION) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, EXTENSION)
}

/// Pi's arguments for a turn.
pub fn args(turn: &PiTurn) -> Vec<String> {
    let mut args: Vec<String> = ["--mode", "rpc", "--no-approve"].iter().map(|s| s.to_string()).collect();
    args.extend(["--session-dir".into(), turn.folder.join("sessions").display().to_string(), "--session-id".into(), turn.session.clone()]);
    args.extend(["-e".into(), turn.extension.display().to_string()]);
    args.extend(["--append-system-prompt".into(), turn.folder.join("instructions.md").display().to_string()]);
    if !turn.model.is_empty() {
        args.extend(["--model".into(), turn.model.clone()]);
    }
    if let Some(effort) = &turn.effort {
        args.extend(["--thinking".into(), effort.clone()]);
    }
    args
}

/// Starts Pi and the turn; its events arrive on the returned `Running`.
pub fn start(turn: PiTurn) -> Result<Running, String> {
    std::fs::create_dir_all(turn.folder.join("sessions")).map_err(|e| format!("couldn't make Pi's folder: {e}"))?;
    std::fs::write(turn.folder.join("instructions.md"), &turn.instructions).map_err(|e| format!("couldn't write the bot's instructions for Pi: {e}"))?;
    write_extension(&turn.extension).map_err(|e| format!("couldn't write Catbot's extension for Pi: {e}"))?;
    let mut command = Command::new(&turn.program);
    let server = turn.mcp.as_ref().map(|s| {
        let env: serde_json::Map<String, Value> = s["env"].as_array().into_iter().flatten().filter_map(|e| Some((e["name"].as_str()?.to_string(), e["value"].clone()))).collect();
        json!({ "command": s["command"], "args": s["args"], "env": env }).to_string()
    });
    command
        .args(args(&turn))
        .env("CATBOT_PI_MODE", mode_word(turn.workspace.mode))
        .env("CATBOT_PI_MCP", server.unwrap_or_default())
        .env("PI_SKIP_VERSION_CHECK", "1")
        .current_dir(&turn.workspace.folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let child = command.spawn().map_err(|e| format!("couldn't start Pi ({}): {e}", turn.program.display()))?;
    let (tx, events) = mpsc::channel(256);
    let answers = Answerer::local();
    let task = tokio::spawn(run(turn, child, tx, answers.clone()));
    Ok(Running::in_process(events, task, answers))
}

#[derive(Default)]
struct Heard {
    said: Vec<String>,
    tokens: Tokens,
    counted: bool,
    errors: Vec<String>,
    stop: Option<String>,
}

async fn send(stdin: &tokio::sync::Mutex<ChildStdin>, record: Value) -> Result<(), String> {
    let mut line = record.to_string();
    line.push('\n');
    stdin.lock().await.write_all(line.as_bytes()).await.map_err(|e| format!("Pi stopped listening ({e})"))
}

async fn run(turn: PiTurn, mut child: Child, tx: mpsc::Sender<Event>, answers: Answerer) {
    use base64::Engine;
    let started = std::time::Instant::now();
    let stdin = Arc::new(tokio::sync::Mutex::new(child.stdin.take().expect("stdin is piped")));
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_tail = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        let text = String::from_utf8_lossy(&buf).trim().to_string();
        text.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect::<String>()
    });
    let heard = Arc::new(Mutex::new(Heard::default()));
    let _ = tx.send(Event::Started { session_id: turn.session.clone(), model: (!turn.model.is_empty()).then(|| turn.model.clone()) }).await;
    let message = if turn.new { first_prompt(&turn) } else { turn.text.clone() };
    let mut prompt = json!({ "id": "catbot-prompt", "type": "prompt", "message": message });
    if !turn.pictures.is_empty() {
        prompt["images"] = json!(turn.pictures.iter().map(|(mime, bytes)| json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes), "mimeType": mime })).collect::<Vec<_>>());
    }
    let ended = match send(&stdin, prompt).await {
        Ok(()) => read(stdout, stdin.clone(), &tx, &answers, &heard).await,
        Err(problem) => Err(problem),
    };
    // Done: closing its input asks Pi to stop in order.
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    let _ = child.start_kill();
    let (said, tokens, errors, stop) = {
        let mut heard = heard.lock().expect("heard lock");
        (heard.said.join("\n\n"), heard.counted.then_some(heard.tokens), std::mem::take(&mut heard.errors), heard.stop.clone())
    };
    if !said.trim().is_empty() {
        let _ = tx.send(Event::Text { text: said.clone() }).await;
    }
    let outcome = match ended {
        Ok(aborted) => {
            let failed = stop.as_deref() == Some("error") || aborted;
            for problem in errors {
                let _ = tx.send(Event::Error { message: explain(&problem, "") }).await;
            }
            let tokens = tokens.or_else(|| Some(Tokens::guess(turn.text.len(), said.len())));
            Outcome { ok: !failed, session_id: Some(turn.session.clone()), result: Some(said), stop_reason: stop, tokens, ..Outcome::default() }
        }
        Err(problem) => {
            let tail = stderr_tail.await.unwrap_or_default();
            let message = explain(&problem, &tail);
            let _ = tx.send(Event::Error { message: message.clone() }).await;
            Outcome { ok: false, result: Some(message), ..Outcome::default() }
        }
    };
    let outcome = Outcome { duration_ms: Some(started.elapsed().as_millis() as u64), ..outcome };
    let _ = tx.send(Event::Finished(outcome)).await;
}

/// A new session's first message: the conversation so far (Pi can't remember it), then the
/// message. The bot's instructions go in Pi's system prompt.
fn first_prompt(turn: &PiTurn) -> String {
    let earlier: Vec<&(bool, String)> = turn.earlier.iter().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
    if earlier.is_empty() {
        return turn.text.clone();
    }
    let mut prompt = String::from("<conversation_so_far>\n");
    for (person, text) in earlier {
        let who = if *person { "Person" } else { "You" };
        let short: String = text.chars().take(4000).collect();
        prompt.push_str(&format!("{who}: {short}\n\n"));
    }
    prompt.push_str("</conversation_so_far>\n\n");
    prompt.push_str(&turn.text);
    prompt
}

/// Reads Pi until it settles: Ok(aborted), or what went wrong.
async fn read(stdout: tokio::process::ChildStdout, stdin: Arc<tokio::sync::Mutex<ChildStdin>>, tx: &mpsc::Sender<Event>, answers: &Answerer, heard: &Arc<Mutex<Heard>>) -> Result<bool, String> {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        // Split on LF only: JSON strings may hold U+2028, which a line reader would split on.
        if reader.read_until(b'\n', &mut line).await.map_err(|e| e.to_string())? == 0 {
            return Err("Pi stopped before it finished".into());
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line) else { continue };
        match record["type"].as_str().unwrap_or_default() {
            "response" if record["id"] == "catbot-prompt" => {
                if record["success"] == false {
                    return Err(record["error"].as_str().unwrap_or("Pi refused the message").to_string());
                }
                if record["data"]["disposition"] == "handled" {
                    return Ok(false);
                }
            }
            "agent_settled" => return Ok(record["aborted"] == true),
            "message_update" => {
                let update = &record["assistantMessageEvent"];
                let text = update["delta"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
                match (update["type"].as_str(), text) {
                    (Some("text_delta"), Some(text)) => {
                        let _ = tx.send(Event::TextDelta { text }).await;
                    }
                    (Some("thinking_delta"), Some(text)) => {
                        let _ = tx.send(Event::ThinkingDelta { text }).await;
                    }
                    _ => {}
                }
            }
            "message_end" if record["message"]["role"] == "assistant" => {
                let message = &record["message"];
                let text: String = message["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("");
                let usage = &message["usage"];
                let mut heard = heard.lock().expect("heard lock");
                if !text.trim().is_empty() {
                    heard.said.push(text);
                }
                if usage.is_object() {
                    heard.tokens.add(Tokens {
                        input: usage["input"].as_u64().unwrap_or(0),
                        output: usage["output"].as_u64().unwrap_or(0),
                        cache_read: usage["cacheRead"].as_u64().unwrap_or(0),
                        cache_write: usage["cacheWrite"].as_u64().unwrap_or(0),
                        estimated: false,
                    });
                    heard.counted = true;
                }
                let stop = message["stopReason"].as_str().map(str::to_string);
                if stop.as_deref() == Some("error")
                    && let Some(problem) = message["errorMessage"].as_str()
                {
                    heard.errors.push(problem.to_string());
                }
                heard.stop = stop;
            }
            "auto_retry_end" if record["success"] == false => {
                if let Some(problem) = record["finalError"].as_str() {
                    heard.lock().expect("heard lock").errors.push(problem.to_string());
                }
            }
            "tool_execution_start" => {
                let id = record["toolCallId"].as_str().unwrap_or("tool").to_string();
                let (name, input) = shown(record["toolName"].as_str().unwrap_or("tool"), &record["args"]);
                let _ = tx.send(Event::ToolStarted { id, name, input }).await;
            }
            "tool_execution_end" => {
                let id = record["toolCallId"].as_str().unwrap_or("tool").to_string();
                let output: String = record["result"]["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n");
                let _ = tx.send(Event::ToolFinished { id, ok: record["isError"] != true, output: output.trim().to_string() }).await;
            }
            "extension_ui_request" => {
                let (stdin, tx, answers, record) = (stdin.clone(), tx.clone(), answers.clone(), record.clone());
                tokio::spawn(async move { answer_ui(&record, &stdin, &tx, &answers).await });
            }
            "extension_error" => eprintln!("catbot: a Pi extension failed ({}): {}", record["extensionPath"].as_str().unwrap_or("?"), record["error"].as_str().unwrap_or("?")),
            _ => {}
        }
    }
}

/// An extension's question: Catbot's own approvals become cards; anything else that wants an
/// answer is let go (no one is at a terminal).
async fn answer_ui(record: &Value, stdin: &tokio::sync::Mutex<ChildStdin>, tx: &mpsc::Sender<Event>, answers: &Answerer) {
    let id = record["id"].clone();
    let reply = match record["method"].as_str() {
        Some("confirm") if record["title"] == ASKS => {
            let asked: Value = serde_json::from_str(record["message"].as_str().unwrap_or("{}")).unwrap_or_default();
            let (tool, input) = shown(asked["tool"].as_str().unwrap_or("tool"), &asked["input"]);
            let question = format!("ask-{}", uuid::Uuid::new_v4());
            let waiting = answers.expect(&question);
            let _ = tx.send(Event::ApprovalRequested { id: question, tool, input }).await;
            let yes = matches!(waiting.await, Decision::Allow);
            json!({ "type": "extension_ui_response", "id": id, "confirmed": yes })
        }
        Some("select" | "confirm" | "input" | "editor") => json!({ "type": "extension_ui_response", "id": id, "cancelled": true }),
        _ => return,
    };
    let _ = send(stdin, reply).await;
}

/// A Pi tool call as Catbot shows it, named like Claude's tools.
pub fn shown(tool: &str, args: &Value) -> (String, Value) {
    let path = args["path"].as_str().or(args["file_path"].as_str()).map(str::to_string);
    let with_path = |name: &str| match &path {
        Some(p) => (name.to_string(), json!({ "file_path": p })),
        None => (name.to_string(), json!({ "description": args.to_string() })),
    };
    match tool {
        "bash" => ("Bash".into(), json!({ "command": args["command"] })),
        "read" => with_path("Read"),
        "edit" => with_path("Edit"),
        "write" => with_path("Write"),
        "grep" | "find" => ("Search".into(), json!({ "pattern": args["pattern"].as_str().or(args["query"].as_str()).unwrap_or_default() })),
        "ls" => with_path("List"),
        other => {
            let name = other.strip_prefix("mcp__catbot__").unwrap_or(other).to_string();
            (name, if args.is_object() { args.clone() } else { json!({}) })
        }
    }
}

/// Pi's own folder, where its sign-ins and models are: `$PI_CODING_AGENT_DIR` or `~/.pi/agent`.
pub fn agent_dir() -> PathBuf {
    std::env::var_os("PI_CODING_AGENT_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| crate::store::user_home().join(".pi").join("agent"))
}

/// Whether Pi has a sign-in saved or models of the person's own: Some(true) when it plainly has,
/// None when Catbot can't tell (a provider's key may be in the environment).
pub fn signed_in() -> Option<bool> {
    let dir = agent_dir();
    let saved = std::fs::read_to_string(dir.join("auth.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()));
    (saved || dir.join("models.json").is_file()).then_some(true)
}

/// The models Pi can use now (those with a sign-in or key), as `provider/model`, with a label and
/// whether it thinks, from `pi --list-models`.
pub async fn models(program: &Path) -> Result<Vec<(String, String, bool)>, String> {
    let mut command = Command::new(program);
    command.arg("--list-models").env("PI_SKIP_VERSION_CHECK", "1").env("PI_OFFLINE", "1").stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output()).await.map_err(|_| "Pi took too long to list its models".to_string())?.map_err(|e| format!("couldn't start Pi: {e}"))?;
    Ok(parse_models(&String::from_utf8_lossy(&output.stdout)))
}

/// `pi --list-models`' table: a header, then `provider model context … thinking …` a line.
pub fn parse_models(table: &str) -> Vec<(String, String, bool)> {
    let mut lines = table.lines();
    let Some(head) = lines.next().filter(|head| head.split_whitespace().take(2).eq(["provider", "model"])) else {
        return Vec::new();
    };
    let thinking = head.split_whitespace().position(|w| w == "thinking");
    lines
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let (provider, model) = (*words.first()?, *words.get(1)?);
            let thinks = thinking.and_then(|at| words.get(at)).is_some_and(|w| *w == "yes");
            Some((format!("{provider}/{model}"), format!("{model} ({provider})"), thinks))
        })
        .collect()
}

/// A failure, with how to put it right.
pub fn explain(problem: &str, stderr: &str) -> String {
    let said = stderr.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut message = format!("Pi didn't finish: {}.", problem.trim_end_matches('.'));
    if !said.is_empty() && !problem.contains(said) {
        message.push_str(&format!(" It said: {said}"));
    }
    let lower = format!("{problem} {stderr}").to_lowercase();
    if ["no api key", "api key", "no model", "unauthorized", "401", "not logged in", "authentication"].iter().any(|w| lower.contains(w)) {
        message.push_str(" Set Pi up on this computer: run `pi auth login` (or give it a provider key), and pick a model in this bot's settings.");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_levels() {
        let turn = PiTurn {
            program: PathBuf::from("pi"),
            folder: PathBuf::from("/c/pi/bot1"),
            extension: PathBuf::from("/c/pi/catbot-pi.ts"),
            model: "openai/gpt-5".into(),
            session: "s-1".into(),
            new: true,
            instructions: "You are Scout.".into(),
            earlier: vec![(true, "Plant carrots?".into())],
            text: "And beans?".into(),
            pictures: Vec::new(),
            workspace: Workspace { folder: PathBuf::from("/b"), roots: vec![PathBuf::from("/b")], mode: PermissionMode::Default, catbot: None },
            mcp: None,
            effort: Some("high".into()),
        };
        assert_eq!(
            args(&turn),
            ["--mode", "rpc", "--no-approve", "--session-dir", "/c/pi/bot1/sessions", "--session-id", "s-1", "-e", "/c/pi/catbot-pi.ts", "--append-system-prompt", "/c/pi/bot1/instructions.md", "--model", "openai/gpt-5", "--thinking", "high"]
        );
        assert!(first_prompt(&turn).starts_with("<conversation_so_far>\nPerson: Plant carrots?") && first_prompt(&turn).ends_with("And beans?"));
        assert_eq!((mode_word(PermissionMode::Default), mode_word(PermissionMode::AcceptEdits), mode_word(PermissionMode::BypassPermissions)), ("ask", "edits", "full"));
    }

    #[test]
    fn tools_are_shown_like_claudes() {
        assert_eq!(shown("bash", &json!({ "command": "ls" })), ("Bash".to_string(), json!({ "command": "ls" })));
        assert_eq!(shown("edit", &json!({ "path": "/b/a.md" })).1["file_path"], "/b/a.md");
        assert_eq!(shown("mcp__catbot__page_publish", &json!({ "x": 1 })).0, "page_publish");
        assert!(explain("No API key for provider openai", "").contains("pi auth login"));
        let table = "provider  model  context  max-out  thinking  images\nstub      stub   128K     16.4K    no        no\nopenai    gpt-5  400K     128K     yes       yes\n";
        assert_eq!(parse_models(table), [("stub/stub".to_string(), "stub (stub)".to_string(), false), ("openai/gpt-5".to_string(), "gpt-5 (openai)".to_string(), true)]);
        assert!(parse_models("No models available. Use /login …").is_empty());
    }
}
