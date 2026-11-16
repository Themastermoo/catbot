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
    Router::new()
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
