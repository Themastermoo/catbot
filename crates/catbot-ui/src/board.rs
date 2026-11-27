//! The task board (stage 30.3), in place of the chat: work given to bots, each done in its own
//! chat (`server/board.rs`). New items at the top; then what's going, what waits, and what's
//! finished.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::Bot;
use crate::live::State;
use crate::mascots::BotMark;
use crate::settings::Ui;

/// The states in the order the board shows them, with their headings.
const SECTIONS: [(&str, &str); 7] = [
    ("asking", "Asking you"),
    ("working", "Working"),
    ("ready", "Up next"),
    ("waiting", "Waiting for other items"),
    ("paused", "Paused"),
    ("failed", "Couldn't be done"),
    ("done", "Done"),
];

const FIELD: &str = "mt-1 w-full rounded-lg bg-inset px-3 py-2 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const LABEL: &str = "block text-[12px] font-medium text-ink-secondary";
const SMALL_BUTTON: &str = "rounded-md px-2 py-1 text-[12px] text-ink-secondary hover:bg-control hover:text-ink disabled:opacity-40";

/// At most `lines` lines, then an ellipsis.
fn clamp(lines: u32) -> String {
    format!("display:-webkit-box;-webkit-line-clamp:{lines};-webkit-box-orient:vertical")
}

fn bots(state: &State) -> Vec<Bot> {
    state.bots.iter().filter(|b| b.channel.is_none()).cloned().collect()
}

