//! The sidebar: a search field and Catbot's bots, one row each.
//!
//! Classes and sizes follow the full layout of OpenMausBot's `src/components/Sidebar.tsx`
//! (`Sidebar` and `BotListItem`, Apache-2.0, see NOTICE). Their bot picture is their mascot
//! artwork, which Catbot doesn't copy: a bot wears one of Catbot's own (`mascots.rs`).

use dioxus::prelude::*;

use crate::bots::{Activity, Bot};
use crate::chat::clock;
use crate::mascots::BotMark;

/// Their focus ring for rows (`SIMPLE_ROW_FOCUS`).
const ROW_FOCUS: &str = "focus-visible:outline-2 focus-visible:-outline-offset-2! focus-visible:outline-focus";
/// Their raised-control look for the search field (`ELEVATED_CONTROL`).
const ELEVATED_CONTROL: &str = "bg-composer ring-1 ring-inset ring-ink/[0.07]";

#[component]
pub fn Sidebar(bots: ReadSignal<Vec<Bot>>, selected: Signal<Option<String>>) -> Element {
    let mut query = use_signal(String::new);
    let mut making = use_signal(|| false);
    let shown: Vec<Bot> = bots.read().iter().filter(|b| b.matches(&query.read())).cloned().collect();
    let empty = bots.read().is_empty();
    rsx! {
        aside {
            "aria-label": "Bots",
            "data-sidebar": true,
            class: "relative flex h-full w-[320px] shrink-0 flex-col border-r-[0.5px] border-edge bg-panel",
            div { "data-sidebar-top-row": true, class: "relative flex h-12 items-center pl-4 pr-2",
                span { class: "min-w-0 flex-1 text-[15px] font-semibold text-ink", "Catbot" }
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
                        BotRow { key: "{bot.id}", selected: selected.read().as_deref() == Some(bot.id.as_str()), bot, on_select: move |id| selected.set(Some(id)) }
                    }
                }
            }
        }
    }
}

#[component]
fn BotRow(bot: Bot, selected: bool, on_select: EventHandler<String>) -> Element {
    let id = bot.id.clone();
    let key_id = bot.id.clone();
    let waiting = bot.activity == Activity::WaitingOnYou;
    let working = bot.activity == Activity::Working;
    let mood = if waiting { "waiting" } else if working { "working" } else { "idle" }.to_string();
    let row_state = if selected { "bg-raised" } else { "hover:bg-raised/50" };
    let preview_color = if waiting { "font-semibold text-warning" } else { "text-ink-secondary" };
    let time = bot.last.as_ref().filter(|_| selected).map(|last| clock(last.at));
    let preview = bot.preview();
    rsx! {
        div { class: "group relative",
            div {
                role: "button",
                tabindex: 0,
                "aria-current": if selected { "page" },
                "data-sidebar-bot-row": "{bot.id}",
                class: "flex w-full items-center gap-3 rounded-xl py-2 pl-2.5 pr-9 text-left transition-colors duration-150 max-md:rounded-2xl max-md:py-2.5 {ROW_FOCUS} {row_state}",
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
                    div { class: "flex items-center justify-between gap-2",
                        span { class: "flex min-w-0 items-center gap-1.5 truncate text-[13px] leading-[18px] {preview_color}",
                            span { dir: "auto", class: "truncate", "{preview}" }
                        }
                        if bot.unread {
                            span { class: "size-2 shrink-0 rounded-full bg-accent", "aria-label": "Unread" }
                        }
                    }
                }
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
