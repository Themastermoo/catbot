//! A bot's conversation, read only: its header and transcript, with earlier messages loaded
//! as the person scrolls up.
//!
//! Rows follow the full layout of OpenMausBot's `src/components/ChatView.tsx`, `ActivityRun.tsx`,
//! `ToolActivity.tsx` and `DaySeparator.tsx` (Apache-2.0, see NOTICE), for the message kinds
//! Catbot sends. A run of tool steps folds into one line, as theirs does; their extra folding of
//! a finished turn under "Worked for" is not copied yet.

use dioxus::prelude::*;
use dioxus::html::HasFileData;
use serde_json::Value;

use crate::api;
use crate::attach::Attached;
use crate::bots::{Activity, Bot};
use crate::live::{self, Live, State};
use crate::markdown;
use crate::mascots::BotMark;

/// Messages fetched at a time.
const PAGE: usize = 50;
/// How close to the top (px) the person scrolls before earlier messages load.
const LOAD_EARLIER_AT: f64 = 160.0;
/// Their elevated card look (`ELEVATED_CARD`, `ELEVATED_FLOAT`).
const ELEVATED_FLOAT: &str = "bg-composer ring-[0.5px] ring-ink/10 shadow-(--shadow-float)";
/// Their composer: the floating surface on its own softer lift.
const ELEVATED_COMPOSER: &str = "bg-composer ring-[0.5px] ring-ink/10 shadow-(--shadow-composer) [transition:border-radius_300ms,box-shadow_150ms]";
/// Their simple bubble's width: at most 560px, 80% of the column, and 82px short of it.
const SIMPLE_BUBBLE_MAX: &str = "max-w-[min(var(--chat-bubble-max),var(--chat-bubble-pct),calc(100%-82px))]";
/// Their small round controls in the message box (`SIMPLE_CONTROL`).
const SIMPLE_CONTROL: &str = "flex size-7 shrink-0 items-center justify-center rounded-full bg-ink/[0.06] text-ink-tertiary ring-[0.5px] ring-inset ring-ink/[0.08] hover:bg-ink/[0.09] hover:text-ink disabled:opacity-40";

/// One message of the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub id: String,
    /// When it was sent, in ms since the epoch.
    pub at: f64,
    /// The person's own id for a message they sent, so the saved copy can replace the one shown
    /// while it was on its way.
    pub send_id: Option<String>,
    pub body: Body,
    /// Files it came with.
    pub attachments: Vec<Attached>,
    /// The person's reactions on it.
    pub reactions: Vec<String>,
    /// The bot that said it (a channel has several).
    pub bot_id: Option<String>,
    /// A message written earlier and sent at a set time: when it was for, and whether it went
    /// out late (stage 21).
    pub scheduled: Option<(f64, bool)>,
    /// Sent while the bot worked (stage 29.14): waiting for the turn to end, or steered into it.
    pub held: Held,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    No,
    /// Waiting for the turn to end (it can be steered in).
    Waiting,
    /// Steered into the turn while it ran.
    Steered,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Something the person wrote.
    Person(String),
    /// Something the bot wrote (markdown).
    Reply(String),
    /// A step of the bot's work: a tool it used, or a note. `ok` is None while it runs.
    Step { name: String, ok: Option<bool> },
    /// One bot handing the work to another (stage 29.12), by name: drawn as a chip with both.
    Handoff { from: String, to: String },
    /// A line about the team in the Team Chat (stage 30.2): whose, what kind ("handoff",
    /// "finished", "failed", "waiting"), and the chat it opens (`chat_of` owns `thread`).
    TeamUpdate { bot_id: String, kind: String, thread: String, chat_of: String, text: String },
    /// A picture being asked for and drawn (`pictures.rs`).
    Picture(Box<crate::pictures::Card>),
    TeamChange(Box<crate::team::Change>),
    /// What a bot did to one of its triggers (`triggers.rs`).
    Trigger(Box<Value>),
    /// Something Catbot wrote for the Chief of Staff: a problem to look at, or a check-in.
    FromCatbot { from: String, text: String },
    /// What a turn changed in the bot's folder, and whether it was undone.
    Changes { summary: String, files: Vec<crate::changes::FileChange>, undone: bool },
    /// An approval card: what the bot asked to do, and how the person answered.
    Card {
        title: String,
        subtitle: String,
        answer: Answer,
        /// The question's id, for answering it.
        request_id: Option<String>,
        /// The tool asked for ("Bash", "Write", ...).
        tool: Option<String>,
        /// The "Always allow" button's words, when this kind of question can be always allowed.
        always: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Waiting,
    Allowed,
    Denied,
    /// The turn ended before anyone answered.
    Dismissed,
}

impl Msg {
    /// A `WireMessage`, or None for a kind this view doesn't draw.
    pub fn from_wire(wire: &Value) -> Option<Msg> {
        let id = wire["id"].as_str()?.to_string();
        let at = wire["at"].as_f64().unwrap_or(0.0);
        let text = || wire["text"].as_str().unwrap_or_default().to_string();
        let body = match (wire["kind"].as_str()?, wire["role"].as_str()) {
            ("text", Some("user")) => match wire["fromCatbot"].as_str() {
                Some(from) => Body::FromCatbot { from: from.to_string(), text: text() },
                None => Body::Person(text()),
            },
            ("text", _) => Body::Reply(text()),
            ("activity", _) if wire["teamUpdate"].is_object() => {
                let update = &wire["teamUpdate"];
                let field = |k: &str| update[k].as_str().unwrap_or_default().to_string();
                let name = wire["tool"]["name"].as_str().unwrap_or_default();
                Body::TeamUpdate { bot_id: field("botId"), kind: field("kind"), thread: field("threadId"), chat_of: field("chatOf"), text: name.strip_prefix("notice: ").unwrap_or(name).to_string() }
            }
            ("activity", _) if wire["handoff"].is_object() => Body::Handoff {
                from: wire["handoff"]["from"].as_str().unwrap_or_default().to_string(),
                to: wire["handoff"]["to"].as_str().unwrap_or_default().to_string(),
            },
            ("activity", _) => {
                let name = wire["tool"]["name"].as_str().unwrap_or_default();
                Body::Step { name: name.strip_prefix("notice: ").unwrap_or(name).to_string(), ok: wire["tool"]["ok"].as_bool() }
            }
            ("image", _) => Body::Picture(Box::new(crate::pictures::Card::from_wire(&wire["image"]))),
            ("team-change", _) => Body::TeamChange(Box::new(crate::team::Change::from_wire(&wire["teamChange"]))),
            ("trigger", _) => Body::Trigger(Box::new(wire["trigger"].clone())),
            ("changes", _) => {
                let changes = &wire["changes"];
                Body::Changes {
                    summary: changes["summary"].as_str().unwrap_or("Changed files").to_string(),
                    files: crate::changes::files(&changes["files"]),
                    undone: changes["undone"].as_bool().unwrap_or(false),
                }
            }
            ("options", _) => {
                let card = &wire["card"];
                let answer = match card["answered"].as_str() {
                    Some("allow" | "always") if card["dismissed"] != true => Answer::Allowed,
                    Some(_) if card["dismissed"] == true => Answer::Dismissed,
                    Some(_) => Answer::Denied,
                    None if card["dismissed"] == true => Answer::Dismissed,
                    None => Answer::Waiting,
                };
                Body::Card {
                    title: card["title"].as_str().unwrap_or("Approval needed").to_string(),
                    subtitle: card["subtitle"].as_str().unwrap_or_default().to_string(),
                    answer,
                    request_id: card["requestId"].as_str().map(str::to_string),
                    tool: card["tool"].as_str().map(str::to_string),
                    always: card["always"].as_str().filter(|_| card["options"].as_array().is_some_and(|o| o.iter().any(|x| x == "Always allow"))).map(str::to_string),
                }
            }
            _ => return None,
        };
        let send_id = wire["sendId"].as_str().map(str::to_string);
        let attachments = Attached::list(&wire["attachments"]);
        let reactions = wire["reactions"].as_array().map(|r| r.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let bot_id = wire["botId"].as_str().map(str::to_string);
        let scheduled = wire["scheduled"]["for"].as_f64().map(|due| (due, wire["scheduled"]["late"] == true));
        let held = if wire["held"] == true {
            Held::Waiting
        } else if wire["steered"] == true {
            Held::Steered
        } else {
            Held::No
        };
        Some(Msg { id, at, send_id, body, attachments, reactions, bot_id, scheduled, held })
    }

    fn is_step(&self) -> bool {
        matches!(self.body, Body::Step { .. })
    }
}

/// What the transcript draws: single messages, and runs of two or more steps folded into a line.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Item {
    One(Msg),
    Run(Vec<Msg>),
}

impl Item {
    fn first(&self) -> &Msg {
        match self {
            Item::One(m) => m,
            Item::Run(steps) => &steps[0],
        }
    }

    fn key(&self) -> String {
        match self {
            Item::One(m) => m.id.clone(),
            Item::Run(steps) => format!("run:{}", steps[0].id),
        }
    }
}

/// Their `groupTranscript`, for Catbot's kinds: neighbouring steps on the same day fold together.
pub fn group(messages: &[Msg]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut run: Vec<Msg> = Vec::new();
    let flush = |run: &mut Vec<Msg>, items: &mut Vec<Item>| match run.len() {
        0 => {}
        1 => items.push(Item::One(run.remove(0))),
        _ => items.push(Item::Run(std::mem::take(run))),
    };
    for m in messages {
        if m.is_step() {
            if run.first().is_some_and(|first| day(first.at) != day(m.at)) {
                flush(&mut run, &mut items);
            }
            run.push(m.clone());
        } else {
            flush(&mut run, &mut items);
            items.push(Item::One(m.clone()));
        }
    }
    flush(&mut run, &mut items);
    items
}

/// Their `describeRun`: "3 steps · Read ×2, Bash".
pub fn describe_run(steps: &[Msg]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for step in steps {
        let Body::Step { name, .. } = &step.body else { continue };
        match counts.iter_mut().find(|(n, _)| n == name) {
            Some((_, count)) => *count += 1,
            None => counts.push((name.clone(), 1)),
        }
    }
    let names: Vec<String> = counts.iter().map(|(n, c)| if *c > 1 { format!("{n} ×{c}") } else { n.clone() }).collect();
    let mut tools = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > 3 {
        tools.push_str(&format!(" +{} more", names.len() - 3));
    }
    format!("{} steps · {tools}", steps.len())
}

/// The person's local day number for a time, so two times can be compared by day.
pub fn day(at: f64) -> i64 {
    let offset_ms = js_offset_minutes(at) * 60_000.0;
    ((at - offset_ms) / 86_400_000.0).floor() as i64
}

#[cfg(target_arch = "wasm32")]
fn js_offset_minutes(at: f64) -> f64 {
    js_sys::Date::new(&at.into()).get_timezone_offset()
}

/// In the desktop app, the computer's own time zone (minutes behind UTC, as JavaScript counts).
#[cfg(not(target_arch = "wasm32"))]
fn js_offset_minutes(at: f64) -> f64 {
    use chrono::{Local, Offset, TimeZone};
    match Local.timestamp_millis_opt(at as i64).single() {
        Some(when) => -f64::from(when.offset().fix().local_minus_utc()) / 60.0,
        None => 0.0,
    }
}

/// How close to the bottom (px) still counts as reading the newest messages, so new ones scroll into view.
const FOLLOW_WITHIN: f64 = 120.0;

