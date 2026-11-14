//! The Claude driver: runs the `claude` CLI for one turn and turns its stream into [`Event`]s.
//!
//! One process per turn. The conversation continues across turns through Claude's own
//! session id (`--session-id` on the first turn, `--resume` after), so nothing has to stay
//! running between messages.
//!
//! The CLI speaks JSON Lines: we write one user message to its stdin, close stdin, and read
//! events from stdout until the `result` line.

use std::{path::PathBuf, process::Stdio};

use serde::Serialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
};

/// How much the agent may do without asking. With no approval channel yet, anything that
/// would need a "yes" is refused, so `Default` is the safe setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PermissionMode {
    /// Ask for anything risky (refused for now, since nothing can answer yet).
    Default,
    /// Edit files in the working folder without asking.
    AcceptEdits,
    /// Claude decides what is low-risk enough to run.
    Auto,
    /// Plan only, change nothing.
    Plan,
    /// Run everything without asking. Never on your own computer.
    BypassPermissions,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
        }
    }
}

/// Which conversation a turn belongs to.
#[derive(Debug, Clone)]
pub enum Session {
    /// Start a conversation under this id (a UUID we choose, so we know it before Claude answers).
    New(String),
    /// Continue a conversation Claude already has.
    Resume(String),
}

/// Everything one turn needs.
#[derive(Debug, Clone)]
pub struct Turn {
    pub prompt: String,
    pub session: Session,
    pub model: Option<String>,
    pub permission_mode: PermissionMode,
    pub cwd: PathBuf,
    /// Added to Claude Code's own system prompt: the bot's personality and instructions.
    pub system_prompt: Option<String>,
    /// The program to run and any leading arguments: normally just `["claude"]`.
    pub cli: Vec<String>,
    /// Keep the machine's own Claude settings, CLAUDE.md and MCP servers out of the bot.
    pub isolated: bool,
}

/// What a turn reports while it runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The CLI is up: which conversation, which model.
    Started { session_id: String, model: Option<String> },
    /// A piece of the reply, as it is written.
    TextDelta { text: String },
    /// A piece of the model's visible reasoning.
    ThinkingDelta { text: String },
    /// A finished block of reply text (the complete copy of what the deltas spelled out).
    Text { text: String },
    /// A tool call began.
    ToolStarted { id: String, name: String, input: Value },
    /// A tool call ended.
    ToolFinished { id: String, ok: bool, output: String },
    /// Something went wrong that the person should see (signed out, usage limit, API error).
    Error { message: String },
    /// The turn is over.
    Finished(Outcome),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Outcome {
    pub ok: bool,
    pub session_id: Option<String>,
    pub result: Option<String>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub stop_reason: Option<String>,
    /// Tool calls refused because they needed approval.
    pub permission_denials: Vec<Value>,
}

/// The command line for a turn, after the program name.
pub fn args(turn: &Turn) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--verbose", // stream-json output requires it
        "--include-partial-messages",
        "--permission-mode",
        turn.permission_mode.as_str(),
    ]
    .map(String::from)
    .to_vec();
    if turn.isolated {
        // Without these a bot inherits this machine's MCP servers, plugins, hooks and CLAUDE.md.
        args.extend(["--strict-mcp-config", "--setting-sources", "project"].map(String::from));
    }
    if let Some(model) = &turn.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(system) = &turn.system_prompt {
        args.extend(["--append-system-prompt".into(), system.clone()]);
    }
    match &turn.session {
        Session::New(id) => args.extend(["--session-id".into(), id.clone()]),
        Session::Resume(id) => args.extend(["--resume".into(), id.clone()]),
    }
    args
}

/// The one stdin line that carries the person's message.
pub fn user_message(prompt: &str) -> String {
    json!({ "type": "user", "message": { "role": "user", "content": prompt } }).to_string()
}

