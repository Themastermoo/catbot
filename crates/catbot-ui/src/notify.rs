//! Desktop notifications, in the Linux and Windows apps only (Moo's choice): a bot finished a
//! turn, or a bot is asking for approval. Each can be switched on or off in Settings →
//! Notifications (kept on this device). None is shown for the chat already open in front of
//! the person. Clicking one opens its chat (on Linux; Windows only shows them).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::live::State;
use crate::settings::Ui;

/// Which notifications this device shows.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    /// When a bot finishes a turn.
    pub finished: bool,
    /// When a bot asks for approval.
    pub asking: bool,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { finished: true, asking: true }
    }
}

/// Whether this app can show notifications at all.
pub const AVAILABLE: bool = cfg!(all(feature = "desktop", not(target_os = "android")));

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Finished,
    Asking,
}

/// The notification a live frame could call for, and its chat.
pub fn wanted(frame: &Value) -> Option<(Kind, String)> {
    if frame["kind"] != "runtime" {
        return None;
    }
    let event = &frame["event"];
    let thread = event["threadId"].as_str()?.to_string();
    match event["type"].as_str()? {
        // Stopped by the person: they know.
        "turn.completed" if event["stopReason"] != "interrupted" => Some((Kind::Finished, thread)),
        "request.opened" => Some((Kind::Asking, thread)),
        _ => None,
    }
}

/// What a notification says: its title and body, from the frame and what's known of the bot.
pub fn words(kind: Kind, frame: &Value, bot_name: &str, chat: Option<&str>, last: &str) -> (String, String) {
    let event = &frame["event"];
    let place = chat.filter(|c| !c.trim().is_empty()).map(|c| format!(" · {c}")).unwrap_or_default();
    match kind {
        Kind::Asking => (format!("{bot_name} is asking{place}"), format!("Approve: {}", event["summary"].as_str().or(event["tool"].as_str()).unwrap_or("a step"))),
        Kind::Finished if event["ok"] == false => (format!("{bot_name} ran into a problem{place}"), short(last)),
        Kind::Finished => (format!("{bot_name} finished{place}"), short(last)),
    }
}

fn short(text: &str) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = line.chars().take(160).collect();
    if line.chars().count() > 160 { format!("{cut}…") } else { cut }
}

/// Shows a notification for a frame, if it's wanted, allowed and not about what's in front of
/// the person.
pub fn on_frame(frame: &Value, prefs: Prefs, state: Signal<State>, ui: Ui) {
    let Some((kind, thread)) = wanted(frame) else { return };
    let allowed = match kind {
        Kind::Finished => prefs.finished,
        Kind::Asking => prefs.asking,
    };
    if !AVAILABLE || !allowed {
        return;
    }
    let state = state.peek();
    let Some(bot) = state.bots.iter().find(|b| b.has_chat(&thread)) else { return };
    let open_here = ui.selected.peek().as_deref() == Some(bot.id.as_str()) && bot.thread_id == thread;
    if open_here && window_in_front() {
        return;
    }
    let chat = bot.chats.iter().find(|c| c.id == thread).map(|c| c.title.as_str());
    let (title, body) = words(kind, frame, &bot.name, chat, &bot.preview());
    show(&title, &body, bot.id.clone(), thread);
}

#[cfg(all(feature = "desktop", not(target_os = "android")))]
fn window_in_front() -> bool {
    dioxus::desktop::window().window.is_focused()
}

#[cfg(not(all(feature = "desktop", not(target_os = "android"))))]
fn window_in_front() -> bool {
    true
}

/// Chats whose notification was clicked: (bot, chat).
#[cfg(all(feature = "desktop", not(target_os = "android")))]
static CLICKED: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

