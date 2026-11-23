//! A bot's panel on the right, as MausBot's simple layout shows it (`bot-panel/PanelShell.tsx`,
//! MIT): round buttons at the top (its settings, close), the bot large with its name and what it's
//! doing, then three tabs.
//!
//! - **Details**: its model and thinking level, its chats, things to ask (the Chief only), the
//!   conversation's tools (find, start fresh, spending) and its routines.
//! - **Library**: what it keeps: its memory notes, skills, folder and published pages.
//! - **Computer**: its own computer, live, or where it works without one; Codex access and how
//!   much it may do without asking.
//!
//! The gear opens its full settings (`settings::BotSettings`) as a page of the panel. A channel
//! keeps the plain settings panel.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::{Activity, Bot};
use crate::live::State;
use crate::mascots::BotMark;
use crate::settings::Ui;
use crate::sidebar::{Icon, MENU_ROW};

/// Their round header buttons (`roundButtonCls`).
const ROUND: &str = "flex size-[34px] shrink-0 items-center justify-center rounded-full bg-composer text-ink ring-1 ring-inset ring-ink/[0.07] hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)]";
/// A label over a group of rows (`CardLabel`).
const LABEL: &str = "mx-2 mb-1.5 mt-5 flex min-h-5 items-center justify-between gap-3 text-[13px] font-semibold text-ink-secondary";
/// A quiet line under a group.
const NOTE: &str = "px-2 text-[12px] leading-4 text-ink-tertiary";

/// The Chief's example asks (theirs, for their helper bot): a click sends one in the open chat.
const ASKS: [&str; 4] = ["What are my bots up to?", "Set up a bot for my inbox", "Make me a morning routine", "How do I connect my email?"];

/// A page of the panel, over its tabs: its settings, its activity, or one run inspected.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Settings,
    Activity,
    Inspector(Option<i64>),
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Details,
    Library,
    Computer,
}

