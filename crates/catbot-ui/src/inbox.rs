//! The Inbox (stage 19.5): its row at the top of the sidebar and its panel on the right, as
//! OpenMausBot's `SidebarInboxRow.tsx` and `inbox/InboxPanel.tsx` draw them in the simple layout
//! (Apache-2.0, see NOTICE).
//!
//! The list comes from the server (`/api/inbox`): asks waiting on the person (approvals, changes a
//! bot asks to make, a picture to choose), and news (a bot's message, a trigger's result or
//! failure). Under it, the chats at work or unread, and the ones lately finished, from the bots
//! the app already has. An ask or a message opens in the panel, to answer it there; news opens
//! its chat.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::{Activity, Bot};
use crate::chat::{approval_line, short_when};
use crate::live::State;
use crate::mascots::BotMark;
use crate::settings::Ui;
use crate::sidebar::Icon;

/// How many finished chats the panel keeps in sight.
const FINISHED: usize = 10;

/// What a line waiting on the person asks.
#[derive(Clone, Debug, PartialEq)]
pub enum Card {
    /// A step a running turn waits on: Allow once or Deny.
    Approval { request: String, tool: Option<String> },
    /// A change a bot asks to make: Approve or Decline.
    Change { title: String, lines: Vec<String>, warning: Option<String> },
    /// A picture whose AI is still to be chosen, in its chat.
    Picture,
}

/// One line of the Inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    /// "ask", "message", "result" or "notice".
    pub kind: String,
    /// "needs-you", "info" or "done".
    pub tab: String,
    pub from: String,
    pub from_bot: String,
    pub color: String,
    /// The bot (or channel) whose chat it came from.
    pub place: String,
    pub thread: String,
    pub title: String,
    pub message: Option<String>,
    pub text: String,
    /// When, in ms since the epoch.
    pub at: f64,
    pub unread: bool,
    pub card: Option<Card>,
}

impl Row {
    fn from_wire(w: &Value) -> Option<Row> {
        let card = &w["card"];
        let card = match card["type"].as_str() {
            Some("approval") => Some(Card::Approval { request: card["requestId"].as_str()?.to_string(), tool: card["tool"].as_str().map(str::to_string) }),
            Some("change") => Some(Card::Change {
                title: card["title"].as_str().unwrap_or_default().to_string(),
                lines: card["lines"].as_array().map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
                warning: card["warning"].as_str().filter(|w| !w.is_empty()).map(str::to_string),
            }),
            Some("picture") => Some(Card::Picture),
            _ => None,
        };
        Some(Row {
            id: w["id"].as_str()?.to_string(),
            kind: w["kind"].as_str().unwrap_or("notice").to_string(),
            tab: w["tab"].as_str().unwrap_or("info").to_string(),
            from: w["from"]["name"].as_str().unwrap_or_default().to_string(),
            from_bot: w["from"]["botId"].as_str().unwrap_or_default().to_string(),
            color: w["from"]["color"].as_str().unwrap_or("slate").to_string(),
            place: w["origin"]["botId"].as_str().unwrap_or_default().to_string(),
            thread: w["origin"]["threadId"].as_str().unwrap_or_default().to_string(),
            title: w["origin"]["title"].as_str().unwrap_or_default().to_string(),
            message: w["origin"]["messageId"].as_str().map(str::to_string),
            text: w["text"].as_str().unwrap_or_default().to_string(),
            at: w["at"].as_f64().unwrap_or(0.0),
            unread: w["unread"] == true,
            card,
        })
    }

    pub fn list(answer: &Value) -> Vec<Row> {
        answer["rows"].as_array().map(|rows| rows.iter().filter_map(Row::from_wire).collect()).unwrap_or_default()
    }

    /// Waits on the person's answer.
    pub fn asks(&self) -> bool {
        self.tab == "needs-you"
    }

    /// What kind of line it is, after its sender.
    pub fn subject(&self) -> String {
        match (&self.card, self.kind.as_str()) {
            (Some(Card::Approval { .. }), _) => "Approval".into(),
            (Some(Card::Change { .. }), _) => "Asks to make a change".into(),
            (Some(Card::Picture), _) => "A picture to draw".into(),
            (_, "result") => "Trigger report".into(),
            (_, "notice") => "Needs a look".into(),
            _ => "Message".into(),
        }
    }

    /// The line in bold: what is asked, or the news's first line.
    pub fn headline(&self) -> String {
        match &self.card {
            Some(Card::Approval { tool, .. }) if self.asks() => approval_line(&self.from, tool.as_deref()),
            Some(Card::Change { title, .. }) if !title.is_empty() => title.clone(),
            _ => first_line(&self.text).unwrap_or_else(|| self.subject()),
        }
    }

    /// The muted line under it, when there's more to say.
    pub fn snippet(&self) -> Option<String> {
        let text = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let headline = self.headline();
        if text.is_empty() || text == headline {
            return None;
        }
        Some(text.strip_prefix(&headline).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or(text))
    }

    /// The chat it came from, when the subject doesn't already say it.
    pub fn place_label(&self) -> Option<String> {
        (self.kind != "message" && !self.title.is_empty()).then(|| format!("in {}", self.title))
    }

    /// Whether a search keeps it: every word typed is in its sender, chat, headline or words.
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!("{} {} {} {}", self.from, self.title, self.headline(), self.text).to_lowercase();
        query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(140).collect())
}

/// The quick actions a row offers on hover, its own answer first (their `quickActions`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Allow,
    Deny,
    Reply,
    Done,
    Open,
}

