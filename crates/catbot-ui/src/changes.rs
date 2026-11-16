//! The row after a turn that changed the bot's folder: what changed, the changes themselves, and
//! undoing them (`server/changes.rs`). Moo chose "see changes and undo".

use dioxus::prelude::*;
use serde_json::Value;

use crate::api;

/// One file a turn changed.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    pub path: String,
    /// "added", "changed" or "deleted".
    pub how: String,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

pub fn files(wire: &Value) -> Vec<FileChange> {
    wire.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|f| {
                    Some(FileChange {
                        path: f["path"].as_str()?.to_string(),
                        how: f["how"].as_str().unwrap_or("changed").to_string(),
                        added: f["added"].as_u64(),
                        removed: f["removed"].as_u64(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Closed,
    Files,
    Patch,
}

#[component]
pub fn ChangesRow(thread: String, message: String, summary: String, files: Vec<FileChange>, undone: bool, working: bool) -> Element {
    let mut open = use_signal(|| Open::Closed);
    let mut patch = use_signal(|| None::<String>);
    let mut confirming = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let added: u64 = files.iter().filter_map(|f| f.added).sum();
    let removed: u64 = files.iter().filter_map(|f| f.removed).sum();
    let base = format!("/api/threads/{thread}/changes/{message}");

    let patch_path = base.clone();
    let show_patch = move |_| {
        open.set(Open::Patch);
        if patch.peek().is_none() {
            let path = patch_path.clone();
            spawn(async move {
                match api::get(&path).await {
                    Ok(answer) => patch.set(Some(answer["patch"].as_str().unwrap_or_default().to_string())),
                    Err(err) => problem.set(Some(err)),
                }
            });
        }
    };
    let act = move |action: &'static str| {
        let path = format!("{base}/{action}");
        confirming.set(false);
        spawn(async move {
            if let Err(err) = api::post(&path, &serde_json::json!({})).await {
                problem.set(Some(err));
            }
        });
    };
    let mut act_undo = act.clone();
    let mut act_redo = act;

    rsx! {
        div { class: "flex w-full justify-start", "data-changes": "{message}",
            div { class: "w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] rounded-xl border border-hairline/40 bg-panel text-[13px]",
                button {
                    r#type: "button",
                    "data-changes-toggle": true,
                    "aria-expanded": "{open() != Open::Closed}",
                    class: "flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-raised/60 rounded-xl",
                    onclick: move |_| open.set(if open() == Open::Closed { Open::Files } else { Open::Closed }),
                    // Lucide `FilePen` (ISC).
                    svg { "aria-hidden": "true", class: "shrink-0 text-ink-secondary", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M12.5 22H18a2 2 0 0 0 2-2V7l-5-5H6a2 2 0 0 0-2 2v9.5" }
                        path { d: "M14 2v4a2 2 0 0 0 2 2h4" }
                        path { d: "M13.378 15.626a1 1 0 1 0-3.004-3.004l-5.01 5.012a2 2 0 0 0-.506.854l-.837 2.87a.5.5 0 0 0 .62.62l2.87-.837a2 2 0 0 0 .854-.506z" }
                    }
                    span { class: if undone { "min-w-0 flex-1 truncate text-ink-tertiary line-through" } else { "min-w-0 flex-1 truncate text-ink" }, "{summary}" }
                    if added > 0 { span { class: "shrink-0 tabular-nums text-success", "+{added}" } }
                    if removed > 0 { span { class: "shrink-0 tabular-nums text-danger", "−{removed}" } }
                    if undone { span { "data-changes-undone": true, class: "shrink-0 rounded-full bg-raised px-2 py-0.5 text-[11px] text-ink-secondary", "Undone" } }
                }
                if open() != Open::Closed {
                    div { class: "flex flex-col gap-2 border-t border-hairline/40 px-3 py-2",
                        ul { class: "flex flex-col gap-0.5",
                            for file in files.iter().cloned() {
                                li { key: "{file.path}", class: "flex items-center gap-2",
                                    span { class: match file.how.as_str() { "added" => "w-16 shrink-0 text-success", "deleted" => "w-16 shrink-0 text-danger", _ => "w-16 shrink-0 text-ink-secondary" }, "{file.how}" }
                                    span { class: "min-w-0 flex-1 truncate font-mono text-[12px] text-ink", "{file.path}" }
                                    if let Some(n) = file.added.filter(|n| *n > 0) { span { class: "tabular-nums text-success", "+{n}" } }
                                    if let Some(n) = file.removed.filter(|n| *n > 0) { span { class: "tabular-nums text-danger", "−{n}" } }
                                }
                            }
                        }
                        if open() == Open::Patch {
                            match patch() {
                                Some(text) => rsx! { Patch { text } },
                                None => rsx! { div { class: "text-ink-tertiary", "Loading the changes…" } },
                            }
                        }
                        if let Some(err) = problem() {
                            div { role: "alert", class: "text-danger", "{err}" }
                        }
                        div { class: "flex flex-wrap items-center gap-2",
                            if open() != Open::Patch {
                                button { r#type: "button", "data-show-patch": true, class: "rounded-lg bg-raised px-2.5 py-1 text-ink hover:bg-control", onclick: show_patch, "Show changes" }
                            }
                            if undone {
                                button {
                                    r#type: "button",
                                    "data-redo": true,
                                    disabled: working,
                                    class: "rounded-lg bg-raised px-2.5 py-1 text-ink hover:bg-control disabled:opacity-40",
                                    onclick: move |_| act_redo("redo"),
                                    "Put them back"
                                }
                            } else if confirming() {
                                span { class: "text-ink-secondary", "The folder goes back to how it was before this turn, and any later changes go too." }
                                button { r#type: "button", "data-undo-yes": true, disabled: working, class: "rounded-lg bg-danger px-2.5 py-1 font-semibold text-white disabled:opacity-40", onclick: move |_| act_undo("undo"), "Undo" }
                                button { r#type: "button", class: "rounded-lg px-2.5 py-1 text-ink-secondary hover:bg-raised", onclick: move |_| confirming.set(false), "Keep" }
                            } else {
                                button {
                                    r#type: "button",
                                    "data-undo": true,
                                    disabled: working,
                                    title: if working { "Wait for the bot to finish" } else { "Put the bot's folder back as it was before this turn" },
                                    class: "rounded-lg bg-raised px-2.5 py-1 text-ink hover:bg-control disabled:opacity-40",
                                    onclick: move |_| confirming.set(true),
                                    "Undo these changes"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A patch, its added and removed lines coloured.
#[component]
fn Patch(text: String) -> Element {
    let lines: Vec<(String, &'static str)> = text
        .lines()
        .map(|line| {
            let tone = if line.starts_with("diff --git") {
                "font-semibold text-ink"
            } else if line.starts_with("+++") || line.starts_with("---") || line.starts_with("index ") || line.starts_with("new file") || line.starts_with("deleted file") {
                "text-ink-tertiary"
            } else if line.starts_with('+') {
                "bg-success/10 text-success"
            } else if line.starts_with('-') {
                "bg-danger/10 text-danger"
            } else if line.starts_with("@@") {
                "text-accent"
            } else {
                "text-ink-secondary"
            };
            (line.to_string(), tone)
        })
        .collect();
    rsx! {
        pre { "data-patch": true, class: "max-h-[360px] overflow-auto rounded-lg bg-raised p-2 font-mono text-[12px] leading-[18px]",
            for (i, (line, tone)) in lines.into_iter().enumerate() {
                div { key: "{i}", class: "whitespace-pre {tone}", "{line}" }
            }
        }
    }
}