#[component]
pub fn BotPanel(bot: Bot) -> Element {
    let mut ui = use_context::<Ui>();
    let mut tab = use_signal(|| Tab::Details);
    let mut page = use_signal(|| None::<Page>);
    let close = move |_| ui.settings_open.set(false);
    rsx! {
        aside {
            "aria-label": "{bot.name}",
            "data-bot-panel": true,
            class: "animate-panel-in absolute inset-0 z-40 flex h-full min-w-0 flex-col border-l border-hairline/40 bg-app md:relative md:z-20 md:w-[380px] md:shrink-0",
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    if page().is_some() { page.set(None) } else { ui.settings_open.set(false) }
                }
            },
            div { class: "flex h-[50px] shrink-0 items-start justify-end gap-2.5 px-[13px] pt-[13px]",
                button { r#type: "button", "data-panel-gear": true, "aria-label": "{bot.name}'s settings", title: "{bot.name}'s settings", "aria-current": if page() == Some(Page::Settings) { "page" } else { "false" }, class: ROUND, onclick: move |_| page.set(if page() == Some(Page::Settings) { None } else { Some(Page::Settings) }),
                    Icon { name: "settings", size: 16 }
                }
                button { r#type: "button", "aria-label": "Close", title: "Close", class: ROUND, onclick: close,
                    Icon { name: "x", size: 16 }
                }
            }
            div { class: "relative min-h-0 flex-1 overflow-y-auto px-4 pb-7",
                if let Some(shown) = page() {
                    div { "data-panel-page-view": match shown { Page::Settings => "settings", Page::Activity => "activity", Page::Inspector(_) => "inspector" },
                        button { r#type: "button", class: "-ml-1 flex items-center gap-1 rounded-md px-1 py-0.5 text-[13px] text-ink-secondary hover:text-ink",
                            onclick: move |_| page.set(match page() { Some(Page::Inspector(Some(_))) => Some(Page::Activity), _ => None }),
                            svg { "aria-hidden": "true", width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", path { d: "m15 18-6-6 6-6" } }
                            if matches!(shown, Page::Inspector(Some(_))) { "Activity" } else { "Details" }
                        }
                        match shown {
                            Page::Settings => rsx! {
                                h2 { class: "mt-2 text-[20px] font-semibold text-ink", "{bot.name}'s settings" }
                                crate::settings::BotSettings { bot: bot.clone() }
                            },
                            Page::Activity => rsx! {
                                h2 { class: "mt-2 text-[20px] font-semibold text-ink", "Activity" }
                                p { class: "mt-1 text-[13px] text-ink-secondary", "{bot.name}'s turns, the newest first." }
                                ActivityPage { bot: bot.clone(), on_open: move |id| page.set(Some(Page::Inspector(Some(id)))) }
                            },
                            Page::Inspector(run) => rsx! {
                                Inspector { key: "{run:?}", bot: bot.clone(), run }
                            },
                        }
                    }
                } else {
                    Hero { bot: bot.clone() }
                    div { role: "tablist", "aria-label": "{bot.name}", class: "sticky top-0 z-[1] -mx-4 mt-[15px] flex justify-center bg-app px-4 pb-2.5 pt-1",
                        div { class: "flex min-w-0 gap-0.5 rounded-full bg-ink/[0.05] p-[3px]",
                            for (id, label) in [(Tab::Details, "Details"), (Tab::Library, "Library"), (Tab::Computer, "Computer")] {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    role: "tab",
                                    "aria-selected": "{tab() == id}",
                                    "data-panel-tab": "{label.to_lowercase()}",
                                    class: if tab() == id { "h-[26px] min-w-0 shrink truncate rounded-full bg-ink/[0.1] px-3 text-[13px] font-medium leading-[18px] text-ink" } else { "h-[26px] min-w-0 shrink truncate rounded-full px-3 text-[13px] leading-[18px] text-ink-secondary transition-colors hover:text-ink" },
                                    onclick: move |_| tab.set(id),
                                    "{label}"
                                }
                            }
                        }
                    }
                    div { role: "tabpanel",
                        match tab() {
                            Tab::Details => rsx! { Details { bot: bot.clone(), on_page: move |p| page.set(Some(p)) } },
                            Tab::Library => rsx! { Library { bot: bot.clone(), on_settings: move |_| page.set(Some(Page::Settings)) } },
                            Tab::Computer => rsx! { Computer { bot: bot.clone(), on_settings: move |_| page.set(Some(Page::Settings)) } },
                        }
                    }
                }
            }
        }
    }
}

/// The bot large, its name, and what it's doing (`PanelHero`).
#[component]
fn Hero(bot: Bot) -> Element {
    let doing = match bot.activity {
        Activity::Working => "Working…",
        Activity::WaitingOnYou => "Needs you",
        Activity::Idle => "Ready",
    };
    rsx! {
        div { class: "mt-3.5 flex flex-col items-center text-center", "data-panel-hero": true,
            div { class: "relative",
                BotMark { color: bot.color.clone(), initial: bot.initial(), size: 88, mark: bot.mark.clone(), state: bot.activity.mood() }
                if bot.chief {
                    span { title: "Chief of Staff", class: "absolute -bottom-0.5 -right-0.5 flex size-[22px] items-center justify-center rounded-full bg-[#E8993A] text-white ring-[3px] ring-app",
                        Icon { name: "crown", size: 12, filled: true }
                    }
                }
            }
            h2 { class: "mt-2 max-w-full truncate text-[17px] font-medium leading-6 text-ink", "{bot.name}" }
            p { class: if bot.activity == Activity::WaitingOnYou { "text-[13px] text-warning" } else { "text-[13px] text-ink-secondary" }, "data-panel-status": true, "{doing}" }
        }
    }
}

/// A row that leads somewhere: an icon, a title, a value and a chevron.
#[component]
fn NavRow(icon: &'static str, title: String, #[props(default)] value: String, #[props(default)] data: String, onclick: EventHandler<()>) -> Element {
    rsx! {
        button { r#type: "button", class: "{MENU_ROW} min-h-[42px]", "data-panel-row": "{data}", onclick: move |_| onclick.call(()),
            span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: icon, size: 15 } }
            span { class: "min-w-0 flex-1 truncate", "{title}" }
            if !value.is_empty() {
                span { class: "min-w-0 max-w-[50%] truncate text-[13px] text-ink-secondary", "{value}" }
            }
            span { class: "shrink-0 text-ink-tertiary", Icon { name: "chevron-right", size: 15 } }
        }
    }
}

