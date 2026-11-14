//! `catbot serve`: the server the interface talks to.
//!
//! Catbot and the reference server run side by side, split by bot. Bots made in Catbot are
//! answered here, from Catbot's own store and Claude driver; everything else (their bots,
//! settings, routines…) is passed through to the reference server unchanged. The bot list and
//! the live event stream are merged, so the interface sees one team. Over time Catbot answers
//! more, until the reference server has nothing left to do.

pub mod events;
pub mod hub;
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
    store::{Bot, NewBot, Role, Store},
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct Options {
    pub listen: SocketAddr,
    pub upstream: String,
    pub home: PathBuf,
    pub codex: PathBuf,
    pub cli: Vec<String>,
    /// Record the traffic passed to the reference server into this folder.
    pub record: Option<PathBuf>,
    /// Catbot's own interface (the `dx` build's `public` folder), served at `/`.
    pub ui: Option<PathBuf>,
}

pub struct App {
    store: Mutex<Store>,
    pub hub: hub::Hub,
    pub turns: turns::Turns,
    pub proxy: Arc<Proxy>,
    pub codex: PathBuf,
    pub cli: Vec<String>,
    /// The approval helper (this program), so every turn can ask before it acts.
    pub helper: Option<Vec<String>>,
    /// Catbot's own interface, when built: served for every path outside `/api`.
    pub ui: Option<PathBuf>,
}

impl App {
    pub fn new(
        store: Store,
        proxy: Arc<Proxy>,
        codex: PathBuf,
        cli: Vec<String>,
        helper: Option<Vec<String>>,
        ui: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self { store: Mutex::new(store), hub: hub::Hub::default(), turns: turns::Turns::default(), proxy, codex, cli, helper, ui })
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

