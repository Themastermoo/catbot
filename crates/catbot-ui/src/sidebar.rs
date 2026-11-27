//! The sidebar, in Grok Bot's look as MausBot's default ("simple") layout has it (stage 19.1,
//! Moo's choice 2026-11-21): round Search and New buttons in the top corner, the Chief of Staff
//! as a tile of its own, the bots as roomy rows, and at the foot the account button (with its
//! menu) and a "Connect apps" pill. A bot with more than one chat can open them beneath its row
//! (`chats.rs`).
//!
//! Classes and sizes follow OpenMausBot 0.1.176's `Sidebar.tsx`, `SidebarSimpleTop.tsx`,
//! `SidebarTiles.tsx`, `SidebarRowMarkers.tsx`, `SidebarFooter.tsx`, `SidebarConnectApps.tsx` and
//! `SidebarProfileMenu.tsx` (Apache-2.0, see NOTICE). Their pictures and app logos aren't copied:
//! a bot wears one of Catbot's cats (`mascots.rs`), and an app shows its initial.

use std::collections::HashSet;

use dioxus::prelude::*;

use crate::bots::{Activity, Bot};
use crate::chats::{self, ChatList, Chevron};
use crate::live::State;
use crate::mascots::BotMark;
use crate::settings::Ui;

/// Their focus ring for rows and tiles (`SIMPLE_ROW_FOCUS`).
const ROW_FOCUS: &str = "focus-visible:outline-2 focus-visible:-outline-offset-2! focus-visible:outline-focus";
/// Their raised control (`ELEVATED_CONTROL`), with its hover and open tint (`ELEVATED_HOVER`).
const ELEVATED: &str = "bg-composer ring-1 ring-inset ring-ink/[0.07] transition-colors duration-[120ms] hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)] aria-expanded:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)]";
/// The round buttons in the sidebar's top corner (`SIMPLE_TOP_BUTTON`).
const TOP_BUTTON: &str = "flex size-9 shrink-0 items-center justify-center rounded-full text-ink/90 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus";
/// Their menu sheet and its rows (`menu-recipe.ts`, `SidebarPopoverMenu.tsx`).
pub const MENU_SHEET: &str = "popover-surface flex flex-col gap-1 rounded-[var(--radius-menu)] border border-hairline/50 bg-menu p-1.5 shadow-lg";
pub const MENU_ROW: &str = "flex min-h-8 w-full items-center gap-2 rounded-lg px-2 py-1.5 text-start text-[14px] leading-5 text-ink hover:bg-menu-hover focus-visible:bg-menu-hover focus-visible:outline-none disabled:opacity-60";
const MENU_SEPARATOR: &str = "mx-1.5 h-px shrink-0 bg-hairline/50";
/// The Chief's crown badge colour (their `#E8993A`).
const CROWN: &str = "bg-[#E8993A]";

/// The dialogs the sidebar opens.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    NewBot,
    NewChannel,
    Devices,
    Settings(&'static str),
    Triggers,
    Memory,
    Usage,
}

