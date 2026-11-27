//! Settings, as one window grouped the way MausBot's (and Grok Bot's) is (stage 19.2): a column
//! on the left with a search box and the pages in groups, and the open page on the right.
//!
//! - **This computer** (its overview: "At a glance" and the sidebar's look): Phone & devices, AI,
//!   Voice, Apps, Computers & secrets, Spending, Import & export.
//! - **Your team**: Chief of Staff, Skills, Email, Published pages, Hand-offs.
//! - **This app** (version, notifications, about): Appearance.
//!
//! The layout and classes follow OpenMausBot 0.1.176's settings window (Apache-2.0, see NOTICE).
//! MausBot's account, plan and language pages have nothing to stand for in Catbot.

use dioxus::prelude::*;
use serde_json::Value;

use crate::api;
use crate::live::State;
use crate::looks::{self, Looks, SKINS, TINTS};
use crate::sidebar::Icon;

/// A card of rows (their settings card).
const CARD: &str = "overflow-hidden rounded-xl bg-composer ring-1 ring-inset ring-ink/[0.07]";
const ROW: &str = "flex w-full min-h-[52px] items-center gap-3 px-4 py-3 text-left [&+&]:border-t [&+&]:border-hairline/50";
const SECTION: &str = "px-4 pb-2 pt-6 text-[13px] font-semibold text-ink first:pt-2";
const NAV_ITEM: &str = "flex h-8 w-full items-center gap-2.5 rounded-lg px-2 text-left text-[13px] text-ink";

/// A page of the window: its key, name, glyph and words a search finds it by.
struct Page {
    key: &'static str,
    name: &'static str,
    glyph: &'static str,
    words: &'static str,
}

const COMPUTER: [Page; 8] = [
    Page { key: "devices", name: "Phone & devices", glyph: "phone", words: "pair pairing phone android remote away tailscale qr" },
    Page { key: "ai", name: "AI", glyph: "sparkles", words: "providers programs claude codex grok cli gemini kimi cursor antigravity sign in install engines ollama lm studio" },
    // The APIs that take a key, on a page of their own (stage 29.9).
    Page { key: "apis", name: "APIs", glyph: "key", words: "api apis keys openai anthropic xai grok gemini google openrouter groq mistral deepseek cerebras minimax" },
    Page { key: "voice", name: "Voice", glyph: "mic", words: "voice microphone speak talk listen whisper kokoro piper elevenlabs read aloud mute" },
    Page { key: "apps", name: "Apps", glyph: "puzzle", words: "connect apps mcp servers composio tools gmail slack notion" },
    Page { key: "computers", name: "Computers & secrets", glyph: "monitor", words: "computer boat docker secrets 1password vault" },
    Page { key: "spending", name: "Spending", glyph: "coins", words: "usage cost tokens limits plan spend money" },
    Page { key: "import", name: "Import & export", glyph: "arrows", words: "team file import export grok bot move mausbot" },
];
const TEAM: [Page; 5] = [
    Page { key: "chief", name: "Chief of Staff", glyph: "crown", words: "chief brief check-ins morning" },
    Page { key: "skills", name: "Skills", glyph: "zap", words: "skills library skill.md" },
    Page { key: "email", name: "Email", glyph: "mail", words: "email inbox imap smtp gmail mail" },
    Page { key: "pages", name: "Published pages", glyph: "rocket", words: "pages publish netlify cloudflare site" },
    Page { key: "handoffs", name: "Hand-offs", glyph: "handoff", words: "hand-offs handoffs delegate bots" },
];
const APP: [Page; 1] = [Page { key: "appearance", name: "Appearance", glyph: "palette", words: "theme dark light skin colour color look" }];

/// The page a key names (old tab names included).
fn known(key: &str) -> &'static str {
    match key {
        "general" | "computer" | "" => "computer",
        "team" => "import",
        "notifications" => "app",
        other => COMPUTER.iter().chain(TEAM.iter()).chain(APP.iter()).find(|p| p.key == other).map(|p| p.key).unwrap_or(if other == "app" { "app" } else { "computer" }),
    }
}

fn title_of(key: &str) -> &'static str {
    match key {
        "computer" => "This computer",
        "app" => "This app",
        other => COMPUTER.iter().chain(TEAM.iter()).chain(APP.iter()).find(|p| p.key == other).map(|p| p.name).unwrap_or("Settings"),
    }
}

fn group_of(key: &str) -> &'static str {
    if TEAM.iter().any(|p| p.key == key) {
        "Your team"
    } else if key == "app" || APP.iter().any(|p| p.key == key) {
        "This app"
    } else {
        "This computer"
    }
}

