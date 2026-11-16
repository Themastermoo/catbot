//! Runs a bot's turn for the interface: the driver's events become live frames, approval
//! questions become cards, and the exchange is saved.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tokio::sync::Notify;

use super::{App, hub::runtime, wire};
use crate::{
    approval::{Answerer, Decision},
    chat,
    claude::{self, Event, Outcome, Session},
    console::{Step, summarize},
    store::{Bot, Conversation, Role},
};

/// A turn in progress, as the web side sees it.
struct Handle {
    turn_id: String,
    answerer: Option<Answerer>,
    cancel: Arc<Notify>,
    /// Approval cards still waiting for the person: request id → the card's message id.
    open_cards: HashMap<String, i64>,
}

/// The turns running now, one at most per conversation.
#[derive(Default)]
pub struct Turns {
    running: Mutex<HashMap<String, Handle>>,
}

impl Turns {
    pub fn busy(&self, thread_id: &str) -> bool {
        self.running.lock().expect("turns lock").contains_key(thread_id)
    }

    /// What the conversation's bot is doing: "idle", "working" or "waiting-on-you".
    pub fn activity(&self, thread_id: &str) -> &'static str {
        match self.running.lock().expect("turns lock").get(thread_id) {
            None => "idle",
            Some(handle) if !handle.open_cards.is_empty() => "waiting-on-you",
            Some(_) => "working",
        }
    }

    fn add_card(&self, thread_id: &str, request_id: &str, message_id: i64) {
        if let Some(handle) = self.running.lock().expect("turns lock").get_mut(thread_id) {
            handle.open_cards.insert(request_id.to_string(), message_id);
        }
    }

    /// Takes a card off the open list, returning its message id.
    fn close_card(&self, thread_id: &str, request_id: &str) -> Option<i64> {
        self.running.lock().expect("turns lock").get_mut(thread_id)?.open_cards.remove(request_id)
    }

    fn open_cards(&self, thread_id: &str) -> Vec<String> {
        self.running.lock().expect("turns lock").get(thread_id).map(|h| h.open_cards.keys().cloned().collect()).unwrap_or_default()
    }

    /// Stops the conversation's turn. False if none was running.
    pub fn interrupt(&self, thread_id: &str) -> bool {
        match self.running.lock().expect("turns lock").get(thread_id) {
            Some(handle) => {
                handle.cancel.notify_one();
                true
            }
            None => false,
        }
    }

    /// Answers an approval question. Returns the turn id it belonged to, or None if there is
    /// no such open question.
    pub fn answer(&self, thread_id: &str, request_id: &str, decision: Decision) -> Option<String> {
        let running = self.running.lock().expect("turns lock");
        let handle = running.get(thread_id)?;
        let answerer = handle.answerer.as_ref()?;
        answerer.answer(request_id, decision).then(|| handle.turn_id.clone())
    }

    fn set_answerer(&self, thread_id: &str, answerer: Option<Answerer>) {
        if let Some(handle) = self.running.lock().expect("turns lock").get_mut(thread_id) {
            handle.answerer = answerer;
        }
    }

    fn finish(&self, thread_id: &str) {
        self.running.lock().expect("turns lock").remove(thread_id);
    }
}

/// Starts a turn on a conversation. Fails if one is already running there.
pub fn start(app: Arc<App>, bot: Bot, conversation: Conversation, request_id: i64, text: String) -> Result<String, &'static str> {
    let turn_id = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(Notify::new());
    {
        let mut running = app.turns.running.lock().expect("turns lock");
        if running.contains_key(&conversation.id) {
            return Err("this bot is still working on the last message");
        }
        running.insert(
            conversation.id.clone(),
            Handle { turn_id: turn_id.clone(), answerer: None, cancel: cancel.clone(), open_cards: HashMap::new() },
        );
    }
    let id = turn_id.clone();
    tokio::spawn(async move {
        let thread_id = conversation.id.clone();
        run(&app, &bot, conversation, request_id, &text, &id, cancel).await;
        app.turns.finish(&thread_id);
        app.announce_bot(&bot.id);
    });
    Ok(turn_id)
}

/// What one run of the CLI produced.
#[derive(Default)]
struct Result_ {
    reply: String,
    steps: Vec<Step>,
    outcome: Option<Outcome>,
    errors: Vec<String>,
    stopped: bool,
}

