//! Published pages on Netlify or Cloudflare Pages (stage 13.12.2), as well as from Catbot itself.
//!
//! Moo's choices (2026-11-16): he picks where each page goes on its card, from a default set in
//! Settings › Pages; each page gets a site of its own (taking it down deletes that site); and
//! Cloudflare goes through Cloudflare's own tool, wrangler (it needs Node), not an upload of
//! Catbot's own. Netlify is reached directly, through its documented deploy API.
//!
//! The keys stay in Catbot's database and never go to a bot or back to the app.
//! For tests: `CATBOT_NETLIFY_API`, `CATBOT_CLOUDFLARE_API` and `CATBOT_WRANGLER` (the command,
//! words split on spaces) stand in for the real services.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use super::App;

const SETTING: &str = "page-hosts";

/// Where pages can go, and the keys for each.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hosts {
    /// "catbot", "netlify" or "cloudflare": what a new page's card starts on.
    pub default: String,
    pub netlify_token: String,
    pub cloudflare_token: String,
    pub cloudflare_account: String,
}

impl Hosts {
    /// The places a page can go now, in order: (id, the words for it).
    pub fn options(&self) -> Vec<(&'static str, &'static str)> {
        let mut list = vec![("catbot", "This computer (Catbot)")];
        if !self.netlify_token.is_empty() {
            list.push(("netlify", "Netlify"));
        }
        if !self.cloudflare_token.is_empty() && !self.cloudflare_account.is_empty() {
            list.push(("cloudflare", "Cloudflare Pages"));
        }
        list
    }

    /// The default, if it can still be used.
    pub fn start(&self) -> &'static str {
        self.options().into_iter().map(|(id, _)| id).find(|id| *id == self.default).unwrap_or("catbot")
    }

    /// As the app sees them: never the keys.
    pub fn shown(&self) -> Value {
        let tail = |t: &str| t.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>();
        json!({
            "default": self.start(),
            "netlify": (!self.netlify_token.is_empty()).then(|| json!({ "ends": tail(&self.netlify_token) })),
            "cloudflare": (!self.cloudflare_token.is_empty()).then(|| json!({ "account": self.cloudflare_account, "ends": tail(&self.cloudflare_token) })),
            "wrangler": wrangler_command().map(|c| c.join(" ")),
        })
    }
}

pub fn label(host: &str) -> &'static str {
    match host {
        "netlify" => "Netlify",
        "cloudflare" => "Cloudflare Pages",
        _ => "this computer",
    }
}

