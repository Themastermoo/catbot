//! Triggers (stage 14): every schedule, with when it runs next, how its last run went and its
//! history, and a form to make or change one (`server/triggers.rs`). Each trigger keeps one chat
//! of its own in its bot's chat list; "Open chat" goes there.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::chat::{clock, parse_time, short_when};
use crate::live::State;
use crate::settings::Ui;

const ICON_BUTTON: &str = "flex size-8 shrink-0 items-center justify-center rounded-full text-ink-secondary hover:bg-raised hover:text-ink";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const SMALL: &str = "rounded-lg bg-inset px-2 py-1.5 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";
const LINK: &str = "rounded-md px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-ink disabled:opacity-50";
const DAYS: [(u32, &str); 7] = [(1, "Mon"), (2, "Tue"), (3, "Wed"), (4, "Thu"), (5, "Fri"), (6, "Sat"), (7, "Sun")];

/// "Wed 9:00 AM", or "Today 9:00 AM".
fn when_text(at: &str) -> String {
    let t = parse_time(at);
    if t.is_nan() {
        return String::new();
    }
    let now = crate::chat::js_now();
    if crate::chat::day(t) == crate::chat::day(now) {
        format!("Today {}", clock(t))
    } else {
        format!("{} {}", crate::chat::weekday(t).chars().take(3).collect::<String>(), clock(t))
    }
}

