//! Published pages (stage 13.12): a page a bot made, put online at a link.
//!
//! Moo's choice (2026-11-16): both. By default Catbot serves it itself, from this computer
//! through the remote door (Tailscale Funnel), at an address nobody can guess; up only while the
//! computer is on. Or, when he chooses, on his own Netlify or Cloudflare Pages account (stage
//! 13.12.2). A bot asks first: the person sees what would go online and approves it. Every page
//! is listed in Settings › Pages, where it can be taken down.
//!
//! A page is served as its own sandboxed origin (Content-Security-Policy `sandbox`), so nothing
//! on it can reach Catbot's own routes.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bot_tools::Grant;
use super::{Answer, App, bad, fail};
use crate::store::Role;

/// What a page may hold, and how much (MausBot's limits).
pub const MOST_BYTES: u64 = 5 * 1024 * 1024;
pub const MOST_FILES: usize = 200;
const SETTING: &str = "pages";

/// The kinds of file a page may have, and how each is served.
pub fn content_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "geojson" | "topojson" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// One published page, kept in the settings as `pages`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub bot_id: String,
    pub thread: String,
    pub published_at: String,
    /// "catbot", "netlify" or "cloudflare".
    pub host: String,
    /// The page's own site there: Netlify's site id, or Cloudflare's project name.
    pub site: String,
    pub url: String,
    pub files: usize,
    pub bytes: u64,
}

pub fn pages(app: &App) -> Vec<Page> {
    app.with_store(|store| store.setting(SETTING)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn keep(app: &App, list: &[Page]) -> Result<(), String> {
    app.with_store(|store| store.set_setting(SETTING, &json!(list))).ok_or_else(|| "couldn't save the list of pages".to_string())?;
    app.hub.send(json!({ "kind": "pages" }));
    Ok(())
}

fn pages_dir(app: &App) -> Option<PathBuf> {
    app.with_store(|store| Ok(store.home().join("pages")))
}

/// The files of a page to publish: a folder (with an index.html) or one HTML file, each with
/// where it goes on the page. Checked against the limits, refusing symlinks.
pub fn gather(source: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let meta = std::fs::symlink_metadata(source).map_err(|_| format!("there's nothing at {}", source.display()))?;
    if meta.file_type().is_symlink() {
        return Err("symlinks can't be published as pages".into());
    }
    let mut files = Vec::new();
    if meta.is_file() {
        if !matches!(content_type(source), Some(t) if t.starts_with("text/html")) {
            return Err("a single file to publish must be an .html page".into());
        }
        files.push((source.to_path_buf(), "index.html".to_string()));
    } else if meta.is_dir() {
        let index = source.join("index.html");
        if !std::fs::symlink_metadata(&index).is_ok_and(|m| m.is_file()) {
            return Err("the folder needs an index.html, the page people open first".into());
        }
        fn walk(dir: &Path, base: &Path, files: &mut Vec<(PathBuf, String)>) -> Result<(), String> {
            for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue; // .claude, .git and the like stay home.
                }
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_symlink() {
                    return Err(format!("{} is a symlink, which a published page can't include", path.strip_prefix(base).unwrap_or(&path).display()));
                }
                if kind.is_dir() {
                    walk(&path, base, files)?;
                } else if kind.is_file() {
                    if content_type(&path).is_none() {
                        return Err(format!("{} isn't a kind of file a page can have", path.strip_prefix(base).unwrap_or(&path).display()));
                    }
                    let relative = path.strip_prefix(base).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
                    files.push((path, relative));
                }
                if files.len() > MOST_FILES {
                    return Err(format!("a page can have at most {MOST_FILES} files"));
                }
            }
            Ok(())
        }
        walk(source, source, &mut files)?;
    } else {
        return Err(format!("there's nothing at {}", source.display()));
    }
    let bytes: u64 = files.iter().filter_map(|(p, _)| std::fs::symlink_metadata(p).ok()).map(|m| m.len()).sum();
    if bytes > MOST_BYTES {
        return Err(format!("a page can be at most {} MB", MOST_BYTES / 1024 / 1024));
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(files)
}

fn size_words(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    }
}

/// Where Catbot's own pages are seen from: the public address through the remote door, or (with
/// no door) only this computer.
pub(super) async fn base_url(app: &App) -> (String, bool) {
    if app.remote.get().is_some()
        && let Some(public) = super::devices::public_url().await
    {
        return (public, true);
    }
    let local = app.local.get().map(|a| format!("http://{}:{}", if a.ip().is_unspecified() { "127.0.0.1".to_string() } else { a.ip().to_string() }, a.port())).unwrap_or_default();
    (local, false)
}

