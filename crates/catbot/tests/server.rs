//! `catbot serve` end to end: a fake reference server behind it, the stand-in `claude` under it,
//! and plain HTTP calls in front, the same calls the interface makes.

use std::{convert::Infallible, net::SocketAddr, time::Duration};

use axum::{Router, body::Body, response::Response, routing::{get, post}};
use bytes::Bytes;
use catbot::{proxy::Proxy, server, store::Store};
use futures_util::stream;
use http_body_util::{BodyExt, Full};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde_json::{Value, json};

const FAKE_CLAUDE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.py");

/// A stand-in for the reference server: one bot of its own, a config, and a live stream that
/// says hello and sends one frame.
fn reference_server() -> Router {
    Router::new()
        .route("/api/bots", get(|| async {
            axum::Json(json!({ "bots": [{ "id": "their-bot", "name": "MausBot", "threadId": "their-thread" }],
                "botQueuedMessages": {}, "sections": [], "groups": [], "computerControl": { "their-bot": { "held": false } } }))
        }))
        .route("/api/config", get(|| async { axum::Json(json!({ "from": "reference" })) }))
        .route("/api/bots/their-bot/read", post(|| async { axum::Json(json!({ "read": "by reference" })) }))
        .route("/api/events", get(|| async {
            let frames = [
                "data: {\"kind\":\"hello\",\"cursor\":\"ref:1\",\"resumed\":false}\n\n".to_string(),
                "id: ref:2\ndata: {\"kind\":\"notify\",\"notification\":{\"title\":\"from reference\"}}\n\n".to_string(),
            ];
            let body = stream::iter(frames.map(|f| Ok::<_, Infallible>(Bytes::from(f))))
                .chain(stream::pending());
            Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(body)).unwrap()
        }))
}

use futures_util::StreamExt;

/// A stand-in interface build: an index page and one asset.
fn ui_dir(base: &std::path::Path) -> std::path::PathBuf {
    let ui = base.join("ui");
    std::fs::create_dir_all(ui.join("assets")).unwrap();
    std::fs::write(ui.join("index.html"), "<!doctype html><title>Catbot</title>").unwrap();
    std::fs::write(ui.join("assets/app.css"), "body{}").unwrap();
    ui
}

struct Setup {
    base: String,
    /// The remote door.
    away: String,
    /// The bots' memory vault.
    memory: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

async fn listen(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

async fn setup() -> Setup {
    let reference = listen(reference_server()).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("home")).unwrap();
    let proxy = Proxy::new(&format!("http://{reference}"), None, 16 * 1024).unwrap();
    let memory = dir.path().join("memory");
    catbot::memory::prepare(&memory, None).unwrap();
    let app = server::App::new(
        store,
        proxy,
        dir.path().join("no-codex"),
        Some(memory.clone()),
        vec!["python3".into(), FAKE_CLAUDE.into()],
        Some(vec![env!("CARGO_BIN_EXE_catbot").into()]),
        Some(ui_dir(dir.path())),
    );
    let addr = listen(server::router(app.clone())).await;
    app.local.set(addr).unwrap();
    let away = listen(server::remote::router(app.clone())).await;
    app.remote.set(away).unwrap();
    Setup { base: format!("http://{addr}"), away: format!("http://{away}"), memory, _dir: dir }
}

impl Setup {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.send(&self.base, None, method, path, body).await
    }

    /// A call at the remote door, with a device's key if given.
    async fn away(&self, key: Option<&str>, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.send(&self.away, key, method, path, body).await
    }

    async fn send(&self, base: &str, key: Option<&str>, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let body = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
        let mut req = hyper::Request::builder().method(method).uri(format!("{base}{path}")).header("content-type", "application/json");
        if let Some(key) = key {
            req = req.header("authorization", format!("Bearer {key}"));
        }
        let req = req.body(Full::new(body)).unwrap();
        let resp = client.request(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Uploads a file as an attachment to a bot, the way the interface does.
    async fn upload(&self, bot: &str, name: &str, mime: &str, bytes: &[u8]) -> (u16, Value) {
        let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let req = hyper::Request::post(format!("{}/api/bots/{bot}/attachments", self.base))
            .header("content-type", mime)
            .header("x-file-name", name.replace(' ', "%20"))
            .body(Full::new(Bytes::from(bytes.to_vec())))
            .unwrap();
        let resp = client.request(req).await.unwrap();
        let status = resp.status().as_u16();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// A GET whose body isn't JSON: its status, type and bytes.
    async fn raw(&self, path: &str) -> (u16, String, Vec<u8>) {
        let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let resp = client.request(hyper::Request::get(format!("{}{path}", self.base)).body(Full::new(Bytes::new())).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let kind = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        (status, kind, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    /// A GET at either door, with no key: its status, its Content-Security-Policy, and its text.
    async fn page(&self, away: bool, path: &str) -> (u16, String, String) {
        let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let base = if away { &self.away } else { &self.base };
        let resp = client.request(hyper::Request::get(format!("{base}{path}")).body(Full::new(Bytes::new())).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let csp = resp.headers().get("content-security-policy").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        (status, csp, String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).to_string())
    }

    /// Opens the live stream; frames arrive on the returned channel.
    async fn events(&self) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
        self.stream(&self.base, None).await
    }

    /// Opens the live stream at the remote door; the channel closes when the stream ends.
    async fn events_away(&self, key: &str) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
        self.stream(&self.away, Some(key)).await
    }

    async fn stream(&self, base: &str, key: Option<&str>) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
        let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let mut req = hyper::Request::get(format!("{base}/api/events?window=1"));
        if let Some(key) = key {
            req = req.header("authorization", format!("Bearer {key}"));
        }
        let req = req.body(Full::new(Bytes::new())).unwrap();
        let mut body = client.request(req).await.unwrap().into_body();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut buf = String::new();
            while let Some(Ok(frame)) = body.frame().await {
                let Some(chunk) = frame.data_ref() else { continue };
                buf.push_str(&String::from_utf8_lossy(chunk));
                while let Some(end) = buf.find("\n\n") {
                    let block: String = buf.drain(..end + 2).collect();
                    if let Some(data) = block.lines().find_map(|l| l.strip_prefix("data: "))
                        && let Ok(value) = serde_json::from_str(data)
                        && tx.send(value).is_err()
                    {
                        return;
                    }
                }
            }
        });
        rx
    }
}

/// Waits for the first frame that matches.
async fn until(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = rx.recv().await.expect("stream ended");
            if matches(&frame) {
                return frame;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {what} frame"))
}

/// Waits for several frames that may come in any order (a chat's name can arrive before or
/// after its turn ends).
type Wanted<'a> = Vec<(&'a str, Box<dyn Fn(&Value) -> bool>)>;

async fn until_all(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, wanted: Wanted<'_>) {
    let mut left = wanted;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !left.is_empty() {
            let frame = rx.recv().await.expect("stream ended");
            left.retain(|(_, matches)| !matches(&frame));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {} frame", left.iter().map(|(what, _)| *what).collect::<Vec<_>>().join(", ")));
}

fn runtime(frame: &Value, kind: &str) -> bool {
    frame["kind"] == "runtime" && frame["event"]["type"] == kind
}

#[tokio::test]
async fn the_bot_list_has_both_teams() {
    let s = setup().await;
    let (status, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    assert_eq!(status, 201);
    assert_eq!(made["bot"]["name"], "Scout");
    let (_, list) = s.call("GET", "/api/bots?messages=50", None).await;
    let names: Vec<&str> = list["bots"].as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["MausBot", "Scout"]);
    let scout_id = made["bot"]["id"].as_str().unwrap();
    assert!(list["computerControl"][scout_id].is_object());
    assert!(list["computerControl"]["their-bot"].is_object());
}

#[tokio::test]
async fn everything_else_goes_to_the_reference_server() {
    let s = setup().await;
    assert_eq!(s.call("GET", "/api/config", None).await.1, json!({ "from": "reference" }));
    assert_eq!(s.call("POST", "/api/bots/their-bot/read", Some(json!({}))).await.1, json!({ "read": "by reference" }));
}

#[tokio::test]
async fn a_turn_streams_asks_and_is_kept() {
    let s = setup().await;
    let mut events = s.events().await;
    let hello = until(&mut events, "hello", |f| f["kind"] == "hello").await;
    assert_eq!(hello["resumed"], false);
    until(&mut events, "reference", |f| f["kind"] == "notify").await;

    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    let (status, sent) = s
        .call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "approve", "threadId": thread, "sendId": "send-1" })))
        .await;
    assert_eq!(status, 202);
    assert_eq!(sent["message"]["sendId"], "send-1");

    let asked = until(&mut events, "request.opened", |f| runtime(f, "request.opened")).await;
    let request = asked["event"]["requestId"].as_str().unwrap().to_string();
    assert_eq!(asked["event"]["tool"], "Bash");
    let card = until(&mut events, "card", |f| f["kind"] == "message" && f["message"]["kind"] == "options").await;
    assert_eq!(card["message"]["card"]["requestId"], request.as_str());
    assert_eq!(card["message"]["card"]["options"], json!(["Allow", "Deny"]));
    until(&mut events, "waiting bot", |f| f["kind"] == "bot" && f["bot"]["activity"] == "waiting-on-you").await;
    assert_eq!(s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "x" }))).await.0, 409);

    let (status, _) = s.call("POST", &format!("/api/threads/{thread}/respond"), Some(json!({ "requestId": request, "behavior": "allow" }))).await;
    assert_eq!(status, 200);
    until(&mut events, "request.resolved", |f| runtime(f, "request.resolved")).await;
    let settled = until(&mut events, "card answered", |f| f["kind"] == "message.patch").await;
    assert_eq!(settled["message"]["card"]["answered"], "allow");
    assert_eq!(settled["message"]["card"]["dismissed"], false);
    let delta = until(&mut events, "content.delta", |f| runtime(f, "content.delta")).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["kind"] == "text" && f["message"]["role"] == "bot").await;
    assert_eq!(reply["message"]["text"], "Hello, approve");
    assert_eq!(reply["message"]["itemId"], delta["event"]["itemId"], "the kept reply replaces the streamed one");
    let done = until(&mut events, "turn.completed", |f| runtime(f, "turn.completed")).await;
    assert_eq!(done["event"]["ok"], true);
    until(&mut events, "idle bot", |f| f["kind"] == "bot" && f["bot"]["busy"] == false).await;

    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=10"), None).await;
    let kept: Vec<String> = page["messages"].as_array().unwrap().iter().map(describe).collect();
    assert_eq!(kept, ["user approve", "card Approval needed: Bash echo hi → allow", "bot Bash echo hi", "bot Hello, approve"]);
    assert_eq!(page["activeLeafId"], page["messages"][3]["id"]);
}

#[tokio::test]
async fn stopping_closes_the_card_and_says_so() {
    let s = setup().await;
    let mut events = s.events().await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "approve" }))).await;
    let asked = until(&mut events, "request.opened", |f| runtime(f, "request.opened")).await;
    let (_, stopped) = s.call("POST", &format!("/api/bots/{bot}/interrupt"), Some(json!({ "threadId": thread }))).await;
    assert_eq!(stopped["stopped"], true);
    let closed = until(&mut events, "request.resolved", |f| runtime(f, "request.resolved")).await;
    assert_eq!(closed["event"]["requestId"], asked["event"]["requestId"]);
    assert_eq!(closed["event"]["source"], "system");
    let done = until(&mut events, "turn.completed", |f| runtime(f, "turn.completed")).await;
    assert_eq!(done["event"]["stopReason"], "interrupted");
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages"), None).await;
    let kept: Vec<String> = page["messages"].as_array().unwrap().iter().map(describe).collect();
    assert_eq!(kept, ["user approve", "card Approval needed: Bash echo hi → deny (dismissed)", "bot Bash echo hi", "bot notice: Stopped."]);
}

/// A message as one line: who, and what it says or shows.
fn describe(m: &Value) -> String {
    match m["kind"].as_str() {
        Some("options") => {
            let card = &m["card"];
            let dismissed = if card["dismissed"] == true { " (dismissed)" } else { "" };
            format!("card {}: {} → {}{dismissed}", card["title"].as_str().unwrap(), card["subtitle"].as_str().unwrap(), card["answered"].as_str().unwrap_or("open"))
        }
        _ => format!("{} {}", m["role"].as_str().unwrap(), m["text"].as_str().or(m["tool"]["name"].as_str()).unwrap()),
    }
}

