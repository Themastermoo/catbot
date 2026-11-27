//! Desktop pets, in the interface: the switch that makes a bot a pet (with a word about the
//! computer when it's not the first), which opens from the bot's picture in the list, Settings ›
//! This app's list, and the little tutorial window about the desktop pet ([[Catbot Desktop Pet]]
//! in the Codex).
//!
//! The pets themselves are windows of the Linux and Windows app, which come out when its main
//! window closes; the server keeps which bots are pets (`server::pets`).

use dioxus::prelude::*;
use serde_json::json;

use crate::api;
use crate::bots::Bot;
use crate::live::State;
use crate::mascots::BotMark;

/// The tutorial's pages: a title, what it says, and which picture goes with it.
const STEPS: [(&str, &str, Scene); 4] = [
    (
        "Meet your desktop pet",
        "Close Catbot's window and your pet's cat comes out onto your desktop. Your bots keep working the whole time; the cat is a little window of Catbot's own.",
        Scene::Out,
    ),
    (
        "It wanders about",
        "The cat walks along the bottom of the screen, above the taskbar, and now and then hops up to sit on top of an open window. Drag it to put it somewhere.",
        Scene::Walk,
    ),
    (
        "Click it to chat",
        "Click the cat for a small chat beside it: its last reply and a box to write in. It bobs while it's working and hops when it needs you. Open Catbot brings the full window back.",
        Scene::Chat,
    ),
    (
        "Sending it home",
        "Right-click the cat to send it home, or open Catbot and all the cats come in. Pets are off until you tick a bot; each extra pet uses a little more of your computer.",
        Scene::Home,
    ),
];

#[derive(Clone, Copy, PartialEq)]
enum Scene {
    Out,
    Walk,
    Chat,
    Home,
}

/// Whether the tutorial was seen, and how many pets there are, from the server.
async fn status() -> (bool, usize) {
    match api::get("/api/pets").await {
        Ok(found) => (found["tutorialSeen"].as_bool().unwrap_or(false), found["pets"].as_array().map(Vec::len).unwrap_or(0)),
        Err(_) => (true, 0),
    }
}

