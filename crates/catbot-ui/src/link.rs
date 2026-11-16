//! The app's link to Catbot at home: the public address and this device's key, kept after
//! pairing (`pair.rs`) and sent with every request.
//!
//! - The phone app has only the link: it pairs before anything else.
//! - The Windows app has two places (`has_own`): Catbot on this PC, which it starts itself
//!   (`local.rs`), and Catbot at home through the link, paired when first wanted. A switch in
//!   the sidebar picks one; the choice is kept.
//! - The Linux desktop app is at home: no link.
//!
//! Kept in a small JSON file: in the Android app, in its own private storage; on Windows in
//! `%LOCALAPPDATA%\Catbot\home-link.json`; elsewhere only if `CATBOT_LINK_FILE` names one (for
//! trying pairing out on a computer; `CATBOT_PLACES=1` tries the two places).

#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::sync::RwLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Catbot's public address, e.g. https://mooterminalmain.tail1234.ts.net
    pub url: String,
    /// This device's key, as `Authorization: Bearer`.
    pub key: String,
    /// What home calls this device.
    pub name: String,
}

static CURRENT: RwLock<Option<Option<Link>>> = RwLock::new(None);

/// Where the link is kept, if this kind of app keeps one.
fn file() -> Option<std::path::PathBuf> {
    if let Some(file) = std::env::var_os("CATBOT_LINK_FILE") {
        return Some(file.into());
    }
    #[cfg(target_os = "android")]
    return Some(std::path::PathBuf::from("/data/data/com.moo.catbot/files/catbot-link.json"));
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA").map(|dir| std::path::PathBuf::from(dir).join("Catbot").join("home-link.json"));
    #[cfg(not(any(target_os = "android", windows)))]
    return None;
}

/// Whether this app runs its own Catbot beside the link to home (the Windows app).
pub fn has_own() -> bool {
    cfg!(all(windows, not(target_arch = "wasm32"))) || std::env::var_os("CATBOT_PLACES").is_some_and(|v| v == "1")
}

/// Whether this app must pair before it can do anything (the phone app).
pub fn pairs() -> bool {
    file().is_some() && !has_own()
}

/// Where the Windows app is talking: Catbot on this PC, or Catbot at home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    ThisPc,
    Home,
}

fn place_file() -> Option<std::path::PathBuf> {
    file().map(|f| f.with_file_name("place"))
}

/// The place chosen last; at home only while paired.
pub fn place() -> Place {
    if !has_own() {
        return if current().is_some() { Place::Home } else { Place::ThisPc };
    }
    let home = place_file().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|s| s.trim() == "home");
    if home && current().is_some() { Place::Home } else { Place::ThisPc }
}

/// Keeps the choice of place.
pub fn set_place(place: Place) {
    if let Some(file) = place_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, if place == Place::Home { "home" } else { "this-pc" });
    }
}

/// The link requests go through now: the phone's always; the Windows app's only while it's at
/// home.
pub fn active() -> Option<Link> {
    if has_own() && place() != Place::Home {
        return None;
    }
    current()
}

/// The link, if paired.
pub fn current() -> Option<Link> {
    if let Some(known) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return known.clone();
    }
    let loaded = file().and_then(|path| std::fs::read(path).ok()).and_then(|bytes| serde_json::from_slice::<Link>(&bytes).ok());
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded.clone());
    loaded
}

/// Keeps a new link (after pairing).
pub fn save(link: Link) -> Result<(), String> {
    if let Some(path) = file() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(&link).map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| format!("couldn't keep the pairing: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Some(link));
    Ok(())
}

/// Forgets the link: home removed this device, or the person unpaired it here.
pub fn forget() {
    if let Some(path) = file() {
        let _ = std::fs::remove_file(path);
    }
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(None);
}

/// What a pairing QR code says: `catbot://pair?url=…&code=…` → (address, code).
pub fn parse_invite(text: &str) -> Option<(String, String)> {
    let query = text.trim().strip_prefix("catbot://pair?")?;
    let (mut url, mut code) = (None, None);
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let value = decode(value)?;
        match key {
            "url" => url = Some(value),
            "code" => code = Some(value),
            _ => {}
        }
    }
    let url = url?.trim().trim_end_matches('/').to_string();
    (url.starts_with("https://") || url.starts_with("http://")).then_some(())?;
    Some((url, code?))
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invites_read_back() {
        let link = crate::devices::pairing_link("https://moo.tail1.ts.net", "ABCDEFGHJK");
        assert_eq!(parse_invite(&link), Some(("https://moo.tail1.ts.net".into(), "ABCDEFGHJK".into())));
        assert_eq!(parse_invite("https://example.com"), None);
        assert_eq!(parse_invite("catbot://pair?url=ftp%3A%2F%2Fx&code=A"), None);
        assert_eq!(parse_invite("catbot://pair?code=A"), None);
    }
}
