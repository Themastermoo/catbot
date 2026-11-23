//! A bot's turn on an agent program over the Agent Client Protocol (ACP): Grok CLI, Gemini CLI,
//! GitHub Copilot, Cursor, OpenCode and the rest (`engines.rs`).
//!
//! The program runs in the bot's folder for the turn and talks JSON-RPC over its stdin and stdout:
//!
//! ```text
//! catbot → initialize, (authenticate), session/load or session/new, session/prompt
//! agent  → session/update notifications (reply text, thinking, tool calls) as it works,
//!          session/request_permission when a step needs a yes, fs/read_text_file and
//!          fs/write_text_file for files, and finally the prompt's answer (its stopReason)
//! ```
//!
//! The agent signs in with its own login and plan, and uses its own tools; Catbot shows its steps,
//! turns its permission requests into approval cards (answered for it where the bot's approval
//! level allows), and serves its file requests inside the bot's folders only. The session id is
//! kept, so the next message resumes it when the agent can load sessions; when it can't, the new
//! session is given the conversation so far.
//!
//! Follows OpenMausBot's server/drivers/acp/core.ts (Apache-2.0) in outline.

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot},
};

use crate::approval::{Answerer, Decision};
use crate::claude::{Event, Outcome, PermissionMode, Running};
use crate::engines::Engine;
use crate::tools::Workspace;

const START_TIME: Duration = Duration::from_secs(90);

/// A turn on an agent program.
pub struct AcpTurn {
    pub engine: &'static Engine,
    pub program: PathBuf,
    pub model: String,
    /// The session to carry on, if the conversation had one.
    pub session: Option<String>,
    /// The bot's standing instructions, given at the start of a new session.
    pub instructions: String,
    /// The conversation before this message: (true for the person, text).
    pub earlier: Vec<(bool, String)>,
    /// The person's message.
    pub text: String,
    pub workspace: Workspace,
    /// MCP servers the agent may use (Catbot's own tools), in ACP's form.
    pub mcp: Vec<Value>,
    /// What the program needs in its environment (`engines::agent_env`).
    pub env: Vec<(String, String)>,
    /// The thinking level to pick, one of the agent's own `thought_level` values. None: its default.
    pub effort: Option<String>,
}

/// Starts the program and the turn; its events arrive on the returned `Running`.
pub fn start(turn: AcpTurn) -> std::io::Result<Running> {
    let args = (turn.engine.args)(turn.workspace.mode, &turn.model);
    let mut command = Command::new(&turn.program);
    command
        .args(&args)
        .envs(turn.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(&turn.workspace.folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let child = command.spawn().map_err(|err| {
        std::io::Error::new(err.kind(), format!("couldn't start {} ({}): {err}", turn.engine.name, turn.program.display()))
    })?;
    let (tx, events) = mpsc::channel(256);
    let answers = Answerer::local();
    let task = tokio::spawn(run(turn, child, tx, answers.clone()));
    Ok(Running::in_process(events, task, answers))
}

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// The JSON-RPC link to the agent.
#[derive(Clone)]
struct Link {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    waiting: Waiting,
    next: Arc<Mutex<u64>>,
}

impl Link {
    async fn send(&self, message: Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin.lock().await.write_all(line.as_bytes()).await.map_err(|e| format!("the agent stopped listening ({e})"))
    }

    /// A request, and its result (or the agent's error message).
    async fn call(&self, method: &str, params: Value, within: Option<Duration>) -> Result<Value, String> {
        let id = {
            let mut next = self.next.lock().expect("id lock");
            *next += 1;
            *next
        };
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().expect("waiting lock").insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await?;
        let answer = match within {
            Some(limit) => tokio::time::timeout(limit, rx).await.map_err(|_| format!("{method} got no answer in {} seconds", limit.as_secs()))?,
            None => rx.await,
        };
        answer.map_err(|_| "the agent stopped".to_string())?
    }

    async fn reply(&self, id: &Value, result: Value) {
        let _ = self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result })).await;
    }

    async fn refuse(&self, id: &Value, code: i64, message: &str) {
        let _ = self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })).await;
    }
}

