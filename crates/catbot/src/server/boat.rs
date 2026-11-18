//! Boat cloud computers (stage 17.2): a bot's computer on boat.dev instead of in a container
//! here, when Moo chooses. Boat bills per second while one runs and nothing while it's stopped;
//! a stop saves its disk, and the next use resumes it.
//!
//! Catbot keeps an account key (settings `boat`, never sent back: only its last four
//! characters) and makes each bot one sandbox, with none of the account's secrets attached
//! (`noEnv`). A sandbox is Ubuntu with Xorg on `:0` at 1920×1080, Chrome, xdotool and ffmpeg,
//! user `user` with sudo. The API: <https://docs.boat.dev/api/v1.md>.

use std::time::Duration;

use serde_json::{Value, json};

use super::App;

const SETTING: &str = "boat";
/// Archived on its own after this long, in case Catbot isn't there to stop it.
const TTL_SECONDS: u64 = 8 * 60 * 60;
/// The states in which a sandbox takes commands.
const RUNNING: [&str; 3] = ["ready", "idle", "running"];
pub const WIDTH: i64 = 1920;
pub const HEIGHT: i64 = 1080;
/// The longest command Boat runs in one call.
pub const MOST_SECONDS: u64 = 600;

fn api() -> String {
    std::env::var("CATBOT_BOAT_API").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "https://boat.dev/api/v1".into()).trim_end_matches('/').to_string()
}

pub fn key(app: &App) -> Option<String> {
    let kept = app.with_store(|store| store.setting(SETTING)).flatten()?;
    kept["key"].as_str().filter(|k| !k.is_empty()).map(str::to_string)
}

/// What the settings show: connected or not, and the key's last four characters.
pub fn shown(app: &App) -> Value {
    match key(app) {
        Some(key) => json!({ "connected": true, "ends": key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>() }),
        None => json!({ "connected": false }),
    }
}

/// Saves a key once Boat accepts it; None forgets it.
pub async fn connect(app: &App, key: Option<&str>) -> Result<(), String> {
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        None => {
            app.with_store(|store| store.set_setting(SETTING, &Value::Null));
            Ok(())
        }
        Some(key) => {
            if !(key.starts_with("boat_") || key.starts_with("box_")) {
                return Err("A Boat API key starts with boat_ (make one at boat.dev, or with: boat api-key create catbot --preset ci).".into());
            }
            call_with(key, reqwest::Method::GET, "/sandboxes", None, Duration::from_secs(30)).await.map_err(|e| format!("Boat didn't take that key: {e}"))?;
            app.with_store(|store| store.set_setting(SETTING, &json!({ "key": key })));
            Ok(())
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(20)).build().unwrap_or_default()
}

async fn call_with(key: &str, method: reqwest::Method, path: &str, body: Option<Value>, wait: Duration) -> Result<(u16, Value), String> {
    let mut request = client().request(method, format!("{}{path}", api())).bearer_auth(key).timeout(wait);
    if let Some(body) = &body {
        request = request.json(body);
    }
    let answer = request.send().await.map_err(|e| format!("Boat didn't answer: {e}"))?;
    let status = answer.status().as_u16();
    let body: Value = answer.json().await.unwrap_or(Value::Null);
    if (200..300).contains(&status) && body["ok"] != false {
        return Ok((status, body));
    }
    let said = body["message"].as_str().or(body["error"]["message"].as_str()).unwrap_or("no reason given");
    let code = body["code"].as_str().or(body["error"]["code"].as_str()).unwrap_or_default();
    Err(match code {
        "billing_required" => "Boat wants a plan or a card on the account first (billing_required).".to_string(),
        "" => format!("{said} ({status})"),
        code => format!("{said} ({code})"),
    })
}

async fn call(app: &App, method: reqwest::Method, path: &str, body: Option<Value>, wait: Duration) -> Result<(u16, Value), String> {
    let key = key(app).ok_or("Boat isn't connected: add its key in Settings › Computers.")?;
    call_with(&key, method, path, body, wait).await
}

