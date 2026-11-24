//! Scheduled messages (stage 21): the clock beside Send, and the chat's Scheduled list.
//!
//! Moo's choices (2026-11-23): a clock button by Send with quick times (in an hour, tonight,
//! tomorrow morning) and a date and time of your own; what's waiting is in a "Scheduled" list
//! above the message box, not in the chat; one due while Catbot was off goes out when it's back,
//! marked late. In bot chats and channels, not the desktop pet's chat. Catbot's server sends them
//! (`server/scheduled.rs`), so the app can be closed.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::chat::{ahead_when, js_now};
use crate::live::State;
use crate::sidebar::Icon;

/// A message set to go out later, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub text: String,
    pub at: f64,
    /// "waiting" or "failed" (sent ones leave the list).
    pub state: String,
    pub error: Option<String>,
}

impl Item {
    fn from_wire(wire: &Value) -> Option<Item> {
        Some(Item {
            id: wire["id"].as_str()?.to_string(),
            text: wire["text"].as_str().unwrap_or_default().to_string(),
            at: wire["at"].as_f64()?,
            state: wire["state"].as_str().unwrap_or("waiting").to_string(),
            error: wire["error"].as_str().map(str::to_string),
        })
    }
}

/// The quick times: in an hour; tonight at 8 (while that's still ahead); tomorrow at 9.
pub fn quick_times(now: f64) -> Vec<(&'static str, f64)> {
    let mut times = vec![("In 1 hour", now + 3_600_000.0)];
    let tonight = local_at(now, 0, 20);
    if tonight > now + 10.0 * 60_000.0 {
        times.push(("Tonight", tonight));
    }
    times.push(("Tomorrow morning", local_at(now, 1, 9)));
    times
}

/// `days` from `now`'s day, at `hour`:00 on this computer's clock.
fn local_at(now: f64, days: i32, hour: u32) -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new(&now.into());
        d.set_date((d.get_date() as i32 + days) as u32);
        d.set_hours(hour);
        d.set_minutes(0);
        d.set_seconds(0);
        d.set_milliseconds(0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone};
        let Some(today) = Local.timestamp_millis_opt(now as i64).single() else { return now };
        let date = today.date_naive() + chrono::Duration::days(i64::from(days));
        date.and_hms_opt(hour, 0, 0).and_then(|t| Local.from_local_datetime(&t).earliest()).map(|t| t.timestamp_millis() as f64).unwrap_or(now)
    }
}

/// A time as a date-and-time box shows it: "2026-11-24T20:00".
pub fn to_input(at: f64) -> String {
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

/// What a date-and-time box says, as a time (on this computer's clock).
pub fn from_input(value: &str) -> Option<f64> {
    if value.len() < 16 {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Without a zone, JavaScript reads "YYYY-MM-DDTHH:MM" as local time.
        let at = js_sys::Date::new(&value.into()).get_time();
        (!at.is_nan()).then_some(at)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone};
        let naive = chrono::NaiveDateTime::parse_from_str(&value[..16], "%Y-%m-%dT%H:%M").ok()?;
        Local.from_local_datetime(&naive).earliest().map(|t| t.timestamp_millis() as f64)
    }
}

/// Schedules the words for `at`. Err with the server's words if it says no.
pub async fn schedule(bot_id: &str, thread_id: &str, text: &str, at: f64) -> Result<(), String> {
    let body = json!({ "botId": bot_id, "threadId": thread_id, "text": text, "at": at as i64 });
    api::post("/api/scheduled", &body).await.map(|_| ())
}

