//! Talking to `catbot serve`, over the same routes OpenMausBot's interface uses.
//!
//! Two ways in, one shape: in a browser (wasm) through the browser's own fetch, from the page's
//! own server; in the desktop app (native) through reqwest, to `catbot serve` on this computer
//! (`CATBOT_URL`, else http://127.0.0.1:8788). The phone app goes to Catbot's public address
//! instead, with its device key (`link.rs`); if home has removed the device, the link is
//! forgotten and the app asks to pair again.

use serde_json::Value;

/// GET a path and read the answer as JSON.
pub async fn get(path: &str) -> Result<Value, String> {
    call("GET", path, None).await
}

/// POST JSON to a path and read the answer as JSON.
pub async fn post(path: &str, body: &Value) -> Result<Value, String> {
    call("POST", path, Some(body)).await
}

/// POST JSON, and read the status beside the answer: for routes whose refusals carry a question
/// back (going back to an older version answers 409 with what it needs).
pub async fn post_raw(path: &str, body: &Value) -> Result<(u16, Value), String> {
    let (status, text) = send("POST", path, Some(body)).await?;
    let answer = serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "error": text }));
    Ok((status, answer))
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

/// Where `catbot serve` is: the paired public address (the phone app), else this computer.
#[cfg(not(target_arch = "wasm32"))]
pub fn server() -> String {
    if let Some(link) = crate::link::active() {
        return link.url;
    }
    std::env::var("CATBOT_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "http://127.0.0.1:8788".into()).trim_end_matches('/').to_string()
}

/// A request to Catbot, with this device's key when it's paired.
#[cfg(not(target_arch = "wasm32"))]
pub fn request(method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    let request = client().request(method, format!("{}{path}", server()));
    match crate::link::active() {
        Some(link) => request.bearer_auth(link.key),
        None => request,
    }
}

/// After an answer: a paired device that's told it isn't paired forgets its link.
#[cfg(not(target_arch = "wasm32"))]
pub fn check_paired(status: u16) {
    if status == 401 && crate::link::active().is_some() {
        crate::link::forget();
    }
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
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut request = request(method, path);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|e| format!("Catbot didn't answer at {} ({e})", server()))?;
    let status = response.status().as_u16();
    check_paired(status);
    Ok((status, response.text().await.unwrap_or_default()))
}

/// Sends a file's bytes as an upload (its name in `X-File-Name`), and reads the answer as JSON.
pub async fn upload(path: &str, name: &str, mime: &str, bytes: Vec<u8>) -> Result<Value, String> {
    let name = crate::markdown::encode(name);
    #[cfg(target_arch = "wasm32")]
    let (status, text) = {
        use gloo_net::http::RequestBuilder;
        let body = js_sys::Uint8Array::from(bytes.as_slice());
        let request = RequestBuilder::new(path)
            .method(gloo_net::http::Method::POST)
            .header("content-type", mime)
            .header("x-file-name", &name)
            .body(body)
            .map_err(|e| e.to_string())?;
        let response = request.send().await.map_err(|e| format!("Catbot didn't answer ({e})"))?;
        (response.status(), response.text().await.unwrap_or_default())
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (status, text) = {
        let response = request(reqwest::Method::POST, path)
            .header("content-type", mime)
            .header("x-file-name", name)
            .body(bytes)
            .send()
            .await
            .map_err(|e| format!("Catbot didn't answer at {} ({e})", server()))?;
        let status = response.status().as_u16();
        check_paired(status);
        (status, response.text().await.unwrap_or_default())
    };
    if !(200..300).contains(&status) {
        let said = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(text);
        return Err(if said.trim().is_empty() { format!("the upload answered {status}") } else { said });
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// POSTs JSON and reads the answer as bytes, with their type (a voice's audio, `voice.rs`).
pub async fn post_for_bytes(path: &str, body: &Value) -> Result<(Vec<u8>, String), String> {
    #[cfg(target_arch = "wasm32")]
    let (status, kind, bytes) = {
        use gloo_net::http::{Method, RequestBuilder};
        let request = RequestBuilder::new(path).method(Method::POST).json(body).map_err(|e| e.to_string())?;
        let response = request.send().await.map_err(|e| format!("Catbot didn't answer ({e})"))?;
        let kind = response.headers().get("content-type").unwrap_or_default();
        (response.status(), kind, response.binary().await.unwrap_or_default())
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (status, kind, bytes) = {
        let response = request(reqwest::Method::POST, path).json(body).send().await.map_err(|e| format!("Catbot didn't answer at {} ({e})", server()))?;
        let status = response.status().as_u16();
        check_paired(status);
        let kind = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        (status, kind, response.bytes().await.map(|b| b.to_vec()).unwrap_or_default())
    };
    if !(200..300).contains(&status) {
        let said = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["error"].as_str().map(str::to_string));
        return Err(said.unwrap_or_else(|| format!("{path} answered {status}")));
    }
    Ok((bytes, kind))
}

/// A GET to Catbot at home, whichever place the app is showing (the phone copying a bot from the
/// PC, stage 26).
#[cfg(not(target_arch = "wasm32"))]
pub async fn from_home(path: &str) -> Result<Vec<u8>, String> {
    let link = crate::link::current().ok_or("this phone isn't paired with your PC")?;
    let response = client().get(format!("{}{path}", link.url)).bearer_auth(link.key).send().await.map_err(|e| format!("your PC didn't answer ({e})"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("your PC answered {status}"));
    }
    response.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
pub async fn from_home(_path: &str) -> Result<Vec<u8>, String> {
    Err("only the phone app copies from home".into())
}

/// A file's bytes (an attachment, say), with this device's key when it's paired.
#[cfg(not(target_arch = "wasm32"))]
pub async fn bytes(path: &str) -> Result<Vec<u8>, String> {
    let response = request(reqwest::Method::GET, path).send().await.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    check_paired(status);
    if !(200..300).contains(&status) {
        return Err(format!("{path} answered {status}"));
    }
    response.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

/// One client for the app's life (it keeps connections open between calls).
#[cfg(not(target_arch = "wasm32"))]
pub fn client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new).clone()
}
