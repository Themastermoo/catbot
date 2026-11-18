//! Settings › Computers (stage 17.1): which bots have a computer of their own, whether each is
//! on, and a live view of its screen that can be watched or taken over.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const BUTTON: &str = "rounded-lg bg-control px-2.5 py-1 text-[12px] font-medium text-ink hover:bg-raised disabled:opacity-50";

/// Where a bot's live view is, or None where it can't be shown (the phone, away from home:
/// the screen is carried only through the home door).
pub fn view_url(bot_id: &str, use_it: bool) -> Option<String> {
    let only = if use_it { 0 } else { 1 };
    let path = format!("/computer/view/vnc.html?autoconnect=1&reconnect=1&resize=scale&show_dot=1&view_only={only}&path=computer/{bot_id}/websockify");
    #[cfg(target_arch = "wasm32")]
    {
        Some(path)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if crate::link::active().is_some() {
            return None;
        }
        Some(format!("{}{path}", api::server()))
    }
}

fn state_words(state: &str) -> (&'static str, &'static str) {
    match state {
        "on" => ("On", "text-success"),
        "setting up" => ("Setting up (the first time takes a few minutes)…", "text-ink-secondary"),
        "starting" => ("Starting…", "text-ink-secondary"),
        "stopping" => ("Stopping…", "text-ink-secondary"),
        _ => ("Off", "text-ink-tertiary"),
    }
}

