//! Settings › Computers (stage 17.1): which bots have a computer of their own, whether each is
//! on, and a live view of its screen that can be watched or taken over. Stage 17.2: a computer
//! can be at Boat instead (boat.dev), with Boat's key kept here.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const BUTTON: &str = "rounded-lg bg-control px-2.5 py-1 text-[12px] font-medium text-ink hover:bg-raised disabled:opacity-50";

/// Where a live view Catbot gave is opened from here: Catbot's own view is on this server (the
/// desktop app adds its address); away from home it isn't reachable, so None.
pub fn view_url(url: &str) -> Option<String> {
    if !url.starts_with('/') {
        return Some(url.to_string());
    }
    #[cfg(target_arch = "wasm32")]
    {
        Some(url.to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if crate::link::active().is_some() {
            return None;
        }
        Some(format!("{}{url}", api::server()))
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
    let mut watching = use_signal(|| None::<(String, String, bool)>);
    let mut boat_key = use_signal(String::new);
    let mut connecting = use_signal(|| false);
    // Bumped when a move is called off, so the Where menu shows where it still is.
    let mut kept = use_signal(|| 0u32);
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
    let boat_on = list["boat"]["connected"] == true;
    let boat_ends = list["boat"]["ends"].as_str().unwrap_or_default().to_string();
    let here = list["engine"].is_string();
    let connect = use_callback(move |key: Option<String>| {
        connecting.set(true);
        problem.set(None);
        spawn(async move {
            match api::call("PUT", "/api/computers/boat", Some(&json!({ "key": key }))).await {
                Ok(_) => boat_key.set(String::new()),
                Err(err) => problem.set(Some(err)),
            }
            if let Ok(list) = api::get("/api/computers").await {
                found.set(list);
            }
            connecting.set(false);
        });
    });
    let bots: Vec<(String, String)> = state.read().bots.iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    rsx! {
        section { class: "flex flex-col gap-3", "data-computers": true,
            h2 { class: HEADING, "Computers" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "A bot with a computer has a small Linux desktop with a browser of its own: in a container on this computer, or at Boat. Nothing of yours is in it, and what it does inside it doesn't ask; sending, posting and using your apps still do. You can watch it work, or take over."
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
                    let kind = entry["kind"].as_str().unwrap_or("container").to_string();
                    let at_boat = kind == "boat";
                    let can_run = has && (at_boat || here);
                    let b6 = bot_id.clone();
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
                                    disabled: !has && !here && !boat_on,
                                    label: format!("{n1} has a computer"),
                                    on_toggle: move |_| {
                                        if has {
                                            confirm.set(Some((b1.clone(), "remove")));
                                        } else {
                                            let kind = if here { "container" } else { "boat" };
                                            act.call(("PUT", format!("/api/computers/{b1}"), Some(json!({ "on": true, "kind": kind }))));
                                        }
                                    },
                                }
                            }
                            if has {
                                div { class: "flex flex-wrap items-center gap-2",
                                    select {
                                        key: "{bot_id}-{kept}",
                                        "data-computer-kind": true,
                                        "aria-label": "Where {name}'s computer runs",
                                        class: "rounded-lg border border-hairline/60 bg-raised px-2 py-1 text-[12px] text-ink",
                                        disabled: !(now == "on" || now == "off"),
                                        onchange: move |e| {
                                            let to = if e.value() == "boat" { "move-boat" } else { "move-container" };
                                            confirm.set(Some((b6.clone(), to)));
                                        },
                                        option { value: "container", selected: !at_boat, disabled: !here, "On this computer" }
                                        option { value: "boat", selected: at_boat, disabled: !boat_on, "At Boat (cloud)" }
                                    }
                                    if now == "on" {
                                        button { r#type: "button", class: BUTTON, "data-computer-watch": true, onclick: move |_| watching.set(Some((b2.clone(), n2.clone(), at_boat))), "Watch" }
                                        button { r#type: "button", class: BUTTON, "data-computer-stop": true, onclick: move |_| act.call(("POST", format!("/api/computers/{b3}/stop"), None)), "Stop" }
                                    } else if now == "off" {
                                        button { r#type: "button", class: BUTTON, "data-computer-start": true, disabled: !can_run, onclick: move |_| act.call(("POST", format!("/api/computers/{b3}/start"), None)), "Start" }
                                    }
                                    if now == "on" || now == "off" {
                                        button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-danger", onclick: move |_| confirm.set(Some((b4.clone(), "reset"))), "Reset…" }
                                    }
                                }
                            }
                            if let Some(what) = asking {
                                div { class: "flex flex-wrap items-center gap-2 rounded-lg bg-inset px-2 py-1.5", "data-computer-confirm": "{what}",
                                    span { class: "min-w-0 flex-1 text-[12px] text-ink",
                                        match what {
                                            "reset" => "Throw this computer away and start fresh? Everything on it goes.",
                                            "move-boat" => "Move this bot's computer to Boat? Its computer here and everything on it goes; Boat bills while it's on.",
                                            "move-container" => "Move this bot's computer here? Its computer at Boat and everything on it goes.",
                                            _ => "Take this bot's computer away? Everything on it goes.",
                                        }
                                    }
                                    button {
                                        r#type: "button",
                                        class: "rounded-lg bg-danger px-2.5 py-1 text-[12px] font-medium text-white",
                                        "data-computer-confirm-yes": true,
                                        onclick: move |_| {
                                            match what {
                                                "reset" => act.call(("POST", format!("/api/computers/{b5}/reset"), None)),
                                                "move-boat" => act.call(("PUT", format!("/api/computers/{b5}"), Some(json!({ "on": true, "kind": "boat" })))),
                                                "move-container" => act.call(("PUT", format!("/api/computers/{b5}"), Some(json!({ "on": true, "kind": "container" })))),
                                                _ => act.call(("PUT", format!("/api/computers/{b5}"), Some(json!({ "on": false })))),
                                            }
                                        },
                                        match what {
                                            "reset" => "Reset",
                                            "move-boat" | "move-container" => "Move it",
                                            _ => "Take it away",
                                        }
                                    }
                                    button {
                                        r#type: "button",
                                        class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised",
                                        onclick: move |_| {
                                            confirm.set(None);
                                            kept += 1;
                                        },
                                        "Keep"
                                    }
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
            div { class: "flex flex-col gap-2 rounded-lg border border-hairline/40 px-3 py-2", "data-boat": true,
                div { class: "flex items-center gap-2",
                    span { class: "flex-1 text-[14px] font-medium text-ink", "Boat (cloud computers)" }
                    if boat_on {
                        span { class: "text-[12px] text-ink-tertiary", "Connected · key …{boat_ends}" }
                        button { r#type: "button", class: BUTTON, disabled: connecting(), onclick: move |_| connect.call(None), "Disconnect" }
                    }
                }
                p { class: "text-[12px] leading-relaxed text-ink-secondary",
                    "A bot's computer can be at boat.dev instead: on while this computer is off, and nothing runs here. Boat bills per second while one is on ($20 a month plan) and nothing while it's stopped. Each gets none of your Boat account's secrets."
                }
                if !boat_on {
                    div { class: "flex gap-2",
                        input {
                            class: "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent",
                            r#type: "password",
                            placeholder: "Boat API key (boat_…)",
                            "data-boat-key": true,
                            value: "{boat_key}",
                            oninput: move |e| boat_key.set(e.value()),
                        }
                        button {
                            r#type: "button",
                            class: "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50",
                            "data-boat-connect": true,
                            disabled: connecting() || boat_key().trim().is_empty(),
                            onclick: move |_| connect.call(Some(boat_key())),
                            "Connect"
                        }
                    }
                }
            }
        }
        if let Some((bot_id, name, at_boat)) = watching() {
            ComputerView { bot_id, name, at_boat, on_close: move |_| watching.set(None) }
        }
    }
}

/// A bot's screen, live: watched by default, or used ("Let me use it"). Catbot gives the
/// address: its own view for a computer here, Boat's for one there.
#[component]
pub fn ComputerView(bot_id: String, name: String, at_boat: bool, on_close: EventHandler<()>) -> Element {
    let mut use_it = use_signal(|| false);
    let mut url = use_signal(|| None::<Result<Option<String>, String>>);
    let id = bot_id.clone();
    use_effect(move || {
        let how = if use_it() { "use" } else { "watch" };
        let id = id.clone();
        url.set(None);
        spawn(async move {
            let found = api::get(&format!("/api/computers/{id}/view/{how}")).await;
            url.set(Some(found.map(|f| f["url"].as_str().and_then(view_url))));
        });
    });
    // While it's open the computer isn't idle (Boat's view can't be counted from here).
    let id = bot_id.clone();
    use_future(move || {
        let id = id.clone();
        async move {
            loop {
                crate::events::pause(60_000).await;
                let _ = api::call("POST", &format!("/api/computers/{id}/watching"), None).await;
            }
        }
    });
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
                    if let Some(Ok(Some(link))) = url() {
                        if at_boat {
                            a { class: "text-[12px] text-accent underline", href: "{link}", target: "_blank", rel: "noopener", "Open in a new tab" }
                        }
                    }
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
                match url() {
                    None => rsx! {
                        div { class: "flex aspect-[16/10] w-full items-center justify-center rounded-lg border border-hairline/40 bg-black text-[13px] text-white/70", "Connecting…" }
                    },
                    Some(Ok(Some(link))) => rsx! {
                        iframe {
                            key: "{link}",
                            title: "{name}'s screen",
                            src: "{link}",
                            class: "aspect-[16/10] w-full rounded-lg border border-hairline/40 bg-black",
                        }
                    },
                    Some(Ok(None)) => rsx! {
                        p { class: "text-[13px] text-ink-secondary", "The live view is at home only: open Catbot on the computer it runs on." }
                    },
                    Some(Err(err)) => rsx! {
                        p { role: "alert", class: "text-[13px] text-danger", "{err}" }
                    },
                }
                p { class: "text-[12px] text-ink-tertiary",
                    if use_it() { "Your clicks and typing go to the bot's computer. The bot can see what you do." } else { "Watching only: your clicks don't reach it." }
                }
            }
        }
    }
}
