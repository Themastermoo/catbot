//! Updates in the interface (stage 28.4): the banner that asks before installing, and Settings ›
//! This app's versions with Go back.
//!
//! Moo's choices (2026-11-25): one click installs (Catbot downloads, waits for the bots to
//! finish, restarts); going back keeps today's chats unless the older version can't read them,
//! and then the copy from before the update is offered, never taken without asking.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;

/// `GET /api/version` from this device's own Catbot.
async fn read() -> Option<Value> {
    match api::own("GET", "/api/version", None).await {
        Ok((200, answer)) => Some(answer),
        _ => None,
    }
}

/// What the banner shows, worked out from `GET /api/version` (or an `update` frame's state).
#[derive(Clone, Debug, PartialEq)]
pub enum Show {
    Nothing,
    /// A newer release this device can install: `(version, notes)`.
    Offer(String, Option<String>),
    /// Downloaded already, not installed.
    Ready(String),
    /// Downloading: `(version, percent)`.
    Downloading(String, u8),
    /// Installs once the bots finish.
    Waiting(String),
    /// Installed; Catbot restarts into it.
    Installed(String),
    Problem(String),
    /// A new version didn't start and the launcher went back: `(version, why)`.
    Failed(String, String),
    /// Just updated: `(from, to)`.
    Updated(String, String),
}

impl Show {
    /// What closing it remembers, so it doesn't come back.
    pub fn key(&self) -> Option<String> {
        match self {
            Show::Offer(v, _) => Some(format!("offer:{v}")),
            Show::Failed(v, _) => Some(format!("failed:{v}")),
            Show::Updated(_, to) => Some(format!("updated:{to}")),
            Show::Problem(p) => Some(format!("problem:{}", p.chars().take(40).collect::<String>())),
            _ => None,
        }
    }
}

pub fn show(info: &Value) -> Show {
    let update = &info["update"];
    let dismissed = |key: Option<String>| key.is_some_and(|k| update["dismissed"].as_array().is_some_and(|d| d.iter().any(|w| *w == json!(k))));
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    if let Some(d) = update["downloading"].as_object() {
        let (got, of) = (d["got"].as_f64().unwrap_or(0.0), d["of"].as_f64().unwrap_or(0.0));
        let pct = if of > 0.0 { (got / of * 100.0).clamp(0.0, 100.0) as u8 } else { 0 };
        return Show::Downloading(text(&d["version"]), pct);
    }
    let ready = text(&update["ready"]["version"]);
    if update["waiting"] == true {
        return Show::Waiting(ready);
    }
    if let Some(to) = update["installing"]["to"].as_str().filter(|to| *to != text(&info["version"])) {
        return Show::Installed(to.to_string());
    }
    let candidates = [
        update["error"].as_str().map(|e| Show::Problem(e.to_string())),
        update["failed"]["version"].as_str().map(|v| Show::Failed(v.to_string(), text(&update["failed"]["why"]))),
        (!ready.is_empty()).then(|| Show::Ready(ready.clone())),
        (info["checked"]["newer"] == true && info["checked"]["offer"]["canInstall"] == true)
            .then(|| Show::Offer(text(&info["checked"]["offer"]["version"]), info["checked"]["offer"]["notes"].as_str().map(str::to_string))),
        update["updated"]["to"].as_str().filter(|to| *to == text(&info["version"])).map(|to| Show::Updated(text(&update["updated"]["from"]), to.to_string())),
    ];
    candidates.into_iter().flatten().find(|s| !dismissed(s.key())).unwrap_or(Show::Nothing)
}

/// An `update` frame. On the Windows app's own Catbot, `restart` closes the app with code 75 so
/// the launcher starts the version now current.
pub fn on_frame(frame: &Value, own: bool) {
    if frame["kind"] != "update" || !own {
        return;
    }
    if frame["restart"] == true {
        restart();
    }
    if let Some(apk) = frame["openInstaller"].as_str() {
        open_installer(apk);
    }
}

/// The banner saw an install finish on this device's own Catbot (its frames may not reach this
/// window when it's showing home's).
fn act_on_installed(info: &Value) {
    match info["update"]["platform"].as_str() {
        Some("windows") => restart(),
        Some("android") => {
            if let Some(apk) = info["update"]["ready"]["apk"].as_str() {
                open_installer(apk);
            }
        }
        _ => {}
    }
}