fn matches(page: &Page, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || page.name.to_lowercase().contains(&q) || page.words.split(' ').any(|w| w.starts_with(&q))
}

/// The page the Settings window shows, for a page that hands over to another.
#[derive(Clone, Copy)]
struct ShownPage(Signal<&'static str>);

#[component]
pub fn SettingsWindow(on_close: EventHandler<()>, start: String) -> Element {
    // On a phone the list and a page take turns: it opens on the list, unless asked for a page.
    let mut showing_page = use_signal(|| !matches!(start.as_str(), "" | "general" | "computer"));
    let mut page = use_signal(move || known(&start));
    use_context_provider(|| ShownPage(page));
    let mut query = use_signal(String::new);
    let state = use_context::<Signal<State>>();
    let bots = state.read().bots.iter().filter(|b| b.channel.is_none()).count();
    let at_home = !crate::link::pairs() && !crate::link::has_own();
    let mut go = move |key: &'static str| {
        page.set(key);
        showing_page.set(true);
    };
    // "/" puts the cursor in the search box, as in MausBot; Esc closes.
    use_hook(move || {
        spawn(async move {
            let mut keys = document::eval(
                r#"const onKey = (event) => {
                    if (!document.querySelector("[data-settings-window]")) { document.removeEventListener("keydown", onKey); return; }
                    const typing = event.target.closest && event.target.closest("input, textarea, select, [contenteditable]");
                    if (event.key === "/" && !typing) { event.preventDefault(); document.getElementById("settings-search")?.focus(); }
                    else if (event.key === "Escape" && !typing) { dioxus.send("close"); }
                };
                document.addEventListener("keydown", onKey);"#,
            );
            while keys.recv::<String>().await.is_ok() {
                on_close.call(());
            }
        });
    });
    let current = page();
    let q = query();
    let computer: Vec<&Page> = COMPUTER.iter().filter(|p| (at_home || p.key != "devices") && matches(p, &q)).collect();
    let team: Vec<&Page> = TEAM.iter().filter(|p| matches(p, &q)).collect();
    let app: Vec<&Page> = APP.iter().filter(|p| matches(p, &q)).collect();
    let (nav_class, page_class) = if showing_page() { ("max-md:hidden", "") } else { ("", "max-md:hidden") };
    let version = env!("CARGO_PKG_VERSION");
    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 max-md:p-0", onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                "aria-label": "Settings",
                "data-app-settings": true,
                "data-settings-window": true,
                class: "flex h-[min(88vh,760px)] w-full max-w-[960px] overflow-hidden rounded-2xl bg-panel shadow-2xl max-md:h-full max-md:rounded-none",
                onclick: move |e| e.stop_propagation(),
                nav { "aria-label": "Settings pages", class: "flex w-[240px] shrink-0 flex-col border-r-[0.5px] border-edge bg-panel max-md:w-full max-md:border-r-0 {nav_class}",
                    div { class: "flex items-center gap-2 px-3 pb-2 pt-3",
                        div { class: "flex h-8 min-w-0 flex-1 items-center gap-2 rounded-lg bg-composer px-2.5 ring-1 ring-inset ring-ink/[0.07] focus-within:ring-focus",
                            span { class: "text-ink-secondary", Icon { name: "search", size: 14 } }
                            input {
                                id: "settings-search",
                                "data-settings-search": true,
                                placeholder: "Search settings",
                                "aria-label": "Search settings",
                                class: "min-w-0 flex-1 bg-transparent text-[13px] text-ink placeholder:text-ink-tertiary focus:outline-none",
                                value: "{query}",
                                oninput: move |e| query.set(e.value()),
                                onkeydown: move |e| if e.key() == Key::Escape { e.prevent_default(); query.set(String::new()); },
                            }
                            kbd { class: "rounded border border-hairline px-1 font-sans text-[11px] text-ink-tertiary", "/" }
                        }
                        button { r#type: "button", "aria-label": "Close settings", class: "flex size-8 shrink-0 items-center justify-center rounded-lg text-ink-secondary hover:bg-raised hover:text-ink md:hidden", onclick: move |_| on_close.call(()), Icon { name: "x", size: 16 } }
                    }
                    div { class: "min-h-0 flex-1 overflow-y-auto px-3 pb-3",
                        if q.trim().is_empty() || "this computer".contains(&q.trim().to_lowercase()) {
                            GroupHead { selected: current == "computer", glyph: "laptop", name: "This computer", line: format!("{bots} bot{}", if bots == 1 { "" } else { "s" }), on_pick: move |_| go("computer") }
                        }
                        for p in computer {
                            NavItem { key: "{p.key}", selected: current == p.key, glyph: p.glyph, name: p.name, on_pick: move |_| go(p.key) }
                        }
                        if !team.is_empty() {
                            div { class: "px-2 pb-1 pt-4 text-[12px] font-medium text-ink-tertiary", "Your team" }
                        }
                        for p in team {
                            NavItem { key: "{p.key}", selected: current == p.key, glyph: p.glyph, name: p.name, on_pick: move |_| go(p.key) }
                        }
                        if q.trim().is_empty() || "this app notifications about version".split(' ').any(|w| w.starts_with(&q.trim().to_lowercase())) {
                            div { class: "pt-3",
                                GroupHead { selected: current == "app", glyph: "catbot", name: "This app", line: format!("Catbot {version}"), on_pick: move |_| go("app") }
                            }
                        }
                        for p in app {
                            NavItem { key: "{p.key}", selected: current == p.key, glyph: p.glyph, name: p.name, on_pick: move |_| go(p.key) }
                        }
                    }
                }
                div { class: "flex min-w-0 flex-1 flex-col bg-app {page_class}",
                    div { class: "flex shrink-0 items-start gap-2 px-6 pb-2 pt-5 max-md:px-4",
                        button { r#type: "button", "aria-label": "Back to settings", class: "-ml-1 flex size-8 shrink-0 items-center justify-center rounded-lg text-ink-secondary hover:bg-raised hover:text-ink md:hidden", onclick: move |_| showing_page.set(false),
                            span { class: "rotate-180", Icon { name: "chevron-right", size: 16 } }
                        }
                        div { class: "min-w-0 flex-1",
                            h2 { "data-settings-title": true, class: "text-[17px] font-semibold leading-6 text-ink", "{title_of(current)}" }
                            p { class: "mt-0.5 flex items-center gap-1.5 text-[13px] text-ink-secondary",
                                "For "
                                span { class: "text-ink", "{group_of(current).to_lowercase()}" }
                            }
                        }
                        button { r#type: "button", "aria-label": "Close settings", "data-settings-close": true, class: "flex size-8 shrink-0 items-center justify-center rounded-lg text-ink-secondary hover:bg-raised hover:text-ink", onclick: move |_| on_close.call(()), Icon { name: "x", size: 16 } }
                    }
                    div { class: "min-h-0 flex-1 overflow-y-auto px-6 pb-8 pt-2 max-md:px-4", "data-settings-page": "{current}",
                        match current {
                            "computer" => rsx! { ComputerPage { on_go: go } },
                            "devices" => rsx! { crate::devices::DevicesDialog { embedded: true, on_close: move |_| {} } },
                            "ai" => rsx! { AiPage { apis: false } },
                            "apis" => rsx! { AiPage { apis: true } },
                            "voice" => rsx! { crate::voice::VoiceSettings {} },
                            "apps" => rsx! { crate::apps::AppsSettings {} },
                            "computers" => rsx! { crate::computers::ComputersSettings {} },
                            "spending" => rsx! { crate::usage::UsageDialog { embedded: true, on_close: move |_| {} } },
                            "import" => rsx! { crate::teams::TeamFiles {} },
                            "chief" => rsx! { crate::chief::ChiefOptions {} },
                            "skills" => rsx! { crate::skills::SkillsLibrary {} },
                            "email" => rsx! { crate::email::EmailSettings {} },
                            "pages" => rsx! { crate::pages::PagesList {} },
                            "handoffs" => rsx! { crate::handoffs::HandoffSettings {} },
                            "app" => rsx! { AppPage {} },
                            "appearance" => rsx! { AppearancePage {} },
                            _ => rsx! {},
                        }
                    }
                }
            }
        }
    }
}

