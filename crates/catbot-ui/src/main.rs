//! Catbot's interface: the chat app, rebuilt in Dioxus with OpenMausBot's look.
//!
//! Layout and class names follow OpenMausBot's `src/App.tsx` and `src/components/Sidebar.tsx`
//! (Apache-2.0, see NOTICE), so the stylesheet's tokens and skins apply unchanged.

mod api;
mod bots;
mod chat;
mod events;
mod live;
mod mascots;
mod markdown;
mod settings;
mod sidebar;

use dioxus::prelude::*;

use bots::Bot;
use sidebar::Sidebar;

const STYLES: Asset = asset!("/assets/tailwind.css");
/// The bundled fonts, copied through as a folder so `fonts.css` finds its files beside it.
const FONTS: Asset = asset!("/assets/fonts", AssetOptions::folder());

fn main() {
    dioxus::launch(App);
}

/// Follows the system's light or dark setting, live, as OpenMausBot does by default:
/// Daylight when light, Midnight when dark (`src/lib/skins.ts`).
const FOLLOW_SYSTEM_SKIN: &str = r#"
    const dark = matchMedia("(prefers-color-scheme: dark)");
    const apply = () => { document.documentElement.dataset.skin = dark.matches ? "midnight" : "daylight"; };
    apply();
    dark.addEventListener("change", apply);
"#;

#[component]
fn App() -> Element {
    use_hook(|| {
        document::eval(FOLLOW_SYSTEM_SKIN);
    });
    let mut state = use_context_provider(|| Signal::new(live::State::default()));
    let mut selected = use_signal(|| None::<String>);
    let settings_open = use_signal(|| false);
    use_context_provider(|| settings::Ui { selected, settings_open });
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
                Err(err) => problem.set(Some(err)),
            }
        });
    };

    // The event stream, for as long as the app is open.
    use_future(move || async move {
        let mut connected_before = false;
        loop {
            events::follow(|frame| {
                if state.write().apply(&frame) == live::After::Resync {
                    if connected_before {
                        catch_up();
                    }
                    connected_before = true;
                }
            })
            .await;
            // The stream ended (Catbot restarted, or the network dropped): try again shortly.
            gloo_timers::future::TimeoutFuture::new(2_000).await;
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
        document::Stylesheet { href: "{FONTS}/fonts.css" }
        document::Stylesheet { href: STYLES }
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
    match (problem(), open) {
        (Some(err), _) => rsx! {
            main { class: "relative flex h-full min-w-0 flex-1 flex-col items-center justify-center bg-app",
                p { role: "alert", class: "text-[13px] text-danger", "Can't reach Catbot: {err}" }
            }
        },
        // A one-item keyed list, so switching bots starts the conversation view fresh:
        // Dioxus only honours a key among siblings, and a reused view would keep the last bot's messages.
        (None, Some(bot)) => rsx! {
            for bot in std::iter::once(bot) {
                chat::Chat { key: "{bot.id}", bot: bot.clone() }
                if (ui.settings_open)() {
                    settings::SettingsPanel { key: "settings-{bot.id}", bot }
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
