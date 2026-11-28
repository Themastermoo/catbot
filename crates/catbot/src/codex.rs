//! A bot's turn on Codex, OpenAI's coding agent, with Moo's ChatGPT plan (stage 12.4.1).
//!
//! Codex runs headless as `codex app-server`, JSON-RPC over its stdin and stdout, one line a
//! message (the way OpenMausBot's server/drivers/codex.ts drives it, Apache-2.0):
//!
//! ```text
//! catbot → initialize, initialized, thread/resume or thread/start, turn/start
//! codex  → item/started and item/completed (its reply, commands, file changes, tool calls),
//!          item/agentMessage/delta and reasoning deltas as it writes, thread/tokenUsage/updated,
//!          approval requests when a step needs a yes, and finally turn/completed
//! ```
//!
//! **One sign-in, shared** (Moo's choice, 2026-11-18). Codex keeps its login in `auth.json` in its
//! home. Catbot's turns run in a Codex home of their own (`<catbot home>/codex`) so the bots' chats
//! stay out of the person's Codex history, with the person's own `auth.json` (`$CODEX_HOME` or
//! `~/.codex`) linked in: Codex writes through the link when it refreshes the login, so the person's
//! Codex and Catbot's stay signed in together. Signing in from Catbot signs in the person's home.
//!
//! Approvals follow the bot's level: "Ask" runs Codex read-only, so every change asks; "Auto-accept
//! edits" lets it write in the bot's folders and asks for commands; "Full access" asks nothing.
//! Catbot's own tools (pages, triggers, apps, the bot's computer…) are given to it as an MCP server.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot},
};

use crate::approval::{Answerer, Decision};
use crate::claude::{AbortOnDrop, Event, Outcome, PermissionMode, Running, Tokens, read_stderr_tail, truncate_output};
use crate::tools::Workspace;

const START_TIME: Duration = Duration::from_secs(90);
/// What the bot's settings say when Codex isn't signed in.
pub const SIGN_IN: &str = "Sign in with ChatGPT in this bot's settings (or run `codex login` on this computer).";

// ---- the homes and the shared sign-in ------------------------------------------------------

/// The person's own Codex home: `$CODEX_HOME`, or `~/.codex`.
pub fn person_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| crate::store::user_home().join(".codex"))
}

/// Catbot's Codex home, where the bots' chats are kept.
pub fn bots_home(catbot_home: &Path) -> PathBuf {
    catbot_home.join("codex")
}

/// Whether the person's Codex is signed in (by ChatGPT or a key).
pub fn signed_in() -> bool {
    person_home().join("auth.json").is_file()
}

/// Links the person's sign-in into Catbot's Codex home. False when there's no sign-in to link.
pub fn link_sign_in(home: &Path) -> std::io::Result<bool> {
    std::fs::create_dir_all(home)?;
    let theirs = person_home().join("auth.json");
    let ours = home.join("auth.json");
    let present = std::fs::symlink_metadata(&ours).ok();
    if !theirs.is_file() {
        if present.is_some() {
            std::fs::remove_file(&ours)?;
        }
        return Ok(false);
    }
    #[cfg(unix)]
    {
        if present.as_ref().is_some_and(|m| m.file_type().is_symlink()) && std::fs::read_link(&ours).ok().as_deref() == Some(theirs.as_path()) {
            return Ok(true);
        }
        if present.is_some() {
            std::fs::remove_file(&ours)?;
        }
        std::os::unix::fs::symlink(&theirs, &ours)?;
    }
    // Windows: a hard link needs no Developer Mode, and Codex writes into the file in place.
    #[cfg(windows)]
    {
        if present.is_some() {
            std::fs::remove_file(&ours)?;
        }
        std::fs::hard_link(&theirs, &ours)?;
    }
    Ok(true)
}

// ---- the JSON-RPC link ----------------------------------------------------------------------

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

#[derive(Clone)]
struct Link {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    waiting: Waiting,
    next: Arc<Mutex<u64>>,
}

struct WaitGuard {
    waiting: Waiting,
    id: u64,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.waiting.lock().expect("waiting lock").remove(&self.id);
    }
}

impl Link {
    fn new(stdin: ChildStdin) -> Self {
        Self { stdin: Arc::new(tokio::sync::Mutex::new(stdin)), waiting: Arc::default(), next: Arc::default() }
    }

    async fn send(&self, message: Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin.lock().await.write_all(line.as_bytes()).await.map_err(|e| format!("Codex stopped listening ({e})"))
    }