/// A group's own row at its top: its glyph in a square, its name and a line under it.
#[component]
fn GroupHead(selected: bool, glyph: &'static str, name: &'static str, line: String, on_pick: EventHandler<()>) -> Element {
    let surface = if selected { "bg-raised" } else { "hover:bg-raised/60" };
    rsx! {
        button { r#type: "button", "data-settings-nav": "{name}", "aria-current": if selected { "page" }, class: "flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left {surface}", onclick: move |_| on_pick.call(()),
            span { class: "flex size-7 shrink-0 items-center justify-center rounded-md bg-raised text-ink-secondary ring-1 ring-inset ring-ink/[0.05]",
                if glyph == "catbot" {
                    img { src: asset!("/assets/icon/catbot-32.png"), alt: "", class: "size-5" }
                } else {
                    Icon { name: glyph, size: 15 }
                }
            }
            span { class: "flex min-w-0 flex-1 flex-col",
                span { class: "truncate text-[14px] font-medium leading-5 text-ink", "{name}" }
                span { class: "truncate text-[12px] leading-4 text-ink-secondary", "{line}" }
            }
        }
    }
}

#[component]
fn NavItem(selected: bool, glyph: &'static str, name: &'static str, on_pick: EventHandler<()>) -> Element {
    let surface = if selected { "bg-raised font-medium ring-1 ring-inset ring-accent/40" } else { "hover:bg-raised/60" };
    rsx! {
        button { r#type: "button", "data-settings-nav": "{name}", "aria-current": if selected { "page" }, class: "{NAV_ITEM} {surface}", onclick: move |_| on_pick.call(()),
            span { class: "flex size-4 shrink-0 items-center justify-center text-ink-secondary", Icon { name: glyph, size: 15 } }
            span { class: "truncate", "{name}" }
        }
    }
}

