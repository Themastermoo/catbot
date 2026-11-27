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
    /// The bot taking the turn (in a channel, whichever member it is).
    bot_id: String,
    answerer: Option<Answerer>,
    /// How to steer it, when its AI takes words mid-turn (stage 29.14).
    steer: Option<claude::Steer>,
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
    /// Whether any bot is in the middle of a turn (an update waits for none).
    pub fn any_busy(&self) -> bool {
        !self.running.lock().expect("turns lock").is_empty()
    }

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

    /// An open card's bot (the one asking) and message.
    pub fn card_of(&self, thread_id: &str, request_id: &str) -> Option<(String, i64)> {
        let running = self.running.lock().expect("turns lock");
        let handle = running.get(thread_id)?;
        handle.open_cards.get(request_id).map(|m| (handle.bot_id.clone(), *m))
    }

    /// Takes a card off the open list, returning its message id.
    fn close_card(&self, thread_id: &str, request_id: &str) -> Option<i64> {
        self.running.lock().expect("turns lock").get_mut(thread_id)?.open_cards.remove(request_id)
    }

    /// Every approval card waiting now: its chat, the bot asking, the request and the card's message.
    pub fn all_open_cards(&self) -> Vec<(String, String, String, i64)> {
        let running = self.running.lock().expect("turns lock");
        let mut cards: Vec<(String, String, String, i64)> = running
            .iter()
            .flat_map(|(thread, h)| h.open_cards.iter().map(move |(request, message)| (thread.clone(), h.bot_id.clone(), request.clone(), *message)))
            .collect();
        cards.sort_by_key(|c| c.3);
        cards
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

    /// How to steer the conversation's running turn, if it can be.
    pub fn steer(&self, thread_id: &str) -> Option<claude::Steer> {
        self.running.lock().expect("turns lock").get(thread_id)?.steer.clone()
    }

    fn set_steer(&self, thread_id: &str, steer: Option<claude::Steer>) {
        if let Some(handle) = self.running.lock().expect("turns lock").get_mut(thread_id) {
            handle.steer = steer;
        }
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
    let (turn_id, cancel) = register(&app, &conversation.id, &bot.id)?;
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
        app.doing.lock().expect("doing lock").remove(&thread_id);
        app.announce_bot(&bot.id);
        if let Some(reply) = reply.filter(|_| !next.is_empty()) {
            super::handoffs::after_reply(&app, &bot, &thread_id, request_id, &reply, next).await;
        }
        // What the person said meanwhile (stage 29.11).
        super::release_held(&app, &thread_id);
        // Board work that waited for this bot (stage 30.3).
        super::board::tick(&app);
    });
    Ok(turn_id)
}

/// What a tool says a bot is doing: writing code, designing, or looking things up. None
/// leaves what it was doing as it was.
pub fn doing_of(tool: &str) -> Option<&'static str> {
    let t = tool.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["blender", "comfy", "image", "picture", "draw", "design", "figma", "svg", "canvas", "render", "sketch"]) {
        Some("designing")
    } else if has(&["bash", "edit", "write", "patch", "command", "shell", "notebook", "terminal", "exec", "filechange"]) {
        Some("coding")
    } else if has(&["read", "grep", "glob", "search", "fetch", "web", "browse", "list", "find", "view"]) || t == "ls" {
        Some("researching")
    } else {
        None
    }
}

/// Runs a turn to its end, for bots answering one after another (a channel's, or a chain of
/// hand-offs), and gives back its reply. A `guest` is a bot answering in another bot's chat: it
/// starts afresh, shown the chat, and leaves that bot's session alone.
pub async fn run_through(app: &Arc<App>, bot: &Bot, conversation: Conversation, request_id: i64, text: &str, guest: bool) -> Result<Option<String>, &'static str> {
    let (turn_id, cancel) = register(app, &conversation.id, &bot.id)?;
    let thread_id = conversation.id.clone();
    let reply = run(app, bot, conversation, request_id, text, &turn_id, cancel, guest).await;
    app.turns.finish(&thread_id);
    app.doing.lock().expect("doing lock").remove(&thread_id);
    Ok(reply)
}