#[component]
fn Details(bot: Bot, on_page: EventHandler<Page>) -> Element {
    let mut state = use_context::<Signal<State>>();
    let mut ui = use_context::<Ui>();
    let mut all_chats = use_signal(|| false);
    let mut triggers = use_signal(|| None::<Value>);
    let mut showing_triggers = use_signal(|| false);
    use_hook(move || {
        spawn(async move {
            if let Ok(list) = api::get("/api/triggers").await {
                triggers.set(Some(list));
            }
        });
    });
    let mine: Vec<Value> = triggers().and_then(|t| t["triggers"].as_array().cloned()).unwrap_or_default().into_iter().filter(|t| t["botId"] == bot.id.as_str()).collect();
    let on = mine.iter().filter(|t| t["enabled"] == true).count();
    let chats: Vec<crate::bots::Chat> = bot.chats.iter().filter(|c| !c.archived).cloned().collect();
    let shown: Vec<crate::bots::Chat> = if all_chats() { chats.clone() } else { chats.iter().take(4).cloned().collect() };
    let (bot_id, start_id) = (bot.id.clone(), bot.id.clone());
    rsx! {
        div { "data-panel-tabpanel": "details",
            section { class: "pt-2", "data-panel-card": "model",
                crate::model_menu::ModelRow { bot: bot.clone() }
                crate::model_menu::Thinking { key: "{bot.engine}-{bot.model}", bot: bot.clone() }
            }
            section { "data-panel-card": "chats",
                div { class: LABEL,
                    span { "Chats" }
                    button { r#type: "button", class: "rounded-md px-1 text-[13px] font-normal text-ink-secondary hover:bg-ink/[0.04] hover:text-ink", onclick: move |_| crate::chats::start(state, ui, start_id.clone()), "New chat" }
                }
                for chat in shown {
                    {
                        let (id, thread) = (bot_id.clone(), chat.id.clone());
                        let open_now = chat.id == bot.thread_id;
                        rsx! {
                            button { key: "{chat.id}", r#type: "button", "data-panel-chat": "{chat.id}", "aria-current": if open_now { "true" } else { "false" },
                                class: if open_now { "{MENU_ROW} bg-ink/[0.05]" } else { "{MENU_ROW}" },
                                onclick: move |_| crate::chats::open(state, ui, &id, &thread),
                                span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "hash", size: 14 } }
                                span { class: "min-w-0 flex-1 truncate", if chat.title.is_empty() { "New chat" } else { "{chat.title}" } }
                                if chat.activity == Activity::Working {
                                    span { class: "size-1.5 shrink-0 rounded-full bg-accent", title: "Working" }
                                }
                            }
                        }
                    }
                }
                if chats.len() > 4 {
                    button { r#type: "button", class: "px-2 py-1 text-[13px] text-ink-secondary hover:text-ink", onclick: move |_| all_chats.toggle(),
                        if all_chats() { "Show fewer" } else { "Show all {chats.len()} chats" }
                    }
                }
            }
            if bot.chief {
                section { "data-panel-card": "try-asking",
                    div { class: LABEL, span { "Try asking" } }
                    for ask in ASKS {
                        {
                            let (id, thread) = (bot.id.clone(), bot.thread_id.clone());
                            rsx! {
                                button { key: "{ask}", r#type: "button", class: "{MENU_ROW}", "data-panel-ask": "{ask}",
                                    onclick: move |_| {
                                        let (id, thread) = (id.clone(), thread.clone());
                                        spawn(async move {
                                            let send = crate::live::send_id();
                                            state.write().sending(&thread, crate::live::optimistic(ask, &send, crate::chat::js_now(), Vec::new()));
                                            let body = json!({ "text": ask, "threadId": thread, "sendId": send, "attachments": [] });
                                            if api::post(&format!("/api/bots/{id}/messages"), &body).await.is_err() {
                                                state.write().unsend(&thread, &send);
                                            }
                                        });
                                    },
                                    span { class: "min-w-0 flex-1 truncate", "{ask}" }
                                }
                            }
                        }
                    }
                }
            }
            section { "data-panel-card": "conversation",
                div { class: LABEL, span { "This conversation" } }
                NavRow { icon: "search", title: "Find in conversation", data: "find", onclick: move |_| ui.find.set(Some(String::new())) }
                NavRow { icon: "plus", title: "Start fresh", data: "start-fresh", onclick: move |_| crate::chats::start(state, ui, bot.id.clone()) }
                NavRow { icon: "coins", title: "Spending", data: "usage", onclick: move |_| ui.prefs.set(Some("spending".into())) }
                NavRow { icon: "gauge", title: "Activity", data: "activity", onclick: move |_| on_page.call(Page::Activity) }
                NavRow { icon: "search", title: "Inspector", data: "inspector", onclick: move |_| on_page.call(Page::Inspector(None)) }
            }
            section { "data-panel-card": "routines",
                div { class: LABEL, span { "Routines" } }
                NavRow { icon: "zap", title: "Routines", value: if mine.is_empty() { "None".to_string() } else { format!("{on} on") }, data: "routines", onclick: move |_| showing_triggers.set(true) }
            }
        }
        if showing_triggers() {
            crate::triggers::TriggersDialog { on_close: move |_| showing_triggers.set(false) }
        }
    }
}

