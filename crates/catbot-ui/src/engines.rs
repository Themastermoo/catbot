//! Which AI a bot runs on, in its settings: the AI, its model, and the AI's key (or, for an
//! agent program, whether it's installed and how to install and sign it in).
//!
//! The list comes from `GET /api/engines` (no key ever comes back, only its last four
//! characters). Picking another AI is a change to the bot like any other; its models come from
//! the API itself once it has a key (`GET /api/engines/:id/models`). Keys are typed here and
//! saved at home only: on the phone, or the Windows app talking to home, the box says so.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::Bot;
use crate::live::State;

const LABEL: &str = "block text-[12px] font-medium text-ink-secondary";
const FIELD: &str = "mt-1 w-full rounded-lg bg-inset px-3 py-2 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const SMALL_BUTTON: &str = "rounded-full px-2.5 py-1 text-[12px] font-medium text-ink-secondary hover:bg-raised hover:text-ink disabled:opacity-50";

#[component]
pub fn EnginePicker(bot: Bot, saving: bool, change: Callback<Value>) -> Element {
    let state = use_context::<Signal<State>>();
    let mut engines = use_signal(Vec::<Value>::new);
    let mut models = use_signal(|| None::<(String, Vec<String>)>);
    let mut problem = use_signal(|| None::<String>);

    // The list, now and whenever a key is saved or forgotten anywhere.
    use_effect(move || {
        let _ = state.read().engines_changed;
        spawn(async move {
            match api::get("/api/engines").await {
                Ok(answer) => engines.set(answer["engines"].as_array().cloned().unwrap_or_default()),
                Err(err) => problem.set(Some(format!("Couldn't load the AIs: {err}"))),
            }
        });
    });

    let engine_id = bot.engine.clone();
    let engine = engines.read().iter().find(|e| e["id"] == engine_id.as_str()).cloned();
    let ready = engine.as_ref().is_some_and(|e| e["ready"] == true);

    // The API's own models, once it's ready (fetched again when the bot changes AI or a key arrives).
    let wanted = engine_id.clone();
    use_effect(use_reactive!(|(wanted, ready)| {
        if !ready || wanted == "claude" {
            return;
        }
        spawn(async move {
            if let Ok(answer) = api::get(&format!("/api/engines/{wanted}/models")).await {
                let list = answer["models"].as_array().map(|l| l.iter().filter_map(|m| m.as_str().map(str::to_string)).collect()).unwrap_or_default();
                models.set(Some((wanted, list)));
            }
        });
    }));

    let Some(engine) = engine else {
        return rsx! {
            label { class: LABEL, "AI" }
            p { class: "mt-1 text-[13px] text-ink-secondary", if let Some(err) = problem() { "{err}" } else { "Loading…" } }
        };
    };

    // The models to offer: the API's own list, else the suggested ones; the bot's own always.
    let suggested: Vec<(String, String)> =
        engine["models"].as_array().map(|l| l.iter().map(|m| (str_of(m, "id"), str_of(m, "label"))).collect()).unwrap_or_default();
    let mut options: Vec<(String, String)> = match models() {
        Some((id, list)) if id == engine_id && !list.is_empty() => list.into_iter().map(|m| (m.clone(), m)).collect(),
        _ => suggested.into_iter().filter(|(id, _)| !id.is_empty()).collect(),
    };
    let default_model = str_of(&engine, "defaultModel");
    let agent = engine["kind"] == "agent";
    let default_label = if engine_id == "claude" || agent {
        "Default".to_string()
    } else if default_model.is_empty() {
        "Pick a model".to_string()
    } else {
        format!("Default ({default_model})")
    };
    options.insert(0, (String::new(), default_label));
    if !options.iter().any(|(id, _)| *id == bot.model) {
        options.push((bot.model.clone(), bot.model.clone()));
    }
    let list = engines();
    let picked = engine_id.clone();

    rsx! {
        label { class: LABEL, "AI"
            select {
                class: FIELD,
                "data-engine": true,
                disabled: saving,
                onchange: move |e| change.call(json!({ "modelSelection": { "instanceId": e.value() } })),
                for e in list {
                    {
                        let id = str_of(&e, "id");
                        let name = str_of(&e, "name");
                        let shown = match (e["ready"] == true, e["kind"].as_str()) {
                            (true, _) => name,
                            (false, Some("agent")) => format!("{name} (not installed)"),
                            (false, _) => format!("{name} (needs a key)"),
                        };
                        rsx! { option { key: "{id}", value: "{id}", selected: id == picked, "{shown}" } }
                    }
                }
            }
        }
        if agent {
            // An agent program takes the model by name, or uses its own.
            label { class: "mt-3 {LABEL}", "Model"
                input {
                    key: "{engine_id}",
                    class: FIELD,
                    "data-model": true,
                    initial_value: "{bot.model}",
                    placeholder: "Its own default",
                    autocomplete: "off",
                    onchange: {
                        let engine_id = engine_id.clone();
                        move |e: FormEvent| change.call(json!({ "modelSelection": { "instanceId": engine_id, "model": e.value().trim() } }))
                    },
                }
            }
            AgentBox { engine: engine.clone() }
        } else {
        label { class: "mt-3 {LABEL}", "Model"
            select {
                class: FIELD,
                "data-model": true,
                disabled: saving,
                onchange: {
                    let engine_id = engine_id.clone();
                    move |e: FormEvent| change.call(json!({ "modelSelection": { "instanceId": engine_id, "model": e.value() } }))
                },
                for (id, label) in options {
                    option { key: "{id}", value: "{id}", selected: id == bot.model, "{label}" }
                }
            }
        }
        if engine["kind"] == "api" {
            KeyBox { engine: engine.clone() }
        }
        }
    }
}

