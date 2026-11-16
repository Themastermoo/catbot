//! The sidebar: a search field and Catbot's bots, one row each. A bot with more than one chat
//! gets an arrow that opens its chats beneath it (`chats.rs`).
//!
//! Classes and sizes follow the full layout of OpenMausBot's `src/components/Sidebar.tsx`
//! (`Sidebar` and `BotListItem`, Apache-2.0, see NOTICE). Their bot picture is their mascot
//! artwork, which Catbot doesn't copy: a bot wears one of Catbot's own (`mascots.rs`).

use std::collections::HashSet;

use dioxus::prelude::*;

use crate::bots::{Activity, Bot};
use crate::chat::clock;
use crate::chats::{self, ChatList, Chevron};
use crate::live::State;
use crate::mascots::BotMark;
use crate::settings::Ui;

/// Their focus ring for rows (`SIMPLE_ROW_FOCUS`).
const ROW_FOCUS: &str = "focus-visible:outline-2 focus-visible:-outline-offset-2! focus-visible:outline-focus";
/// Their raised-control look for the search field (`ELEVATED_CONTROL`).
const ELEVATED_CONTROL: &str = "bg-composer ring-1 ring-inset ring-ink/[0.07]";

#[component]
pub fn Sidebar(bots: ReadSignal<Vec<Bot>>, selected: Signal<Option<String>>) -> Element {
    let mut query = use_signal(String::new);
    let mut making = use_signal(|| false);
    let mut pairing = use_signal(|| false);
    let mut remembering = use_signal(|| false);
    let mut configuring = use_signal(|| false);
    let mut measuring = use_signal(|| false);
    let mut grouping = use_signal(|| false);
    // Bots whose chats are showing.
    let mut unfolded = use_signal(HashSet::<String>::new);
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let shown: Vec<Bot> = bots.read().iter().filter(|b| b.matches(&query.read())).cloned().collect();
    let empty = bots.read().is_empty();
    // On a phone the list gives way to the open chat.
    let phone = if (ui.showing_chat)() { "max-md:hidden" } else { "" };
    // Pairing other devices is done at home: not on the phone, nor in the Windows app.
    let at_home = !crate::link::pairs() && !crate::link::has_own();
    // Taken every time (hooks keep their order), used only in the Windows app.
    let all_places = use_context::<crate::settings::Places>();
    let places = crate::link::has_own().then_some(all_places);
    let mut pairing_home = use_signal(|| false);
    rsx! {
        aside {
            "aria-label": "Bots",
            "data-sidebar": true,
            class: "relative flex h-full w-[320px] shrink-0 flex-col border-r-[0.5px] border-edge bg-panel max-md:w-full max-md:border-r-0 {phone}",
            div { "data-sidebar-top-row": true, class: "relative flex h-12 items-center pl-4 pr-2",
                span { class: "min-w-0 flex-1 text-[15px] font-semibold text-ink", "Catbot" }
                button {
                    r#type: "button",
                    "aria-label": "Memory",
                    title: "The bots' memory",
                    "data-open-memory": true,
                    class: "flex size-8 shrink-0 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink",
                    onclick: move |_| remembering.set(true),
                    // Lucide `BookOpen` (ISC).
                    svg { "aria-hidden": "true", width: "17", height: "17", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M12 7v14" }
                        path { d: "M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z" }
                    }
                }
                button {
                    r#type: "button",
                    "aria-label": "Usage",
                    title: "Usage and limits",
                    "data-open-usage": true,
                    class: "flex size-8 shrink-0 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink",
                    onclick: move |_| measuring.set(true),
                    // Lucide `Gauge` (ISC).
                    svg { "aria-hidden": "true", width: "17", height: "17", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "m12 14 4-4" }
                        path { d: "M3.34 19a10 10 0 1 1 17.32 0" }
                    }
                }
                if crate::notify::AVAILABLE { button {
                    r#type: "button",
                    "aria-label": "Settings",
                    title: "Settings",
                    "data-open-settings": true,
                    class: "flex size-8 shrink-0 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink",
                    onclick: move |_| configuring.set(true),
                    // Lucide `Bell` (ISC).
                    svg { "aria-hidden": "true", width: "17", height: "17", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M10.268 21a2 2 0 0 0 3.464 0" }
                        path { d: "M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326" }
                    }
                } }
                if at_home { button {
                    r#type: "button",
                    "aria-label": "Devices",
                    title: "Devices away from home",
                    "data-open-devices": true,
                    class: "flex size-8 shrink-0 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink",
                    onclick: move |_| pairing.set(true),
                    // Lucide `Smartphone` (ISC).
                    svg { "aria-hidden": "true", width: "17", height: "17", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        rect { x: "5", y: "2", width: "14", height: "20", rx: "2" }
                        path { d: "M12 18h.01" }
                    }
                } }
                button {
                    r#type: "button",
                    "aria-label": "New channel",
                    title: "New channel: a group chat of several bots",
                    "data-open-new-channel": true,
                    class: "flex size-8 shrink-0 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink",
                    onclick: move |_| grouping.set(true),
                    // Lucide `Hash` (ISC).
                    svg { "aria-hidden": "true", width: "17", height: "17", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M4 9h16M4 15h16M10 3 8 21M16 3l-2 18" }
                    }
                }
                button {
                    r#type: "button",
                    "aria-label": "New Bot",
                    title: "New Bot",
                    class: "flex size-8 shrink-0 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink",
                    onclick: move |_| making.set(true),
                    svg { "aria-hidden": "true", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M5 12h14M12 5v14" }
                    }
                }
            }
            if making() {
                crate::settings::NewBotDialog { on_close: move |_| making.set(false) }
            }
            if pairing() {
                crate::devices::DevicesDialog { on_close: move |_| pairing.set(false) }
            }
            if grouping() {
                crate::channels::NewChannelDialog { on_close: move |_| grouping.set(false) }
            }
            if measuring() {
                crate::usage::UsageDialog { on_close: move |_| measuring.set(false) }
            }
            if configuring() {
                crate::notify::SettingsDialog { on_close: move |_| configuring.set(false) }
            }
            if remembering() {
                crate::memory::MemoryDialog { on_close: move |_| remembering.set(false) }
            }
            if let Some(places) = places {
                PlaceSwitch { places, on_pair: move |_| pairing_home.set(true) }
                if pairing_home() {
                    crate::pair::PairScreen {
                        removed: false,
                        computer: true,
                        on_cancel: move |_| pairing_home.set(false),
                        on_paired: move |_| {
                            pairing_home.set(false);
                            go_to(places, crate::link::Place::Home);
                        },
                    }
                }
            }
            div { class: "px-3 pt-1 pb-3",
                div { class: "flex h-8 items-center gap-2 rounded-full px-3 focus-within:ring-focus {ELEVATED_CONTROL}",
                    SearchIcon {}
                    input {
                        id: "sidebar-search",
                        placeholder: "Search",
                        "aria-label": "Search bots and messages",
                        class: "w-full bg-transparent text-[13px] text-ink placeholder:text-ink-tertiary focus:outline-none",
                        oninput: move |e| query.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Escape {
                                query.set(String::new());
                                document::eval(r#"const box = document.getElementById("sidebar-search"); if (box) box.value = "";"#);
                            }
                        },
                    }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto px-2",
                div { class: "flex flex-col gap-0.5",
                    if empty {
                        div { class: "px-3 py-6 text-center text-[13px] text-ink-secondary",
                            "No bots yet. Make one with the + above."
                        }
                    } else if shown.is_empty() {
                        div { class: "px-3 py-6 text-center text-[13px] text-ink-secondary", "Nothing matches “{query}”" }
                    }
                    for bot in shown {
                        BotRow {
                            key: "{bot.id}",
                            selected: selected.read().as_deref() == Some(bot.id.as_str()),
                            // Open by the arrow, or while a search finds one of its chats.
                            unfolded: unfolded.read().contains(&bot.id) || chat_found(&bot, &query.read()),
                            query: query.read().clone(),
                            bot,
                            on_select: move |id| {
                                selected.set(Some(id));
                                let mut showing = ui.showing_chat;
                                showing.set(true);
                            },
                            on_fold: move |id: String| {
                                let mut open = unfolded.write();
                                if !open.remove(&id) {
                                    open.insert(id);
                                }
                            },
                            on_new_chat: move |id: String| {
                                unfolded.write().insert(id.clone());
                                chats::start(state, ui, id);
                            },
                        }
                    }
                }
            }
        }
    }
}

