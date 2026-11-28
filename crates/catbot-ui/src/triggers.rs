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
pub fn TriggersDialog(on_close: EventHandler<()>, #[props(default)] edit: Option<Value>) -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let mut list = use_signal(Vec::<Value>::new);
    let mut problem = use_signal(|| None::<String>);
    // None: the list. Some(Null): a new trigger. Some(trigger): changing that one.
    let mut editing = use_signal(move || edit.clone());
    use_effect(move || {
        let _ = (rev.triggers)();
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
    let mut showing_address = use_signal(|| false);
    let has_address = trigger["hasAddress"].as_bool().unwrap_or(false);
    let watches_mail = trigger["kind"] == "email";
    let watches_app = trigger["appEvent"]["appName"].as_str().map(str::to_string);
    let problem_now = trigger["problem"].as_str().map(str::to_string);
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
                    if let Some(p) = &problem_now {
                        div { class: "text-[12px] text-danger", "Can't look: {p}" }
                    }
                    div { class: "text-[11px] text-ink-tertiary",
                        if has_address { if on { "Waiting for its address to be called" } else { "Not listening" } } else if let Some(app_name) = &watches_app { if on { "Waiting for {app_name} to send an event" } else { "Not listening" } } else if watches_mail { if on { "Looks at the mailbox every 5 minutes" } else { "Not looking" } } else if let Some(next) = &next { "Next: {next}" } else if on { "Doesn't run again" } else { "Not running" }
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
                if has_address {
                    button { r#type: "button", class: LINK, "data-trigger-address": true, onclick: move |_| showing_address.set(!showing_address()), if showing_address() { "Hide address" } else { "Address" } }
                }
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
            if showing_address() {
                Address { id: id.clone() }
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
                            let source = match run["source"].as_str() { Some("manual") => "by hand", Some("schedule") => "on time", Some("app") => "app event", Some(other) => other, None => "" }.to_string();
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
    let start_kind = match trigger["kind"].as_str() {
        Some("webhook") => "webhook".to_string(),
        Some("email") => "email".to_string(),
        Some("app") => "app".to_string(),
        _ => s["type"].as_str().unwrap_or("weekly").to_string(),
    };
    // A trigger keeps its kind: a schedule stays a schedule, a webhook a webhook, and so on.
    let existing_kind = (!trigger.is_null()).then(|| trigger["kind"].as_str().unwrap_or("schedule").to_string());
    let mut mail_from = use_signal(|| trigger["email"]["from"].as_str().unwrap_or_default().to_string());
    let mut mail_subject = use_signal(|| trigger["email"]["subject"].as_str().unwrap_or_default().to_string());
    let mut mail_folder = use_signal(|| trigger["email"]["folder"].as_str().unwrap_or("INBOX").to_string());
    // An app event: the app, the event, and its settings (name, value).
    let mut app_toolkit = use_signal(|| trigger["appEvent"]["toolkit"].as_str().unwrap_or_default().to_string());
    let mut app_event = use_signal(|| trigger["appEvent"]["event"].as_str().unwrap_or_default().to_string());
    let mut app_config = use_signal(|| {
        trigger["appEvent"]["config"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect::<Vec<_>>()).unwrap_or_default()
    });
    let mut composio_apps = use_signal(Vec::<Value>::new);
    let mut app_events = use_signal(Vec::<Value>::new);
    let start_days: Vec<u32> = s["days"].as_array().map(|d| d.iter().filter_map(|x| x.as_u64().map(|x| x as u32)).collect()).unwrap_or_else(|| vec![1, 2, 3, 4, 5]);
    let first_bot = state.read().bots.iter().find(|b| b.channel.is_none()).map(|b| b.id.clone()).unwrap_or_default();
    let mut bot = use_signal(|| trigger["botId"].as_str().map(str::to_string).unwrap_or(first_bot));
    let mut name = use_signal(|| trigger["name"].as_str().unwrap_or_default().to_string());
    let mut prompt = use_signal(|| trigger["prompt"].as_str().unwrap_or_default().to_string());
    let mut kind = use_signal(|| start_kind);
    use_effect(move || {
        if kind() != "app" {
            return;
        }
        spawn(async move {
            if let Ok(found) = api::get("/api/composio").await {
                composio_apps.set(found["apps"].as_array().cloned().unwrap_or_default());
            }
        });
    });
    use_effect(move || {
        let toolkit = app_toolkit();
        if kind() != "app" || toolkit.is_empty() {
            return;
        }
        spawn(async move {
            if let Ok(found) = api::get(&format!("/api/composio/events?search={toolkit}")).await {
                app_events.set(found["events"].as_array().cloned().unwrap_or_default());
            }
        });
    });
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
    // A webhook just made: its address, shown before the form closes.
    let mut made = use_signal(|| None::<String>);
    let save = move |_| {
        let mut body = json!({ "name": name(), "prompt": prompt() });
        if kind() == "webhook" {
            if is_new {
                body["kind"] = json!("webhook");
            }
        } else if kind() == "app" {
            if is_new {
                body["kind"] = json!("app");
                let config: serde_json::Map<String, Value> = app_config()
                    .into_iter()
                    .filter(|(_, v)| !v.trim().is_empty())
                    .map(|(k, v)| {
                        let typed = v.trim().parse::<i64>().map(Value::from).or_else(|_| v.trim().parse::<bool>().map(Value::from)).unwrap_or(Value::from(v));
                        (k, typed)
                    })
                    .collect();
                body["app"] = json!({ "toolkit": app_toolkit(), "event": app_event(), "config": config });
            }
        } else if kind() == "email" {
            if is_new {
                body["kind"] = json!("email");
            }
            body["email"] = json!({ "from": mail_from(), "subject": mail_subject(), "folder": mail_folder() });
        } else {
            body["schedule"] = When { kind: kind(), at: at(), time: time(), days: days(), every: every(), from: from(), to: to(), start: start.clone() }.schedule();
        }
        if is_new {
            body["botId"] = json!(bot());
        }
        let (method, path) = if is_new { ("POST", "/api/triggers".to_string()) } else { ("PUT", format!("/api/triggers/{id}")) };
        let webhook = kind() == "webhook" && is_new;
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call(method, &path, Some(&body)).await {
                Ok(answer) if webhook => made.set(answer["trigger"]["id"].as_str().map(str::to_string)),
                Ok(_) => on_done.call(()),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    if let Some(made_id) = made() {
        return rsx! {
            div { class: "flex flex-col gap-3",
                p { class: "text-[13px] text-ink", "Made. Paste this address into the app that will call it:" }
                Address { id: made_id }
                div { class: "flex justify-end",
                    button { r#type: "button", class: PRIMARY, onclick: move |_| on_done.call(()), "Done" }
                }
            }
        };
    }
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
                div { role: "radiogroup", class: "flex flex-wrap gap-1",
                    for (value, label) in [("weekly", "On days at a time"), ("interval", "Every so often"), ("once", "Once"), ("webhook", "When called (webhook)"), ("email", "When an email arrives"), ("app", "When something happens in an app")] {
                        button { key: "{value}", r#type: "button", role: "radio", "aria-checked": "{kind() == value}", "data-trigger-kind": "{value}",
                            disabled: existing_kind.as_deref().is_some_and(|k| {
                                let this = match value { "webhook" => "webhook", "email" => "email", "app" => "app", _ => "schedule" };
                                k != this
                            }),
                            class: if kind() == value { "whitespace-nowrap rounded-lg bg-accent px-3 py-1.5 text-[12px] font-medium text-white" } else { "whitespace-nowrap rounded-lg bg-control px-3 py-1.5 text-[12px] text-ink-secondary" },
                            onclick: move |_| kind.set(value.to_string()),
                            "{label}"
                        }
                    }
                }
                match kind().as_str() {
                    "app" if !is_new => rsx! {
                        p { class: "text-[12px] text-ink-secondary", "It watches for the same event. To watch for another, make a new trigger." }
                    },
                    "app" => rsx! {
                        p { class: "text-[12px] leading-relaxed text-ink-secondary",
                            "Composio watches the app and sends each event to this trigger's own address at the remote door, so it needs the remote door's public address (Devices). The bot has to be ticked for the app (Settings › Apps)."
                        }
                        if composio_apps.read().is_empty() {
                            p { class: "text-[12px] text-warning", "No apps are connected through Composio yet (Settings › Apps)." }
                        }
                        div { class: "flex flex-wrap items-center gap-2 text-[13px] text-ink-secondary",
                            "App"
                            select { class: "{SMALL} w-44", "data-trigger-app": true,
                                onchange: move |e| { app_toolkit.set(e.value()); app_event.set(String::new()); app_config.set(Vec::new()); },
                                option { value: "", selected: app_toolkit().is_empty(), "Choose…" }
                                for a in composio_apps() {
                                    {
                                        let tk = a["toolkit"].as_str().unwrap_or_default().to_string();
                                        let nm = a["name"].as_str().unwrap_or_default().to_string();
                                        rsx! { option { key: "{tk}", value: "{tk}", selected: app_toolkit() == tk, "{nm}" } }
                                    }
                                }
                            }
                            "Event"
                            select { class: "{SMALL} min-w-0 flex-1", "data-trigger-app-event": true,
                                onchange: move |e| {
                                    let chosen = e.value();
                                    // Its settings, empty to fill in.
                                    let fields: Vec<(String, String)> = app_events().iter().find(|x| x["event"] == chosen.as_str()).and_then(|x| x["config"]["properties"].as_object().map(|o| o.keys().map(|k| (k.clone(), String::new())).collect())).unwrap_or_default();
                                    app_config.set(fields);
                                    app_event.set(chosen);
                                },
                                option { value: "", selected: app_event().is_empty(), "Choose…" }
                                for ev in app_events() {
                                    {
                                        let slug = ev["event"].as_str().unwrap_or_default().to_string();
                                        let nm = ev["name"].as_str().unwrap_or_default().to_string();
                                        rsx! { option { key: "{slug}", value: "{slug}", selected: app_event() == slug, "{nm}" } }
                                    }
                                }
                            }
                        }
                        for (i, (field, value)) in app_config().into_iter().enumerate() {
                            {
                                let hint = app_events().iter().find(|x| x["event"] == app_event().as_str()).and_then(|x| x["config"]["properties"][&field]["description"].as_str().map(str::to_string)).unwrap_or_default();
                                rsx! {
                                    label { key: "{field}", class: "flex flex-col gap-0.5 text-[12px] text-ink-secondary",
                                        "{field}"
                                        input { class: "{SMALL}", placeholder: "{hint}", value: "{value}", oninput: move |e| { app_config.write()[i].1 = e.value(); } }
                                    }
                                }
                            }
                        }
                    },
                    "email" => rsx! {
                        p { class: "text-[12px] leading-relaxed text-ink-secondary",
                            "Catbot looks at the mailbox (Settings › Email) every 5 minutes. Mail already there doesn't count; new mail that fits goes to the bot, several at once in one run. The bot has to be one that may use the mailbox."
                        }
                        div { class: "flex flex-wrap items-center gap-2 text-[13px] text-ink-secondary",
                            "From"
                            input { class: "{SMALL} w-48", placeholder: "anyone", "data-trigger-mail-from": true, value: "{mail_from}", oninput: move |e| mail_from.set(e.value()) }
                            "about"
                            input { class: "{SMALL} w-48", placeholder: "anything", "data-trigger-mail-subject": true, value: "{mail_subject}", oninput: move |e| mail_subject.set(e.value()) }
                            "in"
                            input { class: "{SMALL} w-28", value: "{mail_folder}", oninput: move |e| mail_folder.set(e.value()) }
                        }
                    },
                    "webhook" => rsx! {
                        p { class: "text-[12px] leading-relaxed text-ink-secondary",
                            "Another app calls its address (a form, GitHub, a home-automation hub) and the bot gets to work with what it sent. You'll get the address once it's made."
                        }
                    },
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

/// What a bot did to one of its triggers, in its chat (stage 14.2): the trigger as it is now,
/// with Pause or Resume, Undo, and Edit. A bot sets triggers up at once (Moo's choice); this
/// card is how the person sees it and takes it back.
#[component]
pub fn TriggerCard(card: Value) -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let id = card["id"].as_str().unwrap_or_default().to_string();
    let action = card["action"].as_str().unwrap_or_default().to_string();
    let said = card["said"].as_str().unwrap_or("A trigger changed").to_string();
    let mut now = use_signal(|| None::<Value>);
    let mut gone = use_signal(|| false);
    let mut undone = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut editing = use_signal(|| false);
    let look_id = id.clone();
    use_effect(move || {
        let _ = (rev.triggers)();
        let path = format!("/api/triggers/{look_id}");
        spawn(async move {
            match api::get(&path).await {
                Ok(found) => {
                    now.set(Some(found["trigger"].clone()));
                    gone.set(false);
                }
                Err(_) => gone.set(true),
            }
        });
    });
    let current = now();
    let name = current.as_ref().and_then(|t| t["name"].as_str()).or(card["name"].as_str()).unwrap_or_default().to_string();
    let when = current.as_ref().and_then(|t| t["when"].as_str()).or(card["when"].as_str()).unwrap_or_default().to_string();
    let on = current.as_ref().is_some_and(|t| t["enabled"] == true);
    let next = current.as_ref().and_then(|t| t["nextAt"].as_str()).map(when_text);
    let status = if gone() {
        "Deleted".to_string()
    } else if !on {
        "Paused".to_string()
    } else {
        next.map(|n| format!("Next: {n}")).unwrap_or_else(|| "Doesn't run again".into())
    };
    let act = use_callback(move |(method, path, body, undoing): (&'static str, String, Option<Value>, bool)| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call(method, &path, body.as_ref()).await {
                Ok(_) => {
                    if undoing {
                        undone.set(true);
                    }
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    });
    // What Undo does: take a new trigger away, put a changed one back as it was, or bring a
    // deleted one back.
    let undo = match action.as_str() {
        "created" => Some(("DELETE", format!("/api/triggers/{id}"), None)),
        "deleted" => {
            let d = &card["deleted"];
            Some(("POST", "/api/triggers".to_string(), Some(json!({ "botId": d["botId"], "name": d["name"], "prompt": d["prompt"], "schedule": d["schedule"], "kind": d["kind"], "email": d["email"] }))))
        }
        _ if card["before"].is_object() => Some(("PUT", format!("/api/triggers/{id}"), Some(card["before"].clone()))),
        _ => None,
    };
    let can_undo = undo.is_some() && !undone() && (action == "deleted" || !gone());
    let pause_path = format!("/api/triggers/{id}");
    rsx! {
        div { class: "flex w-full justify-start", "data-trigger-card": "{id}",
            div { class: "flex w-full max-w-[min(var(--chat-prose-max),var(--chat-card-pct))] flex-col gap-1 rounded-xl border border-hairline/50 bg-card px-3 py-2.5 text-[13px]",
                div { class: "flex items-center gap-2 text-[12px] text-ink-secondary",
                    // Lucide `Zap` (ISC).
                    svg { "aria-hidden": "true", width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z" }
                    }
                    "{said}"
                }
                div { class: "font-semibold text-ink", "{name}" }
                div { class: "text-ink-secondary", "{when} · {status}" }
                if undone() {
                    p { class: "text-ink-tertiary", "Undone." }
                }
                if card["kind"] == "webhook" && !gone() && current.is_some() {
                    Address { id: id.clone() }
                }
                div { class: "flex flex-wrap gap-1 pt-1",
                    if !gone() && current.is_some() {
                        button { r#type: "button", class: LINK, disabled: busy(), "data-trigger-card-pause": true,
                            onclick: move |_| act.call(("PUT", pause_path.clone(), Some(json!({ "enabled": !on })), false)),
                            if on { "Pause" } else { "Resume" }
                        }
                        button { r#type: "button", class: LINK, "data-trigger-card-edit": true, onclick: move |_| editing.set(true), "Edit" }
                    }
                    if can_undo {
                        if let Some((method, path, body)) = undo.clone() {
                            button { r#type: "button", class: LINK, disabled: busy(), "data-trigger-card-undo": true,
                                onclick: move |_| act.call((method, path.clone(), body.clone(), true)),
                                "Undo"
                            }
                        }
                    }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "text-[12px] text-danger", "{err}" }
                }
            }
        }
        if editing() {
            if let Some(trigger) = current.clone() {
                TriggersDialog { edit: Some(trigger), on_close: move |_| editing.set(false) }
            }
        }
    }
}

/// A webhook's address (`server/hooks.rs`), with Copy, and New secret to retire the old one.
/// The bot that made it never sees it; the person pastes it into the app that will call it.
#[component]
fn Address(id: String) -> Element {
    let mut address = use_signal(|| None::<Value>);
    let mut asking = use_signal(|| false);
    let mut copied = use_signal(|| None::<&'static str>);
    let mut problem = use_signal(|| None::<String>);
    let look = format!("/api/triggers/{id}/address");
    use_effect(move || {
        let path = look.clone();
        spawn(async move {
            match api::get(&path).await {
                Ok(found) => address.set(Some(found)),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let renew = format!("/api/triggers/{id}/secret");
    let copy = use_callback(move |(what, text): (&'static str, String)| {
        let text = serde_json::to_string(&text).unwrap_or_default();
        document::eval(&format!("navigator.clipboard?.writeText({text});"));
        copied.set(Some(what));
    });
    let Some(a) = address() else {
        return rsx! {
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        };
    };
    let url = a["url"].as_str().unwrap_or_default().to_string();
    let bearer_url = a["bearerUrl"].as_str().unwrap_or_default().to_string();
    let secret = a["secret"].as_str().unwrap_or_default().to_string();
    let public = a["public"].as_bool().unwrap_or(false);
    let (u, s) = (url.clone(), secret.clone());
    rsx! {
        div { class: "flex flex-col gap-1.5 rounded-lg bg-inset px-3 py-2", "data-webhook-address": true,
            div { class: "flex items-center gap-2",
                input { class: "min-w-0 flex-1 bg-transparent font-mono text-[12px] text-ink focus:outline-none", readonly: true, value: "{url}", "data-webhook-url": true }
                button { r#type: "button", class: LINK, onclick: move |_| copy.call(("address", u.clone())), if copied() == Some("address") { "Copied" } else { "Copy address" } }
            }
            p { class: "text-[11px] leading-relaxed text-ink-tertiary",
                "Send it with POST. If the app can add a header, use {bearer_url} with "
                code { "Authorization: Bearer" }
                " and the secret instead; that keeps the secret out of its logs. "
                button { r#type: "button", class: "underline hover:text-ink", onclick: move |_| copy.call(("secret", s.clone())), if copied() == Some("secret") { "Copied" } else { "Copy secret" } }
            }
            if !public {
                p { class: "text-[11px] text-warning", "Only this computer can reach it until the remote door has a public address (Devices)." }
            }
            div { class: "flex gap-1",
                if asking() {
                    span { class: "text-[11px] text-ink-secondary", "The old address stops working." }
                    button { r#type: "button", class: LINK, "data-webhook-renew": true,
                        onclick: move |_| {
                            let path = renew.clone();
                            spawn(async move {
                                match api::post(&path, &json!({})).await {
                                    Ok(found) => address.set(Some(found)),
                                    Err(err) => problem.set(Some(err)),
                                }
                                asking.set(false);
                                copied.set(None);
                            });
                        },
                        "Make a new secret"
                    }
                    button { r#type: "button", class: LINK, onclick: move |_| asking.set(false), "Keep it" }
                } else {
                    button { r#type: "button", class: LINK, onclick: move |_| asking.set(true), "New secret…" }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}
