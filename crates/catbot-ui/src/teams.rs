//! Settings › Team (stage 15.4): a whole team in from one Markdown file (MausBot's portable
//! format), after a look at what it would make; and the team out as one.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";

#[component]
pub fn TeamFiles() -> Element {
    // Grok Bot's exports are read at home (they're in this computer's Downloads).
    let at_home = crate::link::active().is_none();
    rsx! {
        if at_home {
            crate::grok::GrokMove {}
        }
        Import {}
        Export {}
    }
}

#[component]
fn Import() -> Element {
    let mut text = use_signal(String::new);
    let mut link = use_signal(String::new);
    let mut plan = use_signal(|| None::<Value>);
    let mut leave = use_signal(Vec::<String>::new);
    // Replacing a Chief you have is opt-in; with none, the team's own Chief is taken.
    let state = use_context::<Signal<State>>();
    let chief = use_signal(|| !state.peek().bots.iter().any(|b| b.chief));
    let mut busy = use_signal(|| false);
    let mut said = use_signal(|| None::<String>);
    let mut problem = use_signal(|| None::<String>);
    let look = move |_| {
        busy.set(true);
        problem.set(None);
        said.set(None);
        let body = if text.peek().trim().is_empty() { json!({ "url": link.peek().trim() }) } else { json!({ "text": *text.peek() }) };
        spawn(async move {
            match api::post("/api/teams/preview", &body).await {
                Ok(answer) => {
                    text.set(answer["text"].as_str().unwrap_or_default().to_string());
                    leave.set(Vec::new());
                    plan.set(Some(answer["plan"].clone()));
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let make = move |_| {
        busy.set(true);
        let body = json!({ "text": *text.peek(), "leaveOut": *leave.peek(), "makeChief": chief() });
        spawn(async move {
            match api::post("/api/teams/import", &body).await {
                Ok(answer) => {
                    let bots: Vec<&str> = answer["bots"].as_array().map(|b| b.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                    let mut line = format!("Made {}.", bots.join(", "));
                    if let Some(rooms) = answer["channels"].as_array().filter(|r| !r.is_empty()) {
                        line.push_str(&format!(" Channels: {}.", rooms.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
                    }
                    if let Some(name) = answer["chief"].as_str() {
                        line.push_str(&format!(" {name} is now your Chief of Staff."));
                    }
                    said.set(Some(line));
                    plan.set(None);
                    text.set(String::new());
                    link.set(String::new());
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let pick = move |e: FormEvent| {
        let Some(file) = e.files().into_iter().next() else { return };
        spawn(async move {
            match file.read_bytes().await {
                Ok(bytes) => text.set(String::from_utf8_lossy(&bytes).to_string()),
                Err(err) => problem.set(Some(format!("couldn't read that file: {err}"))),
            }
        });
        document::eval(r#"const input = document.getElementById("team-file"); if (input) input.value = "";"#);
    };
    rsx! {
        section { class: "flex flex-col gap-3", "data-team-import": true,
            h2 { class: HEADING, "Import a team" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "One Markdown file holds a whole team: its bots, their roles, the channels they share and who's Chief of Staff. It's MausBot's format, so teams from BotMRR and MausBot work too. You'll see what it would make before anything is made."
            }
            if plan().is_none() {
                textarea { class: "{FIELD} resize-y font-mono text-[12px]", rows: 5, placeholder: "Paste a team file here", "data-team-text": true, value: "{text}", oninput: move |e| text.set(e.value()) }
                div { class: "flex flex-wrap items-center gap-2",
                    input { id: "team-file", r#type: "file", accept: ".md,.markdown,.json,text/markdown", class: "hidden", onchange: pick }
                    button { r#type: "button", class: BUTTON, onclick: move |_| { document::eval(r#"document.getElementById("team-file")?.click();"#); }, "Choose a file…" }
                    span { class: "text-[12px] text-ink-tertiary", "or a link:" }
                    input { class: "{FIELD} min-w-0 flex-1", placeholder: "https://github.com/…/team.md", "data-team-link": true, value: "{link}", oninput: move |e| link.set(e.value()) }
                }
                button {
                    r#type: "button",
                    "data-team-look": true,
                    class: "self-start {PRIMARY}",
                    disabled: busy() || (text.read().trim().is_empty() && link.read().trim().is_empty()),
                    onclick: look,
                    "Look at it"
                }
            }
            if let Some(found) = plan() {
                Review { plan: found, leave, chief }
                div { class: "flex gap-2",
                    button { r#type: "button", "data-team-make": true, class: PRIMARY, disabled: busy() || leave.read().len() >= plan().map(|p| p["bots"].as_array().map(Vec::len).unwrap_or(0)).unwrap_or(0), onclick: make, "Make this team" }
                    button { r#type: "button", class: BUTTON, onclick: move |_| plan.set(None), "Back" }
                }
            }
            if let Some(line) = said() {
                p { class: "text-[13px] text-ink", "data-team-made": true, "{line}" }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}

/// What a team file would make: its bots (each can be left out), channels, Chief, and what isn't brought in.
#[component]
fn Review(plan: Value, leave: Signal<Vec<String>>, chief: Signal<bool>) -> Element {
    let name = plan["name"].as_str().unwrap_or("This team").to_string();
    let tagline = plan["tagline"].as_str().unwrap_or_default().to_string();
    let bots = plan["bots"].as_array().cloned().unwrap_or_default();
    let rooms = plan["channels"].as_array().cloned().unwrap_or_default();
    let left_out: Vec<String> = plan["leftOut"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let chief_key = plan["chief"]["key"].as_str().map(str::to_string);
    let chief_name = plan["chief"]["name"].as_str().unwrap_or_default().to_string();
    let chief_kept = chief_key.as_ref().is_some_and(|k| !leave.read().contains(k));
    rsx! {
        div { class: "flex flex-col gap-3 rounded-xl border border-hairline/50 p-3", "data-team-review": true,
            div {
                div { class: "text-[15px] font-semibold text-ink", "{name}" }
                if !tagline.is_empty() {
                    div { class: "text-[13px] text-ink-secondary", "{tagline}" }
                }
            }
            div { class: "flex flex-col gap-1.5",
                for bot in bots {
                    {
                        let key = bot["key"].as_str().unwrap_or_default().to_string();
                        let kept = !leave.read().contains(&key);
                        let title = bot["title"].as_str().unwrap_or_default().to_string();
                        let about = bot["about"].as_str().unwrap_or_default().to_string();
                        let taken = bot["nameTaken"].as_bool().unwrap_or(false);
                        let bot_name = bot["name"].as_str().unwrap_or_default().to_string();
                        let color = bot["color"].as_str().unwrap_or("blue").to_string();
                        rsx! {
                            label { key: "{key}", class: "flex items-start gap-2 rounded-lg px-1 py-1 hover:bg-raised/50",
                                input {
                                    r#type: "checkbox",
                                    class: "mt-1",
                                    checked: kept,
                                    "data-team-bot": "{bot_name}",
                                    onchange: move |_| {
                                        let mut list = leave.write();
                                        match list.iter().position(|k| *k == key) {
                                            Some(at) => { list.remove(at); }
                                            None => list.push(key.clone()),
                                        }
                                    },
                                }
                                span { class: "mt-1.5 size-2.5 shrink-0 rounded-full", background: "var(--identity-{color})" }
                                span { class: "min-w-0",
                                    span { class: "text-[14px] font-medium text-ink", "{bot_name}" }
                                    if !title.is_empty() {
                                        span { class: "text-[13px] text-ink-secondary", " · {title}" }
                                    }
                                    if taken {
                                        span { class: "text-[12px] text-ink-tertiary", " (you have a {bot_name}; this one gets a number)" }
                                    }
                                    span { class: "block text-[12px] leading-snug text-ink-secondary", "{about}" }
                                }
                            }
                        }
                    }
                }
            }
            if !rooms.is_empty() {
                div { class: "text-[13px] text-ink",
                    for room in rooms {
                        {
                            let members: Vec<&str> = room["members"].as_array().map(|m| m.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                            let lead = room["lead"].as_str().unwrap_or_default();
                            let room_name = room["name"].as_str().unwrap_or_default();
                            rsx! {
                                p { key: "{room_name}", "Channel #{room_name}: {members.join(\", \")}" if !lead.is_empty() { " (lead: {lead})" } }
                            }
                        }
                    }
                }
            }
            if chief_key.is_some() {
                label { class: "flex items-center gap-2 text-[13px] text-ink",
                    input { r#type: "checkbox", "data-team-chief": true, checked: chief() && chief_kept, disabled: !chief_kept, onchange: move |_| chief.set(!chief()) }
                    "Make {chief_name} your Chief of Staff"
                }
            }
            if !left_out.is_empty() {
                div { class: "text-[12px] text-ink-secondary",
                    "Not brought in: "
                    "{left_out.join(\"; \")}."
                }
            }
            p { class: "text-[12px] text-ink-tertiary", "{plan[\"safety\"].as_str().unwrap_or_default()}" }
        }
    }
}

#[component]
fn Export() -> Element {
    let state = use_context::<Signal<State>>();
    let mut name = use_signal(String::new);
    let mut tagline = use_signal(String::new);
    let mut author = use_signal(String::new);
    let mut left = use_signal(Vec::<String>::new);
    let mut result = use_signal(|| None::<Value>);
    let mut problem = use_signal(|| None::<String>);
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone())).collect();
    let all: Vec<String> = bots.iter().map(|(id, _)| id.clone()).collect();
    let save = move |_| {
        let ids: Vec<String> = all.iter().filter(|id| !left.peek().contains(id)).cloned().collect();
        let body = json!({ "name": name.peek().trim(), "tagline": tagline.peek().trim(), "author": author.peek().trim(), "botIds": ids, "save": true });
        spawn(async move {
            match api::post("/api/teams/export", &body).await {
                Ok(answer) => {
                    problem.set(None);
                    // In a browser away from home, the file comes down too.
                    let file = serde_json::to_string(&answer["filename"]).unwrap_or_default();
                    let text = serde_json::to_string(&answer["markdown"]).unwrap_or_default();
                    if answer["savedTo"].is_null() {
                        document::eval(&format!(
                            "const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob([{text}], {{ type: 'text/markdown' }})); a.download = {file}; a.click();"
                        ));
                    }
                    result.set(Some(answer));
                }
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    let copy = move |_| {
        if let Some(found) = result() {
            let text = serde_json::to_string(&found["markdown"]).unwrap_or_default();
            document::eval(&format!("navigator.clipboard?.writeText({text});"));
        }
    };
    rsx! {
        section { class: "flex flex-col gap-3", "data-team-export": true,
            h2 { class: HEADING, "Export your team" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Saves the bots you pick, the channels they share and your Chief of Staff as one team file anyone can import. Chats, keys, AIs and approval levels never go in, and anything that looks like a key is taken out."
            }
            input { class: FIELD, placeholder: "Team name", "data-export-name": true, value: "{name}", oninput: move |e| name.set(e.value()) }
            input { class: FIELD, placeholder: "What it's for, in one sentence", value: "{tagline}", oninput: move |e| tagline.set(e.value()) }
            input { class: FIELD, placeholder: "Your name, as its author (optional)", value: "{author}", oninput: move |e| author.set(e.value()) }
            div { class: "flex flex-wrap gap-x-4 gap-y-1",
                for (id, bot_name) in bots {
                    {
                        let kept = !left.read().contains(&id);
                        rsx! {
                            label { key: "{id}", class: "flex items-center gap-1.5 text-[13px] text-ink",
                                input { r#type: "checkbox", checked: kept, "data-export-bot": "{bot_name}",
                                    onchange: move |_| {
                                        let mut list = left.write();
                                        match list.iter().position(|k| *k == id) {
                                            Some(at) => { list.remove(at); }
                                            None => list.push(id.clone()),
                                        }
                                    },
                                }
                                "{bot_name}"
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-2",
                button { r#type: "button", class: PRIMARY, "data-export-save": true, onclick: save, "Save the team file" }
                if result().is_some() {
                    button { r#type: "button", class: BUTTON, onclick: copy, "Copy it" }
                }
            }
            if let Some(found) = result() {
                p { class: "text-[12px] text-ink-secondary", "data-export-saved": true,
                    match found["savedTo"].as_str() {
                        Some(path) => format!("Saved to {path}"),
                        None => format!("Downloaded as {}", found["filename"].as_str().unwrap_or_default()),
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
