//! The remote door: the second listener, for devices away from home (the phone app).
//!
//! It listens on this machine only (127.0.0.1:8789 unless `--remote` says otherwise), and
//! Tailscale Funnel carries Catbot's public https address to it. Nothing here is open without
//! a paired device's key, except trading a pairing code for one:
//!
//! * `POST /api/pair` `{ code, name }` → `{ key, device }` (see `devices.rs`)
//!
//! With `Authorization: Bearer <key>`, a device gets Catbot's own routes, the same as at home:
//! its bots, chats, messages, approvals and live events. It does not get the reference server
//! (nothing is passed on), the interface's files, or the device list (devices are paired and
//! removed only at home).

use std::sync::Arc;

use axum::{
    Router,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::Response,
};
use serde_json::json;

use super::{App, bad, bot_route, create_bot, devices, events, json_body, owns_bot, owns_thread, respond, thread_route};
use crate::store::Device;

pub fn router(app: Arc<App>) -> Router {
    Router::new().fallback(dispatch).with_state(app)
}

/// The device a request's key belongs to, if it's paired.
fn device(app: &App, req: &Request) -> Option<Device> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let key = value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer "))?.trim();
    if key.len() != 64 {
        return None;
    }
    let device = app.with_store(|store| store.device_by_key_hash(&devices::key_hash(key))).flatten()?;
    // Heard from: noted at most once a minute.
    let stale = device
        .last_seen_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_none_or(|at| chrono::Utc::now().signed_duration_since(at) > chrono::Duration::seconds(60));
    if stale {
        app.with_store(|store| store.device_seen(&device.id));
    }
    Some(device)
}

fn unauthorized() -> Response {
    respond(Err((StatusCode::UNAUTHORIZED, "this device isn't paired with Catbot".into())))
}

async fn dispatch(State(app): State<Arc<App>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if segments.as_slice() == ["api", "pair"] && method == Method::POST {
        return respond(pair(&app, req).await);
    }
    // Published pages are for anyone with the link (`pages.rs`).
    if let ["p", id, rest @ ..] = segments.as_slice()
        && method == Method::GET
    {
        return super::pages::serve(&app, id, rest).await;
    }
    let Some(device) = device(&app, &req) else { return unauthorized() };
    match segments.as_slice() {
        ["api", "device"] if method == Method::GET => {
            respond(Ok((StatusCode::OK, json!({ "device": { "id": device.id, "name": device.name } }))))
        }
        ["api", "events"] if method == Method::GET => events::events(app, req, events::Door::Away { device: device.id }).await,
        ["api", "bots"] if method == Method::GET => super::catbot_bots(&app, &req),
        ["api", "memory", rest @ ..] if method == Method::GET => respond(super::memory::route(&app, rest, &req)),
        ["api", "attachments", id] if method == Method::GET => super::attachments::serve(&app, id).await,
        ["api", "usage", rest @ ..] if method == Method::GET => super::usage_route(&app, rest, super::usage_query(&req)).await,
        ["api", "limits"] if method == Method::GET => respond(super::limits::route(&app, &method, None, false).await),
        ["api", "handoffs"] if method == Method::GET => respond(super::handoffs::route(&app, None)),
        ["api", "pages"] if method == Method::GET => respond(super::pages::route(&app, &method, &[]).await),
        ["api", "email"] if method == Method::GET => respond(super::email::route(&app, &method, &[], None, None).await),
        ["api", "skills"] if method == Method::GET => respond(super::skills::route(&app, &method, &[], None).await),
        ["api", "chief"] if method == Method::GET => respond(super::chief::route(&app, &method, &[], None)),
        ["api", "engines", rest @ ..] => {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
            respond(super::engines::route(&app, &method, &rest, req, false).await)
        }
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
        _ => respond(Err((StatusCode::NOT_FOUND, "not available away from home".into()))),
    }
}

/// `POST /api/pair`: a pairing code (from the QR code at home) for this device's own key.
async fn pair(app: &Arc<App>, req: Request) -> super::Answer {
    let body = json_body(req).await?;
    let code = body["code"].as_str().ok_or_else(|| bad("a pairing needs its code"))?;
    let name = body["name"].as_str().unwrap_or("");
    let (device, key) = devices::redeem(app, code, name)?;
    Ok((StatusCode::CREATED, json!({ "key": key, "device": { "id": device.id, "name": device.name } })))
}