    async fn call(&self, method: &str, params: Value, within: Option<Duration>) -> Result<Value, String> {
        let id = {
            let mut next = self.next.lock().expect("id lock");
            *next += 1;
            *next
        };
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().expect("waiting lock").insert(id, tx);
        let _guard = WaitGuard { waiting: self.waiting.clone(), id };
        self.send(json!({ "id": id, "method": method, "params": params })).await?;
        let answer = match within {
            Some(limit) => tokio::time::timeout(limit, rx).await.map_err(|_| format!("{method} got no answer in {} seconds", limit.as_secs()))?,
            None => rx.await,
        };
        answer.map_err(|_| "Codex stopped".to_string())?
    }

    async fn notify(&self, method: &str) -> Result<(), String> {
        self.send(json!({ "method": method })).await
    }

    async fn reply(&self, id: &Value, result: Value) {
        let _ = self.send(json!({ "id": id, "result": result })).await;
    }

    async fn refuse(&self, id: &Value, code: i64, message: &str) {
        let _ = self.send(json!({ "id": id, "error": { "code": code, "message": message } })).await;
    }

    /// Hands an answer to whoever asked; false when the message wasn't an answer.
    fn answered(&self, message: &Value) -> bool {
        if message.get("method").is_some() {
            return false;
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else { return false };
        if let Some(waiter) = self.waiting.lock().expect("waiting lock").remove(&id) {
            let result = match message.get("error") {
                Some(error) => Err(error["message"].as_str().unwrap_or("Codex refused").to_string()),
                None => Ok(message["result"].clone()),
            };
            let _ = waiter.send(result);
        }
        true
    }
}

fn spawn(program: &Path, home: &Path, cwd: &Path) -> std::io::Result<Child> {
    spawn_with(program, home, cwd, &[], &[], Stdio::piped())
}

/// Codex's app-server with settings (`-c …`) and environment of its own.
fn spawn_with(program: &Path, home: &Path, cwd: &Path, settings: &[String], env: &[(&str, &str)], stderr: Stdio) -> std::io::Result<Child> {
    let mut command = Command::new(program);
    command.args(settings).envs(env.iter().copied()).env_remove("OPENAI_API_KEY");
    command
        .arg("app-server")
        .env("CODEX_HOME", home)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command.spawn().map_err(|err| std::io::Error::new(err.kind(), format!("couldn't start Codex ({}): {err}", program.display())))
}

async fn initialize(link: &Link) -> Result<(), String> {
    link.call("initialize", json!({ "clientInfo": { "name": "catbot", "title": "Catbot", "version": env!("CARGO_PKG_VERSION") } }), Some(START_TIME)).await?;
    link.notify("initialized").await
}

// ---- a turn ---------------------------------------------------------------------------------

pub struct CodexTurn {
    /// The reasoning effort for this turn (`turn/start`'s `effort`): None, the model's default.
    pub effort: Option<String>,
    pub program: PathBuf,
    /// Catbot's Codex home, with the sign-in linked in.
    pub home: PathBuf,
    pub model: String,
    /// The Codex thread to carry on, if the conversation had one.
    pub session: Option<String>,
    /// The bot's standing instructions (Codex's developer instructions).
    pub instructions: String,
    /// The conversation before this message, for a new thread: (true for the person, text).
    pub earlier: Vec<(bool, String)>,
    pub text: String,
    /// Pictures with the message, as files.
    pub pictures: Vec<PathBuf>,
    pub workspace: Workspace,
    /// Catbot's own tools as an MCP server: `{ name, command, args, env: [{ name, value }] }`.
    pub mcp: Option<Value>,
    /// On the ChatGPT plan (stage 29.10): its token and API address, in place of Codex's sign-in.
    pub plan: Option<(String, String)>,
}

/// Starts Codex and the turn; its events arrive on the returned `Running`.
pub fn start(turn: CodexTurn) -> Result<Running, String> {
    let child = match &turn.plan {
        Some((token, resource)) => {
            std::fs::create_dir_all(&turn.home).map_err(|e| e.to_string())?;
            // Never Codex's own sign-in here: the plan's token is the only one.
            let _ = std::fs::remove_file(turn.home.join("auth.json"));
            spawn_with(&turn.program, &turn.home, &turn.workspace.folder, &crate::chatgpt::codex_args(resource), &[("CATBOT_CHATGPT_TOKEN", token.as_str())], Stdio::piped())
        }
        None => {
            if !link_sign_in(&turn.home).map_err(|e| format!("couldn't link Codex's sign-in: {e}"))? {
                return Err(format!("Codex isn't signed in. {SIGN_IN}"));
            }
            spawn(&turn.program, &turn.home, &turn.workspace.folder)
        }
    }
    .map_err(|e| e.to_string())?;
    let (tx, events) = mpsc::channel(256);
    let answers = Answerer::local();
    // The running turn, once Codex has started it: steered with `turn/steer` (stage 29.14).
    let live: Arc<Mutex<Option<(Link, String, String)>>> = Arc::default();
    let steer = {
        let live = live.clone();
        crate::claude::Steer::new(move |text: String| {
            let now = live.lock().expect("codex live lock").clone();
            async move {
                let (link, thread, turn) = now.ok_or("the turn isn't running yet, or is ending")?;
                let ask = json!({ "threadId": thread, "input": [{ "type": "text", "text": text }], "expectedTurnId": turn });
                link.call("turn/steer", ask, Some(STEER_TIME)).await.map(|_| ())
            }
        })
    };
    let task = tokio::spawn(run(turn, child, tx, answers.clone(), live));
    Ok(Running::in_process(events, task, answers).with_steer(steer))
}

/// How long Codex has to take a steer.
const STEER_TIME: std::time::Duration = std::time::Duration::from_secs(10);

/// Codex's sandbox and approval policy for a bot's approval level.
pub fn policy(mode: PermissionMode) -> (&'static str, &'static str) {
    match mode {
        PermissionMode::BypassPermissions => ("danger-full-access", "never"),
        PermissionMode::AcceptEdits | PermissionMode::Auto => ("workspace-write", "untrusted"),
        PermissionMode::Default | PermissionMode::Plan => ("read-only", "untrusted"),
    }
}

/// The thread's settings: the folders Codex may write in besides the bot's own, network, and
/// Catbot's tools.
fn config(turn: &CodexTurn) -> Value {
    let others: Vec<String> = turn.workspace.roots.iter().filter(|r| **r != turn.workspace.folder).map(|r| r.display().to_string()).collect();
    let mut config = json!({ "sandbox_workspace_write": { "writable_roots": others, "network_access": true } });
    if let Some(server) = &turn.mcp {
        let env: serde_json::Map<String, Value> = server["env"].as_array().into_iter().flatten().filter_map(|e| Some((e["name"].as_str()?.to_string(), e["value"].clone()))).collect();
        let name = server["name"].as_str().unwrap_or("catbot");
        config["mcp_servers"] = json!({ name: { "command": server["command"], "args": server["args"], "env": env, "tool_timeout_sec": 900 } });
    }
    config
}

/// What the reader has heard, shared with the turn.
#[derive(Default)]
struct Heard {
    /// The reply so far (deltas), until a message finishes.
    text: String,
    /// Finished reply messages.
    said: Vec<String>,
    /// File changes, by item: the files each touches.
    changes: HashMap<String, Vec<String>>,
    /// Tool items started and not yet finished.
    open: Vec<String>,
    tokens: Tokens,
    counted: bool,
    /// Errors already shown, so the turn's end doesn't show them again.
    told: Vec<String>,
}

async fn run(turn: CodexTurn, mut child: Child, tx: mpsc::Sender<Event>, answers: Answerer, live: Arc<Mutex<Option<(Link, String, String)>>>) {
    let started = std::time::Instant::now();
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let mut stderr_tail = AbortOnDrop(tokio::spawn(read_stderr_tail(stderr, 1500)));
    let link = Link::new(stdin);
    let heard = Arc::new(Mutex::new(Heard::default()));
    let (done_tx, done_rx) = mpsc::channel(4);
    let reader = AbortOnDrop(tokio::spawn(read(stdout, link.clone(), tx.clone(), answers, heard.clone(), turn.workspace.mode, done_tx)));

    let result = converse(&turn, &link, &tx, done_rx, &live).await;
    live.lock().expect("codex live lock").take();
    let (said, tokens, told) = {
        let mut heard = heard.lock().expect("heard lock");
        let mut said = std::mem::take(&mut heard.said);
        let rest = std::mem::take(&mut heard.text);
        if !rest.trim().is_empty() {
            said.push(rest);
        }
        (said.join("\n\n"), heard.counted.then_some(heard.tokens), std::mem::take(&mut heard.told))
    };
    if !said.trim().is_empty() {
        let _ = tx.send(Event::Text { text: said.clone() }).await;
    }
    let outcome = match result {
        Ok((thread, status, problem)) => {
            let ok = status == "completed";
            if let Some(problem) = problem.as_ref() {
                // Shown once: it may have come as an error notice already.
                if !told.contains(problem) {
                    let _ = tx.send(Event::Error { message: explain(problem, "") }).await;
                }
            } else if !ok && status != "interrupted" && told.is_empty() {
                let _ = tx.send(Event::Error { message: format!("Codex stopped ({status}).") }).await;
            }
            let tokens = tokens.or_else(|| Some(Tokens::guess(turn.text.len(), said.len())));
            Outcome { ok, session_id: Some(thread), result: Some(said), stop_reason: Some(status), tokens, ..Outcome::default() }
        }
        Err(problem) => {
            let _ = child.start_kill();
            let tail = tokio::time::timeout(Duration::from_secs(2), &mut stderr_tail.0).await.ok().and_then(|r| r.ok()).unwrap_or_default();
            let message = explain(&problem, &tail);
            let _ = tx.send(Event::Error { message: message.clone() }).await;
            Outcome { ok: false, result: Some(message), ..Outcome::default() }
        }
    };
    let outcome = Outcome { duration_ms: Some(started.elapsed().as_millis() as u64), ..outcome };
    let _ = child.start_kill();
    drop(reader);
    drop(stderr_tail);
    let _ = tx.send(Event::Finished(outcome)).await;
}

/// The handshake, the thread and the turn: the thread's id, how the turn ended, and the error
/// Codex gave, if any.
async fn converse(turn: &CodexTurn, link: &Link, tx: &mpsc::Sender<Event>, mut done: mpsc::Receiver<Value>, live: &Mutex<Option<(Link, String, String)>>) -> Result<(String, String, Option<String>), String> {
    initialize(link).await?;
    let (sandbox, approval) = policy(turn.workspace.mode);
    let cwd = turn.workspace.folder.display().to_string();
    let settings = json!({
        "cwd": cwd,
        "approvalPolicy": approval,
        "sandbox": sandbox,
        "developerInstructions": turn.instructions,
        "config": config(turn),
    });
    let mut thread = None;
    if let Some(id) = &turn.session {
        let mut resume = settings.clone();
        resume["threadId"] = json!(id);
        if let Ok(resumed) = link.call("thread/resume", resume, Some(START_TIME)).await {
            thread = resumed["thread"]["id"].as_str().map(str::to_string).or_else(|| Some(id.clone()));
        }
    }
    let resumed = thread.is_some();
    let thread = match thread {
        Some(id) => id,
        None => {
            let mut start = settings.clone();
            start["serviceName"] = json!("catbot");
            if !turn.model.is_empty() {
                start["model"] = json!(turn.model);
            }
            let made = link.call("thread/start", start, Some(START_TIME)).await?;
            made["thread"]["id"].as_str().map(str::to_string).ok_or("Codex made no thread")?
        }
    };
    let model = (!turn.model.is_empty()).then(|| turn.model.clone());
    let _ = tx.send(Event::Started { session_id: thread.clone(), model }).await;
    let text = if resumed { turn.text.clone() } else { first_prompt(turn) };
    let mut input = vec![json!({ "type": "text", "text": text })];
    input.extend(turn.pictures.iter().map(|p| json!({ "type": "localImage", "path": p.display().to_string() })));
    let mut ask = json!({ "threadId": thread, "input": input });
    if !turn.model.is_empty() {
        ask["model"] = json!(turn.model);
    }
    if let Some(effort) = &turn.effort {
        ask["effort"] = json!(effort);
    }
    let started = link.call("turn/start", ask, Some(START_TIME)).await?;
    let id = started["turn"]["id"].as_str().unwrap_or_default().to_string();
    if !id.is_empty() {
        *live.lock().expect("codex live lock") = Some((link.clone(), thread.clone(), id.clone()));
    }
    // Its end: the turn/completed for this turn (Codex has one turn at a time on a thread).
    while let Some(finished) = done.recv().await {
        let ended = &finished["turn"];
        if !id.is_empty() && ended["id"].as_str().is_some_and(|t| t != id) {
            continue;
        }
        let status = ended["status"].as_str().unwrap_or("completed").to_string();
        let problem = ended["error"]["message"].as_str().map(str::to_string);
        return Ok((thread, status, problem));
    }
    Err("Codex stopped before the turn finished".into())
}

/// A new thread's first message: the conversation so far (Codex can't remember it), then the
/// message. The bot's instructions go separately, as developer instructions.
fn first_prompt(turn: &CodexTurn) -> String {
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

async fn read(stdout: tokio::process::ChildStdout, link: Link, tx: mpsc::Sender<Event>, answers: Answerer, heard: Arc<Mutex<Heard>>, mode: PermissionMode, done: mpsc::Sender<Value>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else { continue };
        if link.answered(&message) {
            continue;
        }
        let method = message["method"].as_str().unwrap_or_default().to_string();
        let params = message["params"].clone();
        match message.get("id").cloned() {
            // Codex asks something: in its own task, so its updates keep flowing.
            Some(id) => {
                let (link, tx, answers, heard) = (link.clone(), tx.clone(), answers.clone(), heard.clone());
                tokio::spawn(async move { serve(&method, &id, &params, &link, &tx, &answers, &heard, mode).await });
            }
            None if method == "turn/completed" => {
                let _ = done.send(params).await;
            }
            None => notice(&method, &params, &tx, &heard).await,
        }
    }
    link.waiting.lock().expect("waiting lock").clear();
}

/// One of Codex's notifications.
async fn notice(method: &str, params: &Value, tx: &mpsc::Sender<Event>, heard: &Mutex<Heard>) {
    match method {
        "item/agentMessage/delta" => {
            if let Some(text) = params["delta"].as_str().filter(|t| !t.is_empty()) {
                heard.lock().expect("heard lock").text.push_str(text);
                let _ = tx.send(Event::TextDelta { text: text.to_string() }).await;
            }
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            if let Some(text) = params["delta"].as_str().filter(|t| !t.is_empty()) {
                let _ = tx.send(Event::ThinkingDelta { text: text.to_string() }).await;
            }
        }
        "item/started" => {
            let item = &params["item"];
            let id = item["id"].as_str().unwrap_or("item").to_string();
            if item["type"] == "fileChange" {
                let files = item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str().map(str::to_string)).collect();
                heard.lock().expect("heard lock").changes.insert(id.clone(), files);
            }
            if let Some((name, input)) = shown(item) {
                heard.lock().expect("heard lock").open.push(id.clone());
                let _ = tx.send(Event::ToolStarted { id, name, input }).await;
            }
        }
        "item/completed" => {
            let item = &params["item"];
            let id = item["id"].as_str().unwrap_or("item").to_string();
            if item["type"] == "agentMessage" {
                let text = item["text"].as_str().unwrap_or_default().to_string();
                let mut heard = heard.lock().expect("heard lock");
                heard.text.clear();
                if !text.trim().is_empty() {
                    heard.said.push(text);
                }
                return;
            }
            let was_open = {
                let mut heard = heard.lock().expect("heard lock");
                let open = heard.open.contains(&id);
                heard.open.retain(|o| *o != id);
                open
            };
            if was_open {
                let ok = matches!(item["status"].as_str(), Some("completed") | None) && item["exitCode"].as_i64().is_none_or(|c| c == 0) && item["error"].is_null();
                let _ = tx.send(Event::ToolFinished { id, ok, output: output_of(item) }).await;
            }
        }
        "thread/tokenUsage/updated" => {
            let last = &params["tokenUsage"]["last"];
            let cached = last["cachedInputTokens"].as_u64().unwrap_or(0);
            let used = Tokens {
                input: last["inputTokens"].as_u64().unwrap_or(0).saturating_sub(cached),
                output: last["outputTokens"].as_u64().unwrap_or(0),
                cache_read: cached,
                cache_write: last["cacheWriteInputTokens"].as_u64().unwrap_or(0),
                estimated: false,
            };
            let mut heard = heard.lock().expect("heard lock");
            heard.tokens.add(used);
            heard.counted = true;
        }
        "error" if params["willRetry"] != true => {
            let message = params["error"]["message"].as_str().unwrap_or("Codex hit an error").to_string();
            heard.lock().expect("heard lock").told.push(message.clone());
            let _ = tx.send(Event::Error { message: explain(&message, "") }).await;
        }
        _ => {}
    }
}