/// Turns one stdout line into the events it carries (often none: the CLI also reports
/// housekeeping we don't need).
pub fn parse_line(line: &str) -> Vec<Event> {
    let Ok(o) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let s = |v: &Value| v.as_str().map(str::to_string);
    match o["type"].as_str() {
        Some("system") if o["subtype"] == "init" => vec![Event::Started {
            session_id: s(&o["session_id"]).unwrap_or_default(),
            model: s(&o["model"]),
        }],
        Some("stream_event") => {
            // A subagent's narration would interleave with the main reply.
            if !o["parent_tool_use_id"].is_null() {
                return Vec::new();
            }
            let ev = &o["event"];
            if ev["type"] != "content_block_delta" {
                return Vec::new();
            }
            let d = &ev["delta"];
            match d["type"].as_str() {
                Some("text_delta") => s(&d["text"]).filter(|t| !t.is_empty()).map(|text| Event::TextDelta { text }),
                Some("thinking_delta") => {
                    s(&d["thinking"]).filter(|t| !t.is_empty()).map(|text| Event::ThinkingDelta { text })
                }
                _ => None,
            }
            .into_iter()
            .collect()
        }
        Some("assistant") => {
            let content = o["message"]["content"].as_array().cloned().unwrap_or_default();
            let text: String = content.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
            // Signed out, out of usage, or an API failure: the CLI says so as an assistant message.
            if o["is_api_error_message"] == true || o["error"].is_string() {
                let message = if text.trim().is_empty() { s(&o["error"]).unwrap_or_else(|| "API error".into()) } else { text };
                return vec![Event::Error { message }];
            }
            let mut events = Vec::new();
            for block in &content {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = s(&block["text"]).filter(|t| !t.trim().is_empty()) {
                            events.push(Event::Text { text });
                        }
                    }
                    Some("tool_use") => events.push(Event::ToolStarted {
                        id: s(&block["id"]).unwrap_or_default(),
                        name: s(&block["name"]).unwrap_or_default(),
                        input: block["input"].clone(),
                    }),
                    _ => {}
                }
            }
            events
        }
        Some("user") => {
            // With --replay-user-messages the CLI echoes our own message back; it is not news.
            if o["isReplay"] == true {
                return Vec::new();
            }
            let content = o["message"]["content"].as_array().cloned().unwrap_or_default();
            content
                .iter()
                .filter(|b| b["type"] == "tool_result")
                .map(|b| Event::ToolFinished {
                    id: s(&b["tool_use_id"]).unwrap_or_default(),
                    ok: b["is_error"] != true,
                    output: flatten_tool_output(&b["content"]),
                })
                .collect()
        }
        Some("result") => vec![Event::Finished(Outcome {
            ok: o["is_error"] != true,
            session_id: s(&o["session_id"]),
            result: s(&o["result"]),
            cost_usd: o["total_cost_usd"].as_f64(),
            duration_ms: o["duration_ms"].as_u64(),
            stop_reason: s(&o["terminal_reason"]).or_else(|| s(&o["stop_reason"])),
            permission_denials: o["permission_denials"].as_array().cloned().unwrap_or_default(),
        })],
        _ => Vec::new(),
    }
}

/// A tool's result as plain text, cut short: it is for showing, not for keeping.
fn flatten_tool_output(content: &Value) -> String {
    const MAX: usize = 2000;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p["type"].as_str() {
                Some("text") => p["text"].as_str().unwrap_or("").to_string(),
                Some(other) => format!("[{other}]"),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    if text.len() <= MAX {
        return text;
    }
    let mut cut = MAX;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} … [{} more bytes]", &text[..cut], text.len() - cut)
}

/// A turn in progress. Dropping it stops the CLI.
pub struct Running {
    pub events: mpsc::Receiver<Event>,
    child: Option<Child>,
}

impl Running {
    /// Stops the turn now.
    pub async fn stop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill().await;
        }
    }
}

