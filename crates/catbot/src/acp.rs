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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot},
};

use crate::approval::{Answerer, Decision};
use crate::claude::{AbortOnDrop, Event, Outcome, PermissionMode, Running, read_stderr_tail, truncate_output};
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
    // Grok takes words mid-turn as a second prompt on the running session (stage 32.11), once its
    // own config says to steer.
    let steers = turn.engine.id == crate::engines::GROK_CLI;
    if steers {
        grok::ensure_steer();
    }
    let slot: Slot = Arc::default();
    let task = tokio::spawn(run(turn, child, tx, answers.clone(), slot.clone()));
    let running = Running::in_process(events, task, answers);
    Ok(if steers { running.with_steer(grok::steer(slot)) } else { running })
}

/// The running turn's link and session, once it has one: where a steer goes.
type Slot = Arc<Mutex<Option<(Link, String)>>>;

/// Grok's steering (stage 32.11, after OpenMausBot 0.1.175's `ensureGrokSteer`): with
/// `[ui] follow_up_behavior = "steer"` in Grok's `config.toml`, a second `session/prompt` is
/// taken into the running turn at its next step; its own answer is a "removedFromQueue" cancel.
/// Catbot adds the line for a signed-in Grok, never over a choice the person made ("queue"
/// included); with "queue", a message waits for the turn to end as before.
pub mod grok {
    use super::*;

    /// Grok's own folder: `GROK_HOME`, else `~/.grok`.
    pub fn home() -> PathBuf {
        std::env::var_os("GROK_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| crate::store::user_home().join(".grok"))
    }

    /// What `follow_up_behavior` is set to under `[ui]`, if it's set.
    pub fn follow_up(config: &str) -> Option<String> {
        let mut in_ui = false;
        for line in config.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_ui = line == "[ui]";
                continue;
            }
            if in_ui && let Some((key, value)) = line.split_once('=') && key.trim() == "follow_up_behavior" {
                return Some(value.split('#').next().unwrap_or("").trim().trim_matches(['"', '\'']).to_string());
            }
        }
        None
    }

    /// The config with steering turned on, or None to leave it as it is (set already, by Catbot
    /// or the person).
    pub fn with_steer(config: &str) -> Option<String> {
        if follow_up(config).is_some() || config.lines().any(|l| l.trim_start().starts_with("follow_up_behavior")) {
            return None;
        }
        let eol = if config.contains("\r\n") { "\r\n" } else { "\n" };
        let line = format!("follow_up_behavior = \"steer\"{eol}");
        let lines: Vec<&str> = config.split_inclusive('\n').collect();
        if let Some(at) = lines.iter().position(|l| l.trim() == "[ui]") {
            let mut out: String = lines[..=at].concat();
            if !out.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(&line);
            out.push_str(&lines[at + 1..].concat());
            return Some(out);
        }
        let mut out = config.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        if !out.is_empty() {
            out.push_str(eol);
        }
        out.push_str(&format!("[ui]{eol}{line}"));
        Some(out)
    }

    /// Turns steering on in Grok's config, for a signed-in Grok, when nothing is set yet.
    pub fn ensure_steer() {
        let home = home();
        if !home.join("auth.json").is_file() {
            return;
        }
        let path = home.join("config.toml");
        let config = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => return,
        };
        let Some(next) = with_steer(&config) else { return };
        // It may hold keys: private from its first byte, then moved into place.
        let staged = home.join(format!("config.toml.catbot-{}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = options.open(&staged).and_then(|mut f| std::io::Write::write_all(&mut f, next.as_bytes())).and_then(|_| std::fs::rename(&staged, &path));
        if written.is_err() {
            let _ = std::fs::remove_file(&staged);
        } else {
            eprintln!("catbot: turned on steering in Grok's config ({})", path.display());
        }
    }

    /// Whether Grok takes words mid-turn now.
    pub fn steers() -> bool {
        std::fs::read_to_string(home().join("config.toml")).ok().and_then(|c| follow_up(&c)).is_some_and(|v| v == "steer")
    }

    /// Steers the turn: a second prompt on its session, once it has one.
    pub(super) fn steer(slot: Slot) -> crate::claude::Steer {
        crate::claude::Steer::new(move |text: String| {
            let slot = slot.clone();
            async move {
                if !steers() {
                    return Err("Grok's settings have it wait for the turn to end".to_string());
                }
                let Some((link, session)) = slot.lock().expect("slot lock").clone() else { return Err("the turn hasn't started yet".to_string()) };
                link.call("session/prompt", json!({ "sessionId": session, "prompt": [{ "type": "text", "text": text }] }), Some(Duration::from_secs(600))).await.map(|_| ())
            }
        })
    }
}

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// The JSON-RPC link to the agent.
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
        let _guard = WaitGuard { waiting: self.waiting.clone(), id };
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
    /// Whether the prompt has gone. Before then, updates are the agent replaying the session as
    /// it loads (ACP's session/load does, and Grok CLI did), not this turn: they're left out.
    listening: bool,
    /// Something other than reply text came since the last text (a tool call, thinking): the
    /// next text starts a new paragraph.
    gap: bool,
}

