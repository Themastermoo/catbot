//! Catbot inside the phone app (stage 26): only the chat APIs are offered, and a new bot starts
//! on one. Its own test program, because it changes the process's environment.

use axum::body::Body;
use catbot::{proxy::Proxy, server, store::Store};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(app: &axum::Router, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn on_the_phone_only_chat_apis_are_offered_and_new_bots_use_one() {
    unsafe { std::env::set_var("CATBOT_PHONE", "1") };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("home")).unwrap();
    let proxy = Proxy::new("http://127.0.0.1:9", None, 16 * 1024).unwrap();
    let app = server::App::new(store, proxy, dir.path().join("no-codex"), None, vec!["no-claude".into()], None, None);
    let router = server::router(app.clone());

    let (status, list) = call(&router, "GET", "/api/engines", None).await;
    assert_eq!(status, 200, "{list}");
    let engines = list["engines"].as_array().unwrap();
    assert!(engines.iter().all(|e| e["kind"] == "api" && e["needsKey"] == true), "{list}");
    let ids: Vec<&str> = engines.iter().filter_map(|e| e["id"].as_str()).collect();
    for wanted in ["anthropic", "openai", "xai", "google", "openrouter"] {
        assert!(ids.contains(&wanted), "{wanted} offered on the phone");
    }
    assert!(!ids.contains(&"claude") && !ids.contains(&"codex") && !ids.contains(&"ollama"));

    // With no key yet a new bot starts on Claude's API; with an OpenAI key, on OpenAI.
    let (_, made) = call(&router, "POST", "/api/bots", Some(json!({ "name": "Pocket" }))).await;
    assert_eq!(made["bot"]["modelSelection"]["instanceId"], "anthropic", "{made}");
    app.with_store(|store| store.set_engine_key("openai", "sk-phone-1234", None));
    let (_, made) = call(&router, "POST", "/api/bots", Some(json!({ "name": "Pocket 2" }))).await;
    assert_eq!(made["bot"]["modelSelection"]["instanceId"], "openai");
    // A bot copied over with its own AI keeps it.
    let (_, made) = call(&router, "POST", "/api/bots", Some(json!({ "name": "Copied", "modelSelection": { "instanceId": "xai" } }))).await;
    assert_eq!(made["bot"]["modelSelection"]["instanceId"], "xai");
    // A bot copied from the PC comes with its picture, colour, role and description.
    let copy = json!({ "name": "Scout", "soul": "Finds things.", "mark": "robot-full", "color": "orange", "title": "Researcher", "description": "Looks things up.", "avatarZoom": 1.5 });
    let (status, made) = call(&router, "POST", "/api/bots", Some(copy)).await;
    assert_eq!(status, 201, "{made}");
    let bot = &made["bot"];
    assert_eq!((bot["mark"].as_str(), bot["color"].as_str(), bot["soul"].as_str()), (Some("robot-full"), Some("orange"), Some("Finds things.")));
    assert_eq!((bot["title"].as_str(), bot["description"].as_str(), bot["avatarZoom"].as_f64()), (Some("Researcher"), Some("Looks things up."), Some(1.5)));
}