#[component]
pub fn ComputersSettings() -> Element {
    let state = use_context::<Signal<State>>();
    let mut found = use_signal(|| Value::Null);
    let mut problem = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| None::<(String, &'static str)>);
    let mut watching = use_signal(|| None::<(String, String)>);
    use_effect(move || {
        let _ = state.read().computers_changed;
        spawn(async move {
            match api::get("/api/computers").await {
                Ok(list) => found.set(list),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let act = use_callback(move |(method, path, body): (&'static str, String, Option<Value>)| {
        problem.set(None);
        confirm.set(None);
        spawn(async move {
            if let Err(err) = api::call(method, &path, body.as_ref()).await {
                problem.set(Some(err));
            }
            if let Ok(list) = api::get("/api/computers").await {
                found.set(list);
            }
        });
    });
    let list = found();
    let engine = list["engine"].as_str().map(str::to_string);
    let image = list["image"].as_str().unwrap_or("missing").to_string();
    let image_problem = list["imageProblem"].as_str().map(str::to_string);
    let idle = list["idleMinutes"].as_u64().unwrap_or(30);
    let entries: Vec<Value> = list["bots"].as_array().cloned().unwrap_or_default();
    let bots: Vec<(String, String)> = state.read().bots.iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    rsx! {
        section { class: "flex flex-col gap-3", "data-computers": true,
            h2 { class: HEADING, "Computers" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "A bot with a computer has a small Linux desktop with a browser of its own, in a container on this computer. Nothing of yours is in it, and what it does inside it doesn't ask; sending, posting and using your apps still do. You can watch it work, or take over."
            }
            match engine {
                None => rsx! {
                    p { class: "text-[13px] text-warning", "data-computers-engine": "none",
                        "Computers need Podman or Docker. On Kubuntu: sudo apt install podman, then open this again."
                    }
                },
                Some(engine) => rsx! {
                    p { class: "text-[12px] text-ink-tertiary", "data-computers-engine": "{engine}",
                        if idle == 0 { "Runs on {engine}." } else { "Runs on {engine}. A computer nobody uses or watches for {idle} minutes is stopped; its files stay." }
                    }
                },
            }
            if image == "building" {
                p { class: "text-[12px] text-ink-secondary", "Making the computers' image: the first time takes a few minutes." }
            }
            if let Some(err) = image_problem {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
            for (bot_id, name) in bots {
                {
                    let entry = entries.iter().find(|e| e["botId"] == bot_id.as_str()).cloned();
                    let has = entry.is_some();
                    let entry = entry.unwrap_or_default();
                    let now = entry["state"].as_str().unwrap_or("off").to_string();
                    let (words, tone) = state_words(&now);
                    let watchers = entry["watching"].as_u64().unwrap_or(0);
                    let fault = entry["problem"].as_str().map(str::to_string);
                    let asking = confirm().filter(|(id, _)| *id == bot_id).map(|(_, what)| what);
                    let (b1, b2, b3, b4, b5) = (bot_id.clone(), bot_id.clone(), bot_id.clone(), bot_id.clone(), bot_id.clone());
                    let (n1, n2) = (name.clone(), name.clone());
                    rsx! {
                        div { key: "{bot_id}", class: "flex flex-col gap-1.5 rounded-lg border border-hairline/40 px-3 py-2", "data-computer": "{bot_id}",
                            div { class: "flex items-center gap-2",
                                span { class: "min-w-0 flex-1 truncate text-[14px] font-medium text-ink", "{name}" }
                                if has {
                                    span { class: "text-[12px] {tone}", "data-computer-state": "{now}", "{words}" }
                                    if watchers > 0 {
                                        span { class: "text-[12px] text-ink-tertiary", "· watched" }
                                    }
                                }
                                crate::settings::Switch {
                                    checked: has,
                                    disabled: engine_missing(&list),
                                    label: format!("{n1} has a computer"),
                                    on_toggle: move |_| {
                                        if has {
                                            confirm.set(Some((b1.clone(), "remove")));
                                        } else {
                                            act.call(("PUT", format!("/api/computers/{b1}"), Some(json!({ "on": true }))));
                                        }
                                    },
                                }
                            }
                            if has {
                                div { class: "flex flex-wrap items-center gap-2",
                                    if now == "on" {
                                        button { r#type: "button", class: BUTTON, "data-computer-watch": true, onclick: move |_| watching.set(Some((b2.clone(), n2.clone()))), "Watch" }
                                        button { r#type: "button", class: BUTTON, "data-computer-stop": true, onclick: move |_| act.call(("POST", format!("/api/computers/{b3}/stop"), None)), "Stop" }
                                    } else if now == "off" {
                                        button { r#type: "button", class: BUTTON, "data-computer-start": true, disabled: engine_missing(&list), onclick: move |_| act.call(("POST", format!("/api/computers/{b3}/start"), None)), "Start" }
                                    }
                                    if now == "on" || now == "off" {
                                        button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-danger", onclick: move |_| confirm.set(Some((b4.clone(), "reset"))), "Reset…" }
                                    }
                                }
                            }
                            if let Some(what) = asking {
                                div { class: "flex flex-wrap items-center gap-2 rounded-lg bg-inset px-2 py-1.5", "data-computer-confirm": "{what}",
                                    span { class: "min-w-0 flex-1 text-[12px] text-ink",
                                        if what == "reset" { "Throw this computer away and start fresh? Everything on it goes." } else { "Take this bot's computer away? Everything on it goes." }
                                    }
                                    button {
                                        r#type: "button",
                                        class: "rounded-lg bg-danger px-2.5 py-1 text-[12px] font-medium text-white",
                                        "data-computer-confirm-yes": true,
                                        onclick: move |_| {
                                            if what == "reset" {
                                                act.call(("POST", format!("/api/computers/{b5}/reset"), None));
                                            } else {
                                                act.call(("PUT", format!("/api/computers/{b5}"), Some(json!({ "on": false }))));
                                            }
                                        },
                                        if what == "reset" { "Reset" } else { "Take it away" }
                                    }
                                    button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised", onclick: move |_| confirm.set(None), "Keep" }
                                }
                            }
                            if let Some(err) = fault.filter(|_| has) {
                                p { role: "alert", class: "text-[12px] text-danger", "data-computer-problem": true, "{err}" }
                            }
                        }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
        if let Some((bot_id, name)) = watching() {
            ComputerView { bot_id, name, on_close: move |_| watching.set(None) }
        }
    }
}

fn engine_missing(list: &Value) -> bool {
    list["engine"].is_null()
}

/// A bot's screen, live: watched by default, or used ("Let me use it").
#[component]
pub fn ComputerView(bot_id: String, name: String, on_close: EventHandler<()>) -> Element {
    let mut use_it = use_signal(|| false);
    let url = view_url(&bot_id, use_it());
    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4",
            onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                "aria-label": "{name}'s computer",
                "data-computer-view": "{bot_id}",
                class: "flex w-full max-w-[1100px] flex-col gap-3 rounded-2xl bg-panel p-4 shadow-2xl",
                onclick: move |e| e.stop_propagation(),
                div { class: "flex items-center gap-3",
                    span { class: "min-w-0 flex-1 truncate text-[15px] font-semibold text-ink", "{name}'s computer" }
                    label { class: "flex items-center gap-2 text-[13px] text-ink-secondary",
                        crate::settings::Switch { checked: use_it(), disabled: false, label: "Let me use it".to_string(), on_toggle: move |_| use_it.toggle() }
                        "Let me use it"
                    }
                    button {
                        r#type: "button",
                        "aria-label": "Close",
                        class: "flex size-8 items-center justify-center rounded-md text-ink-secondary hover:bg-control hover:text-ink",
                        onclick: move |_| on_close.call(()),
                        "×"
                    }
                }
                match url {
                    Some(url) => rsx! {
                        iframe {
                            key: "{url}",
                            title: "{name}'s screen",
                            src: "{url}",
                            class: "aspect-[16/10] w-full rounded-lg border border-hairline/40 bg-black",
                        }
                    },
                    None => rsx! {
                        p { class: "text-[13px] text-ink-secondary", "The live view is at home only: open Catbot on the computer it runs on." }
                    },
                }
                p { class: "text-[12px] text-ink-tertiary",
                    if use_it() { "Your clicks and typing go to the bot's computer. The bot can see what you do." } else { "Watching only: your clicks don't reach it." }
                }
            }
        }
    }
}