/// Whether a memory note (its path in the vault) is this bot's: its own note or a day of its journal.
pub fn bot_note(path: &str, name: &str) -> bool {
    path == format!("Bots/{name}.md") || (path.starts_with("Journal/") && path.ends_with(&format!(" {name}.md")))
}

/// A folder's path, with the home folder as `~`.
fn short_path(path: &str) -> String {
    for home in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(home)
            && let Some((_, inside)) = rest.split_once('/')
        {
            return format!("~/{inside}");
        }
    }
    path.to_string()
}

#[component]
fn Library(bot: Bot, on_settings: EventHandler<()>) -> Element {
    let mut memory = use_signal(|| None::<Value>);
    let mut pages = use_signal(|| None::<Value>);
    let mut showing_memory = use_signal(|| false);
    let mut copied = use_signal(|| false);
    use_hook(move || {
        spawn(async move {
            if let Ok(found) = api::get("/api/memory").await {
                memory.set(Some(found));
            }
        });
        spawn(async move {
            if let Ok(found) = api::get("/api/pages").await {
                pages.set(Some(found));
            }
        });
    });
    let notes = memory().map(|m| {
        if m["on"] == false {
            "Off".to_string()
        } else {
            let n = m["notes"].as_array().into_iter().flatten().filter(|n| n["path"].as_str().is_some_and(|p| bot_note(p, &bot.name))).count();
            match n {
                0 => "Nothing yet".into(),
                1 => "1 note".into(),
                n => format!("{n} notes"),
            }
        }
    });
    let mine: Vec<Value> = pages().and_then(|p| p["pages"].as_array().cloned()).unwrap_or_default().into_iter().filter(|p| p["botId"] == bot.id.as_str()).collect();
    let skills = match bot.skills.len() {
        0 => "None".to_string(),
        n => n.to_string(),
    };
    let folder = bot.folder.clone();
    rsx! {
        div { "data-panel-tabpanel": "library", class: "pt-2",
            NavRow { icon: "book", title: "Memory", value: notes.unwrap_or_default(), data: "memory", onclick: move |_| showing_memory.set(true) }
            NavRow { icon: "puzzle", title: "Skills", value: skills, data: "skills", onclick: move |_| on_settings.call(()) }
            button { r#type: "button", class: "{MENU_ROW} min-h-[42px]", "data-panel-row": "folder", title: "Copy the folder's path",
                onclick: move |_| {
                    let path = serde_json::to_string(&folder).unwrap_or_default();
                    let _ = document::eval(&format!("navigator.clipboard && navigator.clipboard.writeText({path});"));
                    copied.set(true);
                },
                span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "home", size: 15 } }
                span { class: "min-w-0 flex-1 truncate", "Folder" }
                span { class: "min-w-0 max-w-[60%] truncate text-[13px] text-ink-secondary", dir: "rtl", if copied() { "Copied" } else { bdi { dir: "ltr", "{short_path(&bot.folder)}" } } }
            }
            if !mine.is_empty() {
                div { class: LABEL, span { "Published pages" } }
                for page in mine {
                    a { key: "{page[\"id\"]}", href: "{page[\"url\"].as_str().unwrap_or_default()}", target: "_blank", rel: "noopener", class: "{MENU_ROW}", "data-panel-row": "page",
                        span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "rocket", size: 14 } }
                        span { class: "min-w-0 flex-1",
                            span { class: "block truncate", "{page[\"title\"].as_str().unwrap_or(\"Page\")}" }
                            span { class: "block truncate text-[12px] text-ink-tertiary", "{page[\"url\"].as_str().unwrap_or_default()}" }
                        }
                    }
                }
            }
            p { class: "mt-4 text-center {NOTE}", "What {bot.name} keeps and has made." }
        }
        if showing_memory() {
            crate::memory::MemoryDialog { on_close: move |_| showing_memory.set(false) }
        }
    }
}