#[tokio::test]
async fn bots_are_renamed_retuned_and_deleted() {
    let s = setup().await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({}))).await;
    assert_eq!(made["bot"]["name"], "New bot");
    assert!(s.memory.join("Bots/New bot.md").is_file(), "a new bot gets its memory note");
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let (status, patched) = s
        .call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "name": "Ranger", "approvalMode": "edits", "soul": "Scout.", "modelSelection": { "instanceId": "claude", "model": "claude-opus-5-5" } })))
        .await;
    assert_eq!(status, 200);
    assert_eq!(patched["bot"]["name"], "Ranger");
    assert!(!s.memory.join("Bots/New bot.md").exists());
    assert!(std::fs::read_to_string(s.memory.join("Bots/Ranger.md")).unwrap().contains("# Ranger"), "the note follows the name");
    assert_eq!(patched["bot"]["approvalMode"], "edits");
    assert_eq!(patched["bot"]["modelSelection"]["model"], "claude-opus-5-5");
    let (status, _) = s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "codex", "model": "gpt" } }))).await;
    assert_eq!(status, 400, "Catbot bots run on Claude");
    assert_eq!(s.call("GET", &format!("/api/bots/{bot}/activity"), None).await.0, 404);
    assert_eq!(s.call("DELETE", &format!("/api/bots/{bot}"), None).await.0, 200);
    let (_, list) = s.call("GET", "/api/bots", None).await;
    assert_eq!(list["bots"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn pictures_colours_and_codex() {
    let s = setup().await;
    let (status, made) = s
        .call("POST", "/api/bots", Some(json!({ "name": "Fern", "soul": "Gardens.", "approvalMode": "edits", "mark": "cat-sitting", "color": "teal", "codex": false })))
        .await;
    assert_eq!(status, 201);
    let bot = &made["bot"];
    assert_eq!((bot["name"].as_str(), bot["soul"].as_str()), (Some("Fern"), Some("Gardens.")));
    assert_eq!((bot["mark"].as_str(), bot["color"].as_str()), (Some("cat-sitting"), Some("teal")));
    assert_eq!((bot["approvalMode"].as_str(), bot["codex"].as_bool()), (Some("edits"), Some(false)));
    let id = bot["id"].as_str().unwrap().to_string();

    let (_, patched) = s.call("PATCH", &format!("/api/bots/{id}"), Some(json!({ "mark": "letter", "color": null, "codex": true }))).await;
    assert_eq!(patched["bot"]["mark"], "letter");
    assert!(patched["bot"]["color"].as_str().is_some(), "back to the colour it gets from its id");
    assert_eq!(patched["bot"]["codex"], true);
    for wrong in [json!({ "mark": "mouse" }), json!({ "color": "plaid" }), json!({ "color": 7 })] {
        assert_eq!(s.call("PATCH", &format!("/api/bots/{id}"), Some(wrong.clone())).await.0, 400, "{wrong}");
    }
    let (_, list) = s.call("GET", "/api/bots", None).await;
    let fern = list["bots"].as_array().unwrap().iter().find(|b| b["id"] == id.as_str()).unwrap().clone();
    assert_eq!(fern["mark"], "letter");
}

#[tokio::test]
async fn the_interface_is_served_beside_the_api() {
    let s = setup().await;
    let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
    let get = |path: &str| {
        let req = hyper::Request::get(format!("{}{path}", s.base)).body(Full::new(Bytes::new())).unwrap();
        let client = client.clone();
        async move {
            let resp = client.request(req).await.unwrap();
            let status = resp.status().as_u16();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&body).into_owned())
        }
    };
    let (status, page) = get("/").await;
    assert_eq!(status, 200);
    assert!(page.contains("<title>Catbot</title>"));
    assert_eq!(get("/assets/app.css").await, (200, "body{}".to_string()));
    // A page address with no file of its own gets the app, which routes it.
    assert!(get("/bots/some-bot").await.1.contains("<title>Catbot</title>"));
    // The API is still the API.
    assert_eq!(s.call("GET", "/api/config", None).await.1, json!({ "from": "reference" }));
}

/// The bot's chat list from a bot frame or answer: (title, archived) by chat id.
fn chats(bot: &Value) -> Vec<(String, String, bool)> {
    bot["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["threadId"].as_str().unwrap().to_string(), t["title"].as_str().unwrap().to_string(), t.get("archivedAt").is_some()))
        .collect()
}

fn title_of(bot: &Value, thread: &str) -> String {
    chats(bot).into_iter().find(|(id, _, _)| id == thread).map(|(_, title, _)| title).unwrap_or_default()
}

#[tokio::test]
async fn chats_are_named_switched_and_tidied() {
    let s = setup().await;
    let mut events = s.events().await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Kiwi" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let first = made["bot"]["threadId"].as_str().unwrap().to_string();
    assert_eq!(title_of(&made["bot"], &first), "New chat");

    // The first message names the chat at once, then the namer's better name replaces it.
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Plan the garden beds\nwith raised boxes", "threadId": first }))).await;
    until(&mut events, "first-line name", |f| f["kind"] == "bot" && title_of(&f["bot"], &first) == "Plan the garden beds").await;
    let (named, done) = (first.clone(), first.clone());
    until_all(
        &mut events,
        vec![
            ("generated name", Box::new(move |f: &Value| f["kind"] == "bot" && title_of(&f["bot"], &named) == "Plans for the garden")),
            ("first turn done", Box::new(move |f: &Value| runtime(f, "turn.completed") && f["event"]["threadId"] == done.as_str())),
        ],
    )
    .await;

    // A second chat: the bot opens to it, and the first is still listed.
    let (status, created) = s.call("POST", &format!("/api/bots/{bot}/tasks"), Some(json!({}))).await;
    assert_eq!(status, 201);
    let second = created["task"]["threadId"].as_str().unwrap().to_string();
    assert_ne!(second, first);
    assert_eq!(created["bot"]["threadId"], second.as_str());
    assert_eq!(chats(&created["bot"]).len(), 2);

    // Two chats work at once: the first waits for an approval while the second answers.
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "approve", "threadId": first }))).await;
    let asked = until(&mut events, "request.opened", |f| runtime(f, "request.opened")).await;
    assert_eq!(asked["event"]["threadId"], first.as_str());
    let (status, _) = s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "sleepy hello", "threadId": second }))).await;
    assert_eq!(status, 202, "a second chat isn't held up by the first");
    let done = until(&mut events, "second chat done", |f| runtime(f, "turn.completed")).await;
    assert_eq!(done["event"]["threadId"], second.as_str());
    let (_, listed) = s.call("GET", &format!("/api/bots/{bot}"), None).await;
    assert_eq!(listed["bot"]["activity"], "waiting-on-you", "the bot shows its busiest chat");
    let first_task = listed["bot"]["tasks"].as_array().unwrap().iter().find(|t| t["threadId"] == first.as_str()).unwrap().clone();
    assert_eq!(first_task["activity"], "waiting-on-you");

    // A chat can't be deleted while it's working.
    assert_eq!(s.call("DELETE", &format!("/api/bots/{bot}/tasks/{first}"), None).await.0, 409);
    let request = asked["event"]["requestId"].as_str().unwrap();
    s.call("POST", &format!("/api/threads/{first}/respond"), Some(json!({ "requestId": request, "behavior": "deny" }))).await;
    until(&mut events, "first chat done", |f| runtime(f, "turn.completed") && f["event"]["threadId"] == first.as_str()).await;

    // The person's name wins over the namer's, even one that arrives later ("sleepy" is slow).
    let (status, renamed) = s.call("PATCH", &format!("/api/bots/{bot}/tasks/{second}"), Some(json!({ "title": "Shed" }))).await;
    assert_eq!(status, 200);
    assert_eq!(renamed["task"]["title"], "Shed");
    assert_eq!(s.call("PATCH", &format!("/api/bots/{bot}/tasks/{second}"), Some(json!({ "title": "  " }))).await.0, 400);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (_, listed) = s.call("GET", &format!("/api/bots/{bot}"), None).await;
    assert_eq!(title_of(&listed["bot"], &second), "Shed");

    // Opening a chat: the bot opens to it, with its messages.
    let (_, opened) = s.call("POST", &format!("/api/bots/{bot}/tasks/{first}?messages=50"), None).await;
    assert_eq!(opened["bot"]["threadId"], first.as_str());
    assert_eq!(opened["bot"]["messages"][0]["text"], "Plan the garden beds\nwith raised boxes");

    // Archived and back.
    s.call("PATCH", &format!("/api/bots/{bot}/tasks/{second}"), Some(json!({ "archivedAt": 1 }))).await;
    let (_, listed) = s.call("GET", &format!("/api/bots/{bot}"), None).await;
    assert!(chats(&listed["bot"]).iter().any(|(id, _, archived)| id == &second && *archived));
    s.call("PATCH", &format!("/api/bots/{bot}/tasks/{second}"), Some(json!({ "archivedAt": null }))).await;
    let (_, listed) = s.call("GET", &format!("/api/bots/{bot}"), None).await;
    assert!(chats(&listed["bot"]).iter().all(|(_, _, archived)| !archived));

    // Deleting the open chat: it's gone, with its messages, and the bot opens to the other.
    let (status, after) = s.call("DELETE", &format!("/api/bots/{bot}/tasks/{first}"), None).await;
    assert_eq!(status, 200);
    assert_eq!(after["bot"]["threadId"], second.as_str());
    assert_eq!(chats(&after["bot"]).len(), 1);
    assert_eq!(s.call("GET", &format!("/api/threads/{first}/messages"), None).await.0, 404);
    // Another bot's chat isn't this bot's to touch.
    let (_, other) = s.call("POST", "/api/bots", Some(json!({ "name": "Other" }))).await;
    let other_thread = other["bot"]["threadId"].as_str().unwrap();
    assert_eq!(s.call("DELETE", &format!("/api/bots/{bot}/tasks/{other_thread}"), None).await.0, 404);
}

#[tokio::test]
async fn devices_pair_at_home_and_reach_catbot_from_away() {
    let s = setup().await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Kiwi" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();

    // Nothing at the remote door without a key, and no pairing without a code waiting.
    assert_eq!(s.away(None, "GET", "/api/bots", None).await.0, 401);
    assert_eq!(s.away(Some(&"0".repeat(64)), "GET", "/api/bots", None).await.0, 401);
    assert_eq!(s.away(None, "GET", "/api/events", None).await.0, 401);
    assert_eq!(s.away(None, "POST", "/api/pair", Some(json!({ "code": "ABCDEFGHJK", "name": "x" }))).await.0, 401);

    // At home: a code, good for a while and five tries.
    let (status, overview) = s.call("POST", "/api/devices/pairing", None).await;
    assert_eq!(status, 200);
    let code = overview["pairing"]["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 10);
    assert!(overview["pairing"]["secondsLeft"].as_u64().unwrap() > 500);
    assert_eq!(overview["remote"], s.away.trim_start_matches("http://"));

    // A wrong code counts against it; the right one (any case, with dashes) pairs, once.
    assert_eq!(s.away(None, "POST", "/api/pair", Some(json!({ "code": "WRONGCODE2", "name": "x" }))).await.0, 401);
    let typed = format!("{}-{}", &code[..5], &code[5..]).to_lowercase();
    let (status, paired) = s.away(None, "POST", "/api/pair", Some(json!({ "code": typed, "name": "Moo's phone" }))).await;
    assert_eq!(status, 201, "{paired}");
    let key = paired["key"].as_str().unwrap().to_string();
    assert_eq!(key.len(), 64);
    assert_eq!(s.away(None, "POST", "/api/pair", Some(json!({ "code": code, "name": "again" }))).await.0, 401, "a code works once");

    // With its key, the phone has Catbot's own routes, and nothing else.
    let (status, me) = s.away(Some(&key), "GET", "/api/device", None).await;
    assert_eq!((status, me["device"]["name"].as_str()), (200, Some("Moo's phone")));
    let (_, list) = s.away(Some(&key), "GET", "/api/bots", None).await;
    let names: Vec<&str> = list["bots"].as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Kiwi"], "the reference server's bots stay at home");
    assert_eq!(s.away(Some(&key), "GET", "/api/config", None).await.0, 404, "nothing is passed on from away");
    assert_eq!(s.away(Some(&key), "GET", "/api/devices", None).await.0, 404, "devices are managed at home");

    // The memory vault reads the same from away as at home, and only for a paired phone.
    let (status, home_list) = s.call("GET", "/api/memory", None).await;
    assert_eq!(status, 200);
    assert_eq!(s.away(None, "GET", "/api/memory", None).await.0, 401);
    let (status, away_list) = s.away(Some(&key), "GET", "/api/memory", None).await;
    assert_eq!((status, &away_list), (200, &home_list));
    assert!(away_list["notes"].as_array().unwrap().iter().any(|n| n["path"] == "Bots/Kiwi.md"));
    let (status, note) = s.away(Some(&key), "GET", "/api/memory/note?path=Bots%2FKiwi.md", None).await;
    assert_eq!(status, 200);
    assert!(note["text"].as_str().unwrap().starts_with("# Kiwi"), "{note}");
    let (_, linked) = s.away(Some(&key), "GET", "/api/memory/note?link=Kiwi&from=Home.md", None).await;
    assert_eq!(linked["path"], "Bots/Kiwi.md");
    assert_eq!(s.away(Some(&key), "GET", "/api/memory/note?path=..%2Fhome%2Fcatbot.db", None).await.0, 404);
    assert_eq!(s.away(Some(&key), "POST", "/api/memory", None).await.0, 404, "read-only");
    assert_eq!(s.away(Some(&key), "GET", "/", None).await.0, 404);
    let mut events = s.events_away(&key).await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (status, _) = s.away(Some(&key), "POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "hello from away", "threadId": thread }))).await;
    assert_eq!(status, 202);
    until(&mut events, "a turn finishing", |f| f["kind"] == "runtime" && f["event"]["type"] == "turn.completed").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                match events.recv().await {
                    Some(f) if f["kind"] == "notify" => return true,
                    Some(_) => continue,
                    None => return false,
                }
            }
        })
        .await
        .is_err(),
        "the reference server's frames stay at home"
    );

    // Home sees the device; removing it ends its key and its open stream.
    let (_, overview) = s.call("GET", "/api/devices", None).await;
    let device = overview["devices"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(overview["devices"][0]["name"], "Moo's phone");
    assert!(overview["devices"][0]["lastSeenAt"].is_string());
    assert_eq!(s.call("DELETE", &format!("/api/devices/{device}"), None).await.0, 200);
    let ended = tokio::time::timeout(Duration::from_secs(5), async { while events.recv().await.is_some() {} }).await;
    assert!(ended.is_ok(), "the removed device's stream ends");
    assert_eq!(s.away(Some(&key), "GET", "/api/bots", None).await.0, 401);
    assert_eq!(s.call("DELETE", &format!("/api/devices/{device}"), None).await.0, 404);

    // Five wrong tries and the code is gone, even for the right one after.
    let (_, overview) = s.call("POST", "/api/devices/pairing", None).await;
    let code = overview["pairing"]["code"].as_str().unwrap().to_string();
    for _ in 0..5 {
        assert_eq!(s.away(None, "POST", "/api/pair", Some(json!({ "code": "WRONGCODE2" }))).await.0, 401);
    }
    assert_eq!(s.away(None, "POST", "/api/pair", Some(json!({ "code": code }))).await.0, 401);
    let (_, overview) = s.call("GET", "/api/devices", None).await;
    assert!(overview["pairing"].is_null());
    // And a waiting code can be cancelled.
    s.call("POST", "/api/devices/pairing", None).await;
    let (_, overview) = s.call("DELETE", "/api/devices/pairing", None).await;
    assert!(overview["pairing"].is_null());
}

