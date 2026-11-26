//! Settings › Apps › ComfyUI (stage 26): where ComfyUI is, whether it answers, which bots get it,
//! and the saved workflow `/image` uses (`server/comfyui.rs`). Every run shows a card first.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const SMALL: &str = "rounded-lg bg-inset px-2 py-1.5 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const LINK: &str = "rounded-md px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-ink disabled:opacity-50";

#[component]
pub fn ComfyUiSettings() -> Element {
    let state = use_context::<Signal<State>>();
    let mut now = use_signal(|| None::<Value>);
    let mut url = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut refreshed = use_signal(|| 0u32);
    use_effect(move || {
        let _ = state.read().apps_changed;
        let _ = refreshed();
        spawn(async move {
            match api::get("/api/comfyui").await {
                Ok(found) => {
                    url.set(found["url"].as_str().unwrap_or_default().to_string());
                    now.set(Some(found));
                }
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let put = use_callback(move |body: Value| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call("PUT", "/api/comfyui", Some(&body)).await {
                Ok(answer) => now.set(Some(answer)),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    });
    let Some(c) = now() else {
        return rsx! { h2 { class: HEADING, "ComfyUI" } };
    };
    let open = c["open"] == true;
    let enabled = c["enabled"] == true;
    let ticked: Vec<String> = c["bots"].as_array().map(|x| x.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone())).collect();
    let workflows: Vec<String> = c["workflows"].as_array().map(|w| w.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let chosen = c["imageWorkflow"].as_str().unwrap_or_default().to_string();
    let prompt_at = c["imagePrompt"].as_str().unwrap_or_default().to_string();
    let prompts = c["prompts"].as_array().cloned().unwrap_or_default();
    let devices: Vec<String> = c["devices"].as_array().map(|d| d.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    rsx! {
        h2 { class: HEADING, "ComfyUI" }
        p { class: "text-[13px] leading-relaxed text-ink-secondary",
            "Bots make pictures and videos with the ComfyUI on this computer: your saved workflows (with their prompts, seeds and sizes changed) or ones they build. Every run shows you a card with what will run first; what it makes comes into the chat."
        }
        div { class: "flex flex-col gap-2 rounded-lg border border-hairline/40 px-3 py-2", "data-comfyui": true,
            div { class: "flex gap-2",
                input { class: FIELD, placeholder: "http://127.0.0.1:8188", "data-comfyui-url": true, value: "{url}", oninput: move |e| url.set(e.value()) }
                button { r#type: "button", class: BUTTON, disabled: busy(), onclick: move |_| put.call(json!({ "url": url() })), "Save" }
            }
            div { class: "flex items-center gap-2 text-[13px]",
                if open {
                    span { class: "h-2 w-2 rounded-full bg-success" }
                    span { class: "flex-1 text-ink", "ComfyUI {c[\"version\"].as_str().unwrap_or_default()} · {devices.join(\", \")}" }
                } else {
                    span { class: "h-2 w-2 rounded-full bg-ink-tertiary" }
                    span { class: "flex-1 text-ink-secondary", "Not answering now" }
                }
                button { r#type: "button", class: LINK, onclick: move |_| refreshed.set(refreshed() + 1), "Check again" }
            }
            if let Some(why) = c["problem"].as_str() {
                p { class: "text-[12px] text-ink-tertiary", "{why}" }
            }
            label { class: "flex items-center gap-2 text-[13px] text-ink",
                input { r#type: "checkbox", checked: enabled, "data-comfyui-on": true, onchange: move |_| put.call(json!({ "enabled": !enabled })) }
                "Bots may use ComfyUI"
            }
            div { class: "flex flex-wrap items-center gap-x-4 gap-y-1",
                span { class: "text-[12px] text-ink-secondary", "Bots:" }
                for (bot_id, bot_name) in bots {
                    {
                        let on = ticked.contains(&bot_id);
                        let others = ticked.clone();
                        rsx! {
                            label { key: "{bot_id}", class: "flex items-center gap-1.5 text-[13px] text-ink",
                                input { r#type: "checkbox", checked: on, "data-comfyui-bot": "{bot_name}",
                                    onchange: move |_| {
                                        let mut next: Vec<String> = others.iter().filter(|x| **x != bot_id).cloned().collect();
                                        if !on {
                                            next.push(bot_id.clone());
                                        }
                                        put.call(json!({ "bots": next }));
                                    },
                                }
                                "{bot_name}"
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-2 text-[13px] text-ink",
                span { class: "text-[12px] text-ink-secondary", "For /image:" }
                select { class: SMALL, "data-comfyui-image": true, value: "{chosen}",
                    onchange: move |e| put.call(json!({ "imageWorkflow": e.value() })),
                    option { value: "", "Not used" }
                    for w in workflows.iter() {
                        option { key: "{w}", value: "{w}", selected: *w == chosen, "{w}" }
                    }
                }
                if !chosen.is_empty() {
                    span { class: "text-[12px] text-ink-secondary", "the description goes in" }
                    select { class: SMALL, "data-comfyui-prompt": true, value: "{prompt_at}",
                        onchange: move |e| put.call(json!({ "imagePrompt": e.value() })),
                        option { value: "", "the first prompt (not a negative one)" }
                        for p in prompts.iter() {
                            {
                                let at = p["set"].as_str().unwrap_or_default().to_string();
                                let label = format!("{} › {}", p["node"].as_str().unwrap_or_default(), at);
                                rsx! { option { key: "{at}", value: "{at}", selected: at == prompt_at, "{label}" } }
                            }
                        }
                    }
                }
            }
            if let Some(why) = c["promptProblem"].as_str() {
                p { class: "text-[12px] text-warning", "{why}" }
            }
            if open && workflows.is_empty() {
                p { class: "text-[12px] text-ink-tertiary", "No saved workflows yet: save one in ComfyUI (Workflow › Save) and it shows here." }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
