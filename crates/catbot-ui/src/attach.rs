//! Attachments in the chat: picking and uploading files for a message, and showing them.
//!
//! A file is uploaded as soon as it's picked (`POST /api/bots/:id/attachments`, the bytes as
//! the body), so sending is quick; the message then names it. Pictures show as thumbnails,
//! anything else as a chip that opens the file. In the browser a picture is shown from its
//! address; in the apps (which reach Catbot with a key, or from another origin) it is fetched
//! and shown from its bytes.

use dioxus::prelude::*;
use serde_json::Value;

/// A file on a message.
#[derive(Clone, Debug, PartialEq)]
pub struct Attached {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// `/api/attachments/<id>`.
    pub url: String,
}

impl Attached {
    pub fn from_wire(wire: &Value) -> Option<Attached> {
        Some(Attached {
            id: wire["id"].as_str()?.to_string(),
            name: wire["name"].as_str().unwrap_or("file").to_string(),
            mime: wire["mime"].as_str().unwrap_or("application/octet-stream").to_string(),
            size: wire["size"].as_u64().unwrap_or(0),
            url: wire["url"].as_str().unwrap_or_default().to_string(),
        })
    }

    pub fn list(wire: &Value) -> Vec<Attached> {
        wire.as_array().map(|list| list.iter().filter_map(Attached::from_wire).collect()).unwrap_or_default()
    }

    pub fn is_picture(&self) -> bool {
        ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&self.mime.as_str())
    }
}

/// "1.2 MB", "340 KB", "12 bytes".
pub fn size_words(size: u64) -> String {
    match size {
        s if s >= 1024 * 1024 => format!("{:.1} MB", s as f64 / 1024.0 / 1024.0),
        s if s >= 1024 => format!("{} KB", s / 1024),
        s => format!("{s} bytes"),
    }
}

/// Uploads a picked file for a bot; the attachment the server made of it.
pub async fn upload(bot_id: &str, name: &str, mime: &str, bytes: Vec<u8>) -> Result<Attached, String> {
    let answer = crate::api::upload(&format!("/api/bots/{bot_id}/attachments"), name, mime, bytes).await?;
    Attached::from_wire(&answer["attachment"]).ok_or_else(|| "the upload came back without its file".to_string())
}

/// A message's attachments: pictures as thumbnails, other files as chips. `end` lines them up
/// on the right (the person's messages).
#[component]
pub fn Attachments(files: Vec<Attached>, end: bool) -> Element {
    if files.is_empty() {
        return rsx! {};
    }
    let side = if end { "justify-end" } else { "justify-start" };
    rsx! {
        div { class: "flex w-full flex-wrap gap-2 {side}", "data-attachments": true,
            for file in files {
                if file.is_picture() {
                    Picture { key: "{file.id}", file: file.clone() }
                } else {
                    FileChip { key: "{file.id}", file: file.clone() }
                }
            }
        }
    }
}

#[component]
fn Picture(file: Attached) -> Element {
    let source = use_resource({
        let file = file.clone();
        move || {
            let file = file.clone();
            async move { picture_source(&file).await }
        }
    });
    let alt = file.name.clone();
    rsx! {
        match source() {
            Some(Some(src)) => rsx! {
                a { href: "{src}", target: "_blank", rel: "noreferrer", title: "{alt}",
                    img { src: "{src}", alt: "{alt}", "data-attachment-picture": true, class: "max-h-[220px] max-w-[min(320px,70vw)] rounded-xl border border-hairline/40 object-cover" }
                }
            },
            Some(None) => rsx! { FileChip { file: file.clone() } },
            None => rsx! { div { class: "h-[120px] w-[160px] animate-pulse rounded-xl bg-raised" } },
        }
    }
}

#[component]
fn FileChip(file: Attached) -> Element {
    let size = size_words(file.size);
    let open = file.clone();
    rsx! {
        button {
            r#type: "button",
            "data-attachment-file": true,
            title: "Open {file.name}",
            class: "flex max-w-[280px] items-center gap-2 rounded-xl border border-hairline/40 bg-card px-3 py-2 text-start hover:bg-raised",
            onclick: move |_| {
                let file = open.clone();
                spawn(async move { open_file(&file).await });
            },
            // Lucide `File` (ISC).
            svg { "aria-hidden": "true", class: "shrink-0 text-ink-secondary", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.75", stroke_linecap: "round", stroke_linejoin: "round",
                path { d: "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" }
                path { d: "M14 2v4a2 2 0 0 0 2 2h4" }
            }
            span { class: "min-w-0",
                span { class: "block truncate text-[13px] text-ink", "{file.name}" }
                span { class: "block text-[11px] text-ink-tertiary", "{size}" }
            }
        }
    }
}

/// Where a picture can be drawn from.
async fn picture_source(file: &Attached) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        Some(file.url.clone())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use base64::Engine as _;
        let bytes = crate::api::bytes(&file.url).await.ok()?;
        Some(format!("data:{};base64,{}", file.mime, base64::engine::general_purpose::STANDARD.encode(bytes)))
    }
}

/// Opens a file: in the browser, its address in a new tab; in the apps, saved to Downloads
/// and opened there.
async fn open_file(file: &Attached) {
    #[cfg(target_arch = "wasm32")]
    {
        let url = serde_json::to_string(&file.url).unwrap_or_default();
        document::eval(&format!("window.open({url}, '_blank')"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(bytes) = crate::api::bytes(&file.url).await else { return };
        let dir = downloads();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(&file.name);
        if std::fs::write(&path, bytes).is_ok() {
            open_path(&path);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn downloads() -> std::path::PathBuf {
    #[cfg(target_os = "android")]
    return std::path::PathBuf::from("/storage/emulated/0/Download");
    #[cfg(not(target_os = "android"))]
    {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from).unwrap_or_default();
        home.join("Downloads")
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn open_path(path: &std::path::Path) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "android")]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attachments_read_from_the_wire() {
        let list = Attached::list(&json!([{ "id": "a1", "name": "leaf.png", "mime": "image/png", "size": 2048, "url": "/api/attachments/a1" }, { "name": "no id" }]));
        assert_eq!(list.len(), 1);
        assert!(list[0].is_picture());
        assert_eq!(size_words(2048), "2 KB");
        assert_eq!(size_words(3 * 1024 * 1024 / 2), "1.5 MB");
    }
}