async fn run(turn: AcpTurn, mut child: Child, tx: mpsc::Sender<Event>, answers: Answerer, slot: Slot) {
    let started = std::time::Instant::now();
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let mut stderr_tail = AbortOnDrop(tokio::spawn(read_stderr_tail(stderr, 1500)));
    let link = Link { stdin: Arc::new(tokio::sync::Mutex::new(stdin)), waiting: Arc::default(), next: Arc::default() };
    let heard = Arc::new(Mutex::new(Heard::default()));
    let reader = AbortOnDrop(tokio::spawn(read(stdout, link.clone(), tx.clone(), answers, heard.clone(), turn.workspace.clone())));

    let result = converse(&turn, &link, &tx, &heard, &slot).await;
    *slot.lock().expect("slot lock") = None;
    let text = std::mem::take(&mut heard.lock().expect("heard lock").text);
    if !text.trim().is_empty() {
        let _ = tx.send(Event::Text { text: text.clone() }).await;
    }
    let outcome = match result {
        Ok((session, stop, sent)) => {
            let ok = stop != "refusal";
            if !ok {
                let _ = tx.send(Event::Error { message: format!("{} declined to answer.", turn.engine.name) }).await;
            }
            // Agent programs don't say what they used: a guess from what went in and came out.
            let tokens = Some(crate::claude::Tokens::guess(sent, text.len()));
            Outcome { ok, session_id: Some(session), result: Some(text), stop_reason: Some(stop), tokens, ..Outcome::default() }
        }
        Err(problem) => {
            let _ = child.start_kill();
            let tail = tokio::time::timeout(Duration::from_secs(2), &mut stderr_tail.0).await.ok().and_then(|r| r.ok()).unwrap_or_default();
            let message = explain(turn.engine, &problem, &tail);
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

/// The handshake, the session and the prompt. Returns `(session_id, stop_reason, prompt_bytes)`.
async fn converse(turn: &AcpTurn, link: &Link, tx: &mpsc::Sender<Event>, heard: &Mutex<Heard>, slot: &Slot) -> Result<(String, String, usize), String> {
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
    let mut opened = Value::Null;
    if let (true, Some(id)) = (can_load, turn.session.as_ref())
        && let Ok(loaded) = link.call("session/load", json!({ "sessionId": id, "cwd": cwd, "mcpServers": turn.mcp }), Some(START_TIME)).await
    {
        session = Some(id.clone());
        opened = loaded;
    }
    let prompt = match &session {
        Some(_) => turn.text.clone(),
        None => first_prompt(turn),
    };
    let session = match session {
        Some(id) => id,
        None => {
            let made = match link.call("session/new", json!({ "cwd": cwd, "mcpServers": turn.mcp }), Some(START_TIME)).await {
                Ok(made) => made,
                // Catbot's tools wouldn't start in the agent: it goes on without them, and the
                // chat says so (stage 32.7).
                Err(why) if !turn.mcp.is_empty() => {
                    eprintln!("catbot: {} wouldn't start with Catbot's tools ({why}); going on without them", turn.engine.name);
                    let names: Vec<String> = turn.mcp.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect();
                    let _ = tx.send(Event::Notice { text: crate::claude::tools_missing(&names) }).await;
                    link.call("session/new", json!({ "cwd": cwd, "mcpServers": [] }), Some(START_TIME)).await?
                }
                Err(why) => return Err(why),
            };
            let id = made["sessionId"].as_str().map(str::to_string).ok_or("the agent made no session")?;
            opened = made;
            id
        }
    };
    let options = opened["configOptions"].clone();
    // Its models, if it says (Antigravity takes no model on its command line, so this is the
    // only way to pick one, Moo's ask on 2026-11-27): kept for the model menu, the bot's picked.
    if let Some(models) = agent_models(&opened) {
        remember_models(turn.engine, &models);
        let wanted = turn.model.trim();
        if !wanted.is_empty() && models.current.as_deref() != Some(wanted) && models.options.iter().any(|(id, _)| id == wanted) {
            let set = match &models.via {
                ModelVia::Option(config) => link.call("session/set_config_option", json!({ "sessionId": session, "configId": config, "value": wanted }), Some(START_TIME)).await,
                ModelVia::SetModel => link.call("session/set_model", json!({ "sessionId": session, "modelId": wanted }), Some(START_TIME)).await,
            };
            if let Err(problem) = set {
                eprintln!("catbot: {} didn't take the model {wanted}: {problem}", turn.engine.name);
            }
        }
    }
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
    // The replay (if any) came before session/load's answer, which the reader handles in order.
    heard.lock().expect("heard lock").listening = true;
    *slot.lock().expect("slot lock") = Some((link.clone(), session.clone()));
    let sent = prompt.len();
    let answer = link.call("session/prompt", json!({ "sessionId": session, "prompt": [{ "type": "text", "text": prompt }] }), None).await?;
    let stop = answer["stopReason"].as_str().unwrap_or("end_turn").to_string();
    Ok((session, stop, sent))
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

/// How an agent takes a model: through a config option of category `model`, or ACP's
/// `session/set_model`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ModelVia {
    Option(String),
    SetModel,
}

/// The models an agent offers a session: their ids and names, and the one it's on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AgentModels {
    pub options: Vec<(String, String)>,
    pub current: Option<String>,
    pub via: ModelVia,
}

/// The models in a session's answer (session/new or session/load): a config option of category
/// `model`, or the `models` it lists (`availableModels`, `currentModelId`).
pub fn agent_models(opened: &Value) -> Option<AgentModels> {
    if let Some(option) = opened["configOptions"].as_array().and_then(|o| o.iter().find(|o| o["category"] == "model")) {
        let id = option["id"].as_str().or(option["configId"].as_str())?.to_string();
        let mut values = Vec::new();
        for entry in option["options"].as_array()? {
            for value in entry["options"].as_array().cloned().unwrap_or_else(|| vec![entry.clone()]) {
                if let Some(v) = value["value"].as_str() {
                    values.push((v.to_string(), value["name"].as_str().unwrap_or(v).to_string()));
                }
            }
        }
        return (!values.is_empty()).then(|| AgentModels { options: values, current: option["currentValue"].as_str().map(str::to_string), via: ModelVia::Option(id) });
    }
    let listed = opened["models"]["availableModels"].as_array()?;
    let options: Vec<(String, String)> = listed
        .iter()
        .filter_map(|m| {
            let id = m["modelId"].as_str()?;
            Some((id.to_string(), m["name"].as_str().unwrap_or(id).to_string()))
        })
        .collect();
    (!options.is_empty()).then(|| AgentModels { options, current: opened["models"]["currentModelId"].as_str().map(str::to_string), via: ModelVia::SetModel })
}

/// The models seen since Catbot started, by engine, as `SEEN` for thinking levels.
static SEEN_MODELS: std::sync::Mutex<Option<std::collections::HashMap<String, AgentModels>>> = std::sync::Mutex::new(None);

fn models_file(engine: &Engine) -> Option<PathBuf> {
    crate::engines::agents_dir().map(|dir| dir.join(engine.id).join("models.json"))
}

fn remember_models(engine: &Engine, models: &AgentModels) {
    SEEN_MODELS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(engine.id.to_string(), models.clone());
    let Some(file) = models_file(engine) else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(models) {
        let _ = std::fs::write(file, text);
    }
}

/// The models this agent offered last time it ran. Unknown until a bot has used it once.
pub fn known_models(engine: &Engine) -> Option<AgentModels> {
    if let Some(seen) = SEEN_MODELS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|s| s.get(engine.id)) {
        return Some(seen.clone());
    }
    serde_json::from_str(&std::fs::read_to_string(models_file(engine)?).ok()?).ok()
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
    if !heard.lock().expect("heard lock").listening {
        return;
    }
    match update["sessionUpdate"].as_str().unwrap_or_default() {
        "agent_message_chunk" => {
            if let Some(text) = update["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                let text = {
                    let mut heard = heard.lock().expect("heard lock");
                    let text = if std::mem::take(&mut heard.gap) && !heard.text.trim().is_empty() && !heard.text.ends_with("\n\n") {
                        format!("\n\n{}", text.trim_start())
                    } else {
                        text.to_string()
                    };
                    heard.text.push_str(&text);
                    text
                };
                let _ = tx.send(Event::TextDelta { text }).await;
            }
        }
        "agent_thought_chunk" => {
            if let Some(text) = update["content"]["text"].as_str().filter(|t| !t.is_empty()) {
                heard.lock().expect("heard lock").gap = true;
                let _ = tx.send(Event::ThinkingDelta { text: text.to_string() }).await;
            }
        }
        "tool_call" => {
            let id = update["toolCallId"].as_str().unwrap_or("tool").to_string();
            let (name, input) = shown(update);
            {
                let mut heard = heard.lock().expect("heard lock");
                heard.open_tools.push(id.clone());
                heard.gap = true;
            }
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
    truncate_output(out.trim(), 4000)
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
                if tx.send(Event::ApprovalRequested { id: question.clone(), tool, input }).await.is_err() {
                    answers.cancel(&question);
                    Decision::Deny("turn ended".into())
                } else {
                    waiting.await
                }
            };
            let outcome = match (decision, allow, reject) {
                (Decision::Allow, Some(option), _) => json!({ "outcome": "selected", "optionId": option }),
                (Decision::Deny(_), _, Some(option)) => json!({ "outcome": "selected", "optionId": option }),
                _ => json!({ "outcome": "cancelled" }),
            };
            link.reply(id, json!({ "outcome": outcome })).await;
        }
        "fs/read_text_file" => match file_of(params, workspace) {
            Ok(path) => {
                const MAX_ACP_READ_BYTES: u64 = 10 * 1024 * 1024;
                match tokio::fs::metadata(&path).await {
                    Ok(meta) if meta.len() > MAX_ACP_READ_BYTES => {
                        link.refuse(id, -32602, &format!("{} is too large to read as text ({} bytes)", path.display(), meta.len())).await;
                    }
                    Err(err) => link.refuse(id, -32602, &format!("couldn't read {}: {err}", path.display())).await,
                    Ok(_) => match tokio::fs::read_to_string(&path).await {
                        Ok(content) => {
                            let line = params["line"].as_u64().unwrap_or(1).max(1).min(usize::MAX as u64) as usize;
                            let content = match params["limit"].as_u64() {
                                None if line == 1 => content,
                                limit => {
                                    let lines: Vec<&str> = content.split('\n').collect();
                                    let start = (line - 1).min(lines.len());
                                    let end = limit
                                        .map(|l| start.saturating_add(l.min(usize::MAX as u64) as usize).min(lines.len()))
                                        .unwrap_or(lines.len());
                                    lines[start..end].join("\n")
                                }
                            };
                            link.reply(id, json!({ "content": content })).await;
                        }
                        Err(err) => link.refuse(id, -32602, &format!("couldn't read {}: {err}", path.display())).await,
                    },
                }
            }
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
    let reader = AbortOnDrop(tokio::spawn(async move {
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
    }));
    // The link, from its output. Never shown otherwise: the lines after can carry a code.
    let stderr = child.stderr.take().expect("stderr is piped");
    let watcher = AbortOnDrop(tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Antigravity's line; Gemini CLI prints Google's page among its own words (stage 29.7).
            let printed = line.trim().strip_prefix(LINK_PREFIX).map(|u| u.trim().to_string()).filter(|u| u.starts_with("https://"));
            let google = || (engine.id == "gemini").then(|| crate::login::link_in(&line)).flatten().filter(|u| u.starts_with("https://accounts.google.com/"));
            if let Some(url) = printed.or_else(google)
                && let Some(tx) = link_tx.lock().expect("link lock").take()
            {
                let _ = tx.send((url.to_string(), None));
            }
        }
    }));
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
    let (done_tx, mut done_rx) = oneshot::channel::<Result<(), String>>();
    let asking = link.clone();
    let task = tokio::spawn(async move {
        let _reader = reader;
        let _watcher = watcher;
        let answer = asking.call("authenticate", json!({ "methodId": method }), Some(Duration::from_secs(600))).await.map(|_| ());
        let _ = done_tx.send(answer.clone());
        finished(answer);
        let _ = child.start_kill();
    });
    tokio::select! {
        url = link_rx => match url {
            Ok(link) => Ok((Some(link), task)),
            // Both reader and watcher exited without sending a link: wait briefly for `authenticate`'s
            // outcome rather than treating a subprocess crash as "already signed in".
            Err(_) => match tokio::time::timeout(Duration::from_secs(5), &mut done_rx).await {
                Ok(Ok(Ok(()))) => Ok((None, task)),
                Ok(Ok(Err(problem))) => Err(problem),
                _ => {
                    task.abort();
                    Err(format!("{} stopped before giving a sign-in link", engine.name))
                }
            },
        },
        done = &mut done_rx => match done {
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
    static LOCAL_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = LOCAL_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    });
    let answer = client.get(pasted).timeout(Duration::from_secs(30)).send().await.map_err(|_| "The sign-in had already finished or stopped. Start it again.".to_string())?;
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

    #[test]
    fn grok_steering_is_turned_on_only_where_nothing_is_set() {
        assert_eq!(grok::with_steer("").as_deref(), Some("[ui]\nfollow_up_behavior = \"steer\"\n"));
        assert_eq!(grok::with_steer("model = \"grok-4\"").as_deref(), Some("model = \"grok-4\"\n\n[ui]\nfollow_up_behavior = \"steer\"\n"));
        assert_eq!(grok::with_steer("[ui]\ntheme = \"dark\"\n").as_deref(), Some("[ui]\nfollow_up_behavior = \"steer\"\ntheme = \"dark\"\n"));
        assert_eq!(grok::with_steer("[ui]\nfollow_up_behavior = \"queue\"\n"), None, "the person's choice stays");
        assert_eq!(grok::follow_up("[ui]\nfollow_up_behavior = \"steer\" # mid-turn\n").as_deref(), Some("steer"));
        assert_eq!(grok::follow_up("[other]\nfollow_up_behavior = \"steer\"\n"), None);
    }

    #[tokio::test]
    async fn a_pasted_address_goes_only_where_the_link_said() {
        let link = "https://accounts.google.com/o/oauth2/auth?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A9%2Foauth2callback&state=s";
        assert!(deliver_callback(link, "https://evil.example/oauth2callback?code=c").await.unwrap_err().contains("isn't the address"));
        assert!(deliver_callback(link, "http://localhost:9/other?code=c").await.unwrap_err().contains("isn't the address"));
        let away = "https://accounts.google.com/o/oauth2/auth?redirect_uri=https%3A%2F%2Fexample.com%2Fcb";
        assert!(deliver_callback(away, "https://example.com/cb?code=c").await.unwrap_err().contains("isn't the address"));
    }

    #[test]
    fn an_agent_says_which_models_it_has() {
        // As a config option of category `model`, grouped or not.
        let opened = json!({ "configOptions": [{ "id": "model", "category": "model", "type": "select", "currentValue": "gemini-3-pro", "options": [
            { "group": "Gemini", "options": [{ "value": "gemini-3-pro", "name": "Gemini 3 Pro" }, { "value": "gemini-3-flash", "name": "Gemini 3 Flash" }] },
            { "value": "claude-sonnet", "name": "Claude Sonnet" }
        ] }] });
        let models = agent_models(&opened).unwrap();
        assert_eq!(models.options.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["gemini-3-pro", "gemini-3-flash", "claude-sonnet"]);
        assert_eq!((models.current.as_deref(), models.via), (Some("gemini-3-pro"), ModelVia::Option("model".into())));
        // As ACP's `models` list.
        let opened = json!({ "models": { "currentModelId": "a", "availableModels": [{ "modelId": "a", "name": "Model A" }, { "modelId": "b" }] } });
        let models = agent_models(&opened).unwrap();
        assert_eq!(models.options, [("a".to_string(), "Model A".to_string()), ("b".to_string(), "b".to_string())]);
        assert_eq!(models.via, ModelVia::SetModel);
        assert_eq!(agent_models(&json!({ "configOptions": [] })), None);
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