/// A stand-in chat API, OpenAI-style. Asked to "write", it calls write_file once; given the
/// tool's result, it answers. Every request it gets is kept, with its Authorization header.
fn fake_api(seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>>) -> Router {
    let kept = seen.clone();
    let seen_images = seen.clone();
    Router::new()
        .route("/v1/images/generations", post(move |axum::Json(body): axum::Json<Value>| {
            let seen = seen_images.clone();
            async move {
                seen.lock().unwrap().push(json!({ "images": body }));
                // A one-pixel PNG.
                axum::Json(json!({ "data": [{ "b64_json": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==" }] }))
            }
        }))
        .route("/v1/models", get(|| async { axum::Json(json!({ "data": [{ "id": "gpt-test" }, { "id": "text-embedding-3" }] })) }))
        .route("/v1/chat/completions", post(move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
            let kept = kept.clone();
            async move {
                let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                kept.lock().unwrap().push(json!({ "auth": auth, "body": body }));
                let last = body["messages"].as_array().unwrap().last().unwrap().clone();
                let chunks: Vec<Value> = if last["role"] == "tool" {
                    vec![json!({ "choices": [{ "delta": { "content": "Done: " } }] }), json!({ "choices": [{ "delta": { "content": "the plan is saved." } }] })]
                } else {
                    vec![
                        json!({ "choices": [{ "delta": { "content": "Writing it." } }] }),
                        json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call-1", "function": { "name": "write_file", "arguments": "{\"path\":\"plan.md\"," } }] } }] }),
                        json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "\"content\":\"carrots\"}" } }] } }] }),
                    ]
                };
                let mut chunks = chunks;
                if body["stream_options"]["include_usage"] == true {
                    chunks.push(json!({ "choices": [], "usage": { "prompt_tokens": 120, "completion_tokens": 30, "prompt_tokens_details": { "cached_tokens": 20 } } }));
                }
                let mut text: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
                text.push_str("data: [DONE]\n\n");
                Response::builder().header("content-type", "text/event-stream").body(Body::from(text)).unwrap()
            }
        }))
}

#[tokio::test]
async fn a_bot_on_a_chat_api_uses_its_tools_with_approval() {
    let s = setup().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let api = listen(fake_api(seen.clone())).await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;

    // The engines, with no key yet; then the key and the stand-in's address, saved at home only.
    let (_, list) = s.call("GET", "/api/engines", None).await;
    let openai = list["engines"].as_array().unwrap().iter().find(|e| e["id"] == "openai").unwrap().clone();
    assert_eq!((openai["ready"].as_bool(), openai["keyHint"].is_null()), (Some(false), true));
    assert_eq!(s.call("PUT", "/api/engines/openai/key", Some(json!({ "key": "https://not-a-key" }))).await.0, 400);
    let (status, saved) = s
        .call("PUT", "/api/engines/openai/key", Some(json!({ "key": "sk-test-1234", "url": format!("http://{api}/v1") })))
        .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!((saved["engine"]["ready"].as_bool(), saved["engine"]["keyHint"].as_str()), (Some(true), Some("…1234")));
    assert!(!saved.to_string().contains("sk-test-1234"), "the key itself never goes out");
    let (_, models) = s.call("GET", "/api/engines/openai/models", None).await;
    assert_eq!(models, json!({ "models": ["gpt-test"], "live": true }));

    // A bot moved to it.
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    assert_eq!(made["bot"]["modelSelection"]["instanceId"], "claude");
    let (status, moved) = s
        .call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "openai", "model": "gpt-test" } })))
        .await;
    assert_eq!(status, 200);
    assert_eq!(moved["bot"]["modelSelection"], json!({ "instanceId": "openai", "model": "gpt-test" }));
    assert_eq!(s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "skynet" } }))).await.0, 400);

    // It writes a file, asking first; then answers.
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "write the plan", "threadId": thread }))).await;
    let asked = until(&mut events, "request.opened", |f| runtime(f, "request.opened")).await;
    assert_eq!(asked["event"]["tool"], "Write");
    let request = asked["event"]["requestId"].as_str().unwrap().to_string();
    let plan = s._dir.path().join("home/bots/scout/plan.md");
    assert!(!plan.exists(), "nothing is written before the yes");
    s.call("POST", &format!("/api/threads/{thread}/respond"), Some(json!({ "requestId": request, "behavior": "allow" }))).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["kind"] == "text" && f["message"]["role"] == "bot").await;
    assert_eq!(reply["message"]["text"], "Writing it.\n\nDone: the plan is saved.");
    let done = until(&mut events, "turn.completed", |f| runtime(f, "turn.completed")).await;
    assert_eq!(done["event"]["ok"], true);
    assert_eq!(std::fs::read_to_string(&plan).unwrap(), "carrots");

    // What the API was sent: the key, the model, the bot's instructions and tools, the history.
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0]["auth"], "Bearer sk-test-1234");
    let first = &seen[0]["body"];
    assert_eq!(first["model"], "gpt-test");
    assert!(first["messages"][0]["content"].as_str().unwrap().starts_with("You are Scout"));
    assert_eq!(first["messages"].as_array().unwrap().last().unwrap(), &json!({ "role": "user", "content": "write the plan" }));
    assert!(first["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "run_command"));
    let second = seen[1]["body"]["messages"].as_array().unwrap();
    assert_eq!(second.last().unwrap()["role"], "tool");
    assert!(second.last().unwrap()["content"].as_str().unwrap().starts_with("Wrote"));

    // Keys are managed at home only, and can be forgotten.
    let key = {
        let (_, overview) = s.call("POST", "/api/devices/pairing", None).await;
        let code = overview["pairing"]["code"].as_str().unwrap().to_string();
        let (_, paired) = s.away(None, "POST", "/api/pair", Some(json!({ "code": code, "name": "Phone" }))).await;
        paired["key"].as_str().unwrap().to_string()
    };
    assert_eq!(s.away(Some(&key), "GET", "/api/engines", None).await.0, 200);
    assert_eq!(s.away(Some(&key), "PUT", "/api/engines/openai/key", Some(json!({ "key": "sk-other" }))).await.0, 404);
    let (_, forgot) = s.call("DELETE", "/api/engines/openai/key", None).await;
    assert_eq!(forgot["engine"]["ready"], false);
}

#[tokio::test]
async fn a_bot_on_an_agent_program_streams_asks_and_resumes() {
    // The stand-in agent, as Grok CLI.
    let bin = tempfile::tempdir().unwrap();
    let program = bin.path().join("grok");
    let log = bin.path().join("acp.log");
    std::fs::write(&program, format!("#!/bin/sh\nFAKE_ACP_LOG={} exec python3 {} \"$@\"\n", log.display(), concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-acp.py"))).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // SAFETY: only this test reads this variable.
    unsafe { std::env::set_var("CATBOT_AGENT_GROK_CLI", &program) };

    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, list) = s.call("GET", "/api/engines", None).await;
    let grok = list["engines"].as_array().unwrap().iter().find(|e| e["id"] == "grok-cli").unwrap().clone();
    assert_eq!((grok["kind"].as_str(), grok["ready"].as_bool(), grok["signIn"].as_str()), (Some("agent"), Some(true), Some("grok login")));

    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Groky", "soul": "Be brief." }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "grok-cli", "model": "grok-4" }, "approvalMode": "edits" }))).await;

    // A first message: a new session, given the bot's instructions.
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "hello", "threadId": thread }))).await;
    let thought = until(&mut events, "thinking", |f| runtime(f, "content.delta") && f["event"]["streamKind"] == "reasoning_text").await;
    assert_eq!(thought["event"]["delta"], "hmm");
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert_eq!(reply["message"]["text"], "Echo: hello (new)");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let start = std::fs::read_to_string(&log).unwrap();
    let first: Value = serde_json::from_str(start.lines().next().unwrap()).unwrap();
    assert_eq!(first["args"], json!(["--permission-mode", "acceptEdits", "agent", "-m", "grok-4", "stdio"]));
    assert!(first["cwd"].as_str().unwrap().ends_with("bots/groky"));

    // The next message carries on the agent's session.
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "again", "threadId": thread }))).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert_eq!(reply["message"]["text"], "Echo: again (resumed)");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // Asking: on "Ask for approval" an edit waits for a yes, then the file is written through Catbot.
    s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "approvalMode": "ask" }))).await;
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "please edit", "threadId": thread }))).await;
    let asked = until(&mut events, "request.opened", |f| runtime(f, "request.opened")).await;
    assert_eq!(asked["event"]["tool"], "Edit");
    let notes = s._dir.path().join("home/bots/groky/notes.md");
    assert!(!notes.exists());
    let request = asked["event"]["requestId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/threads/{thread}/respond"), Some(json!({ "requestId": request, "behavior": "allow" }))).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert_eq!(reply["message"]["text"], "Edited.");
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello from the agent");
}

#[tokio::test]
async fn attachments_reach_the_bot_and_the_chat() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();

    // A picture, kept in the bot's folder.
    let picture = b"\x89PNG\r\n\x1a\nfake picture";
    let (status, up) = s.upload(&bot, "garden plan.png", "image/png", picture).await;
    assert_eq!(status, 201, "{up}");
    let attachment = up["attachment"].clone();
    assert_eq!((attachment["name"].as_str(), attachment["mime"].as_str()), (Some("garden plan.png"), Some("image/png")));
    let file = attachment["file"].as_str().unwrap().to_string();
    assert_eq!(std::fs::read(s._dir.path().join("home/bots/scout/attachments").join(&file)).unwrap(), picture);
    let (status, kind, bytes) = s.raw(attachment["url"].as_str().unwrap()).await;
    assert_eq!((status, kind.as_str(), bytes.as_slice()), (200, "image/png", picture.as_slice()));
    assert_eq!(s.raw("/api/attachments/nope").await.0, 404);

    // Sent with a message: the chat shows it, and the bot is told where it is.
    let (status, sent) = s
        .call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "What's in this?", "threadId": thread, "attachments": [attachment["id"]] })))
        .await;
    assert_eq!(status, 202, "{sent}");
    assert_eq!(sent["message"]["attachments"][0]["id"], attachment["id"]);
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    let said = reply["message"]["text"].as_str().unwrap();
    assert!(said.contains(&format!("attachments/{file} (a picture)")), "{said}");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=10"), None).await;
    assert_eq!(page["messages"][0]["attachments"][0]["name"], "garden plan.png");
    assert_eq!(page["messages"][0]["text"], "What's in this?", "the words are kept as typed");

    // Someone else's attachment, or none and no words, is refused.
    let (_, other) = s.call("POST", "/api/bots", Some(json!({ "name": "Kiwi" }))).await;
    let other = other["bot"]["id"].as_str().unwrap().to_string();
    assert_eq!(s.call("POST", &format!("/api/bots/{other}/messages"), Some(json!({ "text": "x", "attachments": [attachment["id"]] }))).await.0, 400);
    assert_eq!(s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": " " }))).await.0, 400);

    // A bot on a chat API is shown the picture itself.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let api = listen(fake_api(seen.clone())).await;
    s.call("PUT", "/api/engines/openai/key", Some(json!({ "key": "sk-test", "url": format!("http://{api}/v1") }))).await;
    s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "openai", "model": "gpt-test" } }))).await;
    let (_, up) = s.upload(&bot, "leaf.png", "image/png", picture).await;
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "and this one?", "threadId": thread, "attachments": [up["attachment"]["id"]] }))).await;
    // (The stand-in answers by writing a file, which asks first: by then it has been sent everything.)
    until(&mut events, "request.opened", |f| runtime(f, "request.opened")).await;
    let seen = seen.lock().unwrap().clone();
    let last = seen[0]["body"]["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["role"], "user");
    assert!(last["content"][0]["text"].as_str().unwrap().starts_with("and this one?"));
    assert!(last["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn reactions_show_on_messages_and_reach_the_bot_once() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Plan the garden", "threadId": thread }))).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let reply_id = reply["message"]["id"].as_str().unwrap().to_string();
    let path = format!("/api/threads/{thread}/messages/{reply_id}/reactions");

    // On, shown live and when the chat is read again; twice is still once.
    let (status, answer) = s.call("POST", &path, Some(json!({ "emoji": "👍" }))).await;
    assert_eq!(status, 200, "{answer}");
    let patch = until(&mut events, "patch", |f| f["kind"] == "message.patch").await;
    assert_eq!(patch["message"]["reactions"], json!(["👍"]));
    s.call("POST", &path, Some(json!({ "emoji": "👍" }))).await;
    s.call("POST", &path, Some(json!({ "emoji": "🌱" }))).await;
    s.call("POST", &path, Some(json!({ "emoji": "🌱", "on": false }))).await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=10"), None).await;
    let shown = page["messages"].as_array().unwrap().iter().find(|m| m["id"] == reply_id.as_str()).unwrap().clone();
    assert_eq!(shown["reactions"], json!(["👍"]));
    assert!(shown.get("reactionsUntold").is_none());

    // Words, and other chats' messages, aren't reactions.
    assert_eq!(s.call("POST", &path, Some(json!({ "emoji": "nice" }))).await.0, 400);
    assert_eq!(s.call("POST", &format!("/api/threads/{thread}/messages/cb-999999/reactions"), Some(json!({ "emoji": "👍" }))).await.0, 404);

    // The next message tells the bot, once.
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Thanks", "threadId": thread }))).await;
    let next = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let said = next["message"]["text"].as_str().unwrap();
    assert!(said.contains("the person reacted: 👍 on your reply"), "{said}");
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Again", "threadId": thread }))).await;
    let again = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert!(!again["message"]["text"].as_str().unwrap().contains("reacted"));
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
}

