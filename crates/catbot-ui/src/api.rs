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
