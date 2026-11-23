//! The Team map's message box (stage 19.6), as OpenMausBot's `MapComposer.tsx` and
//! `lib/map-composer.ts` (Apache-2.0, see NOTICE): tag a bot with @, a department, or
//! @everyone; the tagged bots light up on the map as you type, and each gets the message in its
//! own chat. @everyone goes to the Chief of Staff, who passes it on (Moo's choice, 2026-11-23);
//! with no Chief, every bot gets it.

use dioxus::prelude::*;
use serde_json::json;

use crate::api;
use crate::departments::Depts;
use crate::live::State;
use crate::sidebar::Icon;

/// Who a message is for, as the server will read it (`departments::targets` there).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    pub bots: Vec<String>,
    pub everyone: bool,
    pub unknown: Vec<String>,
}

fn continues(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// Reads the @tags: bot names `(id, name, department)`, department names, `everyone`; the longest
/// name wins and never runs on into a longer word.
pub fn targets(text: &str, bots: &[(String, String, String)], departments: &[String]) -> Targets {
    let mut names: Vec<(String, u8, String)> = bots.iter().map(|(id, name, _)| (name.clone(), 0, id.clone())).collect();
    names.extend(departments.iter().map(|d| (d.clone(), 1, d.clone())));
    names.push(("everyone".into(), 2, String::new()));
    names.sort_by_key(|(n, _, _)| std::cmp::Reverse(n.chars().count()));
    let mut out = Targets::default();
    let mut at = 0;
    while let Some(found) = text[at..].find('@') {
        let start = at + found;
        at = start + 1;
        if continues(text[..start].chars().next_back()) {
            continue;
        }
        let rest = &text[start + 1..];
        match names.iter().find(|(name, _, _)| rest.get(..name.len()).is_some_and(|s| s.eq_ignore_ascii_case(name)) && !continues(rest[name.len()..].chars().next())) {
            Some((name, kind, id)) => {
                match kind {
                    0 => out.bots.push(id.clone()),
                    1 => out.bots.extend(bots.iter().filter(|(_, _, d)| d == id).map(|(b, _, _)| b.clone())),
                    _ => out.everyone = true,
                }
                at = start + 1 + name.len();
            }
            None => {
                let word: String = rest.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
                if !word.is_empty() {
                    out.unknown.push(word);
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.bots.retain(|id| seen.insert(id.clone()));
    out
}

/// The @word being typed at the end of the text, if any: what the suggestions match.
fn typing_tag(text: &str) -> Option<String> {
    let start = text.rfind('@')?;
    if continues(text[..start].chars().next_back()) {
        return None;
    }
    let word = &text[start + 1..];
    (word.chars().count() <= 30 && !word.contains('\n')).then(|| word.to_string())
}

/// "Maya, Theo and 18 more".
pub fn summary(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        3 => format!("{}, {} and {}", names[0], names[1], names[2]),
        n => format!("{}, {} and {} more", names[0], names[1], n - 2),
    }
}

#[component]
pub fn MapComposer() -> Element {
    let state = use_context::<Signal<State>>();
    let depts = use_context::<Signal<Depts>>();
    let mut tagged = use_context::<Signal<Vec<String>>>();
    let mut text = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut sent = use_signal(|| None::<String>);
    let bots: Vec<(String, String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone(), b.department.clone())).collect();
    if bots.is_empty() {
        return rsx! {};
    }
    let names: Vec<String> = depts.read().list.iter().map(|d| d.name.clone()).collect();
    let chief = state.read().bots.iter().find(|b| b.chief && b.channel.is_none()).map(|b| b.name.clone());
    let found = targets(&text.read(), &bots, &names);
    // The bots that will get it light up on the map.
    let lit: Vec<String> = if found.everyone && chief.is_none() { bots.iter().map(|b| b.0.clone()).collect() } else { found.bots.clone() };
    if *tagged.peek() != lit {
        tagged.set(lit.clone());
    }
    let will = if found.everyone {
        match &chief {
            Some(name) => format!("Goes to {name}, who passes it on to the bots it concerns"),
            None => format!("Goes to all {} bots, each in its own chat", bots.len()),
        }
    } else if found.bots.len() == 1 {
        "Goes to 1 tagged bot, in its own chat".to_string()
    } else if found.bots.len() > 1 {
        format!("Goes to {} tagged bots, each in its own chat", found.bots.len())
    } else if let Some(word) = found.unknown.iter().find(|w| typing_tag(&text.read()).as_deref() != Some(w.as_str())) {
        // A tag still being typed isn't "nobody" yet.
        format!("Nobody here is called @{word}")
    } else {
        "Tag a bot with @, a department, or @everyone".to_string()
    };
    let ready = (found.everyone || !found.bots.is_empty()) && !sending();
    // Suggestions for the @word being typed.
    let typing = typing_tag(&text.read());
    let suggestions: Vec<(String, &'static str)> = match &typing {
        Some(word) => {
            let word = word.to_lowercase();
            let mut list: Vec<(String, &'static str)> = names.iter().map(|n| (n.clone(), "Department")).collect();
            list.extend(bots.iter().map(|(_, n, _)| (n.clone(), "Bot")));
            list.push(("everyone".into(), "Everyone"));
            list.retain(|(n, _)| n.to_lowercase().starts_with(&word) && n.to_lowercase() != word);
            list.truncate(6);
            list
        }
        None => Vec::new(),
    };
    let send = move || {
        if sending() || text.peek().trim().is_empty() {
            return;
        }
        sending.set(true);
        problem.set(None);
        let body = json!({ "text": text.peek().trim() });
        spawn(async move {
            match api::post("/api/team-map/message", &body).await {
                Ok(answer) => {
                    let names: Vec<String> = answer["sent"].as_array().into_iter().flatten().filter_map(|b| b["name"].as_str().map(str::to_string)).collect();
                    let line = match answer["via"].as_str() {
                        Some(via) => format!("Sent to {via}, who passes it on"),
                        None => format!("Sent to {}", summary(&names)),
                    };
                    let failed: Vec<String> = answer["failed"].as_array().into_iter().flatten().filter_map(|b| Some(format!("{}: {}", b["name"].as_str()?, b["error"].as_str()?))).collect();
                    if !failed.is_empty() {
                        problem.set(Some(failed.join("; ")));
                    }
                    sent.set(Some(line));
                    text.set(String::new());
                    spawn(async move {
                        crate::events::pause(4_000).await;
                        sent.set(None);
                    });
                }
                Err(err) => problem.set(Some(err)),
            }
            sending.set(false);
        });
    };
    let mut complete = move |name: String| {
        let now = text.peek().clone();
        if let Some(at) = now.rfind('@') {
            text.set(format!("{}@{} ", &now[..at], name));
        }
    };
    let (mut send_key, mut send_click) = (send, send);
    rsx! {
        div { "data-map-composer": true, class: "absolute inset-x-0 bottom-4 z-20 mx-auto w-[min(640px,calc(100%-2rem))]",
            onpointerdown: move |e| e.stop_propagation(),
            if !suggestions.is_empty() {
                div { role: "listbox", "aria-label": "Tag", class: "mb-1.5 overflow-hidden rounded-xl border border-hairline/50 bg-menu p-1 shadow-lg",
                    for (name, kind) in suggestions {
                        button { key: "{kind}-{name}", r#type: "button", role: "option", "data-map-suggest": "{name}",
                            class: "flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] text-ink hover:bg-menu-hover",
                            onclick: move |_| complete(name.clone()),
                            span { class: "min-w-0 flex-1 truncate", "@{name}" }
                            span { class: "shrink-0 text-[11px] text-ink-tertiary", "{kind}" }
                        }
                    }
                }
            }
            div { class: "rounded-[22px] bg-composer shadow-composer ring-[0.5px] ring-ink/10",
                div { class: "flex items-end gap-2 px-3 pb-2 pt-2.5",
                    textarea {
                        "data-map-message": true,
                        rows: "1",
                        "aria-label": "Message bots on the map",
                        placeholder: "Message your bots: tag one with @, a department, or @everyone",
                        value: "{text}",
                        class: "max-h-32 min-h-[24px] flex-1 resize-none bg-transparent px-1 py-0.5 text-[14px] leading-6 text-ink outline-none placeholder:text-ink-tertiary",
                        oninput: move |e| { text.set(e.value()); problem.set(None); },
                        onkeydown: move |e: KeyboardEvent| if e.key() == Key::Enter && !e.modifiers().shift() { e.prevent_default(); send_key(); },
                    }
                    button { r#type: "button", "data-map-send": true, disabled: !ready,
                        "aria-label": if found.bots.len() == 1 && !found.everyone { "Send to the tagged bot" } else { "Send to tagged bots" },
                        class: "flex size-8 shrink-0 items-center justify-center rounded-full bg-ink text-app disabled:bg-ink/15 disabled:text-ink-tertiary",
                        onclick: move |_| send_click(),
                        Icon { name: "arrow-up", size: 16 }
                    }
                }
            }
            div { "aria-live": "polite", class: "mt-1.5 px-3 text-center text-[11px] text-ink-tertiary",
                if let Some(err) = problem() {
                    span { role: "alert", class: "text-danger", "{err}" }
                } else if let Some(line) = sent() {
                    span { "data-map-sent": true, class: "text-success", "{line}" }
                } else {
                    span { "data-map-will": true, "{will}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_match_like_the_server_reads_them() {
        let bots = [("1".to_string(), "Kiwi".to_string(), String::new()), ("2".into(), "Scout".into(), "Garden".into()), ("3".into(), "Scout Two".into(), "Garden".into())];
        let depts = ["Garden".to_string()];
        let t = targets("@scout two then @Garden and @nobody", &bots, &depts);
        assert_eq!(t.bots, ["3", "2"]);
        assert_eq!(t.unknown, ["nobody"]);
        assert!(targets("hi @everyone, mail@kiwi.com", &bots, &depts).everyone);
        assert!(targets("mail@kiwi.com", &bots, &depts).bots.is_empty());
        assert_eq!(typing_tag("hello @Sc").as_deref(), Some("Sc"));
        assert_eq!(typing_tag("a@b"), None);
        assert_eq!(summary(&["A".into(), "B".into(), "C".into(), "D".into()]), "A, B and 2 more");
    }
}