/// The tools every bot has for pages.
pub fn tools() -> Vec<Value> {
    vec![json!({
        "name": "page_publish",
        "description": "Put a page you made online at a link the person can share: an .html file, or a folder with an index.html (and its CSS, scripts, pictures and data; at most 200 files, 5 MB). The person sees what would go online and approves it first; you'll hear the link with their next message. To update a page you published before, give its id as update.",
        "inputSchema": { "type": "object", "properties": {
            "path": { "type": "string", "description": "The file or folder, in your working folder." },
            "title": { "type": "string", "description": "What the page is, in a few words." },
            "update": { "type": "string", "description": "The id of a page you published before, to replace it." },
        }, "required": ["path", "title"] },
    })]
}

/// `page_publish`: a card with what would go online, published only when approved.
pub async fn propose(app: &Arc<App>, grant: &Grant, args: &Value) -> Result<String, String> {
    let bot = app.with_store(|store| store.bot_by_id(&grant.bot_id)).flatten().ok_or("the bot is gone")?;
    // In a channel the bot works in the channel's folder.
    let folder = app
        .with_store(|store| store.conversation(&grant.thread))
        .flatten()
        .and_then(|c| c.channel_id)
        .and_then(|id| app.with_store(|store| store.channel(&id)).flatten())
        .map(|c| c.folder)
        .unwrap_or_else(|| bot.folder.clone());
    let wanted = args["path"].as_str().map(str::trim).filter(|p| !p.is_empty()).ok_or("which file or folder?")?;
    let path = if Path::new(wanted).is_absolute() { PathBuf::from(wanted) } else { folder.join(wanted) };
    let source = path.canonicalize().map_err(|_| format!("there's nothing at {wanted}"))?;
    let home = folder.canonicalize().unwrap_or(folder.clone());
    if !source.starts_with(&home) {
        return Err("only files in your own working folder can be published".into());
    }
    let files = gather(&source)?;
    let bytes: u64 = files.iter().filter_map(|(p, _)| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    let title = args["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).unwrap_or("A page").to_string();
    let update = match args["update"].as_str().map(str::trim).filter(|u| !u.is_empty()) {
        Some(id) => Some(pages(app).into_iter().find(|p| p.id == id && p.bot_id == bot.id).ok_or_else(|| format!("you haven't published a page {id}"))?),
        None => None,
    };
    let (_, public) = base_url(app).await;
    let hosts = super::page_hosts::hosts(app);
    // An update stays where it is; a new page's card asks where, when there's a choice.
    let options = if update.is_some() { Vec::new() } else { hosts.options() };
    let shown: Vec<&str> = files.iter().take(8).map(|(_, r)| r.as_str()).collect();
    let mut lines = vec![format!("Files: {}{} ({} in all, {})", shown.join(", "), if files.len() > 8 { ", …" } else { "" }, files.len(), size_words(bytes))];
    let here = if public {
        "served from this computer, at a link only those you give it to can find; up while the computer is on"
    } else {
        "served from this computer, for this computer only (set up the remote door for a public link)"
    };
    match &update {
        Some(old) if old.host != "catbot" => lines.push(format!("Where: {}, replacing the page at {}", super::page_hosts::label(&old.host), old.url)),
        Some(old) => lines.push(format!("Where: {here}; replaces the page at {}", old.url)),
        None if options.len() > 1 => lines.push(if public {
            "Where: pick below. On this computer it's up while the computer is on".to_string()
        } else {
            "Where: pick below. On this computer only this computer can open it (set up the remote door for a public link)".to_string()
        }),
        None => lines.push(format!("Where: {here}")),
    }
    let public = public || options.len() > 1 || update.as_ref().is_some_and(|u| u.host != "catbot");
    let detail = json!({
        "kind": "team-change",
        "heading": format!("{} wants to publish a page", bot.name),
        "askedBy": bot.name,
        "title": if update.is_some() { format!("Update “{title}”") } else { format!("Publish “{title}”") },
        "lines": lines,
        "warning": public.then(|| "Anyone with the link can open it.".to_string()),
        "action": "publish_page",
        "target": bot.id,
        "body": { "source": source, "title": title, "botId": bot.id, "thread": grant.thread, "update": update.map(|u| u.id) },
        "state": "asking",
    });
    let mut detail = detail;
    if options.len() > 1 {
        detail["choice"] = json!({
            "label": "Where",
            "options": options.iter().map(|(id, words)| json!({ "id": id, "label": words })).collect::<Vec<_>>(),
            "value": hosts.start(),
        });
    }
    super::turns::save_and_send(app, &grant.thread, Role::Step, &format!("Page: {title}"), Some(&detail));
    Ok("It's waiting for the person's approval in the chat. You'll hear the link with their next message. Don't publish it again.".into())
}

fn new_id() -> String {
    // 32 letters and digits: nobody guesses a link.
    format!("{}", uuid::Uuid::new_v4().simple())
}

/// An approved page: copied out of the bot's folder (later changes there don't show until it's
/// published again) and listed. The link, in words.
pub async fn publish(app: &Arc<App>, body: &Value) -> Result<String, String> {
    let raw_source = PathBuf::from(body["source"].as_str().unwrap_or_default());
    let bot_id = body["botId"].as_str().unwrap_or_default();
    let thread_id = body["thread"].as_str().unwrap_or_default();
    // Re-verify at publish time that `source` still lives inside the bot's (or channel's) working folder.
    let bot = app.with_store(|store| store.bot_by_id(bot_id)).flatten().ok_or("the bot is gone")?;
    let folder = app
        .with_store(|store| store.conversation(thread_id))
        .flatten()
        .and_then(|c| c.channel_id)
        .and_then(|id| app.with_store(|store| store.channel(&id)).flatten())
        .map(|c| c.folder)
        .unwrap_or_else(|| bot.folder.clone());
    let home = folder.canonicalize().map_err(|_| "the bot's folder isn't readable".to_string())?;
    let source = raw_source.canonicalize().map_err(|_| format!("there's nothing at {}", raw_source.display()))?;
    if !source.starts_with(&home) {
        return Err("only files in the bot's own working folder can be published".into());
    }
    let files = gather(&source)?;
    let dir = pages_dir(app).ok_or("no pages folder")?;
    let mut list = pages(app);
    let old = body["update"].as_str().and_then(|u| list.iter().find(|p| p.id == u)).cloned();
    let id = old.as_ref().map(|p| p.id.clone()).unwrap_or_else(new_id);
    let title = body["title"].as_str().unwrap_or("A page").to_string();
    let host = match &old {
        Some(p) => p.host.clone(),
        None => body["choice"].as_str().unwrap_or("catbot").to_string(),
    };
    let bytes: u64 = files.iter().filter_map(|(p, _)| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    let mut page = Page {
        id: id.clone(),
        title: title.clone(),
        bot_id: body["botId"].as_str().unwrap_or_default().to_string(),
        thread: body["thread"].as_str().unwrap_or_default().to_string(),
        published_at: chrono::Utc::now().to_rfc3339(),
        host: host.clone(),
        url: String::new(),
        site: old.as_ref().map(|p| p.site.clone()).unwrap_or_default(),
        files: files.len(),
        bytes,
    };
    if host != "catbot" {
        let hosts = super::page_hosts::hosts(app);
        let site = (!page.site.is_empty()).then_some(page.site.as_str());
        let (site, url) = match host.as_str() {
            "netlify" if !hosts.netlify_token.is_empty() => super::page_hosts::netlify_publish(&hosts.netlify_token, &files, &title, site).await?,
            "cloudflare" if !hosts.cloudflare_token.is_empty() => {
                // wrangler uploads a folder: a copy of just the page's files.
                let copy = dir.join(format!(".{id}-cloudflare"));
                copy_files(&files, &copy)?;
                let done = super::page_hosts::cloudflare_publish(&hosts, &copy, &title, site).await;
                let _ = std::fs::remove_dir_all(&copy);
                done?
            }
            _ => return Err(format!("{} isn't connected any more (Settings › Pages)", super::page_hosts::label(&host))),
        };
        page.site = site;
        page.url = url.clone();
        list.retain(|p| p.id != id);
        list.push(page);
        keep(app, &list)?;
        return Ok(format!("It's at {url}, on {} (page id {id}).", super::page_hosts::label(&host)));
    }
    let target = dir.join(&id);
    let staging = dir.join(format!(".{id}-new"));
    copy_files(&files, &staging)?;
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
    let (base, public) = base_url(app).await;
    let url = format!("{base}/p/{id}/");
    page.url = url.clone();
    list.retain(|p| p.id != id);
    list.push(page);
    keep(app, &list)?;
    Ok(if public { format!("It's at {url} (page id {id}).") } else { format!("It's at {url}, on this computer only (page id {id}).") })
}

/// The page's files, into a fresh folder.
fn copy_files(files: &[(PathBuf, String)], into: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(into);
    for (from, relative) in files {
        let meta = std::fs::symlink_metadata(from).map_err(|e| format!("couldn't read {relative}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{relative} is not a regular file"));
        }
        let to = into.join(relative);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(from, &to).map_err(|e| format!("couldn't copy {relative}: {e}"))?;
    }
    Ok(())
}

/// `GET /p/:id/*path`: a published page's file, sandboxed. A folder means its index.html.
pub async fn serve(app: &App, id: &str, rest: &[&str]) -> Response {
    let missing = || {
        let mut response = Response::new(Body::from("This page isn't here. It may have been taken down."));
        *response.status_mut() = StatusCode::NOT_FOUND;
        response
    };
    if id.len() != 32 || !id.chars().all(|c| c.is_ascii_hexdigit()) || !pages(app).iter().any(|p| p.id == id && p.host == "catbot") {
        return missing();
    }
    let Some(dir) = pages_dir(app) else {
        return missing();
    };
    let mut path = dir.join(id);
    for part in rest.iter().filter(|p| !p.is_empty()) {
        let decoded = super::urlencoding_decode(part);
        if Path::new(&decoded).components().any(|c| !matches!(c, Component::Normal(_))) || decoded.contains('/') || decoded.contains('\\') {
            return missing();
        }
        path.push(decoded);
    }
    if path.is_dir() {
        path.push("index.html");
    }
    let Some(kind) = content_type(&path) else {
        return missing();
    };
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return missing();
    };
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox allow-scripts allow-forms allow-popups allow-modals allow-downloads"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// `GET /api/pages`; at home `DELETE /api/pages/:id` takes one down, and `GET`/`PUT
/// /api/pages/hosts` are where pages can go.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    match (method, rest) {
        (&Method::GET, ["hosts"]) => Ok((StatusCode::OK, json!({ "hosts": super::page_hosts::hosts(app).shown() }))),
        (&Method::PUT, ["hosts"]) => {
            let shown = super::page_hosts::change(app, &body.unwrap_or_default()).await.map_err(bad)?;
            Ok((StatusCode::OK, json!({ "hosts": shown })))
        }
        (&Method::GET, []) => {
            let names: std::collections::HashMap<String, String> = app.with_store(|store| store.bot_names()).unwrap_or_default().into_iter().collect();
            let list: Vec<Value> = pages(app)
                .into_iter()
                .rev()
                .map(|p| {
                    let bot = names.get(&p.bot_id).cloned().unwrap_or_default();
                    let mut v = json!(p);
                    v["botName"] = json!(bot);
                    v
                })
                .collect();
            Ok((StatusCode::OK, json!({ "pages": list })))
        }
        (&Method::DELETE, [id]) => {
            let mut list = pages(app);
            let page = list.iter().find(|p| p.id == *id).cloned().ok_or_else(|| bad("no such page"))?;
            // On a service: its site is deleted there first, or the page stays listed.
            let hosts = super::page_hosts::hosts(app);
            let gone = match page.host.as_str() {
                "netlify" if !page.site.is_empty() => super::page_hosts::netlify_remove(&hosts.netlify_token, &page.site).await,
                "cloudflare" if !page.site.is_empty() => super::page_hosts::cloudflare_remove(&hosts, &page.site).await,
                _ => Ok(()),
            };
            gone.map_err(|why| (StatusCode::BAD_GATEWAY, format!("couldn't take it down from {}: {why}", super::page_hosts::label(&page.host))))?;
            if let Some(dir) = pages_dir(app) {
                let _ = std::fs::remove_dir_all(dir.join(&page.id));
            }
            list.retain(|p| p.id != page.id);
            keep(app, &list).map_err(fail)?;
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such pages route".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_gathered_within_its_limits() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(site.join("img")).unwrap();
        std::fs::write(site.join("index.html"), "<h1>Hi</h1>").unwrap();
        std::fs::write(site.join("img/a.png"), [0u8; 10]).unwrap();
        std::fs::create_dir_all(site.join(".claude")).unwrap();
        std::fs::write(site.join(".claude/secret.md"), "no").unwrap();
        let files: Vec<String> = gather(&site).unwrap().into_iter().map(|(_, r)| r).collect();
        assert_eq!(files, ["img/a.png", "index.html"], "hidden folders stay home");
        std::fs::write(site.join("run.exe"), "x").unwrap();
        assert!(gather(&site).unwrap_err().contains("run.exe"));
        assert!(gather(&dir.path().join("nope")).is_err());
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert!(gather(&dir.path().join("notes.txt")).is_err(), "one file must be a page");
    }
}