/// A sandbox's state ("ready", "archived"…), or None when Boat has no such sandbox.
pub async fn state(app: &App, id: &str) -> Result<Option<String>, String> {
    match call(app, reqwest::Method::GET, &format!("/sandboxes/{id}"), None, Duration::from_secs(30)).await {
        Ok((_, body)) => Ok(body["sandbox"]["state"].as_str().map(str::to_string).filter(|s| s != "cancelled")),
        Err(e) if e.contains("(404)") || e.contains("(not_found)") => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn is_running(state: &str) -> bool {
    RUNNING.contains(&state)
}

/// Brings a bot's sandbox up: resumes it, or makes one when it has none (or Boat lost it).
/// Gives back its id.
pub async fn bring_up(app: &App, id: &str, name: &str) -> Result<String, String> {
    let mut id = id.to_string();
    if !id.is_empty() {
        match state(app, &id).await? {
            None => id.clear(),
            Some(s) if is_running(&s) => return Ok(id),
            Some(s) if s == "archived" || s == "archiving" => {
                call(app, reqwest::Method::POST, &format!("/sandboxes/{id}/resume"), Some(json!({ "ttlSeconds": TTL_SECONDS })), Duration::from_secs(60)).await?;
            }
            Some(s) if s == "error" => return Err("Boat says this computer has a problem; Reset makes a fresh one.".into()),
            Some(_) => {}
        }
    }
    if id.is_empty() {
        let body = json!({ "name": format!("Catbot: {name}"), "ttlSeconds": TTL_SECONDS, "noEnv": true, "labels": { "catbot": "computer" } });
        let (_, made) = call(app, reqwest::Method::POST, "/sandboxes", Some(body), Duration::from_secs(60)).await?;
        id = made["sandbox"]["id"].as_str().ok_or("Boat made a computer but didn't say its id")?.to_string();
    }
    // Ready in about a minute; resuming a big one can take longer.
    for _ in 0..240 {
        match state(app, &id).await {
            Ok(Some(s)) if is_running(&s) => return Ok(id),
            Ok(Some(s)) if s == "error" => return Err("Boat couldn't start it.".into()),
            Ok(None) => return Err("Boat lost the computer while starting it.".into()),
            _ => tokio::time::sleep(Duration::from_millis(1500)).await,
        }
    }
    Err("Boat took too long to start it.".into())
}

pub async fn stop(app: &App, id: &str) -> Result<(), String> {
    match state(app, id).await? {
        Some(s) if is_running(&s) => {
            call(app, reqwest::Method::POST, &format!("/sandboxes/{id}/stop"), Some(json!({})), Duration::from_secs(60)).await?;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Deletes a sandbox and its saved disk for good.
pub async fn delete(app: &App, id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Ok(());
    }
    let key = key(app).ok_or("Boat isn't connected, so its computer couldn't be deleted there.")?;
    let request = client().delete(format!("{}/sandboxes/{id}", api())).bearer_auth(&key).header("X-Ascii-Confirm-Delete", id).timeout(Duration::from_secs(60));
    let answer = request.send().await.map_err(|e| format!("Boat didn't answer: {e}"))?;
    match answer.status().as_u16() {
        200..300 | 404 => Ok(()),
        status => {
            let body: Value = answer.json().await.unwrap_or(Value::Null);
            Err(format!("Boat didn't delete it: {} ({status})", body["message"].as_str().unwrap_or("no reason given")))
        }
    }
}

/// What a command gave back.
pub struct Output {
    pub code: Option<i32>,
    pub timed_out: bool,
    pub out: String,
    pub err: String,
}

/// Runs a shell command in the sandbox and waits for it (at most ten minutes).
pub async fn run(app: &App, id: &str, command: &str, seconds: u64) -> Result<Output, String> {
    let seconds = seconds.clamp(1, MOST_SECONDS);
    let body = json!({ "command": command, "timeoutSeconds": seconds });
    let (_, done) = call(app, reqwest::Method::POST, &format!("/sandboxes/{id}/commands"), Some(body), Duration::from_secs(seconds + 60)).await?;
    Ok(Output {
        code: done["exitCode"].as_i64().map(|c| c as i32),
        timed_out: done["timedOut"] == true,
        out: done["stdout"].as_str().unwrap_or_default().to_string(),
        err: done["stderr"].as_str().unwrap_or_default().to_string(),
    })
}

/// Starts a command and leaves it running (a browser window).
pub async fn start_command(app: &App, id: &str, command: &str) -> Result<(), String> {
    let body = json!({ "command": command, "detached": true });
    call(app, reqwest::Method::POST, &format!("/sandboxes/{id}/commands"), Some(body), Duration::from_secs(60)).await.map(|_| ())
}

/// Writes a file into the sandbox (under /home/user or /tmp).
pub async fn write_file(app: &App, id: &str, path: &str, content: &str) -> Result<(), String> {
    let body = json!({ "path": path, "content": content });
    call(app, reqwest::Method::PUT, &format!("/sandboxes/{id}/files"), Some(body), Duration::from_secs(60)).await.map(|_| ())
}

/// The screen as a JPEG: ffmpeg grabs it (ImageMagick if that fails), carried as base64 text.
pub async fn screenshot(app: &App, id: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let command = "(DISPLAY=:0 ffmpeg -loglevel error -f x11grab -video_size 1920x1080 -i :0 -frames:v 1 -q:v 6 -f mjpeg - 2>/dev/null || DISPLAY=:0 import -window root -quality 70 jpg:-) | base64 -w0";
    let done = run(app, id, command, 60).await?;
    let text: String = done.out.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(text).map_err(|_| "the screenshot came back garbled".to_string())?;
    if bytes.is_empty() {
        return Err(format!("the screenshot didn't work: {}", done.err.trim()));
    }
    Ok(bytes)
}

/// A link to watch the screen (noVNC at Boat), waiting while Boat prepares it.
pub async fn desktop_url(app: &App, id: &str, use_it: bool) -> Result<String, String> {
    for _ in 0..20 {
        let (_, said) = call(app, reqwest::Method::POST, &format!("/sandboxes/{id}/desktop?vnc=1"), Some(json!({ "publicAccess": false })), Duration::from_secs(30)).await?;
        if let Some(url) = said["desktopUrl"].as_str() {
            let glue = if url.contains('?') { '&' } else { '?' };
            let only = if use_it { 0 } else { 1 };
            return Ok(format!("{url}{glue}view_only={only}&resize=scale"));
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    Err("Boat is still getting the screen ready; try again in a moment.".into())
}

/// A word for the shell, quoted.
pub fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's; rm -rf ~"), r"'it'\''s; rm -rf ~'");
    }
}
