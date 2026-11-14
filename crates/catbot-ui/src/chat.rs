//! A bot's conversation, read only: its header and transcript, with earlier messages loaded
//! as the person scrolls up.
//!
//! Rows follow the full layout of OpenMausBot's `src/components/ChatView.tsx`, `ActivityRun.tsx`,
//! `ToolActivity.tsx` and `DaySeparator.tsx` (Apache-2.0, see NOTICE), for the message kinds
//! Catbot sends. A run of tool steps folds into one line, as theirs does; their extra folding of
//! a finished turn under "Worked for" is not copied yet.

use dioxus::prelude::*;
use serde_json::Value;

use crate::api;
use crate::bots::Bot;
use crate::markdown;
use crate::sidebar::BotMark;

/// Messages fetched at a time.
const PAGE: usize = 50;
/// How close to the top (px) the person scrolls before earlier messages load.
const LOAD_EARLIER_AT: f64 = 160.0;
/// Their elevated card look (`ELEVATED_CARD`, `ELEVATED_FLOAT`).
const ELEVATED_FLOAT: &str = "bg-composer ring-[0.5px] ring-ink/10 shadow-(--shadow-float)";

/// One message of the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub id: String,
    /// When it was sent, in ms since the epoch.
    pub at: f64,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Something the person wrote.
    Person(String),
    /// Something the bot wrote (markdown).
    Reply(String),
    /// A step of the bot's work: a tool it used, or a note. `ok` is None while it runs.
    Step { name: String, ok: Option<bool> },
    /// An approval card: what the bot asked to do, and how the person answered.
    Card { title: String, subtitle: String, answer: Answer },
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
            ("text", Some("user")) => Body::Person(text()),
            ("text", _) => Body::Reply(text()),
            ("activity", _) => {
                let name = wire["tool"]["name"].as_str().unwrap_or_default();
                Body::Step { name: name.strip_prefix("notice: ").unwrap_or(name).to_string(), ok: wire["tool"]["ok"].as_bool() }
            }
            ("options", _) => {
                let card = &wire["card"];
                let answer = match card["answered"].as_str() {
                    Some("allow") if card["dismissed"] != true => Answer::Allowed,
                    Some(_) if card["dismissed"] == true => Answer::Dismissed,
                    Some(_) => Answer::Denied,
                    None if card["dismissed"] == true => Answer::Dismissed,
                    None => Answer::Waiting,
                };
                Body::Card {
                    title: card["title"].as_str().unwrap_or("Approval needed").to_string(),
                    subtitle: card["subtitle"].as_str().unwrap_or_default().to_string(),
                    answer,
                }
            }
            _ => return None,
        };
        Some(Msg { id, at, body })
    }

    fn is_step(&self) -> bool {
        matches!(self.body, Body::Step { .. })
    }
}

/// What the transcript draws: single messages, and runs of two or more steps folded into a line.
#[derive(Clone, Debug, PartialEq)]
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
fn day(at: f64) -> i64 {
    let offset_ms = js_offset_minutes(at) * 60_000.0;
    ((at - offset_ms) / 86_400_000.0).floor() as i64
}

#[cfg(target_arch = "wasm32")]
fn js_offset_minutes(at: f64) -> f64 {
    js_sys::Date::new(&at.into()).get_timezone_offset()
}

/// Off the web (tests), days are counted in UTC.
#[cfg(not(target_arch = "wasm32"))]
fn js_offset_minutes(_at: f64) -> f64 {
    0.0
}

