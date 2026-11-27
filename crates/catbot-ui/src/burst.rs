//! A burst's card (stage 32.16, after OpenMausBot 0.1.153's burst cards): the job, its parts and
//! where each stands. A big burst waits here with Run, Edit and Cancel; while it runs, Cancel; once
//! it's over, Retry for parts that failed (Continue after a restart). Each part opens in its chat.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;
use crate::settings::Ui;

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub seq: usize,
    pub title: String,
    pub item: String,
    /// "queued", "running", "done", "failed" or "cancelled".
    pub state: String,
    pub thread: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Burst {
    pub task: String,
    pub bot_id: String,
    pub bot_name: String,
    pub ai: String,
    /// "proposed", "running", "merging", "finished", "cancelled" or "interrupted".
    pub state: String,
    pub note: Option<String>,
    pub parallel: usize,
    pub estimate_minutes: usize,
    pub parts: Vec<Part>,
}

impl Burst {
    pub fn from_wire(wire: &Value) -> Burst {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        Burst {
            task: text(&wire["task"]),
            bot_id: text(&wire["botId"]),
            bot_name: wire["botName"].as_str().unwrap_or("The bot").to_string(),
            ai: text(&wire["ai"]),
            state: wire["state"].as_str().unwrap_or("proposed").to_string(),
            note: wire["note"].as_str().filter(|n| !n.is_empty()).map(str::to_string),
            parallel: wire["parallel"].as_u64().unwrap_or(3) as usize,
            estimate_minutes: wire["estimateMinutes"].as_u64().unwrap_or(3) as usize,
            parts: wire["parts"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .map(|p| Part {
                            seq: p["seq"].as_u64().unwrap_or(0) as usize,
                            title: text(&p["title"]),
                            item: text(&p["item"]),
                            state: p["state"].as_str().unwrap_or("queued").to_string(),
                            thread: p["thread"].as_str().map(str::to_string),
                            error: p["error"].as_str().map(str::to_string),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn count(&self, state: &str) -> usize {
        self.parts.iter().filter(|p| p.state == state).count()
    }

    /// The line under the task: how far it's got.
    pub fn progress(&self) -> String {
        let total = self.parts.len();
        let (done, failed, running) = (self.count("done"), self.count("failed"), self.count("running"));
        match self.state.as_str() {
            "proposed" => format!("{total} parts, {} at a time on {} · about {} min", self.parallel.min(total), self.ai, self.estimate_minutes),
            "running" => format!("{done} of {total} done{} · {running} running", if failed > 0 { format!(", {failed} failed") } else { String::new() }),
            "merging" => format!("All {total} in: {} is writing the merged result", self.bot_name),
            "finished" => format!("{done} of {total} done{}", if failed > 0 { format!(", {failed} failed") } else { String::new() }),
            "cancelled" => format!("Cancelled: {done} of {total} done"),
            "interrupted" => format!("Cut off: {done} of {total} done"),
            other => other.to_string(),
        }
    }
}

#[component]
pub fn BurstCard(thread: String, message: String, burst: Burst) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let mut problem = use_signal(|| None::<String>);
    let mut sending = use_signal(|| false);
    let mut editing = use_signal(|| None::<String>);
    let path = format!("/api/threads/{thread}/burst/{message}");
    let act = use_callback(move |body: Value| {
        let path = path.clone();
        sending.set(true);
        problem.set(None);
        spawn(async move {
            match api::post(&path, &body).await {
                Ok(_) => editing.set(None),
                Err(err) => problem.set(Some(err)),
            }
            sending.set(false);
        });
    });
    let items_text = burst.parts.iter().map(|p| p.item.replace('\n', " ")).collect::<Vec<_>>().join("\n");
    let failed = burst.count("failed") > 0;
    let progress = burst.progress();
    rsx! {
        div { class: "flex w-full justify-start", "data-burst": "{burst.state}",
            div { class: "flex w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] flex-col gap-2 rounded-xl border border-hairline/40 bg-panel px-3 py-2.5 text-[13px]",
                div { class: "flex items-start gap-2",
                    // Lucide `Zap` (ISC).
                    svg { "aria-hidden": "true", class: "mt-0.5 shrink-0 text-ink-secondary", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z" }
                    }
                    div { class: "min-w-0 flex-1",
                        p { class: "text-ink-secondary", "{burst.bot_name}'s burst" }
                        p { class: "font-semibold text-ink", "{burst.task}" }
                        p { class: "text-[12px] text-ink-tertiary", "data-burst-progress": true, "{progress}" }
                    }
                }
                if let Some(draft) = editing() {
                    div { class: "flex flex-col gap-1.5 pl-6",
                        p { class: "text-[12px] text-ink-secondary", "One part a line:" }
                        textarea {
                            "data-burst-items": true,
                            rows: "{burst.parts.len().clamp(3, 12)}",
                            class: "w-full rounded-lg bg-inset px-2 py-1.5 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent",
                            value: "{draft}",
                            oninput: move |e| editing.set(Some(e.value())),
                        }
                    }
                } else {
                    ul { class: "flex flex-col gap-0.5 pl-6",
                        for part in burst.parts.iter().cloned() {
                            {
                                let (mark, look) = match part.state.as_str() {
                                    "done" => ("✓", "text-success"),
                                    "failed" => ("✗", "text-danger"),
                                    "running" => ("●", "text-accent animate-pulse"),
                                    "cancelled" => ("–", "text-ink-tertiary"),
                                    _ => ("○", "text-ink-tertiary"),
                                };
                                let open = part.thread.clone();
                                let bot = burst.bot_id.clone();
                                rsx! {
                                    li { key: "{part.seq}", class: "flex min-w-0 items-baseline gap-2", "data-burst-part": "{part.state}",
                                        span { class: "w-3 shrink-0 text-center {look}", "{mark}" }
                                        span { class: "min-w-0 flex-1 truncate text-ink", title: "{part.item}", "{part.title}" }
                                        if let Some(why) = &part.error {
                                            span { class: "min-w-0 max-w-[45%] truncate text-[12px] text-danger", title: "{why}", "{why}" }
                                        }
                                        if let Some(thread) = open {
                                            button { r#type: "button", "data-burst-open": true, class: "shrink-0 rounded px-1.5 text-[12px] text-ink-secondary hover:bg-raised hover:text-ink",
                                                onclick: move |_| crate::chats::open(state, ui, &bot, &thread), "Open" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(note) = &burst.note {
                    p { class: "pl-6 text-[12px] text-ink-secondary", "{note}" }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "pl-6 text-danger", "{err}" }
                }
                div { class: "flex flex-wrap gap-1.5 pl-6",
                    match burst.state.as_str() {
                        "proposed" => rsx! {
                            button { r#type: "button", "data-burst-run": true, disabled: sending(), class: "rounded-lg bg-accent px-3 py-1.5 font-semibold text-white disabled:opacity-50",
                                onclick: move |_| {
                                    let mut body = json!({ "action": "run" });
                                    if let Some(draft) = editing() {
                                        body["items"] = json!(draft.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>());
                                    }
                                    act.call(body);
                                },
                                "Run"
                            }
                            if editing().is_none() {
                                button { r#type: "button", "data-burst-edit": true, disabled: sending(), class: "rounded-lg px-3 py-1.5 text-ink-secondary hover:bg-raised disabled:opacity-50",
                                    onclick: move |_| editing.set(Some(items_text.clone())), "Edit" }
                            }
                            button { r#type: "button", "data-burst-cancel": true, disabled: sending(), class: "rounded-lg px-3 py-1.5 text-ink-secondary hover:bg-raised disabled:opacity-50",
                                onclick: move |_| act.call(json!({ "action": "cancel" })), "Cancel" }
                        },
                        "running" => rsx! {
                            button { r#type: "button", "data-burst-cancel": true, disabled: sending(), class: "rounded-lg px-3 py-1.5 text-ink-secondary ring-1 ring-inset ring-hairline hover:bg-raised disabled:opacity-50",
                                onclick: move |_| act.call(json!({ "action": "cancel" })), "Cancel" }
                        },
                        "interrupted" => rsx! {
                            button { r#type: "button", "data-burst-retry": true, disabled: sending(), class: "rounded-lg bg-accent px-3 py-1.5 font-semibold text-white disabled:opacity-50",
                                onclick: move |_| act.call(json!({ "action": "retry" })), "Continue" }
                            button { r#type: "button", "data-burst-cancel": true, disabled: sending(), class: "rounded-lg px-3 py-1.5 text-ink-secondary hover:bg-raised disabled:opacity-50",
                                onclick: move |_| act.call(json!({ "action": "cancel" })), "Cancel" }
                        },
                        "finished" if failed => rsx! {
                            button { r#type: "button", "data-burst-retry": true, disabled: sending(), class: "rounded-lg px-3 py-1.5 text-ink-secondary ring-1 ring-inset ring-hairline hover:bg-raised disabled:opacity-50",
                                onclick: move |_| act.call(json!({ "action": "retry" })), "Retry failed" }
                        },
                        _ => rsx! {},
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_card_reads_its_parts_and_says_how_far_it_got() {
        let wire = json!({ "task": "Check the beds", "botName": "Scout", "ai": "Claude", "state": "running", "parallel": 3, "estimateMinutes": 6,
            "parts": [{ "seq": 0, "title": "North", "item": "North", "state": "done", "thread": "t1" }, { "seq": 1, "title": "South", "item": "South", "state": "failed", "error": "no" }, { "seq": 2, "title": "East", "item": "East", "state": "running" }, { "seq": 3, "title": "West", "item": "West", "state": "queued" }] });
        let burst = Burst::from_wire(&wire);
        assert_eq!(burst.parts[0].thread.as_deref(), Some("t1"));
        assert_eq!(burst.progress(), "1 of 4 done, 1 failed · 1 running");
        let mut proposed = wire.clone();
        proposed["state"] = json!("proposed");
        assert_eq!(Burst::from_wire(&proposed).progress(), "4 parts, 3 at a time on Claude · about 6 min");
    }
}