/// Whether a search names one of the bot's chats (not just the bot).
fn chat_found(bot: &Bot, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    !query.is_empty() && bot.chats.iter().any(|c| c.title.to_lowercase().contains(&query))
}

#[component]
fn BotRow(
    bot: Bot,
    selected: bool,
    unfolded: bool,
    query: String,
    on_select: EventHandler<String>,
    on_fold: EventHandler<String>,
    on_new_chat: EventHandler<String>,
) -> Element {
    let id = bot.id.clone();
    let key_id = bot.id.clone();
    let fold_id = bot.id.clone();
    let new_id = bot.id.clone();
    let waiting = bot.activity == Activity::WaitingOnYou;
    let working = bot.activity == Activity::Working;
    let mood = bot.activity.mood().to_string();
    // The arrow shows once there's more than one chat to choose between.
    let has_list = bot.listed_chats().count() > 1 || bot.chats.iter().any(|c| c.archived);
    let expanded = has_list && unfolded;
    let row_state = if selected && !expanded { "bg-raised" } else { "hover:bg-raised/50" };
    let row_left = if has_list { "pl-6" } else { "pl-2.5" };
    let preview_color = if waiting { "font-semibold text-warning" } else { "text-ink-secondary" };
    let time = bot.last.as_ref().filter(|_| selected && !expanded).map(|last| clock(last.at));
    // A draft left in the bot's open chat shows instead, unless the bot is busy or asking.
    let draft = use_context::<Signal<crate::drafts::Drafts>>()
        .read()
        .get(&bot.thread_id)
        .filter(|_| !selected && bot.activity == Activity::Idle)
        .map(|d| d.line());
    let preview = bot.preview();
    let fold_label = if expanded { format!("Hide {} chats", bot.name) } else { format!("Show {} chats", bot.name) };
    rsx! {
        div { class: "group relative",
            div {
                role: "button",
                tabindex: 0,
                "aria-current": if selected && !expanded { "page" },
                "data-sidebar-bot-row": "{bot.id}",
                // The cat's eyes follow the pointer anywhere on its row (see `MASCOT_LIFE`).
                "data-cat-follow": true,
                class: "flex w-full items-center gap-3 rounded-xl py-2 {row_left} pr-9 text-left transition-colors duration-150 max-md:rounded-2xl max-md:py-2.5 {ROW_FOCUS} {row_state}",
                onclick: move |_| on_select.call(id.clone()),
                onkeydown: move |e| {
                    if e.key() == Key::Enter || e.key() == Key::Character(" ".into()) {
                        e.prevent_default();
                        on_select.call(key_id.clone());
                    }
                },
                span { class: "relative flex shrink-0", title: "{bot.name}",
                    BotMark { color: bot.color.clone(), initial: bot.initial(), size: 44, mark: bot.mark.clone(), state: mood }
                    if working {
                        span { "data-testid": "working-dot", class: "absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-panel bg-success" }
                    }
                    if waiting {
                        span {
                            "data-testid": "waiting-dot",
                            role: "status",
                            "aria-label": "Waiting for you…",
                            title: "Waiting for you…",
                            class: "absolute -right-0.5 -bottom-0.5 flex size-4 items-center justify-center overflow-hidden rounded-full border-2 border-panel bg-warning text-[12px] font-black text-panel",
                            span { "aria-hidden": "true", "!" }
                        }
                    }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex items-baseline justify-between gap-2",
                        span { class: "flex min-w-0 grow items-center gap-1.5 truncate text-[15px] font-semibold leading-5 text-ink",
                            span { class: "truncate", "{bot.name}" }
                        }
                        if let Some(time) = time {
                            span { class: "shrink-0 text-[12px] tabular-nums text-ink-secondary", "{time}" }
                        }
                    }
                    if !expanded {
                        div { class: "flex items-center justify-between gap-2",
                            span { class: "flex min-w-0 items-center gap-1.5 truncate text-[13px] leading-[18px] {preview_color}",
                                if let Some(draft) = draft {
                                    span { dir: "auto", class: "truncate", "data-sidebar-draft": true,
                                        span { class: "font-semibold text-danger", "Draft: " }
                                        span { class: "text-ink-secondary", "{draft}" }
                                    }
                                } else {
                                    span { dir: "auto", class: "truncate", "{preview}" }
                                }
                            }
                            if bot.unread {
                                span { class: "size-2 shrink-0 rounded-full bg-accent", "aria-label": "Unread" }
                            }
                        }
                    }
                }
            }
            if has_list {
                button {
                    r#type: "button",
                    "aria-label": "{fold_label}",
                    "aria-expanded": "{expanded}",
                    "data-chat-fold": "{bot.id}",
                    class: "absolute left-0.5 top-[30px] flex size-5 -translate-y-1/2 items-center justify-center rounded text-ink-secondary outline-none hover:text-ink focus-visible:ring-1 focus-visible:ring-accent/60",
                    onclick: move |_| on_fold.call(fold_id.clone()),
                    Chevron { open: expanded, size: 13 }
                }
            }
            button {
                r#type: "button",
                "aria-label": "New chat with {bot.name}",
                title: "New chat",
                "data-row-new-chat": "{bot.id}",
                class: "pointer-events-none absolute right-1 top-[30px] flex size-7 -translate-y-1/2 items-center justify-center rounded text-ink-secondary opacity-0 hover:bg-raised hover:text-ink group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 max-md:pointer-events-auto max-md:opacity-70",
                onclick: move |_| on_new_chat.call(new_id.clone()),
                svg { "aria-hidden": "true", width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "M5 12h14M12 5v14" }
                }
            }
            if expanded {
                ChatList { bot: bot.clone(), current: selected, query }
            }
        }
    }
}

