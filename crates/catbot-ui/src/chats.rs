//! A bot's chats: the list that opens under its row in the sidebar, and what can be done with
//! one (open, start, rename, archive, delete).
//!
//! Rows and their menu follow OpenMausBot's `src/components/SidebarThreadRow.tsx` and the thread
//! list in `Sidebar.tsx` (Apache-2.0, see NOTICE).

use dioxus::prelude::*;
use serde_json::json;

use crate::api;
use crate::bots::{Activity, Bot, Chat};
use crate::chat::short_when;
use crate::live::State;
use crate::settings::Ui;

/// Their menu sheet (`MENU_SHEET`) and one of its items.
const MENU_SHEET: &str = "popover-surface flex flex-col gap-1 rounded-[var(--radius-menu)] border border-hairline/50 bg-menu p-1.5 shadow-lg";
const MENU_ITEM: &str = "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[14px] leading-[18px] text-ink hover:bg-menu-hover disabled:opacity-40";

/// Shows a chat in this window, and has the bot open to it next time.
pub fn open(mut state: Signal<State>, mut ui: Ui, bot_id: &str, thread: &str) {
    ui.map.set(false);
    if let Some(bot) = state.write().bots.iter_mut().find(|b| b.id == bot_id) {
        bot.thread_id = thread.to_string();
    }
    ui.selected.set(Some(bot_id.to_string()));
    ui.showing_chat.set(true);
    let path = format!("/api/bots/{bot_id}/tasks/{thread}?messages=0");
    spawn(async move {
        // Only remembers the choice; this window already shows the chat.
        let _ = api::post(&path, &json!({})).await;
    });
}

/// Branches a chat from one of its messages (`POST …/tasks/:thread/branch`) and shows the branch.
pub fn branch(mut state: Signal<State>, mut ui: Ui, bot_id: String, thread: String, message: String) {
    spawn(async move {
        let path = format!("/api/bots/{bot_id}/tasks/{thread}/branch");
        let Ok(answer) = api::post(&path, &json!({ "messageId": message })).await else { return };
        let Some(branch) = answer["task"]["threadId"].as_str().map(str::to_string) else { return };
        if let Some(bot) = state.write().bots.iter_mut().find(|b| b.id == bot_id) {
            if let Some(newer) = Bot::from_wire(&answer["bot"]) {
                bot.update_from(newer);
            }
            bot.thread_id = branch;
        }
        ui.selected.set(Some(bot_id));
        ui.showing_chat.set(true);
    });
}

/// Starts a new chat with the bot and shows it.
pub fn start(mut state: Signal<State>, mut ui: Ui, bot_id: String) {
    spawn(async move {
        let Ok(answer) = api::post(&format!("/api/bots/{bot_id}/tasks"), &json!({})).await else { return };
        let Some(thread) = answer["task"]["threadId"].as_str().map(str::to_string) else { return };
        if let Some(bot) = state.write().bots.iter_mut().find(|b| b.id == bot_id) {
            if let Some(newer) = Bot::from_wire(&answer["bot"]) {
                bot.update_from(newer);
            }
            bot.thread_id = thread;
        }
        ui.selected.set(Some(bot_id));
        ui.showing_chat.set(true);
    });
}

/// The chats under a bot's row: those in the list, then its archived ones, folded away.
/// `current` is whether this bot is the one open. While a search names chats but not the bot,
/// only those chats show (archived ones unfolded), as in theirs.
#[component]
pub fn ChatList(bot: Bot, current: bool, query: String) -> Element {
    let mut archived_open = use_signal(|| false);
    let query = query.trim().to_lowercase();
    let filtering = !query.is_empty() && !bot.name.to_lowercase().contains(&query);
    let found = |c: &&Chat| !filtering || c.title.to_lowercase().contains(&query);
    let listed: Vec<Chat> = bot.listed_chats().filter(found).cloned().collect();
    let archived: Vec<Chat> = bot.chats.iter().filter(|c| c.archived).filter(found).cloned().collect();
    let open_id = current.then(|| bot.thread_id.clone());
    let count = archived.len();
    let folded = !archived_open() && !filtering;
    rsx! {
        div { role: "group", "aria-label": "{bot.name} chats", "data-chat-list": "{bot.id}", class: "flex flex-col gap-px pb-1",
            for chat in listed {
                ChatRow { key: "{chat.id}", bot_id: bot.id.clone(), current: open_id.as_deref() == Some(chat.id.as_str()), chat }
            }
            if count > 0 {
                button {
                    r#type: "button",
                    "aria-expanded": "{!folded}",
                    "data-archived-toggle": true,
                    class: "flex min-h-8 w-full items-center gap-1.5 rounded-md py-1 pl-6 pr-3 text-left text-[12px] text-ink-secondary hover:bg-raised/30 hover:text-ink",
                    onclick: move |_| archived_open.toggle(),
                    Chevron { open: !folded, size: 11 }
                    span { class: "truncate", "Archived" }
                    span { class: "shrink-0 text-[10px] opacity-50", "{count}" }
                }
                if !folded {
                    for chat in archived {
                        ChatRow { key: "{chat.id}", bot_id: bot.id.clone(), current: open_id.as_deref() == Some(chat.id.as_str()), chat }
                    }
                }
            }
        }
    }
}

