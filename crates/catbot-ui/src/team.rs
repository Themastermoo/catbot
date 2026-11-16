//! A team change the Chief of Staff proposed (`server/team.rs`): exactly what would change, and
//! Approve or Decline. Nothing changes until the person approves.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub title: String,
    pub lines: Vec<String>,
    pub warning: Option<String>,
    pub asked_by: String,
    /// What the card is about, when it isn't a team change (a skill a bot offers).
    pub heading: Option<String>,
    pub state: String,
    pub error: Option<String>,
}

impl Change {
    pub fn from_wire(wire: &Value) -> Change {
        Change {
            title: wire["title"].as_str().unwrap_or_default().to_string(),
            lines: wire["lines"].as_array().map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
            warning: wire["warning"].as_str().map(str::to_string),
            asked_by: wire["askedBy"].as_str().unwrap_or("The Chief").to_string(),
            heading: wire["heading"].as_str().map(str::to_string),
            state: wire["state"].as_str().unwrap_or("asking").to_string(),
            error: wire["error"].as_str().map(str::to_string),
        }
    }
}

#[component]
pub fn TeamChangeCard(thread: String, message: String, change: Change) -> Element {
    let mut problem = use_signal(|| None::<String>);
    let mut sending = use_signal(|| false);
    let send = use_callback(move |approve: bool| {
        let path = format!("/api/threads/{thread}/team/{message}");
        sending.set(true);
        spawn(async move {
            if let Err(err) = api::post(&path, &json!({ "approve": approve })).await {
                problem.set(Some(err));
            }
            sending.set(false);
        });
    });
    let asking = change.state == "asking";
    rsx! {
        div { class: "flex w-full justify-start", "data-team-change": "{change.state}",
            div { class: "flex w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] flex-col gap-2 rounded-xl border border-hairline/40 bg-panel px-3 py-2.5 text-[13px]",
                div { class: "flex items-start gap-2",
                    // Lucide `Users` (ISC).
                    svg { "aria-hidden": "true", class: "mt-0.5 shrink-0 text-ink-secondary", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" }
                        circle { cx: "9", cy: "7", r: "4" }
                        path { d: "M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75" }
                    }
                    div { class: "min-w-0 flex-1",
                        p { class: "text-ink-secondary",
                            match &change.heading {
                                Some(heading) => heading.clone(),
                                None => format!("{} wants to change the team", change.asked_by),
                            }
                        }
                        p { class: "font-semibold text-ink", "{change.title}" }
                    }
                }
                if !change.lines.is_empty() {
                    ul { class: "flex flex-col gap-0.5 pl-6 text-ink",
                        for line in change.lines.iter() {
                            li { class: "list-disc break-words", "{line}" }
                        }
                    }
                }
                if let Some(warning) = &change.warning {
                    p { class: "pl-6 text-warning", "{warning}" }
                }
                match change.state.as_str() {
                    "done" => rsx! { p { class: "text-ink-tertiary", "Approved and done." } },
                    "declined" => rsx! { p { class: "text-ink-tertiary", "Declined. Nothing changed." } },
                    _ => rsx! {},
                }
                if let Some(err) = &change.error {
                    p { role: "alert", class: "text-danger", "Approved, but it didn't work: {err}" }
                }
                if asking {
                    div { class: "flex gap-1.5 pl-6",
                        button {
                            r#type: "button",
                            "data-team-approve": true,
                            disabled: sending(),
                            class: "rounded-lg bg-accent px-3 py-1.5 font-semibold text-white disabled:opacity-50",
                            onclick: move |_| send.call(true),
                            "Approve"
                        }
                        button {
                            r#type: "button",
                            "data-team-decline": true,
                            disabled: sending(),
                            class: "rounded-lg px-3 py-1.5 text-ink-secondary hover:bg-raised disabled:opacity-50",
                            onclick: move |_| send.call(false),
                            "Decline"
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
