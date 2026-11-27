//! A team change the Chief of Staff proposed (`server/team.rs`): exactly what would change, and
//! Approve or Decline. Nothing changes until the person approves.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub title: String,
    pub lines: Vec<String>,
    pub warning: Option<String>,
    pub asked_by: String,
    /// What the card is about, when it isn't a team change (a skill a bot offers).
    pub heading: Option<String>,
    /// What the card says once it's done ("Sent."), when not the usual.
    pub done_text: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub choice: Option<Choice>,
    /// A done change to bots, which Undo takes back in one step (stage 32.12).
    pub can_undo: bool,
    /// The buttons' words, when not Approve and Decline (a review's Looks good, Needs work).
    pub approve_label: Option<String>,
    pub decline_label: Option<String>,
}

/// A choice the person makes as they approve (where a page goes).
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    /// (id, words).
    pub options: Vec<(String, String)>,
    /// The one picked, or to start on.
    pub value: String,
}

impl Change {
    pub fn from_wire(wire: &Value) -> Change {
        Change {
            title: wire["title"].as_str().unwrap_or_default().to_string(),
            lines: wire["lines"].as_array().map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
            warning: wire["warning"].as_str().map(str::to_string),
            asked_by: wire["askedBy"].as_str().unwrap_or("The Chief").to_string(),
            heading: wire["heading"].as_str().map(str::to_string),
            done_text: wire["doneText"].as_str().map(str::to_string),
            state: wire["state"].as_str().unwrap_or("asking").to_string(),
            error: wire["error"].as_str().map(str::to_string),
            can_undo: wire["canUndo"] == true,
            approve_label: wire["approveLabel"].as_str().map(str::to_string),
            decline_label: wire["declineLabel"].as_str().map(str::to_string),
            choice: wire["choice"]["options"].as_array().map(|options| Choice {
                label: wire["choice"]["label"].as_str().unwrap_or("Choose").to_string(),
                options: options.iter().map(|o| (o["id"].as_str().unwrap_or_default().to_string(), o["label"].as_str().unwrap_or_default().to_string())).collect(),
                value: wire["choice"]["value"].as_str().unwrap_or_default().to_string(),
            }),
        }
    }
}