/// What the row's menu is showing.
#[derive(Clone, Copy, PartialEq)]
enum Menu {
    Closed,
    Open,
    ConfirmDelete,
}

#[component]
fn ChatRow(bot_id: String, chat: Chat, current: bool) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let mut menu = use_signal(|| Menu::Closed);
    let mut renaming = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut problem = use_signal(|| None::<String>);
    let working = chat.activity == Activity::Working;
    let waiting = chat.activity == Activity::WaitingOnYou;
    let busy = chat.activity != Activity::Idle;
    let when = short_when(chat.updated);
    let row_state = if current { "bg-raised" } else { "hover:bg-raised/50" };
    let title_look = if current {
        "font-semibold text-ink"
    } else {
        "text-ink-secondary hover:text-ink"
    };
    let faded = if chat.archived && !current { "text-ink-tertiary" } else { "" };
    let has_draft = use_context::<Signal<crate::drafts::Drafts>>().read().get(&chat.id).is_some();

    let (open_bot, open_thread) = (bot_id.clone(), chat.id.clone());
    let path = format!("/api/bots/{bot_id}/tasks/{}", chat.id);
    let (rename_path, archive_path, delete_path) = (path.clone(), path.clone(), path);
    let input_id = format!("rename-{}", chat.id);
    let select_input = format!(r#"const box = document.getElementById("{input_id}"); if (box) {{ box.focus(); box.select(); }}"#);

    // Saves the new name, unless it's empty or unchanged.
    let original = chat.title.clone();
    let mut finish_rename = move |save: bool| {
        if !renaming() {
            return;
        }
        renaming.set(false);
        let name = draft().trim().to_string();
        if !save || name.is_empty() || name == original {
            return;
        }
        let path = rename_path.clone();
        spawn(async move {
            if let Err(err) = api::call("PATCH", &path, Some(&json!({ "title": name }))).await {
                problem.set(Some(err));
            }
        });
    };
    let mut finish_on_blur = finish_rename.clone();

    let archived = chat.archived;
    let archive = move |_| {
        menu.set(Menu::Closed);
        let path = archive_path.clone();
        let at = if archived { serde_json::Value::Null } else { json!(1) };
        spawn(async move {
            if let Err(err) = api::call("PATCH", &path, Some(&json!({ "archivedAt": at }))).await {
                problem.set(Some(err));
            }
        });
    };
    let delete = move |_| {
        menu.set(Menu::Closed);
        let path = delete_path.clone();
        spawn(async move {
            if let Err(err) = api::call("DELETE", &path, None).await {
                problem.set(Some(err));
            }
        });
    };
    let title = chat.title.clone();
    // Lucide `ArchiveRestore` / `Archive`.
    let archive_icon = if archived {
        "M3 3h18v5H3zM5 8v11a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8M12 17v-6M9 14l3-3 3 3"
    } else {
        "M3 3h18v5H3zM5 8v11a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8M10 12h4"
    };
    rsx! {
        div { class: "relative",
            div { class: "group/thread relative flex min-w-0 items-center rounded-md {row_state}",
                if renaming() {
                    input {
                        id: "{input_id}",
                        "aria-label": "Rename chat",
                        initial_value: "{title}",
                        maxlength: 120,
                        dir: "auto",
                        class: "m-1 ml-6 min-w-0 flex-1 rounded border border-accent/50 bg-inset px-2 py-1 text-[13px] text-ink outline-none",
                        onmounted: move |_| {
                            document::eval(&select_input);
                        },
                        oninput: move |e| draft.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                e.prevent_default();
                                finish_rename(true);
                            } else if e.key() == Key::Escape {
                                finish_rename(false);
                            }
                        },
                        onblur: move |_| finish_on_blur(true),
                    }
                } else {
                    button {
                        r#type: "button",
                        "aria-current": if current { "page" },
                        "data-chat-row": "{chat.id}",
                        title: "{chat.title}",
                        class: "flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-md py-1.5 pl-6 pr-1 text-left text-[13px] font-medium outline-none focus-visible:ring-1 focus-visible:ring-accent/60 {title_look}",
                        onclick: move |_| open(state, ui, &open_bot, &open_thread),
                        span { dir: "auto", class: "min-w-0 flex-1 truncate {faded}", "{chat.title}" }
                        if has_draft && !current {
                            span { "data-chat-draft": true, class: "shrink-0 text-[11px] font-semibold text-danger", "Draft" }
                        }
                    }
                    span { class: "flex shrink-0 items-center gap-2 px-1",
                        span { class: "shrink-0 text-[10px] tabular-nums text-ink-secondary", "{when}" }
                        if waiting {
                            span { "data-chat-waiting": true, class: "shrink-0 text-[10px] font-medium text-warning", "Waiting" }
                        } else if working {
                            Spinner {}
                        }
                    }
                    button {
                        r#type: "button",
                        "aria-label": "Chat actions for {chat.title}",
                        title: "Chat actions",
                        "aria-haspopup": "menu",
                        "data-chat-actions": "{chat.id}",
                        class: "mr-0.5 flex size-6 shrink-0 items-center justify-center rounded text-ink-secondary opacity-0 hover:bg-raised hover:text-ink focus-visible:opacity-100 group-hover/thread:opacity-100 max-md:opacity-70",
                        onclick: move |_| menu.set(if menu() == Menu::Closed { Menu::Open } else { Menu::Closed }),
                        Dots {}
                    }
                }
            }
            if menu() != Menu::Closed {
                // Anywhere outside the menu closes it.
                div { class: "fixed inset-0 z-40", onclick: move |_| menu.set(Menu::Closed) }
                div { role: "menu", class: "absolute right-1 top-full z-50 w-[220px] {MENU_SHEET}",
                    if menu() == Menu::Open {
                        button {
                            r#type: "button",
                            role: "menuitem",
                            class: MENU_ITEM,
                            onclick: move |_| {
                                menu.set(Menu::Closed);
                                draft.set(title.clone());
                                renaming.set(true);
                            },
                            Icon { path: "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z" }
                            "Rename"
                        }
                        button {
                            r#type: "button",
                            role: "menuitem",
                            class: MENU_ITEM,
                            disabled: busy,
                            onclick: archive,
                            Icon { path: archive_icon }
                            if archived { "Unarchive" } else { "Archive" }
                        }
                        button {
                            r#type: "button",
                            role: "menuitem",
                            class: "{MENU_ITEM} text-danger",
                            disabled: busy,
                            title: if busy { "Stop it first: this chat is working" },
                            onclick: move |_| menu.set(Menu::ConfirmDelete),
                            Icon { path: "M3 6h18M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" }
                            "Delete"
                        }
                    } else {
                        p { class: "px-2 pt-1 text-[13px] leading-[18px] text-ink",
                            "Delete “{chat.title}”? Its messages are deleted too. This can't be undone."
                        }
                        div { class: "flex justify-end gap-1 px-1 pb-0.5 pt-1",
                            button {
                                r#type: "button",
                                class: "rounded-full px-3 py-1 text-[13px] font-medium text-ink-secondary hover:bg-ink/[0.07] hover:text-ink",
                                onclick: move |_| menu.set(Menu::Closed),
                                "Cancel"
                            }
                            button {
                                r#type: "button",
                                "data-confirm-delete": true,
                                class: "rounded-full bg-danger px-3 py-1 text-[13px] font-medium text-white hover:opacity-90",
                                onclick: delete,
                                "Delete"
                            }
                        }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "px-6 pb-1 text-[11px] text-danger", "{err}" }
            }
        }
    }
}

