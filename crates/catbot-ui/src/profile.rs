//! A bot's own picture and its role and description, in its settings (stage 23).
//!
//! Moo's choices (2026-11-24): upload a picture or have the picture AI draw one; frame it with a
//! zoom slider and by dragging; it shows everywhere the cat does except the desktop pet.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::Bot;
use crate::mascots::{BotMark, Picture};

const BUTTON: &str = "rounded-lg bg-raised px-2.5 py-1.5 text-[13px] font-medium text-ink ring-1 ring-ink/10 hover:bg-raised-hover disabled:opacity-40";
const QUIET: &str = "rounded-lg px-2.5 py-1.5 text-[13px] text-ink-secondary hover:bg-raised hover:text-ink disabled:opacity-40";
const PREVIEW: u32 = 112;

/// Upload, draw, frame or remove the bot's own picture.
#[component]
pub fn PictureEditor(bot: Bot, change: Callback<Value>) -> Element {
    let mut busy = use_signal(|| None::<&'static str>);
    let mut problem = use_signal(|| None::<String>);
    let mut drawing = use_signal(|| false);
    let mut direction = use_signal(String::new);
    let mut drawers = use_signal(Vec::<Value>::new);
    let mut with = use_signal(String::new);
    // The framing while it's being changed; saved when the drag or slider lets go.
    let mut framing = use_signal(|| bot.picture.as_ref().map(|p| (p.zoom, p.x, p.y)).unwrap_or((1.0, 0.5, 0.5)));
    let mut dragging = use_signal(|| None::<(f64, f64, f64, f64)>);
    let saved = bot.picture.as_ref().map(|p| (p.zoom, p.x, p.y));
    use_effect(use_reactive!(|saved| {
        if dragging.peek().is_none()
            && let Some(f) = saved
        {
            framing.set(f);
        }
    }));

    let id = bot.id.clone();
    let pick = move |e: FormEvent| {
        let id = id.clone();
        spawn(async move {
            let Some(file) = e.files().into_iter().next() else { return };
            busy.set(Some("upload"));
            problem.set(None);
            let mime = file.content_type().unwrap_or_else(|| "image/png".into());
            let result = match file.read_bytes().await {
                Ok(bytes) => api::upload(&format!("/api/bots/{id}/picture"), &file.name(), &mime, bytes.to_vec()).await.map(|_| ()),
                Err(err) => Err(format!("couldn't read the picture: {err}")),
            };
            if let Err(err) = result {
                problem.set(Some(err));
            }
            busy.set(None);
            document::eval(r#"const i = document.getElementById("picture-file"); if (i) i.value = "";"#);
        });
    };
    let id = bot.id.clone();
    let open_draw = move |_| {
        drawing.toggle();
        let id = id.clone();
        spawn(async move {
            if let Ok(found) = api::get(&format!("/api/bots/{id}/picture/draw")).await {
                let list = found["drawers"].as_array().cloned().unwrap_or_default();
                if with.peek().is_empty()
                    && let Some(first) = list.first().and_then(|d| d["id"].as_str())
                {
                    with.set(first.to_string());
                }
                drawers.set(list);
            }
        });
    };
    let id = bot.id.clone();
    let draw = move |_| {
        let id = id.clone();
        spawn(async move {
            busy.set(Some("draw"));
            problem.set(None);
            let body = json!({ "prompt": direction.peek().clone(), "with": with.peek().clone() });
            match api::post(&format!("/api/bots/{id}/picture/draw"), &body).await {
                Ok(_) => drawing.set(false),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(None);
        });
    };
    let save_framing = move || {
        let (zoom, x, y) = *framing.peek();
        change.call(json!({ "avatarZoom": (zoom * 100.0).round() / 100.0, "avatarFocusX": (x * 1000.0).round() / 1000.0, "avatarFocusY": (y * 1000.0).round() / 1000.0 }));
    };
    let (zoom, x, y) = framing();
    let preview = bot.picture.as_ref().map(|p| Picture { url: p.url.clone(), zoom, x, y });
    let label = match busy() {
        Some("upload") => "Uploading…",
        Some("draw") => "Drawing… (this can take a minute)",
        _ => "",
    };
    rsx! {
        div { class: "mt-4 flex flex-col gap-3", "data-picture-editor": true,
            div { class: "text-[12px] font-medium text-ink-secondary", "Its own picture" }
            if let Some(picture) = preview.clone() {
                div { class: "flex items-center gap-4",
                    div {
                        "data-picture-frame": true,
                        title: "Drag to move what's in the middle",
                        class: "relative flex shrink-0 cursor-grab touch-none select-none rounded-full ring-2 ring-accent/50 active:cursor-grabbing",
                        onpointerdown: move |e| {
                            let p = e.client_coordinates();
                            let (_, x, y) = *framing.peek();
                            dragging.set(Some((p.x, p.y, x, y)));
                        },
                        onpointermove: move |e| {
                            let Some((sx, sy, x0, y0)) = *dragging.peek() else { return };
                            let p = e.client_coordinates();
                            let (zoom, _, _) = *framing.peek();
                            let span = f64::from(PREVIEW) * zoom;
                            framing.set((zoom, (x0 - (p.x - sx) / span).clamp(0.0, 1.0), (y0 - (p.y - sy) / span).clamp(0.0, 1.0)));
                        },
                        onpointerup: move |_| {
                            if dragging.take().is_some() {
                                save_framing();
                            }
                        },
                        onpointerleave: move |_| {
                            if dragging.take().is_some() {
                                save_framing();
                            }
                        },
                        BotMark { picture: picture, color: bot.color.clone(), initial: bot.initial(), size: PREVIEW }
                    }
                    div { class: "flex min-w-0 flex-1 flex-col gap-2",
                        label { class: "flex items-center gap-2 text-[12px] text-ink-secondary",
                            "Zoom"
                            input {
                                r#type: "range",
                                "data-picture-zoom": true,
                                min: "1",
                                max: "3",
                                step: "0.05",
                                value: "{zoom}",
                                class: "min-w-0 flex-1 accent-[var(--color-accent)]",
                                oninput: move |e| {
                                    if let Ok(z) = e.value().parse::<f64>() {
                                        let (_, x, y) = *framing.peek();
                                        framing.set((z.clamp(1.0, 3.0), x, y));
                                    }
                                },
                                onchange: move |_| save_framing(),
                            }
                        }
                        p { class: "text-[12px] text-ink-tertiary", "Drag the picture to move what's in the middle." }
                        div { class: "flex flex-wrap gap-1",
                            button { r#type: "button", class: QUIET, "data-picture-reset": true,
                                onclick: move |_| {
                                    framing.set((1.0, 0.5, 0.5));
                                    save_framing();
                                },
                                "Reset framing"
                            }
                            button { r#type: "button", class: QUIET, "data-picture-remove": true, onclick: move |_| change.call(json!({ "avatar": null })), "Back to its cat" }
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-2",
                input { id: "picture-file", r#type: "file", accept: "image/png,image/jpeg,image/webp,image/gif", class: "hidden", onchange: pick }
                button { r#type: "button", class: BUTTON, "data-picture-upload": true, disabled: busy().is_some(),
                    onclick: move |_| { document::eval(r#"document.getElementById("picture-file")?.click();"#); },
                    if bot.picture.is_some() { "Upload another" } else { "Upload a picture" }
                }
                button { r#type: "button", class: BUTTON, "data-picture-draw-open": true, disabled: busy().is_some(), onclick: open_draw, "Have one drawn" }
                if !label.is_empty() {
                    span { role: "status", class: "text-[12px] text-ink-secondary", "{label}" }
                }
            }
            if drawing() {
                div { class: "flex flex-col gap-2 rounded-lg bg-raised/60 p-3", "data-picture-draw": true,
                    textarea {
                        rows: 2,
                        maxlength: 400,
                        placeholder: "What should it look like? (a fox in a lab coat, a calm owl…) Leave empty for one that suits its role.",
                        class: "w-full resize-y rounded-lg bg-app px-2.5 py-1.5 text-[13px] text-ink ring-1 ring-ink/10 focus:outline-none",
                        value: "{direction}",
                        oninput: move |e| direction.set(e.value()),
                    }
                    div { class: "flex items-center gap-2",
                        select { class: "min-w-0 flex-1 rounded-lg bg-app px-2 py-1.5 text-[13px] text-ink ring-1 ring-ink/10", "aria-label": "Which AI draws it", "data-picture-with": true,
                            onchange: move |e| with.set(e.value()),
                            for d in drawers.read().iter() {
                                option { value: "{d[\"id\"].as_str().unwrap_or_default()}", selected: d["id"] == with().as_str(), "{d[\"label\"].as_str().unwrap_or_default()}" }
                            }
                        }
                        button { r#type: "button", class: BUTTON, "data-picture-draw-go": true, disabled: busy().is_some() || with().is_empty(), onclick: draw, "Draw" }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
            if bot.picture.is_some() {
                p { class: "text-[12px] text-ink-tertiary", "It shows wherever {bot.name} does; as a desktop pet it stays a cat." }
            }
        }
    }
}

/// The role and description fields: what the bot is for, told to it every turn.
#[component]
pub fn RoleFields(bot: Bot, change: Callback<Value>, field: &'static str, label_class: &'static str) -> Element {
    let current_title = bot.title.clone();
    let current_description = bot.description.clone();
    rsx! {
        label { class: "mt-3 {label_class}", "Role"
            input {
                key: "{bot.id}-title",
                class: field,
                maxlength: 80,
                placeholder: "Researcher, Writer, Gardener…",
                "data-bot-title": true,
                // The saved text: an Undo or another window changes it here too.
                value: "{bot.title}",
                onchange: move |e: FormEvent| {
                    let title = e.value().trim().to_string();
                    if title != current_title {
                        change.call(json!({ "title": title }));
                    }
                },
            }
        }
        label { class: "mt-3 {label_class}", "Description"
            textarea {
                key: "{bot.id}-description",
                rows: 3,
                maxlength: 1000,
                class: "{field} resize-y text-[13px] leading-relaxed",
                placeholder: "A few lines about it, for you and for the team. It's told this too.",
                "data-bot-description": true,
                value: "{bot.description}",
                onchange: move |e: FormEvent| {
                    if e.value().trim() != current_description.trim() {
                        change.call(json!({ "description": e.value().trim() }));
                    }
                },
            }
        }
    }
}

/// The bot's settings history (stage 23.4): each change, newest first, and going back to the
/// settings from before one.
#[component]
pub fn HistoryCard(bot: Bot) -> Element {
    let mut changes = use_signal(Vec::<Value>::new);
    let mut open = use_signal(|| false);
    let mut asking = use_signal(|| None::<i64>);
    let mut problem = use_signal(|| None::<String>);
    // Read again whenever the bot changes (each change is one of these).
    let id = bot.id.clone();
    let seen = format!("{bot:?}");
    use_effect(use_reactive!(|seen, id| {
        let _ = seen;
        let id = id.clone();
        spawn(async move {
            if let Ok(found) = api::get(&format!("/api/bots/{id}/history")).await {
                changes.set(found["changes"].as_array().cloned().unwrap_or_default());
            }
        });
    }));
    let list = changes.read().clone();
    let shown: Vec<Value> = if open() { list.clone() } else { list.iter().take(5).cloned().collect() };
    let bot_id = bot.id.clone();
    rsx! {
        div { class: "rounded-xl bg-card p-4", "data-bot-history": true,
            div { class: "text-[15px] font-medium text-ink", "History" }
            if list.is_empty() {
                p { class: "mt-1 text-[12px] text-ink-secondary", "Changes to {bot.name}'s settings show here, and can be undone." }
            }
            for change in shown {
                {
                    let cid = change["id"].as_i64().unwrap_or_default();
                    let at = crate::chat::parse_time(change["at"].as_str().unwrap_or_default());
                    let when = crate::chat::ahead_when(at);
                    let what = change["what"].as_str().unwrap_or_default().to_string();
                    let bot_id = bot_id.clone();
                    rsx! {
                        div { key: "{cid}", class: "mt-2 flex items-center gap-2 text-[13px]", "data-history-change": "{what}",
                            span { class: "min-w-0 flex-1",
                                span { class: "block truncate text-ink", "{what}" }
                                span { class: "block text-[11px] text-ink-tertiary", "{when}" }
                            }
                            if asking() == Some(cid) {
                                span { class: "text-[12px] text-ink-secondary", "Go back?" }
                                button { r#type: "button", class: QUIET, onclick: move |_| asking.set(None), "Keep" }
                                button { r#type: "button", class: BUTTON, "data-history-confirm": true,
                                    onclick: move |_| {
                                        asking.set(None);
                                        let bot_id = bot_id.clone();
                                        spawn(async move {
                                            if let Err(err) = api::post(&format!("/api/bots/{bot_id}/history/{cid}/undo"), &json!({})).await {
                                                problem.set(Some(err));
                                            }
                                        });
                                    },
                                    "Undo"
                                }
                            } else {
                                button { r#type: "button", class: QUIET, "data-history-undo": "{cid}", title: "Put the settings back as they were before this change", onclick: move |_| asking.set(Some(cid)), "Undo" }
                            }
                        }
                    }
                }
            }
            if list.len() > 5 {
                button { r#type: "button", class: "mt-2 text-[12px] text-ink-secondary hover:text-ink", onclick: move |_| open.toggle(),
                    if open() { "Show fewer" } else { "Show all {list.len()}" }
                }
            }
            p { class: "mt-2 text-[11px] text-ink-tertiary", "Undo puts every setting back as it was just before that change. A replaced picture isn't kept." }
            if let Some(err) = problem() {
                p { role: "alert", class: "mt-1 text-[12px] text-danger", "{err}" }
            }
        }
    }
}

/// The bots put away in the archive, under the sidebar's list (stage 23.5): each can be brought
/// back, or deleted for good.
#[component]
pub fn ArchivedBots() -> Element {
    let state = use_context::<Signal<crate::live::State>>();
    let mut list = use_signal(Vec::<Value>::new);
    let mut open = use_signal(|| false);
    let mut confirm = use_signal(|| None::<String>);
    let mut problem = use_signal(|| None::<String>);
    use_effect(move || {
        let _ = state.read().archived_changed;
        spawn(async move {
            if let Ok(found) = api::get("/api/archived-bots").await {
                list.set(found["bots"].as_array().cloned().unwrap_or_default());
            }
        });
    });
    let bots = list.read().clone();
    if bots.is_empty() {
        return rsx! {};
    }
    let count = bots.len();
    rsx! {
        div { class: "px-3 pt-2", "data-archived-bots": true,
            button { r#type: "button", "aria-expanded": "{open()}", "data-archived-bots-toggle": true,
                class: "flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-[12px] text-ink-secondary hover:bg-raised/40 hover:text-ink",
                onclick: move |_| open.toggle(),
                crate::chats::Chevron { open: open(), size: 11 }
                span { class: "flex-1", "Archived bots" }
                span { class: "text-[10px] opacity-60", "{count}" }
            }
            if open() {
                for b in bots {
                    {
                        let id = b["id"].as_str().unwrap_or_default().to_string();
                        let name = b["name"].as_str().unwrap_or_default().to_string();
                        let picture = b["avatarUrl"].as_str().map(|url| Picture { url: url.to_string(), zoom: 1.0, x: 0.5, y: 0.5 });
                        let color = b["color"].as_str().unwrap_or("blue").to_string();
                        let mark = b["mark"].as_str().unwrap_or("cat-head").to_string();
                        let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                        let (back_id, delete_id) = (id.clone(), id.clone());
                        let asking = confirm() == Some(id.clone());
                        rsx! {
                            div { key: "{id}", class: "flex items-center gap-2 rounded-md px-2 py-1.5 text-[13px]", "data-archived-bot": "{name}",
                                BotMark { picture, color, initial, size: 24, mark }
                                span { class: "min-w-0 flex-1 truncate text-ink-secondary", "{name}" }
                                if asking {
                                    button { r#type: "button", class: QUIET, onclick: move |_| confirm.set(None), "Keep" }
                                    button { r#type: "button", class: "rounded-lg bg-danger px-2 py-1 text-[12px] font-medium text-white", "data-archived-delete-confirm": true,
                                        onclick: move |_| {
                                            confirm.set(None);
                                            let id = delete_id.clone();
                                            spawn(async move {
                                                match api::call("DELETE", &format!("/api/bots/{id}"), None).await {
                                                    Ok(_) => list.write().retain(|b| b["id"] != id.as_str()),
                                                    Err(err) => problem.set(Some(err)),
                                                }
                                            });
                                        },
                                        "Delete for good"
                                    }
                                } else {
                                    button { r#type: "button", class: QUIET, "data-archived-back": "{name}",
                                        onclick: move |_| {
                                            let id = back_id.clone();
                                            spawn(async move {
                                                if let Err(err) = api::post(&format!("/api/bots/{id}/unarchive"), &json!({})).await {
                                                    problem.set(Some(err));
                                                }
                                            });
                                        },
                                        "Bring back"
                                    }
                                    button { r#type: "button", class: QUIET, title: "Delete", "aria-label": "Delete {name}", onclick: move |_| confirm.set(Some(id.clone())), "×" }
                                }
                            }
                        }
                    }
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "px-2 text-[12px] text-danger", "{err}" }
                }
            }
        }
    }
}