    /// Sends a bot's current state (no messages) to every open interface.
    pub fn announce_bot(&self, bot_id: &str) {
        let wire = self.with_store(|store| {
            let Some(bot) = store.bot_by_id(bot_id)? else { return Ok(None) };
            let thread = store.ensure_conversation(&bot.id)?;
            Ok(Some(wire::bot(store, &bot, self.turns.activity(&thread.id), None)?))
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
    let app = App::new(store, proxy, options.codex, options.cli, helper, ui.clone());

    let listener = tokio::net::TcpListener::bind(options.listen)
        .await
        .map_err(|err| format!("could not listen on {}: {err} (is something else using that port?)", options.listen))?;
    eprintln!("catbot: serving http://{} ({bots} Catbot bot(s) in {})", options.listen, options.home.display());
    eprintln!("catbot: everything else goes to the reference server at {}", app.proxy.upstream());
    match &ui {
        Some(dir) => eprintln!("catbot: Catbot's interface: http://{} (from {})", options.listen, dir.display()),
        None => eprintln!("catbot: OpenMausBot's interface: start it with OMB_PORT={} pnpm dev", options.listen.port()),
    }

    axum::serve(listener, router(app)).await?;
    Ok(())
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
        ["api", "events"] if method == Method::GET => events::events(app, req).await,
        ["api", "bots"] if method == Method::GET => list_bots(&app, req).await,
        ["api", "bots"] if method == Method::POST => respond(create_bot(&app, req).await),
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
    app.with_store(|store| Ok(store.bot_by_id(id)?.is_some())).unwrap_or(false)
}

fn owns_thread(app: &App, id: &str) -> bool {
    app.with_store(|store| Ok(store.conversation(id)?.is_some())).unwrap_or(false)
}

/// An answer: a status and a JSON body.
type Answer = Result<(StatusCode, Value), (StatusCode, String)>;

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
            let thread = store.ensure_conversation(&bot.id)?;
            wires.push(wire::bot(store, &bot, app.turns.activity(&thread.id), Some(limit))?);
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
    let wire = app.try_store(|store| wire::bot(store, &bot, "idle", Some(50))).map_err(fail)?;
    app.announce_bot(&bot.id);
    eprintln!("catbot: made {} ({})", bot.name, bot.folder.display());
    Ok((StatusCode::CREATED, json!({ "bot": wire })))
}

async fn bot_route(app: &Arc<App>, id: &str, rest: &[String], req: Request) -> Answer {
    let method = req.method().clone();
    let bot = app.try_store(|store| store.bot_by_id(id)).map_err(fail)?.ok_or((StatusCode::NOT_FOUND, "no such bot".into()))?;
    match (method, rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
        (Method::GET, []) => {
            let thread = app.try_store(|store| store.ensure_conversation(&bot.id)).map_err(fail)?;
            let wire = app.try_store(|store| wire::bot(store, &bot, app.turns.activity(&thread.id), Some(50))).map_err(fail)?;
            Ok((StatusCode::OK, json!({ "bot": wire })))
        }
        (Method::PATCH, []) => patch_bot(app, bot, json_body(req).await?),
        (Method::DELETE, []) => {
            if let Some(thread) = app.try_store(|store| store.current_conversation(&bot.id)).map_err(fail)? {
                app.turns.interrupt(&thread.id);
            }
            let bot = app.try_store(|store| store.delete_bot(&bot.name)).map_err(fail)?;
            app.hub.send(json!({ "kind": "bot.deleted", "botId": bot.id }));
            eprintln!("catbot: deleted {} (its folder stays at {})", bot.name, bot.folder.display());
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        (Method::POST, ["messages"]) => send_message(app, bot, json_body(req).await?),
        (Method::POST, ["read"]) => Ok((StatusCode::OK, json!({ "ok": true }))),
        (Method::POST, ["interrupt"]) => {
            let body = json_body(req).await?;
            let thread = match body["threadId"].as_str() {
                Some(thread) => thread.to_string(),
                None => app.try_store(|store| store.ensure_conversation(&bot.id)).map_err(fail)?.id,
            };
            Ok((StatusCode::OK, json!({ "ok": true, "stopped": app.turns.interrupt(&thread) })))
        }
        (method, rest) => {
            eprintln!("catbot: {method} /api/bots/<{}>/{} isn't something Catbot bots do yet", bot.name, rest.join("/"));
            Err((StatusCode::NOT_FOUND, format!("Catbot bots can't do this yet ({method} {})", rest.join("/"))))
        }
    }
}

fn patch_bot(app: &Arc<App>, mut bot: Bot, body: Value) -> Answer {
    if let Some(name) = body["name"].as_str() {
        bot.name = name.trim().to_string();
    }
    if let Some(soul) = body["soul"].as_str() {
        bot.instructions = soul.trim().to_string();
    }
    if let Some(selection) = body.get("modelSelection").filter(|s| s.is_object()) {
        let instance = selection["instanceId"].as_str().unwrap_or(wire::INSTANCE);
        if !instance.is_empty() && instance != wire::INSTANCE {
            return Err(bad("Catbot bots run on Claude for now"));
        }
        bot.model = selection["model"].as_str().map(str::to_string).filter(|m| !m.is_empty());
    }
    if let Some(level) = body["approvalMode"].as_str() {
        bot.mode = wire::mode_from_approval(level).ok_or_else(|| bad(format!("unknown approval level {level}")))?;
    }
    app.try_store(|store| store.update_bot(&bot)).map_err(bad)?;
    let thread = app.try_store(|store| store.ensure_conversation(&bot.id)).map_err(fail)?;
    let wire = app.try_store(|store| wire::bot(store, &bot, app.turns.activity(&thread.id), None)).map_err(fail)?;
    app.hub.send(json!({ "kind": "bot", "bot": wire }));
    Ok((StatusCode::OK, json!({ "bot": wire })))
}

/// `POST /api/bots/:id/messages`: keep the person's message, show it everywhere, start the turn.
fn send_message(app: &Arc<App>, bot: Bot, body: Value) -> Answer {
    let text = body["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("nothing to send"))?.to_string();
    let conversation = app
        .try_store(|store| {
            let wanted = body["threadId"].as_str().map(|id| store.conversation(id)).transpose()?.flatten();
            match wanted.filter(|c| c.bot_id == bot.id) {
                Some(conversation) => Ok(conversation),
                None => store.ensure_conversation(&bot.id),
            }
        })
        .map_err(fail)?;
    if app.turns.busy(&conversation.id) {
        return Err((StatusCode::CONFLICT, "this bot is still working on the last message".into()));
    }
    let detail = body["sendId"].as_str().map(|id| json!({ "sendId": id }));
    let message = turns::save_and_send(app, &conversation.id, Role::Person, &text, detail.as_ref())
        .ok_or_else(|| fail("couldn't save the message"))?;
    let request_id = message["id"].as_str().and_then(wire::parse_message_id).unwrap_or_default();
    let thread = conversation.id.clone();
    turns::start(app.clone(), bot, conversation, request_id, text).map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
    Ok((StatusCode::ACCEPTED, json!({ "ok": true, "threadId": thread, "message": message })))
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