#[allow(clippy::too_many_arguments)]
async fn run(
    app: &Arc<App>,
    bot: &Bot,
    conversation: Conversation,
    request_id: i64,
    text: &str,
    turn_id: &str,
    cancel: Arc<Notify>,
) {
    let thread = conversation.id.as_str();
    let emit = |event: Value| app.hub.send(runtime(thread, turn_id, event));
    app.announce_bot(&bot.id);
    emit(json!({ "type": "turn.started" }));

    let codex = chat::codex_dir(bot, &app.codex);
    app.seed_memory(bot);
    let mut session = match &conversation.session_id {
        Some(id) => Session::Resume(id.clone()),
        None => Session::New(uuid::Uuid::new_v4().to_string()),
    };
    if let Session::New(id) = &session {
        app.with_store(|store| store.set_session(thread, id));
    }

    let item_id = format!("item-{turn_id}");
    let mut result = drive(app, bot, &session, text, codex.clone(), thread, &item_id, &cancel, &emit).await;
    let lost = result.outcome.as_ref().is_some_and(|o| !o.ok && o.result.as_deref().is_some_and(chat::says_lost))
        || result.errors.iter().any(|e| chat::says_lost(e));
    if matches!(session, Session::Resume(_)) && lost && !result.stopped {
        // Claude no longer has this conversation: carry on in a new session.
        let id = uuid::Uuid::new_v4().to_string();
        app.with_store(|store| store.set_session(thread, &id));
        let note = "Claude no longer had this conversation, so it continued in a fresh one.";
        save_and_send(app, thread, Role::Note, note, None);
        session = Session::New(id);
        result = drive(app, bot, &session, text, codex, thread, &item_id, &cancel, &emit).await;
    }

    // Close the cards of questions nobody answered before the turn ended.
    for request in app.turns.open_cards(thread) {
        emit(json!({ "type": "request.resolved", "requestId": request, "behavior": "deny", "source": "system" }));
        settle_card(app, thread, &request, "deny", true);
    }
    // Keep what happened, and show the kept rows (they replace the live ones).
    for step in &result.steps {
        save_step(app, thread, step);
    }
    if !result.reply.is_empty() {
        let outcome = result.outcome.as_ref();
        let detail = json!({
            "item_id": item_id,
            "turn_id": turn_id,
            "request_message_id": wire::message_id(request_id),
            "cost_usd": outcome.and_then(|o| o.cost_usd),
            "duration_ms": outcome.and_then(|o| o.duration_ms),
        });
        save_and_send(app, thread, Role::Bot, &result.reply, Some(&detail));
    }
    if result.stopped {
        save_and_send(app, thread, Role::Note, "Stopped.", None);
    }
    let outcome = result.outcome.unwrap_or_default();
    emit(json!({
        "type": "turn.completed",
        "ok": outcome.ok && !result.stopped,
        "stopReason": if result.stopped { Some("interrupted".to_string()) } else { outcome.stop_reason.clone() },
        "cost": outcome.cost_usd,
        "denials": outcome.permission_denials.iter().filter_map(|d| d["tool_name"].as_str()).collect::<Vec<_>>(),
    }));
}

