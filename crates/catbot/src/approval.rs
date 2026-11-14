//! Approvals: the bot asks before it acts.
//!
//! Claude Code is started with `--permission-prompt-tool mcp__catbot__approve`. Whenever the
//! bot wants to do something that needs a yes (run a command, edit a file), Claude Code calls
//! that tool and waits. The tool is served by a small helper, `catbot approval-mcp <socket>`,
//! which Claude Code starts as an MCP server. The helper relays each question over a private
//! Unix socket to the [`Broker`] in the turn that owns the bot, which raises an
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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
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
        let socket = dir.join("approve.sock");
        let listener = UnixListener::bind(&socket)?;

        let (program, leading) = helper
            .split_first()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no approval helper given"))?;
        let mut helper_args: Vec<String> = leading.to_vec();
        helper_args.extend(["approval-mcp".to_string(), socket.to_string_lossy().into_owned()]);
        let config = json!({ "mcpServers": { SERVER_NAME: { "command": program, "args": helper_args } } });
        let config_path = dir.join("mcp.json");
        write_private(&config_path, config.to_string().as_bytes())?;

        let pending: Pending = Arc::default();
        let task = tokio::spawn(accept(listener, pending.clone(), events));
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

fn answer_line(id: &str, decision: &Decision) -> String {
    let answer = match decision {
        Decision::Allow => json!({ "t": "answer", "id": id, "behavior": "allow" }),
        Decision::Deny(message) => json!({ "t": "answer", "id": id, "behavior": "deny", "message": message }),
    };
    format!("{answer}\n")
}

/// A folder only this user can open, for the socket and the config.
fn private_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("catbot-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(bytes)
}

/// Accepts the helper's connections (normally one per turn, again after a drop).
async fn accept(listener: UnixListener, pending: Pending, events: mpsc::WeakSender<Event>) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(connection(stream, pending.clone(), events.clone()));
    }
}

async fn connection(stream: UnixStream, pending: Pending, events: mpsc::WeakSender<Event>) {
    let (read, mut write) = stream.into_split();
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
    write: tokio::net::unix::OwnedWriteHalf,
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    alive: bool,
}

impl BrokerLink {
    async fn connect(socket: &Path) -> Option<Self> {
        // A few quick tries ride out a broker that is still starting.
        for pause in [0, 200, 800] {
            tokio::time::sleep(std::time::Duration::from_millis(pause)).await;
            if let Ok(stream) = UnixStream::connect(socket).await {
                let (read, write) = stream.into_split();
                return Some(Self { write, lines: BufReader::new(read).lines(), alive: true });
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
            return if answer["behavior"] == "allow" {
                Decision::Allow
            } else {
                let said = answer["message"].as_str().filter(|m| !m.trim().is_empty()).unwrap_or("The person refused this step.");
                let stop = if said.ends_with(['.', '!', '?']) { "" } else { "." };
                Decision::Deny(format!("{said}{stop}{REFUSED_NEXT_STEP}"))
            };
        }
        self.alive = false;
        Decision::Deny(UNREACHABLE.into())
    }
}