#[component]
pub fn Chat(bot: Bot) -> Element {
    let thread = bot.thread_id.clone();
    let mut state = use_context::<Signal<State>>();
    // Reading replies aloud and voice chat (stage 22), one for each chat.
    let speaker = use_context_provider(|| crate::voice::Speaker::new(bot.id.clone()));
    let mut loading = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    // Whether the person is at the end of the conversation; new messages then scroll into view.
    let mut following = use_signal(|| true);

    // The newest page, whenever this conversation isn't loaded (first open, or after reconnecting).
    let first_thread = thread.clone();
    use_effect(move || {
        let loaded = state.read().threads.get(&first_thread).is_some_and(|t| t.loaded);
        if loaded || *loading.peek() {
            return;
        }
        let thread = first_thread.clone();
        loading.set(true);
        spawn(async move {
            match api::get(&format!("/api/threads/{thread}/messages?limit={PAGE}")).await {
                Ok(page) => {
                    state.write().loaded(&thread, page_messages(&page), page["hasMore"] == true, false);
                    problem.set(None);
                    following.set(true);
                }
                Err(err) => problem.set(Some(err)),
            }
            loading.set(false);
        });
    });

    // Earlier messages, kept in place on screen while the page grows above them.
    let older_thread = thread.clone();
    let load_earlier = move || {
        let thread = older_thread.clone();
        async move {
            let before = state.peek().threads.get(&thread).and_then(|t| t.messages.iter().find(|m| !m.id.starts_with("optimistic-")).map(|m| m.id.clone()));
            let Some(before) = before else { return };
            loading.set(true);
            let keep = document::eval(
                r#"const el = document.getElementById("chat-scroll");
                   const height = el.scrollHeight, top = el.scrollTop;
                   await dioxus.recv();
                   requestAnimationFrame(() => { el.scrollTop = el.scrollHeight - height + top; });"#,
            );
            match api::get(&format!("/api/threads/{thread}/messages?limit={PAGE}&before={before}")).await {
                Ok(page) => state.write().loaded(&thread, page_messages(&page), page["hasMore"] == true, true),
                Err(err) => problem.set(Some(err)),
            }
            let _ = keep.send(true);
            loading.set(false);
        }
    };

    let (messages, more, live, loaded) = {
        let state = state.read();
        match state.threads.get(&thread) {
            Some(t) => (t.messages.clone(), t.more, t.live.clone(), t.loaded),
            None => (Vec::new(), false, None, false),
        }
    };

    // New rows scroll into view while the person is at the end.
    let size = messages.len() + live.as_ref().map_or(0, |l| l.text.len() + l.steps.len() * 1000);
    use_effect(use_reactive!(|size| {
        let _ = size;
        if *following.peek() {
            document::eval(r#"requestAnimationFrame(() => requestAnimationFrame(() => { const el = document.getElementById("chat-scroll"); if (el) el.scrollTop = el.scrollHeight; }));"#);
        }
    }));

    // Each item, with the day line above it when it starts a new day.
    let today = day(js_now());
    let mut previous_day: Option<i64> = None;
    let rows: Vec<(Item, Option<String>)> = group(&messages)
        .into_iter()
        .map(|item| {
            let at = item.first().at;
            let this_day = day(at);
            let stamp = (previous_day != Some(this_day)).then(|| format!("{} {}", day_label(at, today), clock(at)));
            previous_day = Some(this_day);
            (item, stamp)
        })
        .collect();
    let working = bot.activity != Activity::Idle || live.is_some();
    // A chat where more than one bot talks (a channel, or guests a hand-off brought in): every
    // reply is named, and the header shows who's in it (stage 29.12).
    let guests = guests_of(&messages, &bot.id);
    let room = bot.channel.is_some() || !guests.is_empty();
    let faces: Vec<String> = match &bot.channel {
        Some(info) => info.members.iter().map(|m| m.0.clone()).collect(),
        None if room => std::iter::once(bot.id.clone()).chain(guests).collect(),
        None => Vec::new(),
    };
    // Whether what's waiting can be steered into the running turn (stage 29.14).
    let steerable = live.as_ref().is_some_and(|l| l.steerable);
    // The newest hand-off is still going while the chat works.
    let handing = messages.iter().rev().find(|m| matches!(m.body, Body::Handoff { .. })).map(|m| m.id.clone()).filter(|_| working);
    let replies: Vec<(String, String)> = messages.iter().filter_map(|m| match &m.body {
        Body::Reply(text) => Some((m.id.clone(), text.clone())),
        _ => None,
    }).collect();
    // Questions waiting for the person: unanswered cards, while the bot is actually waiting.
    // (A card left unanswered by a turn that's gone, say after a restart, isn't asking anymore.)
    let asking = bot.activity == Activity::WaitingOnYou;
    let pending: Vec<Pending> = if asking { pending_approvals(&messages) } else { Vec::new() };
    rsx! {
        main { class: "relative flex h-full min-w-0 flex-1 flex-col bg-app",
            ChatHead { bot: bot.clone(), faces }
            if let Some(info) = bot.channel.as_ref().filter(|c| c.team_chat) {
                PinnedNotes { text: info.instructions.clone() }
            }
            FindBar { key: "{bot.thread_id}" }
            div {
                id: "chat-scroll",
                class: "glass-scroller min-h-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-y-contain px-5 [overflow-anchor:none]",
                onscroll: move |e| {
                    let data = e.data();
                    let from_end = data.scroll_height() as f64 - data.scroll_top() - data.client_height() as f64;
                    following.set(from_end < FOLLOW_WITHIN);
                    if data.scroll_top() < LOAD_EARLIER_AT && more && !loading() {
                        spawn(load_earlier.clone()());
                    }
                },
                div {
                    // Not their `glass-scroller-content`: it sets the bottom padding from glass-bar sizes this view has no use for.
                    class: "chat-column relative flex w-full flex-col gap-3 pt-2 pb-8",
                    role: "log",
                    "aria-live": "off",
                    "aria-label": "Conversation with {bot.name}",
                    if loading() && loaded {
                        div { class: "flex items-center justify-center gap-2 py-3 text-xs text-ink-tertiary",
                            span { class: "inline-block h-3.5 w-3.5 animate-spin rounded-full border-2 border-accent border-t-transparent" }
                            span { "Loading earlier messages…" }
                        }
                    }
                    if let Some(err) = problem() {
                        div { role: "alert", class: "w-fit max-w-[min(var(--chat-bubble-max),var(--chat-bubble-pct))] rounded-xl border border-danger/30 bg-danger/10 px-3.5 py-2.5 text-[14px] text-danger",
                            "Couldn't load this conversation: {err}"
                        }
                    }
                    if loaded && messages.is_empty() && live.is_none() && problem().is_none() {
                        EmptyChat { bot: bot.clone() }
                    }
                    for (item, stamp) in rows {
                        div { key: "{item.key()}", class: "contents",
                            if let Some(stamp) = stamp {
                                div { class: "py-3 text-center text-[13px] text-ink-secondary", "{stamp}" }
                            }
                            {
                                match item {
                                    Item::One(m) => {
                                        let handing = handing.as_deref() == Some(m.id.as_str());
                                        rsx! { Row { message: m, bot_name: bot.name.clone(), asking, thread: bot.thread_id.clone(), bot_id: bot.id.clone(), working, speakers: bot.channel.clone(), room, handing, steerable } }
                                    }
                                    Item::Run(steps) => rsx! { Run { steps, bot_name: bot.name.clone(), asking } },
                                }
                            }
                        }
                    }
                    if let Some(live) = live {
                        LiveTurn { live, room, bot_id: bot.id.clone(), speakers: bot.channel.clone() }
                    } else if working {
                        Working {}
                    }
                }
            }
            crate::voice::Reader { replies, loaded }
            if (speaker.chat_open)() {
                crate::voice::VoiceChat { bot_id: bot.id.clone(), thread: bot.thread_id.clone(), name: bot.name.clone(), working }
            } else {
                Composer { bot: bot.clone(), working, pending, on_send: move |_| following.set(true) }
            }
        }
    }
}

/// The turn as it happens: its steps, then the reply streaming in.
#[component]
fn LiveTurn(
    live: Live,
    /// A chat with several bots: the reply is named, and so is who's working.
    #[props(default)]
    room: bool,
    #[props(default)] bot_id: String,
    #[props(default)] speakers: Option<crate::bots::ChannelInfo>,
) -> Element {
    let who = live.bot_id.clone().filter(|_| room);
    rsx! {
        div { class: "contents", "data-live-turn": true,
            for step in live.steps {
                Step { key: "{step.id}", name: step.name, ok: step.ok, live: true }
            }
            if live.text.is_empty() {
                Working { who }
            } else {
                Row { message: Msg { id: "live".into(), at: js_now(), send_id: None, body: Body::Reply(live.text), attachments: Vec::new(), reactions: Vec::new(), bot_id: live.bot_id, scheduled: None, held: Held::No }, bot_id, speakers, room, live: true }
            }
        }
    }
}

/// Their working pill: three dots, while the bot thinks. In a chat with several bots, `who`
/// (a bot's id) puts that bot's face and name on it: "Kiwi is working…" (stage 29.12).
#[component]
fn Working(#[props(default)] who: Option<String>) -> Element {
    let who = who.and_then(|id| speaker(&consume_context::<Signal<State>>().read(), &id));
    let label = who.as_ref().map_or("Working…".to_string(), |w| format!("{} is working…", w.name));
    rsx! {
        div { class: "flex justify-start",
            div { class: "ui-pill", role: "status", "aria-label": "{label}", "data-working-bot": who.as_ref().map(|w| w.name.clone()),
                if let Some(w) = &who {
                    BotMark { picture: w.picture.clone(), color: w.color.clone(), initial: w.initial(), size: 18, mark: w.mark.clone(), state: "working" }
                    span { class: "text-[13px] text-ink-secondary",
                        span { class: "bot-identity font-semibold", "data-bot-color": "{w.color}", "{w.name}" }
                        " is working"
                    }
                }
                span { class: "flex items-center gap-1", "aria-hidden": "true",
                    for i in 0..3 {
                        span { key: "{i}", class: "size-1.5 animate-pulse rounded-full bg-ink-secondary", style: "animation-delay: {i * 200}ms" }
                    }
                }
            }
        }
    }
}

/// The typing box, as their full-layout composer: a floating rounded box, Enter sends,
/// Shift+Enter starts a new line. While the bot works the box stays open; what's sent then goes
/// in the chat at once and the bot reads it when its turn ends (stage 29.11). Stop ends the turn.
#[component]
fn Composer(bot: Bot, working: bool, pending: Vec<Pending>, on_send: EventHandler<()>) -> Element {
    let mut state = use_context::<Signal<State>>();
    // This chat's draft, if one was left here (`drafts.rs`).
    let drafts = use_context::<Signal<crate::drafts::Drafts>>();
    let draft = use_hook(|| drafts.peek().get(&bot.thread_id).cloned().unwrap_or_default());
    let mut text = use_signal(|| draft.text.clone());
    let mut problem = use_signal(|| None::<String>);
    // Files picked for the next message, uploading or ready.
    let mut tray = use_signal(|| {
        draft.files.iter().map(|f| Tray { key: live::send_id(), name: f.name.clone(), state: TrayState::Ready(f.clone()) }).collect::<Vec<_>>()
    });
    // The box keeps its own text, so a draft is put back into it once it's there.
    let restored = draft.text.clone();
    use_effect(move || {
        if !restored.is_empty() {
            set_box(&restored);
        }
    });
    // Whatever's typed or attached is the chat's draft until it's sent.
    let draft_thread = bot.thread_id.clone();
    use_effect(move || {
        let files = tray.read().iter().filter_map(|t| match &t.state {
            TrayState::Ready(file) => Some(file.clone()),
            _ => None,
        }).collect();
        crate::drafts::keep(drafts, &draft_thread, crate::drafts::Draft { text: text(), files });
    });
    let waiting_on_you = bot.activity == Activity::WaitingOnYou;

    let add_bot = bot.id.clone();
    let add_files = use_callback(move |files: Vec<dioxus::html::FileData>| {
        for file in files {
            let bot_id = add_bot.clone();
            let key = live::send_id();
            let name = file.name();
            tray.write().push(Tray { key: key.clone(), name: name.clone(), state: TrayState::Uploading });
            spawn(async move {
                let mime = file.content_type().filter(|m| !m.is_empty()).unwrap_or_else(|| "application/octet-stream".into());
                let result = match file.read_bytes().await {
                    Ok(bytes) => crate::attach::upload(&bot_id, &name, &mime, bytes.to_vec()).await,
                    Err(err) => Err(format!("couldn't read {name}: {err}")),
                };
                if let Some(item) = tray.write().iter_mut().find(|t| t.key == key) {
                    item.state = match result {
                        Ok(attached) => TrayState::Ready(attached),
                        Err(err) => TrayState::Failed(err),
                    };
                }
            });
        }
    });

    let send_bot = bot.clone();
    let send = use_callback(move |(words, files): (String, Vec<Attached>)| {
        let bot = send_bot.clone();
        spawn(async move {
            let id = live::send_id();
            state.write().sending(&bot.thread_id, live::optimistic(&words, &id, js_now(), files.clone()));
            let ids: Vec<&str> = files.iter().map(|f| f.id.as_str()).collect();
            let body = serde_json::json!({ "text": words, "threadId": bot.thread_id, "sendId": id, "attachments": ids });
            if let Err(err) = api::post(&format!("/api/bots/{}/messages", bot.id), &body).await {
                state.write().unsend(&bot.thread_id, &id);
                problem.set(Some(err));
                // Put the files back too.
                for file in files {
                    tray.write().push(Tray { key: live::send_id(), name: file.name.clone(), state: TrayState::Ready(file) });
                }
                // Give the words back, ahead of anything typed since.
                let typed = text.peek().clone();
                let back = if typed.trim().is_empty() { words } else { format!("{words}\n{typed}") };
                set_box(&back);
                text.set(back);
            }
        });
    });

    // (A signal, so the submit stays copyable.)
    let image_target = use_signal(|| (bot.id.clone(), bot.thread_id.clone()));
    let mut submit = move || {
        let words = text.peek().trim().to_string();
        // `/image <what>`: a picture, whichever AI the person then picks to draw it.
        if let Some(prompt) = words.strip_prefix("/image").filter(|p| p.starts_with(char::is_whitespace)).map(str::trim).filter(|p| !p.is_empty()) {
            problem.set(None);
            text.set(String::new());
            set_box("");
            on_send.call(());
            let (bot_id, thread) = image_target.peek().clone();
            let (prompt, typed) = (prompt.to_string(), words.clone());
            spawn(async move {
                let body = serde_json::json!({ "prompt": prompt, "threadId": thread });
                if let Err(err) = api::post(&format!("/api/bots/{bot_id}/images"), &body).await {
                    problem.set(Some(err));
                    text.set(typed.clone());
                    set_box(&typed);
                }
            });
            return;
        }
        let uploading = tray.peek().iter().any(|t| t.state == TrayState::Uploading);
        let files: Vec<Attached> = tray.peek().iter().filter_map(|t| match &t.state {
            TrayState::Ready(file) => Some(file.clone()),
            _ => None,
        }).collect();
        if (words.is_empty() && files.is_empty()) || waiting_on_you {
            return;
        }
        if uploading {
            problem.set(Some("Wait a moment: a file is still uploading.".into()));
            return;
        }
        problem.set(None);
        text.set(String::new());
        tray.set(Vec::new());
        set_box("");
        on_send.call(());
        // Busy or not, it goes in the chat now; a busy bot reads it as soon as its turn ends
        // (the server holds it, stage 29.11).
        send((words, files));
    };

    // The clock beside Send: these words, at a time chosen (stage 21).
    let mut scheduling = use_signal(|| false);
    let later_target = use_signal(|| (bot.id.clone(), bot.thread_id.clone()));
    let send_later = move |at: f64| {
        scheduling.set(false);
        let words = text.peek().trim().to_string();
        if words.is_empty() {
            return;
        }
        if !tray.peek().is_empty() {
            problem.set(Some("A scheduled message carries words only: send the files now, or take them off first.".into()));
            return;
        }
        problem.set(None);
        text.set(String::new());
        set_box("");
        let (bot_id, thread) = later_target.peek().clone();
        spawn(async move {
            if let Err(err) = crate::scheduled::schedule(&bot_id, &thread, &words, at).await {
                problem.set(Some(err));
                text.set(words.clone());
                set_box(&words);
            }
        });
    };

    // The microphone (stage 22): its words go in the box, or are sent, as Settings › Voice says.
    let voice = use_context::<crate::voice::VoiceInfo>();
    let speaker = use_context::<crate::voice::Speaker>();
    let talk_on = voice.listen_on() && !waiting_on_you;
    let heard = move |words: String| {
        let typed = text.peek().trim().to_string();
        let next = if typed.is_empty() { words } else { format!("{typed} {words}") };
        text.set(next.clone());
        set_box(&next);
        if voice.send_at_once() {
            submit();
        } else {
            document::eval(r#"document.getElementById("composer-box")?.focus();"#);
        }
    };

    let stop_bot = bot.clone();
    let stop = use_callback(move |_: ()| {
        let bot = stop_bot.clone();
        spawn(async move {
            let body = serde_json::json!({ "threadId": bot.thread_id });
            if let Err(err) = api::post(&format!("/api/bots/{}/interrupt", bot.id), &body).await {
                problem.set(Some(err));
            }
        });
    });

    let placeholder = if waiting_on_you {
        "Answer the approval above to continue".to_string()
    } else if working {
        format!("{} is working. Sends when this turn finishes", bot.name)
    } else if let Some(info) = &bot.channel {
        format!("Message {} · @Name asks one bot, otherwise {} answers", bot.name, info.lead_name())
    } else {
        format!("Message {}", bot.name)
    };
    // In a channel, typing @ offers the members whose names start with what follows it.
    let mentions: Vec<(String, String, String, String)> = match &bot.channel {
        Some(info) => {
            let typed = text.read().clone();
            match typed.rsplit([' ', '\n']).next().filter(|w| w.starts_with('@')) {
                Some(word) => {
                    let start = word[1..].to_lowercase();
                    let mut found: Vec<_> = info.members.iter().filter(|m| m.1.to_lowercase().starts_with(&start)).cloned().collect();
                    if "everyone".starts_with(&start) && info.members.len() > 1 {
                        found.push(("everyone".into(), "everyone".into(), String::new(), String::new()));
                    }
                    found
                }
                None => Vec::new(),
            }
        }
        None => Vec::new(),
    };
    // Typing / at the start offers the bot's skills (and /image).
    let commands: Vec<String> = {
        let typed = text.read().clone();
        match typed.strip_prefix('/').filter(|rest| !rest.contains(char::is_whitespace)) {
            Some(start) if bot.channel.is_none() => std::iter::once("image".to_string())
                .chain(bot.skills.iter().cloned())
                .filter(|name| name.starts_with(start) && name != start)
                .collect(),
            _ => Vec::new(),
        }
    };
    let has_words = !text.read().trim().is_empty();
    let has_text = has_words || tray.read().iter().any(|t| matches!(t.state, TrayState::Ready(_)));
    let pick = move |e: FormEvent| {
        add_files(e.files());
        // Picking the same file again should work too.
        document::eval(r#"const input = document.getElementById("composer-file"); if (input) input.value = "";"#);
    };
    let button_style = format!("--bot-button: var(--identity-{}); --bot-button-ink: #ffffff; --bot-button-hover: color-mix(in srgb, var(--identity-{}) 88%, #000000);", bot.color, bot.color);
    rsx! {
        div { class: "pointer-events-none relative shrink-0 px-5 pb-3",
            div { class: "chat-column pointer-events-auto relative w-full",
                if let Some(err) = problem() {
                    div { role: "alert", class: "mb-2 flex items-center gap-2 rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-[13px] text-danger",
                        span { class: "min-w-0 flex-1", "{err}" }
                        button { r#type: "button", class: "flex size-5 shrink-0 items-center justify-center rounded hover:bg-danger/10", "aria-label": "Dismiss", onclick: move |_| problem.set(None), "×" }
                    }
                }
                if let Some(question) = pending.first() {
                    ApprovalPanel { key: "{question.request_id}", bot: bot.clone(), pending: question.clone(), count: pending.len(), on_cancel_turn: move |_| stop(()) }
                }
                crate::scheduled::List { bot_id: bot.id.clone(), thread_id: bot.thread_id.clone() }
                div { class: "relative",
                    div { "aria-hidden": "true", class: "pointer-events-none absolute -left-5 -right-5 -bottom-3 top-1/2 bg-app" }
                    div {
                        "data-composer": true,
                        class: "composer-shell relative z-[1] rounded-[22px] p-2 has-[textarea:focus]:ring-ink/[0.22] hover:ring-ink/[0.22] {ELEVATED_COMPOSER}",
                        ondragover: move |e| e.prevent_default(),
                        ondrop: move |e| {
                            e.prevent_default();
                            add_files(e.files());
                        },
                        if !tray.read().is_empty() {
                            div { class: "flex flex-wrap gap-1.5 px-1 pb-1.5 pt-1", "data-composer-tray": true,
                                for item in tray.read().iter().cloned() {
                                    {
                                        let key = item.key.clone();
                                        let (note, tone) = match &item.state {
                                            TrayState::Uploading => ("uploading…".to_string(), "text-ink-tertiary"),
                                            TrayState::Ready(file) => (crate::attach::size_words(file.size), "text-ink-tertiary"),
                                            TrayState::Failed(err) => (err.clone(), "text-danger"),
                                        };
                                        rsx! {
                                            span { key: "{key}", "data-tray-file": true, class: "flex max-w-[260px] items-center gap-1.5 rounded-lg bg-raised px-2 py-1 text-[12px] text-ink",
                                                span { class: "min-w-0 truncate", "{item.name}" }
                                                span { class: "shrink-0 truncate {tone}", "{note}" }
                                                button {
                                                    r#type: "button",
                                                    "aria-label": "Remove {item.name}",
                                                    class: "flex size-4 shrink-0 items-center justify-center rounded text-ink-secondary hover:bg-control",
                                                    onclick: move |_| tray.write().retain(|t| t.key != key),
                                                    "×"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if !commands.is_empty() {
                            div { class: "flex flex-wrap gap-1.5 px-1 pb-1.5 pt-1", "data-commands": true,
                                for name in commands {
                                    button {
                                        key: "{name}",
                                        r#type: "button",
                                        "data-command": "{name}",
                                        class: "rounded-full bg-raised px-2.5 py-0.5 font-mono text-[12px] text-ink hover:bg-control",
                                        onclick: move |_| {
                                            let next = format!("/{name} ");
                                            text.set(next.clone());
                                            set_box(&next);
                                            document::eval(r#"document.getElementById("composer-box")?.focus();"#);
                                        },
                                        "/{name}"
                                    }
                                }
                            }
                        }
                        if !mentions.is_empty() {
                            div { class: "flex flex-wrap gap-1.5 px-1 pb-1.5 pt-1", "data-mentions": true,
                                for (id, name, _, _) in mentions {
                                    button {
                                        key: "{id}",
                                        r#type: "button",
                                        "data-mention": "{name}",
                                        class: "rounded-full bg-raised px-2.5 py-0.5 text-[12px] text-ink hover:bg-control",
                                        onclick: move |_| {
                                            let typed = text.peek().clone();
                                            let cut = typed.rfind(['@']).unwrap_or(typed.len());
                                            let next = format!("{}@{} ", &typed[..cut], name);
                                            text.set(next.clone());
                                            set_box(&next);
                                            document::eval(r#"document.getElementById("composer-box")?.focus();"#);
                                        },
                                        "@{name}"
                                    }
                                }
                            }
                        }
                        div { class: "flex items-end gap-2",
                            input { id: "composer-file", r#type: "file", multiple: true, class: "hidden", onchange: pick }
                            // Their +: with one thing to add (files), it adds it straight away.
                            span { class: "flex h-8 shrink-0 items-center",
                            button {
                                r#type: "button",
                                "aria-label": "Add photos & files",
                                title: "Add photos & files",
                                "data-attach": true,
                                disabled: waiting_on_you,
                                class: "{SIMPLE_CONTROL}",
                                onclick: move |_| { document::eval(r#"document.getElementById("composer-file")?.click();"#); },
                                crate::sidebar::Icon { name: "plus", size: 16 }
                            }
                            }
                            textarea {
                                id: "composer-box",
                                rows: 1,
                                // The box keeps its own text; the program only listens (and clears it after
                                // sending). Writing it back on every key loses keys in the desktop app, where
                                // each key's round trip takes longer than the next key.
                                disabled: waiting_on_you,
                                placeholder: "{placeholder}",
                                "aria-label": "Message {bot.name}",
                                class: "block max-h-[9rem] min-h-8 w-full resize-none overflow-y-auto bg-transparent px-0.5 py-1 text-[15px] leading-6 placeholder:text-ink-tertiary focus:outline-none",
                                oninput: move |e| {
                                    text.set(e.value());
                                    document::eval(RESIZE_BOX);
                                },
                                onkeydown: move |e| {
                                    if e.key() == Key::Enter && !e.modifiers().shift() && !e.data().is_composing() {
                                        e.prevent_default();
                                        submit();
                                    }
                                },
                            }
                            div { class: "flex h-8 items-center gap-2",
                                if bot.channel.is_none() {
                                    crate::model_menu::UsageGauge { bot: bot.clone(), working }
                                }
                                if working {
                                    button {
                                        r#type: "button",
                                        "aria-label": "Stop this turn",
                                        title: "Stop",
                                        class: "{SIMPLE_CONTROL}",
                                        onclick: move |_| stop(()),
                                        svg { width: "11", height: "11", view_box: "0 0 24 24", fill: "currentColor", rect { x: "3", y: "3", width: "18", height: "18", rx: "3" } }
                                    }
                                }
                                if talk_on {
                                    crate::voice::MicButton { disabled: false, on_words: heard, on_problem: move |p: String| problem.set(Some(p)) }
                                    button {
                                        r#type: "button",
                                        "aria-label": "Voice chat",
                                        title: "Voice chat",
                                        "data-voice-chat-open": true,
                                        class: "{SIMPLE_CONTROL}",
                                        onclick: move |_| { let mut open = speaker.chat_open; open.set(true); },
                                        crate::sidebar::Icon { name: "audio-lines", size: 15 }
                                    }
                                }
                                if !waiting_on_you {
                                    span { class: "relative flex items-center",
                                        button {
                                            r#type: "button",
                                            "aria-label": "Send later",
                                            "aria-haspopup": "menu",
                                            title: "Send later",
                                            "data-schedule": true,
                                            disabled: !has_words,
                                            class: "{SIMPLE_CONTROL}",
                                            onclick: move |_| scheduling.toggle(),
                                            crate::sidebar::Icon { name: "clock", size: 15 }
                                        }
                                        if scheduling() {
                                            crate::scheduled::Picker { on_pick: send_later, on_close: move |_| scheduling.set(false) }
                                        }
                                    }
                                    button {
                                        r#type: "button",
                                        "aria-label": if working { "Send when this turn finishes" } else { "Send" },
                                        title: if working { "Sends when this turn finishes" } else { "Send" },
                                        style: if working { "" } else { "{button_style}" },
                                        class: if working { "flex size-8 shrink-0 items-center justify-center rounded-full bg-raised text-ink-secondary hover:bg-raised-hover disabled:opacity-40" } else { "bot-button flex size-8 shrink-0 items-center justify-center rounded-full disabled:opacity-40" },
                                        disabled: !has_text,
                                        "data-send": true,
                                        onclick: move |_| submit(),
                                        if working {
                                            svg { width: "15", height: "15", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                                circle { cx: "12", cy: "12", r: "10" }
                                                path { d: "M12 6v6l4 2" }
                                            }
                                        } else {
                                            svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.75", stroke_linecap: "round", stroke_linejoin: "round",
                                                path { d: "m5 12 7-7 7 7" }
                                                path { d: "M12 19V5" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A file picked for the next message.
#[derive(Clone, Debug, PartialEq)]
struct Tray {
    key: String,
    name: String,
    state: TrayState,
}

#[derive(Clone, Debug, PartialEq)]
enum TrayState {
    Uploading,
    Ready(Attached),
    Failed(String),
}

/// Puts text in the typing box (and fits its height to it).
fn set_box(text: &str) {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    document::eval(&format!(r#"const box = document.getElementById("composer-box"); if (box) {{ box.value = {text}; }} {RESIZE_BOX}"#));
}

/// The typing box grows with its text, up to its max height.
const RESIZE_BOX: &str = r#"requestAnimationFrame(() => { const box = document.getElementById("composer-box"); if (box) { box.style.height = "auto"; box.style.height = box.scrollHeight + "px"; } });"#;

fn page_messages(page: &Value) -> Vec<Msg> {
    page["messages"].as_array().map(|list| list.iter().filter_map(Msg::from_wire).collect()).unwrap_or_default()
}

/// The bar above the conversation: the bot's picture and name, in their floating pill.
/// Clicking it opens the bot's settings, as theirs opens the profile.
#[component]
fn ChatHead(
    bot: Bot,
    /// Who's in a chat with several bots (stage 29.12): their faces sit beside the name.
    #[props(default)]
    faces: Vec<String>,
) -> Element {
    let mut ui = use_context::<crate::settings::Ui>();
    let state = use_context::<Signal<State>>();
    let faces: Vec<Speaker> = faces.iter().filter_map(|id| speaker(&state.read(), id)).collect();
    let shown = faces.len().min(FACES_SHOWN);
    let more = faces.len() - shown;
    let who = faces.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
    let open = (ui.settings_open)();
    // Which chat this is, once the bot has more than one (their `ChatHeadPill`).
    let conversation = (bot.chats.len() > 1).then(|| bot.open_chat().map(|c| c.title.clone()).unwrap_or_else(|| "New chat".into()));
    let label = match &conversation {
        Some(chat) => format!("Open settings for {}, {chat}", bot.name),
        None => format!("Open settings for {}", bot.name),
    };
    let elsewhere = bot.busy_elsewhere();
    let bot_id = bot.id.clone();
    rsx! {
        div { class: "relative z-[25] shrink-0 bg-app/85 px-5 py-3 backdrop-blur-md",
            div { class: "flex items-center justify-center px-10",
                button {
                    r#type: "button",
                    "data-chathead-pill": true,
                    title: if bot.channel.is_some() { "Open channel settings" } else { "Open bot settings" },
                    "aria-label": "{label}",
                    "aria-expanded": "{open}",
                    class: "group flex min-w-0 max-w-full items-center gap-2 rounded-full py-0.5 pl-1.5 pr-3 text-ink {ELEVATED_FLOAT} transition-colors duration-[120ms] hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)]",
                    onclick: move |_| ui.settings_open.toggle(),
                    BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 24, mark: bot.mark.clone(), state: bot.activity.mood() }
                    span { class: "truncate text-[14px] font-semibold text-ink", "{bot.name}" }
                    if let Some(chat) = conversation {
                        svg { "aria-hidden": "true", class: "shrink-0 text-ink-tertiary", width: "12", height: "12", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                            path { d: "m9 18 6-6-6-6" }
                        }
                        span { "data-chathead-conversation": true, dir: "auto", class: "min-w-0 max-w-[180px] truncate text-[14px] font-medium leading-5 text-ink-secondary", "{chat}" }
                    }
                    if !faces.is_empty() {
                        span { "data-chathead-faces": true, class: "ml-1 flex shrink-0 items-center", title: "In this chat: {who}", "aria-label": "In this chat: {who}",
                            for (i, face) in faces.into_iter().take(shown).enumerate() {
                                span { key: "{face.id}", class: "inline-flex rounded-full ring-2 ring-app", style: if i > 0 { "margin-left:-6px" } else { "" },
                                    BotMark { picture: face.picture.clone(), color: face.color.clone(), initial: face.initial(), size: 20, mark: face.mark.clone() }
                                }
                            }
                            if more > 0 {
                                span { class: "ml-1 text-[12px] tabular-nums text-ink-tertiary", "+{more}" }
                            }
                        }
                    }
                    if elsewhere {
                        span { role: "img", "data-chathead-elsewhere": true, "aria-label": "Another chat is busy", title: "Another chat is busy", class: "size-1.5 shrink-0 rounded-full bg-accent" }
                    }
                }
            }
            // On a phone: back to the bot list.
            button {
                r#type: "button",
                "data-back-to-list": true,
                "aria-label": "Back to bots",
                class: "absolute left-3 top-1/2 flex size-9 -translate-y-1/2 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink md:hidden",
                onclick: move |_| ui.showing_chat.set(false),
                svg { "aria-hidden": "true", width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "m15 18-6-6 6-6" }
                }
            }
            // The right side (their header): the model pill, the bot's computer, a new chat.
            div { class: "absolute right-4 top-1/2 flex -translate-y-1/2 items-center gap-2",
                if bot.channel.is_none() {
                    crate::model_menu::ModelPill { bot: bot.clone() }
                    ComputerButton { bot_id: bot.id.clone(), name: bot.name.clone() }
                }
            button {
                r#type: "button",
                "data-new-chat": true,
                "aria-label": "New chat with {bot.name}",
                title: "New chat (Ctrl+Shift+N)",
                class: "flex size-8 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink",
                onclick: move |_| crate::chats::start(state, ui, bot_id.clone()),
                // Lucide `SquarePen` (ISC).
                svg { "aria-hidden": "true", width: "17", height: "17", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" }
                    path { d: "M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z" }
                }
            }
            }
        }
    }
}

/// The find bar's search (in the page, so it can mark text): marks every match in the chat with
/// the browser's highlights and scrolls to one; `step` 0 starts at the newest, -1 and 1 move.
/// Returns `[which, of]`, counted from the oldest.
const FIND_JS: &str = r#"if (!window.catbotFind) window.catbotFind = function (q, step) {
  const root = document.getElementById('chat-scroll');
  const hl = (typeof CSS !== 'undefined' && CSS.highlights && typeof Highlight !== 'undefined') ? CSS.highlights : null;
  if (hl) { hl.delete('catbot-find'); hl.delete('catbot-find-now'); }
  document.querySelectorAll('[data-find-now]').forEach(e => e.removeAttribute('data-find-now'));
  q = (q || '').trim().toLowerCase();
  if (!root || !q) { window.catbotFindAt = 0; return [0, 0]; }
  const ranges = [];
  const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, { acceptNode(n) {
    const p = n.parentElement;
    return !p || p.closest('button, [aria-hidden="true"], .sr-only') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
  } });
  let n;
  while ((n = walk.nextNode())) {
    const t = n.nodeValue.toLowerCase();
    for (let i = t.indexOf(q); i >= 0; i = t.indexOf(q, i + q.length)) {
      const r = new Range(); r.setStart(n, i); r.setEnd(n, i + q.length); ranges.push(r);
    }
  }
  const of = ranges.length;
  if (!of) { window.catbotFindAt = 0; return [0, 0]; }
  let at = window.catbotFindAt || 0;
  if (step === 0 || at < 1 || at > of) at = of; else { at += step; if (at < 1) at = of; if (at > of) at = 1; }
  window.catbotFindAt = at;
  const now = ranges[at - 1];
  if (hl) { hl.set('catbot-find', new Highlight(...ranges)); hl.set('catbot-find-now', new Highlight(now)); }
  const el = now.startContainer.parentElement;
  if (el) { if (!hl) el.setAttribute('data-find-now', ''); el.scrollIntoView({ block: 'center' }); }
  return [at, of];
};"#;

/// Find in conversation: a bar under the header that marks the words wherever they're shown in
/// the chat (the browser's highlights) and steps through them, the newest first.
#[component]
fn FindBar() -> Element {
    let mut ui = use_context::<crate::settings::Ui>();
    let mut found = use_signal(|| (0usize, 0usize));
    let Some(query) = (ui.find)() else {
        return rsx! {};
    };
    let look = move |step: i32| {
        let q = (ui.find)().unwrap_or_default();
        spawn(async move {
            let script = format!("{FIND_JS}\nreturn catbotFind({}, {step});", serde_json::to_string(&q).unwrap_or_default());
            if let Ok(answer) = document::eval(&script).await {
                let at = answer.get(0).and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;
                let of = answer.get(1).and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;
                found.set((at, of));
            }
        });
    };
    let mut close = move || {
        let _ = document::eval("window.catbotFind && catbotFind('', 0);");
        ui.find.set(None);
    };
    let (at, of) = found();
    rsx! {
        div { class: "relative z-[24] mx-auto mb-1 flex w-[min(520px,calc(100%-2.5rem))] items-center gap-2 rounded-full bg-composer px-3 py-1.5 ring-1 ring-inset ring-ink/[0.07] shadow-(--shadow-float)", "data-find-bar": true,
            span { class: "shrink-0 text-ink-tertiary", crate::sidebar::Icon { name: "search", size: 15 } }
            input {
                r#type: "text",
                "aria-label": "Find in conversation",
                placeholder: "Find in conversation",
                autofocus: true,
                class: "min-w-0 flex-1 bg-transparent text-[14px] text-ink placeholder:text-ink-tertiary focus:outline-none",
                value: "{query}",
                oninput: move |e| {
                    ui.find.set(Some(e.value()));
                    look(0);
                },
                onkeydown: move |e| match e.key() {
                    Key::Enter => look(if e.modifiers().shift() { 1 } else { -1 }),
                    Key::Escape => close(),
                    _ => {}
                },
                onmounted: move |e| async move { let _ = e.set_focus(true).await; },
            }
            span { class: "shrink-0 text-[12px] tabular-nums text-ink-tertiary", "data-find-count": true,
                if query.trim().is_empty() { "" } else if of == 0 { "None" } else { "{at} of {of}" }
            }
            button { r#type: "button", "aria-label": "Earlier", title: "Earlier (Enter)", class: "flex size-6 items-center justify-center rounded-full text-ink-secondary hover:bg-ink/[0.06] disabled:opacity-40", disabled: of == 0, onclick: move |_| look(-1),
                svg { "aria-hidden": "true", width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", path { d: "m18 15-6-6-6 6" } }
            }
            button { r#type: "button", "aria-label": "Later", title: "Later (Shift+Enter)", class: "flex size-6 items-center justify-center rounded-full text-ink-secondary hover:bg-ink/[0.06] disabled:opacity-40", disabled: of == 0, onclick: move |_| look(1),
                svg { "aria-hidden": "true", width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", path { d: "m6 9 6 6 6-6" } }
            }
            button { r#type: "button", "aria-label": "Close find", class: "flex size-6 items-center justify-center rounded-full text-ink-secondary hover:bg-ink/[0.06]", onclick: move |_| close(),
                crate::sidebar::Icon { name: "x", size: 14 }
            }
        }
    }
}

/// The round computer button in the chat's header: the bot's screen, live, when it has a
/// computer; otherwise Settings › Computers & secrets, where one is given.
#[component]
fn ComputerButton(bot_id: String, name: String) -> Element {
    let ui = use_context::<crate::settings::Ui>();
    let mut showing = use_signal(|| None::<bool>);
    let id = bot_id.clone();
    rsx! {
        button {
            r#type: "button",
            "data-open-computer": true,
            "aria-label": "{name}'s computer",
            title: "{name}'s computer",
            class: "flex size-9 shrink-0 items-center justify-center rounded-full bg-composer text-ink/90 ring-1 ring-inset ring-ink/[0.07] shadow-(--shadow-float) hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)] max-md:hidden",
            onclick: move |_| {
                let id = id.clone();
                spawn(async move {
                    let list = crate::api::get("/api/computers").await.ok();
                    let entry = list.as_ref().and_then(|l| l["bots"].as_array()).and_then(|all| all.iter().find(|e| e["botId"] == id.as_str()).cloned());
                    match entry {
                        Some(entry) => showing.set(Some(entry["kind"] == "boat")),
                        None => {
                            let mut prefs = ui.prefs;
                            prefs.set(Some("computers".into()));
                        }
                    }
                });
            },
            crate::sidebar::Icon { name: "monitor", size: 16 }
        }
        if let Some(at_boat) = showing() {
            crate::computers::ComputerView { bot_id: bot_id.clone(), name: name.clone(), at_boat, on_close: move |_| showing.set(None) }
        }
    }
}

#[component]
fn EmptyChat(bot: Bot) -> Element {
    rsx! {
        div { class: "flex flex-col items-center gap-3 py-24 text-center",
            BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 64, mark: bot.mark.clone(), state: bot.activity.mood() }
            p { class: "text-[15px] font-semibold text-ink", "{bot.name}" }
            p { class: "text-[13px] text-ink-secondary", "Send a message to start the conversation." }
        }
    }
}

/// One message: the person's bubble on the right, the bot's reply on the left, a step, or a card.
#[component]
fn Row(
    message: Msg,
    #[props(default)] bot_name: String,
    #[props(default)] asking: bool,
    #[props(default)] thread: String,
    #[props(default)] bot_id: String,
    #[props(default)] working: bool,
    /// In a channel: its members, to name who said each reply.
    #[props(default)]
    speakers: Option<crate::bots::ChannelInfo>,
    /// A chat where more than one bot talks: every reply carries its speaker's name (stage 29.12).
    #[props(default)]
    room: bool,
    /// A hand-off still going (the chat's newest, while it works).
    #[props(default)]
    handing: bool,
    /// The reply streaming in now.
    #[props(default)]
    live: bool,
    /// The running turn takes words mid-turn: a waiting message gets Steer now (stage 29.14).
    #[props(default)]
    steerable: bool,
) -> Element {
    let when = clock(message.at);
    let full_time = full_date(message.at);
    // Your own messages wear the bot's colour (their default; Settings › Appearance turns it off).
    let looks = use_context::<Signal<crate::looks::Looks>>();
    let colour = use_context::<Signal<State>>().read().bots.iter().find(|b| b.id == bot_id).map(|b| b.color.clone());
    // In simple mode theirs is one even fill (no sheen), with white ink on every colour.
    let (own_class, own_style) = match colour.filter(|_| looks.read().color_mine) {
        Some(c) => ("text-ink antialiased user-bubble-colored".to_string(), format!("background-color: var(--identity-{c}); --color-bubble-user: var(--identity-{c}); --color-ink: #ffffff; --color-ink-secondary: rgb(255 255 255 / 0.82); --color-ink-tertiary: rgb(255 255 255 / 0.7)")),
        None => ("bg-bubble-user text-ink".to_string(), String::new()),
    };
    match &message.body {
        Body::Person(text) => rsx! {
            div { class: "group flex w-full flex-col items-end gap-1.5 animate-msg-in",
                crate::attach::Attachments { files: message.attachments.clone(), end: true }
                if !text.trim().is_empty() {
                div { class: "flex w-full items-center justify-end gap-1.5",
                    span { class: "order-first mr-2 self-end pb-1 text-[11px] tabular-nums text-ink-tertiary opacity-0 transition-opacity group-hover:opacity-100", "{when}" }
                    div {
                        class: "w-fit min-w-9 {SIMPLE_BUBBLE_MAX} rounded-[18px] px-3 py-[7px] text-[15px] leading-[22px] whitespace-pre-wrap [overflow-wrap:anywhere] {own_class}",
                        style: "{own_style}",
                        "data-own-bubble": true,
                        "data-chat-bubble": "simple",
                        title: "{full_time}",
                        div { class: "chat-text", "{text}" }
                    }
                }
                }
                match message.held {
                    Held::Waiting => rsx! { HeldNote { thread: thread.clone(), message: message.id.clone(), steerable: steerable && working } },
                    Held::Steered => rsx! {
                        span { "data-steered": true, class: "flex items-center gap-1 pr-1 text-[11px] text-ink-tertiary", "Steered into the turn" }
                    },
                    Held::No => rsx! {},
                }
                if let Some((due, late)) = message.scheduled {
                    span { "data-scheduled-note": if late { "late" } else { "on-time" }, class: "flex items-center gap-1 pr-1 text-[11px] text-ink-tertiary", title: "Written earlier and sent at the time set",
                        crate::sidebar::Icon { name: "clock", size: 11 }
                        if late { "Scheduled for {ahead_when(due)} · sent late" } else { "Scheduled" }
                    }
                }
                crate::react::Reactions { thread: thread.clone(), message: message.id.clone(), reactions: message.reactions.clone(), end: true, bot_id: bot_id.clone(), working }
            }
        },
        Body::FromCatbot { from, text } => {
            let label = match from.as_str() {
                "watch" => "Catbot's watch",
                "check-in" => "Timed check-in",
                "brief" => "Daily brief",
                "trigger" => "Trigger",
                "board" => "Task board",
                _ => "Catbot",
            };
            // The Chief's notes are for the Chief; a trigger's or a board item's are for whichever bot it starts.
            let whom = if matches!(from.as_str(), "trigger" | "board") { "" } else { " · for the Chief" };
            rsx! {
                div { class: "flex w-full justify-start", "data-from-catbot": "{from}",
                    div { class: "flex w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] flex-col gap-1 rounded-xl border border-dashed border-hairline/60 px-3 py-2.5 text-[13px]", title: "{full_time}",
                        span { class: "text-[11px] font-semibold uppercase tracking-wide text-ink-tertiary", "{label}{whom} · {when}" }
                        div { class: "whitespace-pre-wrap [overflow-wrap:anywhere] text-ink-secondary", "{text}" }
                    }
                }
            }
        }
        Body::Reply(text) => {
            let shape = if markdown::is_formatted(text) {
                "w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] rounded-2xl px-1 text-[15px] leading-relaxed text-ink"
            } else {
                "w-fit min-w-9 max-w-[min(var(--chat-bubble-max),var(--chat-bubble-pct),calc(100%-82px))] rounded-[18px] bg-card bubble-finish px-3 py-[7px] text-[15px] leading-[22px] text-ink"
            };
            let html = markdown::to_html(text);
            // Who said it, in a chat where several bots talk; and whom it hands to, if anyone.
            let (speaker, to) = {
                let state = consume_context::<Signal<State>>();
                let state = state.read();
                let said_by = message.bot_id.clone().unwrap_or_else(|| bot_id.clone());
                let named = room || message.bot_id.as_deref().is_some_and(|id| id != bot_id);
                let who = named.then(|| speaker(&state, &said_by).or_else(|| {
                    // A channel member the app hasn't heard of yet: the channel's own record.
                    let info = speakers.as_ref()?;
                    let (id, name, mark, color) = info.members.iter().find(|m| m.0 == said_by)?.clone();
                    Some(Speaker { id, name, mark, color, picture: None })
                })).flatten();
                let to = match &who {
                    Some(w) => handed_to(text, &candidates(&state, speakers.as_ref()), &w.id),
                    None => Vec::new(),
                };
                (who, to)
            };
            rsx! {
                div { class: "group flex w-full flex-col items-start",
                    if let Some(who) = speaker {
                        SpeakerTag { who, to, working: live }
                    }
                    div { class: "flex w-full items-center justify-start gap-1.5",
                        div { class: "{shape}", title: "{full_time}",
                            div { class: "chat-md min-w-0 [&>*+*]:mt-2", dangerous_inner_html: "{html}" }
                        }
                        crate::voice::SpeakButton { id: message.id.clone(), text: text.clone() }
                        span { class: "ml-2 self-end pb-1 text-[11px] tabular-nums text-ink-tertiary opacity-0 transition-opacity group-hover:opacity-100", "{when}" }
                    }
                    if !message.attachments.is_empty() {
                        div { class: "mt-1.5 w-full", crate::attach::Attachments { files: message.attachments.clone(), end: false } }
                    }
                    crate::react::Reactions { thread: thread.clone(), message: message.id.clone(), reactions: message.reactions.clone(), end: false, bot_id: bot_id.clone(), working }
                }
            }
        }
        Body::Step { name, ok } => rsx! { Step { name: name.clone(), ok: *ok } },
        Body::Handoff { from, to } => rsx! { HandoffChip { from: from.clone(), to: to.clone(), going: handing, at: full_time.clone() } },
        Body::TeamUpdate { bot_id: whose, kind, thread: there, chat_of, text } => rsx! {
            TeamUpdateLine { bot_id: whose.clone(), kind: kind.clone(), thread: there.clone(), chat_of: chat_of.clone(), text: text.clone(), at: full_time.clone(), when: when.clone() }
        },
        Body::Picture(card) => rsx! {
            crate::pictures::PictureCard { thread: thread.clone(), message: message.id.clone(), card: (**card).clone() }
        },
        Body::TeamChange(change) => rsx! {
            crate::team::TeamChangeCard { thread: thread.clone(), message: message.id.clone(), change: (**change).clone() }
        },
        Body::Trigger(card) => rsx! {
            crate::triggers::TriggerCard { card: (**card).clone() }
        },
        Body::Changes { summary, files, undone } => rsx! {
            crate::changes::ChangesRow { thread: thread.clone(), message: message.id.clone(), summary: summary.clone(), files: files.clone(), undone: *undone, working }
        },
        Body::Card { subtitle, answer, tool, .. } => {
            let answer = settled_answer(*answer, asking);
            rsx! { ApprovalRow { line: approval_line(&bot_name, tool.as_deref()), detail: approval_detail(subtitle, tool.as_deref()), answer } }
        }
    }
}

/// The Team Chat's pinned notes, under its header (stage 30.1): one line until opened; Edit
/// opens them in its settings.
#[component]
fn PinnedNotes(text: String) -> Element {
    let mut ui = use_context::<crate::settings::Ui>();
    let mut open = use_signal(|| false);
    let text = text.trim().to_string();
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string();
    let more = text.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    rsx! {
        div { "data-pinned-notes": true, class: "relative z-[24] shrink-0 border-b border-hairline/40 bg-app/85 px-5 py-2 backdrop-blur-md",
            div { class: "chat-column mx-auto flex w-full items-start gap-2 text-[13px]",
                span { class: "shrink-0 pt-0.5 text-[11px] font-semibold uppercase tracking-wide text-ink-tertiary", "Pinned" }
                if text.is_empty() {
                    span { class: "min-w-0 flex-1 text-ink-tertiary", "No notes yet: who does what, the goals, a checklist. Every bot here reads them." }
                } else if open() {
                    div { class: "min-w-0 flex-1 whitespace-pre-wrap [overflow-wrap:anywhere] text-ink-secondary", "{text}" }
                } else {
                    span { class: "min-w-0 flex-1 truncate text-ink-secondary", "{first}" }
                }
                if more {
                    button { r#type: "button", "data-pinned-toggle": true, class: "shrink-0 text-ink-tertiary hover:text-ink hover:underline", onclick: move |_| open.toggle(),
                        if open() { "Less" } else { "More" }
                    }
                }
                button { r#type: "button", "data-pinned-edit": true, class: "shrink-0 text-ink-tertiary hover:text-ink hover:underline", onclick: move |_| ui.settings_open.set(true),
                    if text.is_empty() { "Add" } else { "Edit" }
                }
            }
        }
    }
}

/// Under a message sent while the bot worked: it waits for the turn to end, and Steer now sends
/// it into the running turn when its AI can take it (stage 29.14).
#[component]
fn HeldNote(thread: String, message: String, steerable: bool) -> Element {
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    rsx! {
        div { "data-held": true, class: "flex flex-col items-end gap-0.5 pr-1 text-[11px] text-ink-tertiary",
            span { class: "flex items-center gap-1.5",
                crate::sidebar::Icon { name: "clock", size: 11 }
                "Waiting for the turn to end"
                if steerable {
                    button {
                        r#type: "button",
                        "data-steer": true,
                        disabled: busy(),
                        title: "Send this into what the bot is doing now",
                        class: "rounded-full bg-raised px-2 py-0.5 text-[11px] font-medium text-ink hover:bg-control disabled:opacity-40",
                        onclick: move |_| {
                            let (thread, message) = (thread.clone(), message.clone());
                            spawn(async move {
                                busy.set(true);
                                let body = serde_json::json!({ "messageId": message });
                                problem.set(api::call("POST", &format!("/api/threads/{thread}/steer"), Some(&body)).await.err());
                                busy.set(false);
                            });
                        },
                        if busy() { "Steering…" } else { "Steer now" }
                    }
                }
            }
            if let Some(err) = problem() {
                span { role: "alert", class: "text-right text-danger", style: "max-width:320px", "{err}" }
            }
        }
    }
}

/// How many faces the header shows before "+3".
const FACES_SHOWN: usize = 5;

/// A bot as a chat draws it beside what it said (stage 29.12).
#[derive(Clone, Debug, PartialEq)]
pub struct Speaker {
    pub id: String,
    pub name: String,
    pub mark: String,
    pub color: String,
    pub picture: Option<crate::mascots::Picture>,
}

impl Speaker {
    fn of(bot: &Bot) -> Speaker {
        Speaker { id: bot.id.clone(), name: bot.name.clone(), mark: bot.mark.clone(), color: bot.color.clone(), picture: bot.picture.clone() }
    }

    fn initial(&self) -> String {
        self.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
    }
}

/// A bot by its id (not a channel).
fn speaker(state: &State, id: &str) -> Option<Speaker> {
    state.bots.iter().find(|b| b.id == id && b.channel.is_none()).map(Speaker::of)
}

/// A bot by its name, as a hand-off note gives it.
fn speaker_named(state: &State, name: &str) -> Option<Speaker> {
    state.bots.iter().find(|b| b.name == name && b.channel.is_none()).map(Speaker::of)
}

/// The bots a reply could hand to: a channel's members, or every bot.
fn candidates(state: &State, channel: Option<&crate::bots::ChannelInfo>) -> Vec<Speaker> {
    match channel {
        Some(info) => info.members.iter().filter_map(|m| speaker(state, &m.0)).collect(),
        None => state.bots.iter().filter(|b| b.channel.is_none()).map(Speaker::of).collect(),
    }
}

/// The other bots who answered in a bot's own chat (guests a hand-off brought in), in order.
pub fn guests_of(messages: &[Msg], own: &str) -> Vec<String> {
    let mut guests: Vec<String> = Vec::new();
    for m in messages {
        if let (Body::Reply(_), Some(id)) = (&m.body, &m.bot_id)
            && id != own
            && !guests.contains(id)
        {
            guests.push(id.clone());
        }
    }
    guests
}

/// Whom a reply hands to: the bots @named in its last paragraph, as the server reads it
/// (`handoffs::named`): whole names, longest first, in the order they appear.
pub fn handed_to(reply: &str, candidates: &[Speaker], writer: &str) -> Vec<Speaker> {
    let last = reply.trim_end().rsplit("\n\n").next().unwrap_or_default().to_lowercase();
    let mut others: Vec<&Speaker> = candidates.iter().filter(|c| c.id != writer).collect();
    others.sort_by_key(|c| std::cmp::Reverse(c.name.len()));
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut named: Vec<(usize, Speaker)> = Vec::new();
    for bot in others {
        let tag = format!("@{}", bot.name.to_lowercase());
        let mut from = 0;
        while let Some(found) = last[from..].find(&tag) {
            let at = from + found;
            let end = at + tag.len();
            let whole = last[end..].chars().next().is_none_or(|c| !c.is_alphanumeric())
                && last[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            if whole && !taken.iter().any(|(s, e)| at < *e && end > *s) {
                taken.push((at, end));
                if !named.iter().any(|(_, b)| b.id == bot.id) {
                    named.push((at, bot.clone()));
                }
                break;
            }
            from = end;
        }
    }
    named.sort_by_key(|(at, _)| *at);
    named.into_iter().map(|(_, b)| b).collect()
}

/// A bot's name in its own colour.
#[component]
fn SpeakerName(who: Speaker) -> Element {
    rsx! { span { class: "bot-identity font-semibold", "data-bot-color": "{who.color}", "{who.name}" } }
}

/// Over a reply in a chat with several bots: the speaker's face and name, and "→ Kiwi" when it
/// hands to someone (their `ClusterLabel`).
#[component]
fn SpeakerTag(who: Speaker, #[props(default)] to: Vec<Speaker>, #[props(default)] working: bool) -> Element {
    let last = to.len().saturating_sub(1);
    rsx! {
        div { class: "mb-1 flex flex-wrap items-center gap-1.5 text-[13px] text-ink-secondary", "data-speaker": "{who.name}",
            BotMark { picture: who.picture.clone(), color: who.color.clone(), initial: who.initial(), size: 20, mark: who.mark.clone(), state: if working { "working" } else { "idle" } }
            SpeakerName { who: who.clone() }
            if !to.is_empty() {
                span { class: "text-ink-tertiary", "data-hands-to": true, "→" }
                for (i, one) in to.into_iter().enumerate() {
                    span { key: "{one.id}", class: "inline-flex items-center",
                        SpeakerName { who: one }
                        if i < last { span { class: "text-ink-tertiary", "," } }
                    }
                }
            }
        }
    }
}

/// A line about the team in the Team Chat: the bot's face, what happened, and Open for the chat
/// it happened in (stage 30.2).
#[component]
fn TeamUpdateLine(bot_id: String, kind: String, thread: String, chat_of: String, text: String, at: String, when: String) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<crate::settings::Ui>();
    let who = speaker(&state.read(), &bot_id);
    let dot = match kind.as_str() {
        "waiting" => "bg-warning",
        "failed" => "bg-danger",
        "finished" => "bg-success",
        _ => "bg-accent",
    };
    let can_open = !thread.is_empty() && !chat_of.is_empty() && state.read().bots.iter().any(|b| b.id == chat_of);
    rsx! {
        div { class: "flex justify-start", "data-team-update": "{kind}",
            div { class: "flex min-w-0 max-w-full items-center gap-2 text-[13px] text-ink-secondary", title: "{at}",
                span { "aria-hidden": "true", class: "size-1.5 shrink-0 rounded-full {dot}" }
                if let Some(w) = &who {
                    BotMark { picture: w.picture.clone(), color: w.color.clone(), initial: w.initial(), size: 18, mark: w.mark.clone() }
                }
                span { class: "min-w-0 truncate", "{text}" }
                span { class: "shrink-0 text-[11px] tabular-nums text-ink-tertiary", "{when}" }
                if can_open {
                    button { r#type: "button", "data-team-update-open": true, class: "shrink-0 text-[12px] text-ink-tertiary hover:text-ink hover:underline",
                        onclick: move |_| crate::chats::open(state, ui, &chat_of, &thread),
                        "Open"
                    }
                }
            }
        }
    }
}

/// One bot handing the work to another: both faces, and whether it's still going
/// (their `HandoffStatusChip`).
#[component]
fn HandoffChip(from: String, to: String, going: bool, #[props(default)] at: String) -> Element {
    let state = consume_context::<Signal<State>>();
    let (giver, taker) = {
        let state = state.read();
        (speaker_named(&state, &from), speaker_named(&state, &to))
    };
    let face = |who: &Option<Speaker>, name: &str, state: &'static str| match who {
        Some(w) => rsx! {
            BotMark { picture: w.picture.clone(), color: w.color.clone(), initial: w.initial(), size: 18, mark: w.mark.clone(), state }
            SpeakerName { who: w.clone() }
        },
        None => rsx! { span { class: "font-semibold", "{name}" } },
    };
    let (dot, status) = if going { ("bg-accent animate-pulse", "Working") } else { ("bg-success", "Handed over") };
    rsx! {
        div { class: "flex justify-start", "data-handoff": "{from}→{to}", "data-handoff-state": if going { "working" } else { "done" },
            div { class: "ui-pill text-[13px] text-ink-secondary", role: "status", title: "{from} handed this to {to} · {at}",
                {face(&giver, &from, "idle")}
                span { class: "text-ink-tertiary", "aria-label": "handed this to", "→" }
                {face(&taker, &to, if going { "working" } else { "idle" })}
                span { class: "inline-flex items-center gap-1.5 text-[12px] text-ink-tertiary",
                    span { "aria-hidden": "true", class: "size-1.5 shrink-0 rounded-full {dot}" }
                    "{status}"
                }
            }
        }
    }
}

/// One tool step, as their `ToolActivity` chip draws it closed.
#[component]
fn Step(name: String, ok: Option<bool>, #[props(default)] live: bool) -> Element {
    let failed = ok == Some(false);
    let tone = if failed { "text-danger" } else { "text-ink-secondary" };
    rsx! {
        div { class: "flex min-w-0 justify-start",
            div { "data-testid": "tool-activity", class: "w-fit max-w-full rounded-xl border border-hairline/40 bg-panel text-[13px]",
                div { class: "flex min-h-8 items-center gap-2 rounded-xl px-3 py-1.5 {tone}",
                    span { class: "shrink-0", "aria-hidden": "true",
                        {
                            match ok {
                                Some(true) => rsx! { Icon { path: CHECK, class: "text-success" } },
                                Some(false) => rsx! { Icon { path: CROSS, class: "" } },
                                // Still running: their working dots; a step that never reported back: a circle.
                                None if live => rsx! { span { class: "inline-block size-3 animate-spin rounded-full border-[1.5px] border-ink-secondary border-t-transparent" } },
                                None => rsx! { Icon { path: CIRCLE, class: "" } },
                            }
                        }
                    }
                    span { class: "min-w-0 max-w-[30rem] truncate font-mono leading-5", title: "{name}", "{name}" }
                }
            }
        }
    }
}

/// Two or more steps in a row, folded into one line that opens to show them (their `ActivityRun`).
#[component]
fn Run(steps: Vec<Msg>, #[props(default)] bot_name: String, #[props(default)] asking: bool) -> Element {
    let mut open = use_signal(|| false);
    let summary = describe_run(&steps);
    let pill = "flex min-w-0 max-w-full items-center gap-2 rounded-full border border-hairline/40 bg-panel px-3 py-1.5 text-[13px] text-ink-secondary hover:bg-control";
    if open() {
        return rsx! {
            div { class: "flex flex-col gap-1",
                div { class: "flex min-w-0 justify-start",
                    button { r#type: "button", "aria-expanded": "true", class: "{pill}", onclick: move |_| open.set(false),
                        Icon { path: CHEVRON, class: "rotate-90" }
                        span { class: "min-w-0 truncate font-medium leading-5", "{summary}" }
                    }
                }
                for step in steps {
                    Row { key: "{step.id}", message: step, bot_name: bot_name.clone(), asking }
                }
            }
        };
    }
    rsx! {
        div { class: "flex min-w-0 justify-start",
            button { r#type: "button", "aria-expanded": "false", class: "{pill}", onclick: move |_| open.set(true),
                Icon { path: CHECK, class: "text-success" }
                span { class: "min-w-0 max-w-[480px] truncate font-medium leading-5", "{summary}" }
                Icon { path: CHEVRON, class: "" }
            }
        }
    }
}

/// A question the bot is waiting on, for the panel above the typing box.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub request_id: String,
    pub tool: Option<String>,
    pub subtitle: String,
    /// The "Always allow" button's words, if it can be offered.
    pub always: Option<String>,
}

/// Unanswered approval cards, oldest first (their `pendingApprovals`).
pub fn pending_approvals(messages: &[Msg]) -> Vec<Pending> {
    messages
        .iter()
        .filter_map(|m| match &m.body {
            Body::Card { answer: Answer::Waiting, request_id: Some(id), tool, subtitle, always, .. } => {
                Some(Pending { request_id: id.clone(), tool: tool.clone(), subtitle: subtitle.clone(), always: always.clone() })
            }
            _ => None,
        })
        .collect()
}

/// How a card reads now: one still marked waiting whose bot isn't asking anymore had its turn end.
fn settled_answer(answer: Answer, asking: bool) -> Answer {
    if answer == Answer::Waiting && !asking { Answer::Dismissed } else { answer }
}

/// Their tool phrases (`toolLabel`): "run a command", "write a file"…
pub fn tool_phrase(tool: Option<&str>) -> String {
    match tool {
        None => "take an action".into(),
        Some("Bash" | "shell") => "run a command".into(),
        Some("Read" | "read") => "read a file".into(),
        Some("Write") => "write a file".into(),
        Some("Edit" | "MultiEdit" | "edit") => "edit a file".into(),
        Some("WebFetch" | "fetch") => "fetch a web page".into(),
        Some("WebSearch") => "search the web".into(),
        Some("delete") => "delete a file".into(),
        Some("tool") => "use a tool".into(),
        Some(other) => match other.strip_prefix("mcp__").map(|rest| rest.splitn(2, "__").collect::<Vec<_>>()) {
            Some(parts) if parts.len() == 2 => format!("{} · {}", parts[0], parts[1].replace('_', " ")),
            _ => other.replace('_', " "),
        },
    }
}

/// "Kiwi wants to run a command".
pub fn approval_line(bot_name: &str, tool: Option<&str>) -> String {
    let action = tool_phrase(tool);
    if bot_name.is_empty() { format!("Wants to {action}") } else { format!("{bot_name} wants to {action}") }
}

/// What exactly it wants to do: the card's summary without the tool's name in front.
pub fn approval_detail(subtitle: &str, tool: Option<&str>) -> String {
    let subtitle = subtitle.trim();
    match tool.and_then(|tool| subtitle.strip_prefix(tool)) {
        Some(rest) if rest.starts_with(' ') => rest.trim().to_string(),
        Some("") => String::new(),
        _ => subtitle.to_string(),
    }
}

/// Their code chip for a request's detail (`APPROVAL_CODE_CHIP`).
const APPROVAL_CODE_CHIP: &str = "inline-flex min-w-0 max-w-full items-baseline rounded-full bg-inset px-2 pb-px pt-[3px] font-mono text-[13px] leading-5 text-ink";

/// An approval in the conversation, as their `CompactApprovalRow`: who wanted what, and how it
/// ended. While the question is open the row stays out of sight; the panel above the typing
/// box asks it.
#[component]
fn ApprovalRow(line: String, detail: String, answer: Answer) -> Element {
    let mut open = use_signal(|| false);
    if answer == Answer::Waiting {
        return rsx! {
            div { "data-approval-row": "pending", class: "sr-only", "{line} {detail}. Waiting for your answer below" }
        };
    }
    let (status, icon, icon_class) = match answer {
        Answer::Allowed => ("Allowed", CHECK, "text-success"),
        Answer::Denied => ("Denied", CROSS, ""),
        _ => ("No answer before the turn ended", CLOCK, ""),
    };
    rsx! {
        div { "data-approval-row": "settled", class: "w-full max-w-[840px] text-[13px] text-ink-tertiary",
            div { class: "flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 leading-5",
                span { class: "shrink-0 self-center", Icon { path: icon, class: icon_class, size: 14 } }
                span { class: "min-w-0 break-words", "{line}" }
                if !detail.is_empty() {
                    span { role: "group", "aria-label": "Details", class: "min-w-0 max-w-full",
                        button {
                            r#type: "button",
                            "aria-expanded": "{open}",
                            title: "{detail}",
                            class: "{APPROVAL_CODE_CHIP} text-left hover:bg-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus",
                            onclick: move |_| open.toggle(),
                            span { class: "truncate", "{detail}" }
                        }
                    }
                }
                span { class: "shrink-0", "aria-live": "polite", "· {status}" }
            }
            if open() {
                pre {
                    tabindex: 0,
                    "aria-label": "Details",
                    class: "mt-1.5 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-2xl bg-inset px-4 py-3 font-mono text-[12px] leading-relaxed text-ink",
                    "{detail}"
                }
            }
        }
    }
}

// Their answer buttons (`AskCard.tsx`).
const ASK_LOOK: &str = "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus disabled:opacity-40";
const ASK_BUTTON_SHAPE: &str = "inline-flex min-h-9 shrink-0 items-center justify-center gap-1.5 rounded-full px-4 py-1.5 text-center text-[14px] font-medium leading-5 transition-colors [&>svg]:shrink-0 disabled:cursor-not-allowed pointer-coarse:min-h-11";
const ASK_SECONDARY: &str = "bg-ink/[0.07] text-ink enabled:hover:bg-ink/[0.11] enabled:active:bg-ink/[0.15]";
const ASK_QUIET: &str = "text-ink-secondary enabled:hover:bg-ink/[0.06] enabled:hover:text-ink enabled:active:bg-ink/[0.1]";
const ASK_QUIET_BUTTON: &str = "inline-flex h-7 shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-full px-3 text-[13px] font-medium leading-5 transition-colors -my-1 text-ink-tertiary hover:bg-ink/[0.07] hover:text-ink-secondary";

/// The question waiting for an answer, above the typing box, as their `PendingApprovalPanel`:
/// who asks and what for, the exact request, then Deny and Allow once. Cancel turn is under
/// the details arrow.
#[component]
fn ApprovalPanel(bot: Bot, pending: Pending, count: usize, on_cancel_turn: EventHandler<()>) -> Element {
    let mut open = use_signal(|| false);
    let mut answering = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let line = approval_line(&bot.name, pending.tool.as_deref());
    let detail = approval_detail(&pending.subtitle, pending.tool.as_deref());
    let thread = bot.thread_id.clone();
    let request = pending.request_id.clone();
    let decide = move |behavior: &'static str| {
        if answering() {
            return;
        }
        answering.set(true);
        problem.set(None);
        let (thread, request) = (thread.clone(), request.clone());
        spawn(async move {
            let body = serde_json::json!({ "requestId": request, "behavior": behavior });
            if let Err(err) = api::post(&format!("/api/threads/{thread}/respond"), &body).await {
                problem.set(Some(err));
                answering.set(false);
            }
            // On success the saved card changes and the panel goes away with it.
        });
    };
    let allow_style = format!(
        "--bot-button: var(--identity-{c}); --bot-button-ink: #ffffff; --bot-button-hover: color-mix(in srgb, var(--identity-{c}) 88%, #000000); --bot-button-pressed: color-mix(in srgb, var(--identity-{c}) 80%, #000000);",
        c = bot.color
    );
    let details_id = format!("approval-details-{}", pending.request_id);
    let (mut deny, mut allow, mut always) = (decide.clone(), decide.clone(), decide);
    let chevron: &'static str = if open() { "rotate-180 motion-safe:transition-transform" } else { "motion-safe:transition-transform" };
    rsx! {
        div { class: "mb-2 overflow-hidden rounded-[18px] bg-composer ring-[0.5px] ring-ink/10",
            div { role: "region", "aria-label": "Approval needed", "data-ask-card": "approval", class: "px-4 pb-3.5 pt-3",
                div { class: "flex min-w-0 items-center gap-2", "aria-live": "polite",
                    div { class: "min-w-0 flex-1",
                        div { "data-ask-header": true, class: "flex min-w-0 items-center gap-1.5 text-[13px] leading-[18px] text-ink-secondary",
                            span { "aria-hidden": "true", class: "flex shrink-0", BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 16, mark: bot.mark.clone(), state: bot.activity.mood() } }
                            span { dir: "auto", class: "min-w-0 truncate", "{line}" }
                        }
                    }
                    if count > 1 {
                        span { class: "shrink-0 rounded-full bg-ink/[0.06] px-2 text-[12px] leading-5 tabular-nums text-ink-secondary", "1 of {count}" }
                    }
                    button {
                        r#type: "button",
                        "aria-expanded": "{open}",
                        "aria-controls": "{details_id}",
                        "aria-label": if open() { "Hide details" } else { "Details" },
                        title: if open() { "Hide details" } else { "Details" },
                        class: "{ASK_QUIET_BUTTON} -me-2 w-7 px-0",
                        onclick: move |_| open.toggle(),
                        Icon { path: CHEVRON_DOWN, class: chevron, size: 16 }
                    }
                }
                if !detail.is_empty() {
                    div { class: "mt-2",
                        div { dir: "auto", "aria-label": "Review the details", class: "rounded-xl bg-ink/[0.045] px-3 py-2 font-mono text-[12px] leading-[18px] whitespace-pre-wrap text-ink-secondary [overflow-wrap:anywhere]", "{detail}" }
                    }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-2 text-[13px] leading-[18px] text-warning", "Couldn't answer: {err}" }
                }
                div { class: "mt-3 flex flex-wrap items-center justify-end gap-2",
                    button {
                        r#type: "button",
                        "data-approval-action": "deny",
                        disabled: answering(),
                        class: "{ASK_BUTTON_SHAPE} {ASK_LOOK} {ASK_SECONDARY}",
                        onclick: move |_| deny("deny"),
                        "Deny"
                    }
                    button {
                        r#type: "button",
                        "data-approval-action": "allow",
                        disabled: answering(),
                        class: "bot-button {ASK_BUTTON_SHAPE} {ASK_LOOK}",
                        style: "{allow_style}",
                        onclick: move |_| allow("allow"),
                        "Allow once"
                    }
                    if let Some(words) = pending.always.clone() {
                        button {
                            r#type: "button",
                            "data-approval-action": "always",
                            disabled: answering(),
                            title: "{bot.name} won't ask about this again, anywhere it works. Remove it in its settings.",
                            class: "{ASK_BUTTON_SHAPE} {ASK_LOOK} {ASK_SECONDARY}",
                            onclick: move |_| always("always"),
                            "{words}"
                        }
                    }
                }
                div { id: "{details_id}", hidden: !open(), class: "mt-3 space-y-2",
                    div { class: "flex flex-wrap items-center gap-2",
                        button { r#type: "button", class: "{ASK_BUTTON_SHAPE} {ASK_LOOK} {ASK_QUIET}", onclick: move |_| on_cancel_turn.call(()), "Cancel turn" }
                    }
                }
            }
        }
    }
}

// Lucide shapes (ISC, see NOTICE), drawn at 13px.
const CHECK: &str = "M20 6 9 17l-5-5";
const CROSS: &str = "M18 6 6 18M6 6l12 12";
const CHEVRON: &str = "m9 18 6-6-6-6";
const CIRCLE: &str = "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20";
const CLOCK: &str = "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20M12 6v6l4 2";
const CHEVRON_DOWN: &str = "m6 9 6 6 6-6";

#[component]
fn Icon(path: &'static str, class: &'static str, #[props(default = 13)] size: u32) -> Element {
    rsx! {
        svg {
            class: "shrink-0 {class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: path }
        }
    }
}

pub fn js_now() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now();
    #[cfg(not(target_arch = "wasm32"))]
    return chrono::Utc::now().timestamp_millis() as f64;
}

/// An RFC 3339 time as ms since the epoch (NaN if it isn't one).
pub fn parse_time(at: &str) -> f64 {
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::parse(at);
    #[cfg(not(target_arch = "wasm32"))]
    return chrono::DateTime::parse_from_rfc3339(at).map(|t| t.timestamp_millis() as f64).unwrap_or(f64::NAN);
}

/// "Today", "Yesterday", or "Sat, Nov 14", as their `dayLabel` says it in English.
fn day_label(at: f64, today: i64) -> String {
    match today - day(at) {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ => date_string(at, &[("weekday", "short"), ("month", "short"), ("day", "numeric")], false),
    }
}

/// When a chat was last used, short: "3:41 PM" today, "Yesterday", else "Nov 12".
pub fn short_when(at: f64) -> String {
    match day(js_now()) - day(at) {
        0 => clock(at),
        1 => "Yesterday".into(),
        _ => date_string(at, &[("month", "short"), ("day", "numeric")], false),
    }
}

/// When something is due, ahead: "Today 8:00 PM", "Tomorrow 9:00 AM", "Thu, Nov 26, 9:00 AM"
/// (or behind, for one that went out: "Yesterday 9:00 AM").
pub fn ahead_when(at: f64) -> String {
    match day(at) - day(js_now()) {
        0 => format!("Today {}", clock(at)),
        1 => format!("Tomorrow {}", clock(at)),
        -1 => format!("Yesterday {}", clock(at)),
        _ => format!("{}, {}", date_string(at, &[("weekday", "short"), ("month", "short"), ("day", "numeric")], false), clock(at)),
    }
}

/// "Dec 1".
pub fn month_day(at: f64) -> String {
    #[cfg(target_arch = "wasm32")]
    return date_string(at, &[("month", "short"), ("day", "numeric")], false);
    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone};
        Local.timestamp_millis_opt(at as i64).single().map(|t| t.format("%b %-d").to_string()).unwrap_or_default()
    }
}

/// "Tuesday".
pub fn weekday(at: f64) -> String {
    #[cfg(target_arch = "wasm32")]
    return date_string(at, &[("weekday", "long")], false);
    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone};
        Local.timestamp_millis_opt(at as i64).single().map(|t| t.format("%A").to_string()).unwrap_or_default()
    }
}

/// "3:41 PM".
pub fn clock(at: f64) -> String {
    date_string(at, &[("hour", "numeric"), ("minute", "2-digit")], true)
}

/// The full date and time, for a message's tooltip.
fn full_date(at: f64) -> String {
    date_string(at, &[("dateStyle", "medium"), ("timeStyle", "short")], false)
}

#[cfg(target_arch = "wasm32")]
fn date_string(at: f64, fields: &[(&str, &str)], time_only: bool) -> String {
    let date = js_sys::Date::new(&at.into());
    if date.get_time().is_nan() {
        return String::new();
    }
    let options = js_sys::Object::new();
    for (key, value) in fields {
        let _ = js_sys::Reflect::set(&options, &(*key).into(), &(*value).into());
    }
    if time_only {
        date.to_locale_time_string_with_options("en-US", &options).into()
    } else {
        date.to_locale_string("en-US", &options).into()
    }
}

/// The same three shapes in the desktop app, from the computer's clock and time zone.
#[cfg(not(target_arch = "wasm32"))]
fn date_string(at: f64, fields: &[(&str, &str)], time_only: bool) -> String {
    use chrono::{Local, TimeZone};
    let Some(when) = Local.timestamp_millis_opt(at as i64).single() else { return String::new() };
    let pattern = if time_only {
        "%-I:%M %p" // 3:41 PM
    } else if fields.iter().any(|(k, _)| *k == "weekday") {
        "%a, %b %-d" // Sat, Nov 14
    } else {
        "%b %-d, %Y, %-I:%M %p" // Nov 14, 2026, 3:41 PM
    };
    when.format(pattern).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(id: &str, name: &str, at: f64) -> Msg {
        Msg { id: id.into(), at, send_id: None, body: Body::Step { name: name.into(), ok: Some(true) }, attachments: Vec::new(), reactions: Vec::new(), bot_id: None, scheduled: None, held: Held::No }
    }

    #[test]
    fn wire_messages() {
        let person = Msg::from_wire(&json!({ "id": "cb-1", "at": 5.0, "role": "user", "kind": "text", "text": "hi" })).unwrap();
        assert_eq!(person.body, Body::Person("hi".into()));
        let note = Msg::from_wire(&json!({ "id": "cb-2", "at": 6.0, "role": "bot", "kind": "activity", "tool": { "name": "notice: stopped", "ok": true } })).unwrap();
        assert_eq!(note.body, Body::Step { name: "stopped".into(), ok: Some(true) });
        let card = |card: Value| match Msg::from_wire(&json!({ "id": "cb-3", "at": 7.0, "role": "bot", "kind": "options", "card": card })).unwrap().body {
            Body::Card { answer, .. } => answer,
            other => panic!("{other:?}"),
        };
        assert_eq!(card(json!({ "title": "Approval needed" })), Answer::Waiting);
        assert_eq!(card(json!({ "answered": "allow" })), Answer::Allowed);
        assert_eq!(card(json!({ "answered": "deny" })), Answer::Denied);
        assert_eq!(card(json!({ "answered": "deny", "dismissed": true })), Answer::Dismissed);
        assert!(Msg::from_wire(&json!({ "id": "x", "kind": "screen" })).is_none());
    }

    fn bot_said(id: &str, by: Option<&str>, text: &str) -> Msg {
        Msg { id: id.into(), at: 1.0, send_id: None, body: Body::Reply(text.into()), attachments: Vec::new(), reactions: Vec::new(), bot_id: by.map(str::to_string), scheduled: None, held: Held::No }
    }

    fn who(id: &str, name: &str) -> Speaker {
        Speaker { id: id.into(), name: name.into(), mark: "cat-head".into(), color: "teal".into(), picture: None }
    }

    #[test]
    fn a_hand_off_note_is_a_chip_not_a_step() {
        let note = Msg::from_wire(&json!({ "id": "n1", "at": 1.0, "role": "bot", "kind": "activity", "tool": { "name": "notice: Palette handed this to Gamewright.", "ok": true }, "handoff": { "from": "Palette", "to": "Gamewright" } })).unwrap();
        assert_eq!(note.body, Body::Handoff { from: "Palette".into(), to: "Gamewright".into() });
        // It stands on its own line, between steps.
        let items = group(&[step("a", "Read", 1.0), note, step("b", "Read", 1.0)]);
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn a_message_sent_while_the_bot_works_waits_or_was_steered() {
        let mine = |extra: Value| {
            let mut wire = json!({ "id": "cb-9", "at": 1.0, "role": "user", "kind": "text", "text": "and the beans" });
            wire.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            Msg::from_wire(&wire).unwrap().held
        };
        assert_eq!((mine(json!({ "held": true })), mine(json!({ "steered": true })), mine(json!({}))), (Held::Waiting, Held::Steered, Held::No));
    }

    #[test]
    fn a_team_update_knows_its_bot_and_chat() {
        let line = Msg::from_wire(&json!({ "id": "n2", "at": 1.0, "role": "bot", "kind": "activity", "tool": { "name": "notice: Kiwi is waiting for you in its chat: Bash echo hi.", "ok": true }, "teamUpdate": { "botId": "k", "kind": "waiting", "threadId": "t9", "chatOf": "k" } })).unwrap();
        assert_eq!(line.body, Body::TeamUpdate { bot_id: "k".into(), kind: "waiting".into(), thread: "t9".into(), chat_of: "k".into(), text: "Kiwi is waiting for you in its chat: Bash echo hi.".into() });
        assert!(!line.is_step());
    }

    #[test]
    fn guests_are_the_other_bots_who_answered() {
        let messages = [bot_said("1", Some("pal"), "hi"), bot_said("2", Some("game"), "yo"), bot_said("3", None, "x"), bot_said("4", Some("game"), "again")];
        assert_eq!(guests_of(&messages, "pal"), ["game"]);
        assert!(guests_of(&messages[..1], "pal").is_empty());
    }

    #[test]
    fn a_reply_hands_to_the_bots_named_at_its_end() {
        let team = [who("p", "Palette"), who("g", "Gamewright"), who("g2", "Gamewright Two"), who("k", "Kiwi")];
        let names = |text: &str| handed_to(text, &team, "p").into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names("Done.\n\nOver to @Kiwi and @gamewright two."), ["Kiwi", "Gamewright Two"]);
        // Only the last paragraph, whole names, never the writer.
        assert!(names("As @Kiwi said, all good.\n\nThanks!").is_empty());
        assert!(names("Ask @Kiwis or @Palette").is_empty());
    }

    #[test]
    fn steps_fold_into_runs() {
        let reply = Msg { id: "r".into(), at: 3.0, send_id: None, body: Body::Reply("done".into()), attachments: Vec::new(), reactions: Vec::new(), bot_id: None, scheduled: None, held: Held::No };
        let items = group(&[step("a", "Read", 1.0), step("b", "Read", 1.0), step("c", "Bash", 2.0), reply.clone(), step("d", "Edit", 4.0)]);
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], Item::Run(steps) if steps.len() == 3));
        assert_eq!(items[1], Item::One(reply));
        assert!(matches!(&items[2], Item::One(m) if m.id == "d"));
        if let Item::Run(steps) = &items[0] {
            assert_eq!(describe_run(steps), "3 steps · Read ×2, Bash");
        }
        let many: Vec<Msg> = ["A", "B", "C", "D", "E"].iter().enumerate().map(|(i, n)| step(&i.to_string(), n, 1.0)).collect();
        assert_eq!(describe_run(&many), "5 steps · A, B, C +2 more");
    }

    #[test]
    fn approvals_read_like_theirs() {
        assert_eq!(approval_line("Kiwi", Some("Bash")), "Kiwi wants to run a command");
        assert_eq!(approval_line("Kiwi", Some("Write")), "Kiwi wants to write a file");
        assert_eq!(approval_line("Kiwi", Some("mcp__catbot__approve_it")), "Kiwi wants to catbot · approve it");
        assert_eq!(approval_detail("Bash cd ~/x && ls", Some("Bash")), "cd ~/x && ls");
        assert_eq!(approval_detail("Bashful thing", Some("Bash")), "Bashful thing");
        assert_eq!(approval_detail("Write", Some("Write")), "");
        assert_eq!(settled_answer(Answer::Waiting, false), Answer::Dismissed);
        assert_eq!(settled_answer(Answer::Waiting, true), Answer::Waiting);
        let card = |id: &str, answered: Option<&str>| {
            let mut card = json!({ "title": "Approval needed", "subtitle": "Bash ls", "requestId": id, "tool": "Bash" });
            if let Some(a) = answered {
                card["answered"] = json!(a);
            }
            Msg::from_wire(&json!({ "id": format!("cb-{id}"), "at": 1.0, "role": "bot", "kind": "options", "card": card })).unwrap()
        };
        let pending = pending_approvals(&[card("r1", Some("allow")), card("r2", None), card("r3", None)]);
        assert_eq!(pending.iter().map(|p| p.request_id.as_str()).collect::<Vec<_>>(), ["r2", "r3"]);
        assert_eq!(pending[0].tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn a_new_day_splits_a_run() {
        let items = group(&[step("a", "Read", 1.0), step("b", "Read", 86_400_000.0 + 1.0)]);
        assert_eq!(items.len(), 2);
    }
}