#[component]
pub fn Sidebar(bots: ReadSignal<Vec<Bot>>, selected: Signal<Option<String>>) -> Element {
    let mut query = use_signal(String::new);
    let mut searching = use_signal(|| false);
    let mut adding = use_signal(|| false);
    let mut open = use_signal(|| None::<Open>);
    // Bots whose chats are showing.
    let mut unfolded = use_signal(HashSet::<String>::new);
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    // Ctrl+, (⌘, on a Mac) opens Settings, as in MausBot.
    use_hook(|| {
        spawn(async move {
            let mut keys = document::eval(
                r#"document.addEventListener("keydown", (event) => {
                    if ((event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.key === ",") { event.preventDefault(); dioxus.send("settings"); }
                });"#,
            );
            while keys.recv::<String>().await.is_ok() {
                let mut prefs = ui.prefs;
                prefs.set(Some("general".into()));
            }
        });
    });
    let q = query.read().trim().to_lowercase();
    let look = use_context::<Signal<crate::looks::Looks>>();
    let show_chief = look.read().show_chief;
    let tint = crate::looks::tint_color(&look.read().tint);
    let all = bots.read();
    // The Chief stands apart as a tile while nothing's being searched for. Hidden in Settings,
    // only a search shows it.
    let the_chief = all.iter().find(|b| b.chief && b.channel.is_none()).cloned();
    let chief = the_chief.clone().filter(|_| q.is_empty() && show_chief);
    // The Team Chat has its own row at the top (stage 30.1), and is in the list only for a search.
    let team_chat = all.iter().find(|b| b.is_team_chat()).cloned().filter(|_| q.is_empty());
    let shown: Vec<Bot> = all
        .iter()
        .filter(|b| b.matches(&query.read()) && !the_chief.as_ref().is_some_and(|c| c.id == b.id && q.is_empty()) && !(b.is_team_chat() && q.is_empty()))
        .cloned()
        .collect();
    let empty = all.is_empty();
    drop(all);
    // Grouped by department once there are any (not while searching): each heading, then its bots;
    // General last, headed only when there are departments.
    let depts = use_context::<Signal<crate::departments::Depts>>();
    let mut department_dialog = use_signal(|| None::<crate::departments::Open>);
    let dragging = use_signal(|| None::<String>);
    let folded = look.read().folded.clone();
    let grouped: Vec<(Option<String>, Vec<Bot>)> = if depts.read().list.is_empty() || !q.is_empty() {
        vec![(None, shown.clone())]
    } else {
        crate::departments::groups(&shown, &depts.read()).into_iter().map(|(name, bots)| (Some(name), bots)).collect()
    };
    // On a phone the list gives way to the open chat.
    let phone = if (ui.showing_chat)() { "max-md:hidden" } else { "" };
    let pick = move |id: String| {
        let (mut map, mut board) = (ui.map, ui.board);
        map.set(false);
        board.set(false);
        selected.set(Some(id));
        let mut showing = ui.showing_chat;
        showing.set(true);
    };
    let mut close_search = move || {
        searching.set(false);
        query.set(String::new());
    };
    rsx! {
        aside {
            "aria-label": "Bots",
            "data-sidebar": true,
            class: "relative flex h-full w-[280px] shrink-0 flex-col border-r-[0.5px] border-edge bg-panel max-md:w-full max-md:border-r-0 max-md:bg-app {phone}",
            // The sidebar's colour (Settings › This computer): faded into its top.
            if let Some(color) = tint {
                div { "aria-hidden": "true", "data-sidebar-tint": true, class: "pointer-events-none absolute inset-x-0 top-0 h-[180px]", style: "background: linear-gradient(to bottom, color-mix(in srgb, {color} 22%, transparent), transparent)" }
            }
            div { "data-sidebar-top-row": true, class: "relative flex h-[60px] shrink-0 items-start pl-4 pr-4 pt-3",
                div { class: "min-w-0 flex-1" }
                div { "data-sidebar-top-buttons": true, class: "relative flex shrink-0 items-center gap-2.5",
                    button {
                        r#type: "button",
                        "data-sidebar-search-toggle": true,
                        "aria-label": "Search bots and messages",
                        "aria-expanded": "{searching()}",
                        title: "Search",
                        class: "{TOP_BUTTON} {ELEVATED}",
                        onclick: move |_| if searching() { close_search() } else { searching.set(true) },
                        Icon { name: "search", size: 16 }
                    }
                    button {
                        r#type: "button",
                        "data-sidebar-new": true,
                        "aria-label": "New",
                        "aria-haspopup": "menu",
                        "aria-expanded": "{adding()}",
                        title: "New bot or channel",
                        class: "{TOP_BUTTON} {ELEVATED}",
                        onclick: move |_| adding.toggle(),
                        Icon { name: "plus", size: 17 }
                    }
                    if adding() {
                        div { class: "fixed inset-0 z-40", onclick: move |_| adding.set(false) }
                        div { role: "menu", "data-sidebar-new-menu": true, class: "absolute right-0 top-full z-50 mt-1.5 w-[220px] {MENU_SHEET}",
                            button { r#type: "button", role: "menuitem", "data-new-bot": true, class: MENU_ROW,
                                onclick: move |_| { adding.set(false); open.set(Some(Open::NewBot)); },
                                span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "bot", size: 16 } }
                                span { class: "flex-1", "New bot" }
                            }
                            button { r#type: "button", role: "menuitem", "data-open-new-department": true, class: MENU_ROW,
                                onclick: move |_| { adding.set(false); department_dialog.set(Some(crate::departments::Open::New(Vec::new()))); },
                                span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "users", size: 16 } }
                                div { class: "flex min-w-0 flex-1 flex-col",
                                    span { "New department" }
                                    span { class: "truncate text-[12px] leading-4 text-ink-secondary", "Group bots that work together" }
                                }
                            }
                            button { r#type: "button", role: "menuitem", "data-open-new-channel": true, class: MENU_ROW,
                                onclick: move |_| { adding.set(false); open.set(Some(Open::NewChannel)); },
                                span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "hash", size: 16 } }
                                div { class: "flex min-w-0 flex-1 flex-col",
                                    span { "New channel" }
                                    span { class: "truncate text-[12px] leading-4 text-ink-secondary", "A group chat of several bots" }
                                }
                            }
                        }
                    }
                }
            }
            if searching() {
                div { "data-sidebar-search-pill": true, class: "px-3 pb-2 pt-0.5",
                    div { class: "flex h-[34px] items-center gap-2 rounded-full pl-3 pr-1.5 text-ink-secondary focus-within:ring-focus bg-composer ring-1 ring-inset ring-ink/[0.07]",
                        Icon { name: "search", size: 15 }
                        input {
                            id: "sidebar-search",
                            autofocus: true,
                            placeholder: "Search",
                            "aria-label": "Search bots and messages",
                            class: "min-w-0 flex-1 bg-transparent text-[13px] text-ink placeholder:text-ink-tertiary focus:outline-none",
                            oninput: move |e| query.set(e.value()),
                            onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                            onkeydown: move |e| if e.key() == Key::Escape { e.prevent_default(); close_search(); },
                        }
                        button { r#type: "button", "aria-label": "Close search", class: "flex size-6 shrink-0 items-center justify-center rounded-full text-ink-tertiary hover:bg-ink/[0.06] hover:text-ink",
                            onclick: move |_| close_search(),
                            Icon { name: "x", size: 14 }
                        }
                    }
                }
            }
            if q.is_empty() {
                crate::inbox::InboxRow {}
                TeamMapRow {}
                if let Some(team) = team_chat {
                    TeamChatRow { selected: selected.read().as_deref() == Some(team.id.as_str()), bot: team, on_select: pick }
                }
                BoardRow {}
            }
            div { class: "min-h-0 flex-1 overflow-y-auto",
                if let Some(chief) = chief {
                    div { "data-sidebar-tiles": true, class: "flex flex-wrap justify-center gap-x-1 gap-y-3 px-3 pb-4 pt-2.5",
                        ChiefTile { selected: selected.read().as_deref() == Some(chief.id.as_str()), bot: chief, on_select: pick }
                    }
                }
                div { class: "flex flex-col gap-0.5 px-3",
                    if empty {
                        div { "data-sidebar-empty": true, class: "px-2 py-6 text-center text-[13px] leading-[18px] text-ink-tertiary",
                            "No bots yet. Make one with the + above."
                        }
                    } else if shown.is_empty() && !q.is_empty() {
                        div { class: "px-2 py-6 text-center text-[13px] text-ink-secondary", "Nothing matches “{query}”" }
                    }
                    for (group, members) in grouped {
                        if let Some(name) = group.clone() {
                            crate::departments::Header {
                                key: "dept-{name}",
                                name: name.clone(),
                                bots: members.clone(),
                                folded: folded.contains(&name),
                                dragging,
                                on_toggle: {
                                    let name = name.clone();
                                    move |_| crate::looks::change(look, |l| {
                                        if let Some(at) = l.folded.iter().position(|f| *f == name) { l.folded.remove(at); } else { l.folded.push(name.clone()); }
                                    })
                                },
                                on_open: move |o| department_dialog.set(Some(o)),
                            }
                        }
                        if group.as_ref().is_some_and(|name| name != crate::departments::GENERAL && !folded.contains(name)) && members.is_empty() {
                            div { "data-department-empty": true, class: "px-2 pb-1 text-[12px] leading-4 text-ink-tertiary", "No bots yet. Drag one here, or right-click a bot." }
                        }
                        if group.as_ref().is_none_or(|name| !folded.contains(name)) {
                            for bot in members {
                                BotRow {
                                    key: "{bot.id}",
                                    selected: selected.read().as_deref() == Some(bot.id.as_str()),
                                    // Open by the arrow, or while a search finds one of its chats.
                                    unfolded: unfolded.read().contains(&bot.id) || chat_found(&bot, &query.read()),
                                    query: query.read().clone(),
                                    bot,
                                    on_select: pick,
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
                                    dragging,
                                    on_departments: move |o| department_dialog.set(Some(o)),
                                }
                            }
                        }
                    }
                }
            }
            crate::profile::ArchivedBots {}
            crate::departments::Dialogs { open: department_dialog }
            Footer { bot_count: bots.read().iter().filter(|b| b.channel.is_none()).count(), on_open: move |o| match o {
                // Settings belongs to the app (the chat opens it too).
                Open::Settings(page) => { let mut prefs = ui.prefs; prefs.set(Some(page.to_string())); }
                other => open.set(Some(other)),
            } }
            match open() {
                Some(Open::NewBot) => rsx! { crate::settings::NewBotDialog { on_close: move |_| open.set(None) } },
                Some(Open::NewChannel) => rsx! { crate::channels::NewChannelDialog { on_close: move |_| open.set(None) } },
                Some(Open::Devices) => rsx! { crate::devices::DevicesDialog { on_close: move |_| open.set(None) } },
                Some(Open::Settings(_)) => rsx! {},
                Some(Open::Triggers) => rsx! { crate::triggers::TriggersDialog { on_close: move |_| open.set(None) } },
                Some(Open::Memory) => rsx! { crate::memory::MemoryDialog { on_close: move |_| open.set(None) } },
                Some(Open::Usage) => rsx! { crate::usage::UsageDialog { on_close: move |_| open.set(None) } },
                None => rsx! {},
            }
            if let Some(page) = (ui.prefs)() {
                crate::notify::SettingsDialog { start: page, on_close: move |_| { let mut prefs = ui.prefs; prefs.set(None); } }
            }
        }
    }
}

