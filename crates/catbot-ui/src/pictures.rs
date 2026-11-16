//! The picture card (`server/images.rs`): which AI should draw it, a Blender script to look at
//! before it runs, and how the drawing is going. The picture itself comes as the bot's next
//! message, like an attachment.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;

/// A picture card's state, from the wire.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub prompt: String,
    pub asked_by: String,
    pub state: String,
    pub options: Vec<(String, String)>,
    pub choice_label: String,
    pub script: Option<String>,
    pub error: Option<String>,
}

impl Card {
    pub fn from_wire(wire: &Value) -> Card {
        Card {
            prompt: wire["prompt"].as_str().unwrap_or_default().to_string(),
            asked_by: wire["askedBy"].as_str().unwrap_or("you").to_string(),
            state: wire["state"].as_str().unwrap_or("choosing").to_string(),
            options: wire["options"]
                .as_array()
                .map(|o| o.iter().filter_map(|o| Some((o["id"].as_str()?.to_string(), o["label"].as_str()?.to_string()))).collect())
                .unwrap_or_default(),
            choice_label: wire["choiceLabel"].as_str().unwrap_or_default().to_string(),
            script: wire["script"].as_str().map(str::to_string),
            error: wire["error"].as_str().map(str::to_string),
        }
    }
}

/// Sends the card an answer: a choice, a go-ahead for the script, or no.
fn answer(thread: String, message: String, body: Value, mut problem: Signal<Option<String>>) {
    spawn(async move {
        if let Err(err) = api::post(&format!("/api/threads/{thread}/images/{message}"), &body).await {
            problem.set(Some(err));
        }
    });
}

#[component]
pub fn PictureCard(thread: String, message: String, card: Card) -> Element {
    let problem = use_signal(|| None::<String>);
    let mut showing_script = use_signal(|| true);
    let who = if card.asked_by == "you" { "You asked for a picture".to_string() } else { format!("{} wants to make a picture", card.asked_by) };
    let choosing = card.state == "choosing" || card.state == "failed";
    rsx! {
        div { class: "flex w-full justify-start", "data-picture-card": "{card.state}",
            div { class: "flex w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] flex-col gap-2 rounded-xl border border-hairline/40 bg-panel px-3 py-2.5 text-[13px]",
                div { class: "flex items-start gap-2",
                    // Lucide `Image` (ISC).
                    svg { "aria-hidden": "true", class: "mt-0.5 shrink-0 text-ink-secondary", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        rect { width: "18", height: "18", x: "3", y: "3", rx: "2", ry: "2" }
                        circle { cx: "9", cy: "9", r: "2" }
                        path { d: "m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" }
                    }
                    div { class: "min-w-0 flex-1",
                        p { class: "text-ink-secondary", "{who}" }
                        p { class: "text-ink", "“{card.prompt}”" }
                    }
                }
                match card.state.as_str() {
                    "writing" => rsx! { p { class: "text-ink-secondary", "Claude is writing the Blender script…" } },
                    "drawing" => rsx! { p { class: "text-ink-secondary", "data-picture-drawing": true, "Drawing with {card.choice_label}…" } },
                    "done" => rsx! { p { class: "text-ink-tertiary", "Drawn by {card.choice_label}." } },
                    "cancelled" => rsx! { p { class: "text-ink-tertiary", "Not drawn." } },
                    _ => rsx! {},
                }
                if let Some(err) = &card.error {
                    p { role: "alert", class: "whitespace-pre-wrap text-danger", "It didn't work: {err}" }
                }
                if choosing {
                    p { class: "text-ink-secondary", if card.state == "failed" { "Try another AI?" } else { "Which AI should draw it?" } }
                    div { class: "flex flex-wrap gap-1.5",
                        for (id, label) in card.options.iter().cloned() {
                            {
                                let (thread, message) = (thread.clone(), message.clone());
                                rsx! {
                                    button {
                                        key: "{id}",
                                        r#type: "button",
                                        "data-picture-choice": "{id}",
                                        class: "rounded-lg bg-raised px-2.5 py-1.5 text-ink hover:bg-control",
                                        onclick: move |_| answer(thread.clone(), message.clone(), json!({ "choice": id }), problem),
                                        "{label}"
                                    }
                                }
                            }
                        }
                        {
                            let (thread, message) = (thread.clone(), message.clone());
                            rsx! {
                                button { r#type: "button", class: "rounded-lg px-2.5 py-1.5 text-ink-secondary hover:bg-raised",
                                    onclick: move |_| answer(thread.clone(), message.clone(), json!({ "cancel": true }), problem), "Don't draw it" }
                            }
                        }
                    }
                }
                if card.state == "confirming" {
                    if let Some(script) = &card.script {
                        p { class: "text-ink-secondary", "Claude wrote this Blender script. It runs on this computer, so look it over before running it." }
                        button { r#type: "button", class: "self-start text-accent", onclick: move |_| showing_script.toggle(),
                            if showing_script() { "Hide the script" } else { "Show the script" } }
                        if showing_script() {
                            pre { "data-picture-script": true, class: "max-h-[280px] overflow-auto rounded-lg bg-raised p-2 font-mono text-[12px] leading-[18px] text-ink", "{script}" }
                        }
                        div { class: "flex gap-1.5",
                            {
                                let (thread, message) = (thread.clone(), message.clone());
                                rsx! {
                                    button { r#type: "button", "data-picture-run": true, class: "rounded-lg bg-accent px-2.5 py-1.5 font-semibold text-white",
                                        onclick: move |_| answer(thread.clone(), message.clone(), json!({ "run": true }), problem), "Run in Blender" }
                                }
                            }
                            {
                                let (thread, message) = (thread.clone(), message.clone());
                                rsx! {
                                    button { r#type: "button", class: "rounded-lg px-2.5 py-1.5 text-ink-secondary hover:bg-raised",
                                        onclick: move |_| answer(thread.clone(), message.clone(), json!({ "cancel": true }), problem), "Don't run it" }
                                }
                            }
                        }
                    }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "text-danger", "{err}" }
                }
            }
        }
    }
}
