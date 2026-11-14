//! Catbot's interface: the chat app, rebuilt in Dioxus with OpenMausBot's look.
//!
//! Layout and class names follow OpenMausBot's `src/App.tsx` and `src/components/Sidebar.tsx`
//! (Apache-2.0, see NOTICE), so the stylesheet's tokens and skins apply unchanged.

mod api;

use dioxus::prelude::*;

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
    rsx! {
        document::Stylesheet { href: "{FONTS}/fonts.css" }
        document::Stylesheet { href: STYLES }
        div { class: "flex h-screen flex-col bg-app text-ink",
            div { class: "relative flex min-h-0 flex-1",
                Sidebar {}
                Main {}
            }
        }
    }
}

#[component]
fn Sidebar() -> Element {
    rsx! {
        aside {
            "aria-label": "Bots",
            class: "relative flex h-full w-[320px] shrink-0 flex-col border-r-[0.5px] border-edge bg-panel",
            div { class: "flex items-center gap-2 px-4 pb-2 pt-4",
                span { class: "text-[15px] font-semibold text-ink", "Catbot" }
            }
        }
    }
}

/// What the app knows about the server it came from.
#[derive(Clone, PartialEq)]
enum Connection {
    Reached { catbot_bots: usize },
    Failed(String),
}

#[component]
fn Main() -> Element {
    let connection = use_resource(|| async move {
        match api::get("/api/bots?messages=0").await {
            Ok(list) => {
                let bots = list["bots"].as_array().cloned().unwrap_or_default();
                Connection::Reached { catbot_bots: bots.iter().filter(|b| b["catbot"] == true).count() }
            }
            Err(err) => Connection::Failed(err),
        }
    });
    let status = match &*connection.read() {
        None => "Connecting to Catbot…".to_string(),
        Some(Connection::Reached { catbot_bots }) => format!("Connected to Catbot · {catbot_bots} bot(s)"),
        Some(Connection::Failed(err)) => format!("Can't reach Catbot: {err}"),
    };
    rsx! {
        main {
            class: "relative flex h-full min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-app text-ink-secondary",
            p { class: "text-[13px]", "{status}" }
        }
    }
}