#[cfg(all(feature = "desktop", not(target_os = "android")))]
fn show(title: &str, body: &str, bot: String, thread: String) {
    let mut note = notify_rust::Notification::new();
    note.summary(title).body(body).appname("Catbot").icon("catbot").action("default", "Open");
    // Shown, then waited on in a thread of its own (the wait blocks until it's clicked or closed).
    std::thread::spawn(move || {
        let Ok(handle) = note.show() else { return };
        handle.wait_for_action(|action| {
            if action != "__closed"
                && let Ok(mut clicked) = CLICKED.lock()
            {
                clicked.push((bot, thread));
            }
        });
    });
}

#[cfg(not(all(feature = "desktop", not(target_os = "android"))))]
fn show(_title: &str, _body: &str, _bot: String, _thread: String) {}

/// Opens the chats of clicked notifications, and brings the window forward: for as long as the
/// app runs.
pub async fn follow_clicks(state: Signal<State>, ui: Ui) {
    #[cfg(all(feature = "desktop", not(target_os = "android")))]
    loop {
        crate::events::pause(300).await;
        let clicked: Vec<(String, String)> = CLICKED.lock().map(|mut c| c.drain(..).collect()).unwrap_or_default();
        if let Some((bot, thread)) = clicked.into_iter().last() {
            crate::chats::open(state, ui, &bot, &thread);
            let window = dioxus::desktop::window();
            window.window.set_minimized(false);
            window.window.set_visible(true);
            window.window.set_focus();
        }
    }
    #[cfg(not(all(feature = "desktop", not(target_os = "android"))))]
    let _ = (state, ui);
}

/// This device's choices.
pub fn load() -> Prefs {
    file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(prefs: Prefs) {
    let Some(file) = file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(saved) = serde_json::to_string(&prefs) {
        let _ = std::fs::write(file, saved);
    }
}

/// Where they're kept (beside the drafts); None where there are no notifications.
fn file() -> Option<std::path::PathBuf> {
    if !AVAILABLE {
        return None;
    }
    if let Some(file) = std::env::var_os("CATBOT_PREFS_FILE") {
        return Some(file.into());
    }
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA").map(|dir| std::path::PathBuf::from(dir).join("Catbot").join("app-prefs.json"));
    #[cfg(not(windows))]
    {
        use std::path::PathBuf;
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))?;
        Some(data.join("catbot-app").join("prefs.json"))
    }
}