/// Their search glyph (lucide `Search`, ISC), at 15px.
#[component]
fn SearchIcon() -> Element {
    rsx! {
        svg {
            "aria-hidden": "true",
            class: "shrink-0 text-ink-secondary",
            width: "15",
            height: "15",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "m21 21-4.34-4.34" }
            circle { cx: "11", cy: "11", r: "8" }
        }
    }
}

/// Moves the app to a place and keeps the choice.
fn go_to(mut places: crate::settings::Places, place: crate::link::Place) {
    crate::link::set_place(place);
    places.notice.set(None);
    places.place.set(place);
}

/// The Windows app's switch between its own bots and home's.
#[component]
fn PlaceSwitch(places: crate::settings::Places, on_pair: EventHandler<()>) -> Element {
    use crate::link::Place;
    let here = (places.place)();
    let tab = |on: bool| {
        if on { "flex-1 rounded-full bg-panel px-3 py-1 font-medium text-ink shadow-sm" } else { "flex-1 rounded-full px-3 py-1 text-ink-secondary hover:text-ink" }
    };
    rsx! {
        div { class: "px-3 pb-2",
            div { role: "tablist", "aria-label": "Which Catbot", "data-place-switch": true, class: "flex rounded-full bg-raised p-0.5 text-[13px]",
                button {
                    r#type: "button",
                    role: "tab",
                    "aria-selected": here == Place::ThisPc,
                    "data-place": "this-pc",
                    class: tab(here == Place::ThisPc),
                    onclick: move |_| go_to(places, Place::ThisPc),
                    "This PC"
                }
                button {
                    r#type: "button",
                    role: "tab",
                    "aria-selected": here == Place::Home,
                    "data-place": "home",
                    class: tab(here == Place::Home),
                    onclick: move |_| {
                        if crate::link::current().is_some() {
                            go_to(places, Place::Home);
                        } else {
                            on_pair.call(());
                        }
                    },
                    "Home"
                }
            }
            if let Some(notice) = (places.notice)() {
                p { role: "status", class: "mt-2 px-1 text-[12px] leading-[17px] text-warning", "{notice}" }
            }
        }
    }
}
