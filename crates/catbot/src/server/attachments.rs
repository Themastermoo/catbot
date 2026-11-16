//! Files attached to messages.
//!
//! - `POST /api/bots/:id/attachments`: the file itself as the body, its name in `X-File-Name`
//!   (percent-encoded) and its type in `Content-Type`. It's saved in the bot's folder under
//!   `attachments/`, where the bot can open it, and the answer describes it (with its `id`).
//! - A message sent with `attachments: [id, …]` carries them; the bot is told where they are,
//!   and a bot on a chat API is shown its pictures directly.
//! - `GET /api/attachments/:id`: the file, for the chat (pictures shown, others downloaded).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::store::{Attachment, Bot, Message};

/// The largest file taken.
pub const MOST_BYTES: usize = 25 * 1024 * 1024;

/// Kinds shown in the page as themselves; anything else is offered as a download.
const SHOWN: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf", "text/plain"];

pub async fn upload(app: &Arc<App>, bot: &Bot, req: Request) -> Answer {
    let name = req
        .headers()
        .get("x-file-name")
        .and_then(|v| v.to_str().ok())
        .map(decode)
        .unwrap_or_else(|| "file".into());
    let mime = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|m| m.split(';').next().unwrap_or("").trim().to_lowercase())
        .filter(|m| !m.is_empty() && m.contains('/'))
        .unwrap_or_else(|| guess(&name).to_string());
    let bytes = axum::body::to_bytes(req.into_body(), MOST_BYTES)
        .await
        .map_err(|_| (StatusCode::PAYLOAD_TOO_LARGE, format!("files can be at most {} MB", MOST_BYTES / 1024 / 1024)))?;
    if bytes.is_empty() {
        return Err(bad("the file is empty"));
    }
    let name = clean(&name);
    let dir = bot.folder.join("attachments");
    tokio::fs::create_dir_all(&dir).await.map_err(fail)?;
    let path = free_path(&dir, &format!("{}-{name}", chrono::Local::now().format("%Y-%m-%d")));
    tokio::fs::write(&path, &bytes).await.map_err(fail)?;
    let attachment = app.try_store(|store| store.add_attachment(&bot.id, &name, &path, &mime, bytes.len() as u64)).map_err(fail)?;
    Ok((StatusCode::CREATED, json!({ "attachment": describe(&attachment) })))
}

/// `GET /api/attachments/:id`.
pub async fn serve(app: &Arc<App>, id: &str) -> Response {
    let Some(attachment) = app.with_store(|store| store.attachment(id)).flatten() else {
        return (StatusCode::NOT_FOUND, "no such file").into_response();
    };
    let Ok(bytes) = tokio::fs::read(&attachment.path).await else {
        return (StatusCode::NOT_FOUND, "the file is gone from the bot's folder").into_response();
    };
    let shown = SHOWN.contains(&attachment.mime.as_str());
    let kind = if shown { attachment.mime.clone() } else { "application/octet-stream".into() };
    let disposition = format!("{}; filename=\"{}\"", if shown { "inline" } else { "attachment" }, attachment.name.replace('"', "'"));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, kind),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            (header::CACHE_CONTROL, "private, max-age=86400".into()),
        ],
        bytes,
    )
        .into_response()
}

/// An attachment for the interface and for a message's detail.
pub fn describe(a: &Attachment) -> Value {
    json!({
        "id": a.id,
        "name": a.name,
        "mime": a.mime,
        "size": a.size,
        "url": format!("/api/attachments/{}", a.id),
        "file": a.path.file_name().map(|n| n.to_string_lossy().into_owned()),
    })
}

/// The attachments a message is sent with: each this bot's own.
pub fn for_message(app: &App, bot: &Bot, ids: &Value) -> Result<Vec<Attachment>, (StatusCode, String)> {
    let mut found = Vec::new();
    for id in ids.as_array().into_iter().flatten().filter_map(Value::as_str) {
        let attachment = app
            .try_store(|store| store.attachment(id))
            .map_err(fail)?
            .filter(|a| a.bot_id == bot.id)
            .ok_or_else(|| bad(format!("no attachment {id} for this bot")))?;
        found.push(attachment);
    }
    Ok(found)
}

/// What a bot is told about a message's attachments, after the message.
pub fn note(attachments: &[Value]) -> String {
    let files: Vec<String> = attachments
        .iter()
        .filter_map(|a| {
            let file = a["file"].as_str()?;
            let kind = if a["mime"].as_str().is_some_and(|m| m.starts_with("image/")) { "a picture" } else { "a file" };
            Some(format!("attachments/{file} ({kind})"))
        })
        .collect();
    if files.is_empty() {
        return String::new();
    }
    format!("\n\n[Attached, in your working folder: {}. Open them to see them.]", files.join(", "))
}

/// A person's message as the bot reads it: the words, and where its attachments are.
pub fn person_text(m: &Message) -> String {
    let attached = m.detail.as_ref().and_then(|d| d["attachments"].as_array().cloned()).unwrap_or_default();
    format!("{}{}{}", m.body, note(&attached), super::reactions::note_of(m)).trim().to_string()
}

/// The pictures attached to a message, read from the bot's folder: (type, bytes).
pub fn pictures(m: &Message, folder: &Path) -> Vec<(String, Vec<u8>)> {
    let attached = m.detail.as_ref().and_then(|d| d["attachments"].as_array().cloned()).unwrap_or_default();
    attached
        .iter()
        .filter(|a| a["mime"].as_str().is_some_and(|m| ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&m)))
        .filter_map(|a| {
            let file = a["file"].as_str()?;
            let bytes = std::fs::read(folder.join("attachments").join(file)).ok()?;
            Some((a["mime"].as_str()?.to_string(), bytes))
        })
        .collect()
}

/// A name safe to save: no folders, no characters file systems or links trip on.
fn clean(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base.chars().map(|c| if "<>:\"|?*".contains(c) || c.is_control() { '-' } else { c }).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    let cleaned: String = cleaned.chars().take(120).collect();
    if cleaned.is_empty() { "file".into() } else { cleaned }
}

/// `dir/name`, or `name (2)` and so on if that's taken.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

fn guess(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "md" | "csv" | "log" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_and_kept_apart() {
        assert_eq!(clean("../../etc/passwd"), "passwd");
        assert_eq!(clean("C:\\Users\\me\\plan?.md"), "plan-.md");
        assert_eq!(clean("..."), "file");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), "x").unwrap();
        assert_eq!(free_path(dir.path(), "a.png"), dir.path().join("a (2).png"));
        assert_eq!(decode("garden%20plan.md"), "garden plan.md");
        assert_eq!(guess("Photo.JPG"), "image/jpeg");
    }

    #[test]
    fn the_bot_is_told_where_attachments_are() {
        let attached = vec![json!({ "file": "2026-11-15-a.png", "mime": "image/png" }), json!({ "file": "2026-11-15-b.csv", "mime": "text/plain" })];
        let said = note(&attached);
        assert!(said.contains("attachments/2026-11-15-a.png (a picture)") && said.contains("attachments/2026-11-15-b.csv (a file)"), "{said}");
        assert_eq!(note(&[]), "");
    }
}
