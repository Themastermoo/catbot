//! Desktop notifications, in the Linux and Windows apps only (Moo's choice): a bot finished a
//! turn, a bot is asking for approval, or a bot sent the person a message (its Inbox line). Each can be switched on or off in Settings →
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
    /// When a bot sends the person a message (it lands in the Inbox).
    #[serde(default = "yes")]
    pub messages: bool,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { finished: true, asking: true, messages: true }
    }
}

/// Whether this app can show notifications at all.
pub const AVAILABLE: bool = cfg!(all(feature = "desktop", not(target_os = "android")));

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Finished,
    Asking,
    Message,
}

/// The notification a live frame could call for, and its chat.
pub fn wanted(frame: &Value) -> Option<(Kind, String)> {
    if frame["kind"] == "inbox" && frame["line"]["kind"] == "message" {
        return Some((Kind::Message, frame["line"]["origin"]["threadId"].as_str()?.to_string()));
    }
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
        Kind::Message => (format!("{bot_name} sent you a message{place}"), short(frame["line"]["text"].as_str().unwrap_or_default())),
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
        Kind::Message => prefs.messages,
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
    let name = frame["line"]["from"]["name"].as_str().unwrap_or(&bot.name).to_string();
    let (title, body) = words(kind, frame, &name, chat, &bot.preview());
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

/// Settings: the grouped window (`prefs.rs`), opening on `start` (a page's key).
#[component]
pub fn SettingsDialog(on_close: EventHandler<()>, #[props(default)] start: String) -> Element {
    rsx! { crate::prefs::SettingsWindow { on_close, start } }
}

/// Settings › This app › Notifications: when this computer tells you about a bot.
#[component]
pub fn NotificationSettings() -> Element {
    let mut prefs = use_context::<Signal<Prefs>>();
    let mut change = move |update: fn(&mut Prefs)| {
        update(&mut prefs.write());
        save(*prefs.peek());
    };
    let current = prefs();
    rsx! {
        div { class: "overflow-hidden rounded-xl bg-composer ring-1 ring-inset ring-ink/[0.07]", "data-notification-settings": true,
            div { class: "flex items-center gap-3 px-4 py-3",
                span { class: "flex min-w-0 flex-1 flex-col",
                    span { class: "text-[14px] leading-5 text-ink", "When a bot finishes" }
                    span { class: "text-[12px] leading-[17px] text-ink-secondary", "Shown while Catbot is open, except for the chat you're looking at." }
                }
                crate::settings::Switch {
                    checked: current.finished,
                    disabled: false,
                    label: "Notify when a bot finishes".to_string(),
                    on_toggle: move |_| change(|p| p.finished = !p.finished),
                }
            }
            div { class: "flex items-center gap-3 border-t border-hairline/50 px-4 py-3",
                span { class: "min-w-0 flex-1 text-[14px] leading-5 text-ink", "When a bot asks for approval" }
                crate::settings::Switch {
                    checked: current.asking,
                    disabled: false,
                    label: "Notify when a bot asks for approval".to_string(),
                    on_toggle: move |_| change(|p| p.asking = !p.asking),
                }
            }
            div { class: "flex items-center gap-3 border-t border-hairline/50 px-4 py-3",
                span { class: "flex min-w-0 flex-1 flex-col",
                    span { class: "text-[14px] leading-5 text-ink", "When a bot sends you a message" }
                    span { class: "text-[12px] leading-[17px] text-ink-secondary", "It's in your Inbox too." }
                }
                crate::settings::Switch {
                    checked: current.messages,
                    disabled: false,
                    label: "Notify when a bot sends you a message".to_string(),
                    on_toggle: move |_| change(|p| p.messages = !p.messages),
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
        let message = json!({ "kind": "inbox", "line": { "kind": "message", "text": "Build is green", "origin": { "threadId": "t3" }, "from": { "name": "Scout" } } });
        assert_eq!(wanted(&message), Some((Kind::Message, "t3".into())));
        assert_eq!(wanted(&json!({ "kind": "inbox" })), None);
        let (title, body) = words(Kind::Message, &message, "Scout", None, "");
        assert_eq!((title.as_str(), body.as_str()), ("Scout sent you a message", "Build is green"));

        let (title, body) = words(Kind::Asking, &asking, "Scout", Some("Garden"), "");
        assert_eq!((title.as_str(), body.as_str()), ("Scout is asking · Garden", "Approve: Bash rm old.txt"));
        let (title, body) = words(Kind::Finished, &done, "Scout", None, "The plan\nis   ready");
        assert_eq!((title.as_str(), body.as_str()), ("Scout finished", "The plan is ready"));
    }

    #[test]
    fn both_are_on_until_switched_off() {
        assert_eq!(Prefs::default(), Prefs { finished: true, asking: true, messages: true });
        let saved: Prefs = serde_json::from_str(r#"{"finished":false,"asking":true}"#).unwrap();
        assert!(!saved.finished && saved.asking && saved.messages, "messages are on for those saved before");
    }
}
