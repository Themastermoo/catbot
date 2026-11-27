//! AI on this computer, in the interface (stage 24): Settings › AI's "On this computer" cards for
//! Ollama and LM Studio, and the card in the New bot dialog.
//!
//! What's found comes from `GET /api/local-ai` (again on each `local-ai` frame): running, its
//! models and sizes, the suggested ones with how they fit this computer, and downloads under
//! way. Nothing changes until Moo clicks: Use it, Start, Get, Remove.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const CARD: &str = "overflow-hidden rounded-xl bg-composer ring-1 ring-inset ring-ink/[0.07]";
const BUTTON: &str = "shrink-0 rounded-full bg-control px-3 py-1 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "shrink-0 rounded-full bg-accent px-3 py-1 text-[13px] font-medium text-accent-ink hover:brightness-110 disabled:opacity-50";
const SMALL: &str = "shrink-0 rounded-full px-2.5 py-0.5 text-[12px] font-medium text-ink-secondary hover:bg-raised hover:text-ink disabled:opacity-50";

/// What `GET /api/local-ai` says, kept fresh: None until it answers.
pub fn use_local_ai() -> (Signal<Option<Result<Value, String>>>, Signal<u32>) {
    let mut seen = use_signal(|| None::<Result<Value, String>>);
    let checked = use_signal(|| 0u32);
    let state = use_context::<Signal<State>>();
    let discovery_changes = use_memo(move || {
        let state = state.read();
        // An existing bot also makes its local engine available without explicit opt-in.
        let uses = |id: &str| state.bots.iter().any(|bot| bot.engine == id);
        (state.local_ai_changed, state.engines_changed, state.archived_changed, uses("ollama"), uses("lmstudio"))
    });
    use_effect(move || {
        let _ = checked();
        let _ = discovery_changes();
        spawn(async move {
            seen.set(Some(api::get("/api/local-ai").await));
        });
    });
    (seen, checked)
}

/// A size in bytes as GB, the way Ollama shows it.
pub fn gb(bytes: u64) -> String {
    let gb = bytes as f64 / 1_000_000_000.0;
    if gb >= 10.0 { format!("{gb:.0} GB") } else { format!("{gb:.1} GB") }
}

/// What a suggested model's fit means here.
pub fn fit_words(fit: &str) -> &'static str {
    match fit {
        "fits" => "Fits your graphics card",
        "partly" => "Partly on the graphics card; slower",
        "slow" => "Runs on the processor; slow",
        "too big" => "Too big for this computer",
        _ => "",
    }
}

/// One line about it, for its card and the New bot dialog.
pub fn summary(one: &Value) -> String {
    let models = one["models"].as_array().map(Vec::len).unwrap_or(0);
    let name = if one["id"] == "ollama" { "Ollama" } else { "LM Studio" };
    let count = match models {
        0 => "no models yet".to_string(),
        1 => "1 model".to_string(),
        n => format!("{n} models"),
    };
    match (one["running"] == true, one["installed"] == true) {
        (true, _) => format!("{name} is running with {count}"),
        (false, true) => format!("{name} is installed but not running"),
        (false, false) => format!("{name} isn't installed"),
    }
}

