//! The phone app's link to Catbot at home: the public address and this device's key, kept
//! after pairing (`pair.rs`) and sent with every request. Without a link the app talks to
//! Catbot on this computer, as the desktop app always does.
//!
//! Kept in a small JSON file: in the Android app, in its own private storage; elsewhere only
//! if `CATBOT_LINK_FILE` names one (for trying pairing out on a computer).

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
    #[cfg(target_os = "android")]
    return Some(std::path::PathBuf::from("/data/data/com.moo.catbot/files/catbot-link.json"));
    #[cfg(not(target_os = "android"))]
    return std::env::var_os("CATBOT_LINK_FILE").map(std::path::PathBuf::from);
}

/// Whether this app is one that pairs (the phone app), rather than talking to this computer.
pub fn pairs() -> bool {
    file().is_some()
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