/// The pet switch for a bot: on asks first when another bot is already a pet (each pet is a
/// window that keeps moving), and shows the tutorial the first time.
#[component]
pub fn PetSwitch(bot: Bot, #[props(default)] listed: bool) -> Element {
    let state = use_context::<Signal<State>>();
    let mut warn = use_signal(|| None::<usize>);
    let mut tutorial = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let id = bot.id.clone();
    let save = use_callback(move |on: bool| {
        let id = id.clone();
        spawn(async move {
            problem.set(None);
            let (seen, _) = status().await;
            match crate::settings::save(state, &id, json!({ "pet": on })).await {
                Ok(()) if on && !seen => tutorial.set(true),
                Ok(()) => {}
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let on = bot.pet;
    let name = bot.name.clone();
    rsx! {
        div { class: "flex items-center gap-3", "data-pet-switch": true,
            if listed {
                BotMark { color: bot.color.clone(), initial: bot.initial(), size: 30, mark: bot.mark.clone() }
                div { class: "min-w-0 flex-1 truncate text-[14px] text-ink", "{name}" }
            } else {
                div { class: "min-w-0 flex-1",
                    div { class: "text-[15px] font-medium text-ink", "Desktop pet" }
                    div { class: "mt-0.5 text-[12px] leading-relaxed text-ink-secondary",
                        if on { "{name}'s cat comes out onto your desktop when Catbot's window closes." } else { "Let {name}'s cat walk about your desktop when Catbot's window closes." }
                    }
                }
            }
            crate::settings::Switch { checked: on, disabled: false, label: "{name} as a desktop pet",
                on_toggle: move |_| {
                    if on {
                        save.call(false);
                    } else {
                        spawn(async move {
                            let (_, pets) = status().await;
                            if pets >= 1 { warn.set(Some(pets)) } else { save.call(true) }
                        });
                    }
                }
            }
        }
        if let Some(err) = problem() {
            p { role: "alert", class: "mt-1 text-[12px] text-danger", "{err}" }
        }
        if let Some(pets) = warn() {
            div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4", onclick: move |_| warn.set(None),
                div { role: "alertdialog", "aria-label": "Another desktop pet", "data-pet-warning": true, class: "w-full max-w-[380px] rounded-2xl bg-panel p-5 shadow-xl", onclick: move |e| e.stop_propagation(),
                    h2 { class: "text-[16px] font-semibold text-ink", "Another pet?" }
                    p { class: "mt-2 text-[13px] leading-relaxed text-ink-secondary",
                        if pets == 1 { "You already have a desktop pet. " } else { "You already have {pets} desktop pets. " }
                        "Each one is its own little window that keeps walking and watching its bot, so every extra pet uses more of your computer: a bit more memory, and a little of the processor all day."
                    }
                    div { class: "mt-4 flex justify-end gap-2",
                        button { r#type: "button", class: "rounded-lg px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| warn.set(None), "Not now" }
                        button { r#type: "button", "data-pet-warning-yes": true, class: "rounded-lg bg-accent px-3 py-2 text-[13px] font-medium text-white hover:brightness-110",
                            onclick: move |_| {
                                warn.set(None);
                                save.call(true);
                            },
                            "Add {name} too"
                        }
                    }
                }
            }
        }
        if tutorial() {
            PetTutorial { bot: bot.clone(), on_close: move |_| tutorial.set(false) }
        }
    }
}

/// A bot's picture in the list: clicking it opens a small card with its desktop pet switch
/// (Moo's choice: the switch lives here, not in the bot's settings), and a paw sits on it while
/// the bot is a pet. The card stops clicks and keys on the way out, so the row behind it
/// doesn't open the chat.
#[component]
pub fn PetPicture(
    bot: Bot,
    /// The colour behind the picture, for the paw's ring.
    ring: &'static str,
    /// Under a centred picture (the Chief of Staff's tile) rather than one at the row's start.
    #[props(default)]
    centered: bool,
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);
    let name = bot.name.clone();
    // The card is 248px; under the 60px tile picture it's centred, otherwise it starts at the picture.
    let place = if centered { "-left-[94px]" } else { "-left-1" };
    rsx! {
        span { class: "relative flex",
            span {
                role: "button",
                tabindex: 0,
                class: "relative flex cursor-pointer rounded-full focus-visible:outline-2 focus-visible:outline-accent",
                title: if bot.pet { "{name} is a desktop pet" } else { "Desktop pet" },
                "aria-label": "{name} as a desktop pet",
                "aria-haspopup": "dialog",
                "aria-expanded": "{open()}",
                "data-pet-picture": "{bot.id}",
                onclick: move |e| {
                    e.stop_propagation();
                    open.toggle();
                },
                onkeydown: move |e| {
                    if e.key() == Key::Enter || e.key() == Key::Character(" ".into()) {
                        e.stop_propagation();
                        e.prevent_default();
                        open.toggle();
                    }
                },
                {children}
                if bot.pet {
                    span { "aria-hidden": "true", "data-pet-paw": true, class: "pointer-events-none absolute -bottom-0.5 -left-0.5 flex size-[15px] items-center justify-center rounded-full text-accent", background: "{ring}",
                        crate::sidebar::Icon { name: "paw", size: 10, filled: true }
                    }
                }
            }
            if open() {
                div { class: "fixed inset-0 z-30", "data-pet-backdrop": true,
                    onclick: move |e| {
                        e.stop_propagation();
                        open.set(false);
                    },
                }
                div {
                    role: "dialog",
                    "aria-label": "{name} as a desktop pet",
                    "data-pet-card": "{bot.id}",
                    class: "absolute {place} top-full z-40 mt-1.5 w-[248px] cursor-default rounded-2xl border-[0.5px] border-edge bg-panel p-3 text-left shadow-xl",
                    onclick: move |e| e.stop_propagation(),
                    onkeydown: move |e| {
                        e.stop_propagation();
                        if e.key() == Key::Escape {
                            open.set(false);
                        }
                    },
                    PetSwitch { bot: bot.clone() }
                }
            }
        }
    }
}

/// Settings › This app: every bot with its pet switch, and the tutorial again.
#[component]
pub fn PetsSettings() -> Element {
    let state = use_context::<Signal<State>>();
    let mut tutorial = use_signal(|| false);
    let bots: Vec<Bot> = state.read().bots.iter().filter(|b| b.channel.is_none()).cloned().collect();
    let first = bots.iter().find(|b| b.pet).or(bots.first()).cloned();
    rsx! {
        div { class: "flex flex-col gap-3 px-4 py-3",
            p { class: "text-[13px] leading-[19px] text-ink-secondary",
                "When Catbot's window closes, the bots ticked here come out as cats that walk about your desktop. Click one to chat. They come from the Linux and Windows app."
            }
            for bot in bots {
                PetSwitch { key: "{bot.id}", bot, listed: true }
            }
            if let Some(bot) = first.clone() {
                button { r#type: "button", "data-pet-tutorial-open": true, class: "self-start text-[13px] text-accent hover:underline", onclick: move |_| tutorial.set(true), "Show the desktop pet tutorial" }
                if tutorial() {
                    PetTutorial { bot, on_close: move |_| tutorial.set(false) }
                }
            }
        }
    }
}

/// The little tutorial window about the desktop pet: four pages, each with a picture of a
/// screen and the bot's cat. Closing it marks it seen.
#[component]
pub fn PetTutorial(bot: Bot, on_close: EventHandler<()>) -> Element {
    let mut step = use_signal(|| 0usize);
    let close = move || {
        // The app's task, not the window's: the window goes as soon as it's closed.
        dioxus::core::spawn_forever(async move {
            let _ = api::call("PUT", "/api/pets", Some(&json!({ "tutorialSeen": true }))).await;
        });
        on_close.call(());
    };
    let (title, words, scene) = STEPS[step()];
    let last = step() + 1 == STEPS.len();
    let words = words.replace("your pet's", &format!("{}'s", bot.name));
    rsx! {
        div { class: "fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-4", onclick: move |_| close(),
            div {
                role: "dialog",
                "aria-label": "Desktop pets",
                "data-pet-tutorial": true,
                class: "w-full max-w-[420px] overflow-hidden rounded-2xl bg-panel shadow-xl animate-pop-in",
                onclick: move |e| e.stop_propagation(),
                onkeydown: move |e| match e.key() {
                    Key::Escape => close(),
                    Key::ArrowRight if !last => step += 1,
                    Key::ArrowLeft if step() > 0 => step -= 1,
                    _ => {}
                },
                ScenePicture { key: "{step}", scene, bot: bot.clone() }
                div { class: "px-6 pb-5 pt-4",
                    p { class: "text-[12px] font-medium uppercase tracking-wide text-ink-tertiary", "Desktop pets · {step() + 1} of {STEPS.len()}" }
                    h2 { class: "mt-1 text-[18px] font-semibold text-ink", "data-pet-tutorial-title": true, "{title}" }
                    p { class: "mt-2 min-h-[80px] text-[14px] leading-[21px] text-ink-secondary", "{words}" }
                    div { class: "mt-4 flex items-center gap-2",
                        div { class: "flex flex-1 gap-1.5", "aria-hidden": "true",
                            for n in 0..STEPS.len() {
                                span { key: "{n}", class: if n == step() { "h-1.5 w-4 rounded-full bg-ink" } else { "size-1.5 rounded-full bg-ink/20" } }
                            }
                        }
                        if step() > 0 {
                            button { r#type: "button", class: "rounded-lg px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| step -= 1, "Back" }
                        } else {
                            button { r#type: "button", class: "rounded-lg px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| close(), "Skip" }
                        }
                        button { r#type: "button", "data-pet-tutorial-next": true, autofocus: true, class: "rounded-lg bg-accent px-4 py-2 text-[13px] font-medium text-white hover:brightness-110",
                            onclick: move |_| if last { close() } else { step += 1 },
                            if last { "Got it" } else { "Next" }
                        }
                    }
                }
            }
        }
    }
}

/// A tutorial page's picture: a little desktop (a window, the taskbar) and the bot's cat doing
/// what the page says.
#[component]
fn ScenePicture(scene: Scene, bot: Bot) -> Element {
    let cat = |size: u32, mood: &str| rsx! { BotMark { color: bot.color.clone(), initial: bot.initial(), size, mark: bot.mark.clone(), state: mood.to_string() } };
    rsx! {
        div { class: "pet-scene relative h-[190px] overflow-hidden bg-[linear-gradient(160deg,#5b7fb8,#9a7fc0_55%,#e0a1a0)]", "aria-hidden": "true", "data-pet-scene": match scene { Scene::Out => "out", Scene::Walk => "walk", Scene::Chat => "chat", Scene::Home => "home" },
            // An open window on the desktop.
            div { class: "absolute left-[12%] top-[22%] h-[44%] w-[46%] rounded-md bg-white/90 shadow-lg",
                div { class: "flex h-4 items-center gap-1 rounded-t-md bg-black/10 px-1.5",
                    span { class: "size-1.5 rounded-full bg-[#ff5f57]" }
                    span { class: "size-1.5 rounded-full bg-[#febc2e]" }
                    span { class: "size-1.5 rounded-full bg-[#28c840]" }
                }
                if scene == Scene::Out {
                    div { class: "flex h-[calc(100%-1rem)] items-center justify-center gap-1.5 text-[11px] font-medium text-black/60", "Catbot" span { class: "pet-scene-close text-black/40", "✕" } }
                }
            }
            // The taskbar.
            div { class: "absolute inset-x-0 bottom-0 h-5 bg-black/35" }
            match scene {
                Scene::Out => rsx! {
                    div { class: "pet-scene-pop absolute bottom-5 left-[62%]", {cat(44, "idle")} }
                },
                Scene::Walk => rsx! {
                    div { class: "pet-scene-walk absolute bottom-5 left-0", {cat(40, "working")} }
                    div { class: "absolute left-[28%] top-[calc(22%-34px)]", {cat(34, "idle")} }
                },
                Scene::Chat => rsx! {
                    div { class: "absolute bottom-5 left-[58%]", {cat(44, "waiting")} }
                    div { class: "pet-scene-bubble absolute bottom-[64px] left-[36%] w-[52%] rounded-xl bg-white p-2 text-[10px] leading-[13px] text-black/80 shadow-lg",
                        p { "{bot.name}: Done — the garden plan is in your folder." }
                        div { class: "mt-1.5 h-4 rounded-full bg-black/[0.06] px-1.5 text-[9px] leading-4 text-black/40", "Message {bot.name}" }
                    }
                },
                Scene::Home => rsx! {
                    div { class: "absolute bottom-5 left-[44%]", {cat(44, "idle")} }
                    div { class: "absolute bottom-[56px] left-[56%] w-[34%] rounded-lg bg-white p-1 text-[10px] text-black/80 shadow-lg",
                        div { class: "rounded px-1.5 py-1", "Chat" }
                        div { class: "rounded px-1.5 py-1", "Open Catbot" }
                        div { class: "rounded bg-black/[0.07] px-1.5 py-1 font-medium", "Send home" }
                    }
                },
            }
        }
    }
}