/// The Windows app closes with code 75; the launcher starts the version now current.
fn restart() {
    #[cfg(all(feature = "desktop", windows))]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            spawn(async {
                // A moment for the banner to say so.
                crate::events::pause(800).await;
                std::process::exit(75);
            });
        });
    }
}

/// Hands the downloaded APK to Android's installer, once per file.
fn open_installer(apk: &str) {
    #[cfg(target_os = "android")]
    {
        static OPENED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
        if opened.as_deref() == Some(apk) {
            return;
        }
        *opened = Some(apk.to_string());
        if let Err(err) = crate::android::install_apk(apk) {
            eprintln!("catbot: couldn't open Android's installer: {err}");
            *opened = None;
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = apk;
}

/// The banner across the top of the window. It reads `GET /api/version` at start, and again on
/// each `update` frame (`changed` counts them).
#[component]
pub fn UpdateBanner(changed: ReadSignal<u64>) -> Element {
    let mut info = use_signal(|| Value::Null);
    let mut notes_open = use_signal(|| false);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        let _ = changed();
        spawn(async move {
            if let Some(answer) = read().await {
                info.set(answer);
            }
        });
    });
    // This device's own Catbot may not be the one whose frames arrive (the phone or the Windows
    // app showing home's), so it's also asked now and then: often while something's under way.
    use_future(move || async move {
        loop {
            let active = matches!(show(&info.peek()), Show::Downloading(..) | Show::Waiting(_) | Show::Installed(_));
            crate::events::pause(if active { 3_000 } else { 120_000 }).await;
            if let Some(answer) = read().await {
                info.set(answer);
            }
        }
    });
    let shown = show(&info());
    // Installed: the app restarts into it (Windows), or Android's installer opens (the phone).
    use_effect(move || {
        if let Show::Installed(_) = show(&info()) {
            act_on_installed(&info.peek());
        }
    });
    if shown == Show::Nothing {
        return rsx! {};
    }
    let close = {
        let key = shown.key();
        move |_| {
            let key = key.clone();
            spawn(async move {
                if let Some(key) = key
                    && let Ok((200, answer)) = api::own("POST", "/api/version/dismiss", Some(&json!({ "what": key }))).await
                {
                    info.write()["update"] = answer;
                }
            });
        }
    };
    let mut post = move |path: &'static str, body: Value| {
        busy.set(true);
        spawn(async move {
            let _ = api::own("POST", path, Some(&body)).await;
            if let Some(answer) = read().await {
                info.set(answer);
            }
            busy.set(false);
        });
    };
    let (words, notes) = match &shown {
        Show::Offer(v, notes) => (format!("Catbot {v} is out."), notes.clone()),
        Show::Ready(v) => (format!("Catbot {v} is downloaded and ready to install."), None),
        Show::Downloading(v, pct) => (format!("Getting Catbot {v}… {pct}%"), None),
        Show::Waiting(v) => (format!("Catbot {v} installs as soon as the bots finish what they're doing."), None),
        Show::Installed(v) => (format!("Catbot {v} is installed, and Catbot restarts into it. If it doesn't, close Catbot and open it again."), None),
        Show::Problem(p) => (format!("The update didn't work: {p}"), None),
        Show::Failed(v, why) => (format!("Catbot {v} didn't start ({why}), so this is the version before it again."), None),
        Show::Updated(from, to) => (if from.is_empty() { format!("Updated to Catbot {to}.") } else { format!("Updated from {from} to Catbot {to}.") }, None),
        Show::Nothing => (String::new(), None),
    };
    let tone = if matches!(shown, Show::Problem(_) | Show::Failed(..)) { "bg-red-500/10" } else { "bg-accent/10" };
    let button = "shrink-0 rounded-full bg-accent px-3 py-1 text-[12px] font-medium text-accent-ink hover:brightness-110 disabled:opacity-50";
    let quiet = "shrink-0 rounded-full px-2.5 py-1 text-[12px] font-medium text-ink-secondary hover:bg-control";
    rsx! {
        div { "data-update-banner": true, class: "border-b border-hairline px-4 py-2 text-[13px] {tone}",
            div { class: "flex items-center gap-3",
                span { class: "min-w-0 flex-1 truncate text-ink", "{words}" }
                if notes.is_some() {
                    button { r#type: "button", class: quiet, onclick: move |_| notes_open.toggle(), if notes_open() { "Hide what's new" } else { "What's new" } }
                }
                match &shown {
                    Show::Offer(..) => rsx! {
                        button { r#type: "button", "data-update-now": true, class: button, disabled: busy(), onclick: move |_| post("/api/version/download", json!({ "install": true })), "Update" }
                    },
                    Show::Ready(_) | Show::Problem(_) if !info()["update"]["ready"].is_null() => rsx! {
                        button { r#type: "button", "data-install-now": true, class: button, disabled: busy(), onclick: move |_| post("/api/version/install", json!({})), "Install now" }
                    },
                    Show::Waiting(_) => rsx! {
                        button { r#type: "button", class: quiet, onclick: move |_| post("/api/version/cancel", json!({})), "Not now" }
                    },
                    _ => rsx! {},
                }
                if shown.key().is_some() {
                    button { r#type: "button", "data-update-close": true, class: quiet, "aria-label": "Close", onclick: close,
                        if matches!(shown, Show::Offer(..)) { "Later" } else { "OK" }
                    }
                }
            }
            if notes_open() && let Some(notes) = notes {
                div { class: "mt-2 max-h-40 overflow-y-auto whitespace-pre-wrap text-[12px] text-ink-secondary", "{notes}" }
            }
        }
    }
}

/// Settings › This app: the versions kept on this PC, each with Go back.
#[component]
pub fn Versions() -> Element {
    let mut info = use_signal(|| Value::Null);
    // Going back needs the copy of the chats: what it is, until the person says yes or no.
    let mut asking = use_signal(|| None::<Value>);
    let mut said = use_signal(|| None::<String>);
    use_effect(move || {
        spawn(async move {
            if let Some(answer) = read().await {
                info.set(answer);
            }
        });
    });
    let back = move |to: String, use_snapshot: bool| {
        spawn(async move {
            match api::own("POST", "/api/version/back", Some(&json!({ "to": to, "useSnapshot": use_snapshot }))).await {
                Ok((200, _)) => said.set(Some(format!("Going back to Catbot {to}: it restarts now."))),
                Ok((202, _)) => said.set(Some(format!("Getting Catbot {to}; Android asks you to install it next."))),
                Ok((409, answer)) if answer["needsSnapshot"] == true => asking.set(Some(answer)),
                Ok((_, answer)) => said.set(Some(answer["error"].as_str().or(answer.as_str()).unwrap_or("Going back didn't work.").to_string())),
                Err(err) => said.set(Some(err)),
            }
        });
    };
    let update = info()["update"].clone();
    let mut kept: Vec<Value> = update["kept"].as_array().cloned().unwrap_or_default();
    // The phone keeps one app: it can go back to the rebuilt one the latest release carries.
    if update["platform"] == "android" && update["phoneBack"]["version"].is_string() {
        kept = vec![json!({ "version": info()["version"], "current": true }), json!({ "version": update["phoneBack"]["version"] })];
    }
    if !matches!(update["platform"].as_str(), Some("windows" | "android")) || kept.len() < 2 {
        return rsx! {};
    }
    let title = if update["platform"] == "android" { "Versions for this phone" } else { "Versions on this PC" };
    rsx! {
        div { "data-versions": true, class: "border-t border-hairline/50 px-4 py-3",
            div { class: "text-[13px] font-medium text-ink", "{title}" }
            p { class: "mt-0.5 text-[12px] text-ink-secondary", "Going back keeps your chats. If an older version can't read them, Catbot asks first." }
            for v in kept {
                {
                    let version = v["version"].as_str().unwrap_or_default().to_string();
                    let current = v["current"] == true;
                    let to = version.clone();
                    let here = info()["version"].as_str().unwrap_or_default().to_string();
                    let label = if is_newer(&version, &here) { "Switch to it" } else { "Go back" };
                    rsx! {
                        div { key: "{version}", class: "mt-2 flex items-center gap-2 text-[13px]",
                            span { class: "flex-1 text-ink", "Catbot {version}" }
                            if current {
                                span { class: "text-[12px] text-ink-secondary", "This one" }
                            } else {
                                button { r#type: "button", "data-go-back": "{version}", class: "shrink-0 rounded-full bg-control px-3 py-1 text-[12px] font-medium text-ink hover:bg-raised",
                                    onclick: move |_| back(to.clone(), false), "{label}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(ask) = asking() {
                {
                    let to = ask["to"].as_str().unwrap_or_default().to_string();
                    let at = ask["snapshot"]["at"].as_str().map(|a| crate::chat::short_when(crate::chat::parse_time(a))).unwrap_or_default();
                    let yes = to.clone();
                    rsx! {
                        div { "data-needs-snapshot": true, class: "mt-3 rounded-lg bg-inset px-3 py-2 text-[12px] text-ink",
                            p { "Catbot {to} can't read how your chats are stored now. It can go back with your chats as they were before the update ({at}). Anything since is set aside, not deleted." }
                            div { class: "mt-2 flex gap-2",
                                button { r#type: "button", "data-use-snapshot": true, class: "rounded-full bg-accent px-3 py-1 font-medium text-accent-ink hover:brightness-110",
                                    onclick: move |_| { asking.set(None); back(yes.clone(), true) }, "Go back with that copy"
                                }
                                button { r#type: "button", class: "rounded-full px-3 py-1 font-medium text-ink-secondary hover:bg-control", onclick: move |_| asking.set(None), "Cancel" }
                            }
                        }
                    }
                }
            }
            if let Some(words) = said() {
                p { class: "mt-2 text-[12px] text-ink-secondary", "{words}" }
            }
        }
    }
}

/// `1.2.3` against `1.2.0`, by number.
fn is_newer(theirs: &str, ours: &str) -> bool {
    let n = |v: &str| {
        let mut n: Vec<u64> = v.trim_start_matches('v').split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
        n.resize(3, 0);
        n
    };
    n(theirs) > n(ours)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_says_the_most_pressing_thing() {
        let offer = json!({ "version": "0.2.0", "checked": { "newer": true, "offer": { "version": "0.3.0", "canInstall": true, "notes": "Faster." } }, "update": {} });
        assert_eq!(show(&offer), Show::Offer("0.3.0".into(), Some("Faster.".into())));
        let mut later = offer.clone();
        later["update"]["dismissed"] = json!(["offer:0.3.0"]);
        assert_eq!(show(&later), Show::Nothing, "Later hides that version");
        let mut cant = offer.clone();
        cant["checked"]["offer"]["canInstall"] = json!(false);
        assert_eq!(show(&cant), Show::Nothing, "the home PC is updated by the helper");
        let mut getting = offer.clone();
        getting["update"]["downloading"] = json!({ "version": "0.3.0", "got": 50, "of": 200 });
        assert_eq!(show(&getting), Show::Downloading("0.3.0".into(), 25));
        let mut waiting = offer.clone();
        waiting["update"] = json!({ "waiting": true, "ready": { "version": "0.3.0" } });
        assert_eq!(show(&waiting), Show::Waiting("0.3.0".into()));
        let mut installed = offer.clone();
        installed["update"]["installing"] = json!({ "from": "0.2.0", "to": "0.3.0" });
        assert_eq!(show(&installed), Show::Installed("0.3.0".into()), "not offered again while restarting");
        let failed = json!({ "version": "0.2.0", "update": { "failed": { "version": "0.3.0", "why": "it closed with code 1" } } });
        assert_eq!(show(&failed), Show::Failed("0.3.0".into(), "it closed with code 1".into()));
        let updated = json!({ "version": "0.3.0", "update": { "updated": { "from": "0.2.0", "to": "0.3.0" } } });
        assert_eq!(show(&updated), Show::Updated("0.2.0".into(), "0.3.0".into()));
        let mut seen = updated.clone();
        seen["update"]["dismissed"] = json!(["updated:0.3.0"]);
        assert_eq!(show(&seen), Show::Nothing);
        assert!(is_newer("0.10.0", "0.9.1") && !is_newer("0.2.0", "0.2.0"));
    }
}