/// Starts a turn. Events arrive on [`Running::events`]; the last one is always [`Event::Finished`].
pub async fn start(turn: &Turn) -> std::io::Result<Running> {
    let (program, leading) = turn
        .cli
        .split_first()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no claude program given"))?;
    let mut child = Command::new(program)
        .args(leading)
        .args(args(turn))
        .current_dir(&turn.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                std::io::Error::new(err.kind(), format!("`{program}` was not found: is Claude Code installed?"))
            } else {
                err
            }
        })?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut line = user_message(&turn.prompt);
    line.push('\n');
    stdin.write_all(line.as_bytes()).await?;
    drop(stdin); // end of input: the CLI answers this one message and exits

    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_tail = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        let text = String::from_utf8_lossy(&buf).trim().to_string();
        let start = text.len().saturating_sub(2000);
        let start = (start..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
        text[start..].to_string()
    });

    let (tx, events) = mpsc::channel(256);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut finished = false;
        while let Ok(Some(line)) = lines.next_line().await {
            for event in parse_line(&line) {
                finished |= matches!(event, Event::Finished(_));
                if tx.send(event).await.is_err() {
                    return; // nobody is listening any more
                }
            }
        }
        if !finished {
            // The CLI ended without a result line: crashed, killed, or refused its arguments.
            let tail = stderr_tail.await.unwrap_or_default();
            let message = if tail.is_empty() { "Claude Code stopped without finishing the turn".to_string() } else { tail };
            let _ = tx.send(Event::Error { message }).await;
            let _ = tx.send(Event::Finished(Outcome { ok: false, ..Outcome::default() })).await;
        }
    });

    Ok(Running { events, child: Some(child) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> Turn {
        Turn {
            prompt: "hi".into(),
            session: Session::New("11111111-2222-4333-8444-555555555555".into()),
            model: Some("sonnet".into()),
            permission_mode: PermissionMode::Default,
            cwd: ".".into(),
            system_prompt: Some("You are Assistant.".into()),
            cli: vec!["claude".into()],
            isolated: true,
        }
    }

    #[test]
    fn command_line_matches_the_reference_driver() {
        let a = args(&turn()).join(" ");
        assert!(a.starts_with("-p --output-format stream-json --input-format stream-json --verbose"));
        assert!(a.contains("--permission-mode default"));
        assert!(a.contains("--strict-mcp-config --setting-sources project"));
        assert!(a.contains("--model sonnet"));
        assert!(a.contains("--append-system-prompt You are Assistant."));
        assert!(a.ends_with("--session-id 11111111-2222-4333-8444-555555555555"));

        let mut resumed = turn();
        resumed.session = Session::Resume("abc".into());
        resumed.isolated = false;
        let a = args(&resumed).join(" ");
        assert!(a.ends_with("--resume abc"));
        assert!(!a.contains("--strict-mcp-config"));
    }

    #[test]
    fn user_message_is_one_json_line() {
        let line = user_message("say \"hi\"\nplease");
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["message"]["content"], "say \"hi\"\nplease");
    }

    #[test]
    fn init_and_deltas() {
        assert_eq!(
            parse_line(r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-sonnet-5"}"#),
            vec![Event::Started { session_id: "s1".into(), model: Some("claude-sonnet-5".into()) }]
        );
        assert_eq!(
            parse_line(r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}"#),
            vec![Event::TextDelta { text: "Hel".into() }]
        );
        // A subagent's narration is dropped.
        assert!(parse_line(r#"{"type":"stream_event","parent_tool_use_id":"t1","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"x"}}}"#).is_empty());
        // Housekeeping lines carry nothing.
        assert!(parse_line(r#"{"type":"autocompact_state","value":{}}"#).is_empty());
        assert!(parse_line("not json").is_empty());
    }

    #[test]
    fn tools_start_and_finish() {
        let started = parse_line(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#);
        assert_eq!(started[0], Event::Text { text: "Looking.".into() });
        assert_eq!(started[1], Event::ToolStarted { id: "t1".into(), name: "Bash".into(), input: json!({"command":"ls"}) });
        let finished = parse_line(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"denied"}]}]}}"#);
        assert_eq!(finished, vec![Event::ToolFinished { id: "t1".into(), ok: false, output: "denied".into() }]);
        assert!(parse_line(r#"{"type":"user","isReplay":true,"message":{"content":"hi"}}"#).is_empty());
    }

    /// Captured from Claude Code 2.1.325 with no login.
    #[test]
    fn signed_out_is_an_error() {
        let line = r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"Authentication error · This may be a temporary network issue, please try again"}]},"parent_tool_use_id":null,"session_id":"s","error":"authentication_failed","is_api_error_message":true}"#;
        assert_eq!(
            parse_line(line),
            vec![Event::Error { message: "Authentication error · This may be a temporary network issue, please try again".into() }]
        );
    }

    #[test]
    fn result_ends_the_turn() {
        let line = r#"{"type":"result","is_error":false,"result":"Hi!","session_id":"s1","total_cost_usd":0.0123,"duration_ms":4200,"stop_reason":"end_turn","permission_denials":[{"tool_name":"Bash"}]}"#;
        let events = parse_line(line);
        let [Event::Finished(outcome)] = events.as_slice() else { panic!("expected one Finished, got {events:?}") };
        assert!(outcome.ok);
        assert_eq!(outcome.session_id.as_deref(), Some("s1"));
        assert_eq!(outcome.result.as_deref(), Some("Hi!"));
        assert_eq!(outcome.cost_usd, Some(0.0123));
        assert_eq!(outcome.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(outcome.permission_denials.len(), 1);
    }
}