pub fn actions(row: &Row) -> Vec<Action> {
    match (&row.card, row.asks()) {
        (Some(Card::Approval { .. }) | Some(Card::Change { .. }), true) => vec![Action::Allow, Action::Deny, Action::Open],
        (Some(Card::Picture), true) => vec![Action::Open],
        _ if row.tab == "info" && row.kind == "message" => vec![Action::Reply, Action::Done, Action::Open],
        _ if row.tab == "info" => vec![Action::Done, Action::Open],
        _ => vec![Action::Open],
    }
}

/// What the Inbox row counts: every line waiting on the person and news not read (their
/// `waitingCount`), and chats a bot answered in that no unread line already stands for.
pub fn count(rows: &[Row], bots: &[Bot]) -> usize {
    let lines = rows.iter().filter(|r| r.asks() || (r.tab == "info" && r.unread)).count();
    let covered: std::collections::HashSet<&str> = rows.iter().filter(|r| r.asks() || (r.tab == "info" && r.unread)).map(|r| r.thread.as_str()).collect();
    lines + unread_chats(bots).iter().filter(|c| !covered.contains(c.thread.as_str())).count()
}

/// A chat as the panel's lists show it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatLine {
    pub bot: String,
    pub bot_name: String,
    pub color: String,
    pub mark: String,
    pub initial: String,
    pub thread: String,
    pub title: String,
    pub activity: Activity,
    pub unread: bool,
    pub updated: f64,
}

/// Every chat of every bot and channel, archived ones left out.
fn all_chats(bots: &[Bot]) -> Vec<ChatLine> {
    let mut out = Vec::new();
    for bot in bots {
        let line = |thread: &str, title: &str, activity: Activity, unread: bool, updated: f64| ChatLine {
            bot: bot.id.clone(),
            bot_name: bot.name.clone(),
            color: bot.color.clone(),
            mark: bot.mark.clone(),
            initial: bot.initial(),
            thread: thread.to_string(),
            title: title.to_string(),
            activity,
            unread,
            updated,
        };
        if bot.channel.is_some() {
            let updated = bot.chats.first().map(|c| c.updated).unwrap_or(0.0);
            out.push(line(&bot.thread_id, &format!("#{}", bot.name), bot.activity, bot.unread, updated));
            continue;
        }
        for chat in bot.chats.iter().filter(|c| !c.archived) {
            out.push(line(&chat.id, &chat.title, chat.activity, chat.unread, chat.updated));
        }
    }
    out
}

fn unread_chats(bots: &[Bot]) -> Vec<ChatLine> {
    all_chats(bots).into_iter().filter(|c| c.unread).collect()
}

/// The chats at work, waiting on the person, or answered and unread, apart from those an ask in
/// the list already stands for (their `inboxAttentionEntries`).
pub fn active_chats(bots: &[Bot], rows: &[Row]) -> Vec<ChatLine> {
    let asked: std::collections::HashSet<&str> = rows.iter().filter(|r| r.asks()).map(|r| r.thread.as_str()).collect();
    let mut chats: Vec<ChatLine> = all_chats(bots).into_iter().filter(|c| !asked.contains(c.thread.as_str()) && (c.activity != Activity::Idle || c.unread)).collect();
    chats.sort_by(|a, b| b.updated.total_cmp(&a.updated));
    chats
}

/// Finished chats the person has read, the newest first (their `finishedThreads`).
pub fn finished_chats(bots: &[Bot]) -> Vec<ChatLine> {
    let mut chats: Vec<ChatLine> = all_chats(bots).into_iter().filter(|c| c.activity == Activity::Idle && !c.unread && c.updated > 0.0).collect();
    chats.sort_by(|a, b| b.updated.total_cmp(&a.updated));
    chats.truncate(FINISHED);
    chats
}

/// The Inbox's place in the app: its list as last read, and the panel.
#[derive(Clone, Copy)]
pub struct Inbox {
    pub rows: Signal<Option<Vec<Row>>>,
    pub open: Signal<bool>,
    /// The line open in the panel, to answer or reply to.
    pub reading: Signal<Option<String>>,
    pub wide: Signal<bool>,
}

impl Inbox {
    pub fn new() -> Inbox {
        Inbox { rows: Signal::new(None), open: Signal::new(false), reading: Signal::new(None), wide: Signal::new(false) }
    }

    /// Reads the list again.
    pub fn refresh(self) {
        let mut rows = self.rows;
        spawn(async move {
            if let Ok(answer) = api::get("/api/inbox").await {
                rows.set(Some(Row::list(&answer)));
            }
        });
    }

    /// Changes a row here at once, before the server's answer says so.
    fn patch(self, id: &str, f: impl FnOnce(&mut Row)) {
        let mut rows = self.rows;
        if let Some(row) = rows.write().as_mut().and_then(|r| r.iter_mut().find(|r| r.id == id)) {
            f(row);
        }
    }

    /// Tells the server, and shows its list after.
    fn post(self, path: String) {
        let mut rows = self.rows;
        spawn(async move {
            match api::post(&path, &json!({})).await {
                Ok(answer) if answer["rows"].is_array() => rows.set(Some(Row::list(&answer))),
                _ => self.refresh(),
            }
        });
    }

    pub fn read(self, row: &Row) {
        if row.unread {
            self.patch(&row.id, |r| r.unread = false);
            self.post(format!("/api/inbox/{}/read", row.id));
        }
    }

