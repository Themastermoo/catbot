//! The first run (stage 29.3): a new Catbot, with no bots yet, opens on a short welcome instead
//! of an empty window. Moo's choice, 2026-11-25: "Guided setup".
//!
//! 1. **Pick the AI the bots think with**:
//!    - Claude through Claude Code, with a Claude account;
//!    - an API key (Anthropic, OpenAI, Google Gemini, xAI or OpenRouter);
//!    - a model on this computer, with Ollama.
//!
//!    Each shows what it still needs, using the same setup boxes as Settings › AI (install and
//!    sign-in commands with Copy and Open in Terminal, a key box, Ollama's models).
//! 2. **Check it**: Catbot's free check (`POST /api/ai-trouble/:id/check`), which asks the AI for
//!    nothing that costs anything.
//! 3. **Make the first bot**: a name and, if they like, what it's for. Its chat opens.
//!
//! "I'll set it up myself" goes to the ordinary empty window, which offers the welcome again.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;
use crate::settings::Ui;

/// The API keys offered first; the rest are in Settings › AI.
const KEYS: [(&str, &str); 5] = [("anthropic", "Anthropic"), ("openai", "OpenAI"), ("google", "Google Gemini"), ("xai", "xAI (Grok)"), ("openrouter", "OpenRouter")];

/// Whether the welcome was put aside ("I'll set it up myself"); "Set up Catbot" brings it back.
#[derive(Clone, Copy)]
pub struct Skipped(pub Signal<bool>);

#[derive(Clone, Copy, PartialEq)]
enum Way {
    Claude,
    Key,
    Local,
}