/// Settings → Notifications.
#[component]
pub fn SettingsDialog(on_close: EventHandler<()>) -> Element {
    let mut prefs = use_context::<Signal<Prefs>>();
    let mut change = move |update: fn(&mut Prefs)| {
        update(&mut prefs.write());
        save(*prefs.peek());
    };
    let current = prefs();
    let mut page = use_signal(|| "general");
    let tab = |on: bool| if on { "rounded-lg bg-raised px-3 py-1.5 text-[13px] font-semibold text-ink" } else { "rounded-lg px-3 py-1.5 text-[13px] text-ink-secondary hover:bg-raised/60 hover:text-ink" };
    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4", onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                "aria-label": "Settings",
                "data-app-settings": true,
                class: "flex max-h-[88vh] w-full max-w-[620px] flex-col gap-4 overflow-y-auto rounded-2xl bg-panel p-5 shadow-2xl",
                onclick: move |e| e.stop_propagation(),
                div { class: "flex items-center",
                    span { class: "min-w-0 flex-1 text-[17px] font-semibold text-ink", "Settings" }
                    button {
                        r#type: "button",
                        "aria-label": "Close",
                        class: "flex size-8 items-center justify-center rounded-md text-ink-secondary hover:bg-control hover:text-ink",
                        onclick: move |_| on_close.call(()),
                        "×"
                    }
                }
                div { role: "tablist", class: "flex shrink-0 gap-1 overflow-x-auto whitespace-nowrap", "data-settings-pages": true,
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"general\"}", class: tab(page() == "general"), onclick: move |_| page.set("general"), "General" }
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"chief\"}", "data-settings-chief": true, class: tab(page() == "chief"), onclick: move |_| page.set("chief"), "Chief of Staff" }
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"team\"}", "data-settings-team": true, class: tab(page() == "team"), onclick: move |_| page.set("team"), "Team" }
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"skills\"}", "data-settings-skills": true, class: tab(page() == "skills"), onclick: move |_| page.set("skills"), "Skills" }
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"email\"}", "data-settings-email": true, class: tab(page() == "email"), onclick: move |_| page.set("email"), "Email" }
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"pages\"}", "data-settings-pages-list": true, class: tab(page() == "pages"), onclick: move |_| page.set("pages"), "Pages" }
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"apps\"}", "data-settings-apps": true, class: tab(page() == "apps"), onclick: move |_| page.set("apps"), "Apps" }
                    button { r#type: "button", role: "tab", "aria-selected": "{page() == \"computers\"}", "data-settings-computers": true, class: tab(page() == "computers"), onclick: move |_| page.set("computers"), "Computers" }
                }
                if page() == "computers" {
                    crate::computers::ComputersSettings {}
                }
                if page() == "apps" {
                    crate::apps::AppsSettings {}
                }
                if page() == "pages" {
                    crate::pages::PagesList {}
                }
                if page() == "email" {
                    crate::email::EmailSettings {}
                }
                if page() == "skills" {
                    crate::skills::SkillsLibrary {}
                }
                if page() == "team" {
                    crate::teams::TeamFiles {}
                }
                if page() == "chief" {
                    crate::chief::ChiefOptions {}
                }
                if page() == "general" && AVAILABLE { section { class: "flex flex-col gap-3", "data-notification-settings": true,
                    h2 { class: "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary", "Notifications" }
                    p { class: "text-[13px] text-ink-secondary", "Shown by this computer while Catbot is open, except for the chat you're looking at." }
                    div { class: "flex items-center gap-3",
                        span { class: "min-w-0 flex-1 text-[14px] text-ink", "When a bot finishes" }
                        crate::settings::Switch {
                            checked: current.finished,
                            disabled: false,
                            label: "Notify when a bot finishes".to_string(),
                            on_toggle: move |_| change(|p| p.finished = !p.finished),
                        }
                    }
                    div { class: "flex items-center gap-3",
                        span { class: "min-w-0 flex-1 text-[14px] text-ink", "When a bot asks for approval" }
                        crate::settings::Switch {
                            checked: current.asking,
                            disabled: false,
                            label: "Notify when a bot asks for approval".to_string(),
                            on_toggle: move |_| change(|p| p.asking = !p.asking),
                        }
                    }
                } }
                if page() == "general" {
                    crate::handoffs::HandoffSettings {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finishing_and_asking_are_noticed_and_worded() {
        let done = json!({ "kind": "runtime", "event": { "type": "turn.completed", "threadId": "t1", "ok": true } });
        assert_eq!(wanted(&done), Some((Kind::Finished, "t1".into())));
        let stopped = json!({ "kind": "runtime", "event": { "type": "turn.completed", "threadId": "t1", "stopReason": "interrupted" } });
        assert_eq!(wanted(&stopped), None);
        let asking = json!({ "kind": "runtime", "event": { "type": "request.opened", "threadId": "t2", "tool": "Bash", "summary": "Bash rm old.txt" } });
        assert_eq!(wanted(&asking), Some((Kind::Asking, "t2".into())));
        assert_eq!(wanted(&json!({ "kind": "message" })), None);

        let (title, body) = words(Kind::Asking, &asking, "Scout", Some("Garden"), "");
        assert_eq!((title.as_str(), body.as_str()), ("Scout is asking · Garden", "Approve: Bash rm old.txt"));
        let (title, body) = words(Kind::Finished, &done, "Scout", None, "The plan\nis   ready");
        assert_eq!((title.as_str(), body.as_str()), ("Scout finished", "The plan is ready"));
    }

    #[test]
    fn both_are_on_until_switched_off() {
        assert_eq!(Prefs::default(), Prefs { finished: true, asking: true });
        let saved: Prefs = serde_json::from_str(r#"{"finished":false,"asking":true}"#).unwrap();
        assert!(!saved.finished && saved.asking);
    }
}
