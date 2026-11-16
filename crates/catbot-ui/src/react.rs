//! Reactions on messages: the emoji under a message, the quick row that appears on hover, and
//! the full picker behind its +. The hover row also holds "Branch from here" (`chats::branch`).
//!
//! Choosing an emoji posts it (`POST /api/threads/:thread/messages/:id/reactions`); the
//! server's `message.patch` brings the message back with it, everywhere it's open. Clicking an
//! emoji already on the message takes it off. The bot is told on its next turn.

use dioxus::prelude::*;

use crate::api;
use crate::emoji_list::ALL;

/// The quick row.
pub const QUICK: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];

/// Adds or removes an emoji on a message.
pub fn send(thread: String, message: String, emoji: String, on: bool) {
    spawn(async move {
        let body = serde_json::json!({ "emoji": emoji, "on": on });
        let _ = api::post(&format!("/api/threads/{thread}/messages/{message}/reactions"), &body).await;
    });
}

/// A message's reactions, and the hover row for adding one. `end` lines them up on the right.
#[component]
pub fn Reactions(
    thread: String,
    message: String,
    reactions: Vec<String>,
    end: bool,
    #[props(default)] bot_id: String,
    #[props(default)] working: bool,
) -> Element {
    let state = use_context::<Signal<crate::live::State>>();
    let ui = use_context::<crate::settings::Ui>();
    // Messages still on their way, or still being written, can't be reacted to yet.
    if thread.is_empty() || message.starts_with("optimistic-") || message == "live" {
        return rsx! {};
    }
    let mut picking = use_signal(|| false);
    let side = if end { "justify-end" } else { "justify-start" };
    let (thread_a, message_a, current) = (thread.clone(), message.clone(), reactions.clone());
    let pick = move |emoji: String| {
        picking.set(false);
        let on = !current.contains(&emoji);
        send(thread_a.clone(), message_a.clone(), emoji, on);
    };
    rsx! {
        div { class: "relative flex w-full flex-wrap items-center gap-1 {side}", "data-reactions": true,
            for emoji in reactions.iter().cloned() {
                {
                    let (thread, message) = (thread.clone(), message.clone());
                    rsx! {
                        button {
                            key: "{emoji}",
                            r#type: "button",
                            "data-reaction": "{emoji}",
                            title: "Take off {emoji}",
                            class: "flex h-7 items-center rounded-full border border-hairline/50 bg-card px-2 text-[15px] leading-none hover:bg-raised",
                            onclick: move |_| send(thread.clone(), message.clone(), emoji.clone(), false),
                            "{emoji}"
                        }
                    }
                }
            }
            div {
                "data-react-bar": true,
                class: "flex items-center gap-0.5 rounded-full px-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100 max-md:opacity-60",
                class: if picking() { "opacity-100" },
                for emoji in QUICK {
                    {
                        let mut pick = pick.clone();
                        rsx! {
                            button {
                                key: "{emoji}",
                                r#type: "button",
                                "data-quick-reaction": "{emoji}",
                                "aria-label": "React {emoji}",
                                class: "flex size-7 items-center justify-center rounded-full text-[15px] leading-none hover:bg-raised max-md:hidden",
                                onclick: move |_| pick(emoji.to_string()),
                                "{emoji}"
                            }
                        }
                    }
                }
                button {
                    r#type: "button",
                    "data-more-reactions": true,
                    "aria-label": "Choose a reaction",
                    title: "Choose a reaction",
                    class: "flex size-7 items-center justify-center rounded-full text-ink-tertiary hover:bg-raised hover:text-ink",
                    onclick: move |_| picking.toggle(),
                    // Lucide `SmilePlus` (ISC).
                    svg { "aria-hidden": "true", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M22 11v1a10 10 0 1 1-9-10" }
                        path { d: "M8 14s1.5 2 4 2 4-2 4-2" }
                        line { x1: "9", x2: "9.01", y1: "9", y2: "9" }
                        line { x1: "15", x2: "15.01", y1: "9", y2: "9" }
                        path { d: "M16 5h6" }
                        path { d: "M19 2v6" }
                    }
                }
                if !bot_id.is_empty() {
                    {
                        let (bot_id, thread, message) = (bot_id.clone(), thread.clone(), message.clone());
                        let label = if working { "Branch once the bot has finished" } else { "Branch from here: a new chat with everything up to this message" };
                        rsx! {
                            button {
                                r#type: "button",
                                "data-branch": true,
                                "aria-label": "Branch from here",
                                title: "{label}",
                                disabled: working,
                                class: "flex size-7 items-center justify-center rounded-full text-ink-tertiary hover:bg-raised hover:text-ink disabled:opacity-40",
                                onclick: move |_| crate::chats::branch(state, ui, bot_id.clone(), thread.clone(), message.clone()),
                                // Lucide `GitBranch` (ISC).
                                svg { "aria-hidden": "true", width: "15", height: "15", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                    line { x1: "6", x2: "6", y1: "3", y2: "15" }
                                    circle { cx: "18", cy: "6", r: "3" }
                                    circle { cx: "6", cy: "18", r: "3" }
                                    path { d: "M18 9a9 9 0 0 1-9 9" }
                                }
                            }
                        }
                    }
                }
            }
            if picking() {
                // A click outside closes it.
                div { class: "fixed inset-0 z-30", onclick: move |_| picking.set(false) }
                div { class: if end { "absolute right-0 top-full z-40 mt-1" } else { "absolute left-0 top-full z-40 mt-1" },
                    Picker { on_pick: pick.clone() }
                }
            }
        }
    }
}