/// The Team map's row under the Inbox: opens the map in place of the chat.
#[component]
fn TeamMapRow() -> Element {
    let ui = use_context::<Ui>();
    let open = (ui.map)();
    rsx! {
        div { class: "px-3 pb-1",
            button { r#type: "button", "data-sidebar-team-map": true, "aria-pressed": "{open}", title: "Team map",
                class: "flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none hover:bg-raised/60 focus-visible:ring-2 focus-visible:ring-accent/60",
                onclick: move |_| {
                    let (mut map, mut board, mut showing) = (ui.map, ui.board, ui.showing_chat);
                    map.set(!open);
                    board.set(false);
                    showing.set(!open);
                },
                span { class: if open { "grid size-9 shrink-0 place-items-center rounded-full bg-ink/[0.16] text-ink" } else { "grid size-9 shrink-0 place-items-center rounded-full bg-raised text-ink-secondary" },
                    Icon { name: "users", size: 16 }
                }
                span { class: "min-w-0 flex-1 truncate text-[14px] font-medium leading-5 text-ink", "Team map" }
            }
        }
    }
}

/// The task board, below the Team Chat (stage 30.3): how many items are open.
#[component]
fn BoardRow() -> Element {
    let ui = use_context::<Ui>();
    let state = use_context::<Signal<State>>();
    let open = (ui.board)();
    let mut count = use_signal(|| 0usize);
    use_effect(move || {
        let _ = state.read().board_changed;
        spawn(async move {
            if let Ok(found) = crate::api::get("/api/board").await {
                let open = found["items"].as_array().into_iter().flatten().filter(|i| !matches!(i["state"].as_str(), Some("done" | "failed" | "cancelled"))).count();
                count.set(open);
            }
        });
    });
    rsx! {
        div { class: "px-3 pb-1",
            button { r#type: "button", "data-sidebar-board": true, "aria-pressed": "{open}", title: "Task board",
                class: "flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none hover:bg-raised/60 focus-visible:ring-2 focus-visible:ring-accent/60",
                onclick: move |_| {
                    let (mut board, mut map, mut showing) = (ui.board, ui.map, ui.showing_chat);
                    board.set(!open);
                    map.set(false);
                    showing.set(!open);
                },
                span { class: if open { "grid size-9 shrink-0 place-items-center rounded-full bg-ink/[0.16] text-ink" } else { "grid size-9 shrink-0 place-items-center rounded-full bg-raised text-ink-secondary" },
                    Icon { name: "check-circle", size: 16 }
                }
                span { class: "min-w-0 flex-1 truncate text-[14px] font-medium leading-5 text-ink", "Task board" }
                if count() > 0 {
                    span { class: "shrink-0 text-[12px] tabular-nums text-ink-tertiary", "{count}" }
                }
            }
        }
    }
}