/// A Codex step as Catbot shows it, named like Claude's tools where it can be.
fn shown(item: &Value) -> Option<(String, Value)> {
    Some(match item["type"].as_str()? {
        "commandExecution" => ("Bash".into(), json!({ "command": item["command"] })),
        "fileChange" => {
            let files: Vec<&str> = item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect();
            let input = match files.as_slice() {
                [one] => json!({ "file_path": one }),
                many => json!({ "description": format!("{} files: {}", many.len(), many.join(", ")) }),
            };
            ("Edit".into(), input)
        }
        "mcpToolCall" => {
            let input = if item["arguments"].is_object() { item["arguments"].clone() } else { json!({}) };
            (item["tool"].as_str().unwrap_or("tool").to_string(), input)
        }
        "dynamicToolCall" => (item["tool"].as_str().unwrap_or("tool").to_string(), if item["arguments"].is_object() { item["arguments"].clone() } else { json!({}) }),
        "webSearch" => ("WebSearch".into(), json!({ "query": item["query"] })),
        "imageView" => ("Read".into(), json!({ "file_path": item["path"] })),
        _ => return None,
    })
}

fn output_of(item: &Value) -> String {
    let out = match item["type"].as_str().unwrap_or_default() {
        "commandExecution" => item["aggregatedOutput"].as_str().unwrap_or_default().to_string(),
        "fileChange" => item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect::<Vec<_>>().join("\n"),
        "mcpToolCall" => match item["error"]["message"].as_str() {
            Some(error) => error.to_string(),
            None => item["result"]["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"),
        },
        _ => String::new(),
    };
    truncate_output(out.trim(), 4000)
}