/// A row of a card: a label (and a line under it), what's on the right, and an arrow when it
/// goes somewhere.
#[component]
fn Row(label: String, #[props(default)] line: String, #[props(default)] value: String, #[props(default)] warn: bool, on_go: Option<EventHandler<()>>, children: Element) -> Element {
    let value_class = if warn { "text-danger" } else { "text-ink-secondary" };
    let body = rsx! {
        span { class: "flex min-w-0 flex-1 flex-col",
            span { class: "text-[14px] leading-5 text-ink", "{label}" }
            if !line.is_empty() {
                span { class: "text-[12px] leading-[17px] text-ink-secondary", "{line}" }
            }
        }
        if !value.is_empty() {
            span { class: "shrink-0 text-[13px] {value_class}", "{value}" }
        }
        {children}
    };
    match on_go {
        Some(go) => rsx! {
            button { r#type: "button", class: "{ROW} hover:bg-ink/[0.03]", "data-settings-row": "{label}", onclick: move |_| go.call(()),
                {body}
                span { class: "shrink-0 text-ink-secondary", Icon { name: "chevron-right", size: 14 } }
            }
        },
        None => rsx! { div { class: ROW, "data-settings-row": "{label}", {body} } },
    }
}

/// This computer: what's set up, at a glance, and how the sidebar looks.
#[component]
fn ComputerPage(on_go: EventHandler<&'static str>) -> Element {
    let mut glance = use_signal(|| None::<(Option<usize>, usize, usize, usize)>);
    let mut keyed_count = use_signal(|| 0usize);
    let at_home = !crate::link::pairs() && !crate::link::has_own();
    use_effect(move || {
        spawn(async move {
            let devices = if at_home { api::get("/api/devices").await.ok().map(|d| d["devices"].as_array().map(Vec::len).unwrap_or(0)) } else { None };
            let engines = api::get("/api/engines").await.ok().and_then(|e| e["engines"].as_array().cloned()).unwrap_or_default();
            let ready = engines.iter().filter(|e| e["ready"] == true && e["id"] != "" && !is_api(e)).count();
            keyed_count.set(engines.iter().filter(|e| e["ready"] == true && is_api(e)).count());
            let apps = api::get("/api/apps").await.ok().and_then(|a| a["servers"].as_array().map(|s| s.iter().filter(|x| x["enabled"] != false).count())).unwrap_or(0);
            glance.set(Some((devices, ready, engines.len(), apps)));
        });
    });
    let look = use_context::<Signal<Looks>>();
    let now = look();
    let (devices, ready, _total, apps) = glance().unwrap_or((None, 0, 0, 0));
    let keyed = keyed_count();
    let loading = glance().is_none();
    rsx! {
        h3 { class: SECTION, "At a glance" }
        div { class: CARD, "data-at-a-glance": true,
            if at_home {
                Row { label: "Phone & devices".to_string(), value: if loading { String::new() } else { match devices { Some(0) | None => "Not set up".to_string(), Some(n) => format!("{n} paired") } }, on_go: move |_| on_go.call("devices") }
            }
            Row { label: "AI".to_string(), value: if loading { String::new() } else if ready == 0 { "None ready".to_string() } else { format!("{ready} ready") }, warn: !loading && ready == 0, on_go: move |_| on_go.call("ai") }
            Row { label: "APIs".to_string(), value: if loading { String::new() } else if keyed == 0 { "No keys yet".to_string() } else { format!("{keyed} with a key") }, on_go: move |_| on_go.call("apis") }
            Row { label: "Apps".to_string(), value: if loading { String::new() } else if apps == 0 { "None yet".to_string() } else { format!("{apps} connected") }, on_go: move |_| on_go.call("apps") }
        }
        h3 { class: SECTION, "In the sidebar" }
        div { class: CARD,
            Row { label: "Sidebar colour".to_string(), line: "Tints the top of the sidebar.".to_string(),
                div { class: "flex shrink-0 items-center gap-2", role: "radiogroup", "aria-label": "Sidebar colour",
                    button { r#type: "button", role: "radio", "aria-checked": "{now.tint.is_empty()}", "aria-label": "No colour", title: "None", "data-tint": "",
                        class: if now.tint.is_empty() { "flex size-6 items-center justify-center rounded-full bg-composer text-ink-secondary ring-2 ring-ink" } else { "flex size-6 items-center justify-center rounded-full bg-composer text-ink-secondary ring-1 ring-ink/20" },
                        onclick: move |_| looks::change(look, |l| l.tint.clear()),
                        svg { "aria-hidden": "true", width: "12", height: "12", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2.5", stroke_linecap: "round", path { d: "M19 5 5 19" } }
                    }
                    for (name, color) in TINTS {
                        button { key: "{name}", r#type: "button", role: "radio", "aria-checked": "{now.tint == name}", "aria-label": "{name}", title: "{name}", "data-tint": "{name}",
                            class: if now.tint == name { "size-6 rounded-full ring-2 ring-ink ring-offset-2 ring-offset-[var(--color-composer)]" } else { "size-6 rounded-full" },
                            style: "background: {color}",
                            onclick: move |_| looks::change(look, |l| l.tint = name.to_string()),
                        }
                    }
                }
            }
            Row { label: "Show the Chief of Staff in the sidebar".to_string(), line: "Hidden, it keeps working and search still finds it.".to_string(),
                crate::settings::Switch { checked: now.show_chief, disabled: false, label: "Show the Chief of Staff in the sidebar".to_string(), on_toggle: move |_| { looks::change(look, |l| l.show_chief = !l.show_chief); } }
            }
        }
    }
}

/// The AI Settings › AI opens on, once (an Inbox fix, stage 27).
pub static OPEN_PROVIDER: GlobalSignal<Option<String>> = Signal::global(|| None);

/// An API that takes a key (OpenAI, xAI, OpenRouter…): these have their own page, APIs (stage
/// 29.9). AIs on this computer (Ollama, LM Studio) stay with the programs.
fn is_api(engine: &Value) -> bool {
    engine["kind"] == "api" && engine["needsKey"] == true
}

/// The AIs Catbot can run bots on: the main ones and any that are ready, then the rest. Each
/// opens how to set it up or manage it. `apis`: the APIs page; else the AI page, the programs.
#[component]
fn AiPage(apis: bool) -> Element {
    let shown = use_context::<ShownPage>();
    let mut engines = use_signal(|| None::<Result<Vec<Value>, String>>);
    let mut checked = use_signal(|| 0u32);
    let mut open = use_signal(|| OPEN_PROVIDER.write().take());
    let mut more = use_signal(|| false);
    let state = use_context::<Signal<State>>();
    use_effect(move || {
        let _ = checked();
        let _ = state.read().engines_changed;
        spawn(async move {
            engines.set(Some(api::get("/api/engines").await.map(|e| e["engines"].as_array().cloned().unwrap_or_default())));
        });
    });
    const LEAD_PROGRAMS: [&str; 10] = ["claude", "codex", "chatgpt", "grok-cli", "antigravity", "gemini", "kimi", "cursor", "muse-code", "pi"];
    const LEAD_APIS: [&str; 5] = ["openai", "anthropic", "google", "xai", "openrouter"];
    let lead: &[&str] = if apis { &LEAD_APIS } else { &LEAD_PROGRAMS };
    let all = match engines() {
        None => return rsx! { p { class: "px-4 pt-4 text-[13px] text-ink-tertiary", "Checking…" } },
        Some(Err(err)) => return rsx! { p { role: "alert", class: "px-4 pt-4 text-[13px] text-danger", "{err}" } },
        Some(Ok(list)) => list.into_iter().filter(|e| e["id"] != "").collect::<Vec<_>>(),
    };
    // An Inbox fix for an AI on the other page: hand it over there.
    if let Some(id) = open.peek().clone()
        && let Some(engine) = all.iter().find(|e| e["id"] == id.as_str())
        && is_api(engine) != apis
    {
        *OPEN_PROVIDER.write() = Some(id);
        let mut page = shown.0;
        spawn(async move { page.set(if apis { "ai" } else { "apis" }) });
    }
    let list: Vec<Value> = all.into_iter().filter(|e| is_api(e) == apis).collect();
    let lead_rank = |e: &Value| lead.iter().position(|id| e["id"] == *id);
    let wanted = open.peek().clone();
    let shown_first = |e: &Value| e["ready"] == true || lead_rank(e).is_some() || e["trouble"].is_object() || wanted.as_deref() == e["id"].as_str();
    let mut first: Vec<Value> = list.iter().filter(|e| shown_first(e)).cloned().collect();
    first.sort_by_key(|e| (e["ready"] != true, lead_rank(e).unwrap_or(99)));
    let rest: Vec<Value> = list.iter().filter(|e| !shown_first(e)).cloned().collect();
    let rest_count = rest.len();
    rsx! {
        if !apis && !crate::link::is_phone() {
            crate::local_ai::OnThisComputer {}
        }
        if apis {
            p { class: "px-4 pt-4 text-[13px] leading-[19px] text-ink-secondary", "AIs reached over the internet with a key of your own, billed by their maker. Keys stay on this computer." }
        }
        div { class: "flex items-center px-4 pb-2 pt-6",
            h3 { class: "flex-1 text-[13px] font-semibold text-ink", if apis { "APIs" } else { "AI programs" } }
            button { r#type: "button", "data-check-again": true, class: "rounded-md px-2 py-1 text-[13px] text-ink-secondary hover:bg-raised hover:text-ink", onclick: move |_| *checked.write() += 1, "Check again" }
        }
        div { class: CARD, "data-providers": true,
            for engine in first {
                Provider { key: "{engine[\"id\"]}", open: open().as_deref() == engine["id"].as_str(), engine: engine.clone(), on_toggle: move |id: String| open.set(if open.peek().as_deref() == Some(id.as_str()) { None } else { Some(id) }) }
            }
            if more() {
                for engine in rest {
                    Provider { key: "{engine[\"id\"]}", open: open().as_deref() == engine["id"].as_str(), engine: engine.clone(), on_toggle: move |id: String| open.set(if open.peek().as_deref() == Some(id.as_str()) { None } else { Some(id) }) }
                }
            } else if rest_count > 0 {
                Row { label: if apis { "More APIs".to_string() } else { "More programs".to_string() }, value: "{rest_count}", on_go: move |_| more.set(true) }
            }
        }
    }
}

/// One AI: its mark, name and state, and Set up or Manage.
#[component]
fn Provider(engine: Value, open: bool, on_toggle: EventHandler<String>) -> Element {
    let id = engine["id"].as_str().unwrap_or_default().to_string();
    let name = engine["name"].as_str().unwrap_or_default().to_string();
    let ready = engine["ready"] == true;
    let trouble = engine["trouble"]["text"].as_str().map(str::to_string);
    let state = match (ready, engine["kind"].as_str()) {
        _ if trouble.is_some() => format!("Needs fixing · {}", trouble.clone().unwrap_or_default()),
        (true, _) => engine["keyHint"].as_str().map(|h| format!("Ready · key …{h}")).unwrap_or_else(|| "Ready".to_string()),
        (false, Some("api")) if engine["needsKey"] == false => "Not added · See On this computer".to_string(),
        (false, Some("api")) => "Not set up · Needs a key".to_string(),
        (false, _) if engine["installed"] == false => "Not set up · Not installed".to_string(),
        (false, _) => "Not set up · Needs a sign-in".to_string(),
    };
    let initial: String = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    // Each provider's letter in a colour of its own (not its logo).
    let hue = id.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    rsx! {
        div { class: "[&+&]:border-t [&+&]:border-hairline/50", "data-provider": "{id}",
            div { class: "flex min-h-[60px] items-center gap-3 px-4 py-2.5",
                span { "aria-hidden": "true", class: "flex size-9 shrink-0 items-center justify-center rounded-full text-[14px] font-semibold", style: "background: hsl({hue} 60% 50% / 0.14); color: hsl({hue} 55% 40%)", "{initial}" }
                span { class: "flex min-w-0 flex-1 flex-col",
                    span { class: "truncate text-[14px] leading-5 text-ink", "{name}" }
                    span { class: if trouble.is_some() { "flex items-center gap-1.5 whitespace-normal text-[12px] leading-4 text-danger" } else { "flex items-center gap-1.5 truncate text-[12px] leading-4 text-ink-secondary" },
                        "data-provider-trouble": trouble.is_some(),
                        if trouble.is_some() {
                            span { class: "size-1.5 shrink-0 rounded-full bg-danger" }
                        } else if ready {
                            span { class: "size-1.5 shrink-0 rounded-full bg-success" }
                        }
                        "{state}"
                    }
                }
                button { r#type: "button", "data-provider-toggle": "{id}", "aria-expanded": "{open}",
                    class: if open { "shrink-0 rounded-full px-3 py-1 text-[13px] font-medium text-ink ring-2 ring-accent" } else { "shrink-0 rounded-full bg-control px-3 py-1 text-[13px] font-medium text-ink hover:bg-raised" },
                    onclick: move |_| on_toggle.call(id.clone()),
                    if open { "Close" } else if ready { "Manage" } else { "Set up" }
                }
            }
            if open {
                div { class: "px-4 pb-4", crate::engines::EngineSetup { engine: engine.clone() } }
            }
        }
    }
}

/// This app: its version, notifications, and about.
#[component]
fn AppPage() -> Element {
    let version = env!("CARGO_PKG_VERSION");
    let mut about = use_signal(|| false);
    rsx! {
        h3 { class: SECTION, "Updates" }
        Updates { version: version.to_string() }
        h3 { class: SECTION, "Desktop pets" }
        div { class: CARD, crate::pets::PetsSettings {} }
        if crate::notify::AVAILABLE {
            h3 { class: SECTION, "Notifications" }
            crate::notify::NotificationSettings {}
        }
        h3 { class: SECTION, "Help" }
        div { class: CARD,
            Row { label: "About Catbot".to_string(), on_go: move |_| about.toggle() }
            if about() {
                div { class: "border-t border-hairline/50 px-4 py-3 text-[13px] leading-[19px] text-ink-secondary",
                    p { "Catbot {version}: a home for your AI bots, built in Rust." }
                    p { class: "mt-1", "It follows OpenMausBot's design and wire protocol (Apache-2.0); the full list of what it borrows, and their licences, is in its NOTICE file." }
                }
            }
        }
    }
}

/// The version, and the update check (stage 19.7.4): against Catbot's GitHub repository (its
/// latest release, or its newest commit when it has no releases). Installing and going back are
/// stage 28: the banner (`update.rs`) and the versions kept on this PC.
#[component]
fn Updates(version: String) -> Element {
    let mut info = use_signal(|| None::<Value>);
    let mut typed = use_signal(String::new);
    let mut editing = use_signal(|| false);
    let mut checking = use_signal(|| false);
    let mut outcome = use_signal(|| None::<Result<Value, String>>);
    let at_home = crate::link::active().is_none();
    use_effect(move || {
        spawn(async move {
            if let Ok(answer) = api::get("/api/version").await {
                typed.set(answer["repo"].as_str().unwrap_or_default().to_string());
                if !answer["checked"].is_null() {
                    outcome.set(Some(Ok(answer["checked"].clone())));
                }
                info.set(Some(answer));
            }
        });
    });
    let built = info().and_then(|i| i["built"].as_str().map(|b| {
        let at = crate::chat::parse_time(b);
        let when = crate::chat::short_when(at);
        // Today it reads as a time already.
        if when.contains(':') { format!("Built today at {}", crate::chat::clock(at)) } else { format!("Built {when}, {}", crate::chat::clock(at)) }
    }));
    let repo = info().and_then(|i| i["repo"].as_str().map(str::to_string));
    let save = move |_| {
        spawn(async move {
            match api::call("PUT", "/api/version", Some(&serde_json::json!({ "repo": typed.peek().trim() }))).await {
                Ok(answer) => {
                    info.set(Some(answer));
                    editing.set(false);
                    outcome.set(None);
                }
                Err(err) => outcome.set(Some(Err(err))),
            }
        });
    };
    let check = move |_| {
        checking.set(true);
        spawn(async move {
            outcome.set(Some(api::post("/api/version/check", &serde_json::json!({})).await));
            checking.set(false);
        });
    };
    // Why this device can't install the newer release itself (the home PC: the helper builds it).
    let offer_why_not = match outcome() {
        Some(Ok(found)) => found["offer"]["whyNot"].as_str().map(str::to_string),
        _ => None,
    };
    let said = match outcome() {
        None => None,
        Some(Err(err)) => Some((false, err, None)),
        Some(Ok(found)) => {
            let latest = &found["latest"];
            let name = latest["name"].as_str().unwrap_or_default();
            let url = latest["url"].as_str().map(str::to_string);
            let words = match (latest["kind"].as_str(), found["newer"] == true) {
                (Some("release"), true) => format!("A newer Catbot is out: {name}."),
                (Some("release"), false) => format!("Up to date: the latest release is {name}."),
                (_, true) => format!("Your repository has newer work ({name}{}). The helper builds it.", latest["message"].as_str().map(|m| format!(": {m}")).unwrap_or_default()),
                _ => format!("Up to date with the repository's newest commit ({name})."),
            };
            Some((found["newer"] == true, words, url))
        }
    };
    rsx! {
        div { class: CARD, "data-updates": true,
            Row { label: format!("Catbot {version}"), line: built.unwrap_or_else(|| "Your helper builds each new stage and restarts Catbot; reopen this window to get it.".to_string()) }
            if at_home {
                Row { label: "GitHub repository".to_string(), line: match &repo { Some(r) => format!("Updates are checked at github.com/{r}."), None => "Where updates are checked, once Catbot is on GitHub.".to_string() },
                    button { r#type: "button", "data-edit-repo": true, class: "shrink-0 rounded-full bg-control px-3 py-1 text-[13px] font-medium text-ink hover:bg-raised", onclick: move |_| editing.toggle(),
                        if editing() { "Close" } else if repo.is_some() { "Change" } else { "Set" }
                    }
                }
                if editing() {
                    div { class: "flex items-center gap-2 border-t border-hairline/50 px-4 py-3",
                        input { "data-repo-input": true, value: "{typed}", placeholder: "owner/name", "aria-label": "GitHub repository",
                            class: "min-w-0 flex-1 rounded-md bg-inset px-2 py-1.5 font-mono text-[13px] text-ink outline-none focus:ring-1 focus:ring-accent",
                            oninput: move |e| typed.set(e.value()),
                        }
                        button { r#type: "button", "data-save-repo": true, class: "shrink-0 rounded-full bg-accent px-3 py-1 text-[13px] font-medium text-accent-ink hover:brightness-110", onclick: save, "Save" }
                    }
                }
                if repo.is_some() {
                    Row { label: "Check for updates".to_string(),
                        button { r#type: "button", "data-check-updates": true, disabled: checking(), class: "shrink-0 rounded-full bg-control px-3 py-1 text-[13px] font-medium text-ink hover:bg-raised disabled:opacity-50", onclick: check,
                            if checking() { "Checking…" } else { "Check now" }
                        }
                    }
                }
                if let Some((newer, words, url)) = said {
                    div { "data-update-result": true, class: "border-t border-hairline/50 px-4 py-3 text-[13px]",
                        span { class: if newer { "font-medium text-accent" } else { "text-ink-secondary" }, "{words}" }
                        if let Some(url) = url {
                            " "
                            a { href: "{url}", target: "_blank", rel: "noreferrer", class: "text-accent underline", "See it on GitHub" }
                        }
                        if newer && let Some(why) = offer_why_not.clone() {
                            p { class: "mt-1 text-[12px] text-ink-secondary", "{why}" }
                        } else if newer {
                            p { class: "mt-1 text-[12px] text-ink-secondary", "The banner at the top of the window has Update." }
                        }
                    }
                }
                crate::update::Versions {}
            }
        }
    }
}

/// Appearance: the theme, following the system or one of the stylesheet's skins.
#[component]
fn AppearancePage() -> Element {
    let look = use_context::<Signal<Looks>>();
    let current = look().skin;
    let choice = |id: &'static str, name: &'static str, line: &'static str, on: bool| {
        rsx! {
            button { key: "{id}", r#type: "button", role: "radio", "aria-checked": "{on}", "data-skin-choice": "{id}", class: "{ROW} hover:bg-ink/[0.03]",
                onclick: move |_| looks::change(look, |l| l.skin = id.to_string()),
                span { class: "flex min-w-0 flex-1 flex-col",
                    span { class: "text-[14px] leading-5 text-ink", "{name}" }
                    if !line.is_empty() {
                        span { class: "text-[12px] leading-4 text-ink-secondary", "{line}" }
                    }
                }
                if on {
                    span { class: "shrink-0 text-accent", Icon { name: "check", size: 16 } }
                }
            }
        }
    };
    rsx! {
        h3 { class: SECTION, "Theme" }
        div { class: CARD, role: "radiogroup", "aria-label": "Theme",
            {choice("system", "Follow the system", "Daylight when it's light, Midnight when it's dark.", current == "system")}
        }
        h3 { class: SECTION, "Light" }
        div { class: CARD, role: "radiogroup", "aria-label": "Light themes",
            for (id, name, _) in SKINS.iter().filter(|s| !s.2) {
                {choice(id, name, "", current == *id)}
            }
        }
        h3 { class: SECTION, "Dark" }
        div { class: CARD, role: "radiogroup", "aria-label": "Dark themes",
            for (id, name, _) in SKINS.iter().filter(|s| s.2) {
                {choice(id, name, "", current == *id)}
            }
        }
        h3 { class: SECTION, "Chat" }
        div { class: CARD,
            Row { label: "Your messages in the bot's colour".to_string(), line: "Off, they stay grey.".to_string(),
                crate::settings::Switch { checked: look().color_mine, disabled: false, label: "Your messages in the bot's colour".to_string(), on_toggle: move |_| { looks::change(look, |l| l.color_mine = !l.color_mine); } }
            }
            Row { label: "Suggested replies".to_string(), line: "After a bot replies, the empty message box shows one faded line you might send next; Tab fills it. A small model writes it.".to_string(),
                crate::settings::Switch { checked: look().suggested, disabled: false, label: "Suggested replies".to_string(), on_toggle: move |_| { looks::change(look, |l| l.suggested = !l.suggested); } }
            }
        }
    }
}