/// Runs the CLI once, turning its events into live frames.
#[allow(clippy::too_many_arguments)]
async fn drive(
    app: &Arc<App>,
    bot: &Bot,
    session: &Session,
    text: &str,
    codex: Option<std::path::PathBuf>,
    thread: &str,
    item_id: &str,
    cancel: &Notify,
    emit: &impl Fn(Value),
) -> Result_ {
    let mut result = Result_::default();
    let turn = chat::bot_turn(bot, text.to_string(), session.clone(), app.cli.clone(), app.helper.clone(), codex, app.memory.clone());
    let mut running = match claude::start(&turn).await {
        Ok(running) => running,
        Err(err) => {
            let message = err.to_string();
            emit(json!({ "type": "runtime.error", "message": message, "setup": true }));
            result.steps.push(Step::Error { message: message.clone() });
            result.errors.push(message);
            return result;
        }
    };
    app.turns.set_answerer(thread, running.answerer());

    let mut tools: HashMap<String, usize> = HashMap::new();
    let mut gap = false; // a text block ended; the next one starts a new paragraph
    loop {
        let event = tokio::select! {
            event = running.events.recv() => event,
            _ = cancel.notified() => {
                running.stop().await;
                result.stopped = true;
                break;
            }
        };
        let Some(event) = event else { break };
        match event {
            Event::Started { session_id, model } => {
                emit(json!({ "type": "session.started", "sessionId": session_id, "model": model }));
            }
            Event::TextDelta { text } => {
                let delta = if gap { format!("\n\n{text}") } else { text };
                gap = false;
                emit(json!({ "type": "content.delta", "streamKind": "assistant_text", "delta": delta, "itemId": item_id }));
            }
            Event::ThinkingDelta { text } => {
                emit(json!({ "type": "content.delta", "streamKind": "reasoning_text", "delta": text, "itemId": format!("{item_id}-thinking") }));
            }
            Event::Text { text } => {
                if !result.reply.is_empty() {
                    result.reply.push_str("\n\n");
                }
                result.reply.push_str(text.trim_end());
                gap = true;
            }
            Event::ToolStarted { id, name, input } => {
                let detail = summarize(&input);
                emit(json!({ "type": "item.started", "itemType": "tool", "itemId": id, "title": name, "summary": detail, "input": detail }));
                tools.insert(id, result.steps.len());
                result.steps.push(Step::Tool { name, detail, ok: None, note: None });
            }
            Event::ToolFinished { id, ok, output } => {
                let short: String = output.chars().take(500).collect();
                emit(json!({ "type": "item.completed", "itemType": "tool", "itemId": id, "ok": ok, "output": short }));
                if let Some(Step::Tool { ok: done, note, .. }) = tools.get(&id).and_then(|i| result.steps.get_mut(*i)) {
                    *done = Some(ok);
                    if !ok {
                        *note = Some(output.lines().next().unwrap_or("").chars().take(200).collect());
                    }
                }
            }
            Event::ApprovalRequested { id, tool, input } => {
                let summary = format!("{tool} {}", summarize(&input)).trim().to_string();
                emit(json!({ "type": "request.opened", "requestId": id, "requestType": "permission", "tool": tool, "summary": summary }));
                // The card the person answers, as the reference server draws it.
                let card = json!({
                    "title": "Approval needed",
                    "subtitle": summary,
                    "options": ["Allow", "Deny"],
                    "requestId": id,
                    "requestType": "permission",
                    "tool": tool,
                });
                if let Some(message) = save_and_send(app, thread, Role::Card, &summary, Some(&json!({ "card": card })))
                    && let Some(message_id) = message["id"].as_str().and_then(wire::parse_message_id)
                {
                    app.turns.add_card(thread, &id, message_id);
                    app.announce_bot(&bot.id);
                }
            }
            Event::Error { message } => {
                emit(json!({ "type": "runtime.error", "message": message }));
                result.steps.push(Step::Error { message: message.clone() });
                result.errors.push(message);
            }
            Event::Finished(outcome) => {
                if let Some(id) = outcome.session_id.as_deref().filter(|id| !id.is_empty()) {
                    app.with_store(|store| store.set_session(thread, id));
                }
                result.outcome = Some(outcome);
            }
        }
    }
    result
}

/// Marks an approval card answered (`dismissed` when nobody answered it) and shows the change.
pub fn settle_card(app: &App, thread: &str, request_id: &str, behavior: &str, dismissed: bool) {
    let Some(message_id) = app.turns.close_card(thread, request_id) else { return };
    let wire = app.with_store(|store| {
        let message = store.message(message_id)?.ok_or("card vanished")?;
        let mut detail = message.detail.clone().unwrap_or_else(|| json!({}));
        detail["card"]["answered"] = json!(behavior);
        detail["card"]["dismissed"] = json!(dismissed);
        store.update_detail(message_id, &detail)?;
        let message = store.message(message_id)?.ok_or("card vanished")?;
        Ok(wire::message(&message, store.previous_message_id(thread, message_id)?))
    });
    if let Some(wire) = wire {
        app.hub.send(json!({ "kind": "message.patch", "threadId": thread, "message": wire }));
    }
}

fn save_step(app: &Arc<App>, thread: &str, step: &Step) {
    let body = match step {
        Step::Tool { name, detail, .. } => format!("{name} {detail}").trim().to_string(),
        Step::Approval { tool, detail, allowed, .. } => {
            format!("{} {tool} {detail}", if *allowed { "allowed" } else { "refused" }).trim().to_string()
        }
        Step::Error { message } => message.clone(),
    };
    let detail = serde_json::to_value(step).ok();
    save_and_send(app, thread, Role::Step, &body, detail.as_ref());
}

/// Saves a message and sends it to every open interface.
pub fn save_and_send(app: &App, thread: &str, role: Role, body: &str, detail: Option<&Value>) -> Option<Value> {
    let wire = app.with_store(|store| {
        let id = store.add_message(thread, role, body, detail)?;
        let parent = store.previous_message_id(thread, id)?;
        let message = store.message(id)?.ok_or("message vanished")?;
        Ok(wire::message(&message, parent))
    })?;
    app.hub.send(json!({ "kind": "message", "threadId": thread, "message": wire }));
    Some(wire)
}