#[tokio::test]
async fn a_chat_branches_from_a_message_and_the_bot_is_shown_it() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    let mut replies = Vec::new();
    for words in ["Tomatoes or peppers?", "Go with peppers"] {
        s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": words, "threadId": thread }))).await;
        let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
        replies.push(reply["message"]["id"].as_str().unwrap().to_string());
        until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    }

    // From the first reply: a new chat, opened, with the first exchange only.
    let (status, branched) = s.call("POST", &format!("/api/bots/{bot}/tasks/{thread}/branch"), Some(json!({ "messageId": replies[0] }))).await;
    assert_eq!(status, 201, "{branched}");
    let branch = branched["task"]["threadId"].as_str().unwrap().to_string();
    assert_ne!(branch, thread);
    assert_eq!(branched["bot"]["threadId"], branch.as_str());
    assert!(branched["task"]["title"].as_str().unwrap().ends_with("(branch)"), "{branched}");
    let (_, page) = s.call("GET", &format!("/api/threads/{branch}/messages?limit=10"), None).await;
    let texts: Vec<&str> = page["messages"].as_array().unwrap().iter().filter_map(|m| m["text"].as_str()).collect();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert_eq!(texts[0], "Tomatoes or peppers?");
    let (_, original) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=10"), None).await;
    assert_eq!(original["messages"].as_array().unwrap().iter().filter(|m| m["text"].is_string()).count(), 4, "the original is untouched");

    // The bot starts the branch fresh, shown the messages before the branch point only.
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Actually, tomatoes", "threadId": branch }))).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["threadId"] == branch.as_str() && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    let said = reply["message"]["text"].as_str().unwrap();
    assert!(said.contains("<conversation_so_far>") && said.contains("Person: Tomatoes or peppers?"), "{said}");
    assert!(!said.contains("Go with peppers"), "{said}");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // Unknown messages are refused.
    assert_eq!(s.call("POST", &format!("/api/bots/{bot}/tasks/{thread}/branch"), Some(json!({ "messageId": "cb-99999" }))).await.0, 404);
}

#[tokio::test]
async fn a_turns_file_changes_are_shown_and_undone() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    let folder = s._dir.path().join("home/bots/scout");
    let say = |words: &'static str| {
        let (s, bot, thread) = (&s, bot.clone(), thread.clone());
        async move { s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": words, "threadId": thread }))).await }
    };

    // A turn that writes nothing has no changes row.
    say("Plan the garden").await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    say("write plan.md: carrots and beans").await;
    let row = until(&mut events, "changes", |f| f["kind"] == "message" && f["message"]["kind"] == "changes").await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let changes = &row["message"]["changes"];
    assert_eq!(changes["summary"], "Added plan.md");
    assert_eq!(changes["files"][0]["how"], "added");
    assert_eq!(changes["undone"], false);
    let id = row["message"]["id"].as_str().unwrap().to_string();
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=20"), None).await;
    assert_eq!(page["messages"].as_array().unwrap().iter().filter(|m| m["kind"] == "changes").count(), 1);

    let (_, patch) = s.call("GET", &format!("/api/threads/{thread}/changes/{id}"), None).await;
    assert!(patch["patch"].as_str().unwrap().contains("+carrots and beans"), "{patch}");

    // Undo: the file is gone, the row says so, and the bot is told once.
    let (status, undone) = s.call("POST", &format!("/api/threads/{thread}/changes/{id}/undo"), Some(json!({}))).await;
    assert_eq!(status, 200, "{undone}");
    assert_eq!(undone["message"]["changes"]["undone"], true);
    assert!(!folder.join("plan.md").exists());
    assert_eq!(s.call("POST", &format!("/api/threads/{thread}/changes/{id}/undo"), Some(json!({}))).await.0, 409);
    say("Thanks").await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert!(reply["message"]["text"].as_str().unwrap().contains("the person undid your changes to your folder (Added plan.md)"), "{reply}");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // Redo puts it back.
    s.call("POST", &format!("/api/threads/{thread}/changes/{id}/redo"), Some(json!({}))).await;
    assert_eq!(std::fs::read_to_string(folder.join("plan.md")).unwrap(), "carrots and beans\n");
    assert_eq!(s.call("GET", &format!("/api/threads/{thread}/changes/cb-99999"), None).await.0, 404);
}

#[tokio::test]
async fn usage_is_recorded_broken_down_and_the_plan_read() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Plan the garden", "threadId": thread }))).await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // A Claude turn on the plan: its tokens, and no money (the plan covers it).
    let (status, by_bot) = s.call("GET", "/api/usage?period=this-month&by=bot", None).await;
    assert_eq!(status, 200, "{by_bot}");
    let row = &by_bot["rows"][0];
    assert_eq!((row["label"].as_str(), row["turns"].as_u64()), (Some("Scout"), Some(1)));
    assert!(row["newTokens"].as_u64().unwrap() > 200, "{row}");
    assert_eq!(row["cachedTokens"], 1000);
    assert!(row["costUsd"].is_null(), "{row}");

    // A turn on a chat API: the usage it reports.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let api = listen(fake_api(seen.clone())).await;
    s.call("PUT", "/api/engines/openai/key", Some(json!({ "key": "sk-test", "url": format!("http://{api}/v1") }))).await;
    s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "openai", "model": "gpt-test" } }))).await;
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Write it down", "threadId": thread }))).await;
    let ask = until(&mut events, "ask", |f| runtime(f, "request.opened")).await;
    let request = ask["event"]["requestId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/threads/{thread}/respond"), Some(json!({ "requestId": request, "behavior": "allow" }))).await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    assert_eq!(seen.lock().unwrap()[0]["body"]["stream_options"]["include_usage"], true);
    let (_, by_engine) = s.call("GET", "/api/usage?period=30d&by=engine", None).await;
    let openai = by_engine["rows"].as_array().unwrap().iter().find(|r| r["label"] == "OpenAI").unwrap().clone();
    // Two rounds of 100 new + 30 written, and 20 re-read from the cache each.
    assert_eq!((openai["newTokens"].as_u64(), openai["cachedTokens"].as_u64(), openai["estimated"].as_bool()), (Some(260), Some(40), Some(false)));
    let (_, by_day) = s.call("GET", "/api/usage?period=this-month&by=day", None).await;
    assert_eq!(by_day["total"]["turns"], 2);
    assert_eq!(s.call("GET", "/api/usage?period=forever", None).await.0, 400);

    // Claude's plan windows, read with Claude Code's sign-in.
    let plan_api = listen(Router::new().route("/usage", get(|headers: axum::http::HeaderMap| async move {
        assert_eq!(headers.get("authorization").unwrap(), "Bearer oat-test");
        axum::Json(json!({ "five_hour": { "utilization": 78.0, "resets_at": "2099-01-01T00:00:00Z" }, "seven_day": { "utilization": 61.0, "resets_at": "2099-01-02T00:00:00Z" } }))
    }))).await;
    let config = s._dir.path().join("claude-config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"oat-test","expiresAt":4102444800000}}"#).unwrap();
    // SAFETY: only this test reads these.
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &config);
        std::env::set_var("CATBOT_CLAUDE_USAGE_URL", format!("http://{plan_api}/usage"));
    }
    s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "claude", "model": "" } }))).await;
    let (_, plans) = s.call("GET", "/api/usage/plans?fresh=1", None).await;
    let claude = &plans["plans"][0];
    assert_eq!(claude["ok"], true, "{plans}");
    assert_eq!(claude["windows"][0]["usedPercent"], 78.0);
    assert!(!plans.to_string().contains("oat-test"), "the sign-in never goes out");
}

#[tokio::test]
async fn limits_stop_an_ai_and_say_until_when() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let api = listen(fake_api(seen.clone())).await;
    s.call("PUT", "/api/engines/openai/key", Some(json!({ "key": "sk-test", "url": format!("http://{api}/v1") }))).await;
    s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "openai", "model": "gpt-test" } }))).await;

    // Catbot's starting limits for an AI without a plan's own.
    let (_, status) = s.call("GET", "/api/limits", None).await;
    let openai = status["engines"].as_array().unwrap().iter().find(|e| e["engine"] == "openai").unwrap().clone();
    assert_eq!(openai["session"]["limit"], 1_000_000);
    assert!(openai["session"]["resetsAt"].is_null(), "no session yet");
    assert!(!status["engines"].as_array().unwrap().iter().any(|e| e["engine"] == "claude"), "Claude's plan has its own");

    // A small session limit and a price: one turn goes over both.
    let limits = json!({ "monthlyUsd": 0.0005, "engines": { "openai": { "session": 100, "week": 1000000, "priceIn": 2.0, "priceOut": 8.0 } } });
    assert_eq!(s.call("PUT", "/api/limits", Some(limits)).await.0, 200);
    assert_eq!(s.away(None, "PUT", "/api/limits", Some(json!({}))).await.0, 401);
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Write it down", "threadId": thread }))).await;
    let ask = until(&mut events, "ask", |f| runtime(f, "request.opened")).await;
    let request = ask["event"]["requestId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/threads/{thread}/respond"), Some(json!({ "requestId": request, "behavior": "allow" }))).await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    let (_, status) = s.call("GET", "/api/limits", None).await;
    let openai = status["engines"].as_array().unwrap().iter().find(|e| e["engine"] == "openai").unwrap().clone();
    assert_eq!(openai["session"]["used"], 260);
    assert_eq!(openai["session"]["usedPercent"], 100.0);
    assert!(openai["session"]["resetsAt"].is_string());
    // 2 × 200 in + 8 × 60 out, plus 40 cache re-reads at a tenth: per million.
    let spent = status["spend"]["usedUsd"].as_f64().unwrap();
    assert!(spent > 0.0, "{status}");

    let (status, refused) = s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Again", "threadId": thread }))).await;
    assert_eq!(status, 429, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("spend limit"), "{refused}");
    s.call("PUT", "/api/limits", Some(json!({ "engines": { "openai": { "session": 100, "week": 1000000 } } }))).await;
    let (status, refused) = s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Again", "threadId": thread }))).await;
    assert_eq!(status, 429);
    assert!(refused["error"].as_str().unwrap().contains("OpenAI is at Catbot's session limit until"), "{refused}");
    // No limit, no stop.
    s.call("PUT", "/api/limits", Some(json!({ "engines": { "openai": { "session": null, "week": null } } }))).await;
    assert_eq!(s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Again", "threadId": thread }))).await.0, 202);
}

