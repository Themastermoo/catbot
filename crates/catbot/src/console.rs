//! Shows a running turn in the terminal and asks the person about approvals there.
//!
//! The reply goes to stdout; everything around it (tools, questions, errors, cost) goes to
//! stderr, so `catbot … > reply.txt` keeps just the words. What happened is also returned as
//! a [`Transcript`], for whoever wants to keep it.

use std::{
    collections::HashMap,
    io::{BufRead, IsTerminal, Write},
};

use serde::Serialize;
use serde_json::Value;

use crate::{
    approval::{Decision, REFUSED_NEXT_STEP},
    claude::{Event, Outcome, Running},
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Everything worth keeping from one turn.
#[derive(Debug, Default, Serialize)]
pub struct Transcript {
    /// The person stopped the turn (Ctrl+C).
    pub stopped: bool,
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// The bot's reply: every finished text block, in order.
    pub reply: String,
    pub steps: Vec<Step>,
    pub outcome: Option<Outcome>,
}

impl Transcript {
    pub fn ok(&self) -> bool {
        !self.stopped && self.outcome.as_ref().is_some_and(|o| o.ok)
    }
}

/// Something the bot did or ran into on the way to its reply.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    Tool { name: String, detail: String, ok: Option<bool>, note: Option<String> },
    Approval { tool: String, detail: String, allowed: bool, said: Option<String> },
    Error { message: String },
}

