//! Skills (stage 13.8): the library in Settings › Skills, each bot's choice in its settings, and
//! `/name` in the composer.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::Bot;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";

/// The library's skills: (name, description).
fn use_library() -> Signal<Vec<(String, String, Vec<String>)>> {
    let rev = use_context::<crate::live::Revisions>();
    let mut found = use_signal(Vec::new);
    use_effect(move || {
        let _ = (rev.skills)();
        spawn(async move {
            if let Ok(answer) = api::get("/api/skills").await {
                found.set(
                    answer["skills"]
                        .as_array()
                        .map(|list| {
                            list.iter()
                                .map(|s| {
                                    let bots = s["bots"].as_array().map(|b| b.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                                    (s["name"].as_str().unwrap_or_default().to_string(), s["description"].as_str().unwrap_or_default().to_string(), bots)
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                );
            }
        });
    });
    found
}

/// Settings › Skills: what's in the library, adding one, and taking one out.
#[component]
pub fn SkillsLibrary() -> Element {
    let library = use_library();
    let mut text = use_signal(String::new);
    let mut link = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut when = use_signal(String::new);
    let mut how = use_signal(String::new);
    let mut writing = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut said = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| None::<String>);
    let add = move |body: Value| {
        spawn(async move {
            match api::post("/api/skills", &body).await {
                Ok(answer) => {
                    problem.set(None);
                    said.set(Some(format!("Added {}. Turn it on for a bot in that bot's settings.", answer["name"].as_str().unwrap_or("it"))));
                    text.set(String::new());
                    link.set(String::new());
                    name.set(String::new());
                    when.set(String::new());
                    how.set(String::new());
                    writing.set(false);
                }
                Err(err) => problem.set(Some(err)),
            }
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
        document::eval(r#"const input = document.getElementById("skill-file"); if (input) input.value = "";"#);
    };
    let remove = move |skill: String| {
        spawn(async move {
            if let Err(err) = api::call("DELETE", &format!("/api/skills/{skill}"), None).await {
                problem.set(Some(err));
            }
            confirm.set(None);
        });
    };
    rsx! {
        section { class: "flex flex-col gap-3", "data-skills-library": true,
            h2 { class: HEADING, "Skills" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "A skill is a set of instructions for doing one kind of job well. They're shared by all your bots: each bot picks the ones it uses in its settings, and you can run one with /name in the message box. They're the open Agent Skills format (a SKILL.md), so skills made for Claude Code or MausBot work here."
            }
            if library.read().is_empty() {
                p { class: "text-[13px] text-ink-tertiary", "No skills yet." }
            }
            for (skill, description, users) in library() {
                div { key: "{skill}", class: "flex items-start gap-2 rounded-lg border border-hairline/40 px-3 py-2", "data-skill": "{skill}",
                    div { class: "min-w-0 flex-1",
                        div { class: "font-mono text-[13px] font-semibold text-ink", "/{skill}" }
                        div { class: "text-[12px] leading-snug text-ink-secondary", "{description}" }
                        div { class: "text-[11px] text-ink-tertiary", {if users.is_empty() { "No bot uses it yet".to_string() } else { format!("Used by {}", users.join(", ")) }} }
                    }
                    if confirm().as_deref() == Some(skill.as_str()) {
                        button { r#type: "button", class: "rounded-lg bg-danger px-2.5 py-1 text-[12px] font-medium text-white", onclick: { let skill = skill.clone(); move |_| remove(skill.clone()) }, "Remove" }
                        button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised", onclick: move |_| confirm.set(None), "Keep" }
                    } else {
                        button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-danger", "aria-label": "Remove {skill}", onclick: { let skill = skill.clone(); move |_| confirm.set(Some(skill.clone())) }, "Remove…" }
                    }
                }
            }
            h2 { class: "mt-2 {HEADING}", "Add a skill" }
            if writing() {
                input { class: FIELD, placeholder: "name, like bed-plan (lowercase, hyphens)", "data-skill-name": true, value: "{name}", oninput: move |e| name.set(e.value()) }
                input { class: FIELD, placeholder: "When to use it, in one sentence", "data-skill-when": true, value: "{when}", oninput: move |e| when.set(e.value()) }
                textarea { class: "{FIELD} resize-y", rows: 6, placeholder: "The instructions", "data-skill-how": true, value: "{how}", oninput: move |e| how.set(e.value()) }
                div { class: "flex gap-2",
                    button {
                        r#type: "button",
                        class: PRIMARY,
                        "data-skill-save": true,
                        disabled: name.read().trim().is_empty() || when.read().trim().is_empty(),
                        onclick: move |_| {
                            let description = when.peek().trim().replace('\n', " ");
                            let file = format!("---\nname: {}\ndescription: {}\n---\n\n{}\n", name.peek().trim(), serde_json::to_string(&description).unwrap_or_default(), how.peek().trim());
                            add(json!({ "text": file }));
                        },
                        "Save the skill"
                    }
                    button { r#type: "button", class: BUTTON, onclick: move |_| writing.set(false), "Cancel" }
                }
            } else {
                textarea { class: "{FIELD} resize-y font-mono text-[12px]", rows: 4, placeholder: "Paste a SKILL.md", "data-skill-text": true, value: "{text}", oninput: move |e| text.set(e.value()) }
                div { class: "flex flex-wrap items-center gap-2",
                    input { id: "skill-file", r#type: "file", accept: ".md,text/markdown", class: "hidden", onchange: pick }
                    button { r#type: "button", class: BUTTON, onclick: move |_| { document::eval(r#"document.getElementById("skill-file")?.click();"#); }, "Choose a file…" }
                    span { class: "text-[12px] text-ink-tertiary", "or a link:" }
                    input { class: "{FIELD} min-w-0 flex-1", placeholder: "https://github.com/…/skills/name", value: "{link}", oninput: move |e| link.set(e.value()) }
                }
                div { class: "flex flex-wrap gap-2",
                    button {
                        r#type: "button",
                        class: PRIMARY,
                        "data-skill-add": true,
                        disabled: text.read().trim().is_empty() && link.read().trim().is_empty(),
                        onclick: move |_| {
                            let body = if text.peek().trim().is_empty() { json!({ "url": link.peek().trim() }) } else { json!({ "text": *text.peek() }) };
                            add(body);
                        },
                        "Add it"
                    }
                    button { r#type: "button", class: BUTTON, "data-skill-write": true, onclick: move |_| writing.set(true), "Write one" }
                }
            }
            if let Some(line) = said() {
                p { class: "text-[13px] text-ink", "{line}" }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}

/// In a bot's settings: which of the library's skills it uses.
#[component]
pub fn BotSkills(bot: Bot, saving: bool, change: Callback<Value>) -> Element {
    let library = use_library();
    if library.read().is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "mt-4 block text-[12px] font-medium text-ink-secondary", "Skills" }
        div { class: "mt-1 flex flex-col gap-1", "data-bot-skills": true,
            for (skill, description, _) in library() {
                {
                    let on = bot.skills.contains(&skill);
                    let others = bot.skills.clone();
                    let name = skill.clone();
                    rsx! {
                        label { key: "{skill}", class: "flex items-start gap-2 rounded-lg px-2 py-1 hover:bg-raised/50", title: "{description}",
                            input {
                                r#type: "checkbox",
                                class: "mt-1",
                                checked: on,
                                disabled: saving,
                                "data-bot-skill": "{skill}",
                                onchange: move |_| {
                                    let mut next: Vec<String> = others.iter().filter(|s| **s != name).cloned().collect();
                                    if !on {
                                        next.push(name.clone());
                                    }
                                    change.call(json!({ "skills": next }));
                                },
                            }
                            span { class: "min-w-0",
                                span { class: "font-mono text-[13px] text-ink", "/{skill}" }
                                span { class: "block truncate text-[12px] text-ink-secondary", "{description}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