/// Answers one of Codex's requests: approvals become cards, unless the bot's level allows.
#[allow(clippy::too_many_arguments)]
async fn serve(method: &str, id: &Value, params: &Value, link: &Link, tx: &mpsc::Sender<Event>, answers: &Answerer, heard: &Mutex<Heard>, mode: PermissionMode) {
    let edits_free = matches!(mode, PermissionMode::AcceptEdits | PermissionMode::Auto | PermissionMode::BypassPermissions);
    let all_free = mode == PermissionMode::BypassPermissions;
    let plan_only = mode == PermissionMode::Plan;
    let reason = params["reason"].as_str().map(str::to_string);
    match method {
        "item/commandExecution/requestApproval" | "execCommandApproval" => {
            let command = match &params["command"] {
                Value::Array(words) => words.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
                other => other.as_str().unwrap_or_default().to_string(),
            };
            let mut input = json!({ "command": command });
            if let Some(reason) = reason {
                input["description"] = json!(reason);
            }
            let yes = !plan_only && (all_free || ask(tx, answers, "Bash", input).await);
            let decision = match (method, yes) {
                ("execCommandApproval", true) => "approved",
                ("execCommandApproval", false) => "denied",
                (_, true) => "accept",
                (_, false) => "decline",
            };
            link.reply(id, json!({ "decision": decision })).await;
        }
        "item/fileChange/requestApproval" | "applyPatchApproval" => {
            let files: Vec<String> = match method {
                "applyPatchApproval" => params["fileChanges"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default(),
                _ => heard.lock().expect("heard lock").changes.get(params["itemId"].as_str().unwrap_or_default()).cloned().unwrap_or_default(),
            };
            let mut input = match files.as_slice() {
                [one] => json!({ "file_path": one }),
                [] => json!({ "description": "Codex wants to change files" }),
                many => json!({ "description": format!("{} files: {}", many.len(), many.join(", ")) }),
            };
            if let Some(reason) = reason {
                input["reason"] = json!(reason);
            }
            let yes = !plan_only && (edits_free || ask(tx, answers, "Edit", input).await);
            let decision = match (method, yes) {
                ("applyPatchApproval", true) => "approved",
                ("applyPatchApproval", false) => "denied",
                (_, true) => "accept",
                (_, false) => "decline",
            };
            link.reply(id, json!({ "decision": decision })).await;
        }
        "item/permissions/requestApproval" => {
            let input = json!({ "description": reason.unwrap_or_else(|| "Codex asks for more access".into()) });
            let yes = !plan_only && (all_free || ask(tx, answers, "Permissions", input).await);
            let granted = if yes { params["permissions"].clone() } else { json!({}) };
            link.reply(id, json!({ "permissions": granted, "scope": "turn" })).await;
        }
        // Catbot's own tools ask on their own cards; nothing else may ask the person mid-tool.
        "mcpServer/elicitation/request" => link.reply(id, json!({ "action": "decline" })).await,
        _ => link.refuse(id, -32601, "Catbot doesn't answer that").await,
    }
}

/// Puts a question on a card and waits for the answer.
async fn ask(tx: &mpsc::Sender<Event>, answers: &Answerer, tool: &str, input: Value) -> bool {
    let question = format!("ask-{}", uuid::Uuid::new_v4());
    let waiting = answers.expect(&question);
    if tx.send(Event::ApprovalRequested { id: question.clone(), tool: tool.to_string(), input }).await.is_err() {
        answers.cancel(&question);
        return false;
    }
    matches!(waiting.await, Decision::Allow)
}

/// A failure, with how to put it right.
pub fn explain(problem: &str, stderr: &str) -> String {
    let said = stderr.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut message = if problem.starts_with("Codex") { problem.trim_end_matches('.').to_string() + "." } else { format!("Codex didn't finish: {}.", problem.trim_end_matches('.')) };
    if !said.is_empty() && !problem.contains(said) {
        message.push_str(&format!(" It said: {said}"));
    }
    let lower = format!("{problem} {stderr}").to_lowercase();
    if ["login", "log in", "logged in", "sign in", "signed in", "unauthorized", "401", "auth.json", "credential", "token expired", "refresh token"].iter().any(|w| lower.contains(w)) && !message.contains(SIGN_IN) {
        message.push_str(&format!(" {SIGN_IN}"));
    }
    message
}

// ---- the account: who's signed in, signing in, models --------------------------------------

/// A short talk with `codex app-server`: started, initialized, and its notifications on hand.
pub struct Talk {
    link: Link,
    _child: Child,
    reader: tokio::task::JoinHandle<()>,
    pub notes: mpsc::Receiver<Value>,
}

impl Talk {
    pub async fn open(program: &Path, home: &Path) -> Result<Talk, String> {
        std::fs::create_dir_all(home).map_err(|e| format!("couldn't make {}: {e}", home.display()))?;
        // Talk never reads stderr; use Stdio::null so verbose Codex logs cannot fill the 64 KB pipe and deadlock.
        let mut child = spawn_with(program, home, home, &[], &[], Stdio::null()).map_err(|e| e.to_string())?;
        let link = Link::new(child.stdin.take().expect("stdin is piped"));
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, notes) = mpsc::channel(64);
        let reading = link.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(line.trim()) else { continue };
                if reading.answered(&message) {
                    continue;
                }
                if let Some(id) = message.get("id") {
                    reading.refuse(id, -32601, "Catbot doesn't answer that here").await;
                } else {
                    let _ = tx.send(message).await;
                }
            }
            reading.waiting.lock().expect("waiting lock").clear();
        });
        let talk = Talk { link, _child: child, reader, notes };
        initialize(&talk.link).await?;
        Ok(talk)
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.link.call(method, params, Some(START_TIME)).await
    }
}

