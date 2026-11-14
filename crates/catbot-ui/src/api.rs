//! Talking to the server that served this app (`catbot serve`), over the same routes
//! OpenMausBot's interface uses.

use serde_json::Value;

/// GET a path on this server and read the answer as JSON.
pub async fn get(path: &str) -> Result<Value, String> {
    let response = gloo_net::http::Request::get(path).send().await.map_err(|e| e.to_string())?;
    if !response.ok() {
        return Err(format!("{path} answered {}", response.status()));
    }
    response.json::<Value>().await.map_err(|e| e.to_string())
}

/// POST JSON to a path on this server and read the answer as JSON. A refusal comes back as
/// the server's own words.
pub async fn post(path: &str, body: &Value) -> Result<Value, String> {
    let response = gloo_net::http::Request::post(path)
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| format!("Catbot didn't answer ({e})"))?;
    if !response.ok() {
        let said = response.text().await.unwrap_or_default();
        let said = serde_json::from_str::<Value>(&said).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(said);
        return Err(if said.trim().is_empty() { format!("{path} answered {}", response.status()) } else { said });
    }
    response.json::<Value>().await.map_err(|e| e.to_string())
}

/// PATCH or DELETE (or any method) with an optional JSON body; the answer as JSON.
pub async fn call(method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    use gloo_net::http::{Method, RequestBuilder};
    let method = match method {
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        "PUT" => Method::PUT,
        "POST" => Method::POST,
        _ => Method::GET,
    };
    let builder = RequestBuilder::new(path).method(method);
    let request = match body {
        Some(body) => builder.json(body).map_err(|e| e.to_string())?,
        None => builder.build().map_err(|e| e.to_string())?,
    };
    let response = request.send().await.map_err(|e| format!("Catbot didn't answer ({e})"))?;
    if !response.ok() {
        let said = response.text().await.unwrap_or_default();
        let said = serde_json::from_str::<Value>(&said).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(said);
        return Err(if said.trim().is_empty() { format!("{path} answered {}", response.status()) } else { said });
    }
    response.json::<Value>().await.or(Ok(Value::Null))
}
