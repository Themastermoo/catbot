//! Settings › Pages (stage 13.12): every page a bot published, with its link and a way to take
//! it down.

use dioxus::prelude::*;
use serde_json::Value;

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";

#[component]
pub fn PagesList() -> Element {
    let state = use_context::<Signal<State>>();
    let mut list = use_signal(Vec::<Value>::new);
    let mut problem = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| None::<String>);
    use_effect(move || {
        let _ = state.read().pages_changed;
        spawn(async move {
            match api::get("/api/pages").await {
                Ok(found) => list.set(found["pages"].as_array().cloned().unwrap_or_default()),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let take_down = move |id: String| {
        spawn(async move {
            if let Err(err) = api::call("DELETE", &format!("/api/pages/{id}"), None).await {
                problem.set(Some(err));
            }
            confirm.set(None);
        });
    };
    rsx! {
        section { class: "flex flex-col gap-3", "data-pages": true,
            h2 { class: HEADING, "Published pages" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Pages your bots put online, after you approved them. They're served from this computer at links nobody can guess, and stay up while it's on."
            }
            if list.read().is_empty() {
                p { class: "text-[13px] text-ink-tertiary", "Nothing published yet." }
            }
            for page in list() {
                {
                    let id = page["id"].as_str().unwrap_or_default().to_string();
                    let title = page["title"].as_str().unwrap_or("A page").to_string();
                    let url = page["url"].as_str().unwrap_or_default().to_string();
                    let bot = page["botName"].as_str().unwrap_or_default().to_string();
                    let when = page["publishedAt"].as_str().map(crate::chat::parse_time).filter(|at| !at.is_nan()).map(crate::chat::short_when).unwrap_or_default();
                    let files = page["files"].as_u64().unwrap_or(0);
                    let plural = if files == 1 { "" } else { "s" };
                    let asking = confirm().as_deref() == Some(id.as_str());
                    let (down, keep) = (id.clone(), id.clone());
                    rsx! {
                        div { key: "{id}", class: "flex items-start gap-2 rounded-lg border border-hairline/40 px-3 py-2", "data-page": "{id}",
                            div { class: "min-w-0 flex-1",
                                div { class: "text-[14px] font-medium text-ink", "{title}" }
                                a { class: "block truncate text-[12px] text-accent underline", href: "{url}", target: "_blank", rel: "noopener", "{url}" }
                                div { class: "text-[11px] text-ink-tertiary", "By {bot} · {when} · {files} file{plural}" }
                            }
                            if asking {
                                button { r#type: "button", class: "rounded-lg bg-danger px-2.5 py-1 text-[12px] font-medium text-white", "data-page-down": true, onclick: move |_| take_down(down.clone()), "Take down" }
                                button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised", onclick: move |_| confirm.set(None), "Keep" }
                            } else {
                                button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-danger", onclick: move |_| confirm.set(Some(keep.clone())), "Take down…" }
                            }
                        }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