#[tokio::test]
async fn a_backup_ai_answers_when_the_bots_own_is_at_a_limit() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let api = listen(fake_api(seen.clone())).await;
    for engine in ["openai", "xai"] {
        s.call("PUT", &format!("/api/engines/{engine}/key"), Some(json!({ "key": "sk-test", "url": format!("http://{api}/v1") }))).await;
    }

    // Claude turns the turn away at its limit: OpenAI, the first backup, answers instead.
    let (status, changed) = s
        .call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "backups": [{ "engine": "openai", "model": "gpt-test" }, { "engine": "xai", "model": "gpt-test" }, { "engine": "nope" }] })))
        .await;
    assert_eq!(status, 400, "an unknown AI is refused: {changed}");
    let (_, changed) = s
        .call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "backups": [{ "engine": "openai", "model": "gpt-test" }, { "engine": "xai", "model": "gpt-test" }] })))
        .await;
    assert_eq!(changed["bot"]["backups"][1]["engine"], "xai");
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "limit me", "threadId": thread }))).await;
    let note = until(&mut events, "note", |f| f["kind"] == "message" && f["message"]["kind"] == "activity" && f["message"]["tool"]["name"].as_str().is_some_and(|n| n.contains("backup"))).await;
    let said = note["message"]["tool"]["name"].as_str().unwrap();
    assert!(said.contains("Claude turned the last turn away at its usage limit") && said.contains("OpenAI · gpt-test (a backup) is answering"), "{said}");
    let ask = until(&mut events, "ask", |f| runtime(f, "request.opened")).await;
    let request = ask["event"]["requestId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/threads/{thread}/respond"), Some(json!({ "requestId": request, "behavior": "allow" }))).await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // The next message goes straight to the backup; with OpenAI at Catbot's limit, to the next one.
    s.call("PUT", "/api/limits", Some(json!({ "engines": { "openai": { "session": 100 } } }))).await;
    let (status, _) = s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Again", "threadId": thread }))).await;
    assert_eq!(status, 202);
    let note = until(&mut events, "note", |f| f["kind"] == "message" && f["message"]["kind"] == "activity" && f["message"]["tool"]["name"].as_str().is_some_and(|n| n.contains("Grok"))).await;
    assert!(note["message"]["tool"]["name"].as_str().unwrap().contains("Grok (xAI) · gpt-test (a backup)"));
    let ask = until(&mut events, "ask", |f| runtime(f, "request.opened")).await;
    let request = ask["event"]["requestId"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/threads/{thread}/respond"), Some(json!({ "requestId": request, "behavior": "allow" }))).await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let (_, by_engine) = s.call("GET", "/api/usage?period=30d&by=engine", None).await;
    let labels: Vec<&str> = by_engine["rows"].as_array().unwrap().iter().filter_map(|r| r["label"].as_str()).collect();
    assert!(labels.contains(&"Grok (xAI)") && labels.contains(&"OpenAI"), "{labels:?}");

    // None can go: the message is refused, saying why.
    s.call("PUT", "/api/limits", Some(json!({ "engines": { "openai": { "session": 100 }, "xai": { "session": 100 } } }))).await;
    let (status, refused) = s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "Once more", "threadId": thread }))).await;
    assert_eq!(status, 429);
    assert!(refused["error"].as_str().unwrap().contains("its backups can't go either"), "{refused}");
}

