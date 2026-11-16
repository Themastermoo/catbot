//! A bot's settings, in the panel on the right, and making a new bot.
//!
//! The panel follows OpenMausBot's `RemoteAgentSettingsPanel.tsx` (Apache-2.0, see NOTICE),
//! the settings their app shows on a phone or another computer: cards on the panel colour,
//! each change saved as soon as it's made. Catbot's cards hold what Catbot bots have: picture
//! and colour, name, instructions, model, approval level, Codex access, and deleting.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::Bot;
use crate::live::State;
use crate::mascots::{BotMark, COLORS, MARKS};

/// Which bot is open, and whether its settings panel is: shared by the sidebar, chat and panel.
#[derive(Clone, Copy)]
pub struct Ui {
    pub selected: Signal<Option<String>>,
    pub settings_open: Signal<bool>,
    /// On a phone (narrow screens), whether the open chat fills the screen rather than the list.
    pub showing_chat: Signal<bool>,
}

/// Their approval levels, in their words.
const LEVELS: [(&str, &str, &str); 4] = [
    ("ask", "Ask for approval", "Requests approval for commands and file changes"),
    ("edits", "Auto-accept edits", "Approves file edits automatically; other actions can still require approval"),
    ("auto", "Approve for me", "Uses the provider's automatic review to approve routine actions and ask about others."),
    ("full", "Full access", "Full computer access (elevated risk)"),
];

