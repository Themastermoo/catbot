//! Drafts: what's typed in a chat's box (and files attached to it) but not sent yet, kept for
//! each chat so leaving it, or closing Catbot, doesn't lose it.
//!
//! Moo chose to keep drafts on each device only: nothing typed leaves it until it's sent. The
//! browser keeps them in its local storage; the apps in a file of their own (see `file()`).
//! The sidebar shows a chat with a draft as "Draft: …".

use std::collections::BTreeMap;

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::attach::Attached;

/// One chat's draft.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    /// Files already uploaded for it.
    #[serde(default)]
    pub files: Vec<Attached>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.files.is_empty()
    }

    /// The sidebar's line for it: its first words, or the files' names.
    pub fn line(&self) -> String {
        let words: String = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !words.is_empty() {
            return words;
        }
        self.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
    }
}

/// Every chat's draft, by chat (thread) id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Drafts(BTreeMap<String, Draft>);

impl Drafts {
    pub fn get(&self, thread: &str) -> Option<&Draft> {
        self.0.get(thread).filter(|d| !d.is_empty())
    }

    /// Keeps a chat's draft (an empty one is dropped). Whether anything changed.
    pub fn set(&mut self, thread: &str, draft: Draft) -> bool {
        let before = self.0.get(thread).cloned();
        if draft.is_empty() {
            self.0.remove(thread);
        } else {
            self.0.insert(thread.to_string(), draft);
        }
        self.0.get(thread) != before.as_ref()
    }
}

/// The drafts this device kept.
pub fn load() -> Drafts {
    read().and_then(|saved| serde_json::from_str(&saved).ok()).unwrap_or_default()
}

/// Keeps a chat's draft, here and on this device.
pub fn keep(mut drafts: Signal<Drafts>, thread: &str, draft: Draft) {
    if drafts.peek().clone().set(thread, draft.clone())
        && drafts.write().set(thread, draft)
        && let Ok(saved) = serde_json::to_string(&*drafts.peek())
    {
        write(&saved);
    }
}

#[cfg(target_arch = "wasm32")]
const KEY: &str = "catbot-drafts";

#[cfg(target_arch = "wasm32")]
fn read() -> Option<String> {
    web_sys::window()?.local_storage().ok()??.get_item(KEY).ok()?
}

#[cfg(target_arch = "wasm32")]
fn write(saved: &str) {
    // Storage can be off or full (private browsing): then drafts last until the page closes.
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(KEY, saved);
    }
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
    // Written beside and moved into place, so a crash mid-write can't leave half a file.
    let partial = file.with_extension("json.part");
    if std::fs::write(&partial, saved).is_ok() {
        let _ = std::fs::rename(&partial, &file);
    }
}

/// Where the apps keep drafts.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn file() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(file) = std::env::var_os("CATBOT_DRAFTS_FILE") {
        return Some(file.into());
    }
    #[cfg(target_os = "android")]
    return Some(PathBuf::from("/data/data/com.moo.catbot/files/catbot-drafts.json"));
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Catbot").join("app-drafts.json"));
    #[cfg(not(any(target_os = "android", windows)))]
    {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))?;
        Some(data.join("catbot-app").join("drafts.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_drafts_are_dropped_and_lines_read_well() {
        let mut drafts = Drafts::default();
        assert!(drafts.set("t1", Draft { text: "  Plan\nthe   garden ".into(), files: vec![] }));
        assert!(!drafts.set("t1", Draft { text: "  Plan\nthe   garden ".into(), files: vec![] }), "no change");
        assert_eq!(drafts.get("t1").unwrap().line(), "Plan the garden");
        assert!(drafts.set("t1", Draft::default()));
        assert!(drafts.get("t1").is_none());
        let saved = serde_json::to_string(&drafts).unwrap();
        assert_eq!(saved, "{}");
    }

    #[test]
    fn the_apps_keep_drafts_in_a_file() {
        let dir = std::env::temp_dir().join(format!("catbot-drafts-{}", std::process::id()));
        let file = dir.join("drafts.json");
        // SAFETY: this test is the only one that sets it.
        unsafe { std::env::set_var("CATBOT_DRAFTS_FILE", &file) };
        let mut drafts = Drafts::default();
        drafts.set("t1", Draft { text: "half a thought".into(), files: vec![] });
        write(&serde_json::to_string(&drafts).unwrap());
        assert_eq!(load().get("t1").map(|d| d.text.as_str()), Some("half a thought"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