/// What the reader has heard, shared with the turn.
#[derive(Default)]
struct Heard {
    text: String,
    /// Tool calls started and not yet finished.
    open_tools: Vec<String>,
}

async fn run(turn: AcpTurn, mut child: Child, tx: mpsc::Sender<Event>, answers: Answerer) {
    let started = std::time::Instant::now();
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_tail = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        let text = String::from_utf8_lossy(&buf).trim().to_string();
        text.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect::<String>()
    });
    let link = Link { stdin: Arc::new(tokio::sync::Mutex::new(stdin)), waiting: Arc::default(), next: Arc::default() };
    let heard = Arc::new(Mutex::new(Heard::default()));
    let reader = tokio::spawn(read(stdout, link.clone(), tx.clone(), answers, heard.clone(), turn.workspace.clone()));

    let result = converse(&turn, &link, &tx).await;
    let text = std::mem::take(&mut heard.lock().expect("heard lock").text);
    if !text.trim().is_empty() {
        let _ = tx.send(Event::Text { text: text.clone() }).await;
    }
    let outcome = match result {
        Ok((session, stop)) => {
            let ok = stop != "refusal";
            if !ok {
                let _ = tx.send(Event::Error { message: format!("{} declined to answer.", turn.engine.name) }).await;
            }
            // Agent programs don't say what they used: a guess from what went in and came out.
            let sent = first_prompt(&turn).len();
            let tokens = Some(crate::claude::Tokens::guess(sent, text.len()));
            Outcome { ok, session_id: Some(session), result: Some(text), stop_reason: Some(stop), tokens, ..Outcome::default() }
        }
        Err(problem) => {
            let _ = child.start_kill();
            let tail = stderr_tail.await.unwrap_or_default();
            let message = explain(turn.engine, &problem, &tail);
            let _ = tx.send(Event::Error { message: message.clone() }).await;
            Outcome { ok: false, result: Some(message), ..Outcome::default() }
        }
    };
    let outcome = Outcome { duration_ms: Some(started.elapsed().as_millis() as u64), ..outcome };
    let _ = child.start_kill();
    reader.abort();
    let _ = tx.send(Event::Finished(outcome)).await;
}

/// The handshake, the session and the prompt. The session id and why the agent stopped.
async fn converse(turn: &AcpTurn, link: &Link, tx: &mpsc::Sender<Event>) -> Result<(String, String), String> {
    let init = link
        .call(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientInfo": { "name": "catbot", "version": env!("CARGO_PKG_VERSION") },
                "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true }, "terminal": false },
            }),
            Some(START_TIME),
        )
        .await?;
    let offered: Vec<String> = init["authMethods"].as_array().map(|m| m.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    if let Some(method) = turn.engine.auth.iter().find(|wanted| offered.iter().any(|o| o == *wanted)) {
        // A failed sign-in step isn't the end: the agent may already be signed in, and says so if not.
        let _ = link.call("authenticate", json!({ "methodId": method }), Some(START_TIME)).await;
    }
    let cwd = turn.workspace.folder.display().to_string();
    let can_load = init["agentCapabilities"]["loadSession"] == true;
    let mut session = None;
    let mut options = Value::Null;
    if let (true, Some(id)) = (can_load, turn.session.as_ref())
        && let Ok(loaded) = link.call("session/load", json!({ "sessionId": id, "cwd": cwd, "mcpServers": turn.mcp }), Some(START_TIME)).await
    {
        session = Some(id.clone());
        options = loaded["configOptions"].clone();
    }
    let prompt = match &session {
        Some(_) => turn.text.clone(),
        None => first_prompt(turn),
    };
    let session = match session {
        Some(id) => id,
        None => {
            let made = link.call("session/new", json!({ "cwd": cwd, "mcpServers": turn.mcp }), Some(START_TIME)).await?;
            options = made["configOptions"].clone();
            made["sessionId"].as_str().map(str::to_string).ok_or("the agent made no session")?
        }
    };
    // Its thinking levels, if it has them: kept for the model menu, and the bot's level picked.
    if let Some(levels) = thought_levels(&options) {
        remember_levels(turn.engine, &levels);
        if let Some(wanted) = turn.effort.as_ref().filter(|e| levels.options.iter().any(|(value, _)| value == *e)) {
            // Set before every prompt: some agents forget it between prompts.
            let set = json!({ "sessionId": session, "configId": levels.id, "value": wanted });
            if let Err(problem) = link.call("session/set_config_option", set, Some(START_TIME)).await {
                eprintln!("catbot: {} didn't take the thinking level {wanted}: {problem}", turn.engine.name);
            }
        }
    }
    let _ = tx.send(Event::Started { session_id: session.clone(), model: (!turn.model.is_empty()).then(|| turn.model.clone()) }).await;
    let answer = link.call("session/prompt", json!({ "sessionId": session, "prompt": [{ "type": "text", "text": prompt }] }), None).await?;
    let stop = answer["stopReason"].as_str().unwrap_or("end_turn").to_string();
    Ok((session, stop))
}