/// How long ago, in words: "just now", "5 min ago", "3 h ago", "2 days ago".
fn ago(at: &str) -> String {
    let then = crate::chat::parse_time(at);
    if !then.is_finite() || then <= 0.0 {
        return String::new();
    }
    let minutes = ((crate::chat::js_now() - then) / 60_000.0).max(0.0) as i64;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => match minutes / 1440 {
            1 => "yesterday".into(),
            d => format!("{d} days ago"),
        },
    }
}

/// A turn's length: "0.8 s", "12 s", "3 min 4 s".
fn took(seconds: f64) -> String {
    if seconds < 10.0 {
        format!("{seconds:.1} s")
    } else if seconds < 60.0 {
        format!("{} s", seconds.round() as i64)
    } else {
        let s = seconds.round() as i64;
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// The colour of a run's dot: done, failed or stopped.
fn outcome_dot(outcome: &str) -> &'static str {
    match outcome {
        "done" => "bg-[#2fa46a]",
        "stopped" => "bg-ink-tertiary",
        _ => "bg-danger",
    }
}

/// Their Activity: the bot's turns, the newest first, each opening the Inspector.
#[component]
fn ActivityPage(bot: Bot, on_open: EventHandler<i64>) -> Element {
    let id = bot.id.clone();
    let runs = use_resource(move || {
        let id = id.clone();
        async move { api::get(&format!("/api/bots/{id}/runs")).await }
    });
    let list: Option<Result<Vec<Value>, String>> = runs.read().as_ref().map(|r| r.as_ref().map(|v| v["runs"].as_array().cloned().unwrap_or_default()).map_err(Clone::clone));
    rsx! {
        div { class: "mt-4 flex flex-col", "data-panel-activity": true,
            match list {
                None => rsx! { p { class: NOTE, "Looking…" } },
                Some(Err(err)) => rsx! { p { role: "alert", class: "text-[13px] text-danger", "{err}" } },
                Some(Ok(runs)) if runs.is_empty() => rsx! { p { class: "py-6 text-center text-[13px] text-ink-secondary", "Nothing yet. Each turn {bot.name} takes shows here." } },
                Some(Ok(runs)) => rsx! {
                    for run in runs {
                        {
                            let id = run["id"].as_i64().unwrap_or_default();
                            let outcome = run["outcome"].as_str().unwrap_or_default().to_string();
                            let chat = run["chat"].as_str().unwrap_or("New chat").to_string();
                            let tools = run["tools"].as_i64().unwrap_or(0);
                            let mut facts = vec![ago(run["startedAt"].as_str().unwrap_or_default()), took(run["seconds"].as_f64().unwrap_or(0.0))];
                            if tools > 0 {
                                facts.push(if tools == 1 { "1 tool".into() } else { format!("{tools} tools") });
                            }
                            if let Some(cost) = run["costUsd"].as_f64().filter(|c| *c > 0.0) {
                                facts.push(format!("${cost:.2}"));
                            }
                            let facts = facts.join(" · ");
                            rsx! {
                                button { key: "{id}", r#type: "button", class: "{MENU_ROW} min-h-[48px]", "data-panel-run": "{id}", onclick: move |_| on_open.call(id),
                                    span { class: "size-2 shrink-0 rounded-full {outcome_dot(&outcome)}", title: "{outcome}" }
                                    span { class: "min-w-0 flex-1",
                                        span { class: "block truncate", "{chat}" }
                                        span { class: "block truncate text-[12px] text-ink-tertiary", "{facts}" }
                                    }
                                    span { class: "shrink-0 text-ink-tertiary", Icon { name: "chevron-right", size: 15 } }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Their Inspector: one turn (the newest unless one was picked) as a run log, its events, or raw.
#[component]
fn Inspector(bot: Bot, run: Option<i64>) -> Element {
    let mut view = use_signal(|| "log");
    let id = bot.id.clone();
    let found = use_resource(move || {
        let id = id.clone();
        async move {
            let which = match run {
                Some(run) => run,
                None => {
                    let list = api::get(&format!("/api/bots/{id}/runs")).await?;
                    match list["runs"][0]["id"].as_i64() {
                        Some(newest) => newest,
                        None => return Ok(Value::Null),
                    }
                }
            };
            api::get(&format!("/api/bots/{id}/runs/{which}")).await.map(|r| r["run"].clone())
        }
    });
    let shown = found.read().clone();
    rsx! {
        h2 { class: "mt-2 text-[20px] font-semibold text-ink", "Inspector" }
        p { class: "mt-1 text-[13px] text-ink-secondary", "What happened in a turn, step by step." }
        div { class: "mt-4", "data-panel-inspector": true,
            match shown {
                None => rsx! { p { class: NOTE, "Looking…" } },
                Some(Err(err)) => rsx! { p { role: "alert", class: "text-[13px] text-danger", "{err}" } },
                Some(Ok(Value::Null)) => rsx! { p { class: "py-6 text-center text-[13px] text-ink-secondary", "Nothing to inspect yet: {bot.name} hasn't taken a turn." } },
                Some(Ok(run)) => {
                    let outcome = run["outcome"].as_str().unwrap_or_default().to_string();
                    let chat = run["chat"].as_str().unwrap_or("New chat").to_string();
                    let model = [run["engine"].as_str().unwrap_or_default(), run["model"].as_str().unwrap_or_default()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
                    let raw = serde_json::to_string_pretty(&run["events"]).unwrap_or_default();
                    let when = ago(run["startedAt"].as_str().unwrap_or_default());
                    let lines: Vec<(String, &'static str, String)> = run["log"].as_array().cloned().unwrap_or_default().into_iter().map(|l| {
                        let tone = match l["kind"].as_str() {
                            Some("error") => "text-danger",
                            Some("ask") => "text-warning",
                            Some("thinking") | Some("note") => "text-ink-secondary",
                            _ => "text-ink",
                        };
                        (format!("+{:.1}s", l["at"].as_f64().unwrap_or(0.0)), tone, l["text"].as_str().unwrap_or_default().to_string())
                    }).collect();
                    let events: Vec<(String, String, String)> = run["events"].as_array().cloned().unwrap_or_default().into_iter().map(|e| {
                        (format!("{} ms", e["ms"].as_u64().unwrap_or(0)), e["type"].as_str().unwrap_or("?").to_string(), event_gist(&e))
                    }).collect();
                    let length = took(run["seconds"].as_f64().unwrap_or(0.0));
                    rsx! {
                        div { class: "flex items-center gap-2 px-2",
                            span { class: "size-2 shrink-0 rounded-full {outcome_dot(&outcome)}" }
                            span { class: "min-w-0 flex-1 truncate text-[14px] font-medium text-ink", "{chat}" }
                            span { class: "shrink-0 text-[12px] text-ink-tertiary", "{when}" }
                        }
                        p { class: "mt-0.5 px-2 text-[12px] text-ink-tertiary", "{model} · {length}" }
                        div { role: "tablist", class: "mt-3 flex justify-center",
                            div { class: "flex gap-0.5 rounded-full bg-ink/[0.05] p-[3px]",
                                for (key, label) in [("log", "Run log"), ("events", "Events"), ("raw", "Raw")] {
                                    button { key: "{key}", r#type: "button", role: "tab", "aria-selected": "{view() == key}", "data-inspector-view": "{key}",
                                        class: if view() == key { "h-[26px] rounded-full bg-ink/[0.1] px-3 text-[13px] font-medium text-ink" } else { "h-[26px] rounded-full px-3 text-[13px] text-ink-secondary hover:text-ink" },
                                        onclick: move |_| view.set(key),
                                        "{label}"
                                    }
                                }
                            }
                        }
                        match view() {
                            "log" => rsx! {
                                ol { class: "mt-3 flex flex-col gap-1.5 px-2", "data-inspector-log": true,
                                    for (n, (at, tone, text)) in lines.into_iter().enumerate() {
                                        li { key: "{n}", class: "flex gap-2 text-[13px] leading-5",
                                            span { class: "w-12 shrink-0 text-right tabular-nums text-ink-tertiary", "{at}" }
                                            span { class: "min-w-0 flex-1 [overflow-wrap:anywhere] {tone}", "{text}" }
                                        }
                                    }
                                }
                            },
                            "events" => rsx! {
                                ol { class: "mt-3 flex flex-col divide-y divide-hairline/40 px-2", "data-inspector-events": true,
                                    for (n, (ms, kind, gist)) in events.into_iter().enumerate() {
                                        li { key: "{n}", class: "flex gap-2 py-1.5 font-mono text-[12px] leading-4",
                                            span { class: "w-14 shrink-0 text-right tabular-nums text-ink-tertiary", "{ms}" }
                                            span { class: "shrink-0 text-ink", "{kind}" }
                                            span { class: "min-w-0 flex-1 truncate text-ink-secondary", "{gist}" }
                                        }
                                    }
                                }
                            },
                            _ => rsx! {
                                pre { class: "mt-3 max-h-[60vh] overflow-auto rounded-lg bg-ink/[0.04] p-3 font-mono text-[11px] leading-4 whitespace-pre-wrap [overflow-wrap:anywhere] text-ink", "data-inspector-raw": true, "{raw}" }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// An event's gist for the Events list: its title, summary, text or message.
fn event_gist(event: &Value) -> String {
    ["title", "summary", "text", "message", "model", "output"]
        .iter()
        .filter_map(|k| event[*k].as_str())
        .filter(|s| !s.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" · ")
}

const APPROVAL: [(&str, &str); 4] = [("ask", "Asks first"), ("edits", "Edits on its own"), ("auto", "Approves for you"), ("full", "Full access")];

#[component]
fn Computer(bot: Bot, on_settings: EventHandler<()>) -> Element {
    let state = use_context::<Signal<State>>();
    let mut ui = use_context::<Ui>();
    let mut computers = use_signal(|| None::<Value>);
    let mut watching = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    use_hook(move || {
        spawn(async move {
            if let Ok(found) = api::get("/api/computers").await {
                computers.set(Some(found));
            }
        });
    });
    let entry = computers().and_then(|c| c["bots"].as_array().and_then(|all| all.iter().find(|e| e["botId"] == bot.id.as_str()).cloned()));
    let at_boat = entry.as_ref().is_some_and(|e| e["kind"] == "boat");
    let running = entry.as_ref().is_some_and(|e| e["state"] == "running");
    let approval = APPROVAL.iter().find(|(id, _)| *id == bot.approval_mode).map(|(_, l)| l.to_string()).unwrap_or_default();
    let (id, codex) = (bot.id.clone(), bot.codex);
    rsx! {
        div { "data-panel-tabpanel": "computer", class: "pt-2",
            div { class: "mx-2 flex aspect-video flex-col items-center justify-center gap-2 rounded-xl bg-ink/[0.05] p-4 text-center", "data-panel-screen": true,
                if computers().is_none() {
                    span { class: "text-[13px] text-ink-secondary", "Looking…" }
                } else if let Some(entry) = entry.as_ref() {
                    Icon { name: "monitor", size: 26 }
                    span { class: "text-[13px] font-medium text-ink",
                        if at_boat { "Its own computer, at Boat" } else { "Its own computer, here" }
                    }
                    span { class: "text-[12px] text-ink-secondary",
                        if running { "On" } else { "{entry[\"state\"].as_str().unwrap_or(\"Off\")}" }
                    }
                    button { r#type: "button", "data-panel-watch": true, class: "mt-1 rounded-full bg-composer px-3.5 py-1.5 text-[13px] font-medium text-ink ring-1 ring-inset ring-ink/[0.07] hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)]", onclick: move |_| watching.set(true),
                        "Watch its screen"
                    }
                } else {
                    Icon { name: "laptop", size: 26 }
                    span { class: "text-[13px] font-medium text-ink", "No computer of its own" }
                    span { class: "max-w-[85%] text-[12px] text-ink-secondary", "{bot.name} works in its folder on this computer." }
                    button { r#type: "button", class: "mt-1 rounded-full bg-composer px-3.5 py-1.5 text-[13px] font-medium text-ink ring-1 ring-inset ring-ink/[0.07] hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)]", onclick: move |_| ui.prefs.set(Some("computers".into())),
                        "Give it a computer"
                    }
                }
            }
            div { class: LABEL, span { "Where it works" } }
            NavRow { icon: "home", title: "Folder", value: short_path(&bot.folder), data: "where", onclick: move |_| ui.prefs.set(Some("computers".into())) }
            div { class: "{MENU_ROW} min-h-[42px] hover:bg-transparent", "data-panel-row": "codex",
                span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "book", size: 15 } }
                span { class: "min-w-0 flex-1 truncate", "Codex vault" }
                crate::settings::Switch { checked: codex, disabled: false, label: "Codex vault access".to_string(),
                    on_toggle: move |_| {
                        let id = id.clone();
                        spawn(async move {
                            if let Err(err) = crate::settings::save(state, &id, json!({ "codex": !codex })).await {
                                problem.set(Some(err));
                            }
                        });
                    }
                }
            }
            NavRow { icon: "check", title: "Without asking", value: approval, data: "approval", onclick: move |_| on_settings.call(()) }
            if let Some(err) = problem() {
                p { role: "alert", class: "px-2 text-[12px] text-danger", "{err}" }
            }
        }
        if watching() {
            crate::computers::ComputerView { bot_id: bot.id.clone(), name: bot.name.clone(), at_boat, on_close: move |_| watching.set(false) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bots_notes_and_folder() {
        assert!(bot_note("Bots/Scout.md", "Scout"));
        assert!(bot_note("Journal/2026-11-22 Scout.md", "Scout"));
        assert!(!bot_note("Journal/2026-11-22 Scouter.md", "Scout"));
        assert!(!bot_note("Projects/Scout.md", "Scout"));
        assert_eq!(short_path("/home/moo/.local/share/catbot/bots/scout"), "~/.local/share/catbot/bots/scout");
        assert_eq!(short_path("/srv/bots/scout"), "/srv/bots/scout");
    }

    #[test]
    fn runs_in_words() {
        assert_eq!(took(0.84), "0.8 s");
        assert_eq!(took(12.4), "12 s");
        assert_eq!(took(184.0), "3 min 4 s");
        assert_eq!(event_gist(&json!({ "type": "item.started", "title": "Bash", "summary": "ls" })), "Bash · ls");
        assert_eq!(outcome_dot("failed"), "bg-danger");
    }
}