    pub fn done(self, row: &Row) {
        self.patch(&row.id, |r| {
            r.unread = false;
            r.tab = "done".into();
        });
        self.post(format!("/api/inbox/{}/done", row.id));
    }

    pub fn read_all(self) {
        let mut rows = self.rows;
        if let Some(list) = rows.write().as_mut() {
            for row in list.iter_mut() {
                row.unread = false;
            }
        }
        self.post("/api/inbox/read-all".into());
    }

    /// Answers an ask where it was asked: Allow (Approve) or Deny (Decline).
    pub fn answer(self, row: &Row, yes: bool, problem: Option<Signal<Option<String>>>) {
        let (path, body) = match &row.card {
            Some(Card::Approval { request, .. }) => (format!("/api/threads/{}/respond", row.thread), json!({ "requestId": request, "behavior": if yes { "allow" } else { "deny" } })),
            Some(Card::Change { .. }) => (format!("/api/threads/{}/team/{}", row.thread, row.id), json!({ "approve": yes })),
            _ => return,
        };
        let id = row.id.clone();
        self.patch(&id, |r| {
            r.tab = "done".into();
            r.unread = false;
        });
        spawn(async move {
            if let Err(err) = api::post(&path, &body).await
                && let Some(mut problem) = problem
            {
                problem.set(Some(err));
            }
            self.refresh();
        });
    }
}

impl Default for Inbox {
    fn default() -> Self {
        Inbox::new()
    }
}

/// Opens a chat from the Inbox: in the main view, or on a phone instead of the panel.
pub fn open_chat(state: Signal<State>, ui: Ui, inbox: Inbox, bot: &str, thread: &str) {
    crate::chats::open(state, ui, bot, thread);
    let mut open = inbox.open;
    spawn(async move {
        let mut phone = document::eval("dioxus.send(window.matchMedia('(max-width: 767px)').matches)");
        if phone.recv::<bool>().await.unwrap_or(false) {
            open.set(false);
        }
    });
}

/// The Inbox row at the top of the sidebar, shaped like a bot's row: what waits on the person as
/// its count, and a dot while any bot is at work. It stays when nothing waits, with no count.
#[component]
pub fn InboxRow() -> Element {
    let inbox = use_context::<Inbox>();
    let state = use_context::<Signal<State>>();
    let bots = state.read().bots.clone();
    let count = inbox.rows.read().as_deref().map(|rows| count(rows, &bots)).unwrap_or_else(|| count(&[], &bots));
    let running = bots.iter().any(|b| b.activity != Activity::Idle);
    let shown = if count > 9 { "9+".to_string() } else { count.to_string() };
    let label = if count > 0 { format!("Inbox, {count} waiting") } else { "Inbox".to_string() };
    let open = (inbox.open)();
    let mut ui = use_context::<Ui>();
    let toggle = move |_| {
        let mut opened = inbox.open;
        let now = !opened();
        opened.set(now);
        if now {
            ui.settings_open.set(false);
            inbox.refresh();
        }
    };
    rsx! {
        div { class: "px-3 pb-1",
            button {
                r#type: "button",
                "data-sidebar-inbox": "{count}",
                "aria-label": "{label}",
                title: "{label}",
                "aria-pressed": "{open}",
                class: "flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none hover:bg-raised/60 focus-visible:ring-2 focus-visible:ring-accent/60",
                onclick: toggle,
                span { class: if open { "relative grid size-9 shrink-0 place-items-center rounded-full bg-ink/[0.16] text-ink" } else { "relative grid size-9 shrink-0 place-items-center rounded-full bg-raised text-ink-secondary" },
                    Icon { name: "inbox", size: 16 }
                    if running {
                        span { "data-inbox-running": true, "aria-hidden": "true", class: "absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full border-2 border-panel bg-success" }
                    }
                }
                span { class: "min-w-0 flex-1 truncate text-[14px] font-medium leading-5 text-ink", "Inbox" }
                if count > 0 {
                    span { "aria-hidden": "true", "data-inbox-count": true, class: "shrink-0 rounded-full bg-warning px-1.5 text-[11px] font-semibold tabular-nums text-panel", "{shown}" }
                }
            }
        }
    }
}

const ROUND: &str = "flex size-[34px] shrink-0 items-center justify-center rounded-full bg-composer text-ink ring-1 ring-inset ring-ink/[0.07] hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)]";
const ROW_ACTION: &str = "flex size-7 shrink-0 items-center justify-center rounded-full text-ink-secondary transition-colors hover:bg-ink/[0.07] hover:text-ink active:bg-ink/[0.11] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus";
const NAV_BUTTON: &str = "flex size-7 shrink-0 items-center justify-center rounded-md text-ink-secondary hover:bg-raised hover:text-ink disabled:pointer-events-none disabled:opacity-35";