/// An agent's thinking levels (ACP's config option of category `thought_level`): its id, and
/// its values with their names, flattened out of any groups.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThoughtLevels {
    pub id: String,
    pub options: Vec<(String, String)>,
    pub current: Option<String>,
}

/// The `thought_level` option among a session's `configOptions`, if it has one.
pub fn thought_levels(options: &Value) -> Option<ThoughtLevels> {
    let option = options.as_array()?.iter().find(|o| o["category"] == "thought_level")?;
    let id = option["id"].as_str().or(option["configId"].as_str())?.to_string();
    let mut values = Vec::new();
    for entry in option["options"].as_array()? {
        let listed = entry["options"].as_array().cloned().unwrap_or_else(|| vec![entry.clone()]);
        for value in listed {
            if let Some(v) = value["value"].as_str() {
                values.push((v.to_string(), value["name"].as_str().unwrap_or(v).to_string()));
            }
        }
    }
    (!values.is_empty()).then(|| ThoughtLevels { id, options: values, current: option["currentValue"].as_str().map(str::to_string) })
}

/// Where an agent's thinking levels are kept between runs: `<agents dir>/<engine>/thinking.json`.
fn levels_file(engine: &Engine) -> Option<PathBuf> {
    crate::engines::agents_dir().map(|dir| dir.join(engine.id).join("thinking.json"))
}

/// The levels seen since Catbot started, by engine (ahead of the file, which other Catbots in
/// the same process, such as tests, may point elsewhere).
static SEEN: std::sync::Mutex<Option<std::collections::HashMap<String, ThoughtLevels>>> = std::sync::Mutex::new(None);

fn remember_levels(engine: &Engine, levels: &ThoughtLevels) {
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(engine.id.to_string(), levels.clone());
    let Some(file) = levels_file(engine) else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(levels) {
        let _ = std::fs::write(file, text);
    }
}

/// The thinking levels this agent offered last time it ran, if it offered any. Unknown until a
/// bot has used it once.
pub fn known_levels(engine: &Engine) -> Option<ThoughtLevels> {
    if let Some(seen) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|s| s.get(engine.id)) {
        return Some(seen.clone());
    }
    let text = std::fs::read_to_string(levels_file(engine)?).ok()?;
    serde_json::from_str(&text).ok()
}

/// A new session's first message: the bot's instructions, the conversation so far (the agent
/// can't remember it), then the message.
fn first_prompt(turn: &AcpTurn) -> String {
    let mut prompt = String::new();
    if !turn.instructions.trim().is_empty() {
        prompt.push_str("<catbot_instructions>\n");
        prompt.push_str(turn.instructions.trim());
        prompt.push_str("\n</catbot_instructions>\n\n");
    }
    let earlier: Vec<&(bool, String)> = turn.earlier.iter().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
    if !earlier.is_empty() {
        prompt.push_str("<conversation_so_far>\n");
        for (person, text) in earlier {
            let who = if *person { "Person" } else { "You" };
            let short: String = text.chars().take(4000).collect();
            prompt.push_str(&format!("{who}: {short}\n\n"));
        }
        prompt.push_str("</conversation_so_far>\n\n");
    }
    prompt.push_str(&turn.text);
    prompt
}

