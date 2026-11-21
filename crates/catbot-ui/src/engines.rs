//! Which AI a bot runs on, in its settings: the AI, its model, and the AI's key (or, for an
//! agent program, whether it's installed and how to install and sign it in).
//!
//! The list comes from `GET /api/engines` (no key ever comes back, only its last four
//! characters). Picking another AI is a change to the bot like any other; its models come from
//! the API itself once it has a key (`GET /api/engines/:id/models`). Keys are typed here and
//! saved at home only: on the phone, or the Windows app talking to home, the box says so.
//! Codex (12.4.1) signs in with ChatGPT here instead, shared with the person's own Codex. Pi
//! (12.4.4) offers the models it's signed in to.

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
    let codex = engine["kind"] == "codex";
    let pi = engine["kind"] == "pi";
    let listed = models().filter(|(id, _)| *id == engine_id).map(|(_, list)| list.len());
    let default_label = if engine_id == "claude" || agent || codex || pi {
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
                            (false, Some("agent")) if e["installed"] == true => format!("{name} (not signed in)"),
                            (false, Some("agent")) => format!("{name} (not installed)"),
                            (false, Some("codex")) if e["installed"] != true => format!("{name} (not installed)"),
                            (false, Some("codex")) => format!("{name} (not signed in)"),
                            (false, Some("pi")) => format!("{name} (not installed)"),
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
        if codex {
            CodexBox { engine: engine.clone() }
        }
        if pi {
            PiBox { engine: engine.clone(), listed }
        }
        }
    }
}

/// How to set up or manage one AI (stage 19.2, Settings › AI): the same box a bot's settings
/// show for it, chosen by its kind.
#[component]
pub fn EngineSetup(engine: Value) -> Element {
    match engine["kind"].as_str() {
        Some("api") => rsx! { KeyBox { engine } },
        Some("agent") => rsx! { AgentBox { engine } },
        Some("codex") => rsx! { CodexBox { engine } },
        Some("pi") => rsx! { PiBox { engine, listed: None } },
        _ => {
            let (install, sign_in) = (str_of(&engine, "install"), str_of(&engine, "signIn"));
            rsx! {
                div { class: "mt-3 rounded-lg bg-inset/60 px-3 py-2.5 text-[13px] leading-[19px] text-ink",
                    if engine["ready"] == true {
                        p { "Ready. Bots on it use this computer's own sign-in." }
                    } else {
                        p { "Install it on this computer, as the same user that runs Catbot:" }
                        if !install.is_empty() {
                            code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{install}" }
                        }
                        if !sign_in.is_empty() {
                            p { class: "mt-1", "then sign it in: " code { class: "rounded bg-inset px-1 font-mono text-[12px]", "{sign_in}" } }
                        }
                    }
                }
            }
        }
    }
}

