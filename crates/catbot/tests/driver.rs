//! The driver against a stand-in `claude` (tests/fake-claude.py): process handling end to end.

use catbot::{
    approval::Decision,
    claude::{self, Event, Outcome, PermissionMode, Session, Turn},
};

fn turn(prompt: &str) -> Turn {
    Turn {
        prompt: prompt.into(),
        session: Session::New("11111111-2222-4333-8444-555555555555".into()),
        model: Some("sonnet".into()),
        permission_mode: PermissionMode::Default,
        cwd: std::env::temp_dir(),
        system_prompt: Some("You are a test bot.".into()),
        cli: vec!["python3".into(), concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.py").into()],
        isolated: true,
        approval_helper: None,
        extra_dirs: Vec::new(),
        catbot_tools: None,
        effort: None,
    }
}

fn with_approvals(prompt: &str) -> Turn {
    Turn { approval_helper: Some(vec![env!("CARGO_BIN_EXE_catbot").into()]), ..turn(prompt) }
}

/// Runs a turn, answering every approval question with `decision`.
async fn run_answering(turn: Turn, decision: Decision) -> Vec<Event> {
    let mut running = claude::start(&turn).await.expect("starts");
    let mut events = Vec::new();
    while let Some(event) = running.events.recv().await {
        if let Event::ApprovalRequested { id, .. } = &event {
            assert!(running.answer(id, decision.clone()), "the question is still open");
            assert!(!running.answer(id, decision.clone()), "a question is answered once");
        }
        events.push(event);
    }
    events
}

async fn run(turn: Turn) -> Vec<Event> {
    let mut running = claude::start(&turn).await.expect("starts");
    let mut events = Vec::new();
    while let Some(event) = running.events.recv().await {
        events.push(event);
    }
    events
}

fn outcome(events: &[Event]) -> &Outcome {
    match events.last() {
        Some(Event::Finished(outcome)) => outcome,
        other => panic!("the last event should be Finished, got {other:?}"),
    }
}

#[tokio::test]
async fn a_reply_streams_and_finishes() {
    let events = run(turn("Moo")).await;
    assert!(matches!(&events[0], Event::Started { session_id, model }
        if session_id == "11111111-2222-4333-8444-555555555555" && model.as_deref() == Some("sonnet")));
    let streamed: String = events.iter().filter_map(|e| match e { Event::TextDelta { text } => Some(text.as_str()), _ => None }).collect();
    assert_eq!(streamed, "Hello, Moo");
    assert!(events.contains(&Event::Text { text: "Hello, Moo".into() }));
    let outcome = outcome(&events);
    assert!(outcome.ok);
    assert_eq!(outcome.session_id.as_deref(), Some("11111111-2222-4333-8444-555555555555"));
}

#[tokio::test]
async fn resume_continues_the_same_session() {
    let mut t = turn("again");
    t.session = Session::Resume("22222222-0000-4000-8000-000000000000".into());
    let events = run(t).await;
    assert_eq!(outcome(&events).session_id.as_deref(), Some("22222222-0000-4000-8000-000000000000"));
}

#[tokio::test]
async fn a_refused_tool_is_reported() {
    let events = run(turn("tool")).await;
    assert!(events.iter().any(|e| matches!(e, Event::ToolStarted { name, .. } if name == "Bash")));
    assert!(events.iter().any(|e| matches!(e, Event::ToolFinished { ok: false, .. })));
    assert_eq!(outcome(&events).permission_denials.len(), 1);
}

#[tokio::test]
async fn a_crash_becomes_an_error_and_a_failed_finish() {
    let events = run(turn("crash")).await;
    assert!(events.iter().any(|e| matches!(e, Event::Error { message } if message.contains("something broke"))));
    assert!(!outcome(&events).ok);
}

#[tokio::test]
async fn a_missing_cli_is_explained() {
    let mut t = turn("hi");
    t.cli = vec!["claude-that-does-not-exist".into()];
    let err = claude::start(&t).await.err().expect("fails to start");
    assert!(err.to_string().contains("is Claude Code installed?"));
}

#[tokio::test]
async fn an_approved_step_runs() {
    let events = run_answering(with_approvals("approve"), Decision::Allow).await;
    let asked = events.iter().find_map(|e| match e {
        Event::ApprovalRequested { tool, input, .. } => Some((tool.clone(), input.clone())),
        _ => None,
    });
    assert_eq!(asked, Some(("Bash".to_string(), serde_json::json!({"command": "echo hi"}))));
    assert!(events.contains(&Event::ToolFinished { id: "t2".into(), ok: true, output: "hi".into() }));
    assert!(outcome(&events).permission_denials.is_empty());
}

#[tokio::test]
async fn a_refused_step_tells_the_bot_why() {
    let events = run_answering(with_approvals("approve"), Decision::Deny("use /tmp instead".into())).await;
    let refusal = events.iter().find_map(|e| match e {
        Event::ToolFinished { ok: false, output, .. } => Some(output.clone()),
        _ => None,
    });
    let refusal = refusal.expect("the step was refused");
    assert!(refusal.starts_with("use /tmp instead"), "{refusal}");
    assert!(refusal.contains("Do not try another way"), "{refusal}");
    assert_eq!(outcome(&events).permission_denials.len(), 1);
}

#[tokio::test]
async fn without_a_helper_nothing_is_asked() {
    let events = run(turn("approve")).await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    assert_eq!(outcome(&events).permission_denials.len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn the_approval_folder_is_private_and_removed_after_the_turn() {
    use std::os::unix::fs::PermissionsExt;
    let mut running = claude::start(&with_approvals("approve")).await.expect("starts");
    let dir = running.approval_dir().expect("approvals are on").to_path_buf();
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(dir.join("mcp.json")).unwrap().permissions().mode() & 0o777, 0o600);
    while let Some(event) = running.events.recv().await {
        if let Event::ApprovalRequested { id, .. } = &event {
            running.answer(id, Decision::Allow);
        }
    }
    drop(running);
    assert!(!dir.exists(), "{} should be gone", dir.display());
}