/// Reads the agent: answers to Catbot's requests, its updates, and its own requests.
async fn read(
    stdout: tokio::process::ChildStdout,
    link: Link,
    tx: mpsc::Sender<Event>,
    answers: Answerer,
    heard: Arc<Mutex<Heard>>,
    workspace: Workspace,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let method = message["method"].as_str().map(str::to_string);
        match (method, message.get("id").cloned()) {
            // An answer to one of Catbot's requests.
            (None, Some(id)) => {
                let Some(id) = id.as_u64() else { continue };
                let Some(waiter) = link.waiting.lock().expect("waiting lock").remove(&id) else { continue };
                let result = match message.get("error") {
                    Some(error) => Err(error["message"].as_str().unwrap_or("the agent refused").to_string()),
                    None => Ok(message["result"].clone()),
                };
                let _ = waiter.send(result);
            }
            // The agent asks something of Catbot: in its own task, so updates keep flowing.
            (Some(method), Some(id)) => {
                let (link, tx, answers, workspace) = (link.clone(), tx.clone(), answers.clone(), workspace.clone());
                let params = message["params"].clone();
                tokio::spawn(async move { serve(&method, &id, &params, &link, &tx, &answers, &workspace).await });
            }
            (Some(method), None) if method == "session/update" => update(&message["params"]["update"], &tx, &heard).await,
            _ => {}
        }
    }
    // The agent closed its output: anything still waiting won't be answered.
    link.waiting.lock().expect("waiting lock").clear();
}

/// One session update: reply text, thinking, or a tool call's progress.
async fn update(update: &Value, tx: &mpsc::Sender<Event>, heard: &Mutex<Heard>) {
    match update["sessionUpdate"].as_str().unwrap_or_default() {
        "agent_message_chunk" => {
            if let Some(text) = update["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                heard.lock().expect("heard lock").text.push_str(text);
                let _ = tx.send(Event::TextDelta { text: text.to_string() }).await;
            }
        }
        "agent_thought_chunk" => {
            if let Some(text) = update["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                let _ = tx.send(Event::ThinkingDelta { text: text.to_string() }).await;
            }
        }
        "tool_call" => {
            let id = update["toolCallId"].as_str().unwrap_or("tool").to_string();
            let (name, input) = shown(update);
            heard.lock().expect("heard lock").open_tools.push(id.clone());
            let _ = tx.send(Event::ToolStarted { id: id.clone(), name, input }).await;
            if let Some(ok) = finished(update) {
                heard.lock().expect("heard lock").open_tools.retain(|t| *t != id);
                let _ = tx.send(Event::ToolFinished { id, ok, output: output_of(update) }).await;
            }
        }
        "tool_call_update" => {
            let id = update["toolCallId"].as_str().unwrap_or("tool").to_string();
            if let Some(ok) = finished(update) {
                let was_open = {
                    let mut heard = heard.lock().expect("heard lock");
                    let open = heard.open_tools.contains(&id);
                    heard.open_tools.retain(|t| *t != id);
                    open
                };
                if was_open {
                    let _ = tx.send(Event::ToolFinished { id, ok, output: output_of(update) }).await;
                }
            }
        }
        _ => {}
    }
}

fn finished(update: &Value) -> Option<bool> {
    match update["status"].as_str() {
        Some("completed") => Some(true),
        Some("failed") => Some(false),
        _ => None,
    }
}