impl Drop for Talk {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Who Codex is signed in as, in the person's home: `{ signedIn, kind, email, plan }`.
pub async fn account(program: &Path) -> Result<Value, String> {
    let talk = Talk::open(program, &person_home()).await?;
    let read = talk.call("account/read", json!({})).await?;
    let account = &read["account"];
    Ok(match account["type"].as_str() {
        Some("chatgpt") => json!({ "signedIn": true, "kind": "chatgpt", "email": account["email"], "plan": account["planType"] }),
        Some(kind) => json!({ "signedIn": true, "kind": kind }),
        None => json!({ "signedIn": false }),
    })
}

/// The models Codex offers this account: `[{ id, label, efforts, defaultEffort, isDefault }]`,
/// `efforts` being the reasoning levels the model takes. Asked once every five minutes at most.
pub async fn models(program: &Path, catbot_home: &Path) -> Result<Vec<Value>, String> {
    static LISTED: Mutex<Option<(std::time::Instant, Vec<Value>)>> = Mutex::new(None);
    if let Some((at, models)) = LISTED.lock().expect("models lock").as_ref()
        && at.elapsed() < Duration::from_secs(300)
    {
        return Ok(models.clone());
    }
    let home = bots_home(catbot_home);
    link_sign_in(&home).map_err(|e| e.to_string())?;
    let talk = Talk::open(program, &home).await?;
    let listed = talk.call("model/list", json!({})).await?;
    let models = list_models(&listed);
    *LISTED.lock().expect("models lock") = Some((std::time::Instant::now(), models.clone()));
    Ok(models)
}

/// `model/list`'s answer, as Catbot shows it. Its levels come as words or as
/// `{ reasoningEffort, description }`.
pub fn list_models(listed: &Value) -> Vec<Value> {
    listed["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["hidden"] != true)
        .filter_map(|m| {
            let efforts: Vec<&str> = m["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str().or(e["reasoningEffort"].as_str()).or(e["effort"].as_str()))
                .collect();
            Some(json!({
                "id": m["id"].as_str()?,
                "label": m["displayName"].as_str().or(m["id"].as_str())?,
                "efforts": efforts,
                "defaultEffort": m["defaultReasoningEffort"],
                "isDefault": m["isDefault"] == true,
            }))
        })
        .collect()
}

/// A sign-in under way: what to show the person, and the talk that waits for it to finish.
#[derive(Default)]
pub struct SignIn {
    pub state: Mutex<Value>,
    pub task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Starts signing in with ChatGPT, in the person's home: `how` is "code" (a code typed on any
/// device) or "browser" (a page opened on this computer). Gives back what to show; `finished`
/// is called with the outcome (`{ success, error? }`) when Codex says.
pub async fn sign_in(program: &Path, how: &str, finished: impl FnOnce(Value) + Send + 'static) -> Result<(Value, tokio::task::JoinHandle<()>), String> {
    let mut talk = Talk::open(program, &person_home()).await?;
    let kind = if how == "browser" { "chatgpt" } else { "chatgptDeviceCode" };
    let started = talk.call("account/login/start", json!({ "type": kind })).await?;
    let shown = json!({
        "how": how,
        "url": started["verificationUrl"].as_str().or(started["authUrl"].as_str()),
        "code": started["userCode"],
        "loginId": started["loginId"],
    });
    let task = tokio::spawn(async move {
        let waited = tokio::time::timeout(Duration::from_secs(15 * 60), async {
            while let Some(note) = talk.notes.recv().await {
                if note["method"] == "account/login/completed" {
                    return note["params"].clone();
                }
            }
            json!({ "success": false, "error": "Codex stopped" })
        })
        .await
        .unwrap_or_else(|_| json!({ "success": false, "error": "the sign-in took too long" }));
        drop(talk);
        finished(waited);
    });
    Ok((shown, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_levels() {
        assert_eq!(policy(PermissionMode::Default), ("read-only", "untrusted"));
        assert_eq!(policy(PermissionMode::AcceptEdits), ("workspace-write", "untrusted"));
        assert_eq!(policy(PermissionMode::BypassPermissions), ("danger-full-access", "never"));
    }

    #[test]
    fn steps_are_shown_like_claudes() {
        let (name, input) = shown(&json!({ "type": "commandExecution", "id": "c", "command": "ls -la" })).unwrap();
        assert_eq!((name.as_str(), input["command"].as_str()), ("Bash", Some("ls -la")));
        let (name, input) = shown(&json!({ "type": "fileChange", "id": "f", "changes": [{ "path": "/b/a.md", "diff": "", "kind": { "type": "add" } }] })).unwrap();
        assert_eq!((name.as_str(), input["file_path"].as_str()), ("Edit", Some("/b/a.md")));
        assert!(shown(&json!({ "type": "agentMessage", "id": "m", "text": "hi" })).is_none());
        assert_eq!(output_of(&json!({ "type": "mcpToolCall", "result": { "content": [{ "type": "text", "text": "done" }] } })), "done");
    }

    #[test]
    fn failures_say_how_to_sign_in() {
        let said = explain("unexpected status 401 Unauthorized", "");
        assert!(said.starts_with("Codex didn't finish: unexpected status 401 Unauthorized.") && said.contains("Sign in with ChatGPT"), "{said}");
        assert_eq!(explain("Codex stopped", ""), "Codex stopped.");
    }

    #[test]
    fn models_come_with_their_reasoning_levels() {
        let listed = json!({ "data": [
            { "id": "gpt-5.6-sol", "displayName": "GPT-5.6 Sol", "isDefault": true, "defaultReasoningEffort": "medium",
              "supportedReasoningEfforts": [{ "reasoningEffort": "low", "description": "Fast" }, { "reasoningEffort": "medium" }, { "reasoningEffort": "xhigh" }] },
            { "id": "old", "supportedReasoningEfforts": ["low", "high"] },
            { "id": "secret", "hidden": true }
        ] });
        let models = list_models(&listed);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["efforts"], json!(["low", "medium", "xhigh"]));
        assert_eq!((models[0]["defaultEffort"].as_str(), models[0]["isDefault"].as_bool()), (Some("medium"), Some(true)));
        assert_eq!(models[1]["efforts"], json!(["low", "high"]));
        assert_eq!(models[1]["label"], "old");
    }

    #[test]
    fn a_new_thread_gets_the_conversation() {
        let turn = CodexTurn {
            program: PathBuf::from("codex"),
            home: PathBuf::from("/tmp"),
            model: String::new(),
            session: None,
            instructions: "You are Scout.".into(),
            earlier: vec![(true, "Plant carrots?".into()), (false, "Yes, in spring.".into())],
            text: "And beans?".into(),
            pictures: Vec::new(),
            workspace: Workspace { folder: PathBuf::from("/tmp/b"), roots: vec![PathBuf::from("/tmp/b"), PathBuf::from("/tmp/vault")], mode: PermissionMode::Default, catbot: None },
            mcp: Some(json!({ "name": "catbot", "command": "/bin/catbot", "args": ["approval-mcp", "-"], "env": [{ "name": "K", "value": "v" }] })),
            effort: None,
            plan: None,
        };
        let prompt = first_prompt(&turn);
        assert!(prompt.starts_with("<conversation_so_far>\nPerson: Plant carrots?") && prompt.ends_with("And beans?"));
        let config = config(&turn);
        assert_eq!(config["sandbox_workspace_write"]["writable_roots"], json!(["/tmp/vault"]));
        assert_eq!(config["mcp_servers"]["catbot"]["env"], json!({ "K": "v" }));
    }

    #[test]
    fn the_sign_in_is_linked_in() {
        let dir = tempfile::tempdir().unwrap();
        let person = dir.path().join("person");
        std::fs::create_dir_all(&person).unwrap();
        // SAFETY: only this test reads CODEX_HOME.
        unsafe { std::env::set_var("CODEX_HOME", &person) };
        let ours = dir.path().join("catbot/codex");
        assert!(!link_sign_in(&ours).unwrap(), "nothing to link yet");
        std::fs::write(person.join("auth.json"), "{\"a\":1}").unwrap();
        assert!(link_sign_in(&ours).unwrap());
        // Written through the link, the person's file changes.
        std::fs::write(ours.join("auth.json"), "{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(person.join("auth.json")).unwrap(), "{\"a\":2}");
        assert!(link_sign_in(&ours).unwrap());
        std::fs::remove_file(person.join("auth.json")).unwrap();
        assert!(!link_sign_in(&ours).unwrap() && std::fs::symlink_metadata(ours.join("auth.json")).is_err(), "signed out: the link goes");
    }
}
