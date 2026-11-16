//! `catbot serve`: the server the interface talks to.
//!
//! Catbot and the reference server run side by side, split by bot. Bots made in Catbot are
//! answered here, from Catbot's own store and Claude driver; everything else (their bots,
//! settings, routines…) is passed through to the reference server unchanged. The bot list and
//! the live event stream are merged, so the interface sees one team. Over time Catbot answers
//! more, until the reference server has nothing left to do.

pub mod attachments;
pub mod backups;
pub mod changes;
pub mod channels;
pub mod chief;
pub mod handoffs;
pub mod limits;
pub mod reactions;
pub mod usage;
pub mod devices;
pub mod engines;
pub mod images;
pub mod events;
pub mod hub;
pub mod memory;
pub mod remote;
pub mod turns;
pub mod wire;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use http_body_util::{BodyExt, Full};
use serde_json::{Value, json};

use crate::{
    approval::Decision,
    claude::PermissionMode,
    proxy::Proxy,
    record::Recorder,
    store::{self, Bot, Mark, NewBot, Role, Store},
    titles,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct Options {
    pub listen: SocketAddr,
    pub upstream: String,
    pub home: PathBuf,
    pub codex: PathBuf,
    /// The memory vault (`memory.rs`); None turns it off.
    pub memory: Option<PathBuf>,
    pub cli: Vec<String>,
    /// Record the traffic passed to the reference server into this folder.
    pub record: Option<PathBuf>,
    /// Catbot's own interface (the `dx` build's `public` folder), served at `/`.
    pub ui: Option<PathBuf>,
    /// The remote door, for paired devices away from home (`remote.rs`); None turns it off.
    pub remote: Option<SocketAddr>,
}

pub struct App {
    store: Mutex<Store>,
    pub hub: hub::Hub,
    pub turns: turns::Turns,
    pub proxy: Arc<Proxy>,
    pub codex: PathBuf,
    /// The memory vault the bots keep (`memory.rs`), if it's on.
    pub memory: Option<PathBuf>,
    pub cli: Vec<String>,
    /// The approval helper (this program), so every turn can ask before it acts.
    pub helper: Option<Vec<String>>,
    /// Catbot's own interface, when built: served for every path outside `/api`.
    pub ui: Option<PathBuf>,
    /// The model that names new chats (`titles`); None keeps their first-line names.
    pub title_model: Option<String>,
    /// Where the remote door listens, once it does.
    pub remote: std::sync::OnceLock<SocketAddr>,
    /// The pairing code waiting for a device, if any (`devices.rs`).
    pub pairing: std::sync::Mutex<Option<devices::Pairing>>,
    /// The plans' usage windows as last fetched (`usage.rs`), and when.
    pub plan_usage: std::sync::Mutex<Option<(std::time::Instant, Value)>>,
    /// Channels whose bots are answering a message, one after another (`channels.rs`).
    pub channels_busy: std::sync::Mutex<std::collections::HashSet<String>>,
    /// AIs that turned a turn away for their usage limit, and until when (`backups.rs`).
    pub limited_until: std::sync::Mutex<std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>>,
}

impl App {
    pub fn new(
        store: Store,
        proxy: Arc<Proxy>,
        codex: PathBuf,
        memory: Option<PathBuf>,
        cli: Vec<String>,
        helper: Option<Vec<String>>,
        ui: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: Mutex::new(store),
            hub: hub::Hub::default(),
            turns: turns::Turns::default(),
            proxy,
            codex,
            memory,
            cli,
            helper,
            ui,
            title_model: titles::model_from_env(),
            remote: std::sync::OnceLock::new(),
            pairing: std::sync::Mutex::new(None),
            plan_usage: std::sync::Mutex::new(None),
            limited_until: std::sync::Mutex::new(std::collections::HashMap::new()),
            channels_busy: std::sync::Mutex::new(std::collections::HashSet::new()),
        })
    }

    /// Runs `f` with the store, logging (and swallowing) any error.
    pub fn with_store<T>(&self, f: impl FnOnce(&Store) -> Result<T, BoxError>) -> Option<T> {
        let store = self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match f(&store) {
            Ok(value) => Some(value),
            Err(err) => {
                eprintln!("catbot: store: {err}");
                None
            }
        }
    }

    fn try_store<T>(&self, f: impl FnOnce(&Store) -> Result<T, BoxError>) -> Result<T, BoxError> {
        let store = self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&store)
    }

    /// What one chat is doing, for `wire::bot`.
    fn activity(&self) -> impl Fn(&str) -> &'static str + '_ {
        |thread| self.chat_activity(thread)
    }

    /// A chat's activity, counting a chain of hand-offs between turns as working.
    pub fn chat_activity(&self, thread: &str) -> &'static str {
        match self.turns.activity(thread) {
            "idle" if self.channel_busy(thread) => "working",
            other => other,
        }
    }

    /// A bot on the wire, with its chats' activity.
    fn wire_bot(&self, store: &Store, bot: &Bot, messages: Option<usize>) -> Result<Value, BoxError> {
        wire::bot(store, bot, &self.activity(), messages)
    }

    /// Seeds the bot's own note in the memory vault, if the vault is on and the note isn't there.
    pub fn seed_memory(&self, bot: &Bot) {
        if let Some(memory) = &self.memory
            && let Err(err) = crate::memory::ensure_bot_note(memory, bot)
        {
            eprintln!("catbot: couldn't seed {}'s memory note: {err}", bot.name);
        }
    }

    pub fn channel_busy(&self, channel_id: &str) -> bool {
        self.channels_busy.lock().expect("channels lock").contains(channel_id)
    }

    pub fn set_channel_busy(&self, channel_id: &str, busy: bool) {
        let mut set = self.channels_busy.lock().expect("channels lock");
        if busy {
            set.insert(channel_id.to_string());
        } else {
            set.remove(channel_id);
        }
    }

    /// Stops a channel's remaining responders (the one answering is interrupted separately).
    pub fn stop_channel(&self, channel_id: &str) {
        self.set_channel_busy(channel_id, false);
    }

    /// Sends a bot's current state (no messages) to every open interface.
    pub fn announce_bot(&self, bot_id: &str) {
        if channels::is_channel(bot_id) {
            channels::announce(self, bot_id);
            return;
        }
        let wire = self.with_store(|store| {
            let Some(bot) = store.bot_by_id(bot_id)? else { return Ok(None) };
            Ok(Some(self.wire_bot(store, &bot, None)?))
        });
        if let Some(Some(bot)) = wire {
            self.hub.send(json!({ "kind": "bot", "bot": bot }));
        }
    }
}