#[component]
pub fn TeamChangeCard(thread: String, message: String, change: Change) -> Element {
    let mut problem = use_signal(|| None::<String>);
    let mut sending = use_signal(|| false);
    let start = change.choice.as_ref().map(|c| c.value.clone()).unwrap_or_default();
    let mut picked = use_signal(move || start);
    let has_choice = change.choice.is_some();
    let undo_path = format!("/api/threads/{thread}/team/{message}");
    let undo = move |_| {
        let path = undo_path.clone();
        sending.set(true);
        spawn(async move {
            if let Err(err) = api::post(&path, &json!({ "undo": true })).await {
                problem.set(Some(err));
            }
            sending.set(false);
        });
    };
    let send = use_callback(move |approve: bool| {
        let path = format!("/api/threads/{thread}/team/{message}");
        sending.set(true);
        let mut body = json!({ "approve": approve });
        if has_choice && approve {
            body["choice"] = json!(picked());
        }
        spawn(async move {
            if let Err(err) = api::post(&path, &body).await {
                problem.set(Some(err));
            }
            sending.set(false);
        });
    });
    let asking = change.state == "asking";
    rsx! {
        div { class: "flex w-full justify-start", "data-team-change": "{change.state}",
            div { class: "flex w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] flex-col gap-2 rounded-xl border border-hairline/40 bg-panel px-3 py-2.5 text-[13px]",
                div { class: "flex items-start gap-2",
                    // Lucide `Users` (ISC).
                    svg { "aria-hidden": "true", class: "mt-0.5 shrink-0 text-ink-secondary", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" }
                        circle { cx: "9", cy: "7", r: "4" }
                        path { d: "M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75" }
                    }
                    div { class: "min-w-0 flex-1",
                        p { class: "text-ink-secondary",
                            match &change.heading {
                                Some(heading) => heading.clone(),
                                None => format!("{} wants to change the team", change.asked_by),
                            }
                        }
                        p { class: "font-semibold text-ink", "{change.title}" }
                    }
                }
                if !change.lines.is_empty() {
                    ul { class: "flex flex-col gap-0.5 pl-6 text-ink",
                        for line in change.lines.iter() {
                            li { class: "list-disc whitespace-pre-wrap break-words", "{line}" }
                        }
                    }
                }
                if let Some(Choice { label, options, value }) = &change.choice {
                    if asking {
                        label { class: "flex items-center gap-2 pl-6 text-ink",
                            span { class: "text-ink-secondary", "{label}:" }
                            select {
                                "data-team-choice": true,
                                class: "rounded-lg border border-hairline/60 bg-raised px-2 py-1 text-ink",
                                disabled: sending(),
                                onchange: move |e| picked.set(e.value()),
                                for (id, words) in options.iter() {
                                    option { key: "{id}", value: "{id}", selected: picked() == *id, "{words}" }
                                }
                            }
                        }
                    } else if let Some((_, words)) = options.iter().find(|(id, _)| id == value) {
                        p { class: "pl-6 text-ink-secondary", "{label}: {words}" }
                    }
                }
                if let Some(warning) = &change.warning {
                    p { class: "pl-6 text-warning", "{warning}" }
                }
                match change.state.as_str() {
                    "done" => {
                        let text = change.done_text.clone().unwrap_or_else(|| "Approved and done.".into());
                        let (before, link, after) = with_link(&text);
                        rsx! {
                            p { class: "text-ink-tertiary",
                                "{before}"
                                if !link.is_empty() {
                                    a { class: "text-accent underline", href: "{link}", target: "_blank", rel: "noopener", "data-done-link": true, "{link}" }
                                }
                                "{after}"
                            }
                        }
                    }
                    "sending" => rsx! { p { class: "text-ink-secondary", "Sending…" } },
                    "publishing" => rsx! { p { class: "text-ink-secondary", "Publishing…" } },
                    "working" => rsx! { p { class: "text-ink-secondary", "Working…" } },
                    "declined" => rsx! { p { class: "text-ink-tertiary", {change.done_text.clone().unwrap_or_else(|| "Declined. Nothing changed.".into())} } },
                    "undone" => rsx! { p { class: "text-ink-tertiary", "data-team-undone": true, {change.done_text.clone().unwrap_or_else(|| "Undone.".into())} } },
                    _ => rsx! {},
                }
                if let Some(err) = &change.error {
                    p { role: "alert", class: "text-danger", "Approved, but it didn't work: {err}" }
                }
                if asking {
                    div { class: "flex gap-1.5 pl-6",
                        button {
                            r#type: "button",
                            "data-team-approve": true,
                            disabled: sending(),
                            class: "rounded-lg bg-accent px-3 py-1.5 font-semibold text-white disabled:opacity-50",
                            onclick: move |_| send.call(true),
                            {change.approve_label.clone().unwrap_or_else(|| "Approve".into())}
                        }
                        button {
                            r#type: "button",
                            "data-team-decline": true,
                            disabled: sending(),
                            class: "rounded-lg px-3 py-1.5 text-ink-secondary hover:bg-raised disabled:opacity-50",
                            onclick: move |_| send.call(false),
                            {change.decline_label.clone().unwrap_or_else(|| "Decline".into())}
                        }
                    }
                }
                if change.can_undo {
                    div { class: "flex gap-1.5 pl-6",
                        button {
                            r#type: "button",
                            "data-team-undo": true,
                            disabled: sending(),
                            title: "Put back how they were before this change",
                            class: "rounded-lg px-3 py-1.5 text-ink-secondary ring-1 ring-inset ring-hairline hover:bg-raised disabled:opacity-50",
                            onclick: undo,
                            "Undo"
                        }
                    }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "text-danger", "{err}" }
                }
            }
        }
    }
}

/// Splits a note around its first web address, so the card can make it a link: a published
/// page's "It's at https://…".
fn with_link(text: &str) -> (String, String, String) {
    let Some(start) = text.find("https://").or_else(|| text.find("http://")) else {
        return (text.to_string(), String::new(), String::new());
    };
    let rest = &text[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let link = rest[..end].trim_end_matches(['.', ',', ')', ';', ':']);
    (text[..start].to_string(), link.to_string(), text[start + link.len()..].to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_note_with_an_address_becomes_a_link() {
        let (a, l, b) = super::with_link("It's at http://x.test/p/1/, on this computer only.");
        assert_eq!((a.as_str(), l.as_str(), b.as_str()), ("It's at ", "http://x.test/p/1/", ", on this computer only."));
        assert_eq!(super::with_link("Sent.").1, "");
    }
}
