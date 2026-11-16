//! Approvals: the bot asks before it acts.
//!
//! Claude Code is started with `--permission-prompt-tool mcp__catbot__approve`. Whenever the
//! bot wants to do something that needs a yes (run a command, edit a file), Claude Code calls
//! that tool and waits. The tool is served by a small helper, `catbot approval-mcp <socket>`,
//! which Claude Code starts as an MCP server. The helper relays each question over a private
//! channel (a Unix socket; on Windows, a local TCP port behind a one-time token) to the [`Broker`] in the turn that owns the bot, which raises an
//! [`Event::ApprovalRequested`](crate::claude::Event) and waits for [`Decision`].
//!
//! ```text
//! claude ──MCP──▶ catbot approval-mcp ──socket──▶ Broker ──event──▶ person
//!   ▲                                               │
//!   └──────────── allow / deny ◀────────────────────┘
//! ```
//!
//! Anything that goes wrong on the way (the helper can't reach the broker, nobody is
//! listening) ends in a deny, never an allow.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};

use crate::claude::Event;

/// The MCP server's name in Claude Code's config; tools appear as `mcp__catbot__…`.
pub const SERVER_NAME: &str = "catbot";
/// The tool Claude Code calls for a yes or no.
pub const PROMPT_TOOL: &str = "mcp__catbot__approve";

/// The person's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    /// Refused, with what the person said (passed on to the bot).
    Deny(String),
}

/// What the bot is told after a refusal, so it explains instead of looking for a way around.
pub const REFUSED_NEXT_STEP: &str = " Do not try another way to do the same thing. Say in your reply what you wanted to do and why, and ask how the person would like to proceed.";

/// What the bot is told when nobody could be asked.
const UNREACHABLE: &str = "Catbot could not ask the person to approve this step. Skip it, and say in your reply what you wanted to do and why, so the person can decide.";

type Pending = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>;

/// The turn's end of the approval channel: a private folder holding the socket and the MCP
/// config that points Claude Code at the helper. Dropping it closes the socket and removes the folder.
pub struct Broker {
    dir: PathBuf,
    pub config_path: PathBuf,
    pending: Pending,
    task: JoinHandle<()>,
}

impl Broker {
    /// Opens the socket and writes the MCP config. `helper` is the program (and leading
    /// arguments) that runs `catbot`; questions arrive as events on `events`.
    pub fn start(helper: &[String], events: mpsc::WeakSender<Event>) -> std::io::Result<Self> {
        let dir = private_dir()?;
        let (socket, listener) = door::open(&dir)?;

        let (program, leading) = helper
            .split_first()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no approval helper given"))?;
        let mut helper_args: Vec<String> = leading.to_vec();
        helper_args.extend(["approval-mcp".to_string(), socket.to_string_lossy().into_owned()]);
        let config = json!({ "mcpServers": { SERVER_NAME: { "command": program, "args": helper_args } } });
        let config_path = dir.join("mcp.json");
        write_private(&config_path, config.to_string().as_bytes())?;

        let pending: Pending = Arc::default();
        let task = tokio::spawn(door::accept(listener, pending.clone(), events));
        Ok(Self { dir, config_path, pending, task })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The extra command-line arguments that route Claude Code's questions here.
    pub fn args(&self) -> Vec<String> {
        vec![
            "--mcp-config".into(),
            self.config_path.to_string_lossy().into_owned(),
            "--permission-prompt-tool".into(),
            PROMPT_TOOL.into(),
            "--allowedTools".into(),
            format!("mcp__{SERVER_NAME}"),
        ]
    }

    /// Answers a question. Returns false if it was already answered or its turn is gone.
    pub fn answer(&self, id: &str, decision: Decision) -> bool {
        self.answerer().answer(id, decision)
    }

    /// A handle that can answer this turn's questions from anywhere (a web request, say).
    pub fn answerer(&self) -> Answerer {
        Answerer { pending: self.pending.clone() }
    }
}

/// Answers a turn's approval questions. Cheap to clone; outlives nothing it shouldn't:
/// once the turn is over, answering simply returns false.
#[derive(Clone)]
pub struct Answerer {
    pending: Pending,
}

impl Answerer {
    /// An answerer for questions Catbot asks itself (the chat APIs' tools, `api.rs`), with no
    /// helper or socket between.
    pub fn local() -> Self {
        Self { pending: Arc::default() }
    }