pub async fn serve(options: Options) -> Result<(), BoxError> {
    let store = Store::open(&options.home)?;
    let recorder = match &options.record {
        Some(dir) => {
            let (recorder, file) = Recorder::start(dir).await?;
            eprintln!("catbot: recording what goes to the reference server in {}", file.display());
            Some(recorder)
        }
        None => None,
    };
    let proxy = Proxy::new(&options.upstream, recorder, 16 * 1024)?;
    let helper = Some(vec![std::env::current_exe()?.to_string_lossy().into_owned()]);
    let bots = store.bots()?.len();
    let ui = options.ui.filter(|dir| {
        let built = dir.join("index.html").is_file();
        if !built {
            eprintln!("catbot: no interface build at {} (build it with dx, see README); serving the API only", dir.display());
        }
        built
    });
    let memory = options.memory.filter(|dir| match crate::memory::prepare(dir, Some(&options.codex)) {
        Ok(()) => {
            let linked = if crate::memory::has_codex(dir) { ", linked to the Codex" } else { "" };
            eprintln!("catbot: the bots' memory vault: {}{linked}", dir.display());
            for bot in store.bots().unwrap_or_default() {
                if let Err(err) = crate::memory::ensure_bot_note(dir, &bot) {
                    eprintln!("catbot: couldn't seed {}'s memory note: {err}", bot.name);
                }
            }
            true
        }
        Err(err) => {
            eprintln!("catbot: no memory vault: couldn't make {} ({err})", dir.display());
            false
        }
    });
    let app = App::new(store, proxy, options.codex, memory, options.cli, helper, ui.clone());

    let listener = tokio::net::TcpListener::bind(options.listen)
        .await
        .map_err(|err| format!("could not listen on {}: {err} (is something else using that port?)", options.listen))?;
    eprintln!("catbot: serving http://{} ({bots} Catbot bot(s) in {})", options.listen, options.home.display());
    eprintln!("catbot: everything else goes to the reference server at {}", app.proxy.upstream());
    match &ui {
        Some(dir) => eprintln!("catbot: Catbot's interface: http://{} (from {})", options.listen, dir.display()),
        None => eprintln!("catbot: OpenMausBot's interface: start it with OMB_PORT={} pnpm dev", options.listen.port()),
    }

    if let Some(addr) = options.remote {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(door) => {
                let _ = app.remote.set(addr);
                eprintln!("catbot: remote door for paired devices: http://{addr} (Tailscale Funnel points here)");
                let app = app.clone();
                tokio::spawn(async move {
                    if let Err(err) = axum::serve(door, remote::router(app)).await {
                        eprintln!("catbot: the remote door stopped: {err}");
                    }
                });
            }
            Err(err) => eprintln!("catbot: no remote door: could not listen on {addr}: {err}"),
        }
    }

    tokio::spawn(tidy(app.clone(), archive_after_days()));
    axum::serve(listener, router(app)).await?;
    Ok(())
}