/// An agent program: installed or not, and how to install it and sign it in.
#[component]
fn AgentBox(engine: Value) -> Element {
    let name = str_of(&engine, "name");
    let install = str_of(&engine, "install");
    let sign_in = str_of(&engine, "signIn");
    let page = str_of(&engine, "keyPage");
    let installed = engine["installed"] == true;
    let id = str_of(&engine, "id");
    rsx! {
        div { class: "mt-3 rounded-lg bg-inset/60 px-3 py-2.5 text-[13px] leading-[19px] text-ink", "data-agent": "{id}",
            if installed {
                p { "{name} is installed. It uses its own sign-in and plan; if it isn't signed in yet, run this once in a terminal:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{sign_in}" }
            } else {
                p { "{name} isn't installed on this computer. Install it in a terminal:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{install}" }
                p { class: "mt-1", "then sign in:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{sign_in}" }
            }
            p { class: "mt-1.5 text-[12px] text-ink-tertiary",
                "More: "
                a { href: "{page}", target: "_blank", rel: "noreferrer", class: "text-accent underline", "{page}" }
            }
        }
    }
}

/// An API's key (or, for one on this computer, its address): saved, from the environment, or wanted.
#[component]
fn KeyBox(engine: Value) -> Element {
    let mut typing = use_signal(String::new);
    let mut address = use_signal(String::new);
    let mut editing = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let id = str_of(&engine, "id");
    let name = str_of(&engine, "name");
    let needs_key = engine["needsKey"] == true;
    let ready = engine["ready"] == true;
    let hint = engine["keyHint"].as_str().map(str::to_string);
    let from_env = engine["keyFrom"] == "environment";
    let url = str_of(&engine, "url");
    let key_page = str_of(&engine, "keyPage");
    // Keys are kept at home; away from it the box only says how things stand.
    let at_home = crate::link::active().is_none();

    let save = {
        let id = id.clone();
        move |_| {
            let (key, url) = (typing.peek().trim().to_string(), address.peek().trim().to_string());
            let id = id.clone();
            busy.set(true);
            problem.set(None);
            spawn(async move {
                let mut body = json!({});
                if !key.is_empty() {
                    body["key"] = json!(key);
                }
                if !url.is_empty() {
                    body["url"] = json!(url);
                }
                match api::call("PUT", &format!("/api/engines/{id}/key"), Some(&body)).await {
                    Ok(_) => {
                        typing.set(String::new());
                        editing.set(false);
                    }
                    Err(err) => problem.set(Some(err)),
                }
                busy.set(false);
            });
        }
    };
    let forget = {
        let id = id.clone();
        move |_| {
            let id = id.clone();
            busy.set(true);
            spawn(async move {
                if let Err(err) = api::call("DELETE", &format!("/api/engines/{id}/key"), None).await {
                    problem.set(Some(err));
                }
                busy.set(false);
            });
        }
    };

    let asking = at_home && (editing() || (needs_key && !ready));
    rsx! {
        div { class: "mt-3 rounded-lg bg-inset/60 px-3 py-2.5", "data-engine-key": "{id}",
            if needs_key && ready && !editing() {
                div { class: "flex items-center gap-2 text-[13px] text-ink",
                    span { class: "min-w-0 flex-1",
                        if from_env { "Key from the environment" } else { "Key saved " }
                        if let Some(hint) = hint.clone() { span { class: "font-mono text-ink-secondary", "{hint}" } }
                    }
                    if at_home && !from_env {
                        button { r#type: "button", class: SMALL_BUTTON, disabled: busy(), onclick: move |_| editing.set(true), "Change" }
                        button { r#type: "button", class: SMALL_BUTTON, disabled: busy(), onclick: forget, "Remove" }
                    }
                }
            } else if !needs_key && !editing() {
                div { class: "flex items-center gap-2 text-[13px] text-ink",
                    span { class: "min-w-0 flex-1 break-all", "At {url}" }
                    if at_home {
                        button { r#type: "button", class: SMALL_BUTTON, onclick: move |_| editing.set(true), "Change" }
                    }
                }
            } else if !at_home {
                p { class: "text-[13px] text-ink-secondary", "{name} needs a key. Keys are added on the computer at home." }
            }
            if asking {
                if needs_key {
                    label { class: LABEL, "{name} API key"
                        input {
                            r#type: "password",
                            class: FIELD,
                            "data-key-input": true,
                            autocomplete: "off",
                            placeholder: "Paste the key",
                            oninput: move |e| typing.set(e.value()),
                        }
                    }
                    p { class: "mt-1 text-[12px] text-ink-tertiary",
                        "Kept on this computer only. Get one at "
                        a { href: "{key_page}", target: "_blank", rel: "noreferrer", class: "text-accent underline", "{key_page}" }
                    }
                } else {
                    label { class: LABEL, "Address"
                        input { class: FIELD, initial_value: "{url}", autocomplete: "off", oninput: move |e| address.set(e.value()) }
                    }
                }
                div { class: "mt-2 flex justify-end gap-2",
                    if editing() {
                        button { r#type: "button", class: SMALL_BUTTON, onclick: move |_| editing.set(false), "Cancel" }
                    }
                    button {
                        r#type: "button",
                        "data-save-key": true,
                        disabled: busy(),
                        class: "rounded-full bg-accent px-3 py-1 text-[12px] font-semibold text-white hover:opacity-90 disabled:opacity-50",
                        onclick: save,
                        if busy() { "Saving…" } else { "Save" }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "mt-1 text-[12px] text-danger", "{err}" }
            }
        }
    }
}

fn str_of(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}