#[component]
pub fn Chat(bot: Bot) -> Element {
    let thread = bot.thread_id.clone();
    let mut messages = use_signal(Vec::<Msg>::new);
    let mut more = use_signal(|| false);
    let mut loading = use_signal(|| true);
    let mut problem = use_signal(|| None::<String>);

    // The newest page, then the view jumps to the end.
    let first_thread = thread.clone();
    use_future(move || {
        let thread = first_thread.clone();
        async move {
            match api::get(&format!("/api/threads/{thread}/messages?limit={PAGE}")).await {
                Ok(page) => {
                    messages.set(page_messages(&page));
                    more.set(page["hasMore"] == true);
                    loading.set(false);
                    // Two frames: the first lays the rows out, the second has their real height.
                    document::eval(r#"requestAnimationFrame(() => requestAnimationFrame(() => { const el = document.getElementById("chat-scroll"); if (el) el.scrollTop = el.scrollHeight; }));"#);
                }
                Err(err) => {
                    loading.set(false);
                    problem.set(Some(err));
                }
            }
        }
    });

    // Earlier messages, kept in place on screen while the page grows above them.
    let older_thread = thread.clone();
    let load_earlier = move || {
        let thread = older_thread.clone();
        async move {
            let Some(before) = messages.peek().first().map(|m| m.id.clone()) else { return };
            loading.set(true);
            let keep = document::eval(
                r#"const el = document.getElementById("chat-scroll");
                   const height = el.scrollHeight, top = el.scrollTop;
                   await dioxus.recv();
                   requestAnimationFrame(() => { el.scrollTop = el.scrollHeight - height + top; });"#,
            );
            match api::get(&format!("/api/threads/{thread}/messages?limit={PAGE}&before={before}")).await {
                Ok(page) => {
                    let mut earlier = page_messages(&page);
                    earlier.extend(messages.peek().iter().cloned());
                    messages.set(earlier);
                    more.set(page["hasMore"] == true);
                }
                Err(err) => problem.set(Some(err)),
            }
            let _ = keep.send(true);
            loading.set(false);
        }
    };

    // Each item, with the day line above it when it starts a new day.
    let today = day(js_now());
    let mut previous_day: Option<i64> = None;
    let rows: Vec<(Item, Option<String>)> = group(&messages.read())
        .into_iter()
        .map(|item| {
            let at = item.first().at;
            let this_day = day(at);
            let stamp = (previous_day != Some(this_day)).then(|| format!("{} {}", day_label(at, today), clock(at)));
            previous_day = Some(this_day);
            (item, stamp)
        })
        .collect();
    rsx! {
        main { class: "relative flex h-full min-w-0 flex-1 flex-col bg-app",
            ChatHead { bot: bot.clone() }
            div {
                id: "chat-scroll",
                class: "glass-scroller min-h-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-y-contain px-5 [overflow-anchor:none]",
                onscroll: move |e| {
                    let near_top = e.data().scroll_top() < LOAD_EARLIER_AT;
                    if near_top && more() && !loading() {
                        spawn(load_earlier.clone()());
                    }
                },
                div {
                    // Not their `glass-scroller-content`: it sets the bottom padding from glass-bar sizes this view has no use for.
                    class: "chat-column relative flex w-full flex-col gap-3 pt-2 pb-8",
                    role: "log",
                    "aria-live": "off",
                    "aria-label": "Conversation with {bot.name}",
                    if loading() && !messages.read().is_empty() {
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
                    if !loading() && messages.read().is_empty() && problem().is_none() {
                        EmptyChat { bot: bot.clone() }
                    }
                    for (item, stamp) in rows {
                        div { key: "{item.key()}", class: "contents",
                            if let Some(stamp) = stamp {
                                div { class: "py-3 text-center text-[13px] text-ink-secondary", "{stamp}" }
                            }
                            {
                                match item {
                                    Item::One(m) => rsx! { Row { message: m } },
                                    Item::Run(steps) => rsx! { Run { steps } },
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn page_messages(page: &Value) -> Vec<Msg> {
    page["messages"].as_array().map(|list| list.iter().filter_map(Msg::from_wire).collect()).unwrap_or_default()
}

/// The bar above the conversation: the bot's mark and name, in their floating pill.
#[component]
fn ChatHead(bot: Bot) -> Element {
    rsx! {
        div { class: "z-[25] shrink-0 bg-app/85 px-5 py-3 backdrop-blur-md",
            div { class: "flex items-center justify-center",
                div { "data-chathead-pill": true, class: "flex min-w-0 items-center gap-2 rounded-full py-0.5 pl-1.5 pr-3 text-ink {ELEVATED_FLOAT}",
                    BotMark { color: bot.color.clone(), initial: bot.initial(), size: 24 }
                    span { class: "truncate text-[14px] font-semibold text-ink", "{bot.name}" }
                }
            }
        }
    }
}

#[component]
fn EmptyChat(bot: Bot) -> Element {
    rsx! {
        div { class: "flex flex-col items-center gap-3 py-24 text-center",
            BotMark { color: bot.color.clone(), initial: bot.initial(), size: 64 }
            p { class: "text-[15px] font-semibold text-ink", "{bot.name}" }
            p { class: "text-[13px] text-ink-secondary", "No messages yet." }
        }
    }
}

/// One message: the person's bubble on the right, the bot's reply on the left, a step, or a card.
#[component]
fn Row(message: Msg) -> Element {
    let when = clock(message.at);
    let full_time = full_date(message.at);
    match &message.body {
        Body::Person(text) => rsx! {
            div { class: "group flex w-full flex-col items-end animate-msg-in",
                div { class: "flex w-full items-center justify-end gap-1.5",
                    span { class: "order-first mr-2 self-end pb-1 text-[11px] tabular-nums text-ink-tertiary opacity-0 transition-opacity group-hover:opacity-100", "{when}" }
                    div {
                        class: "w-fit max-w-[min(var(--chat-bubble-max),var(--chat-bubble-pct))] rounded-2xl bg-bubble-user px-4 py-2.5 text-[15px] leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere] text-ink",
                        title: "{full_time}",
                        div { class: "chat-text", "{text}" }
                    }
                }
            }
        },
        Body::Reply(text) => {
            let shape = if markdown::is_formatted(text) {
                "w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] rounded-2xl px-1 text-[15px] leading-relaxed text-ink"
            } else {
                "w-fit max-w-[min(var(--chat-bubble-max),var(--chat-bubble-pct))] rounded-2xl bg-card px-4 py-2.5 text-[15px] leading-relaxed text-ink"
            };
            let html = markdown::to_html(text);
            rsx! {
                div { class: "group flex w-full flex-col items-start",
                    div { class: "flex w-full items-center justify-start gap-1.5",
                        div { class: "{shape}", title: "{full_time}",
                            div { class: "chat-md min-w-0 [&>*+*]:mt-2", dangerous_inner_html: "{html}" }
                        }
                        span { class: "ml-2 self-end pb-1 text-[11px] tabular-nums text-ink-tertiary opacity-0 transition-opacity group-hover:opacity-100", "{when}" }
                    }
                }
            }
        }
        Body::Step { name, ok } => rsx! { Step { name: name.clone(), ok: *ok } },
        Body::Card { title, subtitle, answer } => rsx! { Card { title: title.clone(), subtitle: subtitle.clone(), answer: *answer } },
    }
}

/// One tool step, as their `ToolActivity` chip draws it closed.
#[component]
fn Step(name: String, ok: Option<bool>) -> Element {
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
fn Run(steps: Vec<Msg>) -> Element {
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
                    Row { key: "{step.id}", message: step }
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

/// An approval card, read only. Answering it arrives with stage 6.5.
#[component]
fn Card(title: String, subtitle: String, answer: Answer) -> Element {
    let (status, tone) = match answer {
        Answer::Waiting => ("Waiting for your answer", "text-warning"),
        Answer::Allowed => ("Allowed", "text-success"),
        Answer::Denied => ("Denied", "text-danger"),
        Answer::Dismissed => ("No answer before the turn ended", "text-ink-secondary"),
    };
    rsx! {
        div { class: "flex w-full justify-start",
            div { class: "w-full max-w-[min(var(--chat-bubble-max),var(--chat-bubble-pct))] rounded-2xl px-4 py-3 {ELEVATED_FLOAT}",
                div { class: "text-[14px] font-semibold text-ink", "{title}" }
                if !subtitle.is_empty() {
                    div { class: "mt-1 break-words font-mono text-[13px] text-ink-secondary", "{subtitle}" }
                }
                div { class: "mt-2 text-[13px] font-medium {tone}", "{status}" }
            }
        }
    }
}

// Lucide shapes (ISC, see NOTICE), drawn at 13px.
const CHECK: &str = "M20 6 9 17l-5-5";
const CROSS: &str = "M18 6 6 18M6 6l12 12";
const CHEVRON: &str = "m9 18 6-6-6-6";
const CIRCLE: &str = "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20";

#[component]
fn Icon(path: &'static str, class: &'static str) -> Element {
    rsx! {
        svg {
            class: "shrink-0 {class}",
            width: "13",
            height: "13",
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

fn js_now() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now();
    #[cfg(not(target_arch = "wasm32"))]
    return 0.0;
}

/// "Today", "Yesterday", or "Sat, Nov 14", as their `dayLabel` says it in English.
fn day_label(at: f64, today: i64) -> String {
    match today - day(at) {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ => date_string(at, &[("weekday", "short"), ("month", "short"), ("day", "numeric")], false),
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

#[cfg(not(target_arch = "wasm32"))]
fn date_string(at: f64, _fields: &[(&str, &str)], _time_only: bool) -> String {
    format!("{at}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(id: &str, name: &str, at: f64) -> Msg {
        Msg { id: id.into(), at, body: Body::Step { name: name.into(), ok: Some(true) } }
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

    #[test]
    fn steps_fold_into_runs() {
        let reply = Msg { id: "r".into(), at: 3.0, body: Body::Reply("done".into()) };
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
    fn a_new_day_splits_a_run() {
        let items = group(&[step("a", "Read", 1.0), step("b", "Read", 86_400_000.0 + 1.0)]);
        assert_eq!(items.len(), 2);
    }
}