/// After how many days untouched a chat is archived: `CATBOT_ARCHIVE_DAYS`, else 30; 0 never.
fn archive_after_days() -> u32 {
    std::env::var("CATBOT_ARCHIVE_DAYS").ok().and_then(|d| d.trim().parse().ok()).unwrap_or(30)
}

/// Archives chats untouched for `days` days: at start, then once an hour. Each bot keeps its
/// newest chat and the one it opens to; working chats are left alone.
async fn tidy(app: Arc<App>, days: u32) {
    if days == 0 {
        return;
    }
    loop {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
        let archived = app.with_store(|store| store.archive_idle(cutoff, &|thread| app.turns.busy(thread))).unwrap_or_default();
        if !archived.is_empty() {
            eprintln!("catbot: archived {} chat(s) untouched for {days} days", archived.len());
            let mut bots: Vec<&str> = archived.iter().map(|c| c.bot_id.as_str()).collect();
            bots.dedup();
            for bot in bots {
                app.announce_bot(bot);
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

/// The whole server as a router, for `serve` and for tests.
pub fn router(app: Arc<App>) -> Router {
    Router::new().fallback(dispatch).with_state(app)
}

/// Who answers: Catbot for its own bots and their conversations, the reference server for the rest.
async fn dispatch(State(app): State<Arc<App>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        ["api", "events"] if method == Method::GET => events::events(app, req, events::Door::Home).await,
        ["api", "memory", rest @ ..] if method == Method::GET => respond(memory::route(&app, rest, &req)),
        ["api", "attachments", id] if method == Method::GET => attachments::serve(&app, id).await,
        ["api", "usage", rest @ ..] if method == Method::GET => usage_route(&app, rest, usage_query(&req)).await,
        ["api", "limits"] if method == Method::GET || method == Method::PUT => {
            let body = if method == Method::PUT {
                match json_body(req).await {
                    Ok(body) => Some(body),
                    Err(err) => return respond(Err(err)),
                }
            } else {
                None
            };
            respond(limits::route(&app, &method, body, true).await)
        }
        ["api", "handoffs"] if method == Method::GET => respond(handoffs::route(&app, None)),
        ["api", "chief", rest @ ..] => {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
            let body = if method != Method::PUT {
                None
            } else {
                match json_body(req).await {
                    Ok(body) => Some(body),
                    Err(err) => return respond(Err(err)),
                }
            };
            respond(chief::route(&app, &method, &rest, body))
        }
        ["api", "handoffs"] if method == Method::PUT => match json_body(req).await {
            Ok(body) => respond(handoffs::route(&app, Some(body))),
            Err(err) => respond(Err(err)),
        },
        ["api", "engines", rest @ ..] => {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
            respond(engines::route(&app, &method, &rest, req, true).await)
        }
        ["api", "devices", rest @ ..] => {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
            respond(devices::route(&app, &method, &rest).await)
        }
        ["api", "bots"] if method == Method::GET => list_bots(&app, req).await,
        ["api", "bots"] if method == Method::POST => respond(create_bot(&app, req).await),
        ["api", "channels"] if method == Method::POST => respond(match json_body(req).await {
            Ok(body) => channels::create(&app, &body),
            Err(err) => Err(err),
        }),
        ["api", "bots", id, rest @ ..] if owns_bot(&app, id) => {
            let id = id.to_string();
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            respond(bot_route(&app, &id, &rest, req).await)
        }
        ["api", "threads", id, rest @ ..] if owns_thread(&app, id) => {
            let id = id.to_string();
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            respond(thread_route(&app, &id, &rest, req).await)
        }
        _ if serves_ui(&app, &path) => serve_ui(&app, req).await,
        _ => app.proxy.handle(req).await,
    }
}

/// Every path outside the API is Catbot's interface, when it has one.
fn serves_ui(app: &App, path: &str) -> bool {
    app.ui.is_some() && !path.starts_with("/api/") && path != "/api" && !path.starts_with("/.well-known/")
}

/// A file of the interface, or its `index.html` for any page address it doesn't have as a file.
async fn serve_ui(app: &App, req: Request) -> Response {
    use tower::ServiceExt;
    use tower_http::services::{ServeDir, ServeFile};
    let Some(dir) = &app.ui else { return StatusCode::NOT_FOUND.into_response() };
    let files = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
    // Serving files can't fail as a service (its error type is Infallible); a missing file is a 404 answer.
    match files.oneshot(req).await {
        Ok(response) => response.into_response(),
        Err(never) => match never {},
    }
}

fn owns_bot(app: &App, id: &str) -> bool {
    app.with_store(|store| Ok(store.bot_by_id(id)?.is_some() || (channels::is_channel(id) && store.channel(id)?.is_some()))).unwrap_or(false)
}

fn owns_thread(app: &App, id: &str) -> bool {
    app.with_store(|store| Ok(store.conversation(id)?.is_some())).unwrap_or(false)
}

/// An answer: a status and a JSON body.
type Answer = Result<(StatusCode, Value), (StatusCode, String)>;

/// The usage route's query, read before waiting (a request can't be held across a wait).
fn usage_query(req: &Request) -> std::collections::HashMap<&'static str, String> {
    ["period", "by", "fresh"].into_iter().filter_map(|name| Some((name, urlencoding_decode(query_param(req, name)?)))).collect()
}

/// `GET /api/usage…`, at either door.
async fn usage_route(app: &Arc<App>, rest: &[&str], query: std::collections::HashMap<&'static str, String>) -> Response {
    let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    respond(usage::route(app, &rest, |name| query.get(name).cloned()).await)
}

fn respond(answer: Answer) -> Response {
    match answer {
        Ok((status, body)) => (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response(),
        Err((status, error)) => {
            (status, [(header::CONTENT_TYPE, "application/json")], json!({ "error": error }).to_string()).into_response()
        }
    }
}

fn fail(err: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn bad(err: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, err.to_string())
}

async fn json_body(req: Request) -> Result<Value, (StatusCode, String)> {
    let bytes = axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024).await.map_err(bad)?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|err| bad(format!("body is not JSON: {err}")))
}

fn query_param<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.uri().query()?.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

/// `GET /api/bots`: the reference server's list with Catbot's bots added.
async fn list_bots(app: &Arc<App>, req: Request) -> Response {
    let limit = query_param(&req, "messages").and_then(|n| n.parse().ok()).unwrap_or(50usize);
    let path = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/api/bots".into());
    let headers = req.headers().clone();
    drop(req);
    let mut list = match fetch_upstream(app, &headers, &path).await {
        Ok(value) if value["bots"].is_array() => value,
        Ok(_) | Err(_) => json!({ "bots": [], "botQueuedMessages": {}, "sections": [], "groups": [], "computerControl": {} }),
    };
    let ours = app.with_store(|store| {
        let mut wires = Vec::new();
        for bot in store.bots()? {
            wires.push(app.wire_bot(store, &bot, Some(limit))?);
        }
        for channel in store.channels()? {
            wires.push(channels::channel_wire(app, store, &channel, Some(limit))?);
        }
        Ok(wires)
    });
    for bot in ours.unwrap_or_default() {
        if let Some(control) = list["computerControl"].as_object_mut() {
            control.insert(bot["id"].as_str().unwrap_or_default().to_string(), json!({ "held": false, "helpReason": null }));
        }
        if let Some(bots) = list["bots"].as_array_mut() {
            bots.push(bot);
        }
    }
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], list.to_string()).into_response()
}