/// A tool call as Catbot shows it: named like Claude's tools where the kind says what it is.
fn shown(call: &Value) -> (String, Value) {
    let title = call["title"].as_str().unwrap_or("tool").to_string();
    let name = match call["kind"].as_str() {
        Some("execute") => "Bash".to_string(),
        Some("edit") => "Edit".to_string(),
        Some("read") => "Read".to_string(),
        Some("delete") => "Delete".to_string(),
        Some("search") => "Search".to_string(),
        Some("fetch") => "Fetch".to_string(),
        _ => title.clone(),
    };
    let mut input = call["rawInput"].clone();
    if !input.is_object() {
        input = json!({});
    }
    if input["command"].is_null() && input["file_path"].is_null() && input["path"].is_null() {
        let path = call["locations"][0]["path"].as_str().map(str::to_string);
        match path {
            Some(path) => input["file_path"] = json!(path),
            None => input["description"] = json!(title),
        }
    }
    (name, input)
}

fn output_of(update: &Value) -> String {
    let mut out = String::new();
    for item in update["content"].as_array().into_iter().flatten() {
        if let Some(text) = item["content"]["text"].as_str().or(item["text"].as_str()) {
            out.push_str(text);
            out.push('\n');
        }
    }
    if out.is_empty()
        && let Some(raw) = update.get("rawOutput").filter(|r| !r.is_null())
    {
        out = raw.as_str().map(str::to_string).unwrap_or_else(|| raw.to_string());
    }
    out.trim().to_string()
}

/// Answers one of the agent's requests.
async fn serve(method: &str, id: &Value, params: &Value, link: &Link, tx: &mpsc::Sender<Event>, answers: &Answerer, workspace: &Workspace) {
    match method {
        "session/request_permission" => {
            let options = params["options"].as_array().cloned().unwrap_or_default();
            let pick = |want: &str| {
                options
                    .iter()
                    .find(|o| o["kind"] == format!("{want}_once"))
                    .or_else(|| options.iter().find(|o| o["kind"].as_str().is_some_and(|k| k.starts_with(want))))
                    .and_then(|o| o["optionId"].as_str())
                    .map(str::to_string)
            };
            let (allow, reject) = (pick("allow"), pick("reject"));
            let (tool, input) = shown(&params["toolCall"]);
            let free = match workspace.mode {
                PermissionMode::BypassPermissions => true,
                PermissionMode::AcceptEdits | PermissionMode::Auto => matches!(params["toolCall"]["kind"].as_str(), Some("edit") | Some("delete") | Some("move")),
                _ => false,
            };
            let decision = if free {
                Decision::Allow
            } else {
                let question = format!("ask-{}", uuid::Uuid::new_v4());
                let waiting = answers.expect(&question);
                let _ = tx.send(Event::ApprovalRequested { id: question, tool, input }).await;
                waiting.await
            };
            let outcome = match (decision, allow, reject) {
                (Decision::Allow, Some(option), _) => json!({ "outcome": "selected", "optionId": option }),
                (Decision::Deny(_), _, Some(option)) => json!({ "outcome": "selected", "optionId": option }),
                _ => json!({ "outcome": "cancelled" }),
            };
            link.reply(id, json!({ "outcome": outcome })).await;
        }
        "fs/read_text_file" => match file_of(params, workspace) {
            Ok(path) => match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    let line = params["line"].as_u64().unwrap_or(1).max(1) as usize;
                    let content = match params["limit"].as_u64() {
                        None if line == 1 => content,
                        limit => {
                            let lines: Vec<&str> = content.split('\n').collect();
                            let start = (line - 1).min(lines.len());
                            let end = limit.map(|l| (start + l as usize).min(lines.len())).unwrap_or(lines.len());
                            lines[start..end].join("\n")
                        }
                    };
                    link.reply(id, json!({ "content": content })).await;
                }
                Err(err) => link.refuse(id, -32602, &format!("couldn't read {}: {err}", path.display())).await,
            },
            Err(problem) => link.refuse(id, -32602, &problem).await,
        },
        "fs/write_text_file" => match (file_of(params, workspace), params["content"].as_str()) {
            (Ok(path), Some(content)) => {
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                match tokio::fs::write(&path, content).await {
                    Ok(()) => link.reply(id, json!({})).await,
                    Err(err) => link.refuse(id, -32602, &format!("couldn't write {}: {err}", path.display())).await,
                }
            }
            (Err(problem), _) => link.refuse(id, -32602, &problem).await,
            (_, None) => link.refuse(id, -32602, "no content to write").await,
        },
        _ => link.refuse(id, -32601, "method not found").await,
    }
}

