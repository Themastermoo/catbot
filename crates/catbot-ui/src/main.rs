//! Catbot's interface: the chat app, rebuilt in Dioxus with OpenMausBot's look.
//!
//! Layout and class names follow OpenMausBot's `src/App.tsx` and `src/components/Sidebar.tsx`
//! (Apache-2.0, see NOTICE), so the stylesheet's tokens and skins apply unchanged.

mod api;
mod bots;
mod sidebar;

use dioxus::prelude::*;

use bots::Bot;
use sidebar::{BotMark, Sidebar};

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
    let mut bots = use_signal(Vec::<Bot>::new);
    let mut selected = use_signal(|| None::<String>);
    let mut problem = use_signal(|| None::<String>);
    // The bot list, once. Live updates come with the event stream (stage 6.4).
    use_future(move || async move {
        match api::get("/api/bots?messages=20").await {
            Ok(answer) => {
                let list = Bot::list(&answer);
                if selected.peek().is_none() {
                    selected.set(list.first().map(|b| b.id.clone()));
                }
                bots.set(list);
            }
            Err(err) => problem.set(Some(err)),
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

/// The chat area. Until stage 6.3 it shows which bot is open.
#[component]
fn Main(bots: ReadSignal<Vec<Bot>>, selected: ReadSignal<Option<String>>, problem: ReadSignal<Option<String>>) -> Element {
    let open = selected.read().as_ref().and_then(|id| bots.read().iter().find(|b| &b.id == id).cloned());
    rsx! {
        main {
            class: "relative flex h-full min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-app text-ink-secondary",
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[13px] text-danger", "Can't reach Catbot: {err}" }
            } else if let Some(bot) = open {
                BotMark { color: bot.color.clone(), initial: bot.initial(), size: 64 }
                p { class: "text-[15px] font-semibold text-ink", "{bot.name}" }
                p { class: "text-[13px]", "The conversation appears here in the next stage." }
            }
        }
    }
}
