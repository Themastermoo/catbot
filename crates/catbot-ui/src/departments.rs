//! Departments (stage 19.6): bots grouped under folding headings in the sidebar, as MausBot's
//! simple sidebar sections (`SidebarSections.tsx`, `SidebarSectionHeader.tsx`, Apache-2.0, see
//! NOTICE), and the dialogs that make and change them, shared with the Team map.
//!
//! Each department has shared instructions its bots are shown every turn, a shared memory note,
//! and may have a lead of its own. A bot in none is in General.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::{Activity, Bot};
use crate::live::State;
use crate::mascots::BotMark;
use crate::sidebar::{Icon, MENU_ROW, MENU_SHEET};

/// What a bot outside every department is in.
pub const GENERAL: &str = "General";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dept {
    pub name: String,
    pub instructions: String,
    pub updated_at: Option<f64>,
    pub chief: Option<String>,
    pub members: Vec<String>,
    pub memory_note: Option<String>,
}

/// The departments, in their order, as the server keeps them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Depts {
    pub list: Vec<Dept>,
    pub instructions_max: usize,
}

impl Depts {
    pub fn from_wire(answer: &Value) -> Depts {
        let list = answer["departments"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|d| {
                        Some(Dept {
                            name: d["name"].as_str()?.to_string(),
                            instructions: d["instructions"].as_str().unwrap_or_default().to_string(),
                            updated_at: d["updatedAt"].as_f64(),
                            chief: d["chief"].as_str().map(str::to_string),
                            members: d["members"].as_array().map(|m| m.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
                            memory_note: d["memoryNote"].as_str().map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Depts { list, instructions_max: answer["instructionsMax"].as_u64().unwrap_or(24_000) as usize }
    }

    pub fn get(&self, name: &str) -> Option<&Dept> {
        self.list.iter().find(|d| d.name == name)
    }
}

/// Reads the departments again.
pub fn refresh(mut depts: Signal<Depts>) {
    spawn(async move {
        if let Ok(answer) = api::get("/api/departments").await {
            depts.set(Depts::from_wire(&answer));
        }
    });
}

fn path(name: &str) -> String {
    format!("/api/departments/{}", crate::markdown::encode(name))
}

/// Sends a change, and shows the departments as the server answers.
async fn send(method: &str, path: String, body: Value, mut depts: Signal<Depts>) -> Result<(), String> {
    let answer = api::call(method, &path, Some(&body)).await?;
    if answer["departments"].is_array() {
        depts.set(Depts::from_wire(&answer));
    }
    Ok(())
}

/// Moves bots into a department (General for none).
pub fn move_bots(depts: Signal<Depts>, to: &str, bots: Vec<String>) {
    let path = format!("{}/members", path(to));
    // Outlives the menu that asked for it, which closes at once.
    dioxus::core::spawn_forever(async move {
        let _ = send("PUT", path, json!({ "add": bots }), depts).await;
    });
}

/// The sidebar's bots in their groups: each department in order (empty ones too), then General.
pub fn groups(bots: &[Bot], depts: &Depts) -> Vec<(String, Vec<Bot>)> {
    let mut out: Vec<(String, Vec<Bot>)> = depts.list.iter().map(|d| (d.name.clone(), Vec::new())).collect();
    let mut general = Vec::new();
    for bot in bots {
        match out.iter_mut().find(|(name, _)| !bot.department.is_empty() && *name == bot.department) {
            Some((_, list)) => list.push(bot.clone()),
            None => general.push(bot.clone()),
        }
    }
    out.push((GENERAL.to_string(), general));
    out
}

/// What a folded department shows: someone waiting on you wins, then working, then unread.
fn attention(bots: &[Bot]) -> Option<(&'static str, &'static str)> {
    if bots.iter().any(|b| b.activity == Activity::WaitingOnYou) {
        Some(("bg-warning", "Waiting for you"))
    } else if bots.iter().any(|b| b.activity == Activity::Working) {
        Some(("bg-success", "Working"))
    } else if bots.iter().any(|b| b.unread) {
        Some(("bg-accent", "Unread"))
    } else {
        None
    }
}

/// Which dialog a department's menu opened.
#[derive(Clone, Debug, PartialEq)]
pub enum Open {
    New(Vec<String>),
    Rename(String),
    Instructions(String),
    Members(String),
    Delete(String),
    Memory(String),
}

/// A department's heading in the sidebar (their simple "line" header): its name, a dot when
/// folded and something needs you, the fold chevron, its ⋯ menu, and a place to drop a bot.
#[component]
pub fn Header(name: String, bots: Vec<Bot>, folded: bool, on_toggle: EventHandler<()>, on_open: EventHandler<Open>, dragging: Signal<Option<String>>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let mut menu = use_signal(|| false);
    let mut over = use_signal(|| false);
    let general = name == GENERAL;
    let dot = if folded { attention(&bots) } else { None };
    let chevron: &'static str = if folded { "chevron-right" } else { "chevron-down" };
    let toggle_title = if folded { format!("Expand {name}") } else { format!("Collapse {name}") };
    let drop_name = name.clone();
    let mut open = move |what: Open| {
        menu.set(false);
        on_open.call(what);
    };
    let (n1, n2, n3, n4, n5) = (name.clone(), name.clone(), name.clone(), name.clone(), name.clone());
    let drop_class = if over() { "bg-accent/10 outline outline-2 -outline-offset-2 outline-accent/70" } else { "" };
    rsx! {
        div {
            "data-section": "{name}",
            "data-section-drop": over(),
            class: "group/section relative flex h-[30px] items-center gap-1.5 rounded-[6px] px-2 pb-1.5 pt-2 hover:bg-raised/50 {drop_class}",
            ondragover: move |e| {
                if dragging.peek().is_some() {
                    e.prevent_default();
                    over.set(true);
                }
            },
            ondragleave: move |_| over.set(false),
            ondrop: move |e| {
                e.prevent_default();
                over.set(false);
                let mut dragged = dragging;
                if let Some(bot) = dragged.take() {
                    move_bots(depts, &drop_name, vec![bot]);
                }
            },
            button { r#type: "button", "aria-expanded": "{!folded}", title: "{toggle_title}",
                class: "flex min-w-0 flex-1 items-center gap-1.5 rounded text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus",
                onclick: move |_| on_toggle.call(()),
                span { class: "min-w-0 flex-1 truncate text-[13px] leading-[18px] text-ink-secondary", "{name}" }
                if let Some((color, label)) = dot {
                    span { role: "img", "aria-label": label, title: label, class: "size-1.5 shrink-0 rounded-full {color}" }
                }
                span { class: "shrink-0 text-ink-secondary opacity-0 transition-opacity group-hover/section:opacity-100 group-focus-within/section:opacity-100",
                    Icon { name: chevron, size: 12 }
                }
            }
            if !general {
                button { r#type: "button", "aria-label": "Actions for {name} department", title: "Actions for {name} department", "aria-haspopup": "menu", "aria-expanded": "{menu()}",
                    "data-section-menu": "{name}",
                    class: "flex size-5 shrink-0 items-center justify-center rounded text-ink-secondary opacity-0 hover:bg-ink/[0.07] hover:text-ink group-hover/section:opacity-100 group-focus-within/section:opacity-100 aria-expanded:opacity-100",
                    onclick: move |_| menu.toggle(),
                    Icon { name: "more", size: 14 }
                }
            }
            if menu() {
                div { class: "fixed inset-0 z-40", onclick: move |_| menu.set(false) }
                div { role: "menu", "data-section-actions": "{name}", class: "absolute right-0 top-full z-50 mt-1 w-[230px] {MENU_SHEET}",
                    button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: move |_| open(Open::Instructions(n1.clone())), "Shared instructions…" }
                    button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: move |_| open(Open::Members(n2.clone())), "Bots and lead…" }
                    button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: move |_| open(Open::Memory(n3.clone())), "Department memory" }
                    button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: move |_| open(Open::Rename(n4.clone())), "Rename…" }
                    button { r#type: "button", role: "menuitem", class: "{MENU_ROW} text-danger", onclick: move |_| open(Open::Delete(n5.clone())), "Delete department…" }
                }
            }
        }
    }
}

/// The "Move to department" menu a bot's row opens on right-click.
#[component]
pub fn MoveMenu(bot: Bot, on_close: EventHandler<()>, on_open: EventHandler<Open>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let list = depts.read().list.clone();
    let current = bot.department.clone();
    let id = bot.id.clone();
    rsx! {
        div { class: "fixed inset-0 z-40", onclick: move |_| on_close.call(()), oncontextmenu: move |e| { e.prevent_default(); on_close.call(()); } }
        div { role: "menu", "aria-label": "Move {bot.name} to a department", "data-move-menu": "{bot.id}", class: "absolute left-10 top-10 z-50 w-[230px] {MENU_SHEET}",
            div { class: "px-2 pb-0.5 pt-1 text-[12px] text-ink-tertiary", "Move to department" }
            for dept in list {
                MoveItem { key: "{dept.name}", name: dept.name.clone(), bot: id.clone(), here: dept.name == current, on_close }
            }
            if !current.is_empty() {
                MoveItem { name: GENERAL.to_string(), bot: id.clone(), here: false, on_close }
            }
            button { r#type: "button", role: "menuitem", class: MENU_ROW, "data-new-department": true,
                onclick: {
                    let id = id.clone();
                    move |_| { on_close.call(()); on_open.call(Open::New(vec![id.clone()])); }
                },
                Icon { name: "plus", size: 14 }
                "New department…"
            }
        }
    }
}

#[component]
fn MoveItem(name: String, bot: String, here: bool, on_close: EventHandler<()>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let label = if name == GENERAL { "Move to General".to_string() } else { name.clone() };
    rsx! {
        button { r#type: "button", role: "menuitemradio", "aria-checked": "{here}", disabled: here, class: MENU_ROW, "data-move-to": "{name}",
            onclick: move |_| {
                move_bots(depts, &name, vec![bot.clone()]);
                on_close.call(());
            },
            span { class: "min-w-0 flex-1 truncate", "{label}" }
            if here {
                Icon { name: "check", size: 14 }
            }
        }
    }
}

const SCRIM: &str = "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4";
const SHEET: &str = "flex max-h-[min(680px,calc(100dvh-2rem))] w-full flex-col overflow-hidden rounded-2xl bg-panel shadow-2xl";
const LABEL: &str = "block text-[12px] font-medium text-ink-secondary";
const FIELD: &str = "mt-1 w-full rounded-lg bg-inset px-3 py-2 text-[14px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const CANCEL: &str = "rounded-lg px-3 py-2 text-[13px] text-ink-secondary hover:bg-control";
const PRIMARY: &str = "ui-button-primary rounded-lg bg-accent px-3 py-2 text-[13px] font-medium text-accent-ink hover:brightness-110 disabled:opacity-50";

/// Whichever department dialog is open.
#[component]
pub fn Dialogs(open: Signal<Option<Open>>) -> Element {
    let close = move |_| open.set(None);
    match open() {
        Some(Open::New(bots)) => rsx! { NewDialog { start: bots, on_close: close } },
        Some(Open::Rename(name)) => rsx! { RenameDialog { name, on_close: close } },
        Some(Open::Instructions(name)) => rsx! { InstructionsDialog { name, on_close: close } },
        Some(Open::Members(name)) => rsx! { MembersDialog { name, on_close: close } },
        Some(Open::Delete(name)) => rsx! { DeleteDialog { name, on_close: close } },
        Some(Open::Memory(name)) => {
            let note = use_context::<Signal<Depts>>().read().get(&name).and_then(|d| d.memory_note.clone()).unwrap_or_default();
            rsx! { crate::memory::MemoryDialog { start: note, on_close: close } }
        }
        None => rsx! {},
    }
}

/// The bots a dialog can pick from: every bot, channels left out.
fn pickable(state: &State) -> Vec<Bot> {
    state.bots.iter().filter(|b| b.channel.is_none()).cloned().collect()
}

/// A bot with a tick box, for choosing who's in a department.
#[component]
fn Pick(bot: Bot, ticked: bool, note: String, on_toggle: EventHandler<()>) -> Element {
    rsx! {
        label { class: "flex cursor-pointer items-center gap-2.5 rounded-lg px-2 py-1.5 hover:bg-raised/60", "data-pick-bot": "{bot.id}",
            input { r#type: "checkbox", checked: ticked, class: "size-4 accent-[var(--color-accent)]", onchange: move |_| on_toggle.call(()) }
            BotMark { color: bot.color.clone(), initial: bot.initial(), size: 24, mark: bot.mark.clone() }
            span { class: "min-w-0 flex-1 truncate text-[14px] text-ink", "{bot.name}" }
            if !note.is_empty() {
                span { class: "shrink-0 text-[12px] text-ink-tertiary", "{note}" }
            }
        }
    }
}

#[component]
fn NewDialog(start: Vec<String>, on_close: EventHandler<()>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let state = use_context::<Signal<State>>();
    let mut name = use_signal(String::new);
    let mut chosen = use_signal(|| start.clone());
    let mut saving = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let bots = pickable(&state.read());
    let create = move |_| {
        if saving() {
            return;
        }
        saving.set(true);
        problem.set(None);
        let body = json!({ "name": name.peek().trim(), "botIds": *chosen.peek() });
        spawn(async move {
            match send("POST", "/api/departments".into(), body, depts).await {
                Ok(()) => on_close.call(()),
                Err(err) => problem.set(Some(err)),
            }
            saving.set(false);
        });
    };
    rsx! {
        div { class: SCRIM, onclick: move |_| on_close.call(()),
            div { role: "dialog", "aria-label": "Create department", "data-department-dialog": "new", class: "{SHEET} max-w-[460px] p-5", onclick: move |e| e.stop_propagation(),
                h2 { class: "text-[17px] font-semibold text-ink", "Create department" }
                p { class: "mt-1 text-[13px] leading-relaxed text-ink-secondary", "Departments organize your bots and share instructions. Start empty, or choose existing bots to move here." }
                label { class: "mt-4 {LABEL}", "Department name"
                    input { class: FIELD, maxlength: 60, autofocus: true, placeholder: "Garden", "data-department-name": true, oninput: move |e| name.set(e.value()) }
                }
                div { class: "mt-4 {LABEL}", "Existing bots" }
                div { class: "mt-1 min-h-0 flex-1 overflow-y-auto",
                    for bot in bots {
                        Pick {
                            key: "{bot.id}",
                            ticked: chosen.read().contains(&bot.id),
                            note: if bot.department.is_empty() { String::new() } else { format!("in {}", bot.department) },
                            on_toggle: {
                                let id = bot.id.clone();
                                move |_| {
                                    let mut list = chosen.write();
                                    if let Some(at) = list.iter().position(|b| *b == id) { list.remove(at); } else { list.push(id.clone()); }
                                }
                            },
                            bot,
                        }
                    }
                }
                p { class: "mt-3 text-[12px] leading-relaxed text-ink-tertiary", "Moving changes who they work with and which shared instructions they read. Their chats stay with them." }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
                }
                div { class: "mt-4 flex justify-end gap-2",
                    button { r#type: "button", class: CANCEL, onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "button", disabled: saving() || name.read().trim().is_empty(), class: PRIMARY, "data-department-create": true, onclick: create, "Create department" }
                }
            }
        }
    }
}

#[component]
fn RenameDialog(name: String, on_close: EventHandler<()>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let mut wanted = use_signal(|| name.clone());
    let mut problem = use_signal(|| None::<String>);
    let old = name.clone();
    let save = move |_| {
        let body = json!({ "name": wanted.peek().trim() });
        let path = path(&old);
        spawn(async move {
            match send("PATCH", path, body, depts).await {
                Ok(()) => on_close.call(()),
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    rsx! {
        div { class: SCRIM, onclick: move |_| on_close.call(()),
            div { role: "dialog", "aria-label": "Rename {name} department", class: "{SHEET} max-w-[400px] p-5", onclick: move |e| e.stop_propagation(),
                h2 { class: "text-[17px] font-semibold text-ink", "Rename department" }
                label { class: "mt-4 {LABEL}", "Department name"
                    input { class: FIELD, maxlength: 60, autofocus: true, value: "{wanted}", oninput: move |e| wanted.set(e.value()) }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
                }
                div { class: "mt-4 flex justify-end gap-2",
                    button { r#type: "button", class: CANCEL, onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "button", disabled: wanted.read().trim().is_empty(), class: PRIMARY, onclick: save, "Rename" }
                }
            }
        }
    }
}

/// The department's shared instructions (their `SectionContextDialog`).
#[component]
pub fn InstructionsDialog(name: String, on_close: EventHandler<()>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let dept = depts.read().get(&name).cloned().unwrap_or_default();
    let max = depts.read().instructions_max;
    let saved = dept.instructions.clone();
    let mut text = use_signal(|| dept.instructions.clone());
    let mut saving = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let bytes = text.read().len();
    let dirty = *text.read() != saved;
    let when = dept.updated_at.map(|at| format!(" Last saved {} {}.", crate::chat::short_when(at).to_lowercase(), crate::chat::clock(at))).unwrap_or_default();
    let target = path(&name);
    let save = move |_| {
        saving.set(true);
        problem.set(None);
        let body = json!({ "instructions": *text.peek() });
        let target = target.clone();
        spawn(async move {
            if let Err(err) = send("PATCH", target, body, depts).await {
                problem.set(Some(err));
            }
            saving.set(false);
        });
    };
    let over = if bytes > max { "shrink-0 tabular-nums text-danger" } else { "shrink-0 tabular-nums" };
    rsx! {
        div { class: SCRIM, onclick: move |_| on_close.call(()),
            div { role: "dialog", "aria-label": "{name} shared instructions", "data-department-dialog": "instructions", class: "{SHEET} max-w-[680px]", onclick: move |e| e.stop_propagation(),
                header { class: "flex items-start justify-between gap-4 border-b border-hairline/40 px-6 pb-4 pt-6",
                    div {
                        div { class: "flex items-center gap-2",
                            span { class: "text-accent", Icon { name: "book", size: 19 } }
                            h2 { class: "text-[20px] font-semibold tracking-[-0.01em] text-ink", "{name} shared instructions" }
                        }
                        p { class: "mt-1.5 max-w-[520px] text-[13px] leading-relaxed text-ink-secondary", "Shown to every bot in this department at the start of each turn. Only you can edit them." }
                    }
                    button { r#type: "button", "aria-label": "Close shared instructions", class: "flex size-9 shrink-0 items-center justify-center rounded-lg text-ink-secondary hover:bg-raised hover:text-ink", onclick: move |_| on_close.call(()),
                        Icon { name: "x", size: 19 }
                    }
                }
                div { class: "min-h-0 flex-1 overflow-y-auto px-6 py-5",
                    textarea { "data-department-instructions": true, "aria-label": "{name} shared instructions", value: "{text}", autofocus: true,
                        placeholder: "Goals\n- Keep the beds watered\n\nDecisions\n- Organic seed only\n\nPreferences\n- Short daily updates",
                        class: "min-h-[280px] w-full resize-y rounded-xl border border-hairline/60 bg-inset px-4 py-3 font-mono text-[13px] leading-relaxed text-ink outline-none placeholder:text-ink-tertiary focus:border-accent/50",
                        oninput: move |e| text.set(e.value()),
                    }
                    div { class: "mt-2 flex items-start justify-between gap-4 text-[12px] text-ink-secondary",
                        span { "Keep lasting department facts here. Each bot's private notes stay in its own memory.{when}" }
                        span { class: over, "{bytes} / {max} bytes" }
                    }
                    if let Some(err) = problem() {
                        div { role: "alert", class: "mt-3 rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-[12px] text-danger", "{err}" }
                    }
                }
                footer { class: "flex items-center justify-end gap-2 border-t border-hairline/40 px-6 py-4",
                    button { r#type: "button", class: CANCEL, onclick: move |_| on_close.call(()), if dirty { "Cancel" } else { "Close" } }
                    button { r#type: "button", "data-department-save": true, disabled: saving() || !dirty || bytes > max, class: PRIMARY, onclick: save, "Save shared instructions" }
                }
            }
        }
    }
}

/// Who is in the department, and who leads it.
#[component]
pub fn MembersDialog(name: String, on_close: EventHandler<()>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let state = use_context::<Signal<State>>();
    let dept = depts.read().get(&name).cloned().unwrap_or_default();
    let mut chosen = use_signal(|| dept.members.clone());
    let lead = use_signal(|| dept.chief.clone());
    let mut saving = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let bots = pickable(&state.read());
    let before = dept.members.clone();
    let before_lead = dept.chief.clone();
    let target = name.clone();
    let save = move |_| {
        saving.set(true);
        problem.set(None);
        let now = chosen.peek().clone();
        let add: Vec<String> = now.iter().filter(|id| !before.contains(id)).cloned().collect();
        let remove: Vec<String> = before.iter().filter(|id| !now.contains(id)).cloned().collect();
        let lead_now = lead.peek().clone().filter(|l| now.contains(l));
        let lead_changed = lead_now != before_lead;
        let target = target.clone();
        spawn(async move {
            let mut outcome = Ok(());
            if !add.is_empty() || !remove.is_empty() {
                outcome = send("PUT", format!("{}/members", path(&target)), json!({ "add": add, "remove": remove }), depts).await;
            }
            if outcome.is_ok() && lead_changed {
                outcome = send("PATCH", path(&target), json!({ "chief": lead_now }), depts).await;
            }
            match outcome {
                Ok(()) => on_close.call(()),
                Err(err) => problem.set(Some(err)),
            }
            saving.set(false);
        });
    };
    rsx! {
        div { class: SCRIM, onclick: move |_| on_close.call(()),
            div { role: "dialog", "aria-label": "Bots in {name}", "data-department-dialog": "members", class: "{SHEET} max-w-[460px] p-5", onclick: move |e| e.stop_propagation(),
                h2 { class: "text-[17px] font-semibold text-ink", "Bots in {name}" }
                p { class: "mt-1 text-[13px] leading-relaxed text-ink-secondary", "Ticked bots belong to this department; untick one to move it to General. The lead shares out the department's work, under your Chief of Staff." }
                div { class: "mt-3 min-h-0 flex-1 overflow-y-auto",
                    for bot in bots {
                        div { key: "{bot.id}", class: "flex items-center gap-1",
                            div { class: "min-w-0 flex-1",
                                Pick {
                                    ticked: chosen.read().contains(&bot.id),
                                    note: if bot.department.is_empty() || bot.department == name { String::new() } else { format!("in {}", bot.department) },
                                    on_toggle: {
                                        let id = bot.id.clone();
                                        move |_| {
                                            let mut list = chosen.write();
                                            if let Some(at) = list.iter().position(|b| *b == id) { list.remove(at); } else { list.push(id.clone()); }
                                        }
                                    },
                                    bot: bot.clone(),
                                }
                            }
                            if chosen.read().contains(&bot.id) {
                                LeadToggle { id: bot.id.clone(), lead }
                            }
                        }
                    }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
                }
                div { class: "mt-4 flex justify-end gap-2",
                    button { r#type: "button", class: CANCEL, onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "button", disabled: saving(), class: PRIMARY, "data-department-members-save": true, onclick: save, "Save" }
                }
            }
        }
    }
}

#[component]
fn LeadToggle(id: String, lead: Signal<Option<String>>) -> Element {
    let on = lead.read().as_deref() == Some(id.as_str());
    let label = if on { "Leads it" } else { "Make lead" };
    let cls = if on { "flex h-7 shrink-0 items-center gap-1 rounded-full bg-accent/15 px-2 text-[12px] font-medium text-accent" } else { "flex h-7 shrink-0 items-center gap-1 rounded-full px-2 text-[12px] text-ink-tertiary hover:bg-raised hover:text-ink" };
    rsx! {
        button { r#type: "button", "aria-pressed": "{on}", "data-make-lead": "{id}", class: cls,
            onclick: move |_| lead.set(if on { None } else { Some(id.clone()) }),
            Icon { name: "crown", size: 12 }
            "{label}"
        }
    }
}

#[component]
fn DeleteDialog(name: String, on_close: EventHandler<()>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let mut problem = use_signal(|| None::<String>);
    let target = path(&name);
    let delete = move |_| {
        let target = target.clone();
        spawn(async move {
            match send("DELETE", target, Value::Null, depts).await {
                Ok(()) => on_close.call(()),
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    rsx! {
        div { class: SCRIM, onclick: move |_| on_close.call(()),
            div { role: "alertdialog", "aria-label": "Delete {name} department", class: "{SHEET} max-w-[420px] p-5", onclick: move |e| e.stop_propagation(),
                h2 { class: "text-[17px] font-semibold text-ink", "Delete {name} department?" }
                p { class: "mt-2 text-[13px] leading-relaxed text-ink-secondary", "Its bots move to General with their chats. The department and its shared instructions are deleted; its memory note stays in the memory vault. This can't be undone." }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-2 text-[12px] text-danger", "{err}" }
                }
                div { class: "mt-4 flex justify-end gap-2",
                    button { r#type: "button", class: CANCEL, onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "button", "data-department-delete": true, class: "rounded-lg bg-danger px-3 py-2 text-[13px] font-medium text-white hover:brightness-110", onclick: delete, "Delete department" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bots_fall_into_their_departments_then_general() {
        let bot = |id: &str, dept: &str| {
            let mut b = Bot::from_wire(&json!({ "id": id, "name": id })).unwrap();
            b.department = dept.to_string();
            b
        };
        let depts = Depts::from_wire(&json!({ "departments": [{ "name": "Garden", "members": ["a"] }, { "name": "Empty", "members": [] }] }));
        let grouped = groups(&[bot("a", "Garden"), bot("b", ""), bot("c", "Gone")], &depts);
        let shape: Vec<(String, Vec<String>)> = grouped.into_iter().map(|(n, b)| (n, b.into_iter().map(|b| b.id).collect())).collect();
        assert_eq!(shape, [("Garden".into(), vec!["a".to_string()]), ("Empty".into(), vec![]), ("General".into(), vec!["b".into(), "c".into()])]);
    }
}