/// A file the agent named: absolute, and inside the bot's folders.
fn file_of(params: &Value, workspace: &Workspace) -> Result<PathBuf, String> {
    let path = params["path"].as_str().ok_or("no path given")?;
    if !std::path::Path::new(path).is_absolute() {
        return Err("file paths must be absolute".into());
    }
    workspace.resolve(path)
}

/// The line an agent prints with its sign-in link (Antigravity's words).
const LINK_PREFIX: &str = "Open the following link to authenticate the ACP server: ";

/// Signs an agent in through its own page. Starts the program, asks it to `authenticate` with
/// `method`, and gives back the link to open (None when it was already signed in and finished at
/// once), with a code to type there when it gives one; `finished` hears how it ended. The link
/// comes as Antigravity prints it, or as ACP's URL elicitation (Muse Code: a device code at
/// Meta's page). The program stays up, waiting for the sign-in to finish, for ten minutes at most.
pub async fn sign_in(
    engine: &'static Engine,
    program: PathBuf,
    env: Vec<(String, String)>,
    folder: PathBuf,
    method: &'static str,
    finished: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(Option<(String, Option<String>)>, tokio::task::JoinHandle<()>), String> {
    let _ = std::fs::create_dir_all(&folder);
    let mut command = Command::new(&program);
    command
        .args((engine.args)(PermissionMode::Default, ""))
        .envs(env)
        .current_dir(&folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|e| format!("couldn't start {}: {e}", engine.name))?;
    let link = Link { stdin: Arc::new(tokio::sync::Mutex::new(child.stdin.take().expect("stdin is piped"))), waiting: Arc::default(), next: Arc::default() };
    // Its answers; a sign-in link it asks Catbot to show is taken (and the link, from its output,
    // for agents that print it); anything else it asks is refused (nothing to do mid-sign-in).
    let (link_tx, link_rx) = oneshot::channel::<(String, Option<String>)>();
    let link_tx = Arc::new(Mutex::new(Some(link_tx)));
    let stdout = child.stdout.take().expect("stdout is piped");
    let reading = link.clone();
    let shown = link_tx.clone();
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else { continue };
            match (message["method"].as_str(), message.get("id")) {
                (None, Some(id)) => {
                    let Some(id) = id.as_u64() else { continue };
                    if let Some(waiter) = reading.waiting.lock().expect("waiting lock").remove(&id) {
                        let result = match message.get("error") {
                            Some(error) => Err(error["message"].as_str().unwrap_or("it refused").to_string()),
                            None => Ok(message["result"].clone()),
                        };
                        let _ = waiter.send(result);
                    }
                }
                (Some("elicitation/create"), Some(id)) if message["params"]["mode"] == "url" => {
                    let params = &message["params"];
                    match params["url"].as_str().filter(|u| u.starts_with("https://")) {
                        Some(url) => {
                            if let Some(tx) = shown.lock().expect("link lock").take() {
                                let _ = tx.send((url.to_string(), device_code(url, params["message"].as_str().unwrap_or_default())));
                            }
                            reading.reply(id, json!({ "action": "accept" })).await;
                        }
                        None => reading.reply(id, json!({ "action": "decline" })).await,
                    }
                }
                (Some(_), Some(id)) => reading.refuse(id, -32601, "not now").await,
                _ => {}
            }
        }
        reading.waiting.lock().expect("waiting lock").clear();
    });
    // The link, from its output. Never shown otherwise: the lines after can carry a code.
    let stderr = child.stderr.take().expect("stderr is piped");
    let watcher = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(url) = line.trim().strip_prefix(LINK_PREFIX).map(str::trim).filter(|u| u.starts_with("https://"))
                && let Some(tx) = link_tx.lock().expect("link lock").take()
            {
                let _ = tx.send((url.to_string(), None));
            }
        }
    });
    link.call(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientInfo": { "name": "catbot", "version": env!("CARGO_PKG_VERSION") },
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false, "elicitation": { "url": {} } },
        }),
        Some(START_TIME),
    )
    .await?;
    let (done_tx, done_rx) = oneshot::channel::<Result<(), String>>();
    let asking = link.clone();
    let task = tokio::spawn(async move {
        let answer = asking.call("authenticate", json!({ "methodId": method }), Some(Duration::from_secs(600))).await.map(|_| ());
        let _ = done_tx.send(answer.clone());
        finished(answer);
        let _ = child.start_kill();
        reader.abort();
        watcher.abort();
    });
    tokio::select! {
        url = link_rx => Ok((url.ok(), task)),
        done = done_rx => match done {
            Ok(Ok(())) => Ok((None, task)),
            Ok(Err(problem)) => Err(problem),
            Err(_) => Err("it stopped".into()),
        },
        _ = tokio::time::sleep(Duration::from_secs(60)) => {
            task.abort();
            Err(format!("{} didn't give a sign-in link", engine.name))
        }
    }
}

