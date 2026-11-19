//! Settings › Team › Move a bot from Grok Bot (stage 18.2). Grok Bot has no export button, so its
//! bot writes its own: copy Catbot's prompt into its chat, bring back what it wrote (Catbot looks
//! in Downloads while this is open, or paste it, or choose the file), look at what would be made,
//! and bring the bots over.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const STEP: &str = "text-[14px] font-medium text-ink";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";

#[component]
pub fn GrokMove() -> Element {
    let mut open = use_signal(|| false);
    let mut look = use_signal(|| None::<Value>);
    let mut picked = use_signal(Vec::<String>::new);
    let mut text = use_signal(String::new);
    let mut plan = use_signal(|| None::<Value>);
    let mut leave = use_signal(Vec::<String>::new);
    let mut copied = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut said = use_signal(|| None::<String>);
    let mut problem = use_signal(|| None::<String>);

    // Downloads is looked at only while this is open: on opening, and on "Look again".
    let refresh = move || {
        spawn(async move {
            match api::get("/api/grok-move").await {
                Ok(found) => {
                    // The newest export is picked to start with.
                    if picked.peek().is_empty()
                        && let Some(name) = found["files"][0]["name"].as_str()
                    {
                        picked.set(vec![name.to_string()]);
                    }
                    look.set(Some(found));
                }
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    let body = move || json!({ "text": *text.peek(), "files": *picked.peek() });
    let preview = move |_| {
        busy.set(true);
        problem.set(None);
        said.set(None);
        let body = body();
        spawn(async move {
            match api::post("/api/grok-move/preview", &body).await {
                Ok(answer) => {
                    leave.set(Vec::new());
                    plan.set(Some(answer["plan"].clone()));
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let bring = move |_| {
        busy.set(true);
        let mut body = body();
        body["leaveOut"] = json!(*leave.peek());
        spawn(async move {
            match api::post("/api/grok-move/import", &body).await {
                Ok(answer) => {
                    let lines: Vec<String> = answer["bots"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|b| {
                            let name = b["name"].as_str().unwrap_or("A bot");
                            let skills = b["skills"].as_array().map(Vec::len).unwrap_or(0);
                            let schedules = b["schedules"].as_u64().unwrap_or(0);
                            let mut line = format!("{name} is here");
                            let mut parts = Vec::new();
                            if skills > 0 {
                                parts.push(format!("{skills} skill{} in your library, off", if skills == 1 { "" } else { "s" }));
                            }
                            if schedules > 0 {
                                parts.push(format!("{schedules} paused trigger{}", if schedules == 1 { "" } else { "s" }));
                            }
                            if b["noted"] == true {
                                parts.push("notes in its memory".into());
                            }
                            if !parts.is_empty() {
                                line.push_str(&format!(" ({})", parts.join(", ")));
                            }
                            line
                        })
                        .collect();
                    said.set(Some(format!("{}. Switch on its skills in its settings and its triggers in Triggers when you're ready.", lines.join("; "))));
                    plan.set(None);
                    text.set(String::new());
                    picked.set(Vec::new());
                    refresh();
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let choose = move |e: FormEvent| {
        let Some(file) = e.files().into_iter().next() else { return };
        spawn(async move {
            match file.read_bytes().await {
                Ok(bytes) => text.set(String::from_utf8_lossy(&bytes).to_string()),
                Err(err) => problem.set(Some(format!("couldn't read that file: {err}"))),
            }
        });
        document::eval(r#"const input = document.getElementById("grok-file"); if (input) input.value = "";"#);
    };
    let copy = move |_| {
        if let Some(found) = look() {
            let prompt = serde_json::to_string(&found["prompt"]).unwrap_or_default();
            document::eval(&format!("navigator.clipboard?.writeText({prompt});"));
            copied.set(true);
        }
    };

    let files: Vec<(String, String)> = look()
        .and_then(|l| l["files"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|f| (f["name"].as_str().unwrap_or_default().to_string(), f["modified"].as_str().unwrap_or_default().get(..16).unwrap_or_default().replace('T', " ")))
        .collect();
    let folder = look().and_then(|l| l["folder"].as_str().map(str::to_string)).unwrap_or_else(|| "Downloads".into());
    let prompt = look().and_then(|l| l["prompt"].as_str().map(str::to_string)).unwrap_or_default();
    let nothing = text.read().trim().is_empty() && picked.read().is_empty();

    rsx! {
        section { class: "flex flex-col gap-3", "data-grok-move": true,
            h2 { class: HEADING, "Move a bot from Grok Bot" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Grok Bot has no export button, so its bot writes out its own setup: its instructions, what it remembers, its skills, routines and apps. Catbot makes a bot from that, after you've looked. Keys and passwords never come over."
            }
            if !open() {
                button { r#type: "button", class: "self-start {BUTTON}", "data-grok-start": true,
                    onclick: move |_| {
                        open.set(true);
                        refresh();
                    },
                    "Start"
                }
            } else if let Some(found) = plan() {
                Review { plan: found, leave }
                div { class: "flex gap-2",
                    button { r#type: "button", "data-grok-bring": true, class: PRIMARY,
                        disabled: busy() || leave.read().len() >= plan().map(|p| p["bots"].as_array().map(Vec::len).unwrap_or(0)).unwrap_or(0),
                        onclick: bring,
                        "Bring them over"
                    }
                    button { r#type: "button", class: BUTTON, onclick: move |_| plan.set(None), "Back" }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    h3 { class: STEP, "1. Copy this into your Grok Bot's chat" }
                    p { class: "text-[12px] text-ink-tertiary", "Once for each bot you're moving. It asks the bot to save a file named grok-bot-export-… in your Downloads, or to answer with it." }
                    textarea { class: "{FIELD} resize-y font-mono text-[11px]", rows: 4, readonly: true, "data-grok-prompt": true, value: "{prompt}" }
                    button { r#type: "button", class: "self-start {BUTTON}", "data-grok-copy": true, disabled: prompt.is_empty(), onclick: copy,
                        if copied() { "Copied" } else { "Copy the prompt" }
                    }
                }
                div { class: "flex flex-col gap-2",
                    h3 { class: STEP, "2. Bring back what it wrote" }
                    if files.is_empty() {
                        p { class: "text-[12px] text-ink-secondary", "No export in {folder} yet." }
                    } else {
                        p { class: "text-[12px] text-ink-secondary", "Found in {folder}:" }
                        for (name, when) in files {
                            {
                                let ticked = picked.read().contains(&name);
                                rsx! {
                                    label { key: "{name}", class: "flex items-center gap-2 text-[13px] text-ink",
                                        input { r#type: "checkbox", checked: ticked, "data-grok-file": "{name}",
                                            onchange: move |_| {
                                                let mut list = picked.write();
                                                match list.iter().position(|n| *n == name) {
                                                    Some(at) => { list.remove(at); }
                                                    None => list.push(name.clone()),
                                                }
                                            },
                                        }
                                        span { class: "font-mono text-[12px]", "{name}" }
                                        span { class: "text-[12px] text-ink-tertiary", "{when}" }
                                    }
                                }
                            }
                        }
                    }
                    button { r#type: "button", class: "self-start {BUTTON}", "data-grok-again": true, onclick: move |_| refresh(), "Look again" }
                    textarea { class: "{FIELD} resize-y font-mono text-[12px]", rows: 4, "data-grok-text": true, placeholder: "Or paste its reply here", value: "{text}", oninput: move |e| text.set(e.value()) }
                    div { class: "flex items-center gap-2",
                        input { id: "grok-file", r#type: "file", accept: ".json,.md,.txt,application/json,text/plain", class: "hidden", onchange: choose }
                        button { r#type: "button", class: BUTTON, onclick: move |_| { document::eval(r#"document.getElementById("grok-file")?.click();"#); }, "Choose a file…" }
                    }
                }
                div { class: "flex gap-2",
                    button { r#type: "button", class: PRIMARY, "data-grok-look": true, disabled: busy() || nothing, onclick: preview, "Look at it" }
                    button { r#type: "button", class: BUTTON, onclick: move |_| open.set(false), "Close" }
                }
            }
            if let Some(line) = said() {
                p { class: "text-[13px] text-ink", "data-grok-made": true, "{line}" }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What would be made: each bot (each can be left out) and what comes with it.
#[component]
fn Review(plan: Value, leave: Signal<Vec<String>>) -> Element {
    let bots = plan["bots"].as_array().cloned().unwrap_or_default();
    let names = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    rsx! {
        div { class: "flex flex-col gap-3 rounded-xl border border-hairline/50 p-3", "data-grok-review": true,
            for bot in bots {
                {
                    let key = bot["key"].as_str().unwrap_or_default().to_string();
                    let kept = !leave.read().contains(&key);
                    let name = bot["name"].as_str().unwrap_or_default().to_string();
                    let title = bot["title"].as_str().unwrap_or_default().to_string();
                    let about = bot["about"].as_str().unwrap_or_default().to_string();
                    let color = bot["color"].as_str().unwrap_or("blue").to_string();
                    let taken = bot["nameTaken"].as_bool().unwrap_or(false);
                    let skills = names(&bot["skills"]);
                    let schedules: Vec<String> = bot["schedules"].as_array().into_iter().flatten().map(|s| format!("{} ({})", s["name"].as_str().unwrap_or("routine"), s["when"].as_str().unwrap_or("?"))).collect();
                    let events = names(&bot["events"]);
                    let apps = names(&bot["apps"]);
                    let (memories, facts) = (bot["memories"].as_u64().unwrap_or(0), bot["facts"].as_u64().unwrap_or(0));
                    let noted = match (memories, facts) {
                        (0, 0) => String::new(),
                        (m, 0) => count(m, "thing it remembers", "things it remembers"),
                        (0, f) => count(f, "fact about you", "facts about you"),
                        (m, f) => format!("{} and {}", count(m, "thing it remembers", "things it remembers"), count(f, "fact about you", "facts about you")),
                    };
                    rsx! {
                        div { key: "{key}", class: "flex items-start gap-2",
                            input { r#type: "checkbox", class: "mt-1", checked: kept, "data-grok-bot": "{name}",
                                onchange: move |_| {
                                    let mut list = leave.write();
                                    match list.iter().position(|k| *k == key) {
                                        Some(at) => { list.remove(at); }
                                        None => list.push(key.clone()),
                                    }
                                },
                            }
                            span { class: "mt-1.5 size-2.5 shrink-0 rounded-full", background: "var(--identity-{color})" }
                            div { class: "min-w-0 text-[12px] leading-snug text-ink-secondary",
                                div {
                                    span { class: "text-[14px] font-medium text-ink", "{name}" }
                                    if !title.is_empty() {
                                        span { class: "text-[13px]", " · {title}" }
                                    }
                                    if taken {
                                        span { class: "text-ink-tertiary", " (you have a {name}; this one gets a number)" }
                                    }
                                }
                                if !about.is_empty() {
                                    p { "{about}" }
                                }
                                if !noted.is_empty() {
                                    p { "Its memory note gets {noted}." }
                                }
                                if !skills.is_empty() {
                                    p { "Skills, into your library switched off: {skills}" }
                                }
                                if !schedules.is_empty() {
                                    p { "Triggers, paused: {schedules.join(\", \")}" }
                                }
                                if !events.is_empty() {
                                    p { "Routines on an event, written into its notes to set up again: {events}" }
                                }
                                if !apps.is_empty() {
                                    p { "Apps it used, to connect again in Settings › Apps: {apps}" }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "text-[12px] text-ink-tertiary", "{plan[\"safety\"].as_str().unwrap_or_default()}" }
        }
    }
}
