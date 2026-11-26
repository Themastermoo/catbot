//! Settings › Apps › Blender (stage 25): installs Catbot's add-on into Blender, says whether
//! Blender is open with it on, and which bots get Blender (`server/blender.rs`). Looking runs at
//! once; changes, scripts, renders and saves show a card first.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const LINK: &str = "rounded-md px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-ink disabled:opacity-50";

#[component]
pub fn BlenderSettings() -> Element {
    let state = use_context::<Signal<State>>();
    let mut now = use_signal(|| None::<Value>);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut refreshed = use_signal(|| 0u32);
    let mut confirm_remove = use_signal(|| false);
    use_effect(move || {
        let _ = state.read().apps_changed;
        let _ = refreshed();
        spawn(async move {
            match api::get("/api/blender").await {
                Ok(found) => now.set(Some(found)),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let act = use_callback(move |(method, path, body): (&'static str, &'static str, Option<Value>)| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call(method, path, body.as_ref()).await {
                Ok(answer) => now.set(Some(answer)),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
            confirm_remove.set(false);
        });
    });
    let Some(b) = now() else {
        return rsx! { h2 { class: HEADING, "Blender" } };
    };
    let installed = b["installed"].as_array().is_some_and(|i| !i.is_empty());
    let found = b["found"].as_array().cloned().unwrap_or_default();
    let open = b["open"] == true;
    let enabled = b["enabled"] == true;
    let ticked: Vec<String> = b["bots"].as_array().map(|x| x.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|x| x.channel.is_none()).map(|x| (x.id.clone(), x.name.clone())).collect();
    rsx! {
        h2 { class: HEADING, "Blender" }
        p { class: "text-[13px] leading-relaxed text-ink-secondary",
            "Bots see the Blender open on this computer and, once you approve each one, change the scene, run scripts, render and save. Catbot puts a small add-on into Blender; turn it on once in Blender (Edit › Preferences › Add-ons, search Catbot) and it listens on this computer only."
        }
        div { class: "flex flex-col gap-2 rounded-lg border border-hairline/40 px-3 py-2", "data-blender": true,
            if !installed {
                if found.is_empty() {
                    p { class: "text-[13px] text-warning", "Catbot can't find Blender's settings folder yet. Open Blender once, close it, then install." }
                }
                div {
                    button { r#type: "button", class: PRIMARY, disabled: busy(), "data-blender-install": true, onclick: move |_| act.call(("POST", "/api/blender/install", None)), "Install the add-on" }
                }
            } else {
                div { class: "flex items-center gap-2 text-[13px]",
                    if open {
                        span { class: "h-2 w-2 rounded-full bg-success" }
                        span { class: "flex-1 text-ink", "Blender {b[\"blender\"].as_str().unwrap_or_default()} is open" }
                    } else {
                        span { class: "h-2 w-2 rounded-full bg-ink-tertiary" }
                        span { class: "flex-1 text-ink-secondary", "Not reachable now" }
                    }
                    button { r#type: "button", class: LINK, onclick: move |_| refreshed.set(refreshed() + 1), "Check again" }
                }
                if let Some(why) = b["problem"].as_str() {
                    p { class: "text-[12px] text-ink-tertiary", "{why}" }
                }
                if b["outdated"] == true {
                    p { class: "text-[12px] text-warning", "Blender has an older add-on: install it again, then turn it off and on in Blender." }
                }
                label { class: "flex items-center gap-2 text-[13px] text-ink",
                    input { r#type: "checkbox", checked: enabled, "data-blender-on": true,
                        onchange: move |_| act.call(("PUT", "/api/blender", Some(json!({ "enabled": !enabled })))),
                    }
                    "Bots may use Blender"
                }
                div { class: "flex flex-wrap items-center gap-x-4 gap-y-1",
                    span { class: "text-[12px] text-ink-secondary", "Bots:" }
                    for (bot_id, bot_name) in bots {
                        {
                            let on = ticked.contains(&bot_id);
                            let others = ticked.clone();
                            rsx! {
                                label { key: "{bot_id}", class: "flex items-center gap-1.5 text-[13px] text-ink",
                                    input { r#type: "checkbox", checked: on, "data-blender-bot": "{bot_name}",
                                        onchange: move |_| {
                                            let mut next: Vec<String> = others.iter().filter(|x| **x != bot_id).cloned().collect();
                                            if !on {
                                                next.push(bot_id.clone());
                                            }
                                            act.call(("PUT", "/api/blender", Some(json!({ "bots": next }))));
                                        },
                                    }
                                    "{bot_name}"
                                }
                            }
                        }
                    }
                }
                p { class: "text-[12px] text-ink-tertiary", "Renders go to {b[\"renders\"].as_str().unwrap_or_default()}; exports with no place given go to {b[\"files\"].as_str().unwrap_or_default()}." }
                div { class: "flex gap-1",
                    button { r#type: "button", class: BUTTON, disabled: busy(), onclick: move |_| act.call(("POST", "/api/blender/install", None)), "Install again" }
                    span { class: "flex-1" }
                    if confirm_remove() {
                        button { r#type: "button", class: "rounded-md bg-danger px-2 py-1 text-[12px] font-medium text-white", onclick: move |_| act.call(("DELETE", "/api/blender/install", None)), "Take the add-on out" }
                        button { r#type: "button", class: LINK, onclick: move |_| confirm_remove.set(false), "Keep" }
                    } else {
                        button { r#type: "button", class: "{LINK} hover:text-danger", onclick: move |_| confirm_remove.set(true), "Take the add-on out…" }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
