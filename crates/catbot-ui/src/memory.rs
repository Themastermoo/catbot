//! The memory viewer: the bots' memory vault (`catbot::memory`), read-only, at home and on the
//! phone. A list of notes by folder beside the open note; `[[links]]` open the note they lead
//! to, into the Codex too. On a phone the list and the note take turns, like the bot list and a
//! chat.

use dioxus::prelude::*;
use serde_json::Value;

use crate::{api, markdown};

const ICON_BUTTON: &str = "flex size-10 shrink-0 items-center justify-center rounded-md text-ink-secondary hover:bg-control hover:text-ink";

/// Catches clicks on note links inside the open note and hands their target to Rust.
const LINK_CLICKS: &str = r#"
if (window.__catbotNoteClicks) document.removeEventListener("click", window.__catbotNoteClicks);
window.__catbotNoteClicks = (e) => {
    const a = e.target.closest && e.target.closest("[data-memory-body] a[data-note]");
    if (!a) return;
    e.preventDefault();
    dioxus.send(a.dataset.note);
};
document.addEventListener("click", window.__catbotNoteClicks);
await new Promise(() => {});
"#;

/// The folders in the list, in this order; anything else comes after.
const FOLDERS: [(&str, &str); 4] = [("", "Vault"), ("Bots", "Bots"), ("Projects", "Projects"), ("Journal", "Journal")];