#[tokio::test]
async fn pictures_are_asked_for_drawn_and_kept() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    let folder = s._dir.path().join("home/bots/scout/attachments");
    // As if Blender weren't installed (it may be, where the tests run), until it's wanted below.
    // SAFETY: only this test reads it.
    unsafe { std::env::set_var("CATBOT_BLENDER", s._dir.path().join("no-blender")) };

    // `/image`: the person's message, and a card offering the AIs that can draw.
    let (status, asked) = s.call("POST", &format!("/api/bots/{bot}/images"), Some(json!({ "prompt": "a red barn at dusk", "threadId": thread }))).await;
    assert_eq!(status, 202, "{asked}");
    assert_eq!(asked["message"]["text"], "/image a red barn at dusk");
    let card = asked["card"].clone();
    let choices: Vec<&str> = card["image"]["options"].as_array().unwrap().iter().filter_map(|o| o["id"].as_str()).collect();
    assert_eq!(choices, vec!["claude-svg"], "only Claude can draw with no keys saved and no Blender");
    let id = card["id"].as_str().unwrap().to_string();

    // Claude draws it in SVG; Catbot renders it, keeps it, and shows it.
    let (status, _) = s.call("POST", &format!("/api/threads/{thread}/images/{id}"), Some(json!({ "choice": "claude-svg" }))).await;
    assert_eq!(status, 202);
    let picture = until(&mut events, "picture", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["attachments"].is_array()).await;
    let attachment = &picture["message"]["attachments"][0];
    assert_eq!(attachment["mime"], "image/png");
    assert!(picture["message"]["text"].as_str().unwrap().contains("drawn by Claude, drawing in SVG"));
    let file = attachment["file"].as_str().unwrap();
    assert_eq!(&std::fs::read(folder.join(file)).unwrap()[1..4], b"PNG");
    let done = until(&mut events, "done", |f| f["kind"] == "message.patch" && f["message"]["image"]["state"] == "done").await;
    assert_eq!(done["message"]["image"]["attachment"]["id"], attachment["id"]);
    assert_eq!(s.call("POST", &format!("/api/threads/{thread}/images/{id}"), Some(json!({ "choice": "claude-svg" }))).await.0, 409);

    // The bot asks for one itself; with a key saved, OpenAI's image model draws it.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let api = listen(fake_api(seen.clone())).await;
    s.call("PUT", "/api/engines/openai/key", Some(json!({ "key": "sk-test", "url": format!("http://{api}/v1") }))).await;
    s.call("POST", &format!("/api/bots/{bot}/messages"), Some(json!({ "text": "draw me a fox in the snow", "threadId": thread }))).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert_eq!(reply["message"]["text"], "Coming right up.", "the request is taken out of the reply");
    let card = until(&mut events, "card", |f| f["kind"] == "message" && f["message"]["kind"] == "image").await;
    assert_eq!(card["message"]["image"]["askedBy"], "Scout");
    assert_eq!(card["message"]["image"]["prompt"], "a fox in the snow");
    assert_eq!(card["message"]["image"]["options"][0]["label"], "OpenAI · gpt-image-2");
    let id = card["message"]["id"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/threads/{thread}/images/{id}"), Some(json!({ "choice": "openai" }))).await;
    until(&mut events, "drawn", |f| f["kind"] == "message.patch" && f["message"]["image"]["state"] == "done").await;
    let asked = seen.lock().unwrap().iter().find(|r| r.get("images").is_some()).cloned().unwrap();
    assert_eq!((asked["images"]["model"].as_str(), asked["images"]["prompt"].as_str()), (Some("gpt-image-2"), Some("a fox in the snow")));

    // Blender: the script is shown first and runs only on the go-ahead.
    let blender = s._dir.path().join("fake-blender");
    std::fs::write(&blender, "#!/usr/bin/env python3\nimport sys, base64\nout = sys.argv[sys.argv.index('--') + 1]\nopen(out, 'wb').write(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='))\n").unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&blender, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // SAFETY: only this test reads it.
    unsafe { std::env::set_var("CATBOT_BLENDER", &blender) };
    let (_, asked) = s.call("POST", &format!("/api/bots/{bot}/images"), Some(json!({ "prompt": "a teapot", "threadId": thread }))).await;
    let id = asked["card"]["id"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/threads/{thread}/images/{id}"), Some(json!({ "choice": "claude-blender" }))).await;
    let waiting = until(&mut events, "script", |f| f["kind"] == "message.patch" && f["message"]["image"]["state"] == "confirming").await;
    assert!(waiting["message"]["image"]["script"].as_str().unwrap().contains("import bpy"));
    s.call("POST", &format!("/api/threads/{thread}/images/{id}"), Some(json!({ "run": true }))).await;
    until(&mut events, "rendered", |f| f["kind"] == "message.patch" && f["message"]["image"]["state"] == "done").await;
    let pictures = std::fs::read_dir(&folder).unwrap().count();
    assert_eq!(pictures, 3);
}

#[tokio::test]
async fn a_channel_is_answered_by_its_lead_or_whoever_is_named() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let mut ids = Vec::new();
    for name in ["Scout", "Kiwi"] {
        let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": name }))).await;
        ids.push(made["bot"]["id"].as_str().unwrap().to_string());
    }
    let (status, made) = s.call("POST", "/api/channels", Some(json!({ "name": "Garden", "members": ids, "lead": ids[1] }))).await;
    assert_eq!(status, 201, "{made}");
    let channel = made["bot"]["id"].as_str().unwrap().to_string();
    assert!(channel.starts_with("ch-"));
    assert_eq!(made["bot"]["channel"]["lead"], ids[1].as_str());
    assert_eq!(made["bot"]["channel"]["members"][0]["name"], "Scout");
    let thread = made["bot"]["threadId"].as_str().unwrap().to_string();
    assert!(s._dir.path().join("home/channels/garden").is_dir());
    let (_, list) = s.call("GET", "/api/bots", None).await;
    assert!(list["bots"].as_array().unwrap().iter().any(|b| b["id"] == channel.as_str()));
    // A bot's own chats don't include the channel's transcript.
    let (_, scout) = s.call("GET", &format!("/api/bots/{}", ids[0]), None).await;
    assert_eq!(scout["bot"]["tasks"].as_array().unwrap().len(), 1);
    assert_ne!(scout["bot"]["threadId"], thread.as_str());

    // No @name: the lead answers.
    let (status, sent) = s.call("POST", &format!("/api/bots/{channel}/messages"), Some(json!({ "text": "What should we plant?" }))).await;
    assert_eq!(status, 202, "{sent}");
    let reply = until(&mut events, "lead", |f| f["kind"] == "message" && f["threadId"] == thread.as_str() && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    let said = reply["message"]["text"].as_str().unwrap();
    assert!(said.contains("Person: What should we plant?") && said.contains("as Kiwi"), "{said}");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // Two named: each answers in turn, the second seeing the first.
    s.call("POST", &format!("/api/bots/{channel}/messages"), Some(json!({ "text": "@Scout then @Kiwi: tomatoes or beans?" }))).await;
    let first = until(&mut events, "scout", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert!(first["message"]["text"].as_str().unwrap().contains("as Scout"));
    let second = until(&mut events, "kiwi", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    let said = second["message"]["text"].as_str().unwrap();
    assert!(said.contains("as Kiwi") && said.contains("Scout: Hello"), "Kiwi sees what Scout said: {said}");
    // Settled once both are done.
    until(&mut events, "idle", |f| f["kind"] == "bot" && f["bot"]["id"] == channel.as_str() && f["bot"]["activity"] == "idle").await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=20"), None).await;
    assert_eq!(page["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "bot" && m["kind"] == "text").count(), 3);

    // Renamed and given instructions; a member deleted leaves the channel to the other.
    let (_, changed) = s.call("PATCH", &format!("/api/bots/{channel}"), Some(json!({ "name": "Allotment", "soul": "Keep it short." }))).await;
    assert_eq!((changed["bot"]["name"].as_str(), changed["bot"]["soul"].as_str()), (Some("Allotment"), Some("Keep it short.")));
    s.call("DELETE", &format!("/api/bots/{}", ids[1]), None).await;
    let (status, after) = s.call("GET", &format!("/api/bots/{channel}"), None).await;
    assert_eq!(status, 200);
    assert_eq!(after["bot"]["channel"]["lead"], ids[0].as_str());
    assert_eq!(after["bot"]["messages"].as_array().unwrap().len(), 5, "the talk survives");
    assert_eq!(s.call("DELETE", &format!("/api/bots/{channel}"), None).await.0, 200);
    assert_eq!(s.call("GET", &format!("/api/bots/{channel}"), None).await.0, 404);
}

#[tokio::test]
async fn a_bot_hands_work_on_within_the_limits_set() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let mut ids = Vec::new();
    for (name, soul) in [("Scout", "fake-pass @Kiwi"), ("Kiwi", "fake-pass @Scout")] {
        let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": name, "soul": soul }))).await;
        ids.push(made["bot"]["id"].as_str().unwrap().to_string());
    }
    let (_, limits) = s.call("GET", "/api/handoffs", None).await;
    assert_eq!(limits["handoffs"], json!({ "unlimited": false, "depth": 3, "replies": 6 }));
    let replies = |page: &Value| -> Vec<String> {
        page["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "bot" && m["kind"] == "text").map(|m| m["botId"].as_str().unwrap_or_default().to_string()).collect()
    };
    let notes = |page: &Value| -> Vec<String> {
        page["messages"].as_array().unwrap().iter().filter_map(|m| m["tool"]["name"].as_str()?.strip_prefix("notice: ").map(str::to_string)).collect()
    };

    // Three hand-offs deep: Scout, Kiwi, Scout, Kiwi; the fourth is refused.
    let (status, sent) = s.call("POST", &format!("/api/bots/{}/messages", ids[0]), Some(json!({ "text": "Plan the beds" }))).await;
    assert_eq!(status, 202, "{sent}");
    let thread = sent["threadId"].as_str().unwrap().to_string();
    until(&mut events, "too deep", |f| f["kind"] == "message" && f["message"]["tool"]["name"].as_str().is_some_and(|t| t.contains("3 deep"))).await;
    until(&mut events, "idle", |f| f["kind"] == "bot" && f["bot"]["id"] == ids[0].as_str() && f["bot"]["activity"] == "idle").await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=50"), None).await;
    assert_eq!(replies(&page), [ids[0].as_str(), &ids[1], &ids[0], &ids[1]], "{page}");
    assert_eq!(notes(&page)[0], "Scout handed this to Kiwi.");
    let texts: Vec<&str> = page["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "bot" && m["kind"] == "text").filter_map(|m| m["text"].as_str()).collect();
    assert!(texts[1].contains("Scout: Hello, Plan the beds"), "Kiwi is shown the chat: {}", texts[1]);
    assert!(texts[2].contains("[Kiwi]: Hello,") && texts[2].contains("handed this back"), "Scout is told what Kiwi said: {}", texts[2]);
    // Kiwi was a guest: Scout's chat keeps Scout's own session.
    let (_, scout) = s.call("GET", &format!("/api/bots/{}", ids[0]), None).await;
    assert_eq!(scout["bot"]["tasks"].as_array().unwrap().len(), 1);

    // Fewer replies allowed: Scout, Kiwi, Scout, then the limit.
    let (status, _) = s.call("PUT", "/api/handoffs", Some(json!({ "depth": 10, "replies": 3 }))).await;
    assert_eq!(status, 200);
    s.call("POST", &format!("/api/bots/{}/messages", ids[0]), Some(json!({ "text": "Again", "threadId": thread }))).await;
    until(&mut events, "too many", |f| f["kind"] == "message" && f["message"]["tool"]["name"].as_str().is_some_and(|t| t.contains("at most 3 bot replies"))).await;
    until(&mut events, "idle", |f| f["kind"] == "bot" && f["bot"]["id"] == ids[0].as_str() && f["bot"]["activity"] == "idle").await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=80"), None).await;
    assert_eq!(replies(&page).len(), 4 + 3);

    // Unlimited: it goes on until stopped.
    s.call("PUT", "/api/handoffs", Some(json!({ "unlimited": true }))).await;
    s.call("POST", &format!("/api/bots/{}/messages", ids[0]), Some(json!({ "text": "Keep going", "threadId": thread }))).await;
    for n in 0..5 {
        until(&mut events, &format!("reply {n}"), |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    }
    let (status, stopped) = s.call("POST", &format!("/api/bots/{}/interrupt", ids[0]), Some(json!({ "threadId": thread }))).await;
    assert_eq!((status, stopped["stopped"].as_bool()), (200, Some(true)));
    until(&mut events, "idle", |f| f["kind"] == "bot" && f["bot"]["id"] == ids[0].as_str() && f["bot"]["activity"] == "idle").await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=200"), None).await;
    let count = replies(&page).len();
    assert!(count >= 7 + 5, "{count}");
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=200"), None).await;
    assert!(replies(&page).len() <= count + 1, "stopped");
}

#[tokio::test]
async fn a_chief_of_staff_is_made_or_chosen_and_told_the_team() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout", "soul": "Looks after the garden." }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    assert_eq!(made["bot"]["chief"], false);
    let (_, none) = s.call("GET", "/api/chief", None).await;
    assert_eq!(none["chief"]["botId"], Value::Null);

    // The ready-made Chief: made, starred, and told about Scout.
    let (status, ready) = s.call("POST", "/api/chief/ready-made", None).await;
    assert_eq!(status, 201, "{ready}");
    assert_eq!(ready["name"], "Chief");
    let chief = ready["chief"]["botId"].as_str().unwrap().to_string();
    let (_, bot) = s.call("GET", &format!("/api/bots/{chief}"), None).await;
    assert_eq!((bot["bot"]["chief"].as_bool(), bot["bot"]["name"].as_str()), (Some(true), Some("Chief")));
    assert!(bot["bot"]["soul"].as_str().unwrap().contains("Chief of Staff"));
    s.call("POST", &format!("/api/bots/{chief}/messages"), Some(json!({ "text": "what are your instructions?" }))).await;
    let told = until(&mut events, "told", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    let told = told["message"]["text"].as_str().unwrap();
    assert!(told.contains("Your team:") && told.contains("- Scout (on claude, asks before acting): Looks after the garden."), "{told}");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // Scout made the Chief instead: the star moves.
    let (status, _) = s.call("PUT", "/api/chief", Some(json!({ "botId": scout }))).await;
    assert_eq!(status, 200);
    let (_, list) = s.call("GET", "/api/bots", None).await;
    let starred: Vec<&str> = list["bots"].as_array().unwrap().iter().filter(|b| b["chief"] == true).filter_map(|b| b["name"].as_str()).collect();
    assert_eq!(starred, ["Scout"]);
    assert_eq!(s.call("PUT", "/api/chief", Some(json!({ "botId": "nobody" }))).await.0, 400);
    let (_, cleared) = s.call("PUT", "/api/chief", Some(json!({ "botId": null }))).await;
    assert_eq!(cleared["chief"]["botId"], Value::Null);
}

#[tokio::test]
async fn the_chief_changes_the_team_only_with_a_yes() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    let (_, ready) = s.call("POST", "/api/chief/ready-made", None).await;
    let chief = ready["chief"]["botId"].as_str().unwrap().to_string();
    let card = |f: &Value| f["kind"] == "message" && f["message"]["kind"] == "team-change";
    let ask = |text: &str| json!({ "text": text });

    // Scout to act without asking: a card with a warning; approved, it's done.
    let (_, sent) = s.call("POST", &format!("/api/bots/{chief}/messages"), Some(ask(r#"team: {"action":"change_bot","bot":"scout","approval":"full"}"#))).await;
    let thread = sent["threadId"].as_str().unwrap().to_string();
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert_eq!(reply["message"]["text"], "Proposing it.", "the tag comes out of the reply");
    let shown = until(&mut events, "card", card).await;
    let change = &shown["message"]["teamChange"];
    assert_eq!((change["title"].as_str(), change["state"].as_str()), (Some("Change Scout"), Some("asking")));
    assert_eq!(change["lines"][0], "Approval level: acts without asking");
    assert!(change["warning"].as_str().unwrap().contains("without asking"));
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let id = shown["message"]["id"].as_str().unwrap();
    let (status, answered) = s.call("POST", &format!("/api/threads/{thread}/team/{id}"), Some(json!({ "approve": true }))).await;
    assert_eq!((status, answered["state"].as_str()), (200, Some("done")), "{answered}");
    let (_, bot) = s.call("GET", &format!("/api/bots/{scout}"), None).await;
    assert_eq!(bot["bot"]["approvalMode"], "full");
    assert_eq!(s.call("POST", &format!("/api/threads/{thread}/team/{id}"), Some(json!({ "approve": true }))).await.0, 409);

    // A new bot, declined: nothing is made.
    s.call("POST", &format!("/api/bots/{chief}/messages"), Some(ask(r#"team: {"action":"create_bot","name":"Moss","instructions":"Researches."}"#))).await;
    let shown = until(&mut events, "card", card).await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let id = shown["message"]["id"].as_str().unwrap();
    s.call("POST", &format!("/api/threads/{thread}/team/{id}"), Some(json!({ "approve": false }))).await;
    let (_, list) = s.call("GET", "/api/bots", None).await;
    assert!(!list["bots"].as_array().unwrap().iter().any(|b| b["name"] == "Moss"));

    // One Catbot can't use is a note, not a card.
    s.call("POST", &format!("/api/bots/{chief}/messages"), Some(ask(r#"team: {"action":"delete_bot","bot":"Nobody"}"#))).await;
    until(&mut events, "note", |f| f["kind"] == "message" && f["message"]["tool"]["name"].as_str().is_some_and(|n| n.contains("no bot called Nobody"))).await;
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;

    // The Chief hears how each went with the next message, once.
    s.call("POST", &format!("/api/bots/{chief}/messages"), Some(ask("How did that go?"))).await;
    let heard = until(&mut events, "heard", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    let heard = heard["message"]["text"].as_str().unwrap();
    assert!(heard.contains("couldn't be used: there's no bot called Nobody") && !heard.contains("Change Scout"), "{heard}");
}

#[tokio::test]
async fn the_watch_wakes_the_chief_when_something_goes_wrong() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    // Scout on an AI with no key saved: its turn ends in an error.
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout", "modelSelection": { "instanceId": "cerebras" } }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    let (_, ready) = s.call("POST", "/api/chief/ready-made", None).await;
    let chief = ready["chief"]["botId"].as_str().unwrap().to_string();
    let (_, options) = s.call("GET", "/api/chief", None).await;
    assert_eq!((options["chief"]["watch"].as_bool(), options["chief"]["checkins"]["on"].as_bool()), (Some(true), Some(false)));
    assert_eq!(options["chief"]["checkins"]["days"], json!([1, 2, 3, 4, 5]));
    let (_, nothing) = s.call("POST", "/api/chief/look", None).await;
    assert_eq!(nothing["woke"], false, "nothing wrong yet");

    s.call("POST", &format!("/api/bots/{scout}/messages"), Some(json!({ "text": "Plan the beds" }))).await;
    until(&mut events, "scout done", |f| runtime(f, "turn.completed")).await;
    let (_, woke) = s.call("POST", "/api/chief/look", None).await;
    assert_eq!(woke["woke"], true);
    let asked = until(&mut events, "report", |f| f["kind"] == "message" && f["message"]["fromCatbot"] == "watch").await;
    let report = asked["message"]["text"].as_str().unwrap();
    assert!(report.contains("In Scout's chat") && report.contains("needs an API key"), "{report}");
    let answer = until(&mut events, "chief", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert!(answer["message"]["text"].as_str().unwrap().contains("Catbot's watch found"));
    until(&mut events, "chief done", |f| runtime(f, "turn.completed")).await;
    let (_, again) = s.call("POST", "/api/chief/look", None).await;
    assert_eq!(again["woke"], false, "each problem is reported once");

    // A timed check-in shows the Chief where the team stands; its options are kept.
    let (status, set) = s.call("PUT", "/api/chief", Some(json!({ "checkins": { "on": true, "everyHours": 2 } }))).await;
    assert_eq!(status, 200, "{set}");
    assert_eq!((set["chief"]["checkins"]["everyHours"].as_u64(), set["chief"]["checkins"]["from"].as_str()), (Some(2), Some("09:00")));
    assert_eq!(set["chief"]["botId"], chief.as_str(), "the Chief stays");
    assert_eq!(s.call("PUT", "/api/chief", Some(json!({ "checkins": { "from": "9am" } }))).await.0, 400);
    s.call("POST", "/api/chief/check-in", None).await;
    let asked = until(&mut events, "check-in", |f| f["kind"] == "message" && f["message"]["fromCatbot"] == "check-in").await;
    let status = asked["message"]["text"].as_str().unwrap();
    assert!(status.contains("Timed check-in") && status.contains("Scout: idle"), "{status}");
}

#[tokio::test]
async fn the_chief_writes_the_daily_brief_from_catbots_facts() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    s.call("POST", &format!("/api/bots/{scout}/messages"), Some(json!({ "text": "Plan the beds" }))).await;
    until(&mut events, "scout done", |f| runtime(f, "turn.completed")).await;
    s.call("POST", "/api/chief/ready-made", None).await;
    let (_, options) = s.call("GET", "/api/chief", None).await;
    assert_eq!(options["chief"]["brief"], json!({ "on": true, "time": "08:00", "days": [1, 2, 3, 4, 5] }));

    let (_, sent) = s.call("POST", "/api/chief/brief", None).await;
    assert_eq!(sent["woke"], true);
    let asked = until(&mut events, "facts", |f| f["kind"] == "message" && f["message"]["fromCatbot"] == "brief").await;
    let facts = asked["message"]["text"].as_str().unwrap();
    for part in ["Daily brief from Catbot", "Waiting on the person:\n- nothing", "Scout: 1 reply", "Hello, Plan the beds", "Spending:\n- This month: $0.00"] {
        assert!(facts.contains(part), "{part} in {facts}");
    }
    until(&mut events, "brief written", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;

    let (status, set) = s.call("PUT", "/api/chief", Some(json!({ "brief": { "time": "07:30", "days": [1, 2, 3, 4, 5, 6] } }))).await;
    assert_eq!(status, 200);
    assert_eq!(set["chief"]["brief"], json!({ "on": true, "time": "07:30", "days": [1, 2, 3, 4, 5, 6] }));
    assert_eq!(s.call("PUT", "/api/chief", Some(json!({ "brief": { "time": "half seven" } }))).await.0, 400);
}

#[tokio::test]
async fn a_team_comes_in_from_a_file_after_a_look_and_goes_out_as_one() {
    let s = setup().await;
    let file = "---\nbotmrr: 1\nid: garden\nname: Garden Crew\ntagline: Grow things.\nagents:\n  - key: ada\n    name: Ada\n    title: Lead Gardener\n    description: Plans the beds.\n    appearance:\n      color: purple\n  - key: lin\n    name: Lin\n    description: Waters.\n  - key: moss\n    name: Moss\n    description: Composts.\nchiefOfStaff: ada\nrooms:\n  - key: shed\n    name: Shed\n    members: [ada, lin, moss]\n    bulletin: Keep it tidy.\n    defaultResponder:\n      kind: agent\n      agent: lin\nroutines:\n  - key: daily\n    enabledAfterInstall: false\nrequirements:\n  apps:\n    - slug: gmail\n      label: Gmail\n---\n\n# Garden Crew\n\n## Activation\n\nRead it all.\n";

    // The look before anything is made.
    let (status, looked) = s.call("POST", "/api/teams/preview", Some(json!({ "text": file }))).await;
    assert_eq!(status, 200, "{looked}");
    let plan = &looked["plan"];
    assert_eq!(plan["bots"].as_array().unwrap().len(), 3);
    assert_eq!((plan["chief"]["name"].as_str(), plan["channels"][0]["lead"].as_str()), (Some("Ada"), Some("Lin")));
    let left = plan["leftOut"].to_string();
    assert!(left.contains("1 routine") && left.contains("Gmail"), "{left}");
    let (_, list) = s.call("GET", "/api/bots", None).await;
    assert!(!list["bots"].as_array().unwrap().iter().any(|b| b["name"] == "Ada"), "nothing made by looking");
    assert_eq!(s.call("POST", "/api/teams/preview", Some(json!({ "text": "hello" }))).await.0, 400);

    // Made, leaving Moss out, with Ada as Chief.
    let (status, made) = s.call("POST", "/api/teams/import", Some(json!({ "text": file, "leaveOut": ["moss"], "makeChief": true }))).await;
    assert_eq!(status, 201, "{made}");
    assert_eq!(made["bots"], json!(["Ada", "Lin"]));
    assert_eq!((made["channels"][0].as_str(), made["chief"].as_str()), (Some("Shed"), Some("Ada")));
    let (_, list) = s.call("GET", "/api/bots", None).await;
    let bots = list["bots"].as_array().unwrap();
    let ada = bots.iter().find(|b| b["name"] == "Ada").unwrap();
    assert_eq!((ada["chief"].as_bool(), ada["approvalMode"].as_str(), ada["color"].as_str()), (Some(true), Some("ask"), Some("purple")));
    assert!(ada["soul"].as_str().unwrap().contains("Your role on the Garden Crew team: Lead Gardener."));
    assert!(s._dir.path().join("home/bots/ada/TEAM.md").is_file());
    let shed = bots.iter().find(|b| b["name"] == "Shed").unwrap();
    assert_eq!((shed["soul"].as_str(), shed["channel"]["members"].as_array().unwrap().len()), (Some("Keep it tidy."), 2));

    // Out again as a file that reads back in.
    let (status, out) = s.call("POST", "/api/teams/export", Some(json!({ "name": "Garden Crew", "tagline": "Grow things." }))).await;
    assert_eq!(status, 200, "{out}");
    let markdown = out["markdown"].as_str().unwrap();
    assert!(markdown.starts_with("---\nbotmrr: 1") && markdown.contains("chiefOfStaff: ada") && markdown.contains("## Completion rule"), "{markdown}");
    assert_eq!(out["filename"], "garden-crew.md");
    let (_, again) = s.call("POST", "/api/teams/preview", Some(json!({ "text": markdown }))).await;
    assert_eq!(again["plan"]["bots"].as_array().unwrap().len(), 2);
    assert_eq!(again["plan"]["channels"][0]["lead"], "Lin");
}

#[tokio::test]
async fn skills_come_from_the_library_and_a_bot_can_offer_one() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let skill = "---\nname: bed-plan\ndescription: Lay out garden beds from a sketch.\n---\n\n# Bed plan\n\nMeasure first.\n";
    let (status, added) = s.call("POST", "/api/skills", Some(json!({ "text": skill }))).await;
    assert_eq!((status, added["name"].as_str()), (201, Some("bed-plan")), "{added}");
    assert_eq!(s.call("POST", "/api/skills", Some(json!({ "text": skill }))).await.0, 400, "no two with one name");
    assert_eq!(s.call("POST", "/api/skills", Some(json!({ "text": "---\nname: Bad Name\ndescription: x\n---\n" }))).await.0, 400);

    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    assert_eq!(made["bot"]["skills"], json!([]));
    let (status, changed) = s.call("PATCH", &format!("/api/bots/{scout}"), Some(json!({ "skills": ["bed-plan"] }))).await;
    assert_eq!((status, &changed["bot"]["skills"]), (200, &json!(["bed-plan"])));
    assert_eq!(s.call("PATCH", &format!("/api/bots/{scout}"), Some(json!({ "skills": ["nope"] }))).await.0, 400);
    let (_, list) = s.call("GET", "/api/skills", None).await;
    assert_eq!(list["skills"][0]["bots"], json!(["Scout"]));

    // `/bed-plan …` runs it; the skill is in Scout's folder for the turn.
    s.call("POST", &format!("/api/bots/{scout}/messages"), Some(json!({ "text": "/bed-plan the north beds" }))).await;
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert!(reply["message"]["text"].as_str().unwrap().contains("Use your skill “bed-plan” for this: read .claude/skills/bed-plan/SKILL.md"), "{reply}");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    let placed = s._dir.path().join("home/bots/scout/.claude/skills/bed-plan");
    assert!(placed.join("SKILL.md").is_file() && placed.join(".catbot-skill").is_file());

    // Scout offers one; approved, it's in the library and Scout uses it; the old one is taken off.
    s.call("PATCH", &format!("/api/bots/{scout}"), Some(json!({ "skills": [] }))).await;
    let (_, sent) = s.call("POST", &format!("/api/bots/{scout}/messages"), Some(json!({ "text": "offer a skill" }))).await;
    let thread = sent["threadId"].as_str().unwrap().to_string();
    let reply = until(&mut events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
    assert_eq!(reply["message"]["text"], "This will come up again.");
    let card = until(&mut events, "card", |f| f["kind"] == "message" && f["message"]["kind"] == "team-change").await;
    assert_eq!(card["message"]["teamChange"]["title"], "Save the skill “weed-check”");
    assert_eq!(card["message"]["teamChange"]["heading"], "Scout wants to save a skill");
    until(&mut events, "done", |f| runtime(f, "turn.completed")).await;
    assert!(!placed.exists(), "a skill Scout no longer uses is taken out of its folder");
    let id = card["message"]["id"].as_str().unwrap();
    let (_, answered) = s.call("POST", &format!("/api/threads/{thread}/team/{id}"), Some(json!({ "approve": true }))).await;
    assert_eq!(answered["state"], "done", "{answered}");
    let (_, bot) = s.call("GET", &format!("/api/bots/{scout}"), None).await;
    assert_eq!(bot["bot"]["skills"], json!(["weed-check"]));

    assert_eq!(s.call("DELETE", "/api/skills/bed-plan", None).await.0, 200);
    let (_, list) = s.call("GET", "/api/skills", None).await;
    assert_eq!(list["skills"].as_array().unwrap().len(), 1);
}

/// The fake mail servers (`tests/fake-imap.py`): IMAP's port, SMTP's, and the process to stop.
/// What it's sent goes in `sent`.
fn fake_mail(sent: &std::path::Path) -> (u16, u16, std::process::Child) {
    use std::io::BufRead;
    let mut child = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-imap.py"))
        .env("FAKE_SMTP_LOG", sent)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let ports: Vec<u16> = line.split_whitespace().map(|p| p.parse().unwrap()).collect();
    (ports[0], ports[1], child)
}

#[tokio::test]
async fn a_chosen_bot_reads_the_mailbox_through_catbots_tools() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let sent = s._dir.path().join("sent.log");
    let (port, smtp, mut imap) = fake_mail(&sent);
    let mailbox = |password: &str, host: &str| json!({ "address": "moo@y.com", "password": password, "imapHost": host, "imapPort": port, "imapSecurity": "plain", "smtpHost": "127.0.0.1", "smtpPort": smtp, "smtpSecurity": "plain" });

    // Kept only once the mailbox lets Catbot in; never without TLS away from this computer.
    let (status, refused) = s.call("PUT", "/api/email", Some(mailbox("wrong", "127.0.0.1"))).await;
    assert_eq!(status, 400);
    assert!(refused["error"].as_str().unwrap().contains("app password"), "{refused}");
    let (status, far) = s.call("PUT", "/api/email", Some(mailbox("right", "mail.example.org"))).await;
    assert_eq!(status, 400);
    assert!(far["error"].as_str().unwrap().contains("without TLS"), "{far}");
    let (status, view) = s.call("PUT", "/api/email", Some(mailbox("right", "127.0.0.1"))).await;
    assert_eq!((status, view["connected"].as_bool(), view["bots"].clone()), (200, Some(true), json!([])), "{view}");
    assert!(view.get("password").is_none(), "the password never goes back");
    let (_, guessed) = s.call("GET", "/api/email/guess?address=moo%40gmail.com", None).await;
    assert_eq!(guessed["imapHost"], "imap.gmail.com");

    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    let ask = |text: &str| json!({ "text": text });
    let reply = async |s: &Setup, events: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, text: &str| -> String {
        s.call("POST", &format!("/api/bots/{scout}/messages"), Some(ask(text))).await;
        let said = until(events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
        until(events, "done", |f| runtime(f, "turn.completed")).await;
        said["message"]["text"].as_str().unwrap().to_string()
    };

    // Not chosen: no mail tools.
    assert!(reply(&s, &mut events, "catbot-tool: mail_search {}").await.contains("no tool mail_search"));

    // Chosen: it searches, reads and lists folders.
    s.call("PUT", "/api/email/bots", Some(json!({ "bots": [scout] }))).await;
    let found = reply(&s, &mut events, r#"catbot-tool: mail_search {"query": "invoice"}"#).await;
    assert!(found.contains("id 2") && found.contains("from Ann <ann@z.com>") && found.contains("“Invoice 42”") && found.contains("unread") && !found.contains("Seeds"), "{found}");
    let newest = reply(&s, &mut events, "catbot-tool: mail_search {}").await;
    assert!(newest.find("id 2").unwrap() < newest.find("id 1").unwrap(), "newest first: {newest}");
    let read = reply(&s, &mut events, r#"catbot-tool: mail_read {"id": 1}"#).await;
    assert!(read.contains("From: Bob <bob@x.com>") && read.contains("The seeds came today."), "{read}");
    assert!(reply(&s, &mut events, "catbot-tool: mail_folders {}").await.contains("INBOX, Archive"));
    let missing = reply(&s, &mut events, r#"catbot-tool: mail_read {"id": 9}"#).await;
    assert!(missing.starts_with("ERROR: ") && missing.contains("no message 9"), "{missing}");

    // Sending asks first: a card with exactly what would go; sent once approved, in the conversation it answers.
    let waiting = reply(&s, &mut events, r#"catbot-tool: mail_send {"reply_to": 1, "body": "Thanks, Bob!"}"#).await;
    assert!(waiting.contains("waiting for the person's approval"), "{waiting}");
    let (_, page) = s.call("GET", &format!("/api/bots/{scout}"), None).await;
    let thread = page["bot"]["threadId"].as_str().unwrap().to_string();
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=50"), None).await;
    let card = page["messages"].as_array().unwrap().iter().rev().find(|m| m["kind"] == "team-change").unwrap().clone();
    let change = &card["teamChange"];
    assert_eq!((change["title"].as_str(), change["heading"].as_str()), (Some("Send “Re: Seeds”"), Some("Scout wants to send an email")));
    assert_eq!(change["lines"], json!(["From: moo@y.com", "To: Bob <bob@x.com>", "A reply, in the same conversation", "Thanks, Bob!"]));
    assert!(!sent.exists(), "nothing goes before the yes");
    let id = card["id"].as_str().unwrap();
    let (_, answered) = s.call("POST", &format!("/api/threads/{thread}/team/{id}"), Some(json!({ "approve": true }))).await;
    assert_eq!(answered["state"], "done", "{answered}");
    let mail = std::fs::read_to_string(&sent).unwrap();
    assert!(mail.contains("RCPT TO:<bob@x.com>") && mail.contains("Subject: Re: Seeds") && mail.contains("In-Reply-To: <seeds@x.com>") && mail.contains("Thanks, Bob!"), "{mail}");
    let bad = reply(&s, &mut events, r#"catbot-tool: mail_send {"to": ["not an address"], "body": "Hi"}"#).await;
    assert!(bad.starts_with("ERROR: ") && bad.contains("isn't an email address"), "{bad}");

    // Disconnected: gone.
    s.call("DELETE", "/api/email", None).await;
    assert!(reply(&s, &mut events, "catbot-tool: mail_search {}").await.contains("no tool mail_search"));
    imap.kill().unwrap();
}

#[tokio::test]
async fn a_bot_publishes_a_page_once_approved_and_it_can_be_taken_down() {
    let s = setup().await;
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;
    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    let site = s._dir.path().join("home/bots/scout/site");
    std::fs::create_dir_all(site.join("img")).unwrap();
    std::fs::write(site.join("index.html"), "<h1>Garden plan</h1>").unwrap();
    std::fs::write(site.join("img/bed.svg"), "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
    let ask = async |s: &Setup, events: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, text: &str| -> String {
        s.call("POST", &format!("/api/bots/{scout}/messages"), Some(json!({ "text": text }))).await;
        let said = until(events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
        until(events, "done", |f| runtime(f, "turn.completed")).await;
        said["message"]["text"].as_str().unwrap().to_string()
    };

    let outside = ask(&s, &mut events, r#"catbot-tool: page_publish {"path": "../../skills", "title": "Sneaky"}"#).await;
    assert!(outside.starts_with("ERROR: "), "{outside}");
    let waiting = ask(&s, &mut events, r#"catbot-tool: page_publish {"path": "site", "title": "Garden plan"}"#).await;
    assert!(waiting.contains("waiting for the person's approval"), "{waiting}");
    let (_, bot) = s.call("GET", &format!("/api/bots/{scout}"), None).await;
    let thread = bot["bot"]["threadId"].as_str().unwrap().to_string();
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=50"), None).await;
    let card = page["messages"].as_array().unwrap().iter().rev().find(|m| m["kind"] == "team-change").unwrap().clone();
    assert_eq!(card["teamChange"]["title"], "Publish “Garden plan”");
    assert!(card["teamChange"]["lines"][0].as_str().unwrap().starts_with("Files: img/bed.svg, index.html (2 in all"), "{card}");
    let (_, listed) = s.call("GET", "/api/pages", None).await;
    assert_eq!(listed["pages"], json!([]), "nothing online before the yes");

    let id = card["id"].as_str().unwrap();
    let (_, answered) = s.call("POST", &format!("/api/threads/{thread}/team/{id}"), Some(json!({ "approve": true }))).await;
    assert_eq!(answered["state"], "done", "{answered}");
    let (_, listed) = s.call("GET", "/api/pages", None).await;
    let published = &listed["pages"][0];
    assert_eq!((published["title"].as_str(), published["botName"].as_str(), published["files"].as_u64()), (Some("Garden plan"), Some("Scout"), Some(2)));
    let page_id = published["id"].as_str().unwrap().to_string();
    assert!(published["url"].as_str().unwrap().ends_with(&format!("/p/{page_id}/")));

    // Served at both doors, with no key, sandboxed; nothing outside it.
    let (status, csp, html) = s.page(false, &format!("/p/{page_id}/")).await;
    assert_eq!((status, html.as_str()), (200, "<h1>Garden plan</h1>"));
    assert!(csp.starts_with("sandbox"), "{csp}");
    assert_eq!(s.page(true, &format!("/p/{page_id}/img/bed.svg")).await.0, 200);
    assert_eq!(s.page(true, &format!("/p/{page_id}/..%2F..%2Fdata.db")).await.0, 404);
    assert_eq!(s.page(true, "/p/0123456789abcdef0123456789abcdef/").await.0, 404);
    std::fs::write(site.join("index.html"), "<h1>Changed</h1>").unwrap();
    assert_eq!(s.page(false, &format!("/p/{page_id}/")).await.2, "<h1>Garden plan</h1>", "a copy: changes show once published again");

    // Updated in place, then taken down.
    ask(&s, &mut events, &format!(r#"catbot-tool: page_publish {{"path": "site", "title": "Garden plan", "update": "{page_id}"}}"#)).await;
    let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=80"), None).await;
    let card = page["messages"].as_array().unwrap().iter().rev().find(|m| m["kind"] == "team-change").unwrap().clone();
    assert_eq!(card["teamChange"]["title"], "Update “Garden plan”");
    s.call("POST", &format!("/api/threads/{thread}/team/{}", card["id"].as_str().unwrap()), Some(json!({ "approve": true }))).await;
    assert_eq!(s.page(false, &format!("/p/{page_id}/")).await.2, "<h1>Changed</h1>");
    assert_eq!(s.call("GET", "/api/pages", None).await.1["pages"].as_array().unwrap().len(), 1);
    assert_eq!(s.call("DELETE", &format!("/api/pages/{page_id}"), None).await.0, 200);
    assert_eq!(s.page(true, &format!("/p/{page_id}/")).await.0, 404);
}

/// A stand-in for Netlify's and Cloudflare's APIs: every call is logged as "METHOD path [body]".
async fn fake_hosts() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = log.clone();
    let addr = listen(Router::new().fallback(move |req: axum::extract::Request| {
        let seen = seen.clone();
        async move {
            let (parts, body) = req.into_parts();
            let path = parts.uri.path().to_string();
            let good = parts.headers.get("authorization").is_some_and(|v| v == "Bearer good-token");
            let body = String::from_utf8_lossy(&body.collect().await.unwrap().to_bytes()).to_string();
            seen.lock().unwrap().push(format!("{} {path} {body}", parts.method));
            let answer = |status: u16, v: Value| Response::builder().status(status).header("content-type", "application/json").body(Body::from(v.to_string())).unwrap();
            if !good {
                return answer(401, json!({ "message": "Access Denied" }));
            }
            let method = parts.method.as_str();
            match (method, path.split('/').collect::<Vec<_>>().as_slice()) {
                ("GET", ["", "user"]) => answer(200, json!({ "id": "u1" })),
                ("GET", ["", "user", "tokens", "verify"]) => answer(200, json!({ "success": true })),
                ("POST", ["", "sites"]) => {
                    let name = serde_json::from_str::<Value>(&body).unwrap()["name"].as_str().unwrap().to_string();
                    answer(201, json!({ "id": "site-1", "name": name, "ssl_url": format!("https://{name}.netlify.app") }))
                }
                ("GET", ["", "sites", id]) => answer(200, json!({ "id": id, "ssl_url": "https://again.netlify.app" })),
                ("POST", ["", "sites", _, "deploys"]) => {
                    let files: Value = serde_json::from_str::<Value>(&body).unwrap()["files"].clone();
                    let required: Vec<Value> = files.as_object().unwrap().values().cloned().collect();
                    answer(200, json!({ "id": "deploy-1", "required": required, "state": "uploading" }))
                }
                ("PUT", ["", "deploys", _, "files", ..]) => answer(200, json!({})),
                ("GET", ["", "deploys", _]) => answer(200, json!({ "id": "deploy-1", "state": "ready" })),
                ("DELETE", ["", "sites", _]) => answer(204, Value::Null),
                _ => answer(404, json!({ "message": "Not Found" })),
            }
        }
    }))
    .await;
    (format!("http://{addr}"), log)
}

#[tokio::test]
async fn pages_go_to_netlify_or_cloudflare_as_the_person_picks_on_the_card() {
    let s = setup().await;
    let (api, calls) = fake_hosts().await;
    let wrangler_log = s._dir.path().join("wrangler.log");
    // SAFETY: only this test reads these.
    unsafe {
        std::env::set_var("CATBOT_NETLIFY_API", &api);
        std::env::set_var("CATBOT_CLOUDFLARE_API", &api);
        std::env::set_var("CATBOT_WRANGLER", format!("python3 {}", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-wrangler.py")));
        std::env::set_var("FAKE_WRANGLER_LOG", &wrangler_log);
    }
    let wrangled = || std::fs::read_to_string(&wrangler_log).unwrap_or_default().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect::<Vec<_>>();
    let mut events = s.events().await;
    until(&mut events, "hello", |f| f["kind"] == "hello").await;

    // Connecting: each key is checked, and never comes back.
    let (status, refused) = s.call("PUT", "/api/pages/hosts", Some(json!({ "netlify": { "token": "bad-token" } }))).await;
    assert_eq!(status, 400);
    assert!(refused["error"].as_str().unwrap_or(&refused.to_string()).contains("didn't accept"), "{refused}");
    let (_, shown) = s.call("PUT", "/api/pages/hosts", Some(json!({ "netlify": { "token": "good-token" }, "default": "netlify" }))).await;
    assert_eq!((shown["hosts"]["default"].as_str(), shown["hosts"]["netlify"]["ends"].as_str()), (Some("netlify"), Some("oken")), "{shown}");
    assert_eq!(s.call("PUT", "/api/pages/hosts", Some(json!({ "cloudflare": { "token": "good-token", "account": "not hex" } }))).await.0, 400);
    let (_, shown) = s.call("PUT", "/api/pages/hosts", Some(json!({ "cloudflare": { "token": "good-token", "account": "0123456789abcdef0123456789abcdef" } }))).await;
    assert!(!shown.to_string().contains("good-token"), "{shown}");
    assert_eq!(shown["hosts"]["cloudflare"]["account"], "0123456789abcdef0123456789abcdef");

    let (_, made) = s.call("POST", "/api/bots", Some(json!({ "name": "Scout" }))).await;
    let scout = made["bot"]["id"].as_str().unwrap().to_string();
    let site = s._dir.path().join("home/bots/scout/site");
    std::fs::create_dir_all(site.join("img")).unwrap();
    std::fs::write(site.join("index.html"), "<h1>Garden plan</h1>").unwrap();
    std::fs::write(site.join("img/bed.svg"), "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
    let (_, bot) = s.call("GET", &format!("/api/bots/{scout}"), None).await;
    let thread = bot["bot"]["threadId"].as_str().unwrap().to_string();
    let ask = async |s: &Setup, events: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, text: &str| -> Value {
        s.call("POST", &format!("/api/bots/{scout}/messages"), Some(json!({ "text": text }))).await;
        until(events, "reply", |f| f["kind"] == "message" && f["message"]["role"] == "bot" && f["message"]["kind"] == "text").await;
        until(events, "done", |f| runtime(f, "turn.completed")).await;
        let (_, page) = s.call("GET", &format!("/api/threads/{thread}/messages?limit=80"), None).await;
        page["messages"].as_array().unwrap().iter().rev().find(|m| m["kind"] == "team-change").unwrap().clone()
    };
    let answer = async |s: &Setup, card: &Value, choice: Option<&str>| -> (u16, Value) {
        let mut body = json!({ "approve": true });
        if let Some(choice) = choice {
            body["choice"] = json!(choice);
        }
        s.call("POST", &format!("/api/threads/{thread}/team/{}", card["id"].as_str().unwrap()), Some(body)).await
    };

    // The card asks where, starting on the default.
    let card = ask(&s, &mut events, r#"catbot-tool: page_publish {"path": "site", "title": "Garden plan"}"#).await;
    let choice = &card["teamChange"]["choice"];
    let ids: Vec<&str> = choice["options"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert_eq!((ids, choice["value"].as_str()), (vec!["catbot", "netlify", "cloudflare"], Some("netlify")), "{card}");
    assert_eq!(answer(&s, &card, Some("mars")).await.0, 400, "only the card's own choices");
    let (_, answered) = answer(&s, &card, None).await;
    assert_eq!(answered["state"], "done", "{answered}");
    let netlify = calls.lock().unwrap().clone();
    assert!(netlify.iter().any(|c| c.starts_with("POST /sites {\"name\":\"catbot-garden-plan-")), "{netlify:?}");
    assert!(netlify.iter().any(|c| c.starts_with("PUT /deploys/deploy-1/files/index.html <h1>Garden plan</h1>")), "{netlify:?}");
    assert!(netlify.iter().any(|c| c.starts_with("PUT /deploys/deploy-1/files/img/bed.svg")), "{netlify:?}");
    let (_, listed) = s.call("GET", "/api/pages", None).await;
    let on_netlify = listed["pages"][0].clone();
    assert_eq!((on_netlify["host"].as_str(), on_netlify["site"].as_str()), (Some("netlify"), Some("site-1")));
    assert!(on_netlify["url"].as_str().unwrap().ends_with(".netlify.app/"), "{on_netlify}");
    assert_eq!(s.page(false, &format!("/p/{}/", on_netlify["id"].as_str().unwrap())).await.0, 404, "Catbot doesn't serve it too");

    // Another page, picked for Cloudflare: wrangler makes its project and uploads a copy.
    let card = ask(&s, &mut events, r#"catbot-tool: page_publish {"path": "site/index.html", "title": "Just the plan"}"#).await;
    let (_, answered) = answer(&s, &card, Some("cloudflare")).await;
    assert_eq!(answered["state"], "done", "{answered}");
    let runs = wrangled();
    assert_eq!(runs[0]["args"].as_array().unwrap()[..3], json!(["pages", "project", "create"]).as_array().unwrap()[..]);
    let project = runs[0]["args"][3].as_str().unwrap().to_string();
    assert!(project.starts_with("catbot-just-the-plan-"), "{project}");
    assert_eq!((runs[0]["token"].as_str(), runs[0]["account"].as_str()), (Some("good-token"), Some("0123456789abcdef0123456789abcdef")));
    assert_eq!(runs[1]["args"].as_array().unwrap()[..2], json!(["pages", "deploy"]).as_array().unwrap()[..]);
    assert_eq!((runs[1]["files"].clone(), runs[1]["index"].as_str()), (json!(["index.html"]), Some("<h1>Garden plan</h1>")));
    let (_, listed) = s.call("GET", "/api/pages", None).await;
    let on_cloudflare = listed["pages"][0].clone();
    assert_eq!(on_cloudflare["url"], format!("https://{project}-x1.pages.dev/"), "the address Cloudflare gave");
    assert!(!s._dir.path().join(format!("home/pages/.{}-cloudflare", on_cloudflare["id"].as_str().unwrap())).exists(), "the copy is cleared away");

    // An update stays where it is, and asks nothing about where.
    let card = ask(&s, &mut events, &format!(r#"catbot-tool: page_publish {{"path": "site", "title": "Garden plan", "update": "{}"}}"#, on_netlify["id"].as_str().unwrap())).await;
    assert!(card["teamChange"]["choice"].is_null(), "{card}");
    assert!(card["teamChange"]["lines"][1].as_str().unwrap().starts_with("Where: Netlify, replacing"), "{card}");
    answer(&s, &card, None).await;
    assert_eq!(calls.lock().unwrap().iter().filter(|c| c.starts_with("POST /sites ")).count(), 1, "the same site again");

    // Taken down there, then here.
    assert_eq!(s.call("DELETE", &format!("/api/pages/{}", on_netlify["id"].as_str().unwrap()), None).await.0, 200);
    assert!(calls.lock().unwrap().iter().any(|c| c.starts_with("DELETE /sites/site-1")));
    assert_eq!(s.call("DELETE", &format!("/api/pages/{}", on_cloudflare["id"].as_str().unwrap()), None).await.0, 200);
    assert_eq!(wrangled().last().unwrap()["args"], json!(["pages", "project", "delete", project, "--yes"]));
    assert_eq!(s.call("GET", "/api/pages", None).await.1["pages"], json!([]));

    // Disconnected: back to this computer only, and no choice on the card.
    let (_, shown) = s.call("PUT", "/api/pages/hosts", Some(json!({ "netlify": null, "cloudflare": null }))).await;
    assert_eq!((shown["hosts"]["default"].as_str(), shown["hosts"]["netlify"].is_null()), (Some("catbot"), true));
}
