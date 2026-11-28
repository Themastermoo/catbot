//! Settings › Pages (stage 13.12): every page a bot published, with its link and a way to take
//! it down.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;

const HEADING: &str = "text-[13px] font-semibold uppercase tracking-wide text-ink-tertiary";
const FIELD: &str = "w-full rounded-lg bg-inset px-3 py-2 text-[13px] text-ink focus:outline-none focus:ring-1 focus:ring-accent";
const BUTTON: &str = "rounded-lg bg-control px-3 py-1.5 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50";
const PRIMARY: &str = "rounded-lg bg-accent px-3 py-1.5 text-[13px] font-semibold text-white disabled:opacity-50";

/// Where pages can go (stage 13.12.2): the default for new pages' cards, and Netlify's and
/// Cloudflare's keys. The keys never come back from Catbot; only their last four characters.
#[component]
fn Hosts() -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let mut hosts = use_signal(|| Value::Null);
    let mut problem = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut netlify_token = use_signal(String::new);
    let mut cloudflare_token = use_signal(String::new);
    let mut cloudflare_account = use_signal(String::new);
    use_effect(move || {
        let _ = (rev.pages)();
        spawn(async move {
            if let Ok(found) = api::get("/api/pages/hosts").await {
                hosts.set(found["hosts"].clone());
            }
        });
    });
    let save = use_callback(move |body: Value| {
        busy.set(true);
        problem.set(None);
        spawn(async move {
            match api::call("PUT", "/api/pages/hosts", Some(&body)).await {
                Ok(found) => {
                    hosts.set(found["hosts"].clone());
                    netlify_token.set(String::new());
                    cloudflare_token.set(String::new());
                }
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    });
    let h = hosts();
    let netlify_on = h["netlify"].is_object();
    let cloudflare_on = h["cloudflare"].is_object();
    let default = h["default"].as_str().unwrap_or("catbot").to_string();
    let wrangler = h["wrangler"].as_str().map(str::to_string);
    let netlify_ends = h["netlify"]["ends"].as_str().unwrap_or_default().to_string();
    let cloudflare_ends = h["cloudflare"]["ends"].as_str().unwrap_or_default().to_string();
    let cloudflare_acct = h["cloudflare"]["account"].as_str().unwrap_or_default().to_string();
    rsx! {
        section { class: "flex flex-col gap-3", "data-page-hosts": true,
            h2 { class: HEADING, "Where pages go" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Each page's card lets you pick. This is where the card starts. Pages on Netlify or Cloudflare get a site of their own and stay up when this computer is off; taking one down deletes its site."
            }
            label { class: "flex items-center gap-2 text-[13px] text-ink",
                span { class: "text-ink-secondary", "New pages start on" }
                select {
                    "data-page-default": true,
                    class: "rounded-lg border border-hairline/60 bg-raised px-2 py-1 text-ink",
                    disabled: busy(),
                    onchange: move |e| save.call(json!({ "default": e.value() })),
                    option { value: "catbot", selected: default == "catbot", "This computer (Catbot)" }
                    if netlify_on {
                        option { value: "netlify", selected: default == "netlify", "Netlify" }
                    }
                    if cloudflare_on {
                        option { value: "cloudflare", selected: default == "cloudflare", "Cloudflare Pages" }
                    }
                }
            }
            div { class: "flex flex-col gap-2 rounded-lg border border-hairline/40 px-3 py-2",
                div { class: "flex items-center gap-2",
                    span { class: "flex-1 text-[14px] font-medium text-ink", "Netlify" }
                    if netlify_on {
                        span { class: "text-[12px] text-ink-tertiary", "Connected · token …{netlify_ends}" }
                        button { r#type: "button", class: BUTTON, disabled: busy(), onclick: move |_| save.call(json!({ "netlify": null })), "Disconnect" }
                    }
                }
                if !netlify_on {
                    p { class: "text-[12px] text-ink-secondary", "A personal access token, from Netlify's User settings › Applications." }
                    div { class: "flex gap-2",
                        input { class: FIELD, r#type: "password", placeholder: "Netlify token", "data-netlify-token": true, value: "{netlify_token}", oninput: move |e| netlify_token.set(e.value()) }
                        button { r#type: "button", class: PRIMARY, disabled: busy() || netlify_token().trim().is_empty(), onclick: move |_| save.call(json!({ "netlify": { "token": netlify_token() } })), "Connect" }
                    }
                }
            }
            div { class: "flex flex-col gap-2 rounded-lg border border-hairline/40 px-3 py-2",
                div { class: "flex items-center gap-2",
                    span { class: "flex-1 text-[14px] font-medium text-ink", "Cloudflare Pages" }
                    if cloudflare_on {
                        span { class: "text-[12px] text-ink-tertiary", "Connected · token …{cloudflare_ends}" }
                        button { r#type: "button", class: BUTTON, disabled: busy(), onclick: move |_| save.call(json!({ "cloudflare": null })), "Disconnect" }
                    }
                }
                if cloudflare_on {
                    p { class: "truncate text-[12px] text-ink-tertiary", "Account {cloudflare_acct}" }
                } else {
                    p { class: "text-[12px] text-ink-secondary", "An API token with Account › Cloudflare Pages › Edit, and the account ID from the right of the dashboard." }
                    input { class: FIELD, r#type: "password", placeholder: "Cloudflare API token", "data-cloudflare-token": true, value: "{cloudflare_token}", oninput: move |e| cloudflare_token.set(e.value()) }
                    div { class: "flex gap-2",
                        input { class: FIELD, placeholder: "Account ID", "data-cloudflare-account": true, value: "{cloudflare_account}", oninput: move |e| cloudflare_account.set(e.value()) }
                        button { r#type: "button", class: PRIMARY, disabled: busy() || cloudflare_token().trim().is_empty() || cloudflare_account().trim().is_empty(), onclick: move |_| save.call(json!({ "cloudflare": { "token": cloudflare_token(), "account": cloudflare_account() } })), "Connect" }
                    }
                }
                match wrangler {
                    Some(command) => rsx! { p { class: "truncate text-[11px] text-ink-tertiary", "Published with wrangler ({command})" } },
                    None => rsx! { p { class: "text-[12px] text-warning", "Cloudflare pages are published with wrangler, which needs Node on this computer: npm install -g wrangler" } },
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
        }
    }
}

#[component]
pub fn PagesList() -> Element {
    let rev = use_context::<crate::live::Revisions>();
    let mut list = use_signal(Vec::<Value>::new);
    let mut problem = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| None::<String>);
    use_effect(move || {
        let _ = (rev.pages)();
        spawn(async move {
            match api::get("/api/pages").await {
                Ok(found) => list.set(found["pages"].as_array().cloned().unwrap_or_default()),
                Err(err) => problem.set(Some(err)),
            }
        });
    });
    let take_down = move |id: String| {
        spawn(async move {
            if let Err(err) = api::call("DELETE", &format!("/api/pages/{id}"), None).await {
                problem.set(Some(err));
            }
            confirm.set(None);
        });
    };
    rsx! {
        section { class: "flex flex-col gap-3", "data-pages": true,
            h2 { class: HEADING, "Published pages" }
            p { class: "text-[13px] leading-relaxed text-ink-secondary",
                "Pages your bots put online, after you approved them. Ones on this computer are at links nobody can guess, and stay up while it's on."
            }
            if list.read().is_empty() {
                p { class: "text-[13px] text-ink-tertiary", "Nothing published yet." }
            }
            for page in list() {
                {
                    let id = page["id"].as_str().unwrap_or_default().to_string();
                    let title = page["title"].as_str().unwrap_or("A page").to_string();
                    let url = page["url"].as_str().unwrap_or_default().to_string();
                    let bot = page["botName"].as_str().unwrap_or_default().to_string();
                    let when = page["publishedAt"].as_str().map(crate::chat::parse_time).filter(|at| !at.is_nan()).map(crate::chat::short_when).unwrap_or_default();
                    let files = page["files"].as_u64().unwrap_or(0);
                    let place = match page["host"].as_str() {
                        Some("netlify") => "On Netlify",
                        Some("cloudflare") => "On Cloudflare",
                        _ => "On this computer",
                    };
                    let plural = if files == 1 { "" } else { "s" };
                    let asking = confirm().as_deref() == Some(id.as_str());
                    let (down, keep) = (id.clone(), id.clone());
                    rsx! {
                        div { key: "{id}", class: "flex items-start gap-2 rounded-lg border border-hairline/40 px-3 py-2", "data-page": "{id}",
                            div { class: "min-w-0 flex-1",
                                div { class: "text-[14px] font-medium text-ink", "{title}" }
                                a { class: "block truncate text-[12px] text-accent underline", href: "{url}", target: "_blank", rel: "noopener", "{url}" }
                                div { class: "text-[11px] text-ink-tertiary", "{place} · by {bot} · {when} · {files} file{plural}" }
                            }
                            if asking {
                                button { r#type: "button", class: "rounded-lg bg-danger px-2.5 py-1 text-[12px] font-medium text-white", "data-page-down": true, onclick: move |_| take_down(down.clone()), "Take down" }
                                button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised", onclick: move |_| confirm.set(None), "Keep" }
                            } else {
                                button { r#type: "button", class: "rounded-lg px-2 py-1 text-[12px] text-ink-secondary hover:bg-raised hover:text-danger", onclick: move |_| confirm.set(Some(keep.clone())), "Take down…" }
                            }
                        }
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", class: "text-[12px] text-danger", "{err}" }
            }
            Hosts {}
        }
    }
}
