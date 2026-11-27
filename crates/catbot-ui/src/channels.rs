//! Channels (stage 15.1): making one, and its settings. A channel is shown as a bot whose
//! `channel` holds its members and lead (`bots.rs`); messages with no @name go to the lead.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::Bot;
use crate::live::State;
use crate::mascots::BotMark;
use crate::settings::Ui;

const FIELD: &str = "mt-1 w-full rounded-lg bg-inset px-3 py-2 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const LABEL: &str = "block text-[12px] font-medium text-ink-secondary";

/// The bots (not channels) to pick members from.
fn bots(state: &State) -> Vec<Bot> {
    state.bots.iter().filter(|b| b.channel.is_none()).cloned().collect()
}

/// Members to choose and a lead among them.
#[component]
fn Members(chosen: Signal<Vec<String>>, lead: Signal<String>, disabled: bool) -> Element {
    let state = use_context::<Signal<State>>();
    let all = bots(&state.read());
    rsx! {
        div { class: "mt-1 flex flex-col gap-1", "data-channel-members": true,
            for bot in all {
                {
                    let id = bot.id.clone();
                    let on = chosen.read().contains(&id);
                    let is_lead = *lead.read() == id;
                    let (toggle_id, lead_id) = (id.clone(), id.clone());
                    rsx! {
                        div { key: "{id}", class: "flex items-center gap-2 rounded-lg px-2 py-1 hover:bg-raised",
                            label { class: "flex min-w-0 flex-1 items-center gap-2",
                                input {
                                    r#type: "checkbox",
                                    checked: on,
                                    disabled,
                                    "data-member": "{bot.name}",
                                    onchange: move |_| {
                                        let mut list = chosen.write();
                                        if let Some(at) = list.iter().position(|m| *m == toggle_id) {
                                            list.remove(at);
                                        } else {
                                            list.push(toggle_id.clone());
                                        }
                                        if !list.contains(&lead.peek()) {
                                            let first = list.first().cloned().unwrap_or_default();
                                            drop(list);
                                            lead.set(first);
                                        }
                                    },
                                }
                                BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 22, mark: bot.mark.clone() }
                                span { class: "truncate text-[14px] text-ink", "{bot.name}" }
                            }
                            if on {
                                button {
                                    r#type: "button",
                                    disabled,
                                    "data-make-lead": "{bot.name}",
                                    class: if is_lead { "rounded-full bg-accent px-2 py-0.5 text-[11px] font-semibold text-white" } else { "rounded-full px-2 py-0.5 text-[11px] text-ink-secondary hover:bg-control" },
                                    title: "The lead answers whatever isn't addressed to someone by @name",
                                    onclick: move |_| lead.set(lead_id.clone()),
                                    if is_lead { "Lead" } else { "Make lead" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn NewChannelDialog(on_close: EventHandler<()>) -> Element {
    let mut state = use_context::<Signal<State>>();
    let mut ui = use_context::<Ui>();
    let mut name = use_signal(String::new);
    let chosen = use_signal(Vec::<String>::new);
    let lead = use_signal(String::new);
    let mut making = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let create = move |_| {
        if making() {
            return;
        }
        let members = chosen.peek().clone();
        if members.is_empty() {
            problem.set(Some("Pick at least one bot.".into()));
            return;
        }
        making.set(true);
        problem.set(None);
        spawn(async move {
            let wanted = name.peek().trim().to_string();
            let body = json!({ "name": if wanted.is_empty() { "New channel".to_string() } else { wanted }, "members": members, "lead": *lead.peek() });
            match api::post("/api/channels", &body).await {
                Ok(answer) => {
                    state.write().apply(&json!({ "kind": "bot", "bot": answer["bot"] }));
                    ui.selected.set(answer["bot"]["id"].as_str().map(str::to_string));
                    on_close.call(());
                }
                Err(err) => problem.set(Some(format!("Couldn't make the channel: {err}"))),
            }
            making.set(false);
        });
    };
    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4", onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                "aria-label": "New channel",
                "data-new-channel": true,
                class: "flex max-h-[88vh] w-full max-w-[440px] flex-col rounded-2xl bg-panel p-5 shadow-2xl",
                onclick: move |e| e.stop_propagation(),
                div { class: "mb-1 text-[17px] font-semibold text-ink", "New channel" }
                p { class: "mb-4 text-[13px] text-ink-secondary", "A group chat with several bots. Name one with @Name to have it answer; otherwise the lead does." }
                label { class: LABEL, "Name"
                    input { class: FIELD, "data-channel-name": true, placeholder: "Garden", value: "{name}", oninput: move |e| name.set(e.value()) }
                }
                div { class: "mt-4 {LABEL}", "Bots" }
                div { class: "min-h-0 overflow-y-auto",
                    Members { chosen, lead, disabled: making() }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-3 text-[13px] text-danger", "{err}" }
                }
                div { class: "mt-5 flex justify-end gap-2",
                    button { r#type: "button", class: "rounded-lg px-3 py-2 text-[14px] text-ink-secondary hover:bg-raised", onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "button", "data-create-channel": true, disabled: making(), class: "rounded-lg bg-accent px-4 py-2 text-[14px] font-semibold text-white disabled:opacity-50", onclick: create, "Make channel" }
                }
            }
        }
    }
}

/// A channel's settings: name, instructions, members and lead, deleting it.
#[component]
pub fn ChannelSettings(bot: Bot, saving: bool, change: Callback<Value>) -> Element {
    // (The Team Chat's own settings are below.)
    let info = bot.channel.clone().unwrap_or_default();
    if info.team_chat {
        return rsx! { TeamChatSettings { info, saving, change } };
    }
    let chosen = use_signal(|| info.members.iter().map(|m| m.0.clone()).collect::<Vec<_>>());
    let lead = use_signal(|| info.lead.clone());
    let folder = info.folder.clone();
    rsx! {
        div { class: "rounded-xl bg-card p-4", "data-channel-settings": true,
            div { class: LABEL, "Members" }
            Members { chosen, lead, disabled: saving }
            p { class: "mt-1.5 text-[12px] text-ink-tertiary", "Working folder: {folder}" }
            div { class: "mt-3 flex justify-end",
                button {
                    r#type: "button",
                    "data-save-members": true,
                    disabled: saving || chosen.read().is_empty(),
                    class: "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-40",
                    onclick: move |_| change.call(json!({ "members": *chosen.peek(), "lead": *lead.peek() })),
                    "Save members"
                }
            }
        }
    }
}

/// The Team Chat's: every bot is in it, so there are no members to pick; the Chief of Staff
/// answers what isn't @named, or with no Chief the bot chosen here (stage 30.1).
#[component]
fn TeamChatSettings(info: crate::bots::ChannelInfo, saving: bool, change: Callback<Value>) -> Element {
    let state = use_context::<Signal<State>>();
    let chief = state.read().bots.iter().find(|b| b.chief && b.channel.is_none()).map(|b| b.name.clone());
    let names = info.members.iter().map(|m| m.1.clone()).collect::<Vec<_>>().join(", ");
    rsx! {
        div { class: "rounded-xl bg-card p-4", "data-channel-settings": true, "data-team-chat-settings": true,
            div { class: LABEL, "Who's in it" }
            p { class: "mt-1 text-[13px] leading-relaxed text-ink", "Every bot: {names}." }
            p { class: "mt-0.5 text-[12px] text-ink-tertiary", "New bots join by themselves; a bot leaves when it's deleted or put away." }
            label { class: "mt-4 {LABEL}", "Answers when no one is @named"
                if let Some(chief) = &chief {
                    p { class: "mt-1 text-[13px] text-ink", "{chief}, your Chief of Staff." }
                } else {
                    select {
                        "data-team-chat-lead": true,
                        disabled: saving,
                        class: "mt-1 w-full rounded-lg border border-hairline/60 bg-app px-2.5 py-1.5 text-[13px] text-ink",
                        onchange: move |e: FormEvent| change.call(json!({ "lead": e.value() })),
                        for (id, name, _, _) in info.members.iter().cloned() {
                            option { key: "{id}", value: "{id}", selected: id == info.lead, "{name}" }
                        }
                    }
                }
            }
            if chief.is_some() {
                p { class: "mt-1 text-[12px] text-ink-tertiary", "With no Chief of Staff, a bot you choose here answers." }
            }
            p { class: "mt-3 text-[12px] text-ink-tertiary", "Working folder: {info.folder}" }
        }
    }
}
