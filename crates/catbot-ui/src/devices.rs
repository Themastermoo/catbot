//! Devices away from home: pairing a phone with a QR code, and the list of paired devices.
//! Opened from the phone button at the top of the sidebar. Only the home interface has it; the
//! server keeps device routes off its remote door.

use dioxus::prelude::*;
use qrcode::{QrCode, render::svg};
use serde_json::Value;

use crate::api;

const ICON_BUTTON: &str = "flex size-10 items-center justify-center rounded-md text-ink-secondary hover:bg-control hover:text-ink";

/// What a pairing QR code says: the scheme the phone app answers to, with Catbot's public
/// address and the code.
pub fn pairing_link(url: &str, code: &str) -> String {
    format!("catbot://pair?url={}&code={code}", encode(url))
}

/// Percent-encoding for a query value.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A code shown for typing: "ABCDE-FGHJK".
fn spaced(code: &str) -> String {
    if code.is_ascii() && code.len() == 10 { format!("{}-{}", &code[..5], &code[5..]) } else { code.to_string() }
}

/// The QR code as an SVG picture, dark on light so any camera reads it.
fn qr_svg(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    Some(code.render::<svg::Color<'_>>().min_dimensions(220, 220).quiet_zone(true).dark_color(svg::Color("#111111")).light_color(svg::Color("#ffffff")).build())
}

/// "5 min ago", "Yesterday", or a date, from an RFC 3339 time.
fn seen(at: Option<&str>) -> String {
    let Some(at) = at else { return "Not yet".into() };
    let then = crate::chat::parse_time(at);
    let minutes = ((crate::chat::js_now() - then) / 60_000.0).floor();
    if minutes < 1.0 {
        "Just now".into()
    } else if minutes < 60.0 {
        format!("{minutes:.0} min ago")
    } else {
        crate::chat::short_when(then)
    }
}

#[component]
/// `embedded`: as a page of the Settings window, without its own frame.
pub fn DevicesDialog(on_close: EventHandler<()>, #[props(default)] embedded: bool) -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let mut overview = use_signal(|| None::<Value>);
    let mut problem = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut removing = use_signal(|| None::<String>);
    let mut just_paired = use_signal(|| None::<String>);
    let mut left = use_signal(|| 0u64);
    let mut on_wifi = use_signal(|| false);

    // Fetched when the dialog opens and whenever a `devices` frame says something changed.
    use_effect(move || {
        let _ = (rev.devices)();
        spawn(async move {
            match api::get("/api/devices").await {
                Ok(answer) => {
                    let before = overview.peek().as_ref().map(|o| o["devices"].as_array().map(Vec::len).unwrap_or(0)).unwrap_or(0);
                    let now = answer["devices"].as_array().map(Vec::len).unwrap_or(0);
                    if overview.peek().is_some() && now > before {
                        just_paired.set(answer["devices"][0]["name"].as_str().map(str::to_string));
                    }
                    left.set(answer["pairing"]["secondsLeft"].as_u64().unwrap_or(0));
                    overview.set(Some(answer));
                }
                Err(err) => problem.set(Some(format!("Couldn't load devices: {err}"))),
            }
        });
    });
    // The countdown, a second at a time, while a code waits.
    use_future(move || async move {
        loop {
            crate::events::pause(1000).await;
            let n = *left.peek();
            if n > 0 {
                left.set(n - 1);
            }
        }
    });

    let change = move |method: &'static str, path: String| {
        if busy() {
            return;
        }
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call(method, &path, None).await {
                Ok(answer) => {
                    left.set(answer["pairing"]["secondsLeft"].as_u64().unwrap_or(0));
                    overview.set(Some(answer));
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let (mut start, mut cancel, mut remove) = (change, change, change);

    let data = overview();
    let tailnet = data.as_ref().and_then(|o| o["publicUrl"].as_str().map(str::to_string));
    let lan = data.as_ref().and_then(|o| o["lanUrl"].as_str().map(str::to_string));
    // Paired over the internet unless the person picks this Wi-Fi (or it's the only way).
    let over_wifi = lan.is_some() && (on_wifi() || tailnet.is_none());
    let public = if over_wifi { lan.clone() } else { tailnet.clone() };
    let door = data.as_ref().and_then(|o| o["remote"].as_str().map(str::to_string));
    let code = data.as_ref().and_then(|o| o["pairing"]["code"].as_str().map(str::to_string)).filter(|_| left() > 0);
    let devices: Vec<Value> = data.as_ref().and_then(|o| o["devices"].as_array().cloned()).unwrap_or_default();
    let picture = match (&public, &code) {
        (Some(url), Some(code)) => qr_svg(&pairing_link(url, code)),
        _ => None,
    };
    let countdown = format!("{}:{:02}", left() / 60, left() % 60);
    rsx! {
        div { class: if embedded { "contents" } else { "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" }, onclick: move |_| if !embedded { on_close.call(()) },
            div {
                role: if embedded { "region" } else { "dialog" },
                "aria-label": "Devices",
                "data-devices-dialog": true,
                class: if embedded { "" } else { "max-h-[90vh] w-full max-w-[460px] overflow-y-auto rounded-2xl bg-panel p-5 shadow-2xl" },
                onclick: move |e| e.stop_propagation(),
                div { class: if embedded { "hidden" } else { "mb-3 flex items-center justify-between" },
                    span { class: "text-[17px] font-semibold text-ink", "Devices away from home" }
                    button { r#type: "button", "aria-label": "Close", class: ICON_BUTTON, onclick: move |_| on_close.call(()),
                        svg { "aria-hidden": "true", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round",
                            path { d: "M18 6 6 18M6 6l12 12" }
                        }
                    }
                }
                p { class: "mb-4 text-[13px] leading-[19px] text-ink-secondary",
                    "A paired phone can talk to your bots and answer their questions from anywhere. Remove a device here and its access ends at once."
                }

                if data.is_none() {
                    p { class: "text-[13px] text-ink-secondary", "Loading…" }
                } else if door.is_none() {
                    p { class: "rounded-lg bg-raised p-3 text-[13px] text-ink", "The remote door is off. Catbot was started with --remote off." }
                } else {
                    if let Some(name) = just_paired() {
                        p { role: "status", "data-just-paired": true, class: "mb-3 rounded-lg bg-success/15 p-3 text-[13px] font-medium text-ink", "Paired: {name}" }
                    }
                    if lan.is_some() && tailnet.is_some() {
                        div { role: "radiogroup", "aria-label": "Pair over", "data-pair-over": true, class: "mb-3 flex gap-1 rounded-full bg-raised p-1 text-[13px]",
                            button { r#type: "button", role: "radio", "aria-checked": "{!over_wifi}",
                                class: if over_wifi { "flex-1 rounded-full px-3 py-1 text-ink-secondary" } else { "flex-1 rounded-full bg-panel px-3 py-1 font-medium text-ink shadow-sm" },
                                onclick: move |_| on_wifi.set(false), "Anywhere (Tailscale)" }
                            button { r#type: "button", role: "radio", "aria-checked": "{over_wifi}", "data-pair-wifi": true,
                                class: if over_wifi { "flex-1 rounded-full bg-panel px-3 py-1 font-medium text-ink shadow-sm" } else { "flex-1 rounded-full px-3 py-1 text-ink-secondary" },
                                onclick: move |_| on_wifi.set(true), "This Wi-Fi only" }
                        }
                    }
                    if public.is_none() {
                        div { class: "mb-3 rounded-lg bg-raised p-3 text-[13px] leading-[19px] text-ink",
                            p { class: "font-medium", "Catbot has no public address yet." }
                            p { class: "mt-1 text-ink-secondary",
                                "Run scripts/remote-setup.sh once to give it one with Tailscale Funnel. Until then, a phone can't reach Catbot."
                            }
                        }
                    }
                    if let Some(code) = code.clone() {
                        div { "data-pairing": true, class: "mb-4 flex flex-col items-center gap-2 rounded-xl bg-raised p-4",
                            if let Some(svg) = picture.clone() {
                                div { class: "overflow-hidden rounded-lg", "data-pairing-qr": true, dangerous_inner_html: "{svg}" }
                                p { class: "text-center text-[13px] text-ink-secondary", "In the Catbot app, tap Pair and scan this." }
                            }
                            p { class: "text-center text-[13px] text-ink-secondary", "Or type the code:" }
                            p { "data-pairing-code": true, class: "font-mono text-[22px] font-semibold tracking-[0.12em] text-ink", "{spaced(&code)}" }
                            if let Some(url) = public.clone() {
                                p { class: "break-all text-center text-[12px] text-ink-secondary", "at {url}" }
                            }
                            p { class: "text-[12px] text-ink-secondary", "Works once, for {countdown}." }
                            button {
                                r#type: "button",
                                class: "rounded-full px-3 py-1 text-[13px] font-medium text-ink-secondary hover:bg-ink/[0.07] hover:text-ink",
                                onclick: move |_| cancel("DELETE", "/api/devices/pairing".into()),
                                "Cancel"
                            }
                        }
                    } else {
                        button {
                            r#type: "button",
                            "data-pair-device": true,
                            disabled: busy(),
                            class: "mb-4 w-full rounded-full bg-accent px-4 py-2 text-[14px] font-semibold text-white hover:opacity-90 disabled:opacity-50",
                            onclick: move |_| {
                                just_paired.set(None);
                                start("POST", "/api/devices/pairing".into());
                            },
                            "Pair a phone"
                        }
                    }

                    div { class: "flex flex-col gap-1",
                        span { class: "px-1 text-[12px] font-medium uppercase tracking-wide text-ink-secondary", "Paired" }
                        if devices.is_empty() {
                            p { class: "px-1 py-2 text-[13px] text-ink-secondary", "No devices yet." }
                        }
                        for device in devices {
                            {
                                let id = device["id"].as_str().unwrap_or_default().to_string();
                                let name = device["name"].as_str().unwrap_or("A device").to_string();
                                let last = seen(device["lastSeenAt"].as_str());
                                let paired = crate::chat::short_when(crate::chat::parse_time(device["createdAt"].as_str().unwrap_or_default()));
                                let confirming = removing().as_deref() == Some(id.as_str());
                                let (ask_id, gone_id) = (id.clone(), id.clone());
                                rsx! {
                                    div { key: "{id}", "data-device": "{id}", class: "flex items-center gap-3 rounded-lg px-2 py-2 hover:bg-raised/50",
                                        svg { "aria-hidden": "true", class: "shrink-0 text-ink-secondary", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.75", stroke_linecap: "round", stroke_linejoin: "round",
                                            rect { x: "5", y: "2", width: "14", height: "20", rx: "2" }
                                            path { d: "M12 18h.01" }
                                        }
                                        div { class: "min-w-0 flex-1",
                                            p { class: "truncate text-[14px] font-medium text-ink", "{name}" }
                                            p { class: "text-[12px] text-ink-secondary", "Paired {paired} · Seen: {last}" }
                                        }
                                        if confirming {
                                            button {
                                                r#type: "button",
                                                class: "rounded-full px-2.5 py-1 text-[13px] text-ink-secondary hover:bg-ink/[0.07]",
                                                onclick: move |_| removing.set(None),
                                                "Keep"
                                            }
                                            button {
                                                r#type: "button",
                                                "data-confirm-remove": true,
                                                class: "rounded-full bg-danger px-2.5 py-1 text-[13px] font-medium text-white hover:opacity-90",
                                                onclick: move |_| {
                                                    removing.set(None);
                                                    remove("DELETE", format!("/api/devices/{gone_id}"));
                                                },
                                                "Remove"
                                            }
                                        } else {
                                            button {
                                                r#type: "button",
                                                "data-remove-device": "{id}",
                                                class: "rounded-full px-2.5 py-1 text-[13px] font-medium text-danger hover:bg-danger/10",
                                                onclick: move |_| removing.set(Some(ask_id.clone())),
                                                "Remove"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if door.is_some() {
                    Reach {}
                }
                if let Some(err) = problem() {
                    p { role: "alert", class: "mt-3 text-[13px] text-danger", "{err}" }
                }
            }
        }
    }
}

/// How phones reach this computer (stage 19.7.1): Same Wi-Fi, keeping awake, and a check of
/// each way in, as MausBot's "How phones reach" group.
#[component]
fn Reach() -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let mut reach = use_signal(|| None::<Value>);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut checks = use_signal(|| None::<Vec<Value>>);
    let mut checking = use_signal(|| false);
    use_effect(move || {
        let _ = (rev.devices)();
        spawn(async move {
            if let Ok(answer) = api::get("/api/devices/reach").await {
                reach.set(Some(answer));
            }
        });
    });
    let mut flip = move |key: &'static str| {
        let Some(now) = reach.peek().clone() else { return };
        if busy() {
            return;
        }
        busy.set(true);
        problem.set(None);
        let body = serde_json::json!({ key: now[key] != true });
        spawn(async move {
            match api::call("PUT", "/api/devices/reach", Some(&body)).await {
                Ok(answer) => reach.set(Some(answer)),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let check = move |_| {
        checking.set(true);
        spawn(async move {
            match api::post("/api/devices/check", &serde_json::json!({})).await {
                Ok(answer) => checks.set(answer["checks"].as_array().cloned()),
                Err(err) => problem.set(Some(err)),
            }
            checking.set(false);
        });
    };
    let Some(now) = reach() else { return rsx! {} };
    let same_wifi = now["sameWifi"] == true;
    let keep_awake = now["keepAwake"] == true;
    let wifi_line = match (now["lanUrl"].as_str(), now["wifiProblem"].as_str()) {
        (_, Some(why)) => format!("Couldn't open it: {why}."),
        (Some(url), _) => format!("On at {url}. Use only on a trusted network."),
        _ => "Pair a phone over your home network, without Tailscale. Use only on a trusted network where both devices can see each other.".to_string(),
    };
    let awake_line = match (now["awakeProblem"].as_str(), now["awake"] == true) {
        (Some(why), _) => {
            let mut letters = why.chars();
            format!("{}{}.", letters.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), letters.as_str())
        }
        (None, true) => "Kept awake now, so paired phones can reach it overnight.".to_string(),
        _ => "While phones can reach Catbot, the computer doesn't go to sleep.".to_string(),
    };
    rsx! {
        div { "data-reach": true, class: "mt-5 flex flex-col gap-1",
            span { class: "px-1 text-[12px] font-medium uppercase tracking-wide text-ink-secondary", "How phones reach this computer" }
            div { class: "flex items-center gap-3 rounded-lg px-2 py-2",
                div { class: "min-w-0 flex-1",
                    p { class: "text-[14px] font-medium text-ink", "Same Wi-Fi" }
                    p { "data-wifi-line": true, class: "text-[12px] leading-4 text-ink-secondary [overflow-wrap:anywhere]", "{wifi_line}" }
                }
                crate::settings::Switch { checked: same_wifi, disabled: busy(), label: "Same Wi-Fi".to_string(), on_toggle: move |_| flip("sameWifi") }
            }
            div { class: "flex items-center gap-3 rounded-lg px-2 py-2",
                div { class: "min-w-0 flex-1",
                    p { class: "text-[14px] font-medium text-ink", "Keep this computer awake" }
                    p { "data-awake-line": true, class: "text-[12px] leading-4 text-ink-secondary", "{awake_line}" }
                }
                crate::settings::Switch { checked: keep_awake, disabled: busy(), label: "Keep this computer awake while phones can reach it".to_string(), on_toggle: move |_| flip("keepAwake") }
            }
            div { class: "flex items-center gap-3 rounded-lg px-2 py-2",
                div { class: "min-w-0 flex-1",
                    p { class: "text-[14px] font-medium text-ink", "Connection check" }
                    p { class: "text-[12px] leading-4 text-ink-secondary", "Tries each way in, the way a phone would." }
                }
                button { r#type: "button", "data-check-connection": true, disabled: checking(),
                    class: "shrink-0 rounded-full bg-control px-3 py-1 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50",
                    onclick: check,
                    if checking() { "Checking…" } else { "Check" }
                }
            }
            if let Some(list) = checks() {
                ul { "data-checks": true, class: "mx-2 mb-1 flex flex-col gap-1 rounded-lg bg-raised p-3",
                    for item in list {
                        CheckLine {
                            key: "{item[\"id\"]}",
                            id: item["id"].as_str().unwrap_or_default().to_string(),
                            ok: item["ok"] == true,
                            line: item["line"].as_str().unwrap_or_default().to_string(),
                        }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "px-2 text-[13px] text-danger", "{err}" }
            }
        }
    }
}

#[component]
fn CheckLine(id: String, ok: bool, line: String) -> Element {
    rsx! {
        li { "data-check": "{id}", "data-ok": "{ok}", class: "flex items-start gap-2 text-[13px] leading-[18px] text-ink",
            span { class: if ok { "mt-1 size-2 shrink-0 rounded-full bg-success" } else { "mt-1 size-2 shrink-0 rounded-full bg-danger" } }
            span { class: "[overflow-wrap:anywhere]", "{line}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_links_and_codes() {
        assert_eq!(pairing_link("https://moo.tail1.ts.net", "ABCDEFGHJK"), "catbot://pair?url=https%3A%2F%2Fmoo.tail1.ts.net&code=ABCDEFGHJK");
        assert_eq!(spaced("ABCDEFGHJK"), "ABCDE-FGHJK");
        let svg = qr_svg(&pairing_link("https://moo.tail1.ts.net", "ABCDEFGHJK")).unwrap();
        assert!(svg.starts_with("<?xml") || svg.starts_with("<svg"));
    }
}
