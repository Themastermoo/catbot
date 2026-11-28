//! Settings › Email (stage 13.9): the person's mailbox, and which bots may use it.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const SMALL: &str = "rounded-lg bg-inset px-2 py-1.5 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";

#[component]
pub fn EmailSettings() -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let mut view = use_signal(|| None::<Value>);
    let mut editing = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    use_effect(move || {
        let _ = (rev.email)();
        spawn(async move {
            match api::get("/api/email").await {
                Ok(found) => view.set(Some(found)),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let Some(current) = view() else { return rsx! {} };
    let connected = current["connected"].as_bool().unwrap_or(false);
    rsx! {
        section { class: "flex flex-col gap-3", "data-email-settings": true,
            h2 { class: HEADING, "Email" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Connect a mailbox and choose which bots may use it. They can search and read it; sending will always ask you first. Gmail, iCloud and Yahoo need an app password, not your normal one. Gmail and Outlook as connected apps come later, with connected apps."
            }
            if connected && !editing() {
                Connected { current: current.clone(), on_change: move |answer: Value| view.set(Some(answer)), on_edit: move |_| editing.set(true) }
            } else {
                ConnectForm { current: current.clone(), on_done: move |answer: Value| { view.set(Some(answer)); editing.set(false); }, on_cancel: move |_| editing.set(false), editing: connected }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}

#[component]
fn Connected(current: Value, on_change: EventHandler<Value>, on_edit: EventHandler<()>) -> Element {
    let state = use_context::<Signal<State>>();
    let mut problem = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| false);
    let address = current["address"].as_str().unwrap_or_default().to_string();
    let allowed: Vec<String> = current["bots"].as_array().map(|b| b.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone())).collect();
    let disconnect = move |_| {
        spawn(async move {
            match api::call("DELETE", "/api/email", None).await {
                Ok(answer) => on_change.call(answer),
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    rsx! {
        div { class: "flex items-center gap-2 rounded-lg border border-hairline/40 px-3 py-2",
            div { class: "min-w-0 flex-1",
                div { class: "text-[14px] font-medium text-ink", "data-email-address": true, "{address}" }
                div { class: "text-[12px] text-ink-secondary", "{current[\"imapHost\"].as_str().unwrap_or_default()}" }
            }
            button { r#type: "button", class: BUTTON, onclick: move |_| on_edit.call(()), "Change" }
            if confirm() {
                button { r#type: "button", class: "rounded-lg bg-danger px-3 py-1.5 text-[13px] font-medium text-white", "data-email-disconnect": true, onclick: disconnect, "Disconnect" }
            } else {
                button { r#type: "button", class: BUTTON, onclick: move |_| confirm.set(true), "Disconnect…" }
            }
        }
        div { class: "text-[12px] font-medium text-ink-secondary", "Bots that may use it" }
        div { class: "flex flex-wrap gap-x-4 gap-y-1", "data-email-bots": true,
            for (id, name) in bots {
                {
                    let on = allowed.contains(&id);
                    let others = allowed.clone();
                    rsx! {
                        label { key: "{id}", class: "flex items-center gap-1.5 text-[13px] text-ink",
                            input { r#type: "checkbox", checked: on, "data-email-bot": "{name}",
                                onchange: move |_| {
                                    let mut next: Vec<String> = others.iter().filter(|b| **b != id).cloned().collect();
                                    if !on {
                                        next.push(id.clone());
                                    }
                                    spawn(async move {
                                        match api::call("PUT", "/api/email/bots", Some(&json!({ "bots": next }))).await {
                                            Ok(answer) => on_change.call(answer),
                                            Err(err) => problem.set(Some(err)),
                                        }
                                    });
                                },
                            }
                            "{name}"
                        }
                    }
                }
            }
        }
        if let Some(err) = problem() {
            p { role: "alert", class: "text-[12px] text-danger", "{err}" }
        }
    }
}

#[component]
fn ConnectForm(current: Value, on_done: EventHandler<Value>, on_cancel: EventHandler<()>, editing: bool) -> Element {
    let s = |key: &str| current[key].as_str().unwrap_or_default().to_string();
    let n = |key: &str, or: u64| current[key].as_u64().unwrap_or(or).to_string();
    let mut address = use_signal(|| s("address"));
    let mut password = use_signal(String::new);
    let mut username = use_signal(|| s("username"));
    let mut imap_host = use_signal(|| s("imapHost"));
    let mut imap_port = use_signal(|| n("imapPort", 993));
    let mut imap_security = use_signal(|| current["imapSecurity"].as_str().unwrap_or("tls").to_string());
    let mut smtp_host = use_signal(|| s("smtpHost"));
    let mut smtp_port = use_signal(|| n("smtpPort", 465));
    let mut smtp_security = use_signal(|| current["smtpSecurity"].as_str().unwrap_or("tls").to_string());
    let mut known = use_signal(|| None::<bool>);
    let mut servers = use_signal(|| editing);
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let look_up = move |_| {
        let wanted = address.peek().trim().to_string();
        if !wanted.contains('@') {
            return;
        }
        spawn(async move {
            let path = format!("/api/email/guess?address={}", wanted.replace('@', "%40"));
            if let Ok(found) = api::get(&path).await {
                let is_known = found["known"].as_bool().unwrap_or(false);
                known.set(Some(is_known));
                if is_known {
                    imap_host.set(found["imapHost"].as_str().unwrap_or_default().into());
                    imap_port.set(found["imapPort"].as_u64().unwrap_or(993).to_string());
                    imap_security.set(found["imapSecurity"].as_str().unwrap_or("tls").into());
                    smtp_host.set(found["smtpHost"].as_str().unwrap_or_default().into());
                    smtp_port.set(found["smtpPort"].as_u64().unwrap_or(465).to_string());
                    smtp_security.set(found["smtpSecurity"].as_str().unwrap_or("tls").into());
                } else {
                    servers.set(true);
                }
            }
        });
    };
    let connect = move |_| {
        busy.set(true);
        problem.set(None);
        let body = json!({
            "address": address.peek().trim(),
            "password": *password.peek(),
            "username": username.peek().trim(),
            "imapHost": imap_host.peek().trim(), "imapPort": imap_port.peek().trim().parse::<u16>().unwrap_or(0), "imapSecurity": *imap_security.peek(),
            "smtpHost": smtp_host.peek().trim(), "smtpPort": smtp_port.peek().trim().parse::<u16>().unwrap_or(0), "smtpSecurity": *smtp_security.peek(),
        });
        spawn(async move {
            match api::call("PUT", "/api/email", Some(&body)).await {
                Ok(answer) => on_done.call(answer),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    let security = |mut value: Signal<String>, label: &'static str| {
        rsx! {
            select { class: SMALL, "aria-label": "{label}", value: "{value}", onchange: move |e| value.set(e.value()),
                option { value: "tls", "TLS" }
                option { value: "starttls", "STARTTLS" }
                option { value: "plain", "None (this computer only)" }
            }
        }
    };
    rsx! {
        input { class: FIELD, r#type: "email", placeholder: "you@example.com", "data-email-field": "address", value: "{address}", oninput: move |e| address.set(e.value()), onblur: look_up }
        input { class: FIELD, r#type: "password", placeholder: if editing { "Password (leave empty to keep it)" } else { "Password or app password" }, "data-email-field": "password", value: "{password}", oninput: move |e| password.set(e.value()) }
        if known() == Some(true) && !servers() {
            p { class: "text-[12px] text-ink-secondary", "Catbot knows this address's mail servers. "
                button { r#type: "button", class: "underline hover:text-ink", onclick: move |_| servers.set(true), "Change them" }
            }
        }
        if servers() {
            input { class: FIELD, placeholder: "Sign-in name, if it isn't the address", value: "{username}", oninput: move |e| username.set(e.value()) }
            div { class: "flex flex-wrap items-center gap-2 text-[12px] text-ink-secondary",
                span { class: "w-24", "Incoming (IMAP)" }
                input { class: "{SMALL} min-w-0 flex-1", placeholder: "imap.example.com", "data-email-field": "imapHost", value: "{imap_host}", oninput: move |e| imap_host.set(e.value()) }
                input { class: "{SMALL} w-20", placeholder: "993", "data-email-field": "imapPort", value: "{imap_port}", oninput: move |e| imap_port.set(e.value()) }
                {security(imap_security, "Incoming security")}
            }
            div { class: "flex flex-wrap items-center gap-2 text-[12px] text-ink-secondary",
                span { class: "w-24", "Outgoing (SMTP)" }
                input { class: "{SMALL} min-w-0 flex-1", placeholder: "smtp.example.com", value: "{smtp_host}", oninput: move |e| smtp_host.set(e.value()) }
                input { class: "{SMALL} w-20", placeholder: "465", value: "{smtp_port}", oninput: move |e| smtp_port.set(e.value()) }
                {security(smtp_security, "Outgoing security")}
            }
        }
        div { class: "flex flex-wrap items-center gap-2",
            button { r#type: "button", class: PRIMARY, "data-email-connect": true, disabled: busy() || address.read().trim().is_empty(), onclick: connect,
                if busy() { "Trying it…" } else if editing { "Save" } else { "Connect" }
            }
            if editing {
                button { r#type: "button", class: BUTTON, onclick: move |_| on_cancel.call(()), "Cancel" }
            }
            if !servers() {
                button { r#type: "button", class: "text-[12px] text-ink-secondary underline hover:text-ink", onclick: move |_| servers.set(true), "Set the servers myself" }
            }
        }
        if let Some(err) = problem() {
            p { role: "alert", class: "text-[12px] text-danger", "{err}" }
        }
    }
}