#[component]
pub fn MemoryDialog(on_close: EventHandler<()>) -> Element {
    let mut overview = use_signal(|| None::<Value>);
    let mut note = use_signal(|| None::<Value>);
    let mut problem = use_signal(|| None::<String>);
    // On a phone: whether the note (rather than the list) fills the screen.
    let mut reading = use_signal(|| false);
    let mut refreshed = use_signal(|| 0u32);

    // `show`: on a phone, turn from the list to the note.
    let open = move |query: String, show: bool| {
        spawn(async move {
            match api::get(&format!("/api/memory/note?{query}")).await {
                Ok(found) => {
                    note.set(Some(found));
                    problem.set(None);
                    if show {
                        reading.set(true);
                    }
                    document::eval(r#"const el = document.querySelector("[data-memory-scroll]"); if (el) el.scrollTop = 0;"#);
                }
                Err(err) => problem.set(Some(format!("Couldn't open that note: {err}"))),
            }
        });
    };

    // The list, when the viewer opens and on Refresh; Home.md first if nothing's open.
    use_effect(move || {
        let _ = refreshed();
        spawn(async move {
            match api::get("/api/memory").await {
                Ok(answer) => {
                    let first = note.peek().is_none() && answer["notes"].as_array().is_some_and(|n| n.iter().any(|n| n["path"] == "Home.md"));
                    overview.set(Some(answer));
                    if first {
                        open("path=Home.md".into(), false);
                    }
                }
                Err(err) => problem.set(Some(format!("Couldn't load the memory vault: {err}"))),
            }
        });
    });
    // Note links: resolved from the open note.
    use_future(move || async move {
        let mut clicks = document::eval(LINK_CLICKS);
        while let Ok(target) = clicks.recv::<String>().await {
            let from = note.peek().as_ref().and_then(|n| n["path"].as_str().map(str::to_string)).unwrap_or_default();
            open(format!("link={}&from={}", markdown::encode(&markdown::decode(&target)), markdown::encode(&from)), true);
        }
    });

    let data = overview();
    let on = data.as_ref().map(|d| d["on"] == true);
    let codex = data.as_ref().is_some_and(|d| d["codex"] == true);
    let where_ = data.as_ref().and_then(|d| d["path"].as_str().map(str::to_string)).unwrap_or_default();
    let notes: Vec<Value> = data.as_ref().and_then(|d| d["notes"].as_array().cloned()).unwrap_or_default();
    let open_path = note().as_ref().and_then(|n| n["path"].as_str().map(str::to_string));
    let groups = group(&notes);
    let (list_class, note_class) = if reading() { ("max-md:hidden", "") } else { ("", "max-md:hidden") };

    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 max-md:p-0", onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                "aria-label": "Memory",
                "data-memory-dialog": true,
                class: "flex h-[85vh] w-full max-w-[980px] overflow-hidden rounded-2xl bg-panel shadow-2xl max-md:h-full max-md:rounded-none",
                onclick: move |e| e.stop_propagation(),

                // The list of notes.
                nav { "aria-label": "Notes", class: "flex w-[270px] shrink-0 flex-col border-r-[0.5px] border-edge max-md:w-full max-md:border-r-0 {list_class}",
                    div { class: "flex h-12 items-center gap-1 pl-4 pr-1",
                        span { class: "min-w-0 flex-1 text-[15px] font-semibold text-ink", "Memory" }
                        button { r#type: "button", "aria-label": "Refresh", title: "Refresh", class: ICON_BUTTON, onclick: move |_| *refreshed.write() += 1,
                            // Lucide `RefreshCw` (ISC).
                            svg { "aria-hidden": "true", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                path { d: "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" }
                                path { d: "M21 3v5h-5" }
                                path { d: "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" }
                                path { d: "M8 16H3v5" }
                            }
                        }
                        button { r#type: "button", "aria-label": "Close", class: "{ICON_BUTTON} md:hidden", onclick: move |_| on_close.call(()), CloseIcon {} }
                    }
                    div { class: "min-h-0 flex-1 overflow-y-auto px-2 pb-3",
                        if on.is_none() {
                            p { class: "px-2 py-2 text-[13px] text-ink-secondary", "Loading…" }
                        } else if on == Some(false) {
                            p { class: "px-2 py-2 text-[13px] leading-[19px] text-ink-secondary",
                                "The memory vault is off. Catbot was started with CATBOT_MEMORY=off."
                            }
                        } else {
                            if codex {
                                button {
                                    r#type: "button",
                                    "data-memory-codex": true,
                                    class: "mb-1 flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-start text-[13px] text-ink hover:bg-raised/60",
                                    onclick: move |_| open(format!("link={}&from=Home.md", markdown::encode("Codex/00 Home")), true),
                                    // Lucide `Library` (ISC).
                                    svg { "aria-hidden": "true", class: "shrink-0 text-ink-secondary", width: "15", height: "15", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                        path { d: "m16 6 4 14M12 6v14M8 8v12M4 4v16" }
                                    }
                                    "Codex"
                                }
                            }
                            for (label, items) in groups {
                                div { key: "{label}", class: "mt-2",
                                    span { class: "px-2 text-[12px] font-medium uppercase tracking-wide text-ink-secondary", "{label}" }
                                    for item in items {
                                        {
                                            let path = item["path"].as_str().unwrap_or_default().to_string();
                                            let title = item["title"].as_str().unwrap_or_default().to_string();
                                            let when = item["modified"].as_str().map(|m| crate::chat::short_when(crate::chat::parse_time(m))).unwrap_or_default();
                                            let here = open_path.as_deref() == Some(path.as_str());
                                            let shade = if here { "bg-raised" } else { "hover:bg-raised/60" };
                                            let query = format!("path={}", markdown::encode(&path));
                                            rsx! {
                                                button {
                                                    key: "{path}",
                                                    r#type: "button",
                                                    "data-memory-note": "{path}",
                                                    class: "flex w-full items-baseline gap-2 rounded-lg px-2 py-1.5 text-start {shade}",
                                                    onclick: move |_| open(query.clone(), true),
                                                    span { class: "min-w-0 flex-1 truncate text-[13px] text-ink", "{title}" }
                                                    span { class: "shrink-0 text-[11px] text-ink-tertiary", "{when}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if !where_.is_empty() {
                                p { class: "mt-4 break-all px-2 text-[11px] text-ink-tertiary", "{where_}" }
                            }
                        }
                    }
                }

                // The open note.
                section { "aria-label": "Note", class: "flex min-w-0 flex-1 flex-col {note_class}",
                    div { class: "flex h-12 items-center gap-1 px-1",
                        button { r#type: "button", "aria-label": "Back to the notes", "data-memory-back": true, class: "{ICON_BUTTON} md:hidden", onclick: move |_| reading.set(false),
                            svg { "aria-hidden": "true", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                                path { d: "m15 18-6-6 6-6" }
                            }
                        }
                        div { class: "min-w-0 flex-1 px-2",
                            if let Some(n) = note() {
                                p { class: "truncate text-[13px] text-ink-secondary", "data-memory-path": true,
                                    "{n[\"path\"].as_str().unwrap_or_default()}"
                                    if n["inCodex"] == true { span { class: "ms-2 rounded bg-raised px-1.5 py-0.5 text-[11px]", "Codex" } }
                                }
                            }
                        }
                        span { class: "rounded px-2 text-[11px] text-ink-tertiary", "Read-only" }
                        button { r#type: "button", "aria-label": "Close", class: "{ICON_BUTTON} max-md:hidden", onclick: move |_| on_close.call(()), CloseIcon {} }
                    }
                    div { "data-memory-scroll": true, class: "min-h-0 flex-1 overflow-y-auto px-6 pb-8 max-md:px-4",
                        if let Some(err) = problem() {
                            p { role: "alert", class: "mb-3 text-[13px] text-danger", "{err}" }
                        }
                        if let Some(n) = note() {
                            {
                                let html = markdown::note_html(n["text"].as_str().unwrap_or_default());
                                rsx! {
                                    div { "data-memory-body": true, class: "chat-md mx-auto max-w-[680px] text-[15px] leading-[23px] text-ink [&>*+*]:mt-2", dangerous_inner_html: "{html}" }
                                }
                            }
                        } else if on == Some(true) {
                            p { class: "text-[13px] text-ink-secondary", "Pick a note." }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CloseIcon() -> Element {
    rsx! {
        svg { "aria-hidden": "true", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round",
            path { d: "M18 6 6 18M6 6l12 12" }
        }
    }
}

/// The notes by folder: the vault's own first, then Bots, Projects, the Journal (newest first)
/// and anything else.
fn group(notes: &[Value]) -> Vec<(String, Vec<Value>)> {
    let mut groups: Vec<(String, Vec<Value>)> = FOLDERS.iter().map(|(_, label)| (label.to_string(), Vec::new())).collect();
    for note in notes {
        let folder = note["folder"].as_str().unwrap_or_default();
        match FOLDERS.iter().position(|(f, _)| *f == folder) {
            Some(i) => groups[i].1.push(note.clone()),
            None => match groups.iter_mut().find(|(label, _)| label == folder) {
                Some(group) => group.1.push(note.clone()),
                None => groups.push((folder.to_string(), vec![note.clone()])),
            },
        }
    }
    if let Some((_, journal)) = groups.iter_mut().find(|(label, _)| label == "Journal") {
        journal.reverse();
    }
    groups.retain(|(_, items)| !items.is_empty());
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notes_are_grouped_by_folder() {
        let notes = vec![
            json!({ "path": "AGENTS.md", "folder": "" }),
            json!({ "path": "Bots/Scout.md", "folder": "Bots" }),
            json!({ "path": "Home.md", "folder": "" }),
            json!({ "path": "Journal/2026-11-14 Scout.md", "folder": "Journal" }),
            json!({ "path": "Journal/2026-11-15 Scout.md", "folder": "Journal" }),
            json!({ "path": "Recipes/Bread.md", "folder": "Recipes" }),
        ];
        let groups = group(&notes);
        let labels: Vec<&str> = groups.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Vault", "Bots", "Journal", "Recipes"]);
        assert_eq!(groups[2].1[0]["path"], "Journal/2026-11-15 Scout.md", "newest day first");
    }
}
