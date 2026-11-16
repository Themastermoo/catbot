//! The hand-off limits in Settings (stage 15.2): how deep a chain of bots handing work to each
//! other may go, how many bot replies one message may bring, or no limits at all.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const FIELD: &str = "w-20 rounded-lg bg-inset px-2 py-1.5 text-right text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-40";

#[component]
pub fn HandoffSettings() -> Element {
    let state = use_context::<Signal<State>>();
    let mut rules = use_signal(|| None::<Value>);
    let mut problem = use_signal(|| None::<String>);
    use_effect(move || {
        let _ = state.read().handoffs_changed;
        spawn(async move {
            match api::get("/api/handoffs").await {
                Ok(found) => rules.set(Some(found["handoffs"].clone())),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let save = use_callback(move |change: Value| {
        let Some(mut wanted) = rules.peek().clone() else { return };
        for (key, value) in change.as_object().into_iter().flatten() {
            wanted[key] = value.clone();
        }
        rules.set(Some(wanted.clone()));
        spawn(async move {
            match api::call("PUT", "/api/handoffs", Some(&wanted)).await {
                Ok(saved) => {
                    problem.set(None);
                    rules.set(Some(saved["handoffs"].clone()));
                }
                Err(err) => problem.set(Some(format!("Couldn't save: {err}"))),
            }
        });
    });
    let Some(current) = rules() else {
        return rsx! {};
    };
    let unlimited = current["unlimited"].as_bool().unwrap_or(false);
    let depth = current["depth"].as_u64().unwrap_or(3);
    let replies = current["replies"].as_u64().unwrap_or(6);
    let number = move |key: &'static str| {
        move |e: FormEvent| {
            if let Ok(n) = e.value().trim().parse::<u64>()
                && n > 0
            {
                save.call(json!({ key: n }));
            }
        }
    };
    rsx! {
        section { class: "flex flex-col gap-3", "data-handoff-settings": true,
            h2 { class: "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary", "Hand-offs" }
            p { class: "text-[13px] text-ink-secondary", "A bot that ends its reply with @Name brings that bot in. Limits stop two bots passing work back and forth for ever." }
            div { class: "flex items-center gap-3",
                span { class: "min-w-0 flex-1 text-[14px] text-ink", "Unlimited hand-offs" }
                crate::settings::Switch {
                    checked: unlimited,
                    disabled: false,
                    label: "Unlimited hand-offs".to_string(),
                    on_toggle: move |_| save.call(json!({ "unlimited": !unlimited })),
                }
            }
            if unlimited {
                p { class: "text-[12px] leading-snug text-warning", "Only Stop, or a bot reaching its spend limit, ends a chain of hand-offs." }
            }
            label { class: "flex items-center gap-3",
                span { class: if unlimited { "min-w-0 flex-1 text-[14px] text-ink-tertiary" } else { "min-w-0 flex-1 text-[14px] text-ink" }, "How many hand-offs deep" }
                input { key: "depth-{depth}", class: FIELD, r#type: "number", min: 1, max: 50, disabled: unlimited, "data-handoff-depth": true, initial_value: "{depth}", onchange: number("depth") }
            }
            label { class: "flex items-center gap-3",
                span { class: if unlimited { "min-w-0 flex-1 text-[14px] text-ink-tertiary" } else { "min-w-0 flex-1 text-[14px] text-ink" }, "Bot replies to one message" }
                input { key: "replies-{replies}", class: FIELD, r#type: "number", min: 1, max: 200, disabled: unlimited, "data-handoff-replies": true, initial_value: "{replies}", onchange: number("replies") }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
