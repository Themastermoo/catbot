//! Catbot's interface: the chat app, rebuilt in Dioxus with OpenMausBot's look.
//!
//! Layout and class names follow OpenMausBot's `src/App.tsx` and `src/components/Sidebar.tsx`
//! (Apache-2.0, see NOTICE), so the stylesheet's tokens and skins apply unchanged.

#[cfg(target_os = "android")]
mod android;
mod api;
mod scheduled;
mod voice;
mod profile;
mod apps;
mod blender;
mod comfyui;
mod computers;
mod attach;
mod emoji_list;
mod react;
mod bots;
mod changes;
mod channels;
mod skills;
mod team;
mod teams;
mod grok;
mod triggers;
mod chief;
mod handoffs;
mod chat;
mod chats;
mod devices;
mod drafts;
mod email;
mod engines;
mod link;
#[cfg(not(target_arch = "wasm32"))]
mod local;
mod local_ai;
mod pages;
mod pair;
mod events;
mod live;
mod mascots;
mod cat_anim;
mod notify;
mod pictures;
mod markdown;
mod memory;
mod settings;
mod usage;
mod sidebar;
mod looks;
mod prefs;
mod departments;
mod update;
mod welcome;
mod inbox;
mod mapcomposer;
mod model_menu;
mod teammap;
mod board;
mod panel;
mod pets;
#[cfg(feature = "desktop")]
mod pet_tops;
#[cfg(feature = "desktop")]
mod pet_window;
#[cfg(feature = "desktop")]
mod tray;

use dioxus::prelude::*;

use bots::Bot;
use sidebar::Sidebar;

const STYLES: Asset = asset!("/assets/tailwind.css");
/// How the bot pictures move (Catbot's own; `tailwind.css` stays as OpenMausBot's).
const MASCOT_STYLES: Asset = asset!("/assets/mascots.css");
/// The bundled fonts, copied through as a folder so `fonts.css` finds its files beside it.
const FONTS: Asset = asset!("/assets/fonts", AssetOptions::folder());
/// Catbot's icon, the robot cat's head in teal (`assets/icon/`, rendered in Blender by
/// `art/pet3d/build_pet.py`'s icon mode): the browser tab's, large and small.
const ICON: Asset = asset!("/assets/icon/catbot-256.png");
const ICON_SMALL: Asset = asset!("/assets/icon/catbot-32.png");

fn main() {
    #[cfg(feature = "desktop")]
    {
        // The desktop pets' own program (`pet_window`), started when the window closes.
        if std::env::args().any(|a| a == "--pets") {
            pet_window::launch();
            return;
        }
        // The app that runs its own Catbot keeps to the tray (stage 29.2): one at a time, so
        // opening it again shows the one already running.
        let tray = tray::wanted();
        if tray && tray::hand_to_running() {
            return;
        }
        // Opening Catbot sends any pets home.
        pet_window::bring_home();
        use dioxus::desktop::{Config, LogicalSize, WindowBuilder, WindowCloseBehaviour};
        let window = WindowBuilder::new()
            .with_title("Catbot")
            .with_inner_size(LogicalSize::new(1280.0, 840.0))
            .with_min_inner_size(LogicalSize::new(720.0, 480.0))
            // Started at login: in the tray until opened.
            .with_visible(!(tray && tray::hidden_start()));
        let mut config = Config::new().with_window(window).with_menu(None);
        if tray {
            tray::listen();
            config = config.with_close_behaviour(WindowCloseBehaviour::WindowHides);
        }
        // The window's icon, built into the program. (Under Wayland the taskbar takes its icon from
        // the app's menu entry instead, which stage 7.4 installs.)
        match dioxus::desktop::icon_from_memory(include_bytes!("../assets/icon/catbot-256.png")) {
            Ok(icon) => config = config.with_icon(icon),
            Err(err) => eprintln!("catbot: couldn't load the window icon: {err}"),
        }
        // The Windows app and the Linux packages start their own Catbot, which stops when the
        // app quits (from the tray).
        local::start();
        dioxus::LaunchBuilder::desktop().with_cfg(config).launch(Root);
    }
    // The phone app runs its own Catbot inside it (stage 26).
    #[cfg(target_os = "android")]
    local::start_on_phone();
    #[cfg(not(feature = "desktop"))]
    dioxus::launch(Root);
}