/// The clock's menu, above the Send button: the quick times and a time of your own.
#[component]
pub fn Picker(on_pick: EventHandler<f64>, on_close: EventHandler<()>) -> Element {
    let now = js_now();
    let mut custom = use_signal(|| to_input(local_at(now, 1, 9)));
    let mut problem = use_signal(|| None::<String>);
    let pick_custom = move |_| {
        match from_input(&custom.peek()) {
            Some(at) if at > js_now() + 30_000.0 => on_pick.call(at),
            Some(_) => problem.set(Some("Choose a time that hasn't passed.".into())),
            None => problem.set(Some("Choose a date and a time.".into())),
        }
    };
    rsx! {
        // A click outside closes it.
        div { class: "fixed inset-0 z-[30]", onclick: move |_| on_close.call(()) }
        div {
            role: "menu",
            "data-schedule-menu": true,
            class: "absolute bottom-11 right-0 z-[31] flex w-[260px] flex-col gap-0.5 rounded-xl bg-panel p-1.5 text-[13px] text-ink shadow-lg ring-1 ring-ink/10",
            onkeydown: move |e| if e.key() == Key::Escape { on_close.call(()) },
            p { class: "px-2 pb-1 pt-1 text-[11px] font-semibold uppercase tracking-wide text-ink-tertiary", "Send later" }
            for (label, at) in quick_times(now) {
                button {
                    key: "{label}",
                    r#type: "button",
                    role: "menuitem",
                    "data-schedule-quick": "{label}",
                    class: "flex items-center justify-between gap-3 rounded-lg px-2 py-1.5 text-left hover:bg-raised",
                    onclick: move |_| on_pick.call(at),
                    span { class: "font-medium", "{label}" }
                    span { class: "text-[12px] text-ink-tertiary", "{ahead_when(at)}" }
                }
            }
            div { class: "mt-1 flex flex-col gap-1.5 border-t border-hairline/40 px-2 pb-1 pt-2",
                label { class: "text-[12px] text-ink-secondary", r#for: "schedule-custom", "Pick a date and time" }
                div { class: "flex items-center gap-1.5",
                    input {
                        id: "schedule-custom",
                        r#type: "datetime-local",
                        "data-schedule-custom": true,
                        class: "min-w-0 flex-1 rounded-md bg-raised px-2 py-1 text-[12px] text-ink ring-1 ring-ink/10 focus:outline-none focus:ring-ink/25",
                        value: "{custom}",
                        min: "{to_input(now)}",
                        oninput: move |e| {
                            problem.set(None);
                            custom.set(e.value());
                        },
                    }
                    button {
                        r#type: "button",
                        "data-schedule-set": true,
                        class: "shrink-0 rounded-md bg-ink px-2.5 py-1 text-[12px] font-medium text-app hover:opacity-90",
                        onclick: pick_custom,
                        "Schedule"
                    }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "text-[12px] text-danger", "{err}" }
                }
            }
        }
    }
}

/// "Scheduled · 2" above the message box, opening this chat's list: change a time, send now,
/// cancel. Shows nothing while nothing waits.
#[component]
pub fn List(bot_id: String, thread_id: String) -> Element {
    let state = use_context::<Signal<State>>();
    let mut open = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut items = use_signal(Vec::<Item>::new);
    // Read again for another chat, and whenever the server says the list changed.
    use_effect(use_reactive!(|bot_id, thread_id| {
        let _ = state.read().scheduled_changed;
        let (bot, thread) = (bot_id.clone(), thread_id.clone());
        spawn(async move {
            if let Ok(found) = api::get(&format!("/api/scheduled?botId={bot}&threadId={thread}")).await {
                let list: Vec<Item> = found["messages"].as_array().into_iter().flatten().filter_map(Item::from_wire).filter(|i| i.state != "sent").collect();
                items.set(list);
            }
        });
    }));
    let count = items.read().len();
    if count == 0 {
        return rsx! {};
    }
    let failed = items.read().iter().any(|i| i.state == "failed");
    let tone = if failed { "text-danger" } else { "text-ink-secondary" };
    let expanded = if open() { "true" } else { "false" };
    rsx! {
        div { class: "mb-2 flex flex-col items-start gap-1.5", "data-scheduled": true,
            button {
                r#type: "button",
                "data-scheduled-chip": true,
                "aria-expanded": "{expanded}",
                class: "flex items-center gap-1.5 rounded-full bg-panel px-2.5 py-1 text-[12px] font-medium {tone} ring-1 ring-ink/10 hover:bg-raised",
                onclick: move |_| open.toggle(),
                Icon { name: "clock", size: 13 }
                if failed { "Scheduled · {count} · one couldn't be sent" } else { "Scheduled · {count}" }
            }
            if open() {
                div { class: "flex w-full flex-col divide-y divide-hairline/40 rounded-xl bg-panel text-[13px] ring-1 ring-ink/10", "data-scheduled-list": true,
                    for item in items.read().iter().cloned() {
                        Entry { key: "{item.id}", item, on_problem: move |err| problem.set(err) }
                    }
                    if let Some(err) = problem() {
                        p { role: "alert", class: "px-3 py-2 text-[12px] text-danger", "{err}" }
                    }
                }
            }
        }
    }
}