/// Plays a turn out in the terminal. With `json`, prints raw events instead (and asks nothing).
pub async fn drive(running: &mut Running, approvals_on: bool, json: bool) -> Result<Transcript, BoxError> {
    let paint = Paint::stderr();
    let mut out = std::io::stdout();
    let mut transcript = Transcript::default();
    let mut tools: HashMap<String, usize> = HashMap::new(); // tool id → index in steps
    let mut streamed = false; // deltas of the current text block were already printed
    let mut mid_line = false; // the reply stopped partway through a line
    loop {
        let event = tokio::select! {
            event = running.events.recv() => event,
            _ = tokio::signal::ctrl_c() => {
                running.stop().await;
                eprintln!("\n{}", paint.dim("stopped"));
                transcript.stopped = true;
                return Ok(transcript);
            }
        };
        let Some(event) = event else { break };
        if json {
            println!("{}", serde_json::to_string(&event)?);
        }
        match event {
            Event::Started { session_id, model } => {
                if !json {
                    let shown = model.clone().unwrap_or_else(|| "default model".into());
                    eprintln!("{}", paint.dim(&format!("● {shown} · session {session_id}")));
                }
                transcript.session_id = Some(session_id);
                transcript.model = model;
            }
            Event::TextDelta { text } => {
                if !json {
                    print!("{text}");
                    out.flush()?;
                }
                streamed = true;
                mid_line = !text.ends_with('\n');
            }
            Event::ThinkingDelta { .. } => {}
            Event::Text { text } => {
                if !json {
                    if !streamed {
                        print!("{text}");
                    }
                    println!();
                }
                if !transcript.reply.is_empty() {
                    transcript.reply.push_str("\n\n");
                }
                transcript.reply.push_str(text.trim_end());
                streamed = false;
                mid_line = false;
            }
            Event::ApprovalRequested { id, tool, input } => {
                let detail = summarize(&input);
                if mid_line && !json {
                    println!();
                    mid_line = false;
                }
                eprintln!("{}", paint.yellow(&format!("  ? {tool} {detail}")));
                eprint!("{}", paint.yellow("    allow? [y]es / [n]o, or type a reason to refuse: "));
                std::io::stderr().flush()?;
                let reply = tokio::select! {
                    line = tokio::task::spawn_blocking(read_answer) => line?,
                    _ = tokio::signal::ctrl_c() => {
                        running.stop().await;
                        eprintln!("\n{}", paint.dim("stopped"));
                        transcript.stopped = true;
                        return Ok(transcript);
                    }
                };
                let decision = decide(&reply);
                if decision == Decision::Allow {
                    eprintln!("{}", paint.dim("    ✓ allowed"));
                }
                let said = (!matches!(reply.to_ascii_lowercase().as_str(), "" | "y" | "yes" | "n" | "no")).then(|| reply.clone());
                transcript.steps.push(Step::Approval { tool, detail, allowed: decision == Decision::Allow, said });
                running.answer(&id, decision);
            }
            Event::ToolStarted { id, name, input } => {
                let detail = summarize(&input);
                if !json {
                    eprintln!("{}", paint.dim(&format!("  ⚙ {name} {detail}")));
                }
                tools.insert(id, transcript.steps.len());
                transcript.steps.push(Step::Tool { name, detail, ok: None, note: None });
            }
            Event::ToolFinished { id, ok, output } => {
                let note = if ok {
                    None
                } else {
                    // What the person or the tool said, not the instructions added for the bot.
                    let said = output.split(REFUSED_NEXT_STEP).next().unwrap_or("");
                    Some(said.lines().next().unwrap_or("").chars().take(200).collect::<String>())
                };
                if let Some(Step::Tool { name, ok: done, note: kept, .. }) = tools.get(&id).and_then(|i| transcript.steps.get_mut(*i)) {
                    if let (Some(note), false) = (&note, json) {
                        eprintln!("{}", paint.red(&format!("    ✗ {name}: {note}")));
                    }
                    *done = Some(ok);
                    *kept = note;
                }
            }
            Event::Notice { text } => {
                if !json {
                    eprintln!("{text}");
                }
            }
            Event::Error { message } => {
                if !json {
                    eprintln!("{}", paint.red(&format!("✗ {message}")));
                }
                transcript.steps.push(Step::Error { message });
            }
            Event::Finished(outcome) => {
                if !json {
                    let mut parts = vec![if outcome.ok { "done".to_string() } else { "failed".to_string() }];
                    if let Some(cost) = outcome.cost_usd {
                        parts.push(format!("${cost:.4}"));
                    }
                    if let Some(ms) = outcome.duration_ms {
                        parts.push(format!("{:.1}s", ms as f64 / 1000.0));
                    }
                    eprintln!("{}", paint.dim(&format!("─ {}", parts.join(" · "))));
                    let denied = outcome.permission_denials.len();
                    if denied > 0 && approvals_on {
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
                }
                if let Some(id) = outcome.session_id.clone().filter(|id| !id.is_empty()) {
                    transcript.session_id = Some(id);
                }
                transcript.outcome = Some(outcome);
            }
        }
    }
    Ok(transcript)
}

/// Whether questions can be asked here: someone at a keyboard, and not in JSON mode.
pub fn can_ask(json: bool) -> bool {
    !json && std::io::stdin().is_terminal()
}

fn read_answer() -> String {
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    line.trim().to_string()
}

/// "y" or "yes" allows; anything else refuses, and words longer than "n"/"no" go to the bot.
pub fn decide(reply: &str) -> Decision {
    match reply.to_ascii_lowercase().as_str() {
        "y" | "yes" => Decision::Allow,
        "" | "n" | "no" => Decision::Deny("The person refused this step.".into()),
        _ => Decision::Deny(format!("The person refused this step and said: {reply}")),
    }
}

/// The one detail of a tool call worth a glance: the command, the file, the search, the address.
pub fn summarize(input: &Value) -> String {
    let detail = ["command", "file_path", "path", "pattern", "url", "query", "description"]
        .iter()
        .find_map(|key| input[*key].as_str())
        .unwrap_or("");
    let line = detail.lines().next().unwrap_or("");
    let short: String = line.chars().take(80).collect();
    if short.len() < line.len() { format!("{short}…") } else { short }
}

/// Terminal colours, only when stderr is a terminal.
pub struct Paint {
    on: bool,
}

impl Paint {
    pub fn stderr() -> Self {
        Self { on: std::io::stderr().is_terminal() }
    }
    fn wrap(&self, code: &str, text: &str) -> String {
        if self.on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }
    pub fn dim(&self, text: &str) -> String {
        self.wrap("2", text)
    }
    pub fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.wrap("33", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.wrap("31", text)
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
