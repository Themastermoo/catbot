//! The model pill at the top right of a chat, and its menu (stage 19.3), as MausBot's simple
//! layout has them (`ModelPicker.tsx`, `SimpleModelMenu.tsx`, `EngineUsage.tsx`, Apache-2.0, see
//! NOTICE): the plan's usage when the AI has one ("5-hour · 98% left · resets 10:10 PM"), the
//! model the bot uses, the AI's other models, other AIs that are ready, and Manage AI accounts.
//! Choosing changes the bot's model for all its chats.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::Bot;
use crate::live::State;
use crate::sidebar::{Icon, MENU_ROW, MENU_SHEET};

/// The AIs and the plans' usage, fetched when the pill first shows and as they change.
#[derive(Clone, Default, PartialEq)]
struct Known {
    engines: Vec<Value>,
    plans: Vec<Value>,
}

fn use_known() -> Signal<Known> {
    let mut known = use_signal(Known::default);
    let state = use_context::<Signal<State>>();
    use_effect(move || {
        let _ = state.read().engines_changed;
        spawn(async move {
            let engines = api::get("/api/engines").await.ok().and_then(|e| e["engines"].as_array().cloned()).unwrap_or_default();
            known.write().engines = engines;
        });
    });
    use_effect(move || {
        let _ = state.read().limits_changed;
        spawn(async move {
            let plans = api::get("/api/usage/plans").await.ok().and_then(|p| p["plans"].as_array().cloned()).unwrap_or_default();
            known.write().plans = plans;
        });
    });
    known
}

/// A model's name as the menu shows it.
fn model_name(engine: Option<&Value>, model: &str) -> String {
    let listed = engine.and_then(|e| e["models"].as_array()).and_then(|models| models.iter().find(|m| m["id"] == model).and_then(|m| m["label"].as_str()));
    match (listed, model) {
        (Some(label), _) if !label.is_empty() => label.to_string(),
        (_, "") => engine.and_then(|e| e["name"].as_str()).map(str::to_string).unwrap_or_else(|| "Default".into()),
        (_, other) => other.to_string(),
    }
}

/// How much of the plan is left, 0–100: the tighter of its windows.
fn left_of(plan: &Value) -> Option<f64> {
    let windows = plan["windows"].as_array()?;
    let used = windows.iter().filter_map(|w| w["usedPercent"].as_f64()).fold(None, |most: Option<f64>, u| Some(most.map_or(u, |m| m.max(u))))?;
    Some((100.0 - used).clamp(0.0, 100.0))
}

/// An AI's letter mark in a colour of its own (not its logo).
#[component]
pub fn EngineMark(id: String, name: String, #[props(default = 18)] size: u32) -> Element {
    let initial: String = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let hue = id.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    let font = size * 11 / 18;
    rsx! {
        span { "aria-hidden": "true", class: "flex shrink-0 items-center justify-center rounded-full font-semibold", style: "width: {size}px; height: {size}px; font-size: {font}px; background: hsl({hue} 60% 50% / 0.16); color: hsl({hue} 55% 40%)", "{initial}" }
    }
}

/// Four bars for how much of the plan is left (`ThreadModelsLine`'s signal).
#[component]
fn Bars(left: f64) -> Element {
    let lit = ((left / 25.0).ceil() as usize).clamp(0, 4);
    rsx! {
        span { "aria-hidden": "true", "data-plan-signal": "{lit}", class: "flex h-3 items-end gap-[2px]",
            for i in 0..4usize {
                span { key: "{i}", class: if i < lit { "w-[2px] rounded-full bg-ink-secondary" } else { "w-[2px] rounded-full bg-ink/15" }, style: "height: {4 + i * 2}px" }
            }
        }
    }
}

