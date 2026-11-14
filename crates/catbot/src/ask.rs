//! `catbot ask`: one message to Claude through the driver, the reply printed as it streams.
//!
//! The reply goes to stdout; everything around it (tools, errors, cost) goes to stderr, so
//! `catbot ask "…" > reply.txt` keeps just the words.

use std::{
    collections::HashMap,
    io::{BufRead, IsTerminal, Write},
    path::PathBuf,
};

use serde_json::Value;

use crate::{
    approval::{Decision, REFUSED_NEXT_STEP},
    claude::{self, Event, PermissionMode, Session, Turn},
};

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
    pub no_approvals: bool,
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
        // Questions need someone at a keyboard to answer them.
        approval_helper: if options.no_approvals || options.json || !std::io::stdin().is_terminal() {
            None
        } else {
            Some(vec![std::env::current_exe()?.to_string_lossy().into_owned()])
        },
    };
    let mut running = claude::start(&turn).await?;

    let mut out = std::io::stdout();
    let mut streamed = false; // deltas of the current text block were already printed
    let mut tools: HashMap<String, String> = HashMap::new();
    let mut ok = false;
    let mut mid_line = false; // the reply stopped partway through a line
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
                mid_line = !text.ends_with('\n');
            }
            Event::ThinkingDelta { .. } => {}
            Event::Text { text } => {
                if !streamed {
                    print!("{text}");
                }
                println!();
                streamed = false;
                mid_line = false;
            }
            Event::ApprovalRequested { id, tool, input } => {
                if mid_line {
                    println!();
                    mid_line = false;
                }
                eprintln!("{}", paint.yellow(&format!("  ? {tool} {}", summarize(&input))));
                eprint!("{}", paint.yellow("    allow? [y]es / [n]o, or type a reason to refuse: "));
                std::io::stderr().flush()?;
                let reply = tokio::select! {
                    line = tokio::task::spawn_blocking(read_answer) => line?,
                    _ = tokio::signal::ctrl_c() => {
                        running.stop().await;
                        eprintln!("\n{}", paint.dim("stopped"));
                        return Ok(false);
                    }
                };
                let decision = decide(&reply);
                if decision == Decision::Allow {
                    eprintln!("{}", paint.dim("    ✓ allowed"));
                }
                running.answer(&id, decision);
            }
            Event::ToolStarted { id, name, input } => {
                eprintln!("{}", paint.dim(&format!("  ⚙ {name} {}", summarize(&input))));
                tools.insert(id, name);
            }
            Event::ToolFinished { id, ok, output } => {
                let name = tools.get(&id).map(String::as_str).unwrap_or("tool");
                if !ok {
                    // Show what the person or the tool said, not the instructions added for the bot.
                    let said = output.split(REFUSED_NEXT_STEP).next().unwrap_or("");
                    let first = said.lines().next().unwrap_or("").chars().take(120).collect::<String>();
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
                if denied > 0 && turn.approval_helper.is_some() {
                    eprintln!("{}", paint.dim(&format!("  {denied} step(s) refused")));
                } else if denied > 0 {
                    eprintln!(
                        "{}",
                        paint.dim(&format!(
                            "  {denied} step(s) needed approval and were refused, because nobody could be asked \
                             (run in a terminal, without --json or --no-approvals, to be asked)."
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

fn read_answer() -> String {
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim().to_string()
}

/// "y" or "yes" allows; anything else refuses, and words longer than "n"/"no" go to the bot.
fn decide(reply: &str) -> Decision {
    match reply.to_ascii_lowercase().as_str() {
        "y" | "yes" => Decision::Allow,
        "" | "n" | "no" => Decision::Deny("The person refused this step.".into()),
        _ => Decision::Deny(format!("The person refused this step and said: {reply}")),
    }
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
    fn yellow(&self, text: &str) -> String {
        if self.on { format!("\x1b[33m{text}\x1b[0m") } else { text.to_string() }
    }
    fn red(&self, text: &str) -> String {
        if self.on { format!("\x1b[31m{text}\x1b[0m") } else { text.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::{decide, summarize};
    use crate::approval::Decision;
    use serde_json::json;

    #[test]
    fn answers() {
        assert_eq!(decide("y"), Decision::Allow);
        assert_eq!(decide("YES"), Decision::Allow);
        assert!(matches!(decide(""), Decision::Deny(_)));
        assert_eq!(decide("no use /tmp instead"), Decision::Deny("The person refused this step and said: no use /tmp instead".into()));
    }

    #[test]
    fn summaries_pick_the_useful_field() {
        assert_eq!(summarize(&json!({"command": "ls -la\necho hi"})), "ls -la");
        assert_eq!(summarize(&json!({"file_path": "/tmp/a.txt", "content": "x"})), "/tmp/a.txt");
        assert_eq!(summarize(&json!({})), "");
        assert!(summarize(&json!({"command": "x".repeat(200)})).ends_with('…'));
    }
}
