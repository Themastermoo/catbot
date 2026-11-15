//! Talking to `catbot serve`, over the same routes OpenMausBot's interface uses.
//!
//! Two ways in, one shape: in a browser (wasm) through the browser's own fetch, from the page's
//! own server; in the desktop app (native) through reqwest, to `catbot serve` on this computer
//! (`CATBOT_URL`, else http://127.0.0.1:8788).

use serde_json::Value;

/// GET a path and read the answer as JSON.
pub async fn get(path: &str) -> Result<Value, String> {
    call("GET", path, None).await
}

/// POST JSON to a path and read the answer as JSON.
pub async fn post(path: &str, body: &Value) -> Result<Value, String> {
    call("POST", path, Some(body)).await
}

/// Any method, with an optional JSON body. A refusal comes back as the server's own words.
pub async fn call(method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    let (status, text) = send(method, path, body).await?;
    if !(200..300).contains(&status) {
        let said = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(text);
        return Err(if said.trim().is_empty() { format!("{path} answered {status}") } else { said });
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

/// Where `catbot serve` is, for the desktop app.
#[cfg(not(target_arch = "wasm32"))]
pub fn server() -> String {
    std::env::var("CATBOT_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "http://127.0.0.1:8788".into()).trim_end_matches('/').to_string()
}

#[cfg(target_arch = "wasm32")]
async fn send(method: &str, path: &str, body: Option<&Value>) -> Result<(u16, String), String> {
    use gloo_net::http::{Method, RequestBuilder};
    let method = match method {
        "POST" => Method::POST,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        "PUT" => Method::PUT,
        _ => Method::GET,
    };
    let builder = RequestBuilder::new(path).method(method);
    let request = match body {
        Some(body) => builder.json(body).map_err(|e| e.to_string())?,
        None => builder.build().map_err(|e| e.to_string())?,
    };
    let response = request.send().await.map_err(|e| format!("Catbot didn't answer ({e})"))?;
    let status = response.status();
    Ok((status, response.text().await.unwrap_or_default()))
}

#[cfg(not(target_arch = "wasm32"))]
async fn send(method: &str, path: &str, body: Option<&Value>) -> Result<(u16, String), String> {
    let client = client();
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut request = client.request(method, format!("{}{path}", server()));
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|e| format!("Catbot didn't answer at {} ({e})", server()))?;
    let status = response.status().as_u16();
    Ok((status, response.text().await.unwrap_or_default()))
}

/// One client for the app's life (it keeps connections open between calls).
#[cfg(not(target_arch = "wasm32"))]
pub fn client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new).clone()
}
