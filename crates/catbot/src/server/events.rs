//! `GET /api/events`: one live stream for the interface, carrying Catbot's own frames and the
//! reference server's, as server-sent events.
//!
//! Every stream starts with `hello` and `resumed: false`, so after any reconnect the interface
//! reloads what it shows instead of trusting a replay. That is simpler than merging two
//! servers' replay cursors, and costs one bot-list fetch per reconnect.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{HeaderValue, header},
    response::Response,
};
use futures_util::{StreamExt, stream};
use http_body_util::{BodyExt, Full};
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle};

use super::App;

enum Out {
    Frame(Value),
    Ping,
    /// Something upstream or here lost its place: end the stream so the interface reloads.
    End,
}

/// Stops the stream's helper tasks when the interface goes away.
struct Tasks(Vec<JoinHandle<()>>);

impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

pub async fn events(app: Arc<App>, req: Request) -> Response {
    let (tx, rx) = mpsc::channel::<Out>(512);

    let mut ours = app.hub.subscribe();
    let to_us = tx.clone();
    let mine = tokio::spawn(async move {
        loop {
            let out = match ours.recv().await {
                Ok(frame) => Out::Frame(frame),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Out::End,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            let end = matches!(out, Out::End);
            if to_us.send(out).await.is_err() || end {
                return;
            }
        }
    });

    let theirs = tokio::spawn(upstream(app.clone(), req, tx.clone()));

    let to_ping = tx;
    let pinger = tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(20));
        every.tick().await;
        loop {
            every.tick().await;
            if to_ping.send(Out::Ping).await.is_err() {
                return;
            }
        }
    });

    let stream_id = format!("cb{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let hello = format!("data: {}\n\n", json!({ "kind": "hello", "cursor": format!("{stream_id}:0"), "resumed": false }));
    let tasks = Tasks(vec![mine, theirs, pinger]);
    let frames = stream::unfold((rx, 0u64, stream_id, tasks), |(mut rx, mut n, stream_id, tasks)| async move {
        let chunk = match rx.recv().await {
            Some(Out::Frame(frame)) => {
                n += 1;
                format!("id: {stream_id}:{n}\ndata: {frame}\n\n")
            }
            Some(Out::Ping) => format!(": keepalive\ndata: {}\n\n", json!({ "kind": "ping" })),
            Some(Out::End) | None => return None,
        };
        Some((Ok::<_, Infallible>(Bytes::from(chunk)), (rx, n, stream_id, tasks)))
    });
    let body = stream::once(async move { Ok::<_, Infallible>(Bytes::from(hello)) }).chain(frames);

    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-transform"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

/// Reads the reference server's own event stream and passes its frames on (all but its hello).
async fn upstream(app: Arc<App>, req: Request, tx: mpsc::Sender<Out>) {
    let query: Vec<&str> = req.uri().query().unwrap_or("").split('&').filter(|p| !p.is_empty() && !p.starts_with("since=")).collect();
    let path = if query.is_empty() { "/api/events".to_string() } else { format!("/api/events?{}", query.join("&")) };
    let Ok(uri) = app.proxy.uri_for(&path) else { return };
    let mut builder = hyper::Request::builder().method("GET").uri(uri);
    for (name, value) in req.headers() {
        if !matches!(name.as_str(), "connection" | "accept-encoding" | "content-length" | "transfer-encoding" | "upgrade") {
            builder = builder.header(name, value);
        }
    }
    let Ok(request) = builder.body(Full::new(Bytes::new())) else { return };
    let response = match app.proxy.request(request).await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            eprintln!("catbot: the reference server's event stream answered {}; Catbot's own frames only", response.status());
            return;
        }
        Err(err) => {
            eprintln!("catbot: no event stream from the reference server ({err}); Catbot's own frames only");
            return;
        }
    };
    let mut body = response.into_body();
    let mut buf = String::new();
    while let Some(Ok(frame)) = body.frame().await {
        let Some(chunk) = frame.data_ref() else { continue };
        buf.push_str(&String::from_utf8_lossy(chunk));
        while let Some(end) = buf.find("\n\n") {
            let block: String = buf.drain(..end + 2).collect();
            let data: String = block.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim_start).collect::<Vec<_>>().join("\n");
            let Ok(frame) = serde_json::from_str::<Value>(&data) else { continue };
            if matches!(frame["kind"].as_str(), Some("hello" | "ping")) {
                continue;
            }
            if tx.send(Out::Frame(frame)).await.is_err() {
                return;
            }
        }
    }
    // Their stream ended (their server restarted): end ours too, so the interface reconnects and reloads.
    let _ = tx.send(Out::End).await;
}