/// Pi (12.4.4): installed or not, and signed in to a provider of the person's own. Its models
/// are the ones Pi has a sign-in or key for (`listed`, once Catbot has asked). Commands and edits
/// stop for a yes on a card, through Catbot's extension.
#[component]
fn PiBox(engine: Value, listed: Option<usize>) -> Element {
    let install = str_of(&engine, "install");
    let sign_in = str_of(&engine, "signIn");
    let page = str_of(&engine, "keyPage");
    let installed = engine["installed"] == true;
    rsx! {
        div { class: "mt-3 rounded-lg bg-inset/60 px-3 py-2.5 text-[13px] leading-[19px] text-ink", "data-agent": "pi",
            if !installed {
                p { "Pi isn't installed on this computer. Install it in a terminal:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{install}" }
                p { class: "mt-1", "then sign it in to a provider (Anthropic, OpenAI, Google and others, with an account or a key):" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{sign_in}" }
            } else if listed == Some(0) {
                p { "Pi is installed but has no models with a sign-in yet. Sign it in to a provider in a terminal, then come back:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{sign_in}" }
            } else {
                p { "Pi uses your own providers and keys. The models above are the ones it's signed in to; to add more, run:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{sign_in}" }
            }
            p { class: "mt-1.5 text-[12px] text-ink-tertiary",
                "Commands and edits wait for your yes on a card, as with the other AIs. More: "
                a { href: "{page}", target: "_blank", rel: "noreferrer", class: "text-accent underline", "{page}" }
            }
        }
    }
}

/// An agent program: installed or not, and how to install it and sign it in. Some Catbot
/// downloads itself (Antigravity) and signs in through their own page.
#[component]
fn AgentBox(engine: Value) -> Element {
    if !engine["download"].is_null() || engine["signsInHere"] == true {
        return rsx! { ManagedAgentBox { engine } };
    }
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

/// An agent Catbot installs and signs in itself: an Install button with its progress, then a
/// sign-in through the agent's own page (Google, for Antigravity). When the browser that
/// finished the sign-in is on another device, the address it ended at is pasted here. Muse Code
/// (12.4.5) signs in with a code typed at Meta's page instead, so it's shown here.
#[component]
fn ManagedAgentBox(engine: Value) -> Element {
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut pasted = use_signal(String::new);
    let id = str_of(&engine, "id");
    let name = str_of(&engine, "name");
    let installed = engine["installed"] == true;
    let signed_in = engine["signedIn"] == true;
    let work = engine["work"].clone();
    let size = str_of(&engine["download"], "size");
    let version = str_of(&engine["download"], "version");
    let with = engine["signInWith"].as_str().unwrap_or("its account").to_string();
    let code = work["code"].as_str().map(str::to_string);
    let at_home = crate::link::active().is_none();
    let act = use_callback(move |(method, path, body): (&'static str, String, Option<Value>)| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            if let Err(err) = api::call(method, &path, body.as_ref()).await {
                problem.set(Some(err));
            } else if path.ends_with("callback") {
                pasted.set(String::new());
            }
            busy.set(false);
        });
    });
    let mb = |bytes: &Value| bytes.as_u64().map(|b| format!("{} MB", b / 1_000_000)).unwrap_or_default();
    let (i1, i2, i3, i4, i5) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
    rsx! {
        div { class: "mt-3 rounded-lg bg-inset/60 px-3 py-2.5 text-[13px] leading-[19px] text-ink", "data-agent": "{id}",
            if work["installing"] == true {
                p { "data-agent-installing": true,
                    "Downloading {name}… {mb(&work[\"got\"])}"
                    if !work["total"].is_null() { " of {mb(&work[\"total\"])}" }
                }
            } else if !installed {
                p { "{name} isn't installed. Catbot can download it ({size}, version {version}) and keep it in its own folder." }
                if at_home {
                    button {
                        r#type: "button",
                        "data-agent-install": true,
                        disabled: busy(),
                        class: "mt-2 rounded-full bg-accent px-3 py-1 text-[12px] font-semibold text-white hover:opacity-90 disabled:opacity-50",
                        onclick: move |_| act.call(("POST", format!("/api/engines/{i1}/install"), None)),
                        "Install {name}"
                    }
                } else {
                    p { class: "mt-1 text-ink-secondary", "It's installed from Catbot on the computer at home." }
                }
            } else if signed_in {
                p { "data-agent-signed-in": true, "{name} is installed and signed in." }
            } else if work["signingIn"] == true && code.is_some() {
                // A device code (Muse Code): typed at the page, on this computer or any other.
                p { "Open {with}'s page, on this computer or your phone, and enter this code:" }
                p { class: "mt-1 font-mono text-[18px] font-semibold tracking-wider", "data-agent-code": true, "{code.clone().unwrap_or_default()}" }
                a { href: "{work[\"url\"].as_str().unwrap_or_default()}", target: "_blank", rel: "noreferrer", class: "mt-1 block text-accent underline", "data-agent-sign-in-link": true, "Open {with}'s page" }
                div { class: "mt-2 flex items-center gap-2",
                    span { class: "flex-1 text-[12px] text-ink-tertiary", "Waiting for the sign-in…" }
                    button { r#type: "button", class: SMALL_BUTTON, onclick: move |_| act.call(("DELETE", format!("/api/engines/{i5}/sign-in"), None)), "Cancel" }
                }
            } else if work["signingIn"] == true {
                p { "Sign in on the page this opens, then come back:" }
                a { href: "{work[\"url\"].as_str().unwrap_or_default()}", target: "_blank", rel: "noreferrer", class: "mt-1 block text-accent underline", "data-agent-sign-in-link": true, "Open the sign-in page" }
                p { class: "mt-2 text-[12px] text-ink-tertiary", "Finished on another device (a phone)? Its browser ends on a page that won't open. Paste that page's address here:" }
                div { class: "mt-1 flex gap-2",
                    input {
                        class: "w-full rounded-lg bg-inset px-2 py-1 text-[12px] text-ink focus:outline-none focus:ring-1 focus:ring-accent",
                        placeholder: "http://127.0.0.1:…/?code=…",
                        "data-agent-callback": true,
                        value: "{pasted}",
                        oninput: move |e| pasted.set(e.value()),
                    }
                    button {
                        r#type: "button",
                        class: SMALL_BUTTON,
                        disabled: busy() || pasted().trim().is_empty(),
                        onclick: move |_| act.call(("POST", format!("/api/engines/{i2}/sign-in/callback"), Some(json!({ "url": pasted().trim() })))),
                        "Send"
                    }
                }
                div { class: "mt-2 flex items-center gap-2",
                    span { class: "flex-1 text-[12px] text-ink-tertiary", "Waiting for the sign-in…" }
                    button { r#type: "button", class: SMALL_BUTTON, onclick: move |_| act.call(("DELETE", format!("/api/engines/{i3}/sign-in"), None)), "Cancel" }
                }
            } else if at_home {
                p { "{name} is installed. Sign it in once with your {with} account:" }
                button {
                    r#type: "button",
                    "data-agent-sign-in": true,
                    disabled: busy(),
                    class: "mt-2 rounded-full bg-accent px-3 py-1 text-[12px] font-semibold text-white hover:opacity-90 disabled:opacity-50",
                    onclick: move |_| act.call(("POST", format!("/api/engines/{i4}/sign-in"), None)),
                    if busy() { "Starting…" } else { "Sign in with {with}" }
                }
            } else {
                p { class: "text-ink-secondary", "{name} isn't signed in. Sign it in from Catbot on the computer at home." }
            }
            if id == "muse-code" && !signed_in {
                p { class: "mt-1.5 text-[12px] text-ink-tertiary", "The sign-in is shared with Muse Code on this computer: if you've already run `muse login`, it's signed in." }
            }
            if id == "antigravity" {
                p { class: "mt-1.5 text-[12px] text-ink-tertiary", "Catbot gets Antigravity's Gemini models: this release offers its others only to editors Google lists." }
            }
            if let Some(err) = work["problem"].as_str().map(str::to_string).or(problem()) {
                p { role: "alert", class: "mt-1 text-[12px] text-danger", "{err}" }
            }
        }
    }
}

/// Codex: installed or not, and signed in to ChatGPT or not, with the way to sign in. The sign-in
/// is the person's own Codex's too, so it's shared both ways.
#[component]
fn CodexBox(engine: Value) -> Element {
    let state = use_context::<Signal<State>>();
    let mut found = use_signal(|| Value::Null);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    use_effect(move || {
        let _ = state.read().engines_changed;
        spawn(async move {
            match api::get("/api/engines/codex/account").await {
                Ok(answer) => found.set(answer),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let start = use_callback(move |how: &'static str| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call("POST", "/api/engines/codex/sign-in", Some(&json!({ "how": how }))).await {
                Ok(answer) => {
                    let mut now = found.peek().clone();
                    now["signIn"] = answer["signIn"].clone();
                    found.set(now);
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    });
    let cancel = move |_| {
        spawn(async move {
            let _ = api::call("DELETE", "/api/engines/codex/sign-in", None).await;
        });
    };
    let at_home = crate::link::active().is_none();
    let install = str_of(&engine, "install");
    let info = found();
    let installed = engine["installed"] == true;
    let account = info["account"].clone();
    let signed_in = account["signedIn"] == true;
    let waiting = info["signIn"]["pending"] == true;
    let url = str_of(&info["signIn"], "url");
    let code = str_of(&info["signIn"], "code");
    let how = str_of(&info["signIn"], "how");
    let failed = info["signIn"]["error"].as_str().map(str::to_string);
    let who = match (account["email"].as_str(), account["plan"].as_str()) {
        (Some(email), Some(plan)) => format!("Signed in to ChatGPT as {email} ({plan} plan)."),
        (Some(email), None) => format!("Signed in to ChatGPT as {email}."),
        _ if account["kind"] == "apiKey" => "Signed in with an OpenAI API key.".to_string(),
        _ => "Signed in.".to_string(),
    };
    rsx! {
        div { class: "mt-3 rounded-lg bg-inset/60 px-3 py-2.5 text-[13px] leading-[19px] text-ink", "data-codex": true,
            if !installed {
                p { "Codex isn't installed on this computer. Install it in a terminal:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px]", "{install}" }
                p { class: "mt-1", "then sign in here." }
            } else if signed_in {
                p { "data-codex-account": true, "{who}" }
                p { class: "mt-1 text-[12px] text-ink-tertiary", "Shared with Codex on this computer. The bots' chats are kept apart from your own Codex history." }
            } else if waiting {
                if how == "browser" {
                    p { "Finish signing in on the ChatGPT page:" }
                    a { href: "{url}", target: "_blank", rel: "noreferrer", class: "mt-1 block break-all text-accent underline", "data-codex-url": true, "Open the ChatGPT sign-in page" }
                } else {
                    p { "On any device, open " a { href: "{url}", target: "_blank", rel: "noreferrer", class: "text-accent underline", "data-codex-url": true, "{url}" } ", sign in to ChatGPT, and enter this code:" }
                    p { class: "mt-1.5 font-mono text-[20px] font-semibold tracking-widest", "data-codex-code": true, "{code}" }
                }
                div { class: "mt-2 flex items-center gap-2",
                    span { class: "flex-1 text-[12px] text-ink-tertiary", "Waiting for ChatGPT…" }
                    button { r#type: "button", class: SMALL_BUTTON, onclick: cancel, "Cancel" }
                }
            } else if at_home {
                p { "Codex runs on your ChatGPT plan. Signing in here signs in Codex on this computer too." }
                div { class: "mt-2 flex flex-wrap gap-2",
                    button {
                        r#type: "button",
                        "data-codex-sign-in": "code",
                        disabled: busy(),
                        class: "rounded-full bg-accent px-3 py-1 text-[12px] font-semibold text-white hover:opacity-90 disabled:opacity-50",
                        onclick: move |_| start.call("code"),
                        "Sign in with ChatGPT"
                    }
                    button { r#type: "button", "data-codex-sign-in": "browser", disabled: busy(), class: SMALL_BUTTON, onclick: move |_| start.call("browser"), "Use this computer's browser" }
                }
                p { class: "mt-1 text-[12px] text-ink-tertiary", "The first gives a code to type on any device. If ChatGPT refuses it, turn on device codes in ChatGPT's security settings, or use the browser." }
            } else {
                p { class: "text-ink-secondary", "Codex isn't signed in. Sign in from Catbot on the computer at home." }
            }
            if let Some(err) = failed.or(problem()) {
                p { role: "alert", class: "mt-1 text-[12px] text-danger", "{err}" }
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

/// A bot's backup AIs: who takes over, in order, when its own AI is at a limit (Usage → limits).
#[component]
pub fn Backups(bot: Bot, saving: bool, change: Callback<Value>) -> Element {
    let state = use_context::<Signal<State>>();
    let mut engines = use_signal(Vec::<Value>::new);
    let mut adding = use_signal(|| None::<(String, String)>);
    use_effect(move || {
        let _ = state.read().engines_changed;
        spawn(async move {
            if let Ok(answer) = api::get("/api/engines").await {
                engines.set(answer["engines"].as_array().cloned().unwrap_or_default());
            }
        });
    });
    let name_of = move |id: &str| engines.read().iter().find(|e| e["id"] == id).and_then(|e| e["name"].as_str().map(str::to_string)).unwrap_or_else(|| id.to_string());
    let ready_of = move |id: &str| engines.read().iter().find(|e| e["id"] == id).is_some_and(|e| e["ready"] == true);
    let list = bot.backups.clone();
    let save = move |list: Vec<(String, String)>| {
        let wire: Vec<Value> = list.iter().map(|(engine, model)| serde_json::json!({ "engine": engine, "model": model })).collect();
        change.call(serde_json::json!({ "backups": wire }));
    };
    let count = list.len();
    rsx! {
        div { class: "mt-4 {LABEL}", "Backup AIs" }
        p { class: "mt-0.5 text-[12px] text-ink-tertiary", "When {bot.name}'s AI is at a limit, the first of these that can go answers instead, and the chat says so." }
        div { class: "mt-1.5 flex flex-col gap-1", "data-backups": true,
            for (i, (engine, model)) in list.iter().cloned().enumerate() {
                {
                    let (up, down, gone) = (list.clone(), list.clone(), list.clone());
                    let save_up = save;
                    let save_down = save;
                    let save_gone = save;
                    let label = if model.is_empty() { name_of(&engine) } else { format!("{} · {model}", name_of(&engine)) };
                    let unready = !ready_of(&engine);
                    rsx! {
                        div { key: "{engine}-{model}", class: "flex items-center gap-1 rounded-lg bg-raised px-2 py-1 text-[13px]", "data-backup": "{engine}",
                            span { class: "w-5 shrink-0 text-ink-tertiary", "{i + 1}." }
                            span { class: "min-w-0 flex-1 truncate text-ink", "{label}" }
                            if unready { span { class: "shrink-0 text-[11px] text-warning", "not set up" } }
                            button { r#type: "button", "aria-label": "Move up", disabled: saving || i == 0, class: SMALL,
                                onclick: move |_| { let mut l = up.clone(); l.swap(i, i - 1); save_up(l); }, "↑" }
                            button { r#type: "button", "aria-label": "Move down", disabled: saving || i + 1 == count, class: SMALL,
                                onclick: move |_| { let mut l = down.clone(); l.swap(i, i + 1); save_down(l); }, "↓" }
                            button { r#type: "button", "aria-label": "Remove {label}", disabled: saving, class: SMALL,
                                onclick: move |_| { let mut l = gone.clone(); l.remove(i); save_gone(l); }, "×" }
                        }
                    }
                }
            }
            match adding() {
                None => rsx! {
                    button { r#type: "button", "data-add-backup": true, disabled: saving, class: "self-start rounded-lg px-2 py-1 text-[13px] text-accent hover:bg-raised",
                        onclick: move |_| adding.set(Some((String::new(), String::new()))), "+ Add a backup AI" }
                },
                Some((engine, model)) => {
                    let options: Vec<(String, String, bool)> = engines.read().iter()
                        .map(|e| (e["id"].as_str().unwrap_or_default().to_string(), e["name"].as_str().unwrap_or_default().to_string(), e["ready"] == true))
                        .collect();
                    let presets: Vec<String> = engines.read().iter().find(|e| e["id"] == engine.as_str())
                        .and_then(|e| e["models"].as_array().cloned()).unwrap_or_default().iter()
                        .filter_map(|m| m["id"].as_str().filter(|id| !id.is_empty()).map(str::to_string)).collect();
                    let current = list.clone();
                    let save_new = save;
                    let (chosen_engine, chosen_model) = (engine.clone(), model.clone());
                    rsx! {
                        div { class: "flex flex-wrap items-center gap-1.5", "data-backup-form": true,
                            select { class: SMALL_FIELD, "aria-label": "Backup AI", value: "{engine}",
                                onchange: move |e| adding.set(Some((e.value(), String::new()))),
                                option { value: "", "Pick an AI…" }
                                for (id, name, ready) in options {
                                    option { key: "{id}", value: "{id}", if ready { "{name}" } else { "{name} (not set up)" } }
                                }
                            }
                            input { class: "{SMALL_FIELD} w-[150px]", "aria-label": "Backup model", list: "backup-models", placeholder: "its default model", value: "{model}",
                                oninput: move |e| { let engine = adding.peek().as_ref().map(|a| a.0.clone()).unwrap_or_default(); adding.set(Some((engine, e.value()))); } }
                            datalist { id: "backup-models", for m in presets { option { key: "{m}", value: "{m}" } } }
                            button { r#type: "button", "data-save-backup": true, disabled: saving || chosen_engine.is_empty(), class: "rounded-lg bg-accent px-2.5 py-1 text-[13px] font-semibold text-white disabled:opacity-40",
                                onclick: move |_| {
                                    let mut l = current.clone();
                                    l.push((chosen_engine.clone(), chosen_model.trim().to_string()));
                                    adding.set(None);
                                    save_new(l);
                                }, "Add" }
                            button { r#type: "button", class: "rounded-lg px-2 py-1 text-[13px] text-ink-secondary hover:bg-raised", onclick: move |_| adding.set(None), "Cancel" }
                        }
                    }
                }
            }
        }
    }
}

const SMALL: &str = "flex size-6 shrink-0 items-center justify-center rounded text-ink-secondary hover:bg-control disabled:opacity-30";
const SMALL_FIELD: &str = "h-8 rounded-lg bg-raised px-2 text-[13px] text-ink outline-none";
