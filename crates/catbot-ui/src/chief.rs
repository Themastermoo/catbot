//! Settings › Chief of Staff (stage 15.3): who the Chief is (one of the bots, or the ready-made
//! Chief Catbot makes), and the Chief's options.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const SELECT: &str = "min-w-0 flex-1 rounded-lg bg-inset px-3 py-2 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";

#[component]
pub fn ChiefOptions() -> Element {
    let state = use_context::<Signal<State>>();
    let mut found = use_signal(|| None::<Value>);
    let mut problem = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        let _ = state.read().chief_changed;
        spawn(async move {
            match api::get("/api/chief").await {
                Ok(answer) => found.set(Some(answer)),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let mut send = move |method: &'static str, path: &'static str, body: Value| {
        busy.set(true);
        spawn(async move {
            match api::call(method, path, Some(&body)).await {
                Ok(answer) => {
                    problem.set(None);
                    found.set(Some(answer));
                }
                Err(err) => problem.set(Some(format!("Couldn't change the Chief: {err}"))),
            }
            busy.set(false);
        });
    };
    let Some(current) = found() else {
        return rsx! {};
    };
    let chief_id = current["chief"]["botId"].as_str().unwrap_or_default().to_string();
    let ready = current["readyMadeName"].as_str().unwrap_or("Chief").to_string();
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone())).collect();
    rsx! {
        section { class: "flex flex-col gap-3", "data-chief-options": true,
            h2 { class: HEADING, "Who it is" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Your Chief of Staff is your main point of contact for the whole team. It knows every bot and channel, hands work to the bot best placed for it, checks what comes back, and gives you one answer. It has a star in the sidebar."
            }
            div { class: "flex items-center gap-2",
                select {
                    class: SELECT,
                    "aria-label": "Chief of Staff",
                    "data-chief-pick": true,
                    disabled: busy(),
                    onchange: move |e| {
                        let id = e.value();
                        send("PUT", "/api/chief", json!({ "botId": if id.is_empty() { Value::Null } else { json!(id) } }));
                    },
                    option { value: "", selected: chief_id.is_empty(), "No Chief of Staff" }
                    for (id, name) in bots {
                        option { key: "{id}", value: "{id}", selected: id == chief_id, "{name}" }
                    }
                }
            }
            button {
                r#type: "button",
                "data-chief-ready-made": true,
                disabled: busy(),
                class: "self-start rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50",
                onclick: move |_| send("POST", "/api/chief/ready-made", json!({})),
                "Add the ready-made {ready} as Chief"
            }
            p { class: "text-[12px] leading-relaxed text-ink-tertiary",
                "The ready-made Chief is a new bot with Chief of Staff instructions. Any bot can be made Chief instead; only one is Chief at a time."
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
