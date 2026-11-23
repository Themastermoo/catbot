//! How Catbot looks on this device (stage 19.2): its theme, the sidebar's colour, and whether
//! the Chief of Staff shows in the sidebar. Kept on the device, like drafts: in the browser's
//! storage, or in a file beside the apps' drafts.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Looks {
    /// "system" (light or dark with the system), or one of the stylesheet's skins.
    pub skin: String,
    /// The sidebar's tint: "" for none, or one of `TINTS`' names.
    pub tint: String,
    /// Whether the Chief of Staff shows in the sidebar (hidden, it keeps working and search
    /// still finds it).
    pub show_chief: bool,
    /// Whether your own messages wear the bot's colour (their default), or stay grey.
    pub color_mine: bool,
    /// The sidebar's folded departments, by name ("General" too).
    pub folded: Vec<String>,
}

impl Default for Looks {
    fn default() -> Self {
        Looks { skin: "system".into(), tint: String::new(), show_chief: true, color_mine: true, folded: Vec::new() }
    }
}

/// The themes the stylesheet has, with their names: light ones, then dark ones.
pub const SKINS: [(&str, &str, bool); 13] = [
    ("daylight", "Daylight", false),
    ("linen", "Linen", false),
    ("meadow", "Meadow", false),
    ("lagoon", "Lagoon", false),
    ("atelier", "Atelier", false),
    ("pop", "Pop", false),
    ("midnight", "Midnight", true),
    ("graphite", "Graphite", true),
    ("dusk", "Dusk", true),
    ("aurora", "Aurora", true),
    ("foundry", "Foundry", true),
    ("diwali", "Diwali", true),
    ("halloween", "Halloween", true),
];

/// The sidebar's colours (MausBot's six: none, then these five).
pub const TINTS: [(&str, &str); 5] = [("blue", "#2B7BE5"), ("teal", "#0F8A7E"), ("olive", "#8B6D1F"), ("red", "#D63B3B"), ("purple", "#7A55D6")];

pub fn tint_color(name: &str) -> Option<&'static str> {
    TINTS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// Applies the theme: following the system's light or dark, live, or one skin.
pub fn apply_skin(skin: &str) {
    let skin = serde_json::to_string(skin).unwrap_or_else(|_| "\"system\"".into());
    document::eval(&format!(
        r#"
        const dark = matchMedia("(prefers-color-scheme: dark)");
        const skin = {skin};
        if (window.__catbotSkin) dark.removeEventListener("change", window.__catbotSkin);
        window.__catbotSkin = () => {{ document.documentElement.dataset.skin = skin === "system" ? (dark.matches ? "midnight" : "daylight") : skin; }};
        window.__catbotSkin();
        if (skin === "system") dark.addEventListener("change", window.__catbotSkin);
        "#
    ));
}

/// The looks this device kept.
pub fn load() -> Looks {
    read().and_then(|saved| serde_json::from_str(&saved).ok()).unwrap_or_default()
}

/// Changes the looks, and keeps them on this device.
pub fn change(mut looks: Signal<Looks>, update: impl FnOnce(&mut Looks)) {
    update(&mut looks.write());
    let now = looks.peek().clone();
    apply_skin(&now.skin);
    if let Ok(saved) = serde_json::to_string(&now) {
        write(&saved);
    }
}

#[cfg(target_arch = "wasm32")]
const KEY: &str = "catbot-looks";

#[cfg(target_arch = "wasm32")]
fn read() -> Option<String> {
    web_sys::window()?.local_storage().ok()??.get_item(KEY).ok()?
}

#[cfg(target_arch = "wasm32")]
fn write(saved: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(KEY, saved);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn file() -> Option<std::path::PathBuf> {
    Some(crate::drafts::file()?.with_file_name("app-looks.json"))
}

#[cfg(not(target_arch = "wasm32"))]
fn read() -> Option<String> {
    std::fs::read_to_string(file()?).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn write(saved: &str) {
    let Some(file) = file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, saved);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_keep_their_defaults_and_names() {
        let saved: Looks = serde_json::from_str(r#"{"tint":"teal"}"#).unwrap();
        assert_eq!((saved.skin.as_str(), saved.tint.as_str(), saved.show_chief), ("system", "teal", true));
        assert_eq!(tint_color("teal"), Some("#0F8A7E"));
        assert_eq!(tint_color(""), None);
        assert!(SKINS.iter().any(|(id, _, dark)| *id == "midnight" && *dark));
    }
}