pub fn hosts(app: &App) -> Hosts {
    app.with_store(|store| store.setting(SETTING)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// `PUT /api/pages/hosts`: `default`, and `netlify: {token}` or `cloudflare: {token, account}`
/// to connect (each key is checked first) or `null` to disconnect.
pub async fn change(app: &App, body: &Value) -> Result<Value, String> {
    let mut h = hosts(app);
    match &body["netlify"] {
        Value::Null if body.get("netlify").is_some() => h.netlify_token.clear(),
        Value::Object(_) => {
            let token = body["netlify"]["token"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("the Netlify token is missing")?;
            netlify_check(token).await?;
            h.netlify_token = token.to_string();
        }
        _ => {}
    }
    match &body["cloudflare"] {
        Value::Null if body.get("cloudflare").is_some() => {
            h.cloudflare_token.clear();
            h.cloudflare_account.clear();
        }
        Value::Object(_) => {
            let token = body["cloudflare"]["token"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("the Cloudflare token is missing")?;
            let account = body["cloudflare"]["account"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("the Cloudflare account ID is missing")?;
            if !account.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("a Cloudflare account ID is 32 letters and digits, from the right of the Cloudflare dashboard".into());
            }
            cloudflare_check(token, account).await?;
            h.cloudflare_token = token.to_string();
            h.cloudflare_account = account.to_string();
        }
        _ => {}
    }
    if let Some(default) = body["default"].as_str() {
        if !["catbot", "netlify", "cloudflare"].contains(&default) {
            return Err("pages go to catbot, netlify or cloudflare".into());
        }
        h.default = default.to_string();
    }
    app.with_store(|store| store.set_setting(SETTING, &json!(h))).ok_or("couldn't save the settings")?;
    app.hub.send(json!({ "kind": "pages" }));
    Ok(h.shown())
}

fn client() -> &'static reqwest::Client {
    crate::http::client()
}

/// A name a service will take, made from the page's title: `catbot-garden-plan-3fa9c1`.
pub fn site_name(title: &str) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(30).collect();
    let slug = slug.trim_end_matches('-');
    let tail = &uuid::Uuid::new_v4().simple().to_string()[..6];
    if slug.is_empty() { format!("catbot-{tail}") } else { format!("catbot-{slug}-{tail}") }
}

// ---- Netlify -------------------------------------------------------------------------------

fn netlify_api() -> String {
    std::env::var("CATBOT_NETLIFY_API").unwrap_or_else(|_| "https://api.netlify.com/api/v1".into())
}

/// Netlify's answer as JSON, or what went wrong in words.
async fn netlify_call(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let resp = request.timeout(Duration::from_secs(60)).send().await.map_err(|e| format!("couldn't reach Netlify: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("Netlify didn't accept the token (make a new one under User settings › Applications › Personal access tokens)".into());
    }
    if !status.is_success() {
        let said = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["message"].as_str().or(v["errors"].as_str()).map(str::to_string)).unwrap_or(text);
        return Err(format!("Netlify said {status}: {}", said.chars().take(300).collect::<String>()));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn netlify_check(token: &str) -> Result<(), String> {
    netlify_call(client().get(format!("{}/user", netlify_api())).bearer_auth(token)).await.map(|_| ())
}

/// A path in a Netlify upload address, each part escaped.
fn escape_path(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Puts the files on Netlify: a new site, or the page's own one again. The site and its address.
pub async fn netlify_publish(token: &str, files: &[(PathBuf, String)], title: &str, site: Option<&str>) -> Result<(String, String), String> {
    let api = netlify_api();
    let http = client();
    let (site_id, mut url) = match site {
        Some(id) => {
            let found = netlify_call(http.get(format!("{api}/sites/{id}")).bearer_auth(token)).await?;
            (id.to_string(), found["ssl_url"].as_str().or(found["url"].as_str()).unwrap_or_default().to_string())
        }
        None => {
            // A name can be taken by someone else's site; a second try gets a new tail.
            let mut made = Err(String::new());
            for _ in 0..2 {
                made = netlify_call(http.post(format!("{api}/sites")).bearer_auth(token).json(&json!({ "name": site_name(title) }))).await;
                if made.is_ok() {
                    break;
                }
            }
            let made = made?;
            let id = made["id"].as_str().ok_or("Netlify didn't say which site it made")?.to_string();
            (id, made["ssl_url"].as_str().or(made["url"].as_str()).unwrap_or_default().to_string())
        }
    };
    // The digest: each file's SHA1. Netlify asks only for the files it doesn't have.
    let mut digest = BTreeMap::new();
    let mut by_hash: BTreeMap<String, &(PathBuf, String)> = BTreeMap::new();
    for file in files {
        let bytes = std::fs::read(&file.0).map_err(|e| format!("couldn't read {}: {e}", file.1))?;
        let hash = format!("{:x}", Sha1::digest(&bytes));
        digest.insert(format!("/{}", file.1), hash.clone());
        by_hash.entry(hash).or_insert(file);
    }
    let deploy = netlify_call(http.post(format!("{api}/sites/{site_id}/deploys")).bearer_auth(token).json(&json!({ "files": digest }))).await?;
    let deploy_id = deploy["id"].as_str().ok_or("Netlify didn't start the deploy")?.to_string();
    let required: HashSet<&str> = deploy["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    for (hash, (path, relative)) in &by_hash {
        if !required.contains(hash.as_str()) {
            continue;
        }
        let bytes = std::fs::read(path).map_err(|e| format!("couldn't read {relative}: {e}"))?;
        netlify_call(http.put(format!("{api}/deploys/{deploy_id}/files/{}", escape_path(relative))).bearer_auth(token).header("content-type", "application/octet-stream").body(bytes)).await?;
    }
    // Live once Netlify says it's ready.
    let mut state = deploy["state"].as_str().unwrap_or_default().to_string();
    for _ in 0..60 {
        if state == "ready" {
            break;
        }
        if state == "error" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(if cfg!(test) { 10 } else { 1000 })).await;
        let now = netlify_call(http.get(format!("{api}/deploys/{deploy_id}")).bearer_auth(token)).await?;
        state = now["state"].as_str().unwrap_or_default().to_string();
        if url.is_empty() {
            url = now["ssl_url"].as_str().unwrap_or_default().to_string();
        }
        if state == "error" {
            return Err(format!("Netlify couldn't put it up: {}", now["error_message"].as_str().unwrap_or("no reason given")));
        }
    }
    if state != "ready" {
        return Err("Netlify is still working on it; it may show up in a minute. Look in Netlify before publishing again".into());
    }
    let url = if url.ends_with('/') { url } else { format!("{url}/") };
    Ok((site_id, url))
}

pub async fn netlify_remove(token: &str, site: &str) -> Result<(), String> {
    match netlify_call(client().delete(format!("{}/sites/{site}", netlify_api())).bearer_auth(token)).await {
        Ok(_) => Ok(()),
        // Already gone from Netlify: gone here too.
        Err(why) if why.contains("404") => Ok(()),
        Err(why) => Err(why),
    }
}

// ---- Cloudflare, through wrangler -----------------------------------------------------------

fn cloudflare_api() -> String {
    std::env::var("CATBOT_CLOUDFLARE_API").unwrap_or_else(|_| "https://api.cloudflare.com/client/v4".into())
}

/// The token is checked as a user token and as an account token (Cloudflare has both).
async fn cloudflare_check(token: &str, account: &str) -> Result<(), String> {
    let api = cloudflare_api();
    for url in [format!("{api}/user/tokens/verify"), format!("{api}/accounts/{account}/tokens/verify")] {
        if let Ok(resp) = client().get(&url).timeout(Duration::from_secs(30)).bearer_auth(token).send().await
            && resp.status().is_success()
            && resp.json::<Value>().await.is_ok_and(|v| v["success"] == true)
        {
            return Ok(());
        }
    }
    Err("Cloudflare didn't accept the token (it needs Account › Cloudflare Pages › Edit)".into())
}

fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for candidate in [dir.join(name), dir.join(format!("{name}.cmd")), dir.join(format!("{name}.exe"))] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// How to run wrangler: the installed one, or through npx (which fetches it the first time).
pub fn wrangler_command() -> Option<Vec<String>> {
    if let Ok(given) = std::env::var("CATBOT_WRANGLER") {
        return Some(given.split_whitespace().map(str::to_string).collect());
    }
    if let Some(found) = find_program("wrangler") {
        return Some(vec![found.to_string_lossy().to_string()]);
    }
    find_program("npx").map(|npx| vec![npx.to_string_lossy().to_string(), "--yes".into(), "wrangler@4".into()])
}

/// Runs wrangler with the account's token; what it printed, or why it failed.
async fn wrangler(h: &Hosts, args: &[&str]) -> Result<String, String> {
    let command = wrangler_command().ok_or("Cloudflare pages need Node and wrangler on this computer (npm install -g wrangler)")?;
    let mut run = tokio::process::Command::new(&command[0]);
    run.args(&command[1..])
        .args(args)
        .env("CLOUDFLARE_API_TOKEN", &h.cloudflare_token)
        .env("CLOUDFLARE_ACCOUNT_ID", &h.cloudflare_account)
        .env("CI", "1")
        .env("WRANGLER_SEND_METRICS", "false")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(300), run.output()).await.map_err(|_| "wrangler took more than five minutes".to_string())?.map_err(|e| format!("couldn't start wrangler: {e}"))?;
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        return Ok(printed);
    }
    let said = format!("{printed}\n{}", String::from_utf8_lossy(&out.stderr));
    let last: Vec<&str> = said.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    Err(format!("wrangler failed: {}", last[last.len().saturating_sub(3)..].join(" ")))
}

/// The address Cloudflare gave a project (it adds a tail when the name's taken).
async fn cloudflare_address(h: &Hosts, project: &str) -> String {
    let fallback = format!("https://{project}.pages.dev/");
    let Ok(listed) = wrangler(h, &["pages", "project", "list", "--json"]).await else {
        return fallback;
    };
    let start = listed.find('[').unwrap_or(0);
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&listed[start..]) else {
        return fallback;
    };
    list.iter()
        .find(|p| p["Project Name"] == project)
        .and_then(|p| p["Project Domains"].as_str())
        .and_then(|d| d.split(',').map(str::trim).find(|d| d.ends_with(".pages.dev")))
        .map(|d| format!("https://{d}/"))
        .unwrap_or(fallback)
}

/// Puts the page on Cloudflare Pages from a copy of its files: a new project, or its own again.
pub async fn cloudflare_publish(h: &Hosts, copy: &Path, title: &str, project: Option<&str>) -> Result<(String, String), String> {
    let project = match project {
        Some(p) => p.to_string(),
        None => {
            let name = site_name(title);
            wrangler(h, &["pages", "project", "create", &name, "--production-branch", "main"]).await?;
            name
        }
    };
    let dir = copy.to_string_lossy().to_string();
    wrangler(h, &["pages", "deploy", &dir, "--project-name", &project, "--branch", "main", "--commit-dirty=true"]).await?;
    let url = cloudflare_address(h, &project).await;
    Ok((project, url))
}

pub async fn cloudflare_remove(h: &Hosts, project: &str) -> Result<(), String> {
    match wrangler(h, &["pages", "project", "delete", project, "--yes"]).await {
        Ok(_) => Ok(()),
        Err(why) if why.contains("not found") || why.contains("8000007") => Ok(()),
        Err(why) => Err(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths_are_made_safe() {
        let name = site_name("Spring Garden — plan #2!");
        assert!(name.starts_with("catbot-spring-garden-plan-2-"), "{name}");
        assert_eq!(name.len(), "catbot-spring-garden-plan-2-".len() + 6);
        assert!(site_name("???").starts_with("catbot-"));
        assert_eq!(escape_path("img/a b#1.png"), "img/a%20b%231.png");
    }

    #[test]
    fn only_connected_hosts_are_offered() {
        let mut h = Hosts { default: "netlify".into(), ..Default::default() };
        assert_eq!(h.start(), "catbot", "not connected: back to this computer");
        h.netlify_token = "abcdef123".into();
        assert_eq!(h.start(), "netlify");
        h.cloudflare_token = "t".into();
        assert_eq!(h.options().len(), 2, "Cloudflare needs its account too");
        assert_eq!(h.shown()["netlify"]["ends"], "f123");
        assert!(!h.shown().to_string().contains("abcdef"), "the key never goes back");
    }
}