/// The models offered by name; Claude Code takes these aliases. A bot set to another model keeps it.
const MODELS: [(&str, &str); 4] = [("", "Default"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];

const CARD: &str = "rounded-xl bg-card p-4";
const LABEL: &str = "block text-[12px] font-medium text-ink-secondary";
const FIELD: &str = "mt-1 w-full rounded-lg bg-inset px-3 py-2 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const ICON_BUTTON: &str = "flex size-10 items-center justify-center rounded-md text-ink-secondary hover:bg-control hover:text-ink";

/// Saves a change and takes the bot the server sends back.
async fn save(mut state: Signal<State>, bot_id: &str, change: Value) -> Result<(), String> {
    let answer = api::call("PATCH", &format!("/api/bots/{bot_id}"), Some(&change)).await?;
    state.write().apply(&json!({ "kind": "bot", "bot": answer["bot"] }));
    Ok(())
}

#[component]
pub fn SettingsPanel(bot: Bot) -> Element {
    let state = use_context::<Signal<State>>();
    let mut ui = use_context::<Ui>();
    let mut saving = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut confirm_delete = use_signal(|| false);

    let id = bot.id.clone();
    let change = use_callback(move |change: Value| {
        let id = id.clone();
        spawn(async move {
            saving.set(true);
            problem.set(None);
            if let Err(err) = save(state, &id, change).await {
                problem.set(Some(err));
            }
            saving.set(false);
        });
    });

    let delete_id = bot.id.clone();
    let delete = move |_| {
        let id = delete_id.clone();
        let mut state = state;
        saving.set(true);
        // A task of the app's, not of this panel: the panel closes when its bot goes, often
        // before the answer arrives (the event stream says so first).
        dioxus::core::spawn_forever(async move {
            match api::call("DELETE", &format!("/api/bots/{id}"), None).await {
                // The app then opens another bot (see `App`).
                Ok(_) => {
                    ui.settings_open.set(false);
                    state.write().apply(&json!({ "kind": "bot.deleted", "botId": id }));
                }
                Err(err) => {
                    let _ = problem.try_write().map(|mut p| *p = Some(err));
                    let _ = saving.try_write().map(|mut s| *s = false);
                }
            }
        });
    };

    let close = move |_| ui.settings_open.set(false);
    let known_model = MODELS.iter().any(|(m, _)| *m == bot.model);
    let full = bot.approval_mode == "full";
    rsx! {
        aside {
            "aria-label": "Bot settings",
            class: "animate-panel-in absolute inset-0 z-40 flex h-full min-w-0 flex-col border-l border-hairline/40 bg-panel md:relative md:z-20 md:w-[400px] md:shrink-0",
            div { class: "flex items-center justify-between px-4 py-3",
                button { r#type: "button", "aria-label": "Collapse bot settings", class: ICON_BUTTON, onclick: close,
                    Glyph { d: "m15 18-6-6 6-6", size: 18 }
                }
                span { class: "text-[15px] font-semibold text-ink", "Bot settings" }
                button { r#type: "button", "aria-label": "Close bot settings", class: ICON_BUTTON, onclick: close,
                    Glyph { d: "M18 6 6 18M6 6l12 12", size: 18 }
                }
            }
            div { class: "flex-1 overflow-y-auto px-5 pb-5",
                div { class: "flex flex-col gap-4 pt-4",
                    // Picture, colour and name.
                    div { class: CARD,
                        div { class: "mb-4 flex items-center gap-3",
                            BotMark { color: bot.color.clone(), initial: bot.initial(), size: 64, mark: bot.mark.clone(), state: bot.activity.mood() }
                            div { class: "min-w-0",
                                div { class: "text-[15px] font-semibold text-ink truncate", "{bot.name}" }
                                div { class: "text-[12px] text-ink-secondary", "Pick a picture and a colour" }
                            }
                        }
                        div { role: "radiogroup", "aria-label": "Picture", class: "grid grid-cols-4 gap-2",
                            for (mark, label) in MARKS {
                                button {
                                    key: "{mark}",
                                    r#type: "button",
                                    role: "radio",
                                    "aria-checked": "{bot.mark == mark}",
                                    "aria-label": "{label}",
                                    title: "{label}",
                                    disabled: saving(),
                                    class: if bot.mark == mark { "flex items-center justify-center rounded-xl bg-raised p-2 ring-2 ring-accent" } else { "flex items-center justify-center rounded-xl p-2 hover:bg-raised" },
                                    "data-cat-follow": true,
                                    onclick: move |_| change(json!({ "mark": mark })),
                                    BotMark { color: bot.color.clone(), initial: bot.initial(), size: 44, mark: mark.to_string() }
                                }
                            }
                        }
                        div { role: "radiogroup", "aria-label": "Colour", class: "mt-3 flex flex-wrap gap-2",
                            for color in COLORS {
                                button {
                                    key: "{color}",
                                    r#type: "button",
                                    role: "radio",
                                    "aria-checked": "{bot.color == color}",
                                    "aria-label": "{color}",
                                    title: "{color}",
                                    disabled: saving(),
                                    class: if bot.color == color { "size-7 rounded-full ring-2 ring-accent ring-offset-2 ring-offset-card" } else { "size-7 rounded-full hover:scale-110 transition-transform" },
                                    background: "var(--identity-{color})",
                                    onclick: move |_| change(json!({ "color": color })),
                                }
                            }
                        }
                        label { class: "mt-4 {LABEL}", "Name"
                            input {
                                key: "{bot.id}-name",
                                class: FIELD,
                                maxlength: 60,
                                initial_value: "{bot.name}",
                                onchange: {
                                    let current = bot.name.clone();
                                    move |e: FormEvent| {
                                        let name = e.value().trim().to_string();
                                        if !name.is_empty() && name != current {
                                            change(json!({ "name": name }));
                                        }
                                    }
                                },
                            }
                        }
                    }
                    // Instructions.
                    div { class: CARD,
                        label { class: LABEL, "Instructions"
                            textarea {
                                key: "{bot.id}-soul",
                                rows: 6,
                                class: "{FIELD} resize-y text-[13px] leading-relaxed",
                                placeholder: "What this bot is for, and how it should work",
                                initial_value: "{bot.soul}",
                                onchange: {
                                    let current = bot.soul.clone();
                                    move |e: FormEvent| {
                                        if e.value().trim() != current.trim() {
                                            change(json!({ "soul": e.value() }));
                                        }
                                    }
                                },
                            }
                        }
                        p { class: "mt-1.5 text-[12px] text-ink-tertiary", "Saved when you click away. Used from its next message." }
                    }
                    // Model and approval level.
                    div { class: CARD,
                        label { class: LABEL, "Model"
                            select {
                                class: FIELD,
                                disabled: saving(),
                                value: "{bot.model}",
                                onchange: move |e| change(json!({ "modelSelection": { "instanceId": "claude", "model": e.value() } })),
                                for (model, label) in MODELS {
                                    option { key: "{model}", value: "{model}", selected: bot.model == model, "{label}" }
                                }
                                if !known_model {
                                    option { value: "{bot.model}", selected: true, "{bot.model}" }
                                }
                            }
                        }
                        div { class: "mt-4 {LABEL}", "Approval level" }
                        div { role: "radiogroup", "aria-label": "Approval level", class: "mt-1 flex flex-col gap-1",
                            for (level, label, desc) in LEVELS {
                                button {
                                    key: "{level}",
                                    r#type: "button",
                                    role: "radio",
                                    "aria-checked": "{bot.approval_mode == level}",
                                    disabled: saving(),
                                    class: if bot.approval_mode == level { "flex w-full items-start gap-2.5 rounded-lg bg-raised px-3 py-2 text-start" } else { "flex w-full items-start gap-2.5 rounded-lg px-3 py-2 text-start hover:bg-raised/50" },
                                    onclick: move |_| change(json!({ "approvalMode": level })),
                                    span { class: if bot.approval_mode == level { "mt-1 size-3.5 shrink-0 rounded-full border-4 border-accent" } else { "mt-1 size-3.5 shrink-0 rounded-full border border-hairline" } }
                                    span { class: "min-w-0",
                                        span { class: "block text-[14px] text-ink", "{label}" }
                                        span { class: if level == "full" { "block text-[12px] leading-snug text-warning" } else { "block text-[12px] leading-snug text-ink-secondary" }, "{desc}" }
                                    }
                                }
                            }
                        }
                        if full {
                            p { class: "mt-2 text-[12px] text-warning", "{bot.name} acts without asking, anywhere it can reach." }
                        }
                    }
                    // Codex.
                    div { class: "flex items-center justify-between gap-4 {CARD}",
                        div { class: "min-w-0",
                            div { class: "text-[15px] font-medium text-ink", "Codex vault" }
                            div { class: "mt-0.5 text-[12px] leading-relaxed text-ink-secondary",
                                if bot.codex { "{bot.name} can read and write your Codex notes." } else { "{bot.name} works only in its own folder." }
                            }
                        }
                        Switch { checked: bot.codex, disabled: saving(), label: "Codex vault access", on_toggle: move |_| change(json!({ "codex": !bot.codex })) }
                    }
                    // Deleting.
                    div { class: CARD,
                        if confirm_delete() {
                            p { class: "text-[13px] leading-relaxed text-ink",
                                "Delete {bot.name}? Its conversation is deleted for good. Its folder of files stays where it is."
                            }
                            div { class: "mt-3 flex justify-end gap-2",
                                button { r#type: "button", class: "rounded-lg px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| confirm_delete.set(false), "Keep it" }
                                button { r#type: "button", disabled: saving(), class: "rounded-lg bg-danger px-3 py-2 text-[13px] font-medium text-white hover:brightness-110 disabled:opacity-50", onclick: delete, "Delete {bot.name}" }
                            }
                        } else {
                            button { r#type: "button", class: "flex items-center gap-2 text-[13px] text-danger hover:underline", onclick: move |_| confirm_delete.set(true),
                                Glyph { d: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6", size: 14 }
                                "Delete this bot…"
                            }
                        }
                    }
                    if let Some(err) = problem() {
                        div { role: "alert", class: "text-[12px] text-danger", "{err}" }
                    }
                }
            }
        }
    }
}

/// Their settings switch (`SettingsPrimitives.tsx`).
#[component]
fn Switch(checked: bool, disabled: bool, label: String, on_toggle: EventHandler<()>) -> Element {
    rsx! {
        button {
            r#type: "button",
            role: "switch",
            "aria-checked": "{checked}",
            "aria-label": "{label}",
            disabled,
            class: if checked { "relative h-6 w-11 shrink-0 cursor-pointer rounded-full bg-accent transition-colors enabled:hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40" } else { "relative h-6 w-11 shrink-0 cursor-pointer rounded-full bg-control transition-colors enabled:hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40" },
            onclick: move |_| on_toggle.call(()),
            span { class: if checked { "absolute top-[3px] left-[21px] h-[18px] w-[18px] rounded-full bg-white transition-[left]" } else { "absolute top-[3px] left-[3px] h-[18px] w-[18px] rounded-full bg-white transition-[left]" } }
        }
    }
}

/// A new bot: name, instructions and picture. It starts out asking before it acts, with the
/// Codex open to it; both can be changed in its settings.
#[component]
pub fn NewBotDialog(on_close: EventHandler<()>) -> Element {
    let mut state = use_context::<Signal<State>>();
    let mut ui = use_context::<Ui>();
    let mut name = use_signal(String::new);
    let mut soul = use_signal(String::new);
    let mut mark = use_signal(|| "cat-head".to_string());
    let mut color = use_signal(|| "green".to_string());
    let mut making = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);

    let create = move |_| {
        if making() {
            return;
        }
        making.set(true);
        problem.set(None);
        spawn(async move {
            let wanted = name.peek().trim().to_string();
            let body = json!({
                "name": if wanted.is_empty() { "New bot".to_string() } else { wanted },
                "soul": soul.peek().trim(),
                "mark": *mark.peek(),
                "color": *color.peek(),
                "approvalMode": "ask",
            });
            match api::post("/api/bots", &body).await {
                Ok(answer) => {
                    state.write().apply(&json!({ "kind": "bot", "bot": answer["bot"] }));
                    ui.selected.set(answer["bot"]["id"].as_str().map(str::to_string));
                    on_close.call(());
                }
                Err(err) => problem.set(Some(format!("Couldn't make a new bot: {err}"))),
            }
            making.set(false);
        });
    };

    let shown_name = if name.read().trim().is_empty() { "New bot".to_string() } else { name.read().trim().to_string() };
    let initial = shown_name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".to_string());
    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4", onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                "aria-label": "New bot",
                class: "w-full max-w-[440px] rounded-2xl bg-panel p-5 shadow-2xl",
                onclick: move |e| e.stop_propagation(),
                div { class: "mb-4 flex items-center justify-between",
                    span { class: "text-[17px] font-semibold text-ink", "New Bot" }
                    button { r#type: "button", "aria-label": "Close", class: ICON_BUTTON, onclick: move |_| on_close.call(()),
                        Glyph { d: "M18 6 6 18M6 6l12 12", size: 18 }
                    }
                }
                div { class: "mb-4 flex items-center gap-3",
                    BotMark { color: color(), initial, size: 64, mark: mark() }
                    div { class: "flex flex-1 flex-col gap-2",
                        div { class: "flex gap-1",
                            for (m, label) in MARKS {
                                button {
                                    key: "{m}",
                                    r#type: "button",
                                    title: "{label}",
                                    "aria-label": "{label}",
                                    "aria-pressed": "{mark() == m}",
                                    class: if mark() == m { "rounded-lg bg-raised p-1 ring-2 ring-accent" } else { "rounded-lg p-1 hover:bg-raised" },
                                    onclick: move |_| mark.set(m.to_string()),
                                    BotMark { color: color(), initial: "?", size: 28, mark: m.to_string() }
                                }
                            }
                        }
                        div { class: "flex flex-wrap gap-1.5",
                            for c in COLORS {
                                button {
                                    key: "{c}",
                                    r#type: "button",
                                    title: "{c}",
                                    "aria-label": "{c}",
                                    "aria-pressed": "{color() == c}",
                                    class: if color() == c { "size-5 rounded-full ring-2 ring-accent ring-offset-1 ring-offset-panel" } else { "size-5 rounded-full" },
                                    background: "var(--identity-{c})",
                                    onclick: move |_| color.set(c.to_string()),
                                }
                            }
                        }
                    }
                }
                label { class: LABEL, "Name"
                    input { class: FIELD, maxlength: 60, placeholder: "New bot", autofocus: true, oninput: move |e| name.set(e.value()) }
                }
                label { class: "mt-3 {LABEL}", "Instructions"
                    textarea { class: "{FIELD} resize-y text-[13px] leading-relaxed", rows: 4, placeholder: "What this bot is for, and how it should work", oninput: move |e| soul.set(e.value()) }
                }
                p { class: "mt-3 text-[12px] leading-relaxed text-ink-secondary",
                    "{shown_name} will ask before acting, and can use your Codex notes. Both can be changed in its settings."
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
                }
                div { class: "mt-4 flex justify-end gap-2",
                    button { r#type: "button", class: "rounded-lg px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "button", disabled: making(), class: "ui-button-primary rounded-lg bg-accent px-3 py-2 text-[13px] font-medium text-accent-ink hover:brightness-110 disabled:opacity-50", onclick: create, "Create bot" }
                }
            }
        }
    }
}

/// A Lucide shape (ISC, see NOTICE).
#[component]
fn Glyph(d: &'static str, size: u32) -> Element {
    rsx! {
        svg { "aria-hidden": "true", width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d }
        }
    }
}