/// The pill, and its menu when open.
#[component]
pub fn ModelPill(bot: Bot) -> Element {
    let known = use_known();
    let mut open = use_signal(|| false);
    let k = known();
    let engine = k.engines.iter().find(|e| e["id"] == bot.engine.as_str()).cloned();
    let name = model_name(engine.as_ref(), &bot.model);
    let plan = k.plans.iter().find(|p| p["engine"] == bot.engine.as_str() && p["ok"] == true).cloned();
    let left = plan.as_ref().and_then(left_of);
    let engine_name = engine.as_ref().and_then(|e| e["name"].as_str()).unwrap_or("AI").to_string();
    rsx! {
        div { class: "relative",
            button {
                r#type: "button",
                "data-model-pill": true,
                "aria-haspopup": "menu",
                "aria-expanded": "{open()}",
                title: "{engine_name}: {name}",
                class: "flex h-9 max-w-[240px] items-center gap-1.5 rounded-full bg-composer pl-2.5 pr-2 text-[13px] text-ink ring-1 ring-inset ring-ink/[0.07] shadow-(--shadow-float) transition-colors duration-[120ms] hover:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)] aria-expanded:bg-[color-mix(in_srgb,var(--color-composer),var(--color-ink)_7%)] max-md:hidden",
                onclick: move |_| open.toggle(),
                EngineMark { id: bot.engine.clone(), name: engine_name.clone(), size: 16 }
                span { class: "min-w-0 truncate", "{name}" }
                if let Some(left) = left {
                    Bars { left }
                }
                span { class: "shrink-0 text-ink-tertiary", svg { "aria-hidden": "true", width: "13", height: "13", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", path { d: "m6 9 6 6 6-6" } } }
            }
            if open() {
                div { class: "fixed inset-0 z-40", onclick: move |_| open.set(false) }
                ModelMenu { bot: bot.clone(), engines: k.engines.clone(), plan, on_done: move |_| open.set(false) }
            }
        }
    }
}

#[component]
fn ModelMenu(bot: Bot, engines: Vec<Value>, plan: Option<Value>, on_done: EventHandler<()>) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<crate::settings::Ui>();
    let mut all = use_signal(|| false);
    let mut others = use_signal(|| false);
    let mut live = use_signal(|| None::<Vec<(String, String)>>);
    let mut problem = use_signal(|| None::<String>);
    let engine = engines.iter().find(|e| e["id"] == bot.engine.as_str()).cloned();
    let engine_id = bot.engine.clone();
    // An AI with a list of its own (Codex, Pi, local ones): asked for once the menu opens.
    use_hook(move || {
        let id = engine_id.clone();
        spawn(async move {
            if let Ok(answer) = api::get(&format!("/api/engines/{id}/models")).await
                && answer["live"] == true
            {
                let labels = answer["labels"].as_array().cloned().unwrap_or_default();
                let list = answer["models"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|m| {
                    let label = labels.iter().find(|l| l["id"] == m).and_then(|l| l["label"].as_str()).unwrap_or(m);
                    (m.to_string(), label.to_string())
                });
                live.set(Some(list.collect()));
            }
        });
    });
    let models: Vec<(String, String)> = live().unwrap_or_else(|| {
        engine.as_ref().and_then(|e| e["models"].as_array()).into_iter().flatten().map(|m| (m["id"].as_str().unwrap_or_default().to_string(), m["label"].as_str().unwrap_or_default().to_string())).collect()
    });
    let current = model_name(engine.as_ref(), &bot.model);
    let engine_name = engine.as_ref().and_then(|e| e["name"].as_str()).unwrap_or("AI").to_string();
    let shown: Vec<(String, String)> = if all() { models.clone() } else { models.iter().take(4).cloned().collect() };
    let hidden = models.len().saturating_sub(4);
    let ready_others: Vec<Value> = engines.iter().filter(|e| e["ready"] == true && e["id"] != bot.engine.as_str() && e["id"] != "").cloned().collect();
    let bot_id = bot.id.clone();
    let pick = move |engine: String, model: String| {
        let id = bot_id.clone();
        spawn(async move {
            match crate::settings::save(state, &id, json!({ "modelSelection": { "instanceId": engine, "model": model } })).await {
                Ok(()) => on_done.call(()),
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    let pick_model = pick.clone();
    rsx! {
        div { role: "menu", "aria-label": "Model", "data-model-menu": true, class: "absolute right-0 top-full z-50 mt-1.5 w-[320px] max-w-[calc(100vw-2rem)] max-h-[70vh] overflow-y-auto {MENU_SHEET}",
            if let Some(plan) = plan.clone() {
                div { class: "px-2 pb-2 pt-1.5", "data-plan-usage": true,
                    div { class: "pb-1.5",
                        span { class: "text-[13px] font-semibold text-ink", "{plan[\"name\"].as_str().unwrap_or(\"Plan\")} usage" }
                    }
                    for window in plan["windows"].as_array().cloned().unwrap_or_default() {
                        {
                            let used = window["usedPercent"].as_f64().unwrap_or(0.0);
                            let left = (100.0 - used).clamp(0.0, 100.0);
                            let label = window["label"].as_str().unwrap_or_default().to_string();
                            let label = if window["kind"] == "session" { "5-hour".to_string() } else if window["kind"] == "week" { "Weekly".to_string() } else { label };
                            let reset = window["resetsAt"].as_str().map(crate::usage::reset_words).unwrap_or_default().replace("Resets", "resets");
                            rsx! {
                                div { key: "{label}", class: "py-1.5", "data-usage-window": "{label}",
                                    div { class: "flex items-baseline justify-between text-[12px]",
                                        span { class: "text-ink-secondary", "{label}" }
                                        span { class: "text-ink", span { class: "font-semibold", "{left.round()}% left" } if !reset.is_empty() { " · {reset}" } }
                                    }
                                    div { class: "mt-1 h-1 overflow-hidden rounded-full bg-ink/10",
                                        div { class: "h-full rounded-full bg-ink/70", style: "width: {used.clamp(0.0, 100.0)}%" }
                                    }
                                }
                            }
                        }
                    }
                }
                div { role: "separator", class: "mx-1.5 h-px shrink-0 bg-hairline/50" }
            }
            div { class: "flex items-center gap-2 rounded-lg px-2 py-1.5 ring-1 ring-accent", "data-current-model": true,
                EngineMark { id: bot.engine.clone(), name: engine_name.clone(), size: 16 }
                span { class: "flex min-w-0 flex-1 flex-col",
                    span { class: "truncate text-[14px] leading-5 text-ink", "{current}" }
                    span { class: "truncate text-[12px] leading-4 text-ink-secondary", "{bot.name}'s model, used in all its chats" }
                }
                span { class: "shrink-0 text-ink", Icon { name: "check", size: 15 } }
            }
            if !models.is_empty() {
                div { role: "separator", class: "mx-1.5 h-px shrink-0 bg-hairline/50" }
                div { class: "flex items-center gap-2 px-2 pb-0.5 pt-1.5 text-[12px] font-medium text-ink-secondary",
                    EngineMark { id: bot.engine.clone(), name: engine_name.clone(), size: 14 }
                    "{engine_name}"
                }
                for (id, label) in shown {
                    {
                        let (e, m) = (bot.engine.clone(), id.clone());
                        let choose = pick_model.clone();
                        let on = id == bot.model;
                        let shown_label = if label.is_empty() { id.clone() } else { label.clone() };
                        rsx! {
                            button { key: "{id}", r#type: "button", role: "menuitemradio", "aria-checked": "{on}", "data-model-choice": "{id}", class: "{MENU_ROW} pl-8",
                                onclick: move |_| choose(e.clone(), m.clone()),
                                span { class: "flex-1 truncate", "{shown_label}" }
                                if on {
                                    span { class: "shrink-0 text-accent", Icon { name: "check", size: 14 } }
                                }
                            }
                        }
                    }
                }
                if hidden > 0 && !all() {
                    button { r#type: "button", class: "{MENU_ROW} pl-8 text-ink-secondary", "data-all-models": true, onclick: move |_| all.set(true),
                        span { class: "flex-1", "Show all {models.len()} models" }
                        svg { "aria-hidden": "true", width: "13", height: "13", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", path { d: "m6 9 6 6 6-6" } }
                    }
                }
            }
            if !ready_others.is_empty() {
                div { role: "separator", class: "mx-1.5 h-px shrink-0 bg-hairline/50" }
                button { r#type: "button", class: MENU_ROW, "aria-expanded": "{others()}", "data-other-ais": true, onclick: move |_| others.toggle(),
                    span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "sparkles", size: 15 } }
                    span { class: "flex-1", "Another AI" }
                    span { class: "shrink-0 text-[13px] text-ink-secondary", "{ready_others.len()} ready" }
                }
                if others() {
                    for e in ready_others {
                        {
                            let id = e["id"].as_str().unwrap_or_default().to_string();
                            let name = e["name"].as_str().unwrap_or_default().to_string();
                            let model = e["defaultModel"].as_str().unwrap_or_default().to_string();
                            let choose = pick.clone();
                            let chosen = id.clone();
                            rsx! {
                                button { key: "{id}", r#type: "button", role: "menuitem", "data-engine-choice": "{id}", class: "{MENU_ROW} pl-3",
                                    onclick: move |_| choose(chosen.clone(), model.clone()),
                                    EngineMark { id: id.clone(), name: name.clone(), size: 16 }
                                    span { class: "flex-1 truncate", "{name}" }
                                }
                            }
                        }
                    }
                }
            }
            div { role: "separator", class: "mx-1.5 h-px shrink-0 bg-hairline/50" }
            button { r#type: "button", class: MENU_ROW, "data-manage-ais": true,
                onclick: move |_| {
                    let mut prefs = ui.prefs;
                    prefs.set(Some("ai".into()));
                    on_done.call(());
                },
                span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: "settings", size: 15 } }
                span { class: "flex-1", "Manage AI accounts" }
                span { class: "shrink-0 text-ink-secondary", Icon { name: "chevron-right", size: 14 } }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "px-2 py-1 text-[12px] text-danger", "{err}" }
            }
        }
    }
}

/// The gauge in the message box: a ring for how much of the AI's plan is used, opening the same
/// usage as the model menu (`EngineUsage.tsx`). Nothing shows for an AI without a plan.
#[component]
pub fn UsageGauge(bot: Bot, working: bool) -> Element {
    let known = use_known();
    let mut open = use_signal(|| false);
    let k = known();
    let plan = k.plans.iter().find(|p| p["engine"] == bot.engine.as_str() && p["ok"] == true).cloned();
    let Some(plan) = plan else {
        return rsx! {
            if working {
                span { "aria-hidden": "true", class: "flex size-7 shrink-0 items-center justify-center rounded-full bg-ink/[0.06] text-ink-tertiary",
                    span { class: "size-3.5 animate-spin rounded-full border-[1.5px] border-ink/20 border-t-ink/60" }
                }
            }
        };
    };
    let left = left_of(&plan).unwrap_or(100.0);
    let used = 100.0 - left;
    // A ring of circumference 2πr with r = 7.
    let around = 2.0 * std::f64::consts::PI * 7.0;
    let dash = around * used / 100.0;
    let tone = if left < 10.0 { "text-danger" } else { "text-ink-secondary" };
    rsx! {
        div { class: "relative",
            button { r#type: "button", "data-usage-gauge": true, "aria-label": "Plan usage: {left.round()}% left", title: "{left.round()}% of the plan left", "aria-expanded": "{open()}",
                class: "flex size-7 shrink-0 items-center justify-center rounded-full bg-ink/[0.06] {tone} hover:bg-ink/[0.09]",
                onclick: move |_| open.toggle(),
                svg { "aria-hidden": "true", width: "18", height: "18", view_box: "0 0 18 18", class: if working { "animate-spin" } else { "" },
                    circle { cx: "9", cy: "9", r: "7", fill: "none", stroke: "currentColor", stroke_opacity: "0.2", stroke_width: "2" }
                    circle { cx: "9", cy: "9", r: "7", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_dasharray: "{dash} {around}", transform: "rotate(-90 9 9)" }
                }
            }
            if open() {
                div { class: "fixed inset-0 z-40", onclick: move |_| open.set(false) }
                div { class: "absolute bottom-full right-0 z-50 mb-2 w-[280px] {MENU_SHEET}", "data-usage-popover": true,
                    div { class: "px-2 py-1.5",
                        p { class: "pb-1 text-[13px] font-semibold text-ink", "{plan[\"name\"].as_str().unwrap_or(\"Plan\")} usage" }
                        for window in plan["windows"].as_array().cloned().unwrap_or_default() {
                            {
                                let used = window["usedPercent"].as_f64().unwrap_or(0.0);
                                let label = window["label"].as_str().unwrap_or_default().to_string();
                                let reset = window["resetsAt"].as_str().map(crate::usage::reset_words).unwrap_or_default();
                                rsx! {
                                    div { key: "{label}", class: "py-1.5",
                                        div { class: "flex items-baseline justify-between text-[12px]",
                                            span { class: "text-ink-secondary", "{label}" }
                                            span { class: "font-semibold text-ink", "{(100.0 - used).clamp(0.0, 100.0).round()}% left" }
                                        }
                                        div { class: "mt-1 h-1 overflow-hidden rounded-full bg-ink/10", div { class: "h-full rounded-full bg-ink/70", style: "width: {used.clamp(0.0, 100.0)}%" } }
                                        if !reset.is_empty() {
                                            p { class: "mt-0.5 text-[11px] text-ink-tertiary", "{reset}" }
                                        }
                                    }
                                }
                            }
                        }
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
    fn models_are_named_and_plans_measured() {
        let engine = json!({ "name": "Claude", "models": [{ "id": "", "label": "Default" }, { "id": "claude-sonnet-5", "label": "Sonnet 5" }] });
        assert_eq!(model_name(Some(&engine), "claude-sonnet-5"), "Sonnet 5");
        assert_eq!(model_name(Some(&engine), ""), "Default");
        assert_eq!(model_name(None, "x-1"), "x-1");
        let plan = json!({ "windows": [{ "usedPercent": 2.0 }, { "usedPercent": 54.0 }] });
        assert_eq!(left_of(&plan), Some(46.0));
        assert_eq!(left_of(&json!({})), None);
    }
}