/// `GET /api/bots` at the remote door: Catbot's bots alone, in the same shape.
fn catbot_bots(app: &Arc<App>, req: &Request) -> Response {
    let limit = query_param(req, "messages").and_then(|n| n.parse().ok()).unwrap_or(50usize).min(500);
    let ours = app.with_store(|store| {
        let mut wires = Vec::new();
        for bot in store.bots()? {
            wires.push(app.wire_bot(store, &bot, Some(limit))?);
        }
        Ok(wires)
    });
    let bots = ours.unwrap_or_default();
    let control: serde_json::Map<String, Value> = bots
        .iter()
        .map(|b| (b["id"].as_str().unwrap_or_default().to_string(), json!({ "held": false, "helpReason": null })))
        .collect();
    let list = json!({ "bots": bots, "botQueuedMessages": {}, "sections": [], "groups": [], "computerControl": control });
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], list.to_string()).into_response()
}

/// A GET to the reference server with the interface's own headers, as JSON.
async fn fetch_upstream(app: &App, headers: &axum::http::HeaderMap, path: &str) -> Result<Value, BoxError> {
    let mut builder = hyper::Request::builder().method("GET").uri(app.proxy.uri_for(path)?);
    for (name, value) in headers {
        if !matches!(name.as_str(), "connection" | "accept-encoding" | "content-length" | "transfer-encoding" | "upgrade") {
            builder = builder.header(name, value);
        }
    }
    let response = app.proxy.request(builder.body(Full::new(Bytes::new()))?).await?;
    if !response.status().is_success() {
        return Err(format!("the reference server answered {}", response.status()).into());
    }
    let body = response.into_body().collect().await?.to_bytes();
    Ok(serde_json::from_slice(&body)?)
}

