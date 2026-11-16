//! Settings › Chief of Staff (stage 15.3): who the Chief is (one of the bots, or the ready-made
//! Chief Catbot makes), and the Chief's options.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const SELECT: &str = "min-w-0 flex-1 rounded-lg bg-inset px-3 py-2 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";

#[component]
pub fn ChiefOptions() -> Element {
    let state = use_context::<Signal<State>>();
    let mut found = use_signal(|| None::<Value>);
    let mut problem = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        let _ = state.read().chief_changed;
        spawn(async move {
            match api::get("/api/chief").await {
                Ok(answer) => found.set(Some(answer)),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let mut send = move |method: &'static str, path: &'static str, body: Value| {
        busy.set(true);
        spawn(async move {
            match api::call(method, path, Some(&body)).await {
                Ok(answer) => {
                    problem.set(None);
                    found.set(Some(answer));
                }
                Err(err) => problem.set(Some(format!("Couldn't change the Chief: {err}"))),
            }
            busy.set(false);
        });
    };
    let Some(current) = found() else {
        return rsx! {};
    };
    let chief_id = current["chief"]["botId"].as_str().unwrap_or_default().to_string();
    let ready = current["readyMadeName"].as_str().unwrap_or("Chief").to_string();
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone())).collect();
    rsx! {
        section { class: "flex flex-col gap-3", "data-chief-options": true,
            h2 { class: HEADING, "Who it is" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Your Chief of Staff is your main point of contact for the whole team. It knows every bot and channel, hands work to the bot best placed for it, checks what comes back, and gives you one answer. It has a star in the sidebar."
            }
            div { class: "flex items-center gap-2",
                select {
                    class: SELECT,
                    "aria-label": "Chief of Staff",
                    "data-chief-pick": true,
                    disabled: busy(),
                    onchange: move |e| {
                        let id = e.value();
                        send("PUT", "/api/chief", json!({ "botId": if id.is_empty() { Value::Null } else { json!(id) } }));
                    },
                    option { value: "", selected: chief_id.is_empty(), "No Chief of Staff" }
                    for (id, name) in bots {
                        option { key: "{id}", value: "{id}", selected: id == chief_id, "{name}" }
                    }
                }
            }
            button {
                r#type: "button",
                "data-chief-ready-made": true,
                disabled: busy(),
                class: "self-start rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50",
                onclick: move |_| send("POST", "/api/chief/ready-made", json!({})),
                "Add the ready-made {ready} as Chief"
            }
            p { class: "text-[12px] leading-relaxed text-ink-tertiary",
                "The ready-made Chief is a new bot with Chief of Staff instructions. Any bot can be made Chief instead; only one is Chief at a time."
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
        if !chief_id.is_empty() {
            CheckIns { options: current["chief"].clone(), on_change: move |answer: Value| found.set(Some(answer)) }
        }
    }
}

const FIELD: &str = "rounded-lg bg-inset px-2 py-1.5 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-40";
const DAYS: [(u64, &str); 7] = [(1, "Mon"), (2, "Tue"), (3, "Wed"), (4, "Thu"), (5, "Fri"), (6, "Sat"), (7, "Sun")];

/// Saves part of the Chief's options; the answer is all of them.
fn save(change: Value, on_change: EventHandler<Value>, mut problem: Signal<Option<String>>) {
    spawn(async move {
        match api::call("PUT", "/api/chief", Some(&change)).await {
            Ok(answer) => {
                problem.set(None);
                on_change.call(answer);
            }
            Err(err) => problem.set(Some(format!("Couldn't save: {err}"))),
        }
    });
}

/// The watch, timed check-ins, and a look now.
#[component]
fn CheckIns(options: Value, on_change: EventHandler<Value>) -> Element {
    let problem = use_signal(|| None::<String>);
    let mut looked = use_signal(|| None::<String>);
    let watch = options["watch"].as_bool().unwrap_or(true);
    let checkins = options["checkins"].clone();
    let timed = checkins["on"].as_bool().unwrap_or(false);
    let every = checkins["everyHours"].as_u64().unwrap_or(3);
    let from = checkins["from"].as_str().unwrap_or("09:00").to_string();
    let to = checkins["to"].as_str().unwrap_or("17:00").to_string();
    let days: Vec<u64> = checkins["days"].as_array().map(|d| d.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    let look = move |path: &'static str| {
        spawn(async move {
            match api::post(path, &json!({})).await {
                Ok(answer) if answer["woke"] == true && path.ends_with("look") => looked.set(Some("Found something; the Chief is looking into it.".into())),
                Ok(answer) if answer["woke"] == true => looked.set(Some("The Chief is checking in.".into())),
                Ok(_) if path.ends_with("look") => looked.set(Some("Nothing wrong found.".into())),
                Ok(_) => looked.set(Some("The Chief is busy; try again in a moment.".into())),
                Err(err) => looked.set(Some(err)),
            }
        });
    };
    rsx! {
        section { class: "flex flex-col gap-3", "data-chief-checkins": true,
            h2 { class: HEADING, "Check-ins" }
            div { class: "flex items-center gap-3",
                div { class: "min-w-0 flex-1",
                    div { class: "text-[14px] text-ink", "Wake the Chief when something goes wrong" }
                    div { class: "text-[12px] leading-snug text-ink-secondary", "Every half hour Catbot looks over the team. When a bot hits an error or has waited a while for your approval, it asks the Chief to deal with it. Costs nothing when all is well." }
                }
                crate::settings::Switch { checked: watch, disabled: false, label: "Wake the Chief when something goes wrong".to_string(), on_toggle: move |_| save(json!({ "watch": !watch }), on_change, problem) }
            }
            div { class: "flex items-center gap-3",
                div { class: "min-w-0 flex-1",
                    div { class: "text-[14px] text-ink", "Timed check-ins" }
                    div { class: "text-[12px] leading-snug text-ink-secondary", "The Chief also looks the whole team over on a schedule and follows up where needed. Each check-in is one AI turn." }
                }
                crate::settings::Switch { checked: timed, disabled: false, label: "Timed check-ins".to_string(), on_toggle: move |_| save(json!({ "checkins": { "on": !timed } }), on_change, problem) }
            }
            if timed {
                div { class: "flex flex-wrap items-center gap-2 pl-1 text-[13px] text-ink",
                    span { "Every" }
                    input { key: "every-{every}", class: "{FIELD} w-16 text-right", r#type: "number", min: 1, max: 24, "data-checkin-every": true, initial_value: "{every}",
                        onchange: move |e| if let Ok(n) = e.value().trim().parse::<u64>() { save(json!({ "checkins": { "everyHours": n } }), on_change, problem) } }
                    span { "hours, from" }
                    input { key: "from-{from}", class: FIELD, r#type: "time", "data-checkin-from": true, initial_value: "{from}",
                        onchange: move |e| save(json!({ "checkins": { "from": e.value() } }), on_change, problem) }
                    span { "to" }
                    input { key: "to-{to}", class: FIELD, r#type: "time", "data-checkin-to": true, initial_value: "{to}",
                        onchange: move |e| save(json!({ "checkins": { "to": e.value() } }), on_change, problem) }
                }
                div { class: "flex flex-wrap gap-1 pl-1", role: "group", "aria-label": "Check-in days",
                    for (day, label) in DAYS {
                        {
                            let on = days.contains(&day);
                            let others = days.clone();
                            rsx! {
                                button {
                                    key: "{day}",
                                    r#type: "button",
                                    "aria-pressed": "{on}",
                                    "data-checkin-day": "{label}",
                                    class: if on { "rounded-full bg-accent px-2.5 py-1 text-[12px] font-semibold text-white" } else { "rounded-full bg-control px-2.5 py-1 text-[12px] text-ink-secondary hover:text-ink" },
                                    onclick: move |_| {
                                        let mut next: Vec<u64> = others.iter().copied().filter(|d| *d != day).collect();
                                        if !on {
                                            next.push(day);
                                            next.sort_unstable();
                                        }
                                        save(json!({ "checkins": { "days": next } }), on_change, problem);
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-2",
                button { r#type: "button", "data-chief-look": true, class: "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised", onclick: move |_| look("/api/chief/look"), "Look for problems now" }
                button { r#type: "button", "data-chief-check-in": true, class: "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised", onclick: move |_| look("/api/chief/check-in"), "Check in now" }
                if let Some(said) = looked() {
                    span { class: "text-[12px] text-ink-secondary", "{said}" }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