/// The Inbox panel on the right: its lines, the chats at work, the finished ones, and Done.
#[component]
pub fn InboxPanel() -> Element {
    let inbox = use_context::<Inbox>();
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let mut query = use_signal(String::new);
    let mut show_done = use_signal(|| false);
    let bots = state.read().bots.clone();
    let rows = inbox.rows.read().clone().unwrap_or_default();
    let count = count(&rows, &bots);
    let q = query.read().trim().to_string();
    let mut listed: Vec<Row> = rows.iter().filter(|r| r.tab != "done").cloned().collect();
    listed.sort_by(|a, b| b.at.total_cmp(&a.at));
    let shown: Vec<Row> = listed.iter().filter(|r| r.matches(&q)).cloned().collect();
    let mut done: Vec<Row> = rows.iter().filter(|r| r.tab == "done" && r.matches(&q)).cloned().collect();
    done.sort_by(|a, b| b.at.total_cmp(&a.at));
    let active = active_chats(&bots, &rows);
    let finished = finished_chats(&bots);
    let any_unread = rows.iter().any(|r| r.unread);
    let reading = inbox.reading.read().clone().and_then(|id| rows.iter().find(|r| r.id == id).cloned());
    let position = reading.as_ref().and_then(|r| shown.iter().position(|s| s.id == r.id));
    let wide = (inbox.wide)();
    let wide_icon: &'static str = if wide { "minimize" } else { "maximize" };
    let wide_label: &'static str = if wide { "Narrower" } else { "Wider" };
    let width = if wide { "md:w-[min(640px,60vw)]" } else { "md:w-[320px]" };
    let open_row = move |row: Row| {
        inbox.read(&row);
        if row.asks() || row.kind == "message" {
            let mut reading = inbox.reading;
            reading.set(Some(row.id.clone()));
        } else {
            open_chat(state, ui, inbox, &row.place, &row.thread);
        }
    };
    let run = move |(row, action): (Row, Action)| match action {
        Action::Allow => inbox.answer(&row, true, None),
        Action::Deny => inbox.answer(&row, false, None),
        Action::Done => inbox.done(&row),
        Action::Reply => {
            inbox.read(&row);
            let mut reading = inbox.reading;
            reading.set(Some(row.id.clone()));
        }
        Action::Open => {
            inbox.read(&row);
            open_chat(state, ui, inbox, &row.place, &row.thread);
        }
    };
    let close = move |_| {
        let (mut open, mut reading) = (inbox.open, inbox.reading);
        open.set(false);
        reading.set(None);
    };
    let step = {
        let shown = shown.clone();
        move |by: isize| {
            let Some(at) = position else { return };
            let next = at as isize + by;
            if next >= 0
                && let Some(row) = shown.get(next as usize)
            {
                inbox.read(row);
                let mut reading = inbox.reading;
                reading.set(Some(row.id.clone()));
            }
        }
    };
    rsx! {
        aside {
            "aria-label": "Inbox",
            "data-inbox-panel": "side",
            "data-wide": wide,
            class: "animate-panel-in absolute inset-0 z-40 flex h-full min-w-0 flex-col border-l-[0.5px] border-ink/[0.06] bg-app md:relative md:z-20 md:shrink-0 {width}",
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    e.prevent_default();
                    let (mut open, mut reading) = (inbox.open, inbox.reading);
                    if reading.peek().is_some() { reading.set(None) } else { open.set(false) }
                }
            },
            div { class: "flex h-[50px] shrink-0 items-start gap-2.5 px-[13px] pt-[13px]",
                div { "data-inbox-tab-chip": true, class: "flex h-[34px] min-w-0 flex-1 items-center gap-2 ps-1 text-[14px]",
                    span { class: "shrink-0 text-ink-secondary", Icon { name: "inbox", size: 16 } }
                    span { class: "min-w-0 truncate font-medium text-ink", "Inbox" }
                    if count > 0 {
                        span { "data-inbox-tab-count": true, class: "shrink-0 tabular-nums text-ink-tertiary", "{count}" }
                    }
                }
                if any_unread {
                    button { r#type: "button", "data-inbox-mark-all": true, class: "mt-[9px] shrink-0 whitespace-nowrap text-[12px] text-accent hover:underline", onclick: move |_| inbox.read_all(), "Mark all as read" }
                }
                button { r#type: "button", class: "{ROUND} max-md:hidden", "aria-pressed": "{wide}", "aria-label": wide_label, title: wide_label,
                    onclick: move |_| { let mut w = inbox.wide; w.toggle(); },
                    Icon { name: wide_icon, size: 15 }
                }
                button { r#type: "button", "data-inbox-close": true, class: ROUND, "aria-label": "Close inbox", title: "Close inbox", onclick: close,
                    Icon { name: "x", size: 16 }
                }
            }
            div { class: "flex min-h-0 flex-1 flex-col overflow-hidden",
                if let Some(row) = reading {
                    Reading { key: "{row.id}", row, position, total: shown.len(), on_step: step }
                } else {
                    div { class: "min-h-0 flex-1 overflow-y-auto",
                        if !listed.is_empty() {
                            label { class: "mx-[13px] my-2.5 flex h-8 items-center gap-2 rounded-lg bg-ink/[0.045] px-2.5 ring-[0.5px] ring-ink/[0.12] focus-within:ring-ink/30",
                                span { class: "shrink-0 text-ink-tertiary", Icon { name: "search", size: 14 } }
                                input { r#type: "search", value: "{query}", placeholder: "Search inbox", "aria-label": "Search inbox",
                                    class: "min-w-0 flex-1 bg-transparent text-[13px] text-ink outline-none placeholder:text-ink-tertiary [&::-webkit-search-cancel-button]:hidden",
                                    oninput: move |e| query.set(e.value()),
                                }
                            }
                        }
                        div { role: "list", "data-inbox-list": true, "aria-label": "Inbox", class: "space-y-0.5 px-1.5 pb-1.5",
                            if shown.is_empty() {
                                if !q.is_empty() && !listed.is_empty() {
                                    p { class: "px-4 py-8 text-center text-[13px] text-ink-secondary", "Nothing in the inbox matches." }
                                } else {
                                    div { "data-inbox-empty": true, class: "mx-auto flex max-w-[16rem] flex-col items-center px-4 py-10 text-center",
                                        span { class: "text-ink-tertiary", Icon { name: "inbox", size: 22 } }
                                        p { class: "mt-3 text-[14px] font-medium text-ink", "Nothing for you yet." }
                                        p { class: "mt-1 text-[13px] leading-[18px] text-ink-tertiary", "When a bot needs your OK, sends you a message or a trigger finishes, it shows up here." }
                                    }
                                }
                            }
                            for row in shown.iter().cloned() {
                                RowItem { key: "{row.id}", row, on_open: open_row, on_action: run }
                            }
                        }
                        if q.is_empty() && !active.is_empty() {
                            section { "data-inbox-active": true, "aria-label": "Active chats", class: "border-t border-hairline/40 px-1.5 py-1.5",
                                div { class: "px-2 py-0.5 text-[12px] font-medium text-ink-secondary", "Active chats" }
                                for chat in active {
                                    ChatItem { key: "{chat.thread}", chat, on_open: move |(bot, thread): (String, String)| open_chat(state, ui, inbox, &bot, &thread) }
                                }
                            }
                        }
                        if q.is_empty() && !finished.is_empty() {
                            section { "data-inbox-finished": true, "aria-label": "Finished", class: "border-t border-hairline/40 px-1.5 py-1.5",
                                div { class: "px-2 py-0.5 text-[12px] font-medium text-ink-secondary", "Finished" }
                                for chat in finished {
                                    ChatItem { key: "{chat.thread}", chat, on_open: move |(bot, thread): (String, String)| open_chat(state, ui, inbox, &bot, &thread) }
                                }
                            }
                        }
                        if !done.is_empty() {
                            section { "data-inbox-done": true, class: "px-1.5 pb-1.5",
                                button { r#type: "button", "data-inbox-done-toggle": true, "aria-expanded": "{show_done}",
                                    class: "flex w-full items-center gap-1.5 rounded-lg px-2 py-1.5 text-[12px] text-ink-tertiary hover:bg-raised/70 hover:text-ink-secondary",
                                    onclick: move |_| show_done.toggle(),
                                    Icon { name: "check", size: 13 }
                                    span { class: "font-medium", "Done" }
                                    span { class: "tabular-nums", "{done.len()}" }
                                }
                                if show_done() {
                                    for row in done.iter().cloned() {
                                        RowItem { key: "{row.id}", row, on_open: open_row, on_action: run }
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

/// The sender's picture: its bot's, when the app knows the bot.
#[component]
fn Sender(row: Row, size: u32) -> Element {
    let state = use_context::<Signal<State>>();
    let bot = state.read().bots.iter().find(|b| b.id == row.from_bot).cloned();
    match bot {
        Some(bot) => rsx! { BotMark { color: bot.color.clone(), initial: bot.initial(), size, mark: bot.mark.clone(), state: bot.activity.mood() } },
        None => rsx! { BotMark { color: row.color.clone(), initial: row.from.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), size } },
    }
}

/// One line of the list (their `InboxRowItem`): who, where and when; what it asks or says; and
/// its quick actions on hover.
#[component]
fn RowItem(row: Row, on_open: EventHandler<Row>, on_action: EventHandler<(Row, Action)>) -> Element {
    let headline = row.headline();
    let snippet = row.snippet();
    let place = row.place_label();
    let when = short_when(row.at);
    let strong = row.unread || row.asks();
    let weight = if row.unread { "font-semibold text-ink" } else { "font-normal text-ink-secondary" };
    let title = [row.from.as_str(), headline.as_str(), place.as_deref().unwrap_or("")].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
    let opened = row.clone();
    rsx! {
        div { "data-inbox-row": "{row.id}", "data-tab": "{row.tab}", "data-unread": row.unread, role: "listitem",
            class: "group relative flex min-w-0 items-center gap-2 rounded-lg pr-1.5 focus-within:bg-raised/70 hover:bg-raised/70",
            button { r#type: "button", title: "{title}", onclick: move |_| on_open.call(opened.clone()),
                class: "flex min-w-0 flex-1 items-start gap-2 py-2 pl-2.5 text-left text-[13px] outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/60 {weight}",
                if row.asks() {
                    span { "data-inbox-waits": true, role: "img", "aria-label": "Waits on you", class: "mt-[7px] size-1.5 shrink-0 rounded-full bg-accent" }
                } else {
                    span { "aria-hidden": "true", class: if row.unread { "mt-[7px] size-1.5 shrink-0 rounded-full bg-accent" } else { "mt-[7px] size-1.5 shrink-0 rounded-full bg-transparent" } }
                }
                span { class: "mt-px shrink-0", "aria-hidden": "true", Sender { row: row.clone(), size: 18 } }
                span { class: "min-w-0 flex-1 space-y-px",
                    span { class: "flex items-baseline gap-1.5",
                        span { "data-inbox-from": true, class: "min-w-0 shrink truncate font-semibold", "{row.from}" }
                        if let Some(place) = place {
                            span { "data-inbox-place": true, class: "min-w-0 flex-1 truncate text-[12px] font-normal text-ink-tertiary", "{place}" }
                        }
                        span { class: "ms-auto shrink-0 ps-1 text-[11px] font-normal tabular-nums text-ink-tertiary group-focus-within:invisible group-hover:invisible", "{when}" }
                    }
                    span { "data-inbox-subject": true, class: if strong { "block truncate text-[13px] font-semibold text-ink" } else { "block truncate text-[13px] font-semibold text-ink-secondary" }, "{headline}" }
                    if let Some(snippet) = snippet {
                        span { class: "block truncate text-[12px] font-normal text-ink-tertiary", "{snippet}" }
                    }
                }
            }
            span { class: "absolute right-1.5 top-1.5 hidden items-center gap-0.5 rounded-md bg-raised group-focus-within:flex group-hover:flex",
                for action in actions(&row) {
                    ActionButton { key: "{action:?}", row: row.clone(), action, on_action }
                }
            }
        }
    }
}

#[component]
fn ActionButton(row: Row, action: Action, on_action: EventHandler<(Row, Action)>) -> Element {
    let change = matches!(row.card, Some(Card::Change { .. }));
    let (icon, label): (&'static str, &'static str) = match action {
        Action::Allow if change => ("check", "Approve"),
        Action::Allow => ("check", "Allow once"),
        Action::Deny if change => ("x", "Decline"),
        Action::Deny => ("x", "Deny"),
        Action::Reply => ("reply", "Reply"),
        Action::Done => ("check", "Done"),
        Action::Open => ("external", "Open in chat"),
    };
    let which = match action {
        Action::Allow => "allow",
        Action::Deny => "deny",
        Action::Reply => "reply",
        Action::Done => "done",
        Action::Open => "open",
    };
    rsx! {
        button { r#type: "button", "data-inbox-action": which, "aria-label": label, title: label, class: ROW_ACTION,
            onclick: move |e| { e.stop_propagation(); on_action.call((row.clone(), action)); },
            Icon { name: icon, size: 14 }
        }
    }
}

/// A chat in the Active or Finished list: its bot, its name, and what it's doing.
#[component]
fn ChatItem(chat: ChatLine, on_open: EventHandler<(String, String)>) -> Element {
    let status = match (chat.activity, chat.unread) {
        (Activity::WaitingOnYou, _) => "Waiting for you",
        (Activity::Working, _) => "Working…",
        (_, true) => "New reply",
        _ => "",
    };
    let when = short_when(chat.updated);
    let (bot, thread) = (chat.bot.clone(), chat.thread.clone());
    let weight = if chat.unread { "font-semibold text-ink" } else { "text-ink-secondary" };
    rsx! {
        button { r#type: "button", "data-inbox-chat": "{chat.thread}", title: "{chat.bot_name} · {chat.title}",
            class: "group flex w-full min-w-0 items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] hover:bg-raised/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/60",
            onclick: move |_| on_open.call((bot.clone(), thread.clone())),
            span { class: "relative shrink-0", "aria-hidden": "true",
                BotMark { color: chat.color.clone(), initial: chat.initial.clone(), size: 18, mark: chat.mark.clone(), state: chat.activity.mood() }
                if chat.unread {
                    span { class: "absolute -right-0.5 -top-0.5 size-2 rounded-full bg-accent ring-2 ring-[var(--color-app)]" }
                }
            }
            span { class: "min-w-0 flex-1 truncate {weight}",
                span { class: "font-medium text-ink", "{chat.bot_name}" }
                span { class: "text-ink-tertiary", " · " }
                "{chat.title}"
            }
            if !status.is_empty() {
                span { class: if chat.activity == Activity::WaitingOnYou { "shrink-0 text-[11px] text-warning" } else { "shrink-0 text-[11px] text-ink-tertiary" }, "{status}" }
            } else {
                span { class: "shrink-0 text-[11px] tabular-nums text-ink-tertiary", "{when}" }
            }
        }
    }
}

/// One line open in the panel (their `InboxReading`): where it came from, what it says, and the
/// person's answer: Allow or Deny, Approve or Decline, or a reply sent to its chat.
#[component]
fn Reading(row: Row, position: Option<usize>, total: usize, on_step: EventHandler<isize>) -> Element {
    let inbox = use_context::<Inbox>();
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let mut problem = use_signal(|| None::<String>);
    let mut reply = use_signal(String::new);
    let mut sending = use_signal(|| false);
    // What the person sent from here, shown under their name.
    let mut sent = use_signal(|| None::<String>);
    let headline = row.headline();
    let place = if row.title.is_empty() { row.from.clone() } else { row.title.clone() };
    let when = short_when(row.at);
    let words = match &row.card {
        Some(Card::Approval { .. }) => None,
        _ => Some(row.text.clone()).filter(|t| !t.trim().is_empty()),
    };
    let command = match &row.card {
        Some(Card::Approval { .. }) => Some(row.text.clone()).filter(|t| !t.trim().is_empty()),
        _ => None,
    };
    let (place_bot, thread) = (row.place.clone(), row.thread.clone());
    let open = use_callback(move |_: ()| open_chat(state, ui, inbox, &place_bot, &thread));
    let back = move |_| {
        let mut reading = inbox.reading;
        reading.set(None);
    };
    let (first, last) = (position.is_none_or(|p| p == 0), position.is_none_or(|p| p + 1 >= total));
    let send = {
        let row = row.clone();
        move |_| {
            let text = reply.read().trim().to_string();
            if text.is_empty() || sending() {
                return;
            }
            sending.set(true);
            problem.set(None);
            let path = format!("/api/bots/{}/messages", row.place);
            let body = json!({ "text": text.clone(), "threadId": row.thread });
            spawn(async move {
                match api::post(&path, &body).await {
                    Ok(_) => {
                        reply.set(String::new());
                        sent.set(Some(text));
                        inbox.refresh();
                    }
                    Err(err) => problem.set(Some(err)),
                }
                sending.set(false);
            });
        }
    };
    let (yes_row, no_row, done_row) = (row.clone(), row.clone(), row.clone());
    rsx! {
        article { "data-inbox-reading": "{row.id}", "aria-label": "{row.from} · {headline}", class: "flex min-h-0 flex-1 flex-col",
            header { class: "flex items-center gap-1 px-2 py-1.5",
                button { r#type: "button", "aria-label": "Back to the list", title: "Back to the list", onclick: back,
                    class: "flex h-7 shrink-0 items-center gap-0.5 rounded-md pe-2 ps-1 text-[13px] font-medium text-ink hover:bg-raised",
                    Icon { name: "chevron-left", size: 16 }
                    "Inbox"
                }
                span { class: "flex-1" }
                if let Some(p) = position {
                    span { "data-inbox-position": true, class: "px-1 text-[12px] tabular-nums text-ink-tertiary", "{p + 1} of {total}" }
                }
                button { r#type: "button", disabled: first, "aria-label": "Previous", title: "Previous", class: NAV_BUTTON, onclick: move |_| on_step.call(-1), Icon { name: "chevron-up", size: 16 } }
                button { r#type: "button", disabled: last, "aria-label": "Next", title: "Next", class: NAV_BUTTON, onclick: move |_| on_step.call(1), Icon { name: "chevron-down", size: 16 } }
                if row.tab == "info" {
                    button { r#type: "button", "data-inbox-action": "done", "aria-label": "Done", title: "Done", class: NAV_BUTTON, onclick: move |_| inbox.done(&done_row), Icon { name: "check", size: 16 } }
                }
            }
            nav { "aria-label": "Where this came from", class: "flex min-w-0 items-center gap-1 border-b border-hairline/40 px-3.5 pb-2 text-[12px] text-ink-tertiary",
                button { r#type: "button", "data-inbox-action": "open", title: "Open in chat", onclick: move |_| open(()), class: "min-w-0 max-w-[45%] shrink truncate rounded hover:text-ink hover:underline", "{place}" }
                span { class: "shrink-0", Icon { name: "chevron-right", size: 12 } }
                span { class: "min-w-0 truncate font-medium text-ink-secondary", "{headline}" }
            }
            div { class: "min-h-0 flex-1 space-y-4 overflow-y-auto px-3.5 py-3",
                section { "aria-label": "{row.from}", class: "flex gap-2.5",
                    span { class: "mt-0.5 shrink-0", "aria-hidden": "true", Sender { row: row.clone(), size: 22 } }
                    div { class: "min-w-0 flex-1",
                        div { class: "flex items-baseline gap-2",
                            span { class: "min-w-0 truncate text-[13px] font-semibold text-ink", "{row.from}" }
                            span { class: "shrink-0 text-[11px] tabular-nums text-ink-tertiary", "{when}" }
                        }
                        div { "data-inbox-words": true, class: "mt-1 space-y-2 text-[14px] leading-6 text-ink",
                            if let Some(Card::Approval { .. }) = &row.card {
                                p { "{headline}" }
                            }
                            if let Some(words) = words {
                                p { class: "whitespace-pre-wrap break-words", "{words}" }
                            }
                            if let Some(command) = command {
                                div { class: "rounded-xl bg-ink/[0.045] px-3 py-2 font-mono text-[12px] leading-[18px] whitespace-pre-wrap text-ink-secondary [overflow-wrap:anywhere]", "{command}" }
                            }
                            if let Some(Card::Change { lines, warning, .. }) = &row.card {
                                ul { class: "list-disc space-y-0.5 ps-5 text-[13px] text-ink-secondary",
                                    for line in lines.iter() {
                                        li { "{line}" }
                                    }
                                }
                                if let Some(warning) = warning {
                                    p { class: "text-[13px] text-warning", "{warning}" }
                                }
                            }
                        }
                    }
                }
                section { "data-inbox-you": true, "aria-label": "You", class: "flex gap-2.5",
                    span { "aria-hidden": "true", class: "mt-0.5 flex size-[22px] shrink-0 items-center justify-center rounded-full bg-ink/[0.08] text-ink-secondary", Icon { name: "user", size: 13 } }
                    div { class: "min-w-0 flex-1 space-y-2",
                        div { class: "text-[13px] font-semibold text-ink", "You" }
                        if let Some(said) = sent() {
                            p { "data-inbox-sent": true, class: "rounded-xl bg-ink/[0.045] px-3 py-2 text-[13px] text-ink-secondary",
                                span { class: "block text-[11px] font-medium text-ink-tertiary", "You replied in the chat" }
                                span { class: "whitespace-pre-wrap break-words", "{said}" }
                            }
                        } else if row.tab == "done" {
                            p { class: "flex items-center gap-1.5 text-[13px] text-ink-secondary", Icon { name: "check", size: 14 } "Done" }
                        }
                        if let Some(err) = problem() {
                            p { role: "alert", class: "text-[13px] text-warning", "Couldn't send: {err}" }
                        }
                        match (&row.card, row.asks()) {
                            (Some(Card::Approval { .. }) | Some(Card::Change { .. }), true) => {
                                let change = matches!(row.card, Some(Card::Change { .. }));
                                rsx! {
                                    div { class: "flex flex-wrap gap-2",
                                        button { r#type: "button", "data-inbox-answer": "deny",
                                            class: "h-8 rounded-full bg-composer px-3.5 text-[13px] font-medium text-ink ring-1 ring-inset ring-ink/[0.1] hover:bg-ink/[0.06]",
                                            onclick: move |_| inbox.answer(&no_row, false, Some(problem)),
                                            if change { "Decline" } else { "Deny" }
                                        }
                                        button { r#type: "button", "data-inbox-answer": "allow",
                                            class: "h-8 rounded-full bg-ink px-3.5 text-[13px] font-medium text-app hover:opacity-90",
                                            onclick: move |_| inbox.answer(&yes_row, true, Some(problem)),
                                            if change { "Approve" } else { "Allow once" }
                                        }
                                    }
                                }
                            }
                            (Some(Card::Picture), true) => rsx! {
                                p { class: "text-[13px] text-ink-secondary", "Choose who draws it in the chat it came from." }
                                button { r#type: "button", class: "h-8 rounded-full bg-composer px-3.5 text-[13px] font-medium text-ink ring-1 ring-inset ring-ink/[0.1] hover:bg-ink/[0.06]", onclick: move |_| open(()), "Open in chat" }
                            },
                            _ if row.kind == "message" => rsx! {
                                div { class: "rounded-2xl bg-composer ring-1 ring-inset ring-ink/[0.08] focus-within:ring-ink/25",
                                    textarea { "data-inbox-reply": true, rows: "3", placeholder: "Write your answer", "aria-label": "Write your answer", value: "{reply}",
                                        class: "block w-full resize-none bg-transparent px-3 pt-2 text-[14px] leading-5 text-ink outline-none placeholder:text-ink-tertiary",
                                        oninput: move |e| reply.set(e.value()),
                                        onkeydown: {
                                            let mut send = send.clone();
                                            move |e: KeyboardEvent| if e.key() == Key::Enter && !e.modifiers().shift() { e.prevent_default(); send(()); }
                                        },
                                    }
                                    div { class: "flex justify-end px-2 pb-2",
                                        button { r#type: "button", "data-inbox-send": true, disabled: reply.read().trim().is_empty() || sending(),
                                            class: "h-7 rounded-full bg-ink px-3 text-[12px] font-medium text-app disabled:opacity-40",
                                            onclick: move |_| { let mut send = send.clone(); send(()); },
                                            "Reply"
                                        }
                                    }
                                }
                            },
                            _ => rsx! {
                                button { r#type: "button", class: "h-8 rounded-full bg-composer px-3.5 text-[13px] font-medium text-ink ring-1 ring-inset ring-ink/[0.1] hover:bg-ink/[0.06]", onclick: move |_| open(()), "Open in chat" }
                            },
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(wire: Value) -> Row {
        Row::from_wire(&wire).unwrap()
    }

    #[test]
    fn asks_read_as_what_they_ask_and_offer_their_answers() {
        let ask = row(json!({ "id": "cb-3", "kind": "ask", "tab": "needs-you", "from": { "name": "Scout", "botId": "b1" }, "origin": { "botId": "b1", "threadId": "t1", "title": "Hi" }, "text": "echo hi", "at": 5.0, "unread": true, "card": { "type": "approval", "requestId": "r1", "tool": "Bash" } }));
        assert_eq!(ask.headline(), "Scout wants to run a command");
        assert_eq!(ask.snippet().as_deref(), Some("echo hi"));
        assert_eq!(actions(&ask), [Action::Allow, Action::Deny, Action::Open]);
        assert_eq!(ask.place_label().as_deref(), Some("in Hi"));
        let change = row(json!({ "id": "cb-4", "kind": "ask", "tab": "needs-you", "from": { "name": "Kiwi" }, "origin": { "threadId": "t1" }, "text": "Make Quill", "card": { "type": "change", "title": "Make Quill", "lines": ["a"] } }));
        assert_eq!((change.headline(), change.snippet()), ("Make Quill".to_string(), None));
        let news = row(json!({ "id": "in-1", "kind": "result", "tab": "info", "from": { "name": "Scout" }, "origin": { "threadId": "t2" }, "text": "Morning: all clear\nmore" }));
        assert_eq!(news.headline(), "Morning: all clear");
        assert_eq!(actions(&news), [Action::Done, Action::Open]);
        assert!(news.matches("morning CLEAR") && !news.matches("rain"));
    }

    #[test]
    fn the_count_takes_asks_unread_news_and_chats_nothing_else_covers() {
        let rows = vec![
            row(json!({ "id": "a", "kind": "ask", "tab": "needs-you", "from": {}, "origin": { "threadId": "t1" }, "unread": false })),
            row(json!({ "id": "b", "kind": "message", "tab": "info", "from": {}, "origin": { "threadId": "t2" }, "unread": true })),
            row(json!({ "id": "c", "kind": "result", "tab": "info", "from": {}, "origin": { "threadId": "t3" }, "unread": false })),
            row(json!({ "id": "d", "kind": "result", "tab": "done", "from": {}, "origin": { "threadId": "t4" }, "unread": true })),
        ];
        let bot = Bot::from_wire(&json!({ "id": "b1", "threadId": "t2", "name": "Scout", "tasks": [
            { "threadId": "t2", "title": "x", "unread": true },
            { "threadId": "t5", "title": "y", "unread": true },
            { "threadId": "t6", "title": "z", "unread": true, "archivedAt": 1 },
        ] }))
        .unwrap();
        assert_eq!(count(&rows, std::slice::from_ref(&bot)), 3, "the ask, the unread message, and t5");
        let active: Vec<String> = active_chats(&[bot], &rows).into_iter().map(|c| c.thread).collect();
        assert_eq!(active, ["t2", "t5"]);
    }
}