/// Their open/closed arrow (lucide `ChevronRight`, ISC).
#[component]
pub fn Chevron(open: bool, size: u32) -> Element {
    let turn = if open { "rotate-90" } else { "" };
    rsx! {
        svg { "aria-hidden": "true", class: "shrink-0 transition-transform {turn}", width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "m9 18 6-6-6-6" }
        }
    }
}

/// A chat at work (lucide `Loader2`, ISC), turning.
#[component]
fn Spinner() -> Element {
    rsx! {
        svg { role: "img", "aria-label": "Working", "data-chat-working": true, class: "shrink-0 animate-spin text-success", width: "11", height: "11", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M21 12a9 9 0 1 1-6.219-8.56" }
        }
    }
}

/// The row's actions button (lucide `MoreHorizontal`, ISC).
#[component]
fn Dots() -> Element {
    rsx! {
        svg { "aria-hidden": "true", width: "15", height: "15", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "12", cy: "12", r: "1" }
            circle { cx: "19", cy: "12", r: "1" }
            circle { cx: "5", cy: "12", r: "1" }
        }
    }
}

/// A 16px menu glyph from one lucide path (ISC).
#[component]
fn Icon(path: String) -> Element {
    rsx! {
        svg { "aria-hidden": "true", class: "shrink-0", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.5", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "{path}" }
        }
    }
}
