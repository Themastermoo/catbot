//! `catbot ask`: one message to Claude through the driver, the reply printed as it streams.
//!
//! The reply goes to stdout; everything around it (tools, errors, cost) goes to stderr, so
//! `catbot ask "…" > reply.txt` keeps just the words.

use std::{
    collections::HashMap,
    io::{IsTerminal, Write},
    path::PathBuf,
};

use serde_json::Value;

use crate::claude::{self, Event, PermissionMode, Session, Turn};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct Options {
    pub prompt: String,
    pub resume: Option<String>,
    pub model: Option<String>,
    pub mode: PermissionMode,
    pub cwd: PathBuf,
    pub system: Option<String>,
    pub cli: String,
    pub inherit_user_config: bool,
    pub json: bool,
}

/// Runs the turn. Returns whether it finished well.
pub async fn run(options: Options) -> Result<bool, BoxError> {
    let paint = Paint { on: std::io::stderr().is_terminal() };
    if options.mode == PermissionMode::BypassPermissions {
        eprintln!("{}", paint.red("! bypass-permissions: the agent runs every command without asking."));
    }
    let session = match &options.resume {
        Some(id) => Session::Resume(id.clone()),
        None => Session::New(uuid::Uuid::new_v4().to_string()),
    };
    let turn = Turn {
        prompt: options.prompt,
        session,
        model: options.model,
        permission_mode: options.mode,
        cwd: options.cwd,
        system_prompt: options.system,
        cli: vec![options.cli],
        isolated: !options.inherit_user_config,
    };
    let mut running = claude::start(&turn).await?;

    let mut out = std::io::stdout();
    let mut streamed = false; // deltas of the current text block were already printed
    let mut tools: HashMap<String, String> = HashMap::new();
    let mut ok = false;
    loop {
        let event = tokio::select! {
            event = running.events.recv() => event,
            _ = tokio::signal::ctrl_c() => {
                running.stop().await;
                eprintln!("\n{}", paint.dim("stopped"));
                return Ok(false);
            }
        };
        let Some(event) = event else { break };
        if options.json {
            println!("{}", serde_json::to_string(&event)?);
            if let Event::Finished(outcome) = &event {
                ok = outcome.ok;
            }
            continue;
        }
        match event {
            Event::Started { session_id, model } => {
                let model = model.unwrap_or_else(|| "default model".into());
                eprintln!("{}", paint.dim(&format!("● {model} · session {session_id}")));
            }
            Event::TextDelta { text } => {
                print!("{text}");
                out.flush()?;
                streamed = true;
            }
            Event::ThinkingDelta { .. } => {}
            Event::Text { text } => {
                if !streamed {
                    print!("{text}");
                }
                println!();
                streamed = false;
            }
            Event::ToolStarted { id, name, input } => {
                eprintln!("{}", paint.dim(&format!("  ⚙ {name} {}", summarize(&input))));
                tools.insert(id, name);
            }
            Event::ToolFinished { id, ok, output } => {
                let name = tools.get(&id).map(String::as_str).unwrap_or("tool");
                if !ok {
                    let first = output.lines().next().unwrap_or("").chars().take(120).collect::<String>();
                    eprintln!("{}", paint.red(&format!("    ✗ {name}: {first}")));
                }
            }
            Event::Error { message } => eprintln!("{}", paint.red(&format!("✗ {message}"))),
            Event::Finished(outcome) => {
                ok = outcome.ok;
                let mut parts = vec![if outcome.ok { "done".to_string() } else { "failed".to_string() }];
                if let Some(cost) = outcome.cost_usd {
                    parts.push(format!("${cost:.4}"));
                }
                if let Some(ms) = outcome.duration_ms {
                    parts.push(format!("{:.1}s", ms as f64 / 1000.0));
                }
                eprintln!("{}", paint.dim(&format!("─ {}", parts.join(" · "))));
                let denied = outcome.permission_denials.len();
                if denied > 0 {
                    eprintln!(
                        "{}",
                        paint.dim(&format!(
                            "  {denied} action(s) needed approval and were refused. Approval cards come later; \
                             --mode accept-edits allows file edits in the working folder."
                        ))
                    );
                }
                if let Some(id) = outcome.session_id.filter(|id| outcome.ok && !id.is_empty()) {
                    eprintln!("{}", paint.dim(&format!("  continue: catbot ask --resume {id} \"…\"")));
                }
            }
        }
    }
    Ok(ok)
}

/// The one detail of a tool call worth a glance: the command, the file, the search, the address.
fn summarize(input: &Value) -> String {
    let detail = ["command", "file_path", "path", "pattern", "url", "query", "description"]
        .iter()
        .find_map(|key| input[*key].as_str())
        .unwrap_or("");
    let line = detail.lines().next().unwrap_or("");
    let short: String = line.chars().take(80).collect();
    if short.len() < line.len() { format!("{short}…") } else { short }
}

struct Paint {
    on: bool,
}

impl Paint {
    fn dim(&self, text: &str) -> String {
        if self.on { format!("\x1b[2m{text}\x1b[0m") } else { text.to_string() }
    }
    fn red(&self, text: &str) -> String {
        if self.on { format!("\x1b[31m{text}\x1b[0m") } else { text.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::summarize;
    use serde_json::json;

    #[test]
    fn summaries_pick_the_useful_field() {
        assert_eq!(summarize(&json!({"command": "ls -la\necho hi"})), "ls -la");
        assert_eq!(summarize(&json!({"file_path": "/tmp/a.txt", "content": "x"})), "/tmp/a.txt");
        assert_eq!(summarize(&json!({})), "");
        assert!(summarize(&json!({"command": "x".repeat(200)})).ends_with('…'));
    }
}