/// Ctrl+Shift+N (⌘⇧N on a Mac) starts a new chat with the open bot, as in OpenMausBot, by
/// pressing the chat header's New chat button. (A browser may keep the keys for itself: Chrome
/// opens an incognito window; the desktop app gets them.)
const NEW_CHAT_KEY: &str = r#"
    document.addEventListener("keydown", (event) => {
        if (!(event.ctrlKey || event.metaKey) || !event.shiftKey || event.altKey || event.key.toLowerCase() !== "n") return;
        const button = document.querySelector("[data-new-chat]");
        if (!button) return;
        event.preventDefault();
        button.click();
    });
"#;

#[component]
fn Root() -> Element {
    #[cfg(feature = "desktop")]
    if tray::wanted() {
        tray::use_tray();
    }
    let looks = use_context_provider(|| Signal::new(looks::load()));
    use_hook(move || {
        looks::apply_skin(&looks.peek().skin);
        document::eval(mascots::MASCOT_LIFE);
        document::eval(NEW_CHAT_KEY);
    });
    // Whether this app is paired with Catbot: always, for the web and desktop apps. The phone
    // app starts unpaired, and goes back to the pairing screen if home removes it.
    let mut paired = use_signal(|| !link::pairs() || link::current().is_some());
    // Set once a paired phone has been told it isn't paired any more.
    let mut removed = use_signal(|| false);
    // Where the Windows app is talking (always "this PC" elsewhere, or home for the phone).
    let mut place = use_signal(link::place);
    let mut notice = use_signal(|| None::<String>);
    use_context_provider(|| settings::Places { place, notice });
    rsx! {
        document::Link { rel: "icon", r#type: "image/png", sizes: "256x256", href: ICON }
        document::Link { rel: "icon", r#type: "image/png", sizes: "32x32", href: ICON_SMALL }
        document::Meta { name: "viewport", content: "width=device-width, initial-scale=1, viewport-fit=cover" }
        document::Stylesheet { href: "{FONTS}/fonts.css" }
        document::Stylesheet { href: STYLES }
        document::Stylesheet { href: MASCOT_STYLES }
        if paired() {
            // Keyed by place, so switching starts the app afresh against the other Catbot.
            for here in std::iter::once(place()) {
                App { key: "{here:?}", place: here, on_unpaired: move |_| {
                    if link::has_own() {
                        link::set_place(link::Place::ThisPc);
                        notice.set(Some("Home removed this computer. Pair it again to reach home.".into()));
                        place.set(link::Place::ThisPc);
                    } else {
                        removed.set(true);
                        paired.set(false);
                    }
                } }
            }
        } else {
            pair::PairScreen { removed: removed(), on_paired: move |_| {
                removed.set(false);
                paired.set(true);
            } }
        }
    }
}

#[component]
fn App(place: link::Place, on_unpaired: EventHandler<()>) -> Element {
    let mut state = use_context_provider(|| Signal::new(live::State::default()));
    use_context_provider(|| Signal::new(drafts::load()));
    let mut selected = use_signal(|| None::<String>);
    let settings_open = use_signal(|| false);
    // On a phone, the bot list and a chat take turns filling the screen.
    let showing_chat = use_signal(|| false);
    let prefs = use_signal(|| None::<String>);
    let find = use_signal(|| None::<String>);
    let map = use_signal(|| false);
    let board = use_signal(|| false);
    let ui = use_context_provider(|| settings::Ui { selected, settings_open, showing_chat, prefs, find, map, board });
    use_context_provider(|| attach::Viewing(Signal::new(None)));
    let notify_prefs = use_context_provider(|| Signal::new(notify::load()));
    let inbox = use_context_provider(inbox::Inbox::new);
    let depts = use_context_provider(|| Signal::new(departments::Depts::default()));
    // Voice settings and what's ready (stage 22), read at start and on each `voice` frame.
    let voice = use_context_provider(voice::VoiceInfo::new);
    use_effect(move || {
        let _ = state.read().voice_changed;
        voice.refresh();
    });
    // The departments, read at start and whenever they change.
    use_effect(move || {
        let _ = state.read().departments_changed;
        departments::refresh(depts);
    });
    // The Inbox's list, read at start and again whenever the server says it changed.
    use_effect(move || {
        let _ = state.read().inbox_changed;
        inbox.refresh();
    });
    // The Inbox and a bot's panel share the right side: opening one closes the other.
    use_effect(move || {
        if (ui.settings_open)() {
            let mut open = inbox.open;
            if *open.peek() {
                open.set(false);
            }
        }
    });
    // The chat in front of the person is read: tell Catbot, once per answer.
    use_effect(move || {
        let Some(id) = selected() else { return };
        let bots = state.read();
        let Some(bot) = bots.bots.iter().find(|b| b.id == id) else { return };
        let unread = if bot.channel.is_some() { bot.unread } else { bot.open_chat().is_some_and(|c| c.unread) };
        if !unread {
            return;
        }
        let (bot_id, thread) = (bot.id.clone(), bot.thread_id.clone());
        drop(bots);
        spawn(async move {
            let _ = api::post(&format!("/api/bots/{bot_id}/read"), &serde_json::json!({ "threadId": thread })).await;
        });
    });
    // On Linux the window's web view keeps the microphone off; voice needs it (stage 22.4).
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    use_hook(voice::let_the_window_listen);
    // Closing the window lets the desktop pets out, if any bot is one.
    #[cfg(feature = "desktop")]
    {
        let main_window = use_hook(|| dioxus::desktop::window().id());
        dioxus::desktop::use_wry_event_handler(move |event, _| {
            if let dioxus::desktop::tao::event::Event::WindowEvent { window_id, event: dioxus::desktop::WindowEvent::CloseRequested, .. } = event
                && *window_id == main_window
                && state.peek().bots.iter().any(|b| b.pet)
            {
                pet_window::come_out();
            }
        });
    }
    use_future(move || notify::follow_clicks(state, ui));
    // Talking to home through the link, and home has removed this device.
    let unpaired = move || place == link::Place::Home && link::current().is_none();
    let mut problem = use_signal(|| None::<String>);
    // Whether the bot list has come at least once (so the welcome doesn't flash up first).
    let mut listed = use_signal(|| false);

    // Fetches the bot list, and has every open conversation fetched again: at start, and after
    // the event stream reconnects (anything could have happened in between).
    let catch_up = move || {
        spawn(async move {
            match api::get("/api/bots?messages=20").await {
                Ok(answer) => {
                    let list = Bot::list(&answer);
                    let current = selected.peek().clone();
                    if current.as_ref().is_none_or(|id| !list.iter().any(|b| &b.id == id)) {
                        selected.set(list.first().map(|b| b.id.clone()));
                    }
                    let mut state = state.write();
                    state.bots = list;
                    for thread in state.threads.values_mut() {
                        thread.loaded = false;
                        thread.live = None;
                        // What was set aside may be missing what came while disconnected: the fetch brings it.
                        thread.since.clear();
                    }
                    problem.set(None);
                    listed.set(true);
                }
                Err(_) if unpaired() => on_unpaired.call(()),
                Err(err) => problem.set(Some(err)),
            }
        });
    };

    // The event stream, for as long as the app is open.
    use_future(move || async move {
        let mut connected_before = false;
        loop {
            events::follow(|frame| {
                let after = state.write().apply(&frame);
                notify::on_frame(&frame, *notify_prefs.peek(), state, ui);
                update::on_frame(&frame, place == link::Place::ThisPc && crate::link::active().is_none());
                if after == live::After::Resync {
                    if connected_before {
                        catch_up();
                    }
                    connected_before = true;
                }
            })
            .await;
            if unpaired() {
                on_unpaired.call(());
                return;
            }
            // The stream ended (Catbot restarted, or the network dropped): try again shortly.
            events::pause(2_000).await;
        }
    });
    use_hook(catch_up);

    let bots = use_memo(move || state.read().bots.clone());
    let update_changed = use_memo(move || state.read().update_changed);
    // The open bot is gone (deleted here or elsewhere): open the first one left. The settings
    // panel can't do this itself, as it closes with its bot.
    use_effect(move || {
        let bots = bots.read();
        let current = selected.read().clone();
        if current.as_ref().is_none_or(|id| !bots.iter().any(|b| &b.id == id)) && !bots.is_empty() {
            selected.set(bots.first().map(|b| b.id.clone()));
        }
    });
    // A new Catbot with no bots opens on the welcome (stage 29.3) until it's done or skipped.
    let mut skipped_welcome = use_context_provider(|| welcome::Skipped(Signal::new(false))).0;
    let welcome = listed() && bots.read().is_empty() && !skipped_welcome() && !unpaired();
    if welcome {
        return rsx! {
            div { class: "flex h-screen flex-col bg-app text-ink",
                update::UpdateBanner { changed: update_changed }
                welcome::Welcome { on_skip: move |_| skipped_welcome.set(true) }
            }
        };
    }
    rsx! {
        div { class: "flex h-screen flex-col bg-app text-ink",
            update::UpdateBanner { changed: update_changed }
            div { class: "relative flex min-h-0 flex-1",
                Sidebar { bots, selected }
                Main { bots, selected, problem }
                if (inbox.open)() {
                    inbox::InboxPanel {}
                }
            }
            attach::PictureViewer {}
        }
    }
}

/// The chat area: the open bot's conversation, and its settings beside it when they're open.
#[component]
fn Main(bots: ReadSignal<Vec<Bot>>, selected: ReadSignal<Option<String>>, problem: ReadSignal<Option<String>>) -> Element {
    let ui = use_context::<settings::Ui>();
    let open = selected.read().as_ref().and_then(|id| bots.read().iter().find(|b| &b.id == id).cloned());
    // On a phone, the chat shows only once a bot has been opened from the list.
    let phone = if (ui.showing_chat)() { "" } else { "max-md:hidden" };
    rsx! {
        div { class: "relative flex h-full min-w-0 flex-1 {phone}",
            if (ui.board)() {
                board::Board {}
            } else if (ui.map)() {
                teammap::TeamMap {}
            } else {
                MainView { open, problem: problem() }
            }
        }
    }
}

#[component]
fn MainView(open: Option<Bot>, problem: Option<String>) -> Element {
    let ui = use_context::<settings::Ui>();
    let inbox = use_context::<inbox::Inbox>();
    let mut skipped = use_context::<welcome::Skipped>().0;
    let no_bots = use_context::<Signal<live::State>>().read().bots.is_empty();
    match (problem, open) {
        (Some(err), _) => rsx! {
            main { class: "relative flex h-full min-w-0 flex-1 flex-col items-center justify-center bg-app",
                p { role: "alert", class: "text-[13px] text-danger", "Can't reach Catbot: {err}" }
            }
        },
        // A one-item keyed list, so switching bots starts the conversation view fresh:
        // Dioxus only honours a key among siblings, and a reused view would keep the last bot's messages.
        (None, Some(bot)) => rsx! {
            for bot in std::iter::once(bot) {
                // Keyed by bot and chat, so opening another chat starts the view fresh too.
                chat::Chat { key: "{bot.id}-{bot.thread_id}", bot: bot.in_open_chat() }
                if (ui.settings_open)() && !(inbox.open)() {
                    if bot.channel.is_some() {
                        settings::SettingsPanel { key: "settings-{bot.id}", bot }
                    } else {
                        panel::BotPanel { key: "panel-{bot.id}", bot }
                    }
                }
            }
        },
        (None, None) => rsx! {
            main { class: "relative flex h-full min-w-0 flex-1 flex-col items-center justify-center gap-2 bg-app text-ink-secondary",
                p { class: "text-[15px] font-semibold text-ink", "No bots yet" }
                p { class: "text-[13px]", "Make one with the + at the top of the sidebar." }
                if no_bots {
                    button { r#type: "button", "data-open-welcome": true, class: "mt-2 rounded-full bg-control px-3 py-1 text-[13px] font-medium text-ink hover:bg-raised", onclick: move |_| skipped.set(false), "Set up Catbot" }
                }
            }
        },
    }
}