#[component]
pub fn TriggersDialog(on_close: EventHandler<()>) -> Element {
    let state = use_context::<Signal<State>>();
    let mut list = use_signal(Vec::<Value>::new);
    let mut problem = use_signal(|| None::<String>);
    // None: the list. Some(Null): a new trigger. Some(trigger): changing that one.
    let mut editing = use_signal(|| None::<Value>);
    use_effect(move || {
        let _ = state.read().triggers_changed;
        spawn(async move {
            match api::get("/api/triggers").await {
                Ok(found) => list.set(found["triggers"].as_array().cloned().unwrap_or_default()),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 max-md:p-0", onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                "aria-label": "Triggers",
                "data-triggers-dialog": true,
                class: "flex max-h-[88vh] w-full max-w-[720px] flex-col overflow-hidden rounded-2xl bg-panel shadow-2xl max-md:h-full max-md:max-h-none max-md:rounded-none",
                onclick: move |e| e.stop_propagation(),
                div { class: "flex h-12 shrink-0 items-center gap-2 border-b-[0.5px] border-edge pl-5 pr-2",
                    span { class: "min-w-0 flex-1 text-[15px] font-semibold text-ink",
                        if editing().is_some_and(|e| e.is_null()) { "New trigger" } else if editing().is_some() { "Change trigger" } else { "Triggers" }
                    }
                    if editing().is_none() {
                        button { r#type: "button", class: PRIMARY, "data-new-trigger": true, onclick: move |_| editing.set(Some(Value::Null)), "New trigger" }
                    }
                    button { r#type: "button", "aria-label": "Close", class: ICON_BUTTON, onclick: move |_| on_close.call(()), "×" }
                }
                div { class: "min-h-0 flex-1 overflow-y-auto px-5 py-4",
                    if let Some(trigger) = editing() {
                        TriggerForm { trigger, on_done: move |_| editing.set(None) }
                    } else {
                        p { class: "mb-3 text-[13px] leading-relaxed text-ink-secondary",
                            "When the time comes, the bot gets the trigger's instructions in a chat of its own, so it can see what it did last time. A time missed while Catbot was off runs once when it starts again."
                        }
                        if list.read().is_empty() {
                            p { class: "text-[13px] text-ink-tertiary", "No triggers yet. Ask a bot (\"every weekday at 9, sum up my inbox\"), or make one here." }
                        }
                        div { class: "flex flex-col gap-2",
                            for trigger in list() {
                                TriggerRow { key: "{trigger[\"id\"].as_str().unwrap_or_default()}", trigger: trigger.clone(), on_edit: move |t| editing.set(Some(t)), on_close }
                            }
                        }
                    }
                    if let Some(err) = problem() {
                        p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
                    }
                }
            }
        }
    }
}

#[component]
fn TriggerRow(trigger: Value, on_edit: EventHandler<Value>, on_close: EventHandler<()>) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| false);
    let mut history = use_signal(|| None::<Vec<Value>>);
    let id = trigger["id"].as_str().unwrap_or_default().to_string();
    let name = trigger["name"].as_str().unwrap_or_default().to_string();
    let bot = trigger["botName"].as_str().unwrap_or_default().to_string();
    let bot_id = trigger["botId"].as_str().unwrap_or_default().to_string();
    let when = trigger["when"].as_str().unwrap_or_default().to_string();
    let on = trigger["enabled"].as_bool().unwrap_or(false);
    let next = trigger["nextAt"].as_str().map(when_text);
    let thread = trigger["threadId"].as_str().map(str::to_string);
    let latest = &trigger["latestRun"];
    let last = latest["status"].as_str().map(|status| {
        let at = latest["startedAt"].as_str().map(parse_time).filter(|t| !t.is_nan()).map(short_when).unwrap_or_default();
        let words = match status {
            "working" => "working now".to_string(),
            "done" => format!("last ran {at}"),
            "failed" => format!("failed {at}: {}", latest["error"].as_str().unwrap_or_default()),
            "skipped" => format!("skipped {at}: {}", latest["error"].as_str().unwrap_or_default()),
            other => other.to_string(),
        };
        (status.to_string(), words)
    });
    let act = use_callback(move |(method, path, body): (&'static str, String, Option<Value>)| {
        busy.set(true);
        spawn(async move {
            if let Err(err) = api::call(method, &path, body.as_ref()).await {
                problem.set(Some(err));
            }
            busy.set(false);
        });
    });
    let (run_id, pause_id, delete_id, history_id) = (id.clone(), id.clone(), id.clone(), id.clone());
    let editable = trigger.clone();
    rsx! {
        div { class: "flex flex-col gap-1 rounded-lg border border-hairline/40 px-3 py-2", "data-trigger": "{id}",
            div { class: "flex items-start gap-2",
                div { class: "min-w-0 flex-1",
                    div { class: "flex items-center gap-2",
                        span { class: "truncate text-[14px] font-medium text-ink", "{name}" }
                        if !on {
                            span { class: "rounded bg-raised px-1.5 text-[11px] text-ink-tertiary", "Paused" }
                        }
                    }
                    div { class: "text-[12px] text-ink-secondary", "{bot} · {when}" }
                    div { class: "text-[11px] text-ink-tertiary",
                        if let Some(next) = &next { "Next: {next}" } else if on { "Doesn't run again" } else { "Not running" }
                        if let Some((status, words)) = &last {
                            span { class: if status == "failed" { "text-danger" } else { "" }, " · {words}" }
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-1",
                button { r#type: "button", class: LINK, disabled: busy(), "data-trigger-run": true, onclick: move |_| act.call(("POST", format!("/api/triggers/{run_id}/run"), Some(json!({})))), "Run now" }
                button { r#type: "button", class: LINK, disabled: busy(), "data-trigger-pause": true, onclick: move |_| act.call(("PUT", format!("/api/triggers/{pause_id}"), Some(json!({ "enabled": !on })))), if on { "Pause" } else { "Resume" } }
                button { r#type: "button", class: LINK, onclick: move |_| on_edit.call(editable.clone()), "Edit" }
                if let Some(thread) = thread {
                    button { r#type: "button", class: LINK, "data-trigger-chat": true,
                        onclick: move |_| {
                            crate::chats::open(state, ui, &bot_id, &thread);
                            on_close.call(());
                        },
                        "Open chat"
                    }
                }
                button { r#type: "button", class: LINK,
                    onclick: move |_| {
                        if history().is_some() {
                            history.set(None);
                            return;
                        }
                        let path = format!("/api/triggers/{history_id}/runs");
                        spawn(async move {
                            if let Ok(found) = api::get(&path).await {
                                history.set(Some(found["runs"].as_array().cloned().unwrap_or_default()));
                            }
                        });
                    },
                    if history().is_some() { "Hide history" } else { "History" }
                }
                span { class: "flex-1" }
                if confirm() {
                    button { r#type: "button", class: "rounded-md bg-danger px-2 py-1 text-[12px] font-medium text-white", "data-trigger-delete": true, onclick: move |_| act.call(("DELETE", format!("/api/triggers/{delete_id}"), None)), "Delete" }
                    button { r#type: "button", class: LINK, onclick: move |_| confirm.set(false), "Keep" }
                } else {
                    button { r#type: "button", class: "{LINK} hover:text-danger", onclick: move |_| confirm.set(true), "Delete…" }
                }
            }
            if confirm() {
                p { class: "text-[11px] text-ink-tertiary", "Its chat stays in {bot}'s chats." }
            }
            if let Some(runs) = history() {
                div { class: "flex flex-col gap-1 border-t border-hairline/30 pt-1", "data-trigger-history": true,
                    if runs.is_empty() {
                        p { class: "text-[12px] text-ink-tertiary", "It hasn't run yet." }
                    }
                    for run in runs.iter().take(20) {
                        {
                            let at = run["startedAt"].as_str().map(parse_time).filter(|t| !t.is_nan()).map(short_when).unwrap_or_default();
                            let status = run["status"].as_str().unwrap_or_default().to_string();
                            let source = match run["source"].as_str() { Some("manual") => "by hand", Some("schedule") => "on time", Some(other) => other, None => "" }.to_string();
                            let said = run["output"].as_str().or(run["error"].as_str()).unwrap_or_default().lines().next().unwrap_or_default().chars().take(140).collect::<String>();
                            rsx! {
                                div { key: "{run[\"id\"].as_str().unwrap_or_default()}", class: "flex gap-2 text-[12px]",
                                    span { class: "w-28 shrink-0 text-ink-tertiary", "{at}" }
                                    span { class: if status == "failed" { "w-16 shrink-0 text-danger" } else { "w-16 shrink-0 text-ink-secondary" }, "{status}" }
                                    span { class: "w-14 shrink-0 text-ink-tertiary", "{source}" }
                                    span { class: "min-w-0 flex-1 truncate text-ink-secondary", "{said}" }
                                }
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

/// What the form's "When" says.
struct When {
    kind: String,
    at: String,
    time: String,
    days: Vec<u32>,
    every: String,
    from: String,
    to: String,
    start: String,
}

impl When {
    /// The schedule it describes.
    fn schedule(&self) -> Value {
        match self.kind.as_str() {
            "once" => json!({ "type": "once", "at": self.at }),
            "interval" => {
                let mut s = json!({ "type": "interval", "everyMinutes": self.every.trim().parse::<u32>().unwrap_or(0), "start": self.start });
                if self.days.len() < 7 {
                    s["days"] = json!(self.days);
                }
                if !self.from.is_empty() && !self.to.is_empty() {
                    s["from"] = json!(self.from);
                    s["to"] = json!(self.to);
                }
                s
            }
            _ => json!({ "type": "weekly", "time": self.time, "days": self.days }),
        }
    }
}

/// Now, as a form's date-and-time box shows it: "2026-11-17T09:00".
fn local_now(plus_minutes: f64) -> String {
    let at = crate::chat::js_now() + plus_minutes * 60_000.0;
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new(&at.into());
        format!("{:04}-{:02}-{:02}T{:02}:{:02}", d.get_full_year(), d.get_month() + 1, d.get_date(), d.get_hours(), d.get_minutes())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone};
        Local.timestamp_millis_opt(at as i64).single().map(|t| t.format("%Y-%m-%dT%H:%M").to_string()).unwrap_or_default()
    }
}

#[component]
fn TriggerForm(trigger: Value, on_done: EventHandler<()>) -> Element {
    let state = use_context::<Signal<State>>();
    let is_new = trigger.is_null();
    let s = trigger["schedule"].clone();
    let start_kind = s["type"].as_str().unwrap_or("weekly").to_string();
    let start_days: Vec<u32> = s["days"].as_array().map(|d| d.iter().filter_map(|x| x.as_u64().map(|x| x as u32)).collect()).unwrap_or_else(|| vec![1, 2, 3, 4, 5]);
    let first_bot = state.read().bots.iter().find(|b| b.channel.is_none()).map(|b| b.id.clone()).unwrap_or_default();
    let mut bot = use_signal(|| trigger["botId"].as_str().map(str::to_string).unwrap_or(first_bot));
    let mut name = use_signal(|| trigger["name"].as_str().unwrap_or_default().to_string());
    let mut prompt = use_signal(|| trigger["prompt"].as_str().unwrap_or_default().to_string());
    let mut kind = use_signal(|| start_kind);
    let mut at = use_signal(|| s["at"].as_str().map(|a| a.chars().take(16).collect()).unwrap_or_else(|| local_now(60.0)));
    let mut time = use_signal(|| s["time"].as_str().unwrap_or("09:00").to_string());
    let mut days = use_signal(|| start_days);
    let mut every = use_signal(|| s["everyMinutes"].as_u64().map(|m| m.to_string()).unwrap_or_else(|| "60".into()));
    let mut from = use_signal(|| s["from"].as_str().unwrap_or_default().to_string());
    let mut to = use_signal(|| s["to"].as_str().unwrap_or_default().to_string());
    let start = s["start"].as_str().map(|a| a.chars().take(16).collect()).unwrap_or_else(|| local_now(0.0));
    let mut problem = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let id = trigger["id"].as_str().unwrap_or_default().to_string();
    let save = move |_| {
        let schedule = When { kind: kind(), at: at(), time: time(), days: days(), every: every(), from: from(), to: to(), start: start.clone() }.schedule();
        let mut body = json!({ "name": name(), "prompt": prompt(), "schedule": schedule });
        if is_new {
            body["botId"] = json!(bot());
        }
        let (method, path) = if is_new { ("POST", "/api/triggers".to_string()) } else { ("PUT", format!("/api/triggers/{id}")) };
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call(method, &path, Some(&body)).await {
                Ok(_) => on_done.call(()),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone())).collect();
    let day_toggles = move || {
        rsx! {
            div { class: "flex flex-wrap gap-1",
                for (n, label) in DAYS {
                    {
                        let on = days.read().contains(&n);
                        rsx! {
                            button { key: "{n}", r#type: "button", "aria-pressed": "{on}", "data-trigger-day": "{n}",
                                class: if on { "rounded-full bg-accent px-2.5 py-1 text-[12px] font-medium text-white" } else { "rounded-full bg-control px-2.5 py-1 text-[12px] text-ink-secondary" },
                                onclick: move |_| {
                                    let mut d = days();
                                    if d.contains(&n) { d.retain(|x| *x != n) } else { d.push(n); d.sort_unstable() }
                                    days.set(d);
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
    };
    rsx! {
        div { class: "flex flex-col gap-3", "data-trigger-form": true,
            if is_new {
                label { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Bot"
                    select { class: FIELD, "data-trigger-bot": true, onchange: move |e| bot.set(e.value()),
                        for (id, name) in bots {
                            option { key: "{id}", value: "{id}", selected: bot() == id, "{name}" }
                        }
                    }
                }
            }
            label { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Name"
                input { class: FIELD, "data-trigger-name": true, placeholder: "Morning inbox", value: "{name}", oninput: move |e| name.set(e.value()) }
            }
            label { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "What the bot should do each time"
                textarea { class: "{FIELD} min-h-24", "data-trigger-prompt": true, placeholder: "Sum up what came into my inbox since yesterday, most important first.", value: "{prompt}", oninput: move |e| prompt.set(e.value()) }
            }
            div { class: "flex flex-col gap-2",
                span { class: "text-[12px] text-ink-secondary", "When" }
                div { role: "radiogroup", class: "flex gap-1",
                    for (value, label) in [("weekly", "On days at a time"), ("interval", "Every so often"), ("once", "Once")] {
                        button { key: "{value}", r#type: "button", role: "radio", "aria-checked": "{kind() == value}", "data-trigger-kind": "{value}",
                            class: if kind() == value { "rounded-lg bg-accent px-3 py-1.5 text-[12px] font-medium text-white" } else { "rounded-lg bg-control px-3 py-1.5 text-[12px] text-ink-secondary" },
                            onclick: move |_| kind.set(value.to_string()),
                            "{label}"
                        }
                    }
                }
                match kind().as_str() {
                    "once" => rsx! {
                        input { class: "{SMALL} w-56", r#type: "datetime-local", "data-trigger-at": true, value: "{at}", oninput: move |e| at.set(e.value()) }
                    },
                    "interval" => rsx! {
                        div { class: "flex items-center gap-2 text-[13px] text-ink-secondary",
                            "Every"
                            input { class: "{SMALL} w-20", r#type: "number", min: "5", max: "1440", "data-trigger-every": true, value: "{every}", oninput: move |e| every.set(e.value()) }
                            "minutes"
                        }
                        {day_toggles()}
                        div { class: "flex items-center gap-2 text-[13px] text-ink-secondary",
                            "Only between"
                            input { class: "{SMALL} w-28", r#type: "time", value: "{from}", oninput: move |e| from.set(e.value()) }
                            "and"
                            input { class: "{SMALL} w-28", r#type: "time", value: "{to}", oninput: move |e| to.set(e.value()) }
                            span { class: "text-[11px] text-ink-tertiary", "(optional)" }
                        }
                    },
                    _ => rsx! {
                        {day_toggles()}
                        div { class: "flex items-center gap-2 text-[13px] text-ink-secondary",
                            "at"
                            input { class: "{SMALL} w-28", r#type: "time", "data-trigger-time": true, value: "{time}", oninput: move |e| time.set(e.value()) }
                        }
                    },
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
            div { class: "flex justify-end gap-2",
                button { r#type: "button", class: BUTTON, onclick: move |_| on_done.call(()), "Cancel" }
                button { r#type: "button", class: PRIMARY, "data-trigger-save": true, disabled: busy(), onclick: save, if is_new { "Make it" } else { "Save" } }
            }
        }
    }
}