/// The code a device sign-in asks the person to type: the link's `code=`, or the one its message
/// names ("…enter the code WVCW-VRLT there.").
fn device_code(url: &str, message: &str) -> Option<String> {
    let from_link = reqwest::Url::parse(url).ok().and_then(|u| u.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()));
    let from_message = || {
        let words: Vec<&str> = message.split_whitespace().collect();
        words.windows(2).find(|w| w[0].eq_ignore_ascii_case("code")).map(|w| w[1].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-').to_string())
    };
    from_link.or_else(from_message).filter(|c| !c.is_empty() && c.len() <= 32)
}

/// Sends the page's "back to the agent" address on, when the browser that finished the sign-in
/// was on another device: only to the local address the sign-in link itself named.
pub async fn deliver_callback(auth_url: &str, pasted: &str) -> Result<(), String> {
    let auth = reqwest::Url::parse(auth_url).map_err(|_| "the sign-in link doesn't read".to_string())?;
    let back = auth.query_pairs().find(|(k, _)| k == "redirect_uri").map(|(_, v)| v.into_owned()).ok_or("the sign-in link names no address to come back to")?;
    let back = reqwest::Url::parse(&back).map_err(|_| "the sign-in link's address doesn't read".to_string())?;
    let pasted = reqwest::Url::parse(pasted.trim()).map_err(|_| "Paste the whole address from the browser's address bar.".to_string())?;
    let local = matches!(back.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) && back.scheme() == "http";
    let same = pasted.scheme() == back.scheme() && pasted.host_str() == back.host_str() && pasted.port_or_known_default() == back.port_or_known_default() && pasted.path() == back.path();
    if !local || !same {
        return Err("That isn't the address the sign-in came back to.".into());
    }
    let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    let answer = client.get(pasted).send().await.map_err(|_| "The sign-in had already finished or stopped. Start it again.".to_string())?;
    if answer.status().is_success() || answer.status().is_redirection() { Ok(()) } else { Err(format!("The sign-in didn't take it ({}).", answer.status())) }
}