#[component]
pub fn Welcome(on_skip: EventHandler<()>) -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let mut engines = use_signal(Vec::<Value>::new);
    let mut asked = use_signal(|| 0u32);
    let phone = crate::link::is_phone();
    let mut way = use_signal(|| if phone { Way::Key } else { Way::Claude });
    let mut provider = use_signal(|| "anthropic".to_string());
    let mut checking = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    // Set once the chosen AI passed its check: the engine to make the first bot on.
    let mut ready_on = use_signal(|| None::<String>);
    use_effect(move || {
        let _ = asked();
        let _ = (rev.engines)();
        spawn(async move {
            if let Ok(found) = api::get("/api/engines").await {
                engines.set(found["engines"].as_array().cloned().unwrap_or_default());
            }
        });
    });
    let find = move |id: &str| engines.read().iter().find(|e| e["id"] == id).cloned();
    // The engine the chosen way would use now.
    let chosen = move || -> Option<String> {
        match way() {
            Way::Claude => Some("claude".to_string()),
            Way::Key => Some(provider()),
            Way::Local => ["ollama", "lmstudio"].into_iter().find(|id| find(id).is_some_and(|e| e["ready"] == true)).map(str::to_string),
        }
    };
    let chosen_ready = chosen().and_then(|id| find(&id)).is_some_and(|e| e["ready"] == true);

    if let Some(engine) = ready_on() {
        let name = find(&engine).and_then(|e| e["name"].as_str().map(str::to_string)).unwrap_or(engine.clone());
        return rsx! { FirstBot { engine, engine_name: name, on_back: move |_| ready_on.set(None) } };
    }

    let check = move |_| {
        let Some(id) = chosen() else { return };
        checking.set(true);
        problem.set(None);
        spawn(async move {
            match api::post(&format!("/api/ai-trouble/{id}/check"), &json!({})).await {
                Ok(answer) if answer["ok"] == true => ready_on.set(Some(id)),
                Ok(answer) => problem.set(Some(answer["problem"].as_str().unwrap_or("It didn't answer the check.").to_string())),
                Err(err) => problem.set(Some(err)),
            }
            checking.set(false);
            *asked.write() += 1;
        });
    };

    let card = |on: bool| if on { "flex w-full items-start gap-3 rounded-xl bg-panel p-3 text-left ring-2 ring-accent" } else { "flex w-full items-start gap-3 rounded-xl bg-panel p-3 text-left ring-1 ring-hairline hover:ring-ink-tertiary" };
    let claude = find("claude");
    let claude_line = match &claude {
        Some(e) if e["ready"] == true => "Ready on this computer.",
        Some(e) if e["installed"] == false => "Needs Claude Code installed.",
        Some(_) => "Needs a sign-in.",
        None => "",
    };
    rsx! {
        div { "data-welcome": true, class: "flex h-full w-full justify-center overflow-y-auto bg-app px-4 py-10 text-ink",
            div { class: "w-full max-w-[560px]",
                crate::mascots::BotMark { color: "teal".to_string(), initial: "C".to_string(), size: 56, mark: "robot-head".to_string() }
                h1 { class: "mt-4 text-[24px] font-semibold", "Welcome to Catbot" }
                p { class: "mt-1 text-[14px] leading-relaxed text-ink-secondary",
                    "Catbot is a team of AI bots you chat with. First, pick the AI they think with. You can add others later in Settings › AI."
                }
                div { class: "mt-6 flex flex-col gap-2",
                    if !phone && claude.is_some() {
                        button { r#type: "button", "data-welcome-way": "claude", class: card(way() == Way::Claude), onclick: move |_| { way.set(Way::Claude); problem.set(None) },
                            Choice { title: "Claude, with your Claude account", line: format!("Through Claude Code, on a Claude Pro or Max plan. {claude_line}") }
                        }
                    }
                    button { r#type: "button", "data-welcome-way": "key", class: card(way() == Way::Key), onclick: move |_| { way.set(Way::Key); problem.set(None) },
                        Choice { title: "An API key", line: "Pay as you go, with a key from Anthropic, OpenAI, Google, xAI or OpenRouter.".to_string() }
                    }
                    if !phone {
                        button { r#type: "button", "data-welcome-way": "local", class: card(way() == Way::Local), onclick: move |_| { way.set(Way::Local); problem.set(None) },
                            Choice { title: "A model on this computer", line: "Free and private, with Ollama. Best on a computer with a good graphics card.".to_string() }
                        }
                    }
                }
                div { class: "mt-4",
                    match way() {
                        Way::Claude => rsx! {
                            if let Some(engine) = claude.clone() {
                                if engine["ready"] != true {
                                    crate::engines::EngineSetup { engine }
                                }
                            }
                        },
                        Way::Key => rsx! {
                            div { class: "flex flex-wrap gap-1.5",
                                for (id, label) in KEYS {
                                    button { key: "{id}", r#type: "button", "data-welcome-provider": "{id}",
                                        class: if provider() == id { "rounded-full bg-accent px-3 py-1 text-[13px] font-medium text-accent-ink" } else { "rounded-full bg-control px-3 py-1 text-[13px] text-ink hover:bg-raised" },
                                        onclick: move |_| { provider.set(id.to_string()); problem.set(None) },
                                        "{label}"
                                    }
                                }
                            }
                            for engine in find(&provider()).into_iter() {
                                crate::engines::EngineSetup { key: "{provider}", engine }
                            }
                        },
                        Way::Local => rsx! { crate::local_ai::OnThisComputer {} },
                    }
                }
                if let Some(problem) = problem() {
                    p { role: "alert", class: "mt-3 text-[13px] text-danger", "{problem}" }
                }
                div { class: "mt-6 flex items-center gap-3",
                    button { r#type: "button", "data-welcome-next": true,
                        class: "rounded-full bg-accent px-5 py-2 text-[14px] font-medium text-accent-ink hover:brightness-110 disabled:opacity-40",
                        disabled: checking() || !chosen_ready,
                        onclick: check,
                        if checking() { "Checking…" } else { "Continue" }
                    }
                    button { r#type: "button", class: "rounded-full px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| *asked.write() += 1, "Check again" }
                    span { class: "flex-1" }
                    button { r#type: "button", "data-welcome-skip": true, class: "text-[13px] text-ink-tertiary hover:text-ink-secondary", onclick: move |_| on_skip.call(()), "I'll set it up myself" }
                }
                if !chosen_ready {
                    p { class: "mt-2 text-[12px] text-ink-tertiary", "Continue lights up once this AI is ready." }
                }
            }
        }
    }
}

#[component]
fn Choice(title: &'static str, line: String) -> Element {
    rsx! {
        span { class: "flex min-w-0 flex-col",
            span { class: "text-[15px] font-medium text-ink", "{title}" }
            span { class: "mt-0.5 text-[13px] leading-snug text-ink-secondary", "{line}" }
        }
    }
}

/// Step two: the first bot, on the AI that passed its check.
#[component]
fn FirstBot(engine: String, engine_name: String, on_back: EventHandler<()>) -> Element {
    let mut state = use_context::<Signal<State>>();
    let mut ui = use_context::<Ui>();
    let mut name = use_signal(|| "Scout".to_string());
    let mut soul = use_signal(String::new);
    let mut making = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let make = move |_| {
        let engine = engine.clone();
        making.set(true);
        problem.set(None);
        spawn(async move {
            let wanted = name.peek().trim().to_string();
            let body = json!({
                "name": if wanted.is_empty() { "Scout".to_string() } else { wanted },
                "soul": soul.peek().trim(),
                "mark": "cat-head",
                "color": "green",
                "approvalMode": "ask",
                "modelSelection": { "instanceId": engine },
            });
            match api::post("/api/bots", &body).await {
                Ok(answer) => {
                    state.write().apply(&json!({ "kind": "bot", "bot": answer["bot"] }));
                    ui.selected.set(answer["bot"]["id"].as_str().map(str::to_string));
                    ui.showing_chat.set(true);
                }
                Err(err) => problem.set(Some(format!("Couldn't make the bot: {err}"))),
            }
            making.set(false);
        });
    };
    rsx! {
        div { "data-welcome-bot": true, class: "flex h-full w-full justify-center overflow-y-auto bg-app px-4 py-10 text-ink",
            div { class: "w-full max-w-[560px]",
                p { class: "text-[13px] font-medium text-success", "✓ {engine_name} is ready" }
                h1 { class: "mt-2 text-[24px] font-semibold", "Make your first bot" }
                p { class: "mt-1 text-[14px] leading-relaxed text-ink-secondary", "Give it a name, and if you like, say what it's for. You can change both, and make more bots, any time." }
                label { class: "mt-6 block text-[13px] font-medium text-ink", "Name" }
                input { "data-welcome-name": true, class: "mt-1 w-full rounded-lg bg-panel px-3 py-2 text-[15px] text-ink ring-1 ring-hairline focus:outline-none focus:ring-2 focus:ring-accent",
                    value: "{name}", maxlength: 40, oninput: move |e| name.set(e.value())
                }
                label { class: "mt-4 block text-[13px] font-medium text-ink", "What it's for (optional)" }
                textarea { "data-welcome-soul": true, rows: 4, class: "mt-1 w-full resize-none rounded-lg bg-panel px-3 py-2 text-[14px] text-ink ring-1 ring-hairline focus:outline-none focus:ring-2 focus:ring-accent",
                    placeholder: "For example: help me plan my week, and keep answers short.",
                    value: "{soul}", oninput: move |e| soul.set(e.value())
                }
                p { class: "mt-2 text-[12px] text-ink-tertiary", "It asks before it changes anything on this computer. That's in its settings too." }
                if let Some(problem) = problem() {
                    p { role: "alert", class: "mt-3 text-[13px] text-danger", "{problem}" }
                }
                div { class: "mt-6 flex items-center gap-3",
                    button { r#type: "button", "data-welcome-make": true,
                        class: "rounded-full bg-accent px-5 py-2 text-[14px] font-medium text-accent-ink hover:brightness-110 disabled:opacity-40",
                        disabled: making(), onclick: make,
                        if making() { "Making it…" } else { "Make it and start chatting" }
                    }
                    button { r#type: "button", class: "rounded-full px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| on_back.call(()), "Back" }
                }
            }
        }
    }
}