#[component]
pub fn Board() -> Element {
    let state = use_context::<Signal<State>>();
    let mut items = use_signal(Vec::<Value>::new);
    let mut problem = use_signal(|| None::<String>);
    let mut adding = use_signal(|| false);
    use_effect(move || {
        let _ = state.read().board_changed;
        spawn(async move {
            match api::get("/api/board").await {
                Ok(found) => {
                    items.set(found["items"].as_array().cloned().unwrap_or_default());
                    problem.set(None);
                }
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let all = items.read().clone();
    let open = all.iter().filter(|i| !matches!(i["state"].as_str(), Some("done" | "failed" | "cancelled"))).count();
    rsx! {
        main { "data-board": true, class: "relative flex h-full min-w-0 flex-1 flex-col bg-app",
            div { class: "flex h-[60px] shrink-0 items-center gap-3 border-b-[0.5px] border-edge px-5",
                span { class: "min-w-0 flex-1 truncate text-[15px] font-semibold text-ink", "Task board" }
                span { class: "text-[12px] tabular-nums text-ink-tertiary", "{open} open" }
                button { r#type: "button", "data-board-new": true, class: "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white", onclick: move |_| adding.toggle(),
                    if adding() { "Close" } else { "New item" }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto px-5 py-4",
                div { class: "mx-auto flex w-full max-w-[760px] flex-col gap-5",
                    if let Some(err) = problem() {
                        div { role: "alert", class: "rounded-xl border border-danger/30 bg-danger/10 px-3.5 py-2.5 text-[13px] text-danger", "{err}" }
                    }
                    if adding() {
                        NewItem { items: all.clone(), on_done: move |_| adding.set(false) }
                    }
                    if all.is_empty() && !adding() {
                        div { class: "rounded-xl bg-card p-6 text-center text-[13px] leading-relaxed text-ink-secondary",
                            p { "Give a bot a piece of work. It starts as soon as that bot is free, in a chat of its own, and its answer is what it came to." }
                            p { class: "mt-1", "An item can wait for others; your Chief of Staff can add items too." }
                        }
                    }
                    for (state_key, heading) in SECTIONS {
                        {
                            let here: Vec<Value> = all.iter().filter(|i| i["state"] == state_key).cloned().collect();
                            rsx! {
                                if !here.is_empty() {
                                    section { key: "{state_key}", "data-board-section": "{state_key}",
                                        h2 { class: "mb-2 text-[11px] font-semibold uppercase tracking-wide text-ink-tertiary", "{heading} · {here.len()}" }
                                        div { class: "flex flex-col gap-2",
                                            for item in here {
                                                ItemCard { key: "{item[\"id\"].as_str().unwrap_or_default()}", item: item.clone(), all: all.clone() }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    {
                        let cancelled = all.iter().filter(|i| i["state"] == "cancelled").count();
                        rsx! {
                            if cancelled > 0 {
                                p { class: "text-[12px] text-ink-tertiary", "{cancelled} cancelled (kept for their chats; delete them from their cards)" }
                            }
                        }
                    }
                    for item in all.iter().filter(|i| i["state"] == "cancelled").cloned() {
                        ItemCard { key: "{item[\"id\"].as_str().unwrap_or_default()}", item: item.clone(), all: all.clone() }
                    }
                }
            }
        }
    }
}

/// One item: what it is, whose, how it stands, and what can be done with it.
#[component]
fn ItemCard(item: Value, all: Vec<Value>) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let id = item["id"].as_str().unwrap_or_default().to_string();
    let title = item["title"].as_str().unwrap_or_default().to_string();
    let text = item["text"].as_str().unwrap_or_default().to_string();
    let now = item["state"].as_str().unwrap_or_default().to_string();
    let result = item["result"].as_str().map(str::to_string);
    let thread = item["thread"].as_str().map(str::to_string);
    let bot_id = item["botId"].as_str().unwrap_or_default().to_string();
    let by = item["by"].as_str().filter(|b| *b != "person").map(str::to_string);
    let bot = state.read().bots.iter().find(|b| b.id == bot_id).cloned();
    let after: Vec<String> = item["after"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|a| all.iter().find(|i| i["id"] == a))
        .map(|i| format!("{}{}", i["title"].as_str().unwrap_or_default(), if i["state"] == "done" { " ✓" } else { "" }))
        .collect();
    let act = move |change: Option<Value>| {
        let id = id.clone();
        spawn(async move {
            busy.set(true);
            let outcome = match change {
                Some(change) => api::call("PATCH", &format!("/api/board/{id}"), Some(&change)).await,
                None => api::call("DELETE", &format!("/api/board/{id}"), None).await,
            };
            busy.set(false);
            problem.set(outcome.err());
        });
    };
    let (dot, label) = match now.as_str() {
        "asking" => ("bg-warning", "Asking you"),
        "working" => ("bg-accent animate-pulse", "Working"),
        "ready" => ("bg-accent", "Up next"),
        "waiting" => ("bg-ink-tertiary", "Waiting"),
        "paused" => ("bg-ink-tertiary", "Paused"),
        "failed" => ("bg-danger", "Couldn't be done"),
        "done" => ("bg-success", "Done"),
        _ => ("bg-ink-tertiary", "Cancelled"),
    };
    let finished = matches!(now.as_str(), "done" | "failed" | "cancelled");
    let open_chat = thread.clone().zip(bot.as_ref().map(|b| b.id.clone()));
    rsx! {
        div { "data-board-item": "{title}", "data-board-state": "{now}", class: "rounded-xl bg-card p-4",
            div { class: "flex items-start gap-2.5",
                if let Some(b) = &bot {
                    BotMark { picture: b.picture.clone(), color: b.color.clone(), initial: b.initial(), size: 28, mark: b.mark.clone(), state: if now == "working" { "working" } else { "idle" } }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-1.5",
                        span { class: "text-[14px] font-semibold text-ink", "{title}" }
                        span { class: "inline-flex items-center gap-1.5 text-[12px] text-ink-tertiary",
                            span { "aria-hidden": "true", class: "size-1.5 shrink-0 rounded-full {dot}" }
                            "{label}"
                        }
                    }
                    div { class: "text-[12px] text-ink-tertiary",
                        match &bot {
                            Some(b) => rsx! { span { class: "bot-identity font-medium", "data-bot-color": "{b.color}", "{b.name}" } },
                            None => rsx! { span { "a bot that's gone" } },
                        }
                        if let Some(by) = &by { " · put on by {by}" }
                        if !after.is_empty() { " · after {after.join(\", \")}" }
                    }
                    if !text.is_empty() {
                        p { class: "mt-1.5 overflow-hidden whitespace-pre-wrap text-[13px] leading-relaxed text-ink-secondary", style: "{clamp(3)}", "{text}" }
                    }
                    if let Some(result) = &result {
                        div { class: "mt-2 rounded-lg bg-inset px-3 py-2 text-[13px] leading-relaxed", "data-board-result": true,
                            span { class: "text-[11px] font-semibold uppercase tracking-wide text-ink-tertiary",
                                if now == "asking" { "Its question" } else if now == "failed" { "Why" } else { "What it came to" }
                            }
                            p { class: "mt-0.5 overflow-hidden whitespace-pre-wrap text-ink", style: "{clamp(6)}", "{result}" }
                        }
                    }
                    div { class: "mt-2 flex flex-wrap items-center gap-1",
                        if let Some((thread, owner)) = open_chat {
                            button { r#type: "button", "data-board-open": true, class: SMALL_BUTTON,
                                onclick: move |_| {
                                    let mut board = ui.board;
                                    board.set(false);
                                    crate::chats::open(state, ui, &owner, &thread);
                                },
                                if now == "asking" { "Answer in its chat" } else { "Open its chat" }
                            }
                        }
                        if matches!(now.as_str(), "ready" | "waiting" | "working" | "asking") {
                            button { r#type: "button", "data-board-pause": true, disabled: busy(), class: SMALL_BUTTON, onclick: {
                                let act = act.clone();
                                move |_| act(Some(json!({ "state": "paused" })))
                            }, "Pause" }
                        }
                        if now == "paused" {
                            button { r#type: "button", "data-board-go": true, disabled: busy(), class: SMALL_BUTTON, onclick: {
                                let act = act.clone();
                                move |_| act(Some(json!({ "state": "ready" })))
                            }, "Go on" }
                        }
                        if matches!(now.as_str(), "failed" | "cancelled") {
                            button { r#type: "button", "data-board-retry": true, disabled: busy(), class: SMALL_BUTTON, onclick: {
                                let act = act.clone();
                                move |_| act(Some(json!({ "state": "ready" })))
                            }, "Try again" }
                        }
                        if !finished {
                            button { r#type: "button", "data-board-cancel": true, disabled: busy(), class: SMALL_BUTTON, onclick: {
                                let act = act.clone();
                                move |_| act(Some(json!({ "state": "cancelled" })))
                            }, "Cancel" }
                        }
                        if finished {
                            button { r#type: "button", "data-board-delete": true, disabled: busy(), class: "{SMALL_BUTTON} hover:text-danger", onclick: {
                                let act = act.clone();
                                move |_| act(None)
                            }, "Delete" }
                        }
                    }
                    if let Some(err) = problem() {
                        p { role: "alert", class: "mt-1 text-[12px] text-danger", "{err}" }
                    }
                }
            }
        }
    }
}

/// A new item: what it is, which bot, and the items it waits for.
#[component]
fn NewItem(items: Vec<Value>, on_done: EventHandler<()>) -> Element {
    let state = use_context::<Signal<State>>();
    let team = bots(&state.read());
    let mut title = use_signal(String::new);
    let mut text = use_signal(String::new);
    let mut bot = use_signal(|| team.first().map(|b| b.id.clone()).unwrap_or_default());
    let mut after = use_signal(Vec::<String>::new);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let open: Vec<Value> = items.iter().filter(|i| !matches!(i["state"].as_str(), Some("failed" | "cancelled"))).cloned().collect();
    let add = move |_| {
        spawn(async move {
            busy.set(true);
            let body = json!({ "title": title(), "text": text(), "botId": bot(), "after": after() });
            match api::call("POST", "/api/board", Some(&body)).await {
                Ok(_) => on_done.call(()),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    rsx! {
        div { "data-board-form": true, class: "rounded-xl bg-card p-4",
            label { class: LABEL, "What"
                input { class: FIELD, placeholder: "A short title", value: "{title}", oninput: move |e| title.set(e.value()) }
            }
            label { class: "mt-3 {LABEL}", "The work"
                textarea { class: "{FIELD} resize-y text-[13px] leading-relaxed", rows: 4, placeholder: "What to do, and what done looks like", value: "{text}", oninput: move |e| text.set(e.value()) }
            }
            label { class: "mt-3 {LABEL}", "For"
                select { "data-board-bot": true, class: FIELD, onchange: move |e: FormEvent| bot.set(e.value()),
                    for b in team.iter().cloned() {
                        option { key: "{b.id}", value: "{b.id}", selected: b.id == bot(), "{b.name}" }
                    }
                }
            }
            if !open.is_empty() {
                div { class: "mt-3 {LABEL}", "After (optional): starts once these are done" }
                div { class: "mt-1 flex flex-wrap gap-1.5",
                    for i in open {
                        {
                            let id = i["id"].as_str().unwrap_or_default().to_string();
                            let on = after.read().contains(&id);
                            let tone = if on { "bg-accent text-white" } else { "bg-inset text-ink-secondary hover:text-ink" };
                            rsx! {
                                button { key: "{id}", r#type: "button", "aria-pressed": "{on}", class: "rounded-full px-2.5 py-1 text-[12px] {tone}",
                                    onclick: move |_| {
                                        let mut list = after.write();
                                        if let Some(at) = list.iter().position(|a| *a == id) { list.remove(at); } else { list.push(id.clone()); }
                                    },
                                    "{i[\"title\"].as_str().unwrap_or_default()}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
            }
            div { class: "mt-3 flex justify-end",
                button { r#type: "button", "data-board-add": true, disabled: busy() || title.read().trim().is_empty() || bot.read().is_empty(),
                    class: "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-40", onclick: add,
                    "Put it on the board"
                }
            }
        }
    }
}
