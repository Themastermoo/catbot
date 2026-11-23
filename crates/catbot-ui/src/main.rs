//! Catbot's interface: the chat app, rebuilt in Dioxus with OpenMausBot's look.
//!
//! Layout and class names follow OpenMausBot's `src/App.tsx` and `src/components/Sidebar.tsx`
//! (Apache-2.0, see NOTICE), so the stylesheet's tokens and skins apply unchanged.

mod api;
mod apps;
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
mod pages;
mod pair;
mod events;
mod live;
mod mascots;
mod notify;
mod pictures;
mod markdown;
mod memory;
mod settings;
mod usage;
mod sidebar;
mod looks;
mod prefs;
mod model_menu;
mod panel;

use dioxus::prelude::*;

use bots::Bot;
use sidebar::Sidebar;

const STYLES: Asset = asset!("/assets/tailwind.css");
/// How the bot pictures move (Catbot's own; `tailwind.css` stays as OpenMausBot's).
const MASCOT_STYLES: Asset = asset!("/assets/mascots.css");
/// The bundled fonts, copied through as a folder so `fonts.css` finds its files beside it.
const FONTS: Asset = asset!("/assets/fonts", AssetOptions::folder());
/// Catbot's icon, the visor cat in teal (`assets/icon/`, drawn from `mascots.rs`): the browser
/// tab's, with a small picture for browsers that don't take SVG icons.
const ICON: Asset = asset!("/assets/icon/catbot.svg");
const ICON_SMALL: Asset = asset!("/assets/icon/catbot-32.png");

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
        let window = WindowBuilder::new().with_title("Catbot").with_inner_size(LogicalSize::new(1280.0, 840.0)).with_min_inner_size(LogicalSize::new(720.0, 480.0));
        let mut config = Config::new().with_window(window).with_menu(None);
        // The window's icon, built into the program. (Under Wayland the taskbar takes its icon from
        // the app's menu entry instead, which stage 7.4 installs.)
        match dioxus::desktop::icon_from_memory(include_bytes!("../assets/icon/catbot-256.png")) {
            Ok(icon) => config = config.with_icon(icon),
            Err(err) => eprintln!("catbot: couldn't load the window icon: {err}"),
        }
        // The Windows app starts its own Catbot, which stops when the app closes.
        local::start();
        dioxus::LaunchBuilder::desktop().with_cfg(config).launch(Root);
    }
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
        document::Link { rel: "icon", r#type: "image/svg+xml", href: ICON }
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
    let ui = use_context_provider(|| settings::Ui { selected, settings_open, showing_chat, prefs, find });
    let notify_prefs = use_context_provider(|| Signal::new(notify::load()));
    use_future(move || notify::follow_clicks(state, ui));
    // Talking to home through the link, and home has removed this device.
    let unpaired = move || place == link::Place::Home && link::current().is_none();
    let mut problem = use_signal(|| None::<String>);

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
                    }
                    problem.set(None);
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
    // The open bot is gone (deleted here or elsewhere): open the first one left. The settings
    // panel can't do this itself, as it closes with its bot.
    use_effect(move || {
        let bots = bots.read();
        let current = selected.read().clone();
        if current.as_ref().is_none_or(|id| !bots.iter().any(|b| &b.id == id)) && !bots.is_empty() {
            selected.set(bots.first().map(|b| b.id.clone()));
        }
    });
    rsx! {
        div { class: "flex h-screen flex-col bg-app text-ink",
            div { class: "relative flex min-h-0 flex-1",
                Sidebar { bots, selected }
                Main { bots, selected, problem }
            }
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
            MainView { open, problem: problem() }
        }
    }
}

#[component]
fn MainView(open: Option<Bot>, problem: Option<String>) -> Element {
    let ui = use_context::<settings::Ui>();
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
                if (ui.settings_open)() {
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
            }
        },
    }
}
