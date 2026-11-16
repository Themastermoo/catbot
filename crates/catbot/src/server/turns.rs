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
    api,
    approval::{Answerer, Decision},
    chat,
    claude::{self, Event, Outcome, Running, Session},
    engines,
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
    let (turn_id, cancel) = register(&app, &conversation.id)?;
    let id = turn_id.clone();
    tokio::spawn(async move {
        let thread_id = conversation.id.clone();
        let reply = run(&app, &bot, conversation, request_id, &text, &id, cancel, false).await;
        // A reply that ends with @Name brings that bot in (`handoffs.rs`); the chat stays busy.
        let next = reply.as_deref().map(|r| super::handoffs::next_after(&app, &bot, r)).unwrap_or_default();
        if !next.is_empty() {
            app.set_channel_busy(&thread_id, true);
        }
        app.turns.finish(&thread_id);
        app.announce_bot(&bot.id);
        if let Some(reply) = reply.filter(|_| !next.is_empty()) {
            super::handoffs::after_reply(&app, &bot, &thread_id, request_id, &reply, next).await;
        }
    });
    Ok(turn_id)
}

/// Runs a turn to its end, for bots answering one after another (a channel's, or a chain of
/// hand-offs), and gives back its reply. A `guest` is a bot answering in another bot's chat: it
/// starts afresh, shown the chat, and leaves that bot's session alone.
pub async fn run_through(app: &Arc<App>, bot: &Bot, conversation: Conversation, request_id: i64, text: &str, guest: bool) -> Result<Option<String>, &'static str> {
    let (turn_id, cancel) = register(app, &conversation.id)?;
    let thread_id = conversation.id.clone();
    let reply = run(app, bot, conversation, request_id, text, &turn_id, cancel, guest).await;
    app.turns.finish(&thread_id);
    Ok(reply)
}

/// Marks a conversation busy with a new turn: its id and its stop signal.
fn register(app: &App, thread: &str) -> Result<(String, Arc<Notify>), &'static str> {
    let turn_id = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(Notify::new());
    let mut running = app.turns.running.lock().expect("turns lock");
    if running.contains_key(thread) {
        return Err("this bot is still working on the last message");
    }
    running.insert(thread.to_string(), Handle { turn_id: turn_id.clone(), answerer: None, cancel: cancel.clone(), open_cards: HashMap::new() });
    Ok((turn_id, cancel))
}