/// The Team Chat, below the Team map: every bot in one room (stage 30.1).
#[component]
fn TeamChatRow(bot: Bot, selected: bool, on_select: EventHandler<String>) -> Element {
    let ui = use_context::<Ui>();
    let id = bot.id.clone();
    let busy = bot.activity != Activity::Idle;
    let count = bot.channel.as_ref().map_or(0, |c| c.members.len());
    let ring = if selected { "bg-ink/[0.16] text-ink" } else { "bg-raised text-ink-secondary" };
    let weight = if bot.unread { "font-semibold" } else { "font-medium" };
    rsx! {
        div { class: "px-3 pb-1",
            button { r#type: "button", "data-sidebar-team-chat": true, "aria-current": if selected { "true" } else { "false" }, title: "Team Chat: every bot, together",
                class: "flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none hover:bg-raised/60 focus-visible:ring-2 focus-visible:ring-accent/60",
                onclick: move |_| {
                    let (mut map, mut board) = (ui.map, ui.board);
                    map.set(false);
                    board.set(false);
                    on_select.call(id.clone());
                },
                span { class: "grid size-9 shrink-0 place-items-center rounded-full {ring}",
                    Icon { name: "message", size: 16 }
                }
                span { class: "min-w-0 flex-1 truncate text-[14px] leading-5 text-ink {weight}", "Team Chat" }
                if busy {
                    span { role: "img", "aria-label": "Working", class: "size-1.5 shrink-0 animate-pulse rounded-full bg-accent" }
                } else if bot.unread {
                    span { role: "img", "aria-label": "Unread", class: "size-2 shrink-0 rounded-full bg-accent" }
                } else {
                    span { class: "shrink-0 text-[12px] tabular-nums text-ink-tertiary", "{count}" }
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

/// The marker on a bot's picture: waiting for you wins, then working (`SidebarRowMarkers.tsx`).
fn status_label(bot: &Bot) -> &'static str {
    match bot.activity {
        Activity::WaitingOnYou => "Waiting for you…",
        Activity::Working => "Working",
        _ => "",
    }
}

/// The Chief of Staff, as a tile above the list (`SidebarBotTile`, primary): its picture large,
/// with the crown, and its name under it.
#[component]
fn ChiefTile(bot: Bot, selected: bool, on_select: EventHandler<String>) -> Element {
    let (id, key_id) = (bot.id.clone(), bot.id.clone());
    let waiting = bot.activity == Activity::WaitingOnYou;
    let working = bot.activity == Activity::Working;
    let tile = if selected { "[--tile:var(--color-raised)]" } else { "[--tile:var(--color-panel)] hover:[--tile:color-mix(in_srgb,var(--color-ink)_6%,var(--color-panel))]" };
    let label = [bot.name.as_str(), "Chief of Staff", status_label(&bot), if bot.unread { "Unread" } else { "" }].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
    rsx! {
        div {
            role: "button",
            tabindex: 0,
            "aria-label": "{label}",
            "aria-current": if selected { "page" },
            "data-sidebar-bot-row": "{bot.id}",
            "data-sidebar-tile": "primary",
            "data-cat-follow": true,
            title: "{bot.name} · Chief of Staff",
            class: "flex h-[92px] w-[95px] shrink-0 cursor-pointer flex-col items-center rounded-xl bg-[var(--tile)] px-2 pb-1 pt-1.5 transition-colors duration-150 {ROW_FOCUS} {tile}",
            onclick: move |_| on_select.call(id.clone()),
            onkeydown: move |e| {
                if e.key() == Key::Enter || e.key() == Key::Character(" ".into()) {
                    e.prevent_default();
                    on_select.call(key_id.clone());
                }
            },
            span { class: "relative flex",
                BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 60, mark: bot.mark.clone(), state: bot.activity.mood().to_string() }
                span { "data-chief-badge": true, "aria-hidden": "true", class: "absolute -bottom-px -right-px flex size-4 items-center justify-center rounded-full text-white ring-2 ring-[var(--tile)] {CROWN}",
                    Icon { name: "crown", size: 10, filled: true }
                }
                if waiting {
                    span { "data-testid": "waiting-dot", "aria-hidden": "true", class: "absolute -right-1 -top-1 flex size-4 items-center justify-center rounded-full bg-warning text-[11px] font-black leading-none text-panel ring-2 ring-[var(--tile)]", "!" }
                } else if working {
                    span { "data-testid": "working-dot", "aria-hidden": "true", class: "absolute -right-0.5 -top-0.5 size-3 rounded-full bg-success ring-2 ring-[var(--tile)]" }
                } else if bot.unread {
                    span { "data-testid": "unread-dot", "aria-hidden": "true", class: "absolute -right-0.5 -top-0.5 size-3 rounded-full bg-accent ring-2 ring-[var(--tile)]" }
                }
            }
            span { class: "mt-1.5 flex w-full min-w-0 justify-center",
                span { class: "min-w-0 truncate text-center text-[12px] leading-4 text-ink", "{bot.name}" }
            }
        }
    }
}

/// A bot (or channel) as a roomy row (`BotListItem`, simple): its picture, its name, and a line
/// of what it last said or is doing. Hovering shows its chats and New chat.
#[component]
fn BotRow(
    bot: Bot,
    selected: bool,
    unfolded: bool,
    query: String,
    on_select: EventHandler<String>,
    on_fold: EventHandler<String>,
    on_new_chat: EventHandler<String>,
    /// The bot being dragged to another department, if one is.
    dragging: Signal<Option<String>>,
    on_departments: EventHandler<crate::departments::Open>,
) -> Element {
    let (id, key_id, fold_id, new_id) = (bot.id.clone(), bot.id.clone(), bot.id.clone(), bot.id.clone());
    let drag_id = bot.id.clone();
    // Right-click: move it to another department.
    let mut moving = use_signal(|| false);
    let is_bot = bot.channel.is_none();
    let waiting = bot.activity == Activity::WaitingOnYou;
    let working = bot.activity == Activity::Working;
    // The arrow shows once there's more than one chat to choose between.
    let has_list = bot.listed_chats().count() > 1 || bot.chats.iter().any(|c| c.archived) || !bot.folders.is_empty();
    let expanded = has_list && unfolded;
    let surface = if selected && !expanded {
        "[--row:var(--color-raised)] bg-[var(--row)]"
    } else {
        "[--row:var(--color-panel)] max-md:[--row:var(--color-app)] bg-[var(--row)] hover:[--row:color-mix(in_srgb,var(--color-ink)_6%,var(--color-panel))]"
    };
    // A draft left in the bot's open chat shows instead, unless the bot is busy or asking.
    let draft = use_context::<Signal<crate::drafts::Drafts>>()
        .read()
        .get(&bot.thread_id)
        .filter(|_| !selected && bot.activity == Activity::Idle)
        .map(|d| d.line());
    let preview = if waiting { "Waiting for you…".to_string() } else { bot.preview() };
    let line_color = if waiting { "text-warning" } else { "text-ink-secondary" };
    let fold_label = if expanded { format!("Hide {} chats", bot.name) } else { format!("Show {} chats", bot.name) };
    let status = [status_label(&bot), if bot.unread { "Unread" } else { "" }].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ");
    rsx! {
        div { class: "group relative",
            div {
                role: "button",
                tabindex: 0,
                "aria-current": if selected && !expanded { "page" },
                "data-sidebar-bot-row": "{bot.id}",
                draggable: if is_bot { "true" } else { "false" },
                ondragstart: move |_| if is_bot { dragging.set(Some(drag_id.clone())) },
                ondragend: move |_| dragging.set(None),
                oncontextmenu: move |e| if is_bot { e.prevent_default(); moving.set(true); },
                // The cat's eyes follow the pointer anywhere on its row (see `MASCOT_LIFE`).
                "data-cat-follow": true,
                class: "flex w-full items-center gap-2.5 rounded-xl p-2 text-left transition-colors duration-150 md:h-[54px] md:py-0 max-md:rounded-2xl max-md:py-2.5 {ROW_FOCUS} {surface}",
                onclick: move |_| on_select.call(id.clone()),
                onkeydown: move |e| {
                    if e.key() == Key::Enter || e.key() == Key::Character(" ".into()) {
                        e.prevent_default();
                        on_select.call(key_id.clone());
                    }
                },
                span { class: "relative -m-[3px] flex shrink-0", title: "{bot.name}",
                    BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 42, mark: bot.mark.clone(), state: bot.activity.mood().to_string() }
                    if working {
                        span { "data-testid": "working-dot", "aria-hidden": "true", title: "Working", class: "absolute bottom-0.5 right-0.5 size-2 rounded-full bg-success ring-2 ring-[var(--row)]" }
                    }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex items-baseline justify-between gap-2 pe-7 group-hover:pe-[3.25rem] group-focus-within:pe-[3.25rem]",
                        span { class: "flex min-w-0 grow items-center gap-1.5 truncate text-[14px] font-medium leading-5 text-ink",
                            span { class: "truncate", "{bot.name}" }
                            if bot.chief {
                                span { class: "shrink-0 text-accent", role: "img", "aria-label": "Chief of Staff", "data-chief-crown": true, Icon { name: "crown", size: 13 } }
                            } else if bot.department_chief {
                                span { class: "shrink-0 text-ink-tertiary", role: "img", "aria-label": "Leads {bot.department}", title: "Leads {bot.department}", "data-lead-crown": true, Icon { name: "crown", size: 12 } }
                            }
                        }
                    }
                    if !expanded {
                        div { class: "flex items-center gap-2",
                            if let Some(draft) = draft {
                                span { dir: "auto", class: "min-w-0 truncate text-[13px] leading-[18px]", "data-sidebar-draft": true,
                                    span { class: "font-semibold text-danger", "Draft: " }
                                    span { class: "text-ink-secondary", "{draft}" }
                                }
                            } else if !preview.is_empty() {
                                span { dir: "auto", class: "min-w-0 truncate text-[13px] leading-[18px] {line_color}", "{preview}" }
                            }
                        }
                    }
                    if !status.is_empty() {
                        span { class: "sr-only", "{status}" }
                    }
                }
            }
            // Unread, or waiting for you (which wins): one dot at the row's top right, in the spot
            // its buttons take on hover (`RowDot`).
            if (bot.unread || waiting) && !expanded {
                span { "aria-hidden": "true", "data-row-dot": true, class: "pointer-events-none absolute right-[3px] top-1.5 flex size-6 items-center justify-center transition-opacity group-hover:opacity-0 group-focus-within:opacity-0",
                    span { "data-testid": if waiting { "waiting-dot" } else { "unread-dot" }, class: if waiting { "size-2 shrink-0 rounded-full bg-warning" } else { "size-2 shrink-0 rounded-full bg-accent" } }
                }
            }
            div { class: "absolute right-1.5 top-[27px] flex -translate-y-1/2 items-center gap-0.5",
                if has_list {
                    button {
                        r#type: "button",
                        "aria-label": "{fold_label}",
                        "aria-expanded": "{expanded}",
                        "data-chat-fold": "{bot.id}",
                        title: if expanded { "Hide chats" } else { "Show chats" },
                        class: if expanded { "flex size-6 items-center justify-center rounded-md text-ink-secondary hover:bg-ink/[0.06] hover:text-ink" } else { "pointer-events-none flex size-6 items-center justify-center rounded-md text-ink-secondary opacity-0 hover:bg-ink/[0.06] hover:text-ink group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 max-md:pointer-events-auto max-md:opacity-70" },
                        onclick: move |_| on_fold.call(fold_id.clone()),
                        Chevron { open: expanded, size: 13 }
                    }
                }
                button {
                    r#type: "button",
                    "aria-label": "New chat with {bot.name}",
                    title: "New chat",
                    "data-row-new-chat": "{bot.id}",
                    class: "pointer-events-none flex size-6 items-center justify-center rounded-md text-ink-secondary opacity-0 hover:bg-ink/[0.06] hover:text-ink group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100 max-md:pointer-events-auto max-md:opacity-70",
                    onclick: move |_| on_new_chat.call(new_id.clone()),
                    Icon { name: "plus", size: 14 }
                }
            }
            if expanded {
                ChatList { bot: bot.clone(), current: selected, query }
            }
            if moving() {
                crate::departments::MoveMenu { bot: bot.clone(), on_close: move |_| moving.set(false), on_open: move |o| on_departments.call(o) }
            }
        }
    }
}

/// The foot of the sidebar (`SimpleSidebarFooter`): the account button and its menu, and
/// Connect apps.
#[component]
fn Footer(bot_count: usize, on_open: EventHandler<Open>) -> Element {
    let mut menu = use_signal(|| false);
    let state = use_context::<Signal<State>>();
    // Shown on their rows: how many triggers are on, and whether any apps are connected.
    let mut triggers = use_signal(|| (0usize, 0usize));
    let mut apps = use_signal(|| 0usize);
    use_effect(move || {
        let _ = state.read().triggers_changed;
        spawn(async move {
            if let Ok(found) = crate::api::get("/api/triggers").await {
                let list = found["triggers"].as_array().cloned().unwrap_or_default();
                triggers.set((list.len(), list.iter().filter(|t| t["enabled"] == true).count()));
            }
        });
    });
    use_effect(move || {
        let _ = state.read().apps_changed;
        spawn(async move {
            if let Ok(found) = crate::api::get("/api/apps").await {
                apps.set(found["servers"].as_array().map(|s| s.iter().filter(|a| a["enabled"] != false).count()).unwrap_or(0));
            }
        });
    });
    // Pairing other devices is done at home: not on the phone, nor in the Windows app.
    let at_home = !crate::link::pairs() && !crate::link::has_own();
    // Taken every time (hooks keep their order), used only in the Windows app.
    let all_places = use_context::<crate::settings::Places>();
    let places = crate::link::has_own().then_some(all_places);
    let mut pairing_home = use_signal(|| false);
    let mut choose = move |o: Open| {
        menu.set(false);
        on_open.call(o);
    };
    let where_ = if crate::link::pairs() || places.is_some_and(|p| (p.place)() == crate::link::Place::Home) { "at home" } else if crate::link::is_phone() { "on this phone" } else { "on this computer" };
    let bots_line = format!("{bot_count} bot{} {where_}", if bot_count == 1 { "" } else { "s" });
    let (count, on) = triggers();
    let label = if apps() > 0 { "Apps" } else { "Connect apps" };
    rsx! {
        div { "data-sidebar-footer": "simple", class: "relative shrink-0 px-4 pb-4 pt-2",
            if let Some(places) = places {
                if let Some(notice) = (places.notice)() {
                    p { role: "status", class: "mb-2 px-1 text-[12px] leading-[17px] text-warning", "{notice}" }
                }
            }
            div { class: "flex items-center gap-2",
                button {
                    r#type: "button",
                    "data-sidebar-account": true,
                    "aria-label": "Catbot menu",
                    "aria-haspopup": "menu",
                    "aria-expanded": "{menu()}",
                    title: "Catbot",
                    class: "shrink-0 rounded-full focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus",
                    onclick: move |_| menu.toggle(),
                    span { class: if menu() { "relative flex size-9 items-center justify-center rounded-full ring-2 ring-ink/15 transition-shadow" } else { "relative flex size-9 items-center justify-center rounded-full ring-2 ring-transparent transition-shadow hover:ring-ink/10" },
                        span { class: "flex size-9 items-center justify-center rounded-full bg-raised text-ink-secondary ring-1 ring-inset ring-ink/[0.04]",
                            Icon { name: "user", size: 16 }
                        }
                    }
                }
                button {
                    r#type: "button",
                    "data-sidebar-nav": "apps",
                    class: "group/apps flex h-9 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-full pl-4 pr-3 text-[14px] font-medium leading-5 text-ink {ELEVATED}",
                    onclick: move |_| on_open.call(Open::Settings("apps")),
                    span { "data-apps-label": true, class: "shrink-0 whitespace-nowrap", "{label}" }
                    // Faces of popular apps, grey until hovered (`SidebarConnectApps`): their
                    // initials, not their logos.
                    span { "aria-hidden": "true", "data-app-faces": true, class: "flex h-[22px] min-w-0 items-center gap-1 overflow-hidden px-1 opacity-60 grayscale motion-safe:transition-[filter,opacity] group-hover/apps:opacity-100 group-hover/apps:grayscale-0",
                        for (letter, tint, fan) in [("G", "text-[#EA4335]", "motion-safe:group-hover/apps:-translate-x-[3px] motion-safe:group-hover/apps:-rotate-[8deg]"), ("S", "text-[#611F69]", ""), ("N", "text-ink", "motion-safe:group-hover/apps:translate-x-[3px] motion-safe:group-hover/apps:rotate-[8deg]")] {
                            span { key: "{letter}", class: "flex size-[18px] shrink-0 items-center justify-center rounded-md text-[11px] font-bold leading-none motion-safe:transition-transform motion-safe:duration-150 group-hover/apps:bg-white {tint} {fan}", "{letter}" }
                        }
                    }
                }
            }
            if menu() {
                div { class: "fixed inset-0 z-40", onclick: move |_| menu.set(false) }
                div { role: "menu", "aria-label": "Catbot", "data-sidebar-menu": true, class: "absolute bottom-full left-3 z-50 mb-1 w-[272px] max-w-[calc(100vw-1.5rem)] {MENU_SHEET}",
                    button { r#type: "button", role: "menuitem", "data-menu-key": "catbot", class: "{MENU_ROW} min-h-12",
                        onclick: move |_| choose(Open::Settings("general")),
                        span { class: "flex size-8 shrink-0 items-center justify-center rounded-full bg-raised text-ink-secondary", Icon { name: "laptop", size: 16 } }
                        div { class: "flex min-w-0 flex-1 flex-col",
                            span { class: "truncate font-medium leading-[18px]", "Catbot" }
                            span { class: "truncate text-[12px] leading-4 text-ink-secondary", "{bots_line}" }
                        }
                        span { class: "shrink-0 text-ink-secondary", Icon { name: "chevron-right", size: 14 } }
                    }
                    div { role: "separator", class: MENU_SEPARATOR }
                    if at_home {
                        button { r#type: "button", role: "menuitem", "data-open-devices": true, class: MENU_ROW,
                            onclick: move |_| choose(Open::Devices),
                            MenuIcon { name: "phone" }
                            div { class: "flex min-w-0 flex-1 flex-col",
                                span { "Phone & devices" }
                                span { class: "truncate text-[12px] leading-4 text-ink-secondary", "Reach your bots away from home" }
                            }
                        }
                    }
                    button { r#type: "button", role: "menuitem", "data-open-settings": true, class: MENU_ROW,
                        onclick: move |_| choose(Open::Settings("general")),
                        MenuIcon { name: "settings" }
                        span { class: "flex-1", "Settings" }
                        span { class: "shrink-0 text-[12px] text-ink-secondary", "Ctrl+," }
                    }
                    div { role: "separator", class: MENU_SEPARATOR }
                    button { r#type: "button", role: "menuitem", "data-open-triggers": true, class: MENU_ROW,
                        onclick: move |_| choose(Open::Triggers),
                        MenuIcon { name: "zap" }
                        span { class: "flex-1", "Triggers" }
                        if count > 0 {
                            span { class: "shrink-0 text-[13px] tabular-nums text-ink-secondary", "{on} on" }
                        }
                    }
                    button { r#type: "button", role: "menuitem", "data-open-memory": true, class: MENU_ROW,
                        onclick: move |_| choose(Open::Memory),
                        MenuIcon { name: "book" }
                        span { class: "flex-1", "Memory" }
                    }
                    button { r#type: "button", role: "menuitem", "data-open-usage": true, class: MENU_ROW,
                        onclick: move |_| choose(Open::Usage),
                        MenuIcon { name: "gauge" }
                        span { class: "flex-1", "Usage" }
                    }
                    button { r#type: "button", role: "menuitem", "data-open-pages": true, class: MENU_ROW,
                        onclick: move |_| choose(Open::Settings("pages")),
                        MenuIcon { name: "rocket" }
                        span { class: "flex-1", "Published pages" }
                    }
                    if let Some(places) = places {
                        div { role: "separator", class: MENU_SEPARATOR }
                        div { class: "px-2 pb-1 pt-2 text-[12px] font-medium leading-4 text-ink-secondary", "Switch place" }
                        for (place, name, glyph, data) in [(crate::link::Place::ThisPc, crate::link::own_name(), if crate::link::is_phone() { "smartphone" } else { "laptop" }, "this-pc"), (crate::link::Place::Home, "Home PC", "home", "home")] {
                            button {
                                key: "{name}",
                                r#type: "button",
                                role: "menuitemradio",
                                "aria-checked": "{(places.place)() == place}",
                                "data-place": data,
                                class: MENU_ROW,
                                onclick: move |_| {
                                    menu.set(false);
                                    if place == crate::link::Place::Home && crate::link::current().is_none() {
                                        pairing_home.set(true);
                                    } else {
                                        go_to(places, place);
                                    }
                                },
                                MenuIcon { name: glyph }
                                span { class: "flex-1", "{name}" }
                                if (places.place)() == place {
                                    span { class: "shrink-0 text-accent", Icon { name: "check", size: 15 } }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(places) = places {
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
        }
    }
}

#[component]
fn MenuIcon(name: &'static str) -> Element {
    rsx! {
        span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name, size: 16 } }
    }
}

/// Moves the app to a place and keeps the choice.
fn go_to(mut places: crate::settings::Places, place: crate::link::Place) {
    crate::link::set_place(place);
    places.notice.set(None);
    places.place.set(place);
}

/// The sidebar's glyphs: Lucide icons (ISC licence), by name.
#[component]
pub fn Icon(name: &'static str, size: u32, #[props(default)] filled: bool) -> Element {
    let paths: &[&str] = match name {
        "search" => &["m21 21-4.34-4.34", "M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0"],
        "plus" => &["M5 12h14", "M12 5v14"],
        "x" => &["M18 6 6 18", "m6 6 12 12"],
        "bot" => &["M12 8V4H8", "M6 8h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2z", "M2 14h2", "M20 14h2", "M15 13v2", "M9 13v2"],
        "hash" => &["M4 9h16", "M4 15h16", "M10 3 8 21", "M16 3l-2 18"],
        "crown" => &["M11.562 3.266a.5.5 0 0 1 .876 0L15.39 8.87a1 1 0 0 0 1.516.294L21.183 5.5a.5.5 0 0 1 .798.519l-2.834 10.246a1 1 0 0 1-.956.734H5.81a1 1 0 0 1-.957-.734L2.02 6.02a.5.5 0 0 1 .798-.519l4.276 3.664a1 1 0 0 0 1.516-.294z", "M5 21h14"],
        "user" => &["M17 8a5 5 0 1 1-10 0 5 5 0 0 1 10 0z", "M20 21a8 8 0 0 0-16 0"],
        "smartphone" => &["M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z", "M12 18h.01"],
        "laptop" => &["M18 5a2 2 0 0 1 2 2v8.526a2 2 0 0 0 .212.897l1.068 2.127a1 1 0 0 1-.9 1.45H3.62a1 1 0 0 1-.9-1.45l1.068-2.127A2 2 0 0 0 4 15.526V7a2 2 0 0 1 2-2z", "M20.054 15.987H3.946"],
        "phone" => &["M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z", "M12 18h.01"],
        "settings" => &[
            "M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915",
            "M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
        ],
        "zap" => &["M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"],
        "book" => &["M12 7v14", "M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z"],
        "gauge" => &["m12 14 4-4", "M3.34 19a10 10 0 1 1 17.32 0"],
        "rocket" => &["M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z", "m12 15-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z", "M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0", "M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"],
        "home" => &["M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8", "M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"],
        "check" => &["M20 6 9 17l-5-5"],
        "sparkles" => &["M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z", "M20 3v4", "M22 5h-4"],
        "key" => &["M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z", "M16.5 7.5h.01"],
        "puzzle" => &["M15.39 4.39a1 1 0 0 0 1.68-.474 2.5 2.5 0 1 1 3.014 3.015 1 1 0 0 0-.474 1.68l1.683 1.682a2.414 2.414 0 0 1 0 3.414L19.61 15.39a1 1 0 0 1-1.68-.474 2.5 2.5 0 1 0-3.014 3.015 1 1 0 0 1 .474 1.68l-1.683 1.682a2.414 2.414 0 0 1-3.414 0L8.61 19.61a1 1 0 0 0-1.68.474 2.5 2.5 0 1 1-3.014-3.015 1 1 0 0 0 .474-1.68l-1.683-1.682a2.414 2.414 0 0 1 0-3.414L4.39 8.61a1 1 0 0 1 1.68.474 2.5 2.5 0 1 0 3.014-3.015 1 1 0 0 1-.474-1.68l1.683-1.682a2.414 2.414 0 0 1 3.414 0z"],
        "lightbulb" => &["M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5", "M9 18h6", "M10 22h4"],
        "monitor" => &["M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z", "M8 21h8", "M12 17v4"],
        "coins" => &["M16 8a6 6 0 1 1-12 0 6 6 0 0 1 12 0z", "M18.09 10.37A6 6 0 1 1 10.34 18", "M7 6h1v4", "m16.71 13.88.7.71-2.82 2.82"],
        "arrows" => &["M8 3 4 7l4 4", "M4 7h16", "m16 21 4-4-4-4", "M20 17H4"],
        "mail" => &["m22 7-8.991 5.727a2 2 0 0 1-2.009 0L2 7", "M4 4h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z"],
        "handoff" => &["M18 8L22 12L18 16", "M2 12H22", "M6 4v16"],
        "palette" => &["M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z", "M14 7.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0z", "M18 11.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0z", "M7 10.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0z", "M9 6.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0z"],
        "chevron-right" => &["m9 18 6-6-6-6"],
        "chevron-left" => &["m15 18-6-6 6-6"],
        "chevron-up" => &["m18 15-6-6-6 6"],
        "chevron-down" => &["m6 9 6 6 6-6"],
        "message" => &["M7.9 20A9 9 0 1 0 4 16.1L2 22z"],
        "minus" => &["M5 12h14"],
        "arrow-right" => &["M5 12h14", "m12 5 7 7-7 7"],
        "arrow-up" => &["m5 12 7-7 7 7", "M12 19V5"],
        "more" => &["M11 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0", "M18 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0", "M4 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0"],
        "users" => &["M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2", "M13 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0z", "M22 21v-2a4 4 0 0 0-3-3.87", "M16 3.13a4 4 0 0 1 0 7.75"],
        "inbox" => &["M22 12h-6l-2 3h-4l-2-3H2", "M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"],
        "mic" => &["M12 19v3", "M19 10v2a7 7 0 0 1-14 0v-2", "M9 5a3 3 0 0 1 6 0v7a3 3 0 0 1-6 0z"],
        "mic-off" => &["M12 19v3", "M15 9.34V5a3 3 0 0 0-5.68-1.33", "M16.95 16.95A7 7 0 0 1 5 12v-2", "M18.89 13.23A7.12 7.12 0 0 0 19 12v-2", "m2 2 20 20", "M9 9v3a3 3 0 0 0 5.12 2.12"],
        "volume" => &["M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z", "M16 9a5 5 0 0 1 0 6", "M19.364 18.364a9 9 0 0 0 0-12.728"],
        "audio-lines" => &["M2 10v3", "M6 6v11", "M10 3v18", "M14 8v7", "M18 5v13", "M22 10v3"],
        "clock" => &["M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0", "M12 6v6l4 2"],
        "maximize" => &["M15 3h6v6", "m21 3-7 7", "m3 21 7-7", "M9 21H3v-6"],
        "minimize" => &["m14 10 7-7", "M20 10h-6V4", "m3 21 7-7", "M4 14h6v6"],
        "reply" => &["M20 20v-7a4 4 0 0 0-4-4H4", "M9 14 4 9l5-5"],
        "external" => &["M15 3h6v6", "M10 14 21 3", "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"],
        "check-circle" => &["M21.801 10A10 10 0 1 1 17 3.335", "m9 11 3 3L22 4"],
        _ => &[],
    };
    let fill = if filled { "currentColor" } else { "none" };
    let stroke_width = if filled { "1.5" } else { "2" };
    rsx! {
        svg { "aria-hidden": "true", width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "{fill}", stroke: "currentColor", stroke_width: "{stroke_width}", stroke_linecap: "round", stroke_linejoin: "round",
            for d in paths.iter() {
                path { d: "{d}" }
            }
        }
    }
}