/// `POST /api/bots`: new bots made in the interface are Catbot bots.
async fn create_bot(app: &Arc<App>, req: Request) -> Answer {
    let body = json_body(req).await?;
    let wanted = body["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or("New bot");
    let bot = app.try_store(|store| {
        let name = store.free_name(wanted)?;
        let instructions = body["soul"].as_str().or(body["personality"].as_str()).unwrap_or("").to_string();
        store.create_bot(NewBot { name, instructions, model: None, mode: PermissionMode::Default, codex: true })
    }).map_err(bad)?;
    // Anything else the new bot was given (model, approval level, picture, colour), on top of the defaults.
    let mut changes = body.clone();
    if let Some(changes) = changes.as_object_mut() {
        changes.remove("name");
        changes.remove("soul");
    }
    let mut bot = bot;
    apply_changes(&mut bot, &changes)?;
    app.try_store(|store| store.update_bot(&bot)).map_err(bad)?;
    app.seed_memory(&bot);
    let wire = app.try_store(|store| app.wire_bot(store, &bot, Some(50))).map_err(fail)?;
    app.announce_bot(&bot.id);
    eprintln!("catbot: made {} ({})", bot.name, bot.folder.display());
    Ok((StatusCode::CREATED, json!({ "bot": wire })))
}

async fn bot_route(app: &Arc<App>, id: &str, rest: &[String], req: Request) -> Answer {
    if channels::is_channel(id) {
        return channels::route(app, id, rest, req).await;
    }
    let method = req.method().clone();
    let bot = app.try_store(|store| store.bot_by_id(id)).map_err(fail)?.ok_or((StatusCode::NOT_FOUND, "no such bot".into()))?;
    match (method, rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
        (Method::GET, []) => {
            let wire = app.try_store(|store| app.wire_bot(store, &bot, Some(50))).map_err(fail)?;
            Ok((StatusCode::OK, json!({ "bot": wire })))
        }
        (Method::PATCH, []) => patch_bot(app, bot, json_body(req).await?),
        (Method::DELETE, []) => {
            for thread in app.try_store(|store| store.conversations(&bot.id)).map_err(fail)? {
                app.turns.interrupt(&thread.id);
            }
            let bot = app.try_store(|store| store.delete_bot(&bot.name)).map_err(fail)?;
            app.hub.send(json!({ "kind": "bot.deleted", "botId": bot.id }));
            eprintln!("catbot: deleted {} (its folder stays at {})", bot.name, bot.folder.display());
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        (Method::POST, ["messages"]) => send_message(app, bot, json_body(req).await?),
        (Method::POST, ["attachments"]) => attachments::upload(app, &bot, req).await,
        (Method::POST, ["images"]) => images::ask(app, &bot, &json_body(req).await?),
        (Method::POST, ["tasks"]) => new_chat(app, bot, json_body(req).await?),
        (Method::POST, ["tasks", thread]) => {
            let limit = query_param(&req, "messages").and_then(|n| n.parse().ok()).unwrap_or(50usize).min(500);
            open_chat(app, bot, thread, limit)
        }
        (Method::PATCH, ["tasks", thread]) => change_chat(app, bot, thread, json_body(req).await?),
        (Method::POST, ["tasks", thread, "branch"]) => branch_chat(app, bot, thread, json_body(req).await?),
        (Method::DELETE, ["tasks", thread]) => delete_chat(app, bot, thread),
        (Method::POST, ["read"]) => Ok((StatusCode::OK, json!({ "ok": true }))),
        (Method::POST, ["interrupt"]) => {
            let body = json_body(req).await?;
            let thread = match body["threadId"].as_str() {
                Some(thread) => thread.to_string(),
                None => app.try_store(|store| store.ensure_conversation(&bot.id)).map_err(fail)?.id,
            };
            // A chain of hand-offs stops too, not just the bot answering now.
            let chained = app.channel_busy(&thread);
            app.stop_channel(&thread);
            Ok((StatusCode::OK, json!({ "ok": true, "stopped": app.turns.interrupt(&thread) || chained })))
        }
        (method, rest) => {
            eprintln!("catbot: {method} /api/bots/<{}>/{} isn't something Catbot bots do yet", bot.name, rest.join("/"));
            Err((StatusCode::NOT_FOUND, format!("Catbot bots can't do this yet ({method} {})", rest.join("/"))))
        }
    }
}

fn patch_bot(app: &Arc<App>, mut bot: Bot, body: Value) -> Answer {
    let before = bot.clone();
    apply_changes(&mut bot, &body)?;
    app.try_store(|store| store.update_bot(&bot)).map_err(bad)?;
    if let Some(list) = body.get("backups") {
        app.try_store(|store| backups::set(store, &bot, list)).map_err(bad)?;
    }
    if let Some(memory) = &app.memory
        && let Err(err) = crate::memory::rename_bot(memory, &before, &bot)
    {
        eprintln!("catbot: couldn't move {}'s memory note to its new name: {err}", bot.name);
    }
    let wire = app.try_store(|store| app.wire_bot(store, &bot, None)).map_err(fail)?;
    app.hub.send(json!({ "kind": "bot", "bot": wire }));
    Ok((StatusCode::OK, json!({ "bot": wire })))
}

/// Takes what a PATCH or POST body says about a bot. Only the fields present change.
fn apply_changes(bot: &mut Bot, body: &Value) -> Result<(), (StatusCode, String)> {
    if let Some(name) = body["name"].as_str() {
        bot.name = name.trim().to_string();
    }
    if let Some(soul) = body["soul"].as_str() {
        bot.instructions = soul.trim().to_string();
    }
    if let Some(selection) = body.get("modelSelection").filter(|s| s.is_object()) {
        let engine = selection["instanceId"].as_str().filter(|i| !i.is_empty()).unwrap_or(&bot.engine).to_string();
        let known = crate::engines::find(&engine).ok_or_else(|| bad(format!("Catbot doesn't know the AI {engine}")))?;
        let model = selection["model"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
        // A new engine without a model named starts on that engine's own default.
        bot.model = if known.id != bot.engine && model.is_none() { None } else { model };
        bot.engine = known.id.to_string();
    }
    if let Some(level) = body["approvalMode"].as_str() {
        bot.mode = wire::mode_from_approval(level).ok_or_else(|| bad(format!("unknown approval level {level}")))?;
    }
    if let Some(mark) = body["mark"].as_str() {
        bot.mark = Mark::parse(mark).ok_or_else(|| bad(format!("unknown picture {mark}")))?;
    }
    match body.get("color") {
        Some(Value::Null) => bot.color = None,
        Some(Value::String(color)) if store::COLORS.contains(&color.as_str()) => bot.color = Some(color.clone()),
        Some(other) => return Err(bad(format!("unknown colour {other}"))),
        None => {}
    }
    if let Some(codex) = body["codex"].as_bool() {
        bot.codex = codex;
    }
    Ok(())
}

/// `POST /api/bots/:id/messages`: keep the person's message, show it everywhere, start the turn.
fn send_message(app: &Arc<App>, bot: Bot, body: Value) -> Answer {
    let attached = attachments::for_message(app, &bot, &body["attachments"])?;
    let text = body["text"].as_str().map(str::trim).unwrap_or("").to_string();
    if text.is_empty() && attached.is_empty() {
        return Err(bad("nothing to send"));
    }
    let conversation = app
        .try_store(|store| {
            let wanted = body["threadId"].as_str().map(|id| store.conversation(id)).transpose()?.flatten();
            match wanted.filter(|c| c.bot_id == bot.id) {
                Some(conversation) => Ok(conversation),
                None => store.ensure_conversation(&bot.id),
            }
        })
        .map_err(fail)?;
    if app.turns.busy(&conversation.id) || app.channel_busy(&conversation.id) {
        return Err((StatusCode::CONFLICT, "this bot is still working on the last message".into()));
    }
    // A Claude plan's windows, read again in the background when they're getting old.
    if bot.engine == crate::engines::CLAUDE && usage::billing(&bot.engine) == "plan" {
        usage::refresh_soon(app);
    }
    // At a limit: a backup AI takes the turn, or none can.
    let switched = backups::choose(app, &bot).map_err(|why| (StatusCode::TOO_MANY_REQUESTS, why))?;
    let mut detail = json!({});
    if let Some(id) = body["sendId"].as_str() {
        detail["sendId"] = json!(id);
    }
    let described: Vec<Value> = attached.iter().map(attachments::describe).collect();
    if !described.is_empty() {
        detail["attachments"] = json!(described);
    }
    // Reactions the bot hasn't heard about go with this message.
    let reacted = format!("{}{}", reactions::untold_note(app, &conversation.id), changes::untold_note(app, &conversation.id));
    if !reacted.is_empty() {
        detail["reactionNote"] = json!(reacted);
    }
    let detail = (detail != json!({})).then_some(detail);
    let message = turns::save_and_send(app, &conversation.id, Role::Person, &text, detail.as_ref())
        .ok_or_else(|| fail("couldn't save the message"))?;
    if conversation.title.is_none() {
        // Named after the chat's first message: this one, or for a chat from before chats had
        // names, the first thing said in it.
        let named = if text.is_empty() { attached[0].name.clone() } else { text.clone() };
        let first = app.with_store(|store| store.first_words(&conversation.id)).flatten().filter(|w| !w.trim().is_empty()).unwrap_or(named);
        name_chat(app, &bot, &conversation.id, &first);
    }
    let request_id = message["id"].as_str().and_then(wire::parse_message_id).unwrap_or_default();
    let thread = conversation.id.clone();
    // What bots brought into this chat said, which the bot's own session didn't see.
    let told = handoffs::untold_note(app, &conversation.id, &bot.id);
    let said = format!("{text}{}{reacted}{told}", attachments::note(&described));
    let mut bot = bot;
    bot.instructions.push_str(&handoffs::roster_note(app, &bot));
    let bot = match switched {
        Some((stand_in, why)) => {
            turns::save_and_send(app, &conversation.id, Role::Note, &why, None);
            // The backup starts its own session, shown the chat so far.
            app.with_store(|store| store.clear_session(&conversation.id));
            stand_in
        }
        None => bot,
    };
    let conversation = app.try_store(|store| store.conversation(&conversation.id)).map_err(fail)?.unwrap_or(conversation);
    turns::start(app.clone(), bot, conversation, request_id, said.trim().to_string()).map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
    Ok((StatusCode::ACCEPTED, json!({ "ok": true, "threadId": thread, "message": message })))
}

/// Names a chat from its first message: its first line at once, then (in the background)
/// a better name from the title model, unless the person names it first.
fn name_chat(app: &Arc<App>, bot: &Bot, thread: &str, text: &str) {
    let first_line = titles::from_message(text);
    if app.try_store(|store| store.set_title(thread, &first_line, false)).is_err() {
        return;
    }
    app.announce_bot(&bot.id);
    let Some(model) = app.title_model.clone() else { return };
    let Ok(cwd) = app.try_store(|store| Ok(store.home().join("titles"))) else { return };
    let (app, bot_id, thread, text) = (app.clone(), bot.id.clone(), thread.to_string(), text.to_string());
    tokio::spawn(async move {
        let Some(name) = titles::generate(app.cli.clone(), cwd, model, &text).await else { return };
        let still_first_line = app
            .with_store(|store| Ok(store.conversation(&thread)?.is_some_and(|c| !c.title_by_person && c.title.as_deref() == Some(&first_line))))
            .unwrap_or(false);
        if still_first_line && app.with_store(|store| store.set_title(&thread, &name, false)).is_some() {
            app.announce_bot(&bot_id);
        }
    });
}

/// One of this bot's chats, or a 404.
fn chat_of(app: &App, bot: &Bot, thread: &str) -> Result<store::Conversation, (StatusCode, String)> {
    app.try_store(|store| store.conversation(thread))
        .map_err(fail)?
        .filter(|c| c.bot_id == bot.id)
        .ok_or((StatusCode::NOT_FOUND, "no such chat".into()))
}

/// `POST /api/bots/:id/tasks`: a new chat. It becomes the one the bot opens to unless the
/// body says `"select": false`; a `title` names it.
fn new_chat(app: &Arc<App>, bot: Bot, body: Value) -> Answer {
    let select = body["select"].as_bool().unwrap_or(true);
    let conversation = app
        .try_store(|store| if select { store.new_conversation(&bot.id) } else { store.start_conversation(&bot.id, None) })
        .map_err(fail)?;
    if let Some(title) = body["title"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
        app.try_store(|store| store.set_title(&conversation.id, title, true)).map_err(fail)?;
    }
    let conversation = chat_of(app, &bot, &conversation.id)?;
    let wire = app.try_store(|store| app.wire_bot(store, &bot, None)).map_err(fail)?;
    app.announce_bot(&bot.id);
    Ok((StatusCode::CREATED, json!({ "bot": wire, "task": wire::task(&bot, &conversation, "idle") })))
}

/// `POST /api/bots/:id/tasks/:thread/branch` with `{ "messageId": … }`: a new chat holding this
/// one up to that message, opened. The bot starts it fresh and is shown the messages so far.
fn branch_chat(app: &Arc<App>, bot: Bot, thread: &str, body: Value) -> Answer {
    let conversation = chat_of(app, &bot, thread)?;
    if app.turns.busy(&conversation.id) {
        return Err((StatusCode::CONFLICT, "wait for this bot to finish before branching the chat".into()));
    }
    let upto = body["messageId"].as_str().and_then(wire::parse_message_id).ok_or_else(|| bad("branch from which message?"))?;
    app.try_store(|store| store.message_in(&conversation.id, upto))
        .map_err(fail)?
        .ok_or((StatusCode::NOT_FOUND, "no such message in this chat".to_string()))?;
    let name = conversation.title.clone().unwrap_or_else(|| "Chat".into());
    let short: String = name.chars().take(100).collect();
    let branch = app
        .try_store(|store| store.branch_conversation(&conversation, upto, &format!("{short} (branch)")))
        .map_err(fail)?;
    let branch = chat_of(app, &bot, &branch.id)?;
    let wire = app.try_store(|store| app.wire_bot(store, &bot, None)).map_err(fail)?;
    app.announce_bot(&bot.id);
    Ok((StatusCode::CREATED, json!({ "bot": wire, "task": wire::task(&bot, &branch, "idle") })))
}

/// `POST /api/bots/:id/tasks/:thread`: the bot opens to this chat from now on. Answers with
/// the bot and the chat's latest messages.
fn open_chat(app: &Arc<App>, bot: Bot, thread: &str, limit: usize) -> Answer {
    let conversation = chat_of(app, &bot, thread)?;
    app.try_store(|store| store.open_conversation(&conversation)).map_err(fail)?;
    let wire = app.try_store(|store| app.wire_bot(store, &bot, Some(limit))).map_err(fail)?;
    Ok((StatusCode::OK, json!({ "bot": wire })))
}

/// `PATCH /api/bots/:id/tasks/:thread`: rename a chat (`title`), or archive it (`archivedAt`:
/// any value) or bring it back (`archivedAt: null`).
fn change_chat(app: &Arc<App>, bot: Bot, thread: &str, body: Value) -> Answer {
    let conversation = chat_of(app, &bot, thread)?;
    if let Some(title) = body.get("title") {
        let title = title.as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("a chat needs a name"))?;
        if title.chars().count() > 120 {
            return Err(bad("that name is too long"));
        }
        app.try_store(|store| store.set_title(&conversation.id, title, true)).map_err(fail)?;
    }
    if let Some(archived) = body.get("archivedAt") {
        app.try_store(|store| store.set_archived(&conversation.id, !archived.is_null())).map_err(fail)?;
    }
    let conversation = chat_of(app, &bot, thread)?;
    let task = wire::task(&bot, &conversation, app.chat_activity(&conversation.id));
    app.announce_bot(&bot.id);
    Ok((StatusCode::OK, json!({ "task": task })))
}

/// `DELETE /api/bots/:id/tasks/:thread`: delete a chat and its messages. Not while it's working.
fn delete_chat(app: &Arc<App>, bot: Bot, thread: &str) -> Answer {
    let conversation = chat_of(app, &bot, thread)?;
    if app.turns.busy(&conversation.id) {
        return Err((StatusCode::CONFLICT, "this chat is still working; stop it first".into()));
    }
    app.try_store(|store| store.delete_conversation(&conversation.id)).map_err(fail)?;
    // A bot always has a chat to open to.
    let wire = app.try_store(|store| app.wire_bot(store, &bot, None)).map_err(fail)?;
    app.announce_bot(&bot.id);
    Ok((StatusCode::OK, json!({ "ok": true, "bot": wire })))
}

async fn thread_route(app: &Arc<App>, thread: &str, rest: &[String], req: Request) -> Answer {
    let method = req.method().clone();
    match (method, rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
        (Method::GET, ["messages"]) => {
            let limit = query_param(&req, "limit").and_then(|n| n.parse().ok()).unwrap_or(50usize).min(500);
            let before = query_param(&req, "before")
                .map(urlencoding_decode)
                .and_then(|b| wire::parse_message_id(&b));
            let (messages, more) = app.try_store(|store| wire::page(store, thread, before, limit)).map_err(fail)?;
            let (latest, _) = app.try_store(|store| store.page(thread, None, 1)).map_err(fail)?;
            let leaf = latest.first().map(|m| json!(wire::message_id(m.id))).unwrap_or(Value::Null);
            Ok((StatusCode::OK, json!({ "messages": messages, "hasMore": more, "activeLeafId": leaf })))
        }
        (Method::POST, ["images", message]) => {
            let body = json_body(req).await?;
            images::answer(app, thread, &urlencoding_decode(message), &body).await
        }
        (Method::GET, ["changes", message]) => changes::route(app, thread, &urlencoding_decode(message), None).await,
        (Method::POST, ["changes", message, action]) => changes::route(app, thread, &urlencoding_decode(message), Some(action)).await,
        (Method::POST, ["messages", message, "reactions"]) => {
            let message = urlencoding_decode(message);
            reactions::react(app, thread, &message, &json_body(req).await?)
        }
        (Method::POST, ["respond"]) => {
            let body = json_body(req).await?;
            let request_id = body["requestId"].as_str().ok_or_else(|| bad("which request?"))?;
            let decision = match body["behavior"].as_str() {
                Some("allow") => Decision::Allow,
                Some("deny") => Decision::Deny(match body["message"].as_str().filter(|m| !m.trim().is_empty()) {
                    Some(said) => format!("The person refused this step and said: {said}"),
                    None => "The person refused this step.".into(),
                }),
                other => return Err(bad(format!("unknown answer {other:?}"))),
            };
            let behavior = if decision == Decision::Allow { "allow" } else { "deny" };
            let turn_id = app
                .turns
                .answer(thread, request_id, decision)
                .ok_or((StatusCode::NOT_FOUND, "that question was already answered, or its turn is over".to_string()))?;
            app.hub.send(hub::runtime(thread, &turn_id, json!({ "type": "request.resolved", "requestId": request_id, "behavior": behavior, "source": "user" })));
            turns::settle_card(app, thread, request_id, behavior, false);
            if let Some(bot) = app.try_store(|store| store.conversation(thread)).ok().flatten() {
                app.announce_bot(&bot.bot_id);
            }
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        (method, rest) => {
            eprintln!("catbot: {method} /api/threads/<catbot>/{} isn't something Catbot does yet", rest.join("/"));
            Err((StatusCode::NOT_FOUND, format!("Catbot can't do this yet ({method} {})", rest.join("/"))))
        }
    }
}

/// Decodes %XX escapes (message ids only ever need these).
fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