/// Settings › AI: the two cards, above the providers.
#[component]
pub fn OnThisComputer() -> Element {
    let (seen, mut checked) = use_local_ai();
    let shown = seen();
    rsx! {
        div { class: "flex items-center px-4 pb-2 pt-2",
            h3 { class: "flex-1 text-[13px] font-semibold text-ink", "On this computer" }
            button { r#type: "button", class: "rounded-md px-2 py-1 text-[13px] text-ink-secondary hover:bg-raised hover:text-ink", onclick: move |_| *checked.write() += 1, "Check again" }
        }
        match shown {
            None => rsx! { p { class: "px-4 pb-2 text-[13px] text-ink-tertiary", "Looking…" } },
            Some(Err(err)) => rsx! { p { role: "alert", class: "px-4 pb-2 text-[13px] text-danger", "{err}" } },
            Some(Ok(all)) => rsx! {
                div { class: CARD, "data-local-ai": true,
                    LocalCard { one: all["ollama"].clone(), all: all.clone() }
                    LocalCard { one: all["lmstudio"].clone(), all: all.clone() }
                }
                p { class: "px-4 pt-2 text-[12px] leading-relaxed text-ink-tertiary",
                    "AI on this computer is free and private: nothing leaves your PC. It's slower and less capable than the online AIs."
                }
            },
        }
    }
}

#[component]
fn LocalCard(one: Value, all: Value) -> Element {
    let id = one["id"].as_str().unwrap_or_default().to_string();
    let ollama = id == "ollama";
    let name = if ollama { "Ollama" } else { "LM Studio" };
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut open = use_signal(|| false);
    let running = one["running"] == true;
    let added = one["added"] == true;
    let installed = one["installed"] == true;
    let line = summary(&one);
    let version = one["version"].as_str().map(|v| format!(" · version {v}")).unwrap_or_default();

    let mut act = move |what: &'static str, id: String| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            if let Err(err) = api::post(&format!("/api/local-ai/{id}/{what}"), &json!({})).await {
                problem.set(Some(err));
            }
            busy.set(false);
        });
    };
    let (use_id, start_id) = (id.clone(), id.clone());
    let dot = if running && added { "size-1.5 shrink-0 rounded-full bg-success" } else { "hidden" };
    rsx! {
        div { class: "[&+&]:border-t [&+&]:border-hairline/50", "data-local": "{id}",
            div { class: "flex min-h-[60px] items-center gap-3 px-4 py-2.5",
                span { "aria-hidden": "true", class: "flex size-9 shrink-0 items-center justify-center rounded-full bg-ink/[0.06] text-[14px] font-semibold text-ink-secondary",
                    if ollama { "O" } else { "L" }
                }
                span { class: "flex min-w-0 flex-1 flex-col",
                    span { class: "truncate text-[14px] leading-5 text-ink", "{name}" }
                    span { class: "flex items-center gap-1.5 truncate text-[12px] leading-4 text-ink-secondary",
                        span { class: dot }
                        "{line}{version}"
                        if running && added { " · ready for bots" }
                    }
                }
                if running && !added {
                    button { r#type: "button", "data-use-local": "{id}", disabled: busy(), class: PRIMARY, onclick: move |_| act("use", use_id.clone()), "Use it" }
                } else if !running && installed {
                    button { r#type: "button", "data-start-local": "{id}", disabled: busy(), class: PRIMARY, onclick: move |_| act("start", start_id.clone()),
                        if busy() { "Starting…" } else { "Start" }
                    }
                }
                if ollama && running {
                    button { r#type: "button", "aria-expanded": "{open()}", "data-models-toggle": true, class: BUTTON, onclick: move |_| open.toggle(),
                        if open() { "Close" } else { "Models" }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "px-4 pb-3 text-[12px] text-danger", "{err}" }
            }
            if !installed && !running {
                GetIt { ollama, install: all["install"].clone() }
            }
            if !ollama && running && one["models"].as_array().is_some_and(Vec::is_empty) {
                p { class: "px-4 pb-3 text-[12px] text-ink-secondary", "Load a model in LM Studio and it shows here." }
            }
            if ollama && running && open() {
                Models { one: one.clone(), all: all.clone() }
            }
        }
    }
}

/// Not installed: where to get it, and (Ollama on Linux) the one command to paste.
#[component]
fn GetIt(ollama: bool, install: Value) -> Element {
    let mut copied = use_signal(|| false);
    let page = install[if ollama { "ollamaPage" } else { "lmstudioPage" }].as_str().unwrap_or_default().to_string();
    let command = install["ollama"].as_str().filter(|c| ollama && c.starts_with("curl")).map(str::to_string);
    rsx! {
        div { class: "px-4 pb-3 text-[12px] leading-relaxed text-ink-secondary", "data-get-local": true,
            if let Some(command) = command {
                p { "Paste this in a terminal (it asks for your password), then choose Check again:" }
                code { class: "mt-1 block break-all rounded bg-inset px-2 py-1 font-mono text-[12px] text-ink", "{command}" }
                div { class: "mt-1 flex items-center gap-2",
                    button { r#type: "button", class: SMALL,
                        onclick: move |_| {
                            let quoted = serde_json::to_string(&command).unwrap_or_default();
                            document::eval(&format!("navigator.clipboard?.writeText({quoted});"));
                            copied.set(true);
                        },
                        if copied() { "Copied" } else { "Copy" }
                    }
                    a { href: "{page}", target: "_blank", rel: "noreferrer", class: "text-accent underline", "Or the download page" }
                }
            } else {
                p {
                    "Install it from "
                    a { href: "{page}", target: "_blank", rel: "noreferrer", class: "text-accent underline", "its download page" }
                    ", open it, then choose Check again."
                }
            }
        }
    }
}

/// Ollama's models: what's on this computer (with Remove), downloads under way, and the
/// suggested ones or any name to get.
#[component]
fn Models(one: Value, all: Value) -> Element {
    let mut problem = use_signal(|| None::<String>);
    let mut removing = use_signal(|| None::<String>);
    let mut asked = use_signal(|| None::<String>);
    let mut typed = use_signal(String::new);
    let have: Vec<Value> = one["models"].as_array().cloned().unwrap_or_default();
    let pulling: Vec<Value> = all["pulling"].as_array().cloned().unwrap_or_default();
    let suggested: Vec<Value> = all["suggested"].as_array().cloned().unwrap_or_default();
    let memory = match (all["memory"]["gpu"].as_f64(), all["memory"]["ram"].as_f64()) {
        (Some(g), Some(r)) => format!("Your graphics card has {g:.0} GB and your PC {r:.0} GB."),
        (None, Some(r)) => format!("No graphics card found; your PC has {r:.0} GB."),
        _ => String::new(),
    };

    let mut get = move |model: String| {
        problem.set(None);
        spawn(async move {
            if let Err(err) = api::post("/api/local-ai/ollama/pull", &json!({ "model": model })).await {
                problem.set(Some(err));
            }
        });
    };
    let stop = move |model: String| {
        spawn(async move {
            let _ = api::call("DELETE", &format!("/api/local-ai/ollama/pull?model={}", urlencode(&model)), None).await;
        });
    };
    let mut remove = move |model: String| {
        removing.set(Some(model.clone()));
        asked.set(None);
        spawn(async move {
            if let Err(err) = api::post("/api/local-ai/ollama/remove", &json!({ "model": model })).await {
                problem.set(Some(err));
            }
            removing.set(None);
        });
    };
    let mut get_typed = move |_: ()| {
        let name = typed.peek().trim().to_string();
        if !name.is_empty() {
            get(name);
            typed.set(String::new());
        }
    };

    rsx! {
        div { class: "border-t border-hairline/50 px-4 pb-4 pt-3", "data-ollama-models": true,
            if have.is_empty() {
                p { class: "text-[12px] text-ink-secondary", "No models yet. Get one below." }
            }
            for m in have {
                {
                    let model = m["name"].as_str().unwrap_or_default().to_string();
                    let size = m["size"].as_u64().map(gb).unwrap_or_default();
                    let detail = [m["parameters"].as_str(), m["quantization"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    let thinks = m["thinks"] == true;
                    let confirming = asked().as_deref() == Some(model.as_str());
                    let gone = removing().as_deref() == Some(model.as_str());
                    let (ask, yes) = (model.clone(), model.clone());
                    rsx! {
                        div { key: "{model}", "data-model-row": "{model}", class: "flex items-center gap-2 py-1.5",
                            span { class: "flex min-w-0 flex-1 flex-col",
                                span { class: "truncate font-mono text-[13px] text-ink", "{model}" }
                                span { class: "truncate text-[12px] text-ink-tertiary",
                                    "{size}"
                                    if !detail.is_empty() { " · {detail}" }
                                    if thinks { " · thinks" }
                                }
                            }
                            if gone {
                                span { class: "text-[12px] text-ink-tertiary", "Removing…" }
                            } else if confirming {
                                span { class: "text-[12px] text-ink-secondary", "Remove {size}?" }
                                button { r#type: "button", "data-remove-yes": "{model}", class: "{SMALL} text-danger", onclick: move |_| remove(yes.clone()), "Remove" }
                                button { r#type: "button", class: SMALL, onclick: move |_| asked.set(None), "Keep" }
                            } else {
                                button { r#type: "button", "data-remove": "{model}", class: SMALL, onclick: move |_| asked.set(Some(ask.clone())), "Remove" }
                            }
                        }
                    }
                }
            }
            for p in pulling {
                {
                    let model = p["model"].as_str().unwrap_or_default().to_string();
                    let (done, total) = (p["done"].as_u64().unwrap_or(0), p["total"].as_u64().unwrap_or(0));
                    let percent = (done * 100).checked_div(total).unwrap_or(0);
                    let words = match p["problem"].as_str() {
                        Some(why) => why.to_string(),
                        None if total > 0 => format!("{} of {} · {percent}%", gb(done), gb(total)),
                        None => p["status"].as_str().unwrap_or("starting").to_string(),
                    };
                    let failed = !p["problem"].is_null();
                    let gone = model.clone();
                    rsx! {
                        div { key: "pull-{model}", "data-pulling": "{model}", class: "py-1.5",
                            div { class: "flex items-center gap-2",
                                span { class: "min-w-0 flex-1 truncate font-mono text-[13px] text-ink", "{model}" }
                                button { r#type: "button", class: SMALL, onclick: move |_| stop(gone.clone()), if failed { "Dismiss" } else { "Stop" } }
                            }
                            if !failed {
                                div { class: "mt-1 h-1.5 overflow-hidden rounded-full bg-ink/[0.08]",
                                    div { class: "h-full rounded-full bg-accent transition-[width]", style: "width: {percent}%" }
                                }
                            }
                            p { class: if failed { "mt-0.5 text-[12px] text-danger" } else { "mt-0.5 text-[12px] text-ink-tertiary" }, "{words}" }
                        }
                    }
                }
            }
            h4 { class: "mt-3 text-[12px] font-semibold text-ink-secondary", "Get a model" }
            if !memory.is_empty() {
                p { class: "text-[12px] text-ink-tertiary", "{memory}" }
            }
            for s in suggested {
                {
                    let model = s["name"].as_str().unwrap_or_default().to_string();
                    let about = s["about"].as_str().unwrap_or_default().to_string();
                    let fit = fit_words(s["fit"].as_str().unwrap_or_default());
                    let have = s["have"] == true;
                    let big = s["fit"] == "too big";
                    let wanted = model.clone();
                    let needs = format!("{:.0}", s["needs"].as_f64().unwrap_or_default());
                    rsx! {
                        div { key: "s-{model}", "data-suggested": "{model}", class: "flex items-center gap-2 py-1.5",
                            span { class: "flex min-w-0 flex-1 flex-col",
                                span { class: "truncate text-[13px] text-ink", span { class: "font-mono", "{model}" } " · {about}" }
                                span { class: if big { "text-[12px] text-danger" } else { "text-[12px] text-ink-tertiary" }, "Needs about {needs} GB · {fit}" }
                            }
                            if have {
                                span { class: "text-[12px] text-ink-tertiary", "Have it" }
                            } else {
                                button { r#type: "button", "data-get": "{model}", class: BUTTON, onclick: move |_| get(wanted.clone()), "Get" }
                            }
                        }
                    }
                }
            }
            div { class: "mt-2 flex items-center gap-2",
                input {
                    class: "min-w-0 flex-1 rounded-lg bg-inset px-3 py-1.5 font-mono text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent",
                    "data-model-name": true,
                    placeholder: "Any model, like mistral:7b",
                    value: "{typed}",
                    oninput: move |e| typed.set(e.value()),
                    onkeydown: move |e| if e.key() == Key::Enter { get_typed(()) },
                }
                button { r#type: "button", "data-get-typed": true, class: BUTTON, onclick: move |_| get_typed(()), "Get" }
            }
            p { class: "mt-1 text-[12px] text-ink-tertiary",
                "Names are on "
                a { href: "https://ollama.com/search", target: "_blank", rel: "noreferrer", class: "underline", "ollama.com/search" }
                ". Suggestions as of November 2026."
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
            }
        }
    }
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// The New bot dialog's card: when an AI on this computer is running, offer it for the new bot.
/// `chosen` holds the engine and model picked (None: the usual default).
#[component]
pub fn NewBotLocal(chosen: Signal<Option<(String, String)>>) -> Element {
    let (seen, _) = use_local_ai();
    let mut problem = use_signal(|| None::<String>);
    let Some(Ok(all)) = seen() else { return rsx! {} };
    let running: Vec<Value> = ["ollama", "lmstudio"].iter().map(|id| all[*id].clone()).filter(|o| o["running"] == true && o["models"].as_array().is_some_and(|m| !m.is_empty())).collect();
    let Some(one) = running.first().cloned() else { return rsx! {} };
    let id = one["id"].as_str().unwrap_or_default().to_string();
    let models: Vec<String> = one["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(str::to_string)).collect();
    let line = summary(&one);
    let picked = chosen().filter(|(e, _)| *e == id);
    let first = models[0].clone();
    let use_id = id.clone();
    let model_id = id.clone();
    rsx! {
        div { class: "mt-3 rounded-xl bg-inset px-3 py-2.5", "data-new-bot-local": "{id}",
            if let Some((_, model)) = picked {
                div { class: "flex items-center gap-2",
                    span { class: "flex-1 text-[12px] text-ink-secondary", "Runs on this computer with" }
                    button { r#type: "button", class: SMALL, onclick: move |_| chosen.set(None), "Don't" }
                }
                select {
                    class: "mt-1 w-full rounded-lg bg-panel px-2 py-1.5 font-mono text-[13px] text-ink",
                    "data-new-bot-model": true,
                    onchange: move |e| chosen.set(Some((model_id.clone(), e.value()))),
                    for m in models {
                        option { key: "{m}", value: "{m}", selected: m == model, "{m}" }
                    }
                }
            } else {
                div { class: "flex items-center gap-2",
                    span { class: "flex-1 text-[12px] leading-snug text-ink-secondary", "{line}" }
                    button { r#type: "button", "data-new-bot-use-local": true, class: BUTTON,
                        onclick: move |_| {
                            let (id, first) = (use_id.clone(), first.clone());
                            spawn(async move {
                                // Adding it is the click; the bot is made with it on Create.
                                match api::post(&format!("/api/local-ai/{id}/use"), &json!({})).await {
                                    Ok(_) => chosen.set(Some((id, first))),
                                    Err(err) => problem.set(Some(err)),
                                }
                            });
                        },
                        "Use it"
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "mt-1 text-[12px] text-danger", "{err}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_lines() {
        assert_eq!(gb(4_920_000_000), "4.9 GB");
        assert_eq!(gb(13_800_000_000), "14 GB");
        assert_eq!(summary(&json!({ "id": "ollama", "running": true, "models": [{}, {}, {}] })), "Ollama is running with 3 models");
        assert_eq!(summary(&json!({ "id": "ollama", "running": false, "installed": true })), "Ollama is installed but not running");
        assert_eq!(summary(&json!({ "id": "lmstudio", "running": false, "installed": false })), "LM Studio isn't installed");
        assert_eq!(fit_words("partly"), "Partly on the graphics card; slower");
        assert_eq!(urlencode("hf.co/a:b"), "hf.co%2Fa%3Ab");
    }
}
