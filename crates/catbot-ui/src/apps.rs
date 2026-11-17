//! Settings › Apps (stage 16): the apps that give bots tools. 16.1 is MCP servers, run here as a
//! command or reached at a link (`server/apps.rs`). Catbot tests each one when it's added or
//! changed, and turns it on only when it answers. The person ticks which bots get its tools.
//! Tools the server marks read-only run freely; every other use asks first.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::live::State;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const SMALL: &str = "rounded-lg bg-inset px-2 py-1.5 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";
const LINK: &str = "rounded-md px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-ink disabled:opacity-50";

#[component]
pub fn AppsSettings() -> Element {
    let state = use_context::<Signal<State>>();
    let mut list = use_signal(Vec::<Value>::new);
    let mut problem = use_signal(|| None::<String>);
    // None: the list. Some(Null): adding one. Some(server): changing it.
    let mut editing = use_signal(|| None::<Value>);
    use_effect(move || {
        let _ = state.read().apps_changed;
        spawn(async move {
            match api::get("/api/apps").await {
                Ok(found) => list.set(found["servers"].as_array().cloned().unwrap_or_default()),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    rsx! {
        section { class: "flex flex-col gap-3", "data-apps": true,
            h2 { class: HEADING, "MCP servers" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Programs that give bots tools: run here as a command, or reached at a link. Only the bots you tick get an app's tools. Tools the app marks as only reading run freely; anything else shows you a card first, with exactly what it would do."
            }
            if let Some(server) = editing() {
                ServerForm { server, on_done: move |_| editing.set(None) }
            } else {
                for server in list() {
                    ServerRow { key: "{server[\"id\"].as_str().unwrap_or_default()}", server: server.clone(), on_edit: move |s| editing.set(Some(s)) }
                }
                if list.read().is_empty() {
                    p { class: "text-[13px] text-ink-tertiary", "No apps yet." }
                }
                div {
                    button { r#type: "button", class: PRIMARY, "data-add-app": true, onclick: move |_| editing.set(Some(Value::Null)), "Add an MCP server" }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}

#[component]
fn ServerRow(server: Value, on_edit: EventHandler<Value>) -> Element {
    let state = use_context::<Signal<State>>();
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| false);
    let mut showing_tools = use_signal(|| false);
    let id = server["id"].as_str().unwrap_or_default().to_string();
    let name = server["name"].as_str().unwrap_or_default().to_string();
    let on = server["enabled"].as_bool().unwrap_or(false);
    let how = if server["transport"] == "command" {
        format!("Runs {}", server["command"].as_str().unwrap_or_default())
    } else {
        server["url"].as_str().unwrap_or_default().to_string()
    };
    let tools = server["tools"].as_array().cloned().unwrap_or_default();
    let reads = tools.iter().filter(|t| t["readOnly"] == true).count();
    let broken = server["problem"].as_str().map(str::to_string);
    let ticked: Vec<String> = server["bots"].as_array().map(|b| b.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let bots: Vec<(String, String)> = state.read().bots.iter().filter(|b| b.channel.is_none()).map(|b| (b.id.clone(), b.name.clone())).collect();
    let act = use_callback(move |(method, path, body): (&'static str, String, Option<Value>)| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            if let Err(err) = api::call(method, &path, body.as_ref()).await {
                problem.set(Some(err));
            }
            busy.set(false);
        });
    });
    let (switch_id, test_id, delete_id) = (id.clone(), id.clone(), id.clone());
    let editable = server.clone();
    rsx! {
        div { class: "flex flex-col gap-1.5 rounded-lg border border-hairline/40 px-3 py-2", "data-app": "{name}",
            div { class: "flex items-start gap-2",
                div { class: "min-w-0 flex-1",
                    div { class: "text-[14px] font-medium text-ink", "{name}" }
                    div { class: "truncate text-[12px] text-ink-tertiary", "{how}" }
                    if let Some(why) = &broken {
                        div { class: "text-[12px] text-danger", "Not working: {why}" }
                    } else {
                        div { class: "text-[12px] text-ink-secondary", "{tools.len()} tools · {reads} only read" }
                    }
                }
                label { class: "flex items-center gap-1.5 text-[12px] text-ink-secondary",
                    input { r#type: "checkbox", checked: on, disabled: busy(), "data-app-on": true,
                        onchange: move |_| act.call(("PUT", format!("/api/apps/{switch_id}"), Some(json!({ "enabled": !on })))),
                    }
                    if on { "On" } else { "Off" }
                }
            }
            div { class: "flex flex-wrap items-center gap-x-4 gap-y-1",
                span { class: "text-[12px] text-ink-secondary", "Bots:" }
                for (bot_id, bot_name) in bots {
                    {
                        let ticked_now = ticked.contains(&bot_id);
                        let others = ticked.clone();
                        let path = format!("/api/apps/{id}");
                        rsx! {
                            label { key: "{bot_id}", class: "flex items-center gap-1.5 text-[13px] text-ink",
                                input { r#type: "checkbox", checked: ticked_now, "data-app-bot": "{bot_name}",
                                    onchange: move |_| {
                                        let mut next: Vec<String> = others.iter().filter(|b| **b != bot_id).cloned().collect();
                                        if !ticked_now {
                                            next.push(bot_id.clone());
                                        }
                                        act.call(("PUT", path.clone(), Some(json!({ "bots": next }))));
                                    },
                                }
                                "{bot_name}"
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-wrap gap-1",
                button { r#type: "button", class: LINK, onclick: move |_| showing_tools.set(!showing_tools()), if showing_tools() { "Hide tools" } else { "Tools" } }
                button { r#type: "button", class: LINK, disabled: busy(), "data-app-test": true, onclick: move |_| act.call(("POST", format!("/api/apps/{test_id}/test"), Some(json!({})))), if busy() { "Testing…" } else { "Test again" } }
                button { r#type: "button", class: LINK, onclick: move |_| on_edit.call(editable.clone()), "Edit" }
                span { class: "flex-1" }
                if confirm() {
                    button { r#type: "button", class: "rounded-md bg-danger px-2 py-1 text-[12px] font-medium text-white", onclick: move |_| act.call(("DELETE", format!("/api/apps/{delete_id}"), None)), "Remove" }
                    button { r#type: "button", class: LINK, onclick: move |_| confirm.set(false), "Keep" }
                } else {
                    button { r#type: "button", class: "{LINK} hover:text-danger", onclick: move |_| confirm.set(true), "Remove…" }
                }
            }
            if showing_tools() {
                div { class: "flex flex-col gap-0.5 border-t border-hairline/30 pt-1",
                    for tool in tools.iter() {
                        {
                            let tool_name = tool["name"].as_str().unwrap_or_default().to_string();
                            let what = tool["description"].as_str().unwrap_or_default().lines().next().unwrap_or_default().chars().take(120).collect::<String>();
                            let free = tool["readOnly"] == true;
                            rsx! {
                                div { key: "{tool_name}", class: "flex gap-2 text-[12px]",
                                    span { class: "w-40 shrink-0 truncate font-mono text-ink", "{tool_name}" }
                                    span { class: if free { "w-16 shrink-0 text-ink-tertiary" } else { "w-16 shrink-0 text-warning" }, if free { "reads" } else { "asks" } }
                                    span { class: "min-w-0 flex-1 truncate text-ink-secondary", "{what}" }
                                }
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
}

/// Name and value rows: headers for a link, environment variables for a command. A saved value
/// shows as its last four characters; leave it so to keep it.
#[component]
fn Pairs(rows: Signal<Vec<(String, String)>>, name_hint: &'static str, value_hint: &'static str) -> Element {
    let mut rows = rows;
    rsx! {
        div { class: "flex flex-col gap-1",
            for (i, (name, value)) in rows().into_iter().enumerate() {
                div { key: "{i}", class: "flex gap-1",
                    input { class: "{SMALL} w-44", placeholder: name_hint, value: "{name}", oninput: move |e| { rows.write()[i].0 = e.value(); } }
                    input { class: "{SMALL} min-w-0 flex-1", placeholder: value_hint, value: "{value}", oninput: move |e| { rows.write()[i].1 = e.value(); } }
                    button { r#type: "button", class: LINK, "aria-label": "Remove this row", onclick: move |_| { rows.write().remove(i); }, "×" }
                }
            }
            div {
                button { r#type: "button", class: LINK, onclick: move |_| rows.write().push((String::new(), String::new())), "+ Add a row" }
            }
        }
    }
}

fn rows_of(v: &Value) -> Vec<(String, String)> {
    v.as_array().map(|a| a.iter().map(|p| (p["name"].as_str().unwrap_or_default().to_string(), p["value"].as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default()
}

fn rows_json(rows: &[(String, String)]) -> Value {
    json!(rows.iter().filter(|(n, _)| !n.trim().is_empty()).map(|(n, v)| json!({ "name": n.trim(), "value": v })).collect::<Vec<_>>())
}

#[component]
fn ServerForm(server: Value, on_done: EventHandler<()>) -> Element {
    let is_new = server.is_null();
    let mut name = use_signal(|| server["name"].as_str().unwrap_or_default().to_string());
    let mut transport = use_signal(|| server["transport"].as_str().unwrap_or("http").to_string());
    let mut url = use_signal(|| server["url"].as_str().unwrap_or_default().to_string());
    let headers = use_signal(|| rows_of(&server["headers"]));
    let mut command = use_signal(|| server["command"].as_str().unwrap_or_default().to_string());
    let mut args = use_signal(|| server["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default());
    let env = use_signal(|| rows_of(&server["env"]));
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);
    // A new one that didn't answer: kept off, with the reason.
    let mut result = use_signal(|| None::<Value>);
    let id = server["id"].as_str().unwrap_or_default().to_string();
    let save = move |_| {
        let mut body = json!({ "name": name(), "transport": transport() });
        if transport() == "command" {
            body["command"] = json!(command());
            body["args"] = json!(args().lines().map(str::trim).filter(|a| !a.is_empty()).collect::<Vec<_>>());
            body["env"] = rows_json(&env());
        } else {
            body["url"] = json!(url());
            body["headers"] = rows_json(&headers());
        }
        let (method, path) = if is_new { ("POST", "/api/apps".to_string()) } else { ("PUT", format!("/api/apps/{id}")) };
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call(method, &path, Some(&body)).await {
                Ok(answer) if answer["server"]["problem"].is_string() => result.set(Some(answer["server"].clone())),
                Ok(_) => on_done.call(()),
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };
    if let Some(saved) = result() {
        let why = saved["problem"].as_str().unwrap_or_default().to_string();
        return rsx! {
            div { class: "flex flex-col gap-2 rounded-lg border border-hairline/40 px-3 py-2",
                p { class: "text-[13px] text-ink", "Saved, but off: it didn't answer." }
                p { class: "text-[12px] text-danger", "{why}" }
                p { class: "text-[12px] text-ink-secondary", "Change it with Edit and it's tested again; it turns on once it answers." }
                div { button { r#type: "button", class: PRIMARY, onclick: move |_| on_done.call(()), "OK" } }
            }
        };
    }
    rsx! {
        div { class: "flex flex-col gap-3 rounded-lg border border-hairline/40 px-3 py-3", "data-app-form": true,
            label { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Name"
                input { class: FIELD, "data-app-name": true, placeholder: "GitHub", value: "{name}", oninput: move |e| name.set(e.value()) }
            }
            div { role: "radiogroup", class: "flex gap-1",
                for (value, label) in [("http", "Reached at a link"), ("command", "Runs as a command here")] {
                    button { key: "{value}", r#type: "button", role: "radio", "aria-checked": "{transport() == value}", "data-app-transport": "{value}",
                        class: if transport() == value { "rounded-lg bg-accent px-3 py-1.5 text-[12px] font-medium text-white" } else { "rounded-lg bg-control px-3 py-1.5 text-[12px] text-ink-secondary" },
                        onclick: move |_| transport.set(value.to_string()),
                        "{label}"
                    }
                }
            }
            if transport() == "command" {
                label { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Command"
                    input { class: FIELD, "data-app-command": true, placeholder: "npx", value: "{command}", oninput: move |e| command.set(e.value()) }
                }
                label { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Arguments, one per line"
                    textarea { class: "{FIELD} min-h-16 font-mono", "data-app-args": true, placeholder: "-y\n@modelcontextprotocol/server-github", value: "{args}", oninput: move |e| args.set(e.value()) }
                }
                div { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Environment (keys it needs)"
                    Pairs { rows: env, name_hint: "GITHUB_TOKEN", value_hint: "value" }
                }
            } else {
                label { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Link"
                    input { class: FIELD, "data-app-url": true, placeholder: "https://example.com/mcp", value: "{url}", oninput: move |e| url.set(e.value()) }
                }
                div { class: "flex flex-col gap-1 text-[12px] text-ink-secondary", "Headers (a key, if it needs one)"
                    Pairs { rows: headers, name_hint: "Authorization", value_hint: "Bearer …" }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
            div { class: "flex justify-end gap-2",
                button { r#type: "button", class: BUTTON, onclick: move |_| on_done.call(()), "Cancel" }
                button { r#type: "button", class: PRIMARY, "data-app-save": true, disabled: busy(), onclick: save, if busy() { "Testing…" } else if is_new { "Add and test" } else { "Save and test" } }
            }
        }
    }
}
