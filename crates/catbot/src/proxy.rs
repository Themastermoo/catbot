//! The pass-through recorder: every request goes to the reference server unchanged, every
//! response comes back unchanged (streams stay streams), and a copy goes into the recording.

use std::{
    net::SocketAddr,
    path::Path,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Instant,
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use http_body_util::{BodyExt, Full};
use hyper::{
    StatusCode, Uri,
    body::Incoming,
    header::{self, HeaderMap, HeaderName},
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo},
};

use crate::record::{self, Record, Recorder};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type HttpClient = Client<HttpConnector, Full<Bytes>>;

/// Passes requests to the reference server unchanged, recording them when asked to. With no
/// reference server ("off", the default since stage 29.1) it answers that Catbot has no such
/// route instead.
pub struct Proxy {
    client: HttpClient,
    upstream: Option<Uri>,
    recorder: Option<Recorder>,
    cap: usize,
    next_id: AtomicU64,
}

type App = Proxy;

impl Proxy {
    pub fn new(upstream: &str, recorder: Option<Recorder>, cap: usize) -> Result<Arc<Self>, BoxError> {
        let upstream = match upstream.trim() {
            "" | "off" => None,
            upstream => {
                let upstream: Uri = upstream.parse()?;
                if upstream.scheme_str() != Some("http") || upstream.authority().is_none() {
                    return Err(format!("--upstream must look like http://127.0.0.1:8799 (or be off), not {upstream}").into());
                }
                Some(upstream)
            }
        };
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        let client = Client::builder(TokioExecutor::new()).build(connector);
        Ok(Arc::new(Self { client, upstream, recorder, cap, next_id: AtomicU64::new(1) }))
    }

    pub fn upstream(&self) -> Option<&Uri> {
        self.upstream.as_ref()
    }

    /// Sends a request on and returns the reference server's answer (a 502 if it has none).
    pub async fn handle(self: &Arc<Self>, req: Request) -> Response {
        if self.upstream.is_none() {
            let path = req.uri().path().to_string();
            return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({ "error": format!("Catbot has nothing at {path}") }))).into_response();
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let path = req.uri().path().to_string();
        let result = if req.headers().contains_key(header::UPGRADE) {
            upgrade(self, id, req).await
        } else {
            forward(self, id, req).await
        };
        result.unwrap_or_else(|err| {
            eprintln!("catbot: {path}: the reference server didn't answer: {err}");
            (StatusCode::BAD_GATEWAY, format!("catbot: the reference server didn't answer: {err}\n")).into_response()
        })
    }

    /// Sends a request the server builds itself (the bot list it merges, the event stream).
    pub async fn request(&self, req: hyper::Request<Full<Bytes>>) -> Result<hyper::Response<Incoming>, BoxError> {
        if self.upstream.is_none() {
            return Err("there's no reference server".into());
        }
        Ok(self.client.request(req).await?)
    }

    /// The reference server's address for a path (and query).
    pub fn uri_for(&self, path_and_query: &str) -> Result<Uri, BoxError> {
        let mut parts = self.upstream.clone().ok_or("there's no reference server")?.into_parts();
        parts.path_and_query = Some(path_and_query.parse()?);
        Ok(Uri::from_parts(parts)?)
    }

    fn record(&self, record: Record) {
        if let Some(recorder) = &self.recorder {
            recorder.write(record);
        }
    }
}

/// `catbot record`: nothing but the recording pass-through.
pub async fn run(listen: SocketAddr, upstream: &str, out: &Path, cap: usize) -> Result<(), BoxError> {
    let (recorder, file) = Recorder::start(out).await?;
    let proxy = Proxy::new(upstream, Some(recorder), cap)?;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|err| format!("could not listen on {listen}: {err} (is something else using that port?)"))?;
    let Some(upstream) = proxy.upstream.clone() else { return Err("recording needs a reference server (--upstream)".into()) };
    eprintln!("catbot: recording http://{listen} -> {upstream}");
    eprintln!("catbot: writing {}", file.display());
    eprintln!("catbot: start the interface with OMB_PORT={} pnpm dev", listen.port());

    let router = Router::new().fallback(handle).with_state(proxy);
    axum::serve(listener, router).await?;
    Ok(())
}

async fn handle(State(proxy): State<Arc<Proxy>>, req: Request) -> Response {
    proxy.handle(req).await
}

/// Headers that belong to one connection and are never passed along.
fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection" | "keep-alive" | "proxy-connection" | "proxy-authenticate" | "proxy-authorization" | "te"
            | "trailer" | "transfer-encoding" | "upgrade"
    )
}

fn upstream_uri(app: &App, uri: &Uri) -> Result<Uri, BoxError> {
    let mut parts = app.upstream.clone().ok_or("there's no reference server")?.into_parts();
    parts.path_and_query = uri.path_and_query().cloned();
    Ok(Uri::from_parts(parts)?)
}