    /// Opens a question: from now on `answer(id, …)` reaches the returned wait, which ends in a
    /// deny if the question is dropped unanswered.
    pub fn expect(&self, id: &str) -> impl std::future::Future<Output = Decision> + Send + 'static {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        self.pending.lock().expect("pending lock").insert(id.to_string(), tx);
        async move {
            match rx.recv().await.and_then(|line| serde_json::from_str::<Value>(&line).ok()) {
                Some(answer) => decision_from(&answer),
                None => Decision::Deny(UNREACHABLE.into()),
            }
        }
    }

    pub fn answer(&self, id: &str, decision: Decision) -> bool {
        let Some(reply) = self.pending.lock().expect("pending lock").remove(id) else { return false };
        reply.send(answer_line(id, &decision)).is_ok()
    }

    /// Whether this question is still waiting for an answer.
    pub fn is_open(&self, id: &str) -> bool {
        self.pending.lock().expect("pending lock").contains_key(id)
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The decision an answer line carries, with the words that keep a refused bot from trying
/// another way.
fn decision_from(answer: &Value) -> Decision {
    if answer["behavior"] == "allow" {
        Decision::Allow
    } else {
        let said = answer["message"].as_str().filter(|m| !m.trim().is_empty()).unwrap_or("The person refused this step.");
        let stop = if said.ends_with(['.', '!', '?']) { "" } else { "." };
        Decision::Deny(format!("{said}{stop}{REFUSED_NEXT_STEP}"))
    }
}

fn answer_line(id: &str, decision: &Decision) -> String {
    let answer = match decision {
        Decision::Allow => json!({ "t": "answer", "id": id, "behavior": "allow" }),
        Decision::Deny(message) => json!({ "t": "answer", "id": id, "behavior": "deny", "message": message }),
    };
    format!("{answer}\n")
}

/// A folder only this user can open, for the socket and the config. (On Windows, the user's
/// own temp folder is already closed to other users.)
fn private_dir() -> std::io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("catbot-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(dir)
    }
    #[cfg(not(unix))]
    {
        let dir = std::env::temp_dir().join(format!("catbot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        Ok(dir)
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// The channel between the broker and the helper. `open` gives the helper a path to find it by.
#[cfg(unix)]
mod door {
    use super::*;
    use tokio::net::{UnixListener, UnixStream};

    pub fn open(dir: &Path) -> std::io::Result<(PathBuf, UnixListener)> {
        let socket = dir.join("approve.sock");
        let listener = UnixListener::bind(&socket)?;
        Ok((socket, listener))
    }

    /// Accepts the helper's connections (normally one per turn, again after a drop).
    pub async fn accept(listener: UnixListener, pending: Pending, events: mpsc::WeakSender<Event>) {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(connection(stream, pending.clone(), events.clone()));
        }
    }

    pub async fn connect(socket: &Path) -> Option<UnixStream> {
        UnixStream::connect(socket).await.ok()
    }
}

/// On Windows: a TCP port on 127.0.0.1, written with a one-time token into a private file; a
/// connection that doesn't start with the token is dropped.
#[cfg(not(unix))]
mod door {
    use super::*;
    use tokio::{
        io::AsyncReadExt,
        net::{TcpListener, TcpStream},
    };

    pub struct Listener {
        listener: TcpListener,
        token: String,
    }

    pub fn open(dir: &Path) -> std::io::Result<(PathBuf, Listener)> {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        std_listener.set_nonblocking(true)?;
        let addr = std_listener.local_addr()?;
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let file = dir.join("approve.door");
        write_private(&file, format!("{addr}\n{token}\n").as_bytes())?;
        Ok((file, Listener { listener: TcpListener::from_std(std_listener)?, token }))
    }

    pub async fn accept(door: Listener, pending: Pending, events: mpsc::WeakSender<Event>) {
        while let Ok((mut stream, _)) = door.listener.accept().await {
            let (token, pending, events) = (door.token.clone(), pending.clone(), events.clone());
            tokio::spawn(async move {
                let mut said = vec![0u8; token.len() + 1];
                let knows = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read_exact(&mut said)).await;
                if matches!(knows, Ok(Ok(_))) && said[..token.len()] == *token.as_bytes() && said[token.len()] == b'\n' {
                    connection(stream, pending, events).await;
                }
            });
        }
    }

    pub async fn connect(file: &Path) -> Option<TcpStream> {
        let text = std::fs::read_to_string(file).ok()?;
        let mut lines = text.lines();
        let (addr, token) = (lines.next()?, lines.next()?);
        let mut stream = TcpStream::connect(addr).await.ok()?;
        stream.write_all(format!("{token}\n").as_bytes()).await.ok()?;
        Some(stream)
    }
}

async fn connection<S: AsyncRead + AsyncWrite + Send + 'static>(stream: S, pending: Pending, events: mpsc::WeakSender<Event>) {
    let (read, mut write) = tokio::io::split(stream);
    let (out, mut outgoing) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = outgoing.recv().await {
            if write.write_all(line.as_bytes()).await.is_err() {
                return;
            }
        }
    });
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(ask) = serde_json::from_str::<Value>(&line) else { continue };
        if ask["t"] != "ask" {
            continue;
        }
        let Some(id) = ask["id"].as_str().map(str::to_string) else { continue };
        let tool = ask["tool"].as_str().unwrap_or("tool").to_string();
        pending.lock().expect("pending lock").insert(id.clone(), out.clone());
        let event = Event::ApprovalRequested { id: id.clone(), tool, input: ask["input"].clone() };
        // Weak, so a finished turn's channel closes even while the helper is still connected.
        let delivered = match events.upgrade() {
            Some(events) => events.send(event).await.is_ok(),
            None => false,
        };
        if !delivered {
            // Nobody is watching this turn any more: refuse rather than hang.
            pending.lock().expect("pending lock").remove(&id);
            let _ = out.send(answer_line(&id, &Decision::Deny(UNREACHABLE.into())));
        }
    }
    drop(out);
    let _ = writer.await;
}