/// Every emoji (`emoji_list.rs`), by group, with a search box; the quick six at the top.
#[component]
pub fn Picker(on_pick: EventHandler<String>) -> Element {
    let mut query = use_signal(String::new);
    let mut group = use_signal(|| 0u8);
    let wanted = query.read().trim().to_lowercase();
    let shown: Vec<&'static str> = if wanted.is_empty() {
        ALL.iter().filter(|e| e.2 == group()).map(|e| e.0).collect()
    } else {
        ALL.iter().filter(|e| e.1.contains(&wanted)).map(|e| e.0).take(160).collect()
    };
    rsx! {
        div {
            "data-emoji-picker": true,
            class: "flex w-[320px] max-w-[90vw] flex-col gap-2 rounded-2xl border border-hairline/40 bg-panel p-2 shadow-lg",
            onclick: move |e| e.stop_propagation(),
            input {
                r#type: "search",
                "data-emoji-search": true,
                placeholder: "Search emoji",
                autofocus: true,
                class: "h-8 w-full rounded-lg bg-raised px-2.5 text-[13px] text-ink outline-none placeholder:text-ink-tertiary",
                oninput: move |e| query.set(e.value()),
            }
            if wanted.is_empty() {
                div { class: "flex justify-between",
                    for (g, label) in GROUPS.iter().enumerate() {
                        {
                            let g = g as u8;
                            let icon = ALL.iter().find(|e| e.2 == g).map(|e| e.0).unwrap_or("·");
                            let on = group() == g;
                            rsx! {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    title: "{label}",
                                    "aria-label": "{label}",
                                    "aria-pressed": "{on}",
                                    class: if on { "flex size-8 items-center justify-center rounded-lg bg-raised text-[16px]" } else { "flex size-8 items-center justify-center rounded-lg text-[16px] opacity-60 hover:opacity-100" },
                                    onclick: move |_| group.set(g),
                                    "{icon}"
                                }
                            }
                        }
                    }
                }
            }
            div { class: "grid max-h-[240px] grid-cols-8 gap-0.5 overflow-y-auto",
                for emoji in shown {
                    button {
                        key: "{emoji}",
                        r#type: "button",
                        "data-emoji": "{emoji}",
                        class: "flex size-9 items-center justify-center rounded-lg text-[20px] leading-none hover:bg-raised",
                        onclick: move |_| on_pick.call(emoji.to_string()),
                        "{emoji}"
                    }
                }
            }
        }
    }
}

/// The emoji groups, in the list's order.
const GROUPS: [&str; 9] = ["Smileys", "People", "Animals and nature", "Food and drink", "Travel and places", "Activities", "Objects", "Symbols", "Flags"];