/// Marks a conversation busy with a new turn: its id and its stop signal.
fn register(app: &App, thread: &str, bot_id: &str) -> Result<(String, Arc<Notify>), &'static str> {
    let turn_id = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(Notify::new());
    let mut running = app.turns.running.lock().expect("turns lock");
    if running.contains_key(thread) {
        return Err("this bot is still working on the last message");
    }
    running.insert(thread.to_string(), Handle { turn_id: turn_id.clone(), bot_id: bot_id.to_string(), answerer: None, steer: None, cancel: cancel.clone(), open_cards: HashMap::new() });
    Ok((turn_id, cancel))
}

/// What one run of the CLI produced.
/// How long an AI that didn't answer gets before it's tried once more (stage 27).
const RETRY_SECONDS: u64 = 5;

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
    // What happens is kept as it's sent, for the bot panel's Activity and Inspector.
    let record = super::runs::Recorder::new();
    let began = std::time::Instant::now();
    let emit = |event: Value| {
        record.push(&event);
        app.hub.send(runtime(thread, turn_id, event))
    };
    // The Chief of Staff is told its role and the whole team, wherever it answers.
    let mut dressed = bot.clone();
    if let Some(chief) = super::chief::chief(app).filter(|c| c.id == bot.id) {
        dressed.instructions.push_str(&super::chief::note(app, &chief));
        // And the task board (stage 30.3).
        dressed.instructions.push_str(&super::board::chief_note(app));
    }
    // Its role and description (stage 23).
    dressed.instructions.push_str(&super::profiles::note(app, bot));
    // Its department: teammates, who leads it, the person's shared instructions, its memory note.
    dressed.instructions.push_str(&super::departments::note(app, bot));
    // The bot's skills, in its folder for this turn (and told where, for AIs that don't look).
    let told = super::skills::place(app, &dressed, &dressed.folder);
    dressed.instructions.push_str(&told);
    // Catbot's own tools for the turn (its mailbox, for one), on a key good until the turn ends.
    let open = super::bot_tools::open(app, &dressed, thread);
    let bot = &dressed;
    app.announce_bot(&bot.id);
    // Which bot answers, for "Kiwi is working…" in a chat with several (stage 29.12).
    emit(json!({ "type": "turn.started", "botId": bot.id }));

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
    let mut result = drive(app, bot, &session, &with_earlier(&session), codex.clone(), thread, &item_id, &cancel, &emit, open.as_ref(), !guest).await;
    let lost = result.outcome.as_ref().is_some_and(|o| !o.ok && o.result.as_deref().is_some_and(chat::says_lost))
        || result.errors.iter().any(|e| chat::says_lost(e));
    if matches!(session, Session::Resume(_)) && lost && !result.stopped {
        // Claude no longer has this conversation: carry on in a new session.
        let id = uuid::Uuid::new_v4().to_string();
        app.with_store(|store| store.set_session(thread, &id));
        let note = "Claude no longer had this conversation, so it continued in a fresh one.";
        save_and_send(app, thread, Role::Note, note, None);
        session = Session::New(id);
        result = drive(app, bot, &session, &with_earlier(&session), codex.clone(), thread, &item_id, &cancel, &emit, open.as_ref(), !guest).await;
    }

    // The bot's AI ran out of usage, credit or quota (any AI, Moo's ask on 2026-11-27): a backup
    // takes the turn, and the AI isn't tried again until the limit lifts.
    let mut ran_on = bot.clone();
    let said: String = result.errors.iter().cloned().chain(result.outcome.as_ref().and_then(|o| o.result.clone())).collect::<Vec<_>>().join(" ");
    if !result.stopped && result.reply.trim().is_empty() && super::backups::says_limit(&said) {
        let until = super::backups::lifts_at(&said).unwrap_or_else(|| super::backups::lifts_guess(&said));
        app.limited_until.lock().expect("limited lock").insert(bot.engine.clone(), until);
        super::usage::forget(app);
        if let Ok(Some((stand_in, why))) = super::backups::choose(app, bot) {
            save_and_send(app, thread, Role::Note, &why, None);
            if !guest {
                app.with_store(|store| store.clear_session(thread));
            }
            let fresh = Session::New(uuid::Uuid::new_v4().to_string());
            result = drive(app, &stand_in, &fresh, text, codex.clone(), thread, &item_id, &cancel, &emit, open.as_ref(), !guest).await;
            ran_on = stand_in;
        }
    }
    // The bot's own AI failed for another reason (stage 27): an outage is tried once more after
    // a few seconds; then the trouble is told (Inbox, Settings, a notification) and a backup
    // answers. An AI that answers is fine again.
    let failed = |r: &Result_| !r.stopped && r.reply.trim().is_empty() && r.steps.iter().all(|s| matches!(s, Step::Error { .. })) && !r.steps.is_empty();
    let words = |r: &Result_| r.errors.iter().cloned().chain(r.outcome.as_ref().and_then(|o| o.result.clone())).collect::<Vec<_>>().join(" ");
    if ran_on.engine == bot.engine
        && failed(&result)
        && let Some(engine) = engines::find(&bot.engine)
        && let Some(mut what) = super::health::classify(engine, &words(&result))
    {
        if what == "down" {
            save_and_send(app, thread, Role::Note, &format!("{} didn't answer, so Catbot is trying again in a few seconds.", engine.name), None);
            let waited = tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(RETRY_SECONDS)) => true,
                _ = cancel.notified() => false,
            };
            if waited {
                result = drive(app, bot, &session, &with_earlier(&session), codex.clone(), thread, &item_id, &cancel, &emit, open.as_ref(), !guest).await;
            }
            what = super::health::classify(engine, &words(&result)).unwrap_or(what);
        }
        if failed(&result) {
            super::health::broke(app, engine, what, &words(&result), &bot.id, thread);
            if let Ok((stand_in, why)) = super::backups::stand_in(app, bot, super::health::why(engine, what)) {
                save_and_send(app, thread, Role::Note, &why, None);
                if !guest {
                    app.with_store(|store| store.clear_session(thread));
                }
                let fresh = Session::New(uuid::Uuid::new_v4().to_string());
                result = drive(app, &stand_in, &fresh, text, codex.clone(), thread, &item_id, &cancel, &emit, open.as_ref(), !guest).await;
                ran_on = stand_in;
            }
        }
    }
    if !failed(&result) && !result.stopped {
        super::health::works(app, &ran_on.engine);
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
    // The Chief of Staff's proposed team changes come out too, each a card asking the person.
    let chief = super::chief::chief(app).filter(|c| c.id == bot.id);
    let mut proposed = Vec::new();
    let mut board_added = Vec::new();
    if chief.is_some() {
        let (reply, found) = super::team::take(&result.reply);
        result.reply = reply;
        proposed = found;
        // Work it puts on the task board (stage 30.3).
        let (reply, found) = super::board::take_added(&result.reply);
        result.reply = reply;
        board_added = found;
    }
    // In a task board item's chat, how the turn settles it.
    let mut settled = None;
    if super::board::is_item_thread(app, thread) {
        let (reply, found) = super::board::take(&result.reply);
        result.reply = reply;
        settled = found;
    }
    // Skills any bot offers come out the same way.
    let (reply, offered) = super::skills::take(&result.reply);
    result.reply = reply;
    // In the Team Chat, new pinned notes a bot proposes (stage 30.1).
    let mut notes = None;
    if super::team_chat::is_thread(app, thread) {
        let (reply, found) = super::team_chat::take(&result.reply);
        result.reply = reply;
        notes = found;
    }
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
        super::inbox::answered_in(app, thread);
    }
    for prompt in pictures {
        super::images::card(app, thread, &prompt, &bot.name);
    }
    if let Some(chief) = &chief {
        super::team::propose(app, thread, chief, proposed);
        if !board_added.is_empty() {
            super::board::add_from_chief(app, thread, chief, board_added);
        }
    }
    super::skills::propose(app, thread, bot, offered);
    if let Some(text) = &notes {
        super::team_chat::propose(app, thread, bot, text);
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
    let model = result.model.clone().or_else(|| bot.model.clone()).unwrap_or_default();
    let outcome = result.outcome.unwrap_or_default();
    emit(json!({
        "type": "turn.completed",
        "ok": outcome.ok && !result.stopped,
        "stopReason": if result.stopped { Some("interrupted".to_string()) } else { outcome.stop_reason.clone() },
        "cost": outcome.cost_usd,
        "denials": outcome.permission_denials.iter().filter_map(|d| d["tool_name"].as_str()).collect::<Vec<_>>(),
    }));
    let how = if result.stopped { "stopped" } else if outcome.ok && result.errors.is_empty() { "done" } else { "failed" };
    let tools = result.steps.iter().filter(|s| matches!(s, Step::Tool { .. })).count();
    record.finish(app, bot, thread, turn_id, how, &model, tools, result.errors.len(), outcome.cost_usd);
    // News for the Team Chat: a turn that failed, or took a while (stage 30.2).
    super::team_chat::after_turn(app, bot, thread, how, began.elapsed(), result.errors.first().map(String::as_str));
    super::board::turn_ended(app, bot, thread, how, if result.stopped { "" } else { &result.reply }, settled);
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
    tools: Option<&super::bot_tools::Open>,
    // Whether the AI's session becomes the chat's. Not a guest's (stage 29.11): a bot handed into
    // another bot's chat answering there must not leave its session for the chat's own bot to
    // resume (it then went on as the guest).
    keep_session: bool,
) -> Result_ {
    let mut result = Result_::default();
    let started = if bot.engine == engines::CLAUDE {
        let mut turn = chat::bot_turn(bot, text.to_string(), session.clone(), app.cli.clone(), app.helper.clone(), codex, app.memory.clone());
        turn.catbot_tools = tools.map(|t| t.access.clone());
        claude::start(&turn).await.map_err(|err| err.to_string())
    } else if engines::find(&bot.engine).is_some_and(|e| e.kind == engines::Kind::Agent) {
        start_agent(app, bot, thread, session, codex, tools)
    } else if bot.engine == engines::CODEX {
        start_codex(app, bot, thread, session, codex, tools, None)
    } else if bot.engine == engines::CHATGPT {
        // The ChatGPT plan (stage 29.10): a fresh token, and a model the plan offers.
        match app.chatgpt.access_token().await {
            Ok(token) => {
                let mut bot = bot.clone();
                if bot.model.as_deref().is_none_or(str::is_empty) {
                    bot.model = app.chatgpt.models().await.ok().and_then(|m| m.first().and_then(|f| f["id"].as_str().map(str::to_string)));
                }
                start_codex(app, &bot, thread, session, codex, tools, Some((token, app.chatgpt.resource().to_string())))
            }
            Err(problem) => Err(problem),
        }
    } else if bot.engine == engines::PI {
        start_pi(app, bot, thread, session, codex, tools)
    } else {
        start_api(app, bot, thread, codex, tools)
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
    // Whether the person can steer it (stage 29.14): the app shows Steer now on what they sent meanwhile.
    let steer = running.steer();
    emit(json!({ "type": "turn.steerable", "steerable": steer.is_some() }));
    app.turns.set_steer(thread, steer);

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
                // What the bot is at, for its desktop pet (stage 29.8).
                if let Some(kind) = doing_of(&name) {
                    let before = app.doing.lock().expect("doing lock").insert(thread.to_string(), kind);
                    if before != Some(kind) {
                        app.announce_bot(&bot.id);
                    }
                }
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
                // Its approval level as it is now, or an "Always allow" it was given: no card.
                if super::allowed::covers(app, &bot.id, &tool, &input) && app.turns.answer(thread, &id, Decision::Allow).is_some() {
                    emit(json!({ "type": "request.resolved", "requestId": id, "behavior": "allow", "source": "rule", "tool": tool, "summary": summary }));
                    continue;
                }
                emit(json!({ "type": "request.opened", "requestId": id, "requestType": "permission", "tool": tool, "summary": summary }));
                // The card the person answers, as the reference server draws it.
                let always = super::allowed::offer(&tool, &input);
                let mut options = vec!["Allow"];
                if always.is_some() {
                    options.push("Always allow");
                }
                options.push("Deny");
                let card = json!({
                    "title": "Approval needed",
                    "subtitle": summary,
                    "options": options,
                    "requestId": id,
                    "requestType": "permission",
                    "tool": tool,
                    "always": always,
                    "rules": super::allowed::rules_for(&tool, &input),
                });
                if let Some(message) = save_and_send(app, thread, Role::Card, &summary, Some(&json!({ "card": card })))
                    && let Some(message_id) = message["id"].as_str().and_then(wire::parse_message_id)
                {
                    app.turns.add_card(thread, &id, message_id);
                    app.announce_bot(&bot.id);
                    super::inbox::changed(app);
                    super::team_chat::waiting(app, bot, thread, &summary);
                }
            }
            Event::Error { message } => {
                emit(json!({ "type": "runtime.error", "message": message }));
                result.steps.push(Step::Error { message: message.clone() });
                result.errors.push(message);
            }
            Event::Finished(outcome) => {
                if let Some(id) = outcome.session_id.as_deref().filter(|id| !id.is_empty() && keep_session) {
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
fn start_api(app: &Arc<App>, bot: &Bot, thread: &str, codex: Option<std::path::PathBuf>, tools: Option<&super::bot_tools::Open>) -> Result<Running, String> {
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
    let workspace = crate::tools::Workspace { folder: bot.folder.clone(), roots, mode: bot.mode, catbot: tools.map(|t| (t.access.clone(), t.tools.clone())) };
    let effort = bot.effort.clone().filter(|e| engines::effort_levels(engine, &model).is_some_and(|l| l.contains(&e.as_str())));
    Ok(api::start(api::ApiTurn { engine, access, model, system, history, pictures, workspace, effort }))
}

/// Starts a turn on an agent program: the program found on this computer, the session to carry
/// on, and what a new session needs to know.
fn start_agent(app: &Arc<App>, bot: &Bot, thread: &str, session: &Session, codex: Option<std::path::PathBuf>, tools: Option<&super::bot_tools::Open>) -> Result<Running, String> {
    let engine = engines::find(&bot.engine).ok_or("no such agent")?;
    let program = engines::agent_program(engine).ok_or_else(|| match engine.download {
        Some(_) => format!("{} isn't installed yet: install it from this bot's settings.", engine.name),
        None => format!("{} isn't installed on this computer. Install it with `{}`, then sign in with `{}`.", engine.name, engine.install, engine.sign_in),
    })?;
    if engines::agent_signed_in(engine) == Some(false) {
        return Err(format!("{} isn't signed in: {}.", engine.name, engine.sign_in));
    }
    let env = engines::agent_env(engine, &program);
    let messages = app.with_store(|store| store.messages(thread, 60)).unwrap_or_default();
    let mut history = with_cue(lines(app, thread, messages, bot), app, thread, bot);
    let text = match history.last() {
        Some((true, _)) => history.pop().map(|(_, text)| text).unwrap_or_default(),
        _ => String::new(),
    };
    let instructions = chat::system_prompt(bot, codex.as_deref(), app.memory.as_deref());
    let roots = std::iter::once(bot.folder.clone()).chain(app.memory.clone()).chain(codex).collect();
    let workspace = crate::tools::Workspace { folder: bot.folder.clone(), roots, mode: bot.mode, catbot: tools.map(|t| (t.access.clone(), t.tools.clone())) };
    let session = match session {
        Session::Resume(id) => Some(id.clone()),
        Session::New(_) => None,
    };
    let model = bot.model.clone().unwrap_or_default();
    let mcp = tools.and_then(|t| super::bot_tools::acp_server(app, t)).into_iter().collect();
    let effort = bot.effort.clone();
    crate::acp::start(crate::acp::AcpTurn { engine, program, model, session, instructions, earlier: history, text, workspace, mcp, env, effort }).map_err(|e| e.to_string())
}

/// Starts a turn on Codex: the program, Catbot's Codex home with the person's sign-in linked in,
/// the thread to carry on, and what a new thread needs to know.
fn start_codex(
    app: &Arc<App>,
    bot: &Bot,
    thread: &str,
    session: &Session,
    codex: Option<std::path::PathBuf>,
    tools: Option<&super::bot_tools::Open>,
    plan: Option<(String, String)>,
) -> Result<Running, String> {
    let engine = engines::find(if plan.is_some() { engines::CHATGPT } else { engines::CODEX }).ok_or("no Codex")?;
    let program = engines::agent_program(engine)
        .ok_or_else(|| format!("Codex isn't installed on this computer. Install it with `{}`, then sign in from this bot's settings.", engine.install))?;
    let messages = app.with_store(|store| store.messages(thread, 60)).unwrap_or_default();
    let pictures = messages.iter().rev().find(|m| m.role == Role::Person).map(|m| super::attachments::picture_files(m, &bot.folder)).unwrap_or_default();
    let mut history = with_cue(lines(app, thread, messages, bot), app, thread, bot);
    let text = match history.last() {
        Some((true, _)) => history.pop().map(|(_, text)| text).unwrap_or_default(),
        _ => String::new(),
    };
    let instructions = chat::system_prompt(bot, codex.as_deref(), app.memory.as_deref());
    let roots = std::iter::once(bot.folder.clone()).chain(app.memory.clone()).chain(codex).collect();
    let workspace = crate::tools::Workspace { folder: bot.folder.clone(), roots, mode: bot.mode, catbot: tools.map(|t| (t.access.clone(), t.tools.clone())) };
    let session = match session {
        Session::Resume(id) => Some(id.clone()),
        Session::New(_) => None,
    };
    // The plan's turns have a Codex home of their own, with no Codex sign-in in it.
    let home = app
        .with_store(|store| Ok(if plan.is_some() { store.home().join("providers").join("chatgpt").join("codex") } else { crate::codex::bots_home(store.home()) }))
        .ok_or("no Catbot home")?;
    let model = bot.model.clone().unwrap_or_default();
    let mcp = tools.and_then(|t| super::bot_tools::acp_server(app, t));
    let effort = bot.effort.clone();
    crate::codex::start(crate::codex::CodexTurn { program, home, model, session, instructions, earlier: history, text, pictures, workspace, mcp, effort, plan })
}

/// Starts a turn on Pi: the program, Catbot's folder for this bot's Pi sessions, the session (a new
/// one gets the conversation so far), and Catbot's extension.
fn start_pi(app: &Arc<App>, bot: &Bot, thread: &str, session: &Session, codex: Option<std::path::PathBuf>, tools: Option<&super::bot_tools::Open>) -> Result<Running, String> {
    let engine = engines::find(engines::PI).ok_or("no Pi")?;
    let program = engines::agent_program(engine).ok_or_else(|| format!("Pi isn't installed on this computer. Install it with `{}`, then sign it in to a provider (`{}`).", engine.install, engine.sign_in))?;
    let messages = app.with_store(|store| store.messages(thread, 60)).unwrap_or_default();
    let pictures = messages.iter().rev().find(|m| m.role == Role::Person).map(|m| super::attachments::pictures(m, &bot.folder)).unwrap_or_default();
    let mut history = with_cue(lines(app, thread, messages, bot), app, thread, bot);
    let text = match history.last() {
        Some((true, _)) => history.pop().map(|(_, text)| text).unwrap_or_default(),
        _ => String::new(),
    };
    let instructions = chat::system_prompt(bot, codex.as_deref(), app.memory.as_deref());
    let roots = std::iter::once(bot.folder.clone()).chain(app.memory.clone()).chain(codex).collect();
    let workspace = crate::tools::Workspace { folder: bot.folder.clone(), roots, mode: bot.mode, catbot: tools.map(|t| (t.access.clone(), t.tools.clone())) };
    let (session, new) = match session {
        Session::Resume(id) => (id.clone(), false),
        Session::New(id) => (id.clone(), true),
    };
    let home = app.with_store(|store| Ok(store.home().join("pi"))).ok_or("no Catbot home")?;
    let model = bot.model.clone().unwrap_or_default();
    let mcp = tools.and_then(|t| super::bot_tools::acp_server(app, t));
    let effort = bot.effort.clone().filter(|e| crate::engines::EFFORT_WORDS.contains(&e.as_str()));
    crate::pi::start(crate::pi::PiTurn { program, folder: home.join(&bot.id), extension: home.join("catbot-pi.ts"), model, session, new, instructions, earlier: history, text, pictures, workspace, mcp, effort })
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
    super::inbox::changed(app);
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
    // The person wrote (not a trigger, not Catbot): what the chat sent them is answered.
    if role == Role::Person && detail.is_none_or(|d| d["fromCatbot"].is_null()) {
        super::inbox::person_spoke(app, thread);
    }
    // A card now asks the person something: it's in the Inbox.
    if detail.is_some_and(|d| d["state"] == "asking" || d["state"] == "choosing") {
        super::inbox::changed(app);
    }
    Some(wire)
}

#[cfg(test)]
mod tests {
    use super::doing_of;

    #[test]
    fn a_tool_says_what_the_bot_is_at() {
        assert_eq!(doing_of("Bash"), Some("coding"));
        assert_eq!(doing_of("Edit"), Some("coding"));
        assert_eq!(doing_of("mcp__blender__run_script"), Some("designing"));
        assert_eq!(doing_of("WebSearch"), Some("researching"));
        assert_eq!(doing_of("Read"), Some("researching"));
        assert_eq!(doing_of("ls"), Some("researching"));
        assert_eq!(doing_of("mcp__catbot__send_mail"), None);
    }
}