/// `catbot approval-mcp <socket>`: the MCP server Claude Code starts for a turn. Speaks
/// JSON-RPC on stdin/stdout, and relays each `approve` call to the broker on `socket`.
pub async fn serve_mcp(socket: &Path) -> std::io::Result<()> {
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut broker: Option<BrokerLink> = None;
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let id = msg["id"].clone();
        let method = msg["method"].as_str().unwrap_or("");
        let reply = match method {
            "initialize" => Some(json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "catbot-approvals", "version": env!("CARGO_PKG_VERSION") },
            })),
            "tools/list" => Some(json!({ "tools": [{
                "name": "approve",
                "description": "Ask the person who owns this bot whether a tool use is allowed",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": { "type": "string" },
                        "input": { "type": "object" },
                        "tool_use_id": { "type": "string" },
                    },
                    "required": ["tool_name", "input"],
                },
            }] })),
            "tools/call" if msg["params"]["name"] == "approve" => {
                let args = &msg["params"]["arguments"];
                let tool = args["tool_name"].as_str().unwrap_or("tool");
                let input = if args["input"].is_object() { args["input"].clone() } else { json!({}) };
                let decision = match broker.as_mut() {
                    Some(link) => link.ask(tool, &input).await,
                    None => match BrokerLink::connect(socket).await {
                        Some(mut link) => {
                            let decision = link.ask(tool, &input).await;
                            broker = Some(link);
                            decision
                        }
                        None => Decision::Deny(UNREACHABLE.into()),
                    },
                };
                if matches!(decision, Decision::Deny(_)) {
                    broker = broker.filter(|link| link.alive);
                }
                let text = match decision {
                    Decision::Allow => json!({ "behavior": "allow", "updatedInput": input }),
                    Decision::Deny(message) => json!({ "behavior": "deny", "message": message }),
                };
                Some(json!({ "content": [{ "type": "text", "text": text.to_string() }] }))
            }
            _ if method.starts_with("notifications/") => None,
            _ if id.is_null() => None,
            _ => {
                let error = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("method not found: {method}") } });
                stdout.write_all(format!("{error}\n").as_bytes()).await?;
                stdout.flush().await?;
                None
            }
        };
        if let Some(result) = reply {
            let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            stdout.write_all(format!("{response}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

/// The helper's connection to the broker.
struct BrokerLink {
    write: Box<dyn AsyncWrite + Unpin + Send>,
    lines: tokio::io::Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>,
    alive: bool,
}

impl BrokerLink {
    async fn connect(socket: &Path) -> Option<Self> {
        // A few quick tries ride out a broker that is still starting.
        for pause in [0, 200, 800] {
            tokio::time::sleep(std::time::Duration::from_millis(pause)).await;
            if let Some(stream) = door::connect(socket).await {
                let (read, write) = tokio::io::split(stream);
                let read: Box<dyn AsyncRead + Unpin + Send> = Box::new(read);
                return Some(Self { write: Box::new(write), lines: BufReader::new(read).lines(), alive: true });
            }
        }
        None
    }

    async fn ask(&mut self, tool: &str, input: &Value) -> Decision {
        let id = uuid::Uuid::new_v4().to_string();
        let ask = json!({ "t": "ask", "id": id, "tool": tool, "input": input });
        if self.write.write_all(format!("{ask}\n").as_bytes()).await.is_err() {
            self.alive = false;
            return Decision::Deny(UNREACHABLE.into());
        }
        while let Ok(Some(line)) = self.lines.next_line().await {
            let Ok(answer) = serde_json::from_str::<Value>(&line) else { continue };
            if answer["t"] != "answer" || answer["id"] != id.as_str() {
                continue;
            }
            return decision_from(&answer);
        }
        self.alive = false;
        Decision::Deny(UNREACHABLE.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helper's side and the broker's side meet through the door, on any platform.
    #[tokio::test]
    async fn a_question_crosses_the_door_and_its_answer_comes_back() {
        let dir = private_dir().unwrap();
        let (path, listener) = door::open(&dir).unwrap();
        let (events_tx, mut events) = mpsc::channel(8);
        let pending: Pending = Arc::default();
        let task = tokio::spawn(door::accept(listener, pending.clone(), events_tx.downgrade()));

        let mut link = BrokerLink::connect(&path).await.expect("the helper reaches the broker");
        let asking = tokio::spawn(async move { link.ask("Bash", &json!({ "command": "ls" })).await });
        let Some(Event::ApprovalRequested { id, tool, .. }) = events.recv().await else { panic!("no question arrived") };
        assert_eq!(tool, "Bash");
        assert!(Answerer { pending: pending.clone() }.answer(&id, Decision::Allow));
        assert_eq!(asking.await.unwrap(), Decision::Allow);

        task.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(unix))]
    #[tokio::test]
    async fn a_stranger_without_the_token_is_dropped() {
        use tokio::io::AsyncReadExt;
        let dir = private_dir().unwrap();
        let (path, listener) = door::open(&dir).unwrap();
        let (events_tx, _events) = mpsc::channel(8);
        let task = tokio::spawn(door::accept(listener, Arc::default(), events_tx.downgrade()));
        let addr = std::fs::read_to_string(&path).unwrap().lines().next().unwrap().to_string();
        let mut stranger = tokio::net::TcpStream::connect(&addr).await.unwrap();
        stranger.write_all(format!("{}\n{}\n", "x".repeat(64), json!({ "t": "ask", "id": "1", "tool": "Bash", "input": {} })).as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        let read = tokio::time::timeout(std::time::Duration::from_secs(3), stranger.read_to_end(&mut buf)).await;
        // Closed (or reset) without an answer, well before the timeout.
        assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))), "the connection is closed without an answer: {read:?}");
        task.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