/// A failure, with how to put it right.
fn explain(engine: &Engine, problem: &str, stderr: &str) -> String {
    let said = stderr.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut message = format!("{} didn't finish: {problem}.", engine.name);
    if !said.is_empty() {
        message.push_str(&format!(" It said: {said}"));
    }
    let lower = format!("{problem} {stderr}").to_lowercase();
    if ["login", "log in", "logged in", "sign in", "signed in", "auth", "unauthorized", "credential", "api key"].iter().any(|w| lower.contains(w)) {
        message.push_str(&format!(" Sign it in on this computer: {}", engine.sign_in));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_pasted_address_goes_only_where_the_link_said() {
        let link = "https://accounts.google.com/o/oauth2/auth?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A9%2Foauth2callback&state=s";
        assert!(deliver_callback(link, "https://evil.example/oauth2callback?code=c").await.unwrap_err().contains("isn't the address"));
        assert!(deliver_callback(link, "http://localhost:9/other?code=c").await.unwrap_err().contains("isn't the address"));
        let away = "https://accounts.google.com/o/oauth2/auth?redirect_uri=https%3A%2F%2Fexample.com%2Fcb";
        assert!(deliver_callback(away, "https://example.com/cb?code=c").await.unwrap_err().contains("isn't the address"));
    }

    #[test]
    fn an_agent_says_how_hard_it_can_think() {
        let options = json!([
            { "id": "mode", "category": "mode", "type": "select", "currentValue": "ask", "options": [{ "value": "ask", "name": "Ask" }] },
            { "id": "effort", "name": "Thinking", "category": "thought_level", "type": "select", "currentValue": "medium", "options": [
                { "value": "low", "name": "Low" },
                { "group": "more", "name": "More", "options": [{ "value": "medium", "name": "Medium" }, { "value": "high", "name": "High" }] }
            ] }
        ]);
        let levels = thought_levels(&options).unwrap();
        assert_eq!(levels.id, "effort");
        assert_eq!(levels.options.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>(), ["low", "medium", "high"]);
        assert_eq!(levels.current.as_deref(), Some("medium"));
        assert_eq!(thought_levels(&json!([{ "id": "mode", "category": "mode", "options": [] }])), None);
        assert_eq!(thought_levels(&Value::Null), None);
    }

    #[test]
    fn a_new_session_gets_instructions_and_the_conversation() {
        let turn = AcpTurn {
            engine: crate::engines::find("grok-cli").unwrap(),
            program: PathBuf::from("grok"),
            model: String::new(),
            session: None,
            instructions: "You are Scout.".into(),
            earlier: vec![(true, "Plant carrots?".into()), (false, "Yes, in spring.".into())],
            text: "And beans?".into(),
            workspace: Workspace { folder: PathBuf::from("/tmp"), roots: vec![PathBuf::from("/tmp")], mode: PermissionMode::Default, catbot: None },
            mcp: Vec::new(),
            effort: None,
            env: Vec::new(),
        };
        let prompt = first_prompt(&turn);
        assert!(prompt.starts_with("<catbot_instructions>\nYou are Scout."));
        assert!(prompt.contains("Person: Plant carrots?") && prompt.contains("You: Yes, in spring."));
        assert!(prompt.ends_with("And beans?"));
    }

    #[test]
    fn tool_calls_are_shown_like_claudes() {
        let (name, input) = shown(&json!({ "title": "Run ls", "kind": "execute", "rawInput": { "command": "ls" } }));
        assert_eq!((name.as_str(), input["command"].as_str()), ("Bash", Some("ls")));
        let (name, input) = shown(&json!({ "title": "Edit a.md", "kind": "edit", "locations": [{ "path": "/x/a.md" }] }));
        assert_eq!((name.as_str(), input["file_path"].as_str()), ("Edit", Some("/x/a.md")));
        assert_eq!(output_of(&json!({ "content": [{ "type": "content", "content": { "type": "text", "text": "done" } }] })), "done");
    }

    #[test]
    fn a_device_code_is_read_from_the_link_or_the_words() {
        assert_eq!(device_code("https://auth.meta.com/oauth/device/?code=WVCW-VRLT", "").as_deref(), Some("WVCW-VRLT"));
        assert_eq!(device_code("https://example.com/device", "Open the link, and enter the code AB12-CD34 there.").as_deref(), Some("AB12-CD34"));
        assert_eq!(device_code("https://example.com/device", "Open the link."), None);
    }

    #[test]
    fn failures_say_how_to_sign_in() {
        let grok = crate::engines::find("grok-cli").unwrap();
        let said = explain(grok, "the agent stopped", "Error: not logged in");
        assert!(said.contains("It said: Error: not logged in") && said.contains("grok login"), "{said}");
    }
}