fn content_type(headers: &HeaderMap) -> Option<String> {
    headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// An ordinary request: sent on as-is, its response streamed back through a [`Tee`].
async fn forward(app: &Arc<App>, id: u64, req: Request) -> Result<Response, BoxError> {
    let started = Instant::now();
    let (parts, body) = req.into_parts();
    let req_bytes = body.collect().await?.to_bytes();

    let mut up = hyper::Request::builder().method(parts.method.clone()).uri(upstream_uri(app, &parts.uri)?);
    for (name, value) in &parts.headers {
        // No compression on the way back, so the recording can read what it copies.
        if !is_hop_by_hop(name) && name != header::ACCEPT_ENCODING {
            up = up.header(name, value);
        }
    }
    let req_type = content_type(&parts.headers);
    let req_body = record::body_value(&req_bytes, req_bytes.len(), req_type.as_deref(), app.cap);
    let resp = app.client.request(up.body(Full::new(req_bytes))?).await?;

    let (rparts, rbody) = resp.into_parts();
    let resp_type = content_type(&rparts.headers);
    let path = parts.uri.path().to_string();
    let query = record::redact_query(parts.uri.query());
    let status = rparts.status.as_u16();

    let mode = if resp_type.as_deref().is_some_and(|t| t.starts_with("text/event-stream")) {
        eprintln!("{:>6} {status} {path}  (live stream opened)", "SSE");
        app.record(Record::SseOpen { id, at: record::now(), path, query, status });
        Mode::Sse
    } else {
        Mode::Http(Some(Pending {
            method: parts.method.to_string(),
            path,
            query,
            status,
            req_type,
            req_body,
            resp_type,
            started,
        }))
    };

    let mut out = Response::builder().status(rparts.status);
    for (name, value) in &rparts.headers {
        if !is_hop_by_hop(name) {
            out = out.header(name, value);
        }
    }
    let tee = Tee { inner: Box::pin(rbody), app: app.clone(), id, mode, captured: Vec::new(), total: 0, done: false };
    Ok(out.body(Body::new(tee))?)
}

/// A websocket: the handshake goes through, then bytes are copied both ways until either side closes.
async fn upgrade(app: &Arc<App>, id: u64, mut req: Request) -> Result<Response, BoxError> {
    let client_side = hyper::upgrade::on(&mut req);
    let mut up = hyper::Request::builder().method(req.method().clone()).uri(upstream_uri(app, req.uri())?);
    for (name, value) in req.headers() {
        if name != header::TRANSFER_ENCODING && name != header::CONTENT_LENGTH {
            up = up.header(name, value);
        }
    }
    let mut resp = app.client.request(up.body(Full::new(Bytes::new()))?).await?;
    let status = resp.status();
    let path = req.uri().path().to_string();
    eprintln!("{:>6} {} {path}  (websocket)", "WS", status.as_u16());
    app.record(Record::Ws { id, at: record::now(), path: path.clone(), status: status.as_u16() });

    let mut out = Response::builder().status(status);
    for (name, value) in resp.headers() {
        out = out.header(name, value);
    }
    if status != StatusCode::SWITCHING_PROTOCOLS {
        return Ok(out.body(Body::new(resp.into_body()))?);
    }
    let upstream_side = hyper::upgrade::on(&mut resp);
    tokio::spawn(async move {
        match tokio::try_join!(client_side, upstream_side) {
            Ok((client, upstream)) => {
                let (mut client, mut upstream) = (TokioIo::new(client), TokioIo::new(upstream));
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            }
            Err(err) => eprintln!("catbot: websocket {path} failed to connect both sides: {err}"),
        }
    });
    Ok(out.body(Body::empty())?)
}

struct Pending {
    method: String,
    path: String,
    query: Option<String>,
    status: u16,
    req_type: Option<String>,
    req_body: Option<serde_json::Value>,
    resp_type: Option<String>,
    started: Instant,
}

enum Mode {
    /// An ordinary response: keep the first `cap` bytes, record once it has all arrived.
    Http(Option<Pending>),
    /// A live event stream: record each chunk as it passes.
    Sse,
}

/// A response body that passes every frame through untouched and copies what it needs.
struct Tee {
    inner: Pin<Box<Incoming>>,
    app: Arc<App>,
    id: u64,
    mode: Mode,
    captured: Vec<u8>,
    total: usize,
    done: bool,
}

impl Tee {
    fn observe(&mut self, data: &Bytes) {
        self.total += data.len();
        match self.mode {
            Mode::Http(_) => {
                let room = self.app.cap.saturating_sub(self.captured.len());
                self.captured.extend_from_slice(&data[..data.len().min(room)]);
            }
            Mode::Sse => {
                if !data.is_empty() {
                    let data = record::sse_value(data, self.app.cap);
                    self.app.record(Record::Sse { id: self.id, at: record::now(), data });
                }
            }
        }
    }

    fn finish(&mut self) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        match &mut self.mode {
            Mode::Http(pending) => {
                let Some(p) = pending.take() else { return };
                let ms = p.started.elapsed().as_millis();
                eprintln!("{:>6} {} {}  {ms}ms", p.method, p.status, p.path);
                let resp_body = record::body_value(&self.captured, self.total, p.resp_type.as_deref(), self.app.cap);
                self.app.record(Record::Http {
                    id: self.id,
                    at: record::now(),
                    method: p.method,
                    path: p.path,
                    query: p.query,
                    status: p.status,
                    ms,
                    req_type: p.req_type,
                    req_body: p.req_body,
                    resp_type: p.resp_type,
                    resp_bytes: self.total,
                    resp_body,
                });
            }
            Mode::Sse => {
                self.app.record(Record::SseClose { id: self.id, at: record::now(), bytes: self.total });
            }
        }
    }
}

impl http_body::Body for Tee {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        let this = &mut *self;
        let polled = this.inner.as_mut().poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.observe(data);
                }
            }
            Poll::Ready(None) | Poll::Ready(Some(Err(_))) => this.finish(),
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// A response that ends early (the browser navigated away) or was never read still gets recorded.
impl Drop for Tee {
    fn drop(&mut self) {
        self.finish();
    }
}
