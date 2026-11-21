//! Usage: the plans' limits up top, the way Claude's own usage page shows them (how much of the
//! current session and the week is used, when each resets, and whether that pace lasts), then
//! the breakdown of every turn by bot, model, AI or day (`server/usage.rs`).

use dioxus::prelude::*;
use serde_json::Value;

use crate::api;
use crate::chat::{clock, day, js_now, month_day, parse_time, weekday};

#[component]
/// `embedded`: as the Settings window's Spending page, without its own frame.
pub fn UsageDialog(on_close: EventHandler<()>, #[props(default)] embedded: bool) -> Element {
    let mut plans = use_signal(|| None::<Result<Value, String>>);
    let mut period = use_signal(|| "this-month".to_string());
    let mut by = use_signal(|| "bot".to_string());
    let mut table = use_signal(|| None::<Result<Value, String>>);
    let mut refreshed = use_signal(|| 0u32);
    let mut limits = use_signal(|| None::<Result<Value, String>>);
    let state = use_context::<Signal<crate::live::State>>();

    use_effect(move || {
        let fresh = if refreshed() > 0 { "?fresh=1" } else { "" };
        spawn(async move { plans.set(Some(api::get(&format!("/api/usage/plans{fresh}")).await)) });
    });
    use_effect(move || {
        let _ = refreshed();
        let _ = state.read().limits_changed;
        spawn(async move { limits.set(Some(api::get("/api/limits").await)) });
    });
    use_effect(move || {
        let (p, b) = (period(), by());
        let _ = refreshed();
        spawn(async move { table.set(Some(api::get(&format!("/api/usage?period={p}&by={b}")).await)) });
    });

    rsx! {
        div { class: if embedded { "contents" } else { "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 max-md:p-0" }, onclick: move |_| if !embedded { on_close.call(()) },
            div {
                role: if embedded { "region" } else { "dialog" },
                "aria-label": "Usage",
                "data-usage-dialog": true,
                class: if embedded { "flex flex-col" } else { "flex max-h-[88vh] w-full max-w-[760px] flex-col overflow-hidden rounded-2xl bg-panel shadow-2xl max-md:h-full max-md:max-h-none max-md:rounded-none" },
                onclick: move |e| e.stop_propagation(),
                div { class: if embedded { "hidden" } else { "flex h-12 shrink-0 items-center gap-1 border-b-[0.5px] border-edge pl-5 pr-2" },
                    span { class: "min-w-0 flex-1 text-[15px] font-semibold text-ink", "Usage" }
                    button { r#type: "button", "aria-label": "Refresh", title: "Refresh", class: ICON_BUTTON, onclick: move |_| *refreshed.write() += 1,
                        // Lucide `RefreshCw` (ISC).
                        svg { "aria-hidden": "true", width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                            path { d: "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" }
                            path { d: "M21 3v5h-5" }
                            path { d: "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" }
                            path { d: "M8 16H3v5" }
                        }
                    }
                    button { r#type: "button", "aria-label": "Close", class: ICON_BUTTON, onclick: move |_| on_close.call(()), "×" }
                }
                div { class: if embedded { "" } else { "min-h-0 flex-1 overflow-y-auto px-5 py-4" },
                    // The limits: the main thing.
                    match plans() {
                        None => rsx! { p { class: "text-[13px] text-ink-tertiary", "Reading the limits…" } },
                        Some(Err(err)) => rsx! { p { role: "alert", class: "text-[13px] text-danger", "{err}" } },
                        Some(Ok(found)) => {
                            let list = found["plans"].as_array().cloned().unwrap_or_default();
                            rsx! {
                                if list.is_empty() {
                                    p { class: "text-[13px] text-ink-secondary", "No bot here runs on a plan with limits Catbot can read." }
                                }
                                for plan in list {
                                    Plan { key: "{plan[\"engine\"]}", plan: plan.clone() }
                                }
                            }
                        }
                    }
                    // Catbot's own limits, and the spend.
                    if let Some(Ok(found)) = limits() {
                        Limits { found }
                    }
                    // The breakdown.
                    section { class: "mt-6 border-t-[0.5px] border-edge pt-4", "data-usage-breakdown": true,
                        div { class: "mb-3 flex flex-wrap items-center gap-2",
                            span { class: "min-w-0 flex-1 text-[15px] font-semibold text-ink", "Breakdown" }
                            select {
                                "aria-label": "Period",
                                "data-usage-period": true,
                                class: SELECT,
                                value: "{period}",
                                onchange: move |e| period.set(e.value()),
                                option { value: "this-month", "This month" }
                                option { value: "last-month", "Last month" }
                                option { value: "30d", "Last 30 days" }
                            }
                            select {
                                "aria-label": "Group by",
                                "data-usage-by": true,
                                class: SELECT,
                                value: "{by}",
                                onchange: move |e| by.set(e.value()),
                                option { value: "bot", "By bot" }
                                option { value: "model", "By model" }
                                option { value: "engine", "By AI" }
                                option { value: "day", "By day" }
                            }
                        }
                        match table() {
                            None => rsx! { p { class: "text-[13px] text-ink-tertiary", "Adding it up…" } },
                            Some(Err(err)) => rsx! { p { role: "alert", class: "text-[13px] text-danger", "{err}" } },
                            Some(Ok(found)) => rsx! { Breakdown { found } },
                        }
                    }
                }
            }
        }
    }
}

const ICON_BUTTON: &str = "flex size-10 shrink-0 items-center justify-center rounded-md text-ink-secondary hover:bg-control hover:text-ink";
const SELECT: &str = "h-8 rounded-lg bg-raised px-2 text-[13px] text-ink outline-none";

/// One plan: its forecast, its session and week bars, and its other windows folded under the week.
#[component]
fn Plan(plan: Value) -> Element {
    let mut more = use_signal(|| false);
    let name = plan["name"].as_str().unwrap_or("Plan").to_string();
    let kind = plan["plan"].as_str().map(|p| format!(" ({} plan)", capital(p))).unwrap_or_default();
    if plan["ok"] != true {
        let error = plan["error"].as_str().unwrap_or("Its usage couldn't be read.").to_string();
        return rsx! {
            div { class: "mb-5", "data-plan": "{name}",
                p { class: "text-[13px] font-semibold text-ink", "{name}{kind}" }
                p { class: "text-[13px] text-ink-secondary", "{error}" }
            }
        };
    }
    let headline = headline(&plan["headline"]);
    let windows = plan["windows"].as_array().cloned().unwrap_or_default();
    let extra = plan["extra"].as_array().cloned().unwrap_or_default();
    let has_extra = !extra.is_empty();
    rsx! {
        div { class: "mb-5 flex flex-col", "data-plan": "{name}",
            p { class: "mb-1 text-[12px] font-semibold uppercase tracking-wide text-ink-tertiary", "{name}{kind}" }
            if let Some(line) = headline {
                p { "data-plan-headline": true, class: "border-b-[0.5px] border-edge pb-3 text-[15px] font-semibold text-ink", "{line}" }
            }
            for window in windows {
                {
                    let week = window["kind"] == "week";
                    let tone = if week { "bg-[#2f7ae5]" } else { "bg-[#f4b11a]" };
                    rsx! {
                        Bar {
                            key: "{window[\"label\"]}",
                            label: window["label"].as_str().unwrap_or_default().to_string(),
                            used: window["usedPercent"].as_f64().unwrap_or(0.0),
                            resets: window["resetsAt"].as_str().map(str::to_string),
                            tone,
                            folding: week && has_extra,
                            open: more(),
                            on_fold: move |_| more.toggle(),
                        }
                    }
                }
            }
            if more() {
                for line in extra {
                    Bar {
                        key: "{line[\"label\"]}",
                        label: line["label"].as_str().unwrap_or_default().to_string(),
                        used: line["usedPercent"].as_f64().unwrap_or(0.0),
                        resets: line["resetsAt"].as_str().map(str::to_string),
                        tone: "bg-[#2f7ae5]/70",
                        small: true,
                    }
                }
            }
        }
    }
}

/// A limit as a bar: its name and reset time on the left, the bar, then how much is used.
#[component]
pub fn Bar(
    label: String,
    used: f64,
    resets: Option<String>,
    tone: &'static str,
    #[props(default)] folding: bool,
    #[props(default)] open: bool,
    #[props(default)] small: bool,
    #[props(default)] on_fold: EventHandler<()>,
    /// Shown instead of "N% used" (a count, say).
    #[props(default)]
    amount: Option<String>,
) -> Element {
    let width = used.clamp(0.0, 100.0);
    let reset = resets.as_deref().map(reset_words);
    let used_words = amount.unwrap_or_else(|| format!("{}% used", used.round() as i64));
    let full = if used >= 100.0 { "text-danger font-semibold" } else { "text-ink-secondary" };
    rsx! {
        div { class: "flex items-center gap-4 py-3", "data-usage-bar": "{label}",
            div { class: "w-[200px] shrink-0 max-md:w-[130px]",
                if folding {
                    button { r#type: "button", class: "flex items-center gap-1 text-[14px] text-ink", "aria-expanded": "{open}", onclick: move |_| on_fold.call(()),
                        "{label}"
                        svg { "aria-hidden": "true", class: if open { "rotate-180 text-ink-tertiary" } else { "text-ink-tertiary" }, width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                            path { d: "m6 9 6 6 6-6" }
                        }
                    }
                } else {
                    p { class: if small { "text-[13px] text-ink-secondary" } else { "text-[14px] text-ink" }, "{label}" }
                }
                if let Some(reset) = reset {
                    p { class: "mt-0.5 text-[12px] text-ink-tertiary", "{reset}" }
                }
            }
            div { class: "h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-control",
                div { class: "h-full rounded-full {tone}", style: "width: {width}%" }
            }
            span { class: "w-[132px] shrink-0 text-right text-[13px] tabular-nums {full}", "{used_words}" }
        }
    }
}

/// "Resets at 1:20 PM" today, "Resets Tuesday 8:00 PM" later.
pub fn reset_words(at: &str) -> String {
    let when = parse_time(at);
    if when.is_nan() {
        return String::new();
    }
    match day(when) - day(js_now()) {
        0 => format!("Resets at {}", clock(when)),
        1..=6 => format!("Resets {} {}", weekday(when), clock(when)),
        _ => format!("Resets {}", month_day(when)),
    }
}

/// The plan's forecast in words, naming the reset: "…reach tomorrow's reset…".
fn headline(forecast: &Value) -> Option<String> {
    let text = forecast["text"].as_str()?;
    let Some(resets) = forecast["resetsAt"].as_str().map(parse_time).filter(|t| !t.is_nan()) else { return Some(text.to_string()) };
    let which = match day(resets) - day(js_now()) {
        0 => "today's reset".to_string(),
        1 => "tomorrow's reset".to_string(),
        _ => format!("{}'s reset", weekday(resets)),
    };
    if forecast["onTrack"] == true {
        return Some(format!("On track. You should reach {which} with room to spare."));
    }
    let Some(limit) = forecast["limitAt"].as_str().map(parse_time).filter(|t| !t.is_nan()) else { return Some(text.to_string()) };
    let when = if day(limit) == day(js_now()) { format!("around {}", clock(limit)) } else { format!("{} around {}", weekday(limit), clock(limit)) };
    Some(format!("At this pace you'll reach the limit {when}, before {which}."))
}

fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// The breakdown's table.
#[component]
fn Breakdown(found: Value) -> Element {
    let rows = found["rows"].as_array().cloned().unwrap_or_default();
    let total = found["total"].clone();
    let estimated = total["estimated"] == true;
    let cached = total["cachedTokens"].as_u64().unwrap_or(0);
    if rows.is_empty() {
        return rsx! { p { class: "text-[13px] text-ink-secondary", "No turns in this period yet." } };
    }
    rsx! {
        table { class: "w-full text-[13px]", "data-usage-table": true,
            thead {
                tr { class: "text-left text-[12px] text-ink-tertiary",
                    th { class: "pb-2 font-medium", "" }
                    th { class: "pb-2 text-right font-medium", "Turns" }
                    th { class: "pb-2 text-right font-medium", title: "Sent and written, leaving out what the AI re-read from its cache", "New tokens" }
                    th { class: "pb-2 text-right font-medium", "Cost" }
                }
            }
            tbody {
                for row in rows.iter().cloned().chain(std::iter::once({
                    let mut total = total.clone();
                    total["label"] = serde_json::json!("All");
                    total["total"] = serde_json::json!(true);
                    total
                })) {
                    tr { key: "{row[\"label\"]}", class: if row["total"] == true { "border-t-[0.5px] border-edge font-semibold text-ink" } else { "text-ink" },
                        td { class: "max-w-[260px] truncate py-1.5 pr-3", "{row[\"label\"].as_str().unwrap_or_default()}" }
                        td { class: "py-1.5 text-right tabular-nums", "{row[\"turns\"].as_u64().unwrap_or(0)}" }
                        td { class: "py-1.5 text-right tabular-nums", "{tokens_words(row[\"newTokens\"].as_u64().unwrap_or(0), row[\"estimated\"] == true)}" }
                        td { class: "py-1.5 text-right tabular-nums text-ink-secondary", "{money(&row[\"costUsd\"])}" }
                    }
                }
            }
        }
        p { class: "mt-3 text-[12px] text-ink-tertiary",
            if cached > 0 { "New tokens leave out {tokens_words(cached, false)} the AIs re-read from their caches. " }
            "Cost counts only AIs paid by use whose cost is known; plans and local AIs show “—”."
            if estimated { " Figures with ~ are Catbot's estimate: that AI doesn't report its usage." }
        }
    }
}

/// Catbot's limits: the spend this month, then each AI's session and week. Edited in place.
#[component]
fn Limits(found: Value) -> Element {
    let mut editing = use_signal(|| false);
    let spend = found["spend"].clone();
    let engines = found["engines"].as_array().cloned().unwrap_or_default();
    let used = spend["usedUsd"].as_f64().unwrap_or(0.0);
    let cap = spend["capUsd"].as_f64();
    rsx! {
        section { class: "mb-2 flex flex-col", "data-catbot-limits": true,
            div { class: "flex items-center",
                p { class: "min-w-0 flex-1 text-[12px] font-semibold uppercase tracking-wide text-ink-tertiary", "Catbot's limits" }
                button {
                    r#type: "button",
                    "data-edit-limits": true,
                    class: "rounded-lg px-2 py-1 text-[13px] text-accent hover:bg-raised",
                    onclick: move |_| editing.toggle(),
                    if editing() { "Close" } else { "Change limits" }
                }
            }
            if editing() {
                LimitsEditor { found: found.clone(), on_done: move |_| editing.set(false) }
            }
            match cap {
                Some(cap) => rsx! {
                    Bar {
                        label: "Spend this month".to_string(),
                        used: if cap > 0.0 { used / cap * 100.0 } else { 100.0 },
                        resets: spend["resetsAt"].as_str().map(str::to_string),
                        tone: "bg-success",
                        amount: format!("{} of {}", money(&serde_json::json!(used)), money(&serde_json::json!(cap))),
                    }
                },
                None => rsx! {
                    p { class: "py-2 text-[13px] text-ink-secondary", "data-spend-line": true,
                        "{money(&serde_json::json!(used))} spent this month on AIs paid by use. No spend limit is set."
                    }
                },
            }
            for engine in engines {
                {
                    let name = engine["name"].as_str().unwrap_or_default().to_string();
                    let windows = [("Current session", engine["session"].clone(), "bg-[#f4b11a]"), ("This week", engine["week"].clone(), "bg-[#2f7ae5]")];
                    rsx! {
                        p { key: "{name}", class: "mt-3 text-[13px] font-semibold text-ink", "{name}" }
                        for (label, window, tone) in windows {
                            if let Some(limit) = window["limit"].as_u64() {
                                Bar {
                                    key: "{name}-{label}",
                                    label: label.to_string(),
                                    used: window["usedPercent"].as_f64().unwrap_or(0.0),
                                    resets: window["resetsAt"].as_str().map(str::to_string),
                                    tone,
                                    amount: format!("{} of {}", tokens_words(window["used"].as_u64().unwrap_or(0), false), tokens_words(limit, false)),
                                }
                            } else {
                                p { key: "{name}-{label}", class: "py-1 text-[13px] text-ink-secondary", "{label}: {tokens_words(window[\"used\"].as_u64().unwrap_or(0), false)} tokens, no limit" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Changing the limits: the month's spend overall and per bot, and each AI's token limits and price.
#[component]
fn LimitsEditor(found: Value, on_done: EventHandler<()>) -> Element {
    let limits = found["limits"].clone();
    let number = |v: &Value| v.as_f64().map(|n| if n.fract() == 0.0 { format!("{n:.0}") } else { n.to_string() }).unwrap_or_default();
    let mut monthly = use_signal(|| number(&limits["monthlyUsd"]));
    let bots: Vec<(String, String)> = found["spend"]["perBot"].as_array().into_iter().flatten()
        .map(|b| (b["botId"].as_str().unwrap_or_default().to_string(), b["name"].as_str().unwrap_or_default().to_string())).collect();
    let mut per_bot = use_signal(|| bots.iter().map(|(id, _)| (id.clone(), number(&limits["perBotUsd"][id]))).collect::<Vec<_>>());
    let engines: Vec<Value> = found["engines"].as_array().cloned().unwrap_or_default();
    let mut rows = use_signal(|| {
        engines.iter().map(|e| {
            let id = e["engine"].as_str().unwrap_or_default().to_string();
            let tokens = |w: &Value| w["limit"].as_u64().map(|n| n.to_string()).unwrap_or_default();
            [id, tokens(&e["session"]), tokens(&e["week"]), number(&e["priceIn"]), number(&e["priceOut"])]
        }).collect::<Vec<_>>()
    });
    let mut problem = use_signal(|| None::<String>);
    let save = move |_| {
        let parse = |s: &str| s.trim().parse::<f64>().ok();
        let mut body = serde_json::json!({ "monthlyUsd": parse(&monthly.peek()), "perBotUsd": {}, "engines": {} });
        for (id, cap) in per_bot.peek().iter() {
            if let Some(cap) = parse(cap) {
                body["perBotUsd"][id] = serde_json::json!(cap);
            }
        }
        for [id, session, week, price_in, price_out] in rows.peek().iter() {
            body["engines"][id] = serde_json::json!({
                "session": session.trim().replace(',', "").parse::<u64>().ok(),
                "week": week.trim().replace(',', "").parse::<u64>().ok(),
                "priceIn": parse(price_in),
                "priceOut": parse(price_out),
            });
        }
        spawn(async move {
            match api::call("PUT", "/api/limits", Some(&body)).await {
                Ok(_) => on_done.call(()),
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    let field = "h-8 w-full rounded-lg bg-raised px-2 text-[13px] text-ink outline-none";
    rsx! {
        div { class: "my-3 flex flex-col gap-3 rounded-xl border border-hairline/40 p-3 text-[13px]", "data-limits-editor": true,
            label { class: "flex items-center gap-3",
                span { class: "min-w-0 flex-1 text-ink", "Spend limit for the month, all bots ($)" }
                input { class: "{field} max-w-[120px]", "data-limit-monthly": true, placeholder: "none", value: "{monthly}", oninput: move |e| monthly.set(e.value()) }
            }
            for (i, (_, name)) in bots.iter().enumerate() {
                label { key: "{name}", class: "flex items-center gap-3",
                    span { class: "min-w-0 flex-1 text-ink-secondary", "{name}'s spend limit ($)" }
                    input { class: "{field} max-w-[120px]", placeholder: "none", value: "{per_bot.read()[i].1}", oninput: move |e| per_bot.write()[i].1 = e.value() }
                }
            }
            if !engines.is_empty() {
                p { class: "text-ink-tertiary", "Token limits count new tokens (leaving out cache re-reads). Leave a box empty for no limit. A price per million tokens lets the spend limit count an AI that doesn't report its cost; cache re-reads count at a tenth of the input price." }
            }
            for (i, engine) in engines.iter().enumerate() {
                div { key: "{engine[\"engine\"].as_str().unwrap_or_default()}", class: "grid grid-cols-2 gap-2 border-t-[0.5px] border-edge pt-2 md:grid-cols-4", "data-limit-engine": "{engine[\"engine\"].as_str().unwrap_or_default()}",
                    p { class: "col-span-full font-semibold text-ink", "{engine[\"name\"].as_str().unwrap_or_default()}" }
                    label { class: "flex flex-col gap-1", span { class: "text-ink-tertiary", "Session (5 h) tokens" }
                        input { class: field, "data-limit-session": true, placeholder: "no limit", value: "{rows.read()[i][1]}", oninput: move |e| rows.write()[i][1] = e.value() } }
                    label { class: "flex flex-col gap-1", span { class: "text-ink-tertiary", "Week tokens" }
                        input { class: field, placeholder: "no limit", value: "{rows.read()[i][2]}", oninput: move |e| rows.write()[i][2] = e.value() } }
                    if engine["billing"] == "key" {
                        label { class: "flex flex-col gap-1", span { class: "text-ink-tertiary", "$ per 1M in" }
                            input { class: field, placeholder: "unknown", value: "{rows.read()[i][3]}", oninput: move |e| rows.write()[i][3] = e.value() } }
                        label { class: "flex flex-col gap-1", span { class: "text-ink-tertiary", "$ per 1M out" }
                            input { class: field, placeholder: "unknown", value: "{rows.read()[i][4]}", oninput: move |e| rows.write()[i][4] = e.value() } }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-danger", "{err}" }
            }
            div { class: "flex justify-end gap-2",
                button { r#type: "button", class: "rounded-lg px-3 py-1.5 text-ink-secondary hover:bg-raised", onclick: move |_| on_done.call(()), "Cancel" }
                button { r#type: "button", "data-save-limits": true, class: "rounded-lg bg-accent px-3 py-1.5 font-semibold text-white", onclick: save, "Save limits" }
            }
        }
    }
}

/// "1.2M", "34.5K", "512", with "~" for an estimate.
pub fn tokens_words(n: u64, estimated: bool) -> String {
    let words = match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        n if n >= 10_000 => format!("{:.0}K", n as f64 / 1_000.0),
        n if n >= 1_000 => format!("{:.1}K", n as f64 / 1_000.0),
        n => n.to_string(),
    };
    if estimated { format!("~{words}") } else { words }
}

fn money(cost: &Value) -> String {
    match cost.as_f64() {
        Some(c) if c < 0.01 && c > 0.0 => "<$0.01".into(),
        Some(c) => format!("${c:.2}"),
        None => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_well() {
        assert_eq!(tokens_words(1_250_000, false), "1.2M");
        assert_eq!(tokens_words(34_567, true), "~35K");
        assert_eq!(tokens_words(1_500, false), "1.5K");
        assert_eq!(money(&serde_json::json!(0.004)), "<$0.01");
        assert_eq!(money(&serde_json::json!(3.2)), "$3.20");
        assert_eq!(money(&Value::Null), "—");
    }
}