/// One waiting (or failed) message in the list.
#[component]
fn Entry(item: Item, on_problem: EventHandler<Option<String>>) -> Element {
    let mut changing = use_signal(|| None::<String>);
    let failed = item.state == "failed";
    let when = ahead_when(item.at);
    let id = item.id.clone();
    let act = use_callback(move |(method, path, body): (&'static str, String, Option<Value>)| {
        spawn(async move {
            match api::call(method, &path, body.as_ref()).await {
                Ok(_) => on_problem.call(None),
                Err(err) => on_problem.call(Some(err)),
            }
        });
    });
    let (send_id, cancel_id, save_id) = (id.clone(), id.clone(), id.clone());
    let start_at = to_input(item.at);
    rsx! {
        div { class: "flex flex-col gap-1.5 px-3 py-2", "data-scheduled-item": "{id}",
            div { class: "flex items-baseline gap-2",
                span { class: if failed { "shrink-0 text-[12px] font-medium text-danger" } else { "shrink-0 text-[12px] font-medium text-ink-secondary" }, "data-scheduled-when": true,
                    if failed { "Couldn't send" } else { "{when}" }
                }
                span { class: "min-w-0 flex-1 truncate text-ink", title: "{item.text}", "{item.text}" }
            }
            if let Some(err) = item.error.clone().filter(|_| failed) {
                p { class: "text-[12px] text-ink-tertiary", "{err}" }
            }
            if let Some(value) = changing() {
                div { class: "flex items-center gap-1.5",
                    input {
                        r#type: "datetime-local",
                        "data-scheduled-time": true,
                        class: "min-w-0 rounded-md bg-raised px-2 py-1 text-[12px] text-ink ring-1 ring-ink/10 focus:outline-none",
                        value: "{value}",
                        min: "{to_input(js_now())}",
                        oninput: move |e| changing.set(Some(e.value())),
                    }
                    button { r#type: "button", "data-scheduled-save": true, class: "rounded-md bg-ink px-2.5 py-1 text-[12px] font-medium text-app hover:opacity-90",
                        onclick: move |_| {
                            let chosen = changing.peek().as_deref().and_then(from_input);
                            match chosen {
                                Some(at) if at > js_now() + 30_000.0 => {
                                    act(("PATCH", format!("/api/scheduled/{save_id}"), Some(json!({ "at": at as i64 }))));
                                    changing.set(None);
                                }
                                _ => on_problem.call(Some("Choose a time that hasn't passed.".into())),
                            }
                        },
                        "Save"
                    }
                    button { r#type: "button", class: "rounded-md px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised", onclick: move |_| changing.set(None), "Keep" }
                }
            } else {
                div { class: "flex items-center gap-1",
                    if !failed {
                        button { r#type: "button", "data-scheduled-change": true, class: "rounded-md px-2 py-0.5 text-[12px] text-ink-secondary hover:bg-raised hover:text-ink",
                            onclick: move |_| changing.set(Some(start_at.clone())),
                            "Change time"
                        }
                    }
                    button { r#type: "button", "data-scheduled-send": true, class: "rounded-md px-2 py-0.5 text-[12px] text-ink-secondary hover:bg-raised hover:text-ink",
                        onclick: move |_| act(("POST", format!("/api/scheduled/{send_id}/send"), Some(json!({})))),
                        if failed { "Try again" } else { "Send now" }
                    }
                    button { r#type: "button", "data-scheduled-cancel": true, class: "rounded-md px-2 py-0.5 text-[12px] text-ink-secondary hover:bg-raised hover:text-danger",
                        onclick: move |_| act(("DELETE", format!("/api/scheduled/{cancel_id}"), None)),
                        if failed { "Remove" } else { "Cancel" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_times_are_ahead_and_tonight_only_while_it_is() {
        let at = |h: u32| local_at(js_now(), 0, h);
        let morning = at(7);
        let times = quick_times(morning);
        assert_eq!(times.iter().map(|t| t.0).collect::<Vec<_>>(), ["In 1 hour", "Tonight", "Tomorrow morning"]);
        assert!(times.iter().all(|t| t.1 > morning));
        let late = at(21);
        assert_eq!(quick_times(late).iter().map(|t| t.0).collect::<Vec<_>>(), ["In 1 hour", "Tomorrow morning"]);
    }

    #[test]
    fn the_time_box_reads_back_what_it_shows() {
        let at = local_at(js_now(), 2, 14);
        assert_eq!(from_input(&to_input(at)), Some(at));
        assert_eq!(from_input("soon"), None);
    }
}
