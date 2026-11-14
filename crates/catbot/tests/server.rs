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

struct Setup {
    base: String,
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
    let app = server::App::new(
        store,
        proxy,
        dir.path().join("no-codex"),
        vec!["python3".into(), FAKE_CLAUDE.into()],
        Some(vec![env!("CARGO_BIN_EXE_catbot").into()]),
    );
    let addr = listen(server::router(app)).await;
    Setup { base: format!("http://{addr}"), _dir: dir }
}

impl Setup {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let body = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
        let req = hyper::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base))
            .header("content-type", "application/json")
            .body(Full::new(body))
            .unwrap();
        let resp = client.request(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Opens the live stream; frames arrive on the returned channel.
    async fn events(&self) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
        let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let req = hyper::Request::get(format!("{}/api/events?window=1", self.base)).body(Full::new(Bytes::new())).unwrap();
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
    let bot = made["bot"]["id"].as_str().unwrap().to_string();
    let (status, patched) = s
        .call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "name": "Ranger", "approvalMode": "edits", "soul": "Scout.", "modelSelection": { "instanceId": "claude", "model": "claude-opus-5-5" } })))
        .await;
    assert_eq!(status, 200);
    assert_eq!(patched["bot"]["name"], "Ranger");
    assert_eq!(patched["bot"]["approvalMode"], "edits");
    assert_eq!(patched["bot"]["modelSelection"]["model"], "claude-opus-5-5");
    let (status, _) = s.call("PATCH", &format!("/api/bots/{bot}"), Some(json!({ "modelSelection": { "instanceId": "codex", "model": "gpt" } }))).await;
    assert_eq!(status, 400, "Catbot bots run on Claude");
    assert_eq!(s.call("GET", &format!("/api/bots/{bot}/activity"), None).await.0, 404);
    assert_eq!(s.call("DELETE", &format!("/api/bots/{bot}"), None).await.0, 200);
    let (_, list) = s.call("GET", "/api/bots", None).await;
    assert_eq!(list["bots"].as_array().unwrap().len(), 1);
}