/// What one run of the CLI produced.
#[derive(Default)]
struct Result_ {
    /// The model the AI said it's using.
    model: Option<String>,
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
    guest: bool,
) -> Option<String> {
    let thread = conversation.id.as_str();
    let emit = |event: Value| app.hub.send(runtime(thread, turn_id, event));
    app.announce_bot(&bot.id);
    emit(json!({ "type": "turn.started" }));

    let codex = chat::codex_dir(bot, &app.codex);
    app.seed_memory(bot);
    // In a channel several bots share the transcript, so each turn starts a fresh session shown
    // everything said there (a bot's own session wouldn't hold what the others said). A guest
    // in another bot's chat does the same, and keeps out of that bot's session.
    let fresh = conversation.channel_id.is_some() || guest;
    let mut session = match &conversation.session_id {
        Some(id) if !fresh => Session::Resume(id.clone()),
        _ => Session::New(uuid::Uuid::new_v4().to_string()),
    };
    if let (Session::New(id), false) = (&session, guest) {
        app.with_store(|store| store.set_session(thread, id));
    }

    // The bot's folder as it was before the turn, to show (and undo) what the turn changes.
    let points = super::changes::of(app, bot);
    let before = super::changes::snapshot(points.clone(), "before a turn").await;

    let item_id = format!("item-{turn_id}");
    // A fresh session (a branched chat, or one Claude lost) is shown what was said before.
    let with_earlier = |session: &Session| match session {
        Session::New(_) if bot.engine == engines::CLAUDE => format!("{}{text}", so_far(app, thread, if fresh { i64::MAX } else { request_id }, bot)),
        _ => text.to_string(),
    };
    let mut result = drive(app, bot, &session, &with_earlier(&session), codex.clone(), thread, &item_id, &cancel, &emit).await;
    let lost = result.outcome.as_ref().is_some_and(|o| !o.ok && o.result.as_deref().is_some_and(chat::says_lost))
        || result.errors.iter().any(|e| chat::says_lost(e));
    if matches!(session, Session::Resume(_)) && lost && !result.stopped {
        // Claude no longer has this conversation: carry on in a new session.
        let id = uuid::Uuid::new_v4().to_string();
        app.with_store(|store| store.set_session(thread, &id));
        let note = "Claude no longer had this conversation, so it continued in a fresh one.";
        save_and_send(app, thread, Role::Note, note, None);
        session = Session::New(id);
        result = drive(app, bot, &session, &with_earlier(&session), codex.clone(), thread, &item_id, &cancel, &emit).await;
    }

    // Claude turned the turn away at its usage limit before doing anything: a backup takes it.
    let mut ran_on = bot.clone();
    let said: String = result.errors.iter().cloned().chain(result.outcome.as_ref().and_then(|o| o.result.clone())).collect::<Vec<_>>().join(" ");
    if bot.engine == engines::CLAUDE && !result.stopped && result.reply.trim().is_empty() && result.steps.is_empty() && super::backups::says_limit(&said) {
        let until = super::backups::lifts_at(&said).unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(1));
        app.limited_until.lock().expect("limited lock").insert(bot.engine.clone(), until);
        super::usage::forget(app);
        if let Ok(Some((stand_in, why))) = super::backups::choose(app, bot) {
            save_and_send(app, thread, Role::Note, &why, None);
            if !guest {
                app.with_store(|store| store.clear_session(thread));
            }
            let fresh = Session::New(uuid::Uuid::new_v4().to_string());
            result = drive(app, &stand_in, &fresh, text, codex, thread, &item_id, &cancel, &emit).await;
            ran_on = stand_in;
        }
    }
    let bot = &ran_on;

    // Close the cards of questions nobody answered before the turn ended.
    for request in app.turns.open_cards(thread) {
        emit(json!({ "type": "request.resolved", "requestId": request, "behavior": "deny", "source": "system" }));
        settle_card(app, thread, &request, "deny", true);
    }
    // Keep what happened, and show the kept rows (they replace the live ones).
    for step in &result.steps {
        save_step(app, thread, step);
    }
    // Pictures the bot asked for come out of its reply, each with a card asking which AI draws it.
    let (reply, pictures) = super::images::take_requests(&result.reply);
    result.reply = reply;
    if !result.reply.is_empty() {
        let outcome = result.outcome.as_ref();
        let detail = json!({
            "item_id": item_id,
            "turn_id": turn_id,
            "request_message_id": wire::message_id(request_id),
            "botId": bot.id,
            "cost_usd": outcome.and_then(|o| o.cost_usd),
            "duration_ms": outcome.and_then(|o| o.duration_ms),
        });
        save_and_send(app, thread, Role::Bot, &result.reply, Some(&detail));
    }
    for prompt in pictures {
        super::images::card(app, thread, &prompt, &bot.name);
    }
    super::changes::after_turn(app, thread, &bot.id, points, before).await;
    // A backup answered: the bot's own AI starts afresh next time, shown everything said.
    let own_engine = app.with_store(|store| store.bot_by_id(&bot.id)).flatten().map(|b| b.engine);
    if !guest && own_engine.is_some_and(|own| own != bot.engine) {
        app.with_store(|store| store.clear_session(thread));
    }
    if let Some(outcome) = &result.outcome {
        let model = result.model.clone().or_else(|| bot.model.clone()).unwrap_or_default();
        super::usage::record(app, bot, thread, &bot.engine, &model, outcome.tokens, outcome.cost_usd);
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
    (!result.stopped && !result.reply.trim().is_empty()).then_some(result.reply)
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
    let started = if bot.engine == engines::CLAUDE {
        let turn = chat::bot_turn(bot, text.to_string(), session.clone(), app.cli.clone(), app.helper.clone(), codex, app.memory.clone());
        claude::start(&turn).await.map_err(|err| err.to_string())
    } else if engines::find(&bot.engine).is_some_and(|e| e.kind == engines::Kind::Agent) {
        start_agent(app, bot, thread, session, codex)
    } else {
        start_api(app, bot, thread, codex)
    };
    let mut running = match started {
        Ok(running) => running,
        Err(err) => {
            let message = err;
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
                if model.is_some() {
                    result.model = model.clone();
                }
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

/// Starts a turn on a chat API: its key and model, the conversation so far (the API keeps
/// none), and the folders its tools may use.
fn start_api(app: &Arc<App>, bot: &Bot, thread: &str, codex: Option<std::path::PathBuf>) -> Result<Running, String> {
    let engine = engines::find(&bot.engine)
        .filter(|e| e.kind == engines::Kind::Api)
        .ok_or_else(|| format!("This bot runs on {}, which Catbot doesn't know. Pick another AI in its settings.", bot.engine))?;
    let saved = app.with_store(|store| store.engine_key(engine.id)).flatten();
    let access = engines::access(engine, saved.as_ref())
        .ok_or_else(|| format!("{} needs an API key first: add it in this bot's settings.", engine.name))?;
    let model = bot.model.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| engine.default_model.to_string());
    let messages = app.with_store(|store| store.messages(thread, 60)).unwrap_or_default();
    // The newest message's pictures, shown to the model itself.
    let pictures = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Person)
        .map(|m| super::attachments::pictures(m, &bot.folder))
        .unwrap_or_default();
    let history = with_cue(lines(app, thread, messages, bot), app, thread, bot);
    let mut system = chat::system_prompt(bot, codex.as_deref(), app.memory.as_deref());
    system.push_str(&api::tool_guide(&bot.folder));
    let roots = std::iter::once(bot.folder.clone()).chain(app.memory.clone()).chain(codex).collect();
    let workspace = crate::tools::Workspace { folder: bot.folder.clone(), roots, mode: bot.mode };
    Ok(api::start(api::ApiTurn { engine, access, model, system, history, pictures, workspace }))
}

/// Starts a turn on an agent program: the program found on this computer, the session to carry
/// on, and what a new session needs to know.
fn start_agent(app: &Arc<App>, bot: &Bot, thread: &str, session: &Session, codex: Option<std::path::PathBuf>) -> Result<Running, String> {
    let engine = engines::find(&bot.engine).ok_or("no such agent")?;
    let program = engines::agent_program(engine).ok_or_else(|| {
        format!("{} isn't installed on this computer. Install it with `{}`, then sign in with `{}`.", engine.name, engine.install, engine.sign_in)
    })?;
    let messages = app.with_store(|store| store.messages(thread, 60)).unwrap_or_default();
    let mut history = with_cue(lines(app, thread, messages, bot), app, thread, bot);
    let text = match history.last() {
        Some((true, _)) => history.pop().map(|(_, text)| text).unwrap_or_default(),
        _ => String::new(),
    };
    let instructions = chat::system_prompt(bot, codex.as_deref(), app.memory.as_deref());
    let roots = std::iter::once(bot.folder.clone()).chain(app.memory.clone()).chain(codex).collect();
    let workspace = crate::tools::Workspace { folder: bot.folder.clone(), roots, mode: bot.mode };
    let session = match session {
        Session::Resume(id) => Some(id.clone()),
        Session::New(_) => None,
    };
    let model = bot.model.clone().unwrap_or_default();
    crate::acp::start(crate::acp::AcpTurn { engine, program, model, session, instructions, earlier: history, text, workspace }).map_err(|e| e.to_string())
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

/// Bots' names by id.
fn names(app: &App) -> std::collections::HashMap<String, String> {
    app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| (b.id, b.name)).collect()
}

/// A conversation as an AI's history: the person's words (and other bots', named, in a
/// channel) on one side, this bot's own replies on the other.
fn lines(app: &App, thread: &str, messages: Vec<crate::store::Message>, bot: &Bot) -> Vec<(bool, String)> {
    let names = names(app);
    let owner = owner_of(app, thread, bot);
    messages
        .into_iter()
        .filter_map(|m| {
            let speaker = m.detail.as_ref().and_then(|d| d["botId"].as_str()).map(str::to_string).unwrap_or_else(|| owner.clone());
            match m.role {
                Role::Person => Some((true, super::attachments::person_text(&m))),
                Role::Bot if m.body.trim().is_empty() => None,
                Role::Bot if speaker != bot.id => Some((true, format!("[{}]: {}", names.get(&speaker).map(String::as_str).unwrap_or("Another bot"), m.body))),
                Role::Bot => Some((false, m.body)),
                _ => None,
            }
        })
        .collect()
}

/// Where the last word may be another bot's (a channel, or a hand-off): a cue that it's this
/// bot's turn.
fn with_cue(mut history: Vec<(bool, String)>, app: &App, thread: &str, bot: &Bot) -> Vec<(bool, String)> {
    let channel = app.with_store(|store| store.conversation(thread)).flatten().and_then(|c| c.channel_id).is_some();
    let another_bot = history.last().is_some_and(|(person, text)| *person && text.starts_with('['));
    if another_bot || (channel && history.last().is_none_or(|(person, _)| !person)) {
        let place = if channel { "in the channel" } else { "in this chat" };
        history.push((true, format!("(It's your turn to answer {place}, as {}.)", bot.name)));
    }
    history
}

/// Whose a reply with no name on it is: the chat's own bot (a channel's: none in particular).
fn owner_of(app: &App, thread: &str, bot: &Bot) -> String {
    match app.with_store(|store| store.conversation(thread)).flatten() {
        Some(c) if c.channel_id.is_none() => c.bot_id,
        _ => bot.id.clone(),
    }
}

/// The chat before message `before`, for a session that starts without it: the last 30 of the
/// person's messages and the bot's replies, or nothing for a new chat.
fn so_far(app: &App, thread: &str, before: i64, bot: &Bot) -> String {
    let page = app.with_store(|store| store.page(thread, Some(before), 30).map(|(page, _)| page)).unwrap_or_default();
    let names = names(app);
    let owner = owner_of(app, thread, bot);
    let earlier: Vec<(String, String)> = page
        .into_iter()
        .filter_map(|m| {
            let speaker = m.detail.as_ref().and_then(|d| d["botId"].as_str()).map(str::to_string).unwrap_or_else(|| owner.clone());
            match m.role {
                Role::Person => Some(("Person".to_string(), super::attachments::person_text(&m))),
                Role::Bot if !m.body.trim().is_empty() => {
                    let who = if speaker == bot.id { "You".to_string() } else { names.get(&speaker).cloned().unwrap_or_else(|| "Another bot".into()) };
                    Some((who, m.body))
                }
                _ => None,
            }
        })
        .collect();
    if earlier.is_empty() {
        return String::new();
    }
    let mut said = String::from("<conversation_so_far>\n");
    for (who, text) in earlier {
        let short: String = text.chars().take(4000).collect();
        said.push_str(&format!("{who}: {short}\n\n"));
    }
    said.push_str("</conversation_so_far>\n\n");
    said
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
