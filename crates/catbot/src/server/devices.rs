//! Pairing devices to reach Catbot from away, managed from home.
//!
//! At home (the local door), the interface asks for a pairing: Catbot makes a one-time code,
//! good for ten minutes and five tries, and the interface shows it as a QR code together with
//! Catbot's public address. The phone app scans it and trades the code at the remote door
//! (`remote.rs`) for its own key. Catbot keeps only a hash of each key, so a key can't be read
//! back out of the store; removing a device ends its key and any live stream it has open.
//!
//! Routes (home only):
//! * `GET    /api/devices`          paired devices, the pairing waiting (if any), the address
//! * `POST   /api/devices/pairing`  a new pairing code (replacing any waiting one)
//! * `DELETE /api/devices/pairing`  cancel the waiting code
//! * `DELETE /api/devices/:id`      unpair a device

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Answer, App, fail};
use crate::store::Device;

/// How long a pairing code works.
pub const CODE_LIFE: Duration = Duration::from_secs(10 * 60);
/// Wrong tries before a code stops working.
pub const TRIES: u32 = 5;
/// Letters a code is made of: no 0/O, 1/I/L, so it can also be typed.
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// A code's length: 10 letters from 31 is about 49 bits.
const CODE_LEN: usize = 10;

/// The one pairing code waiting to be used.
pub struct Pairing {
    pub code: String,
    pub until: Instant,
    pub tries: u32,
}

impl Pairing {
    fn fresh() -> Pairing {
        Pairing { code: code(), until: Instant::now() + CODE_LIFE, tries: 0 }
    }

    fn live(&self) -> bool {
        Instant::now() < self.until && self.tries < TRIES
    }
}

/// Random bytes, from two v4 UUIDs (the operating system's random source).
fn random(n: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n + 16);
    while bytes.len() < n {
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes.truncate(n);
    bytes
}

fn code() -> String {
    // Each byte picks a letter; 256 isn't a multiple of 31, so bytes past the last whole round
    // are skipped rather than favouring the first letters.
    let limit = 256 - 256 % ALPHABET.len();
    let mut out = String::with_capacity(CODE_LEN);
    while out.len() < CODE_LEN {
        for byte in random(16) {
            if (byte as usize) < limit && out.len() < CODE_LEN {
                out.push(ALPHABET[byte as usize % ALPHABET.len()] as char);
            }
        }
    }
    out
}

/// A device's key: 32 random bytes as hex.
pub fn new_key() -> String {
    random(32).iter().map(|b| format!("{b:02x}")).collect()
}

/// What the store keeps of a key.
pub fn key_hash(key: &str) -> String {
    Sha256::digest(key.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A code as typed: capitals, no spaces or dashes.
fn normalize(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Trades a pairing code for a new device's key, at the remote door. Wrong codes count against
/// the waiting code; the right one is used up.
pub fn redeem(app: &App, code: &str, name: &str) -> Result<(Device, String), (StatusCode, String)> {
    let mut pairing = app.pairing.lock().unwrap_or_else(|e| e.into_inner());
    let Some(waiting) = pairing.as_mut().filter(|p| p.live()) else {
        *pairing = None;
        return Err((StatusCode::UNAUTHORIZED, "no pairing is waiting; make a new code at home".into()));
    };
    // Compared in full whatever the input, so timing says nothing about how much was right.
    let given = normalize(code);
    let same = given.len() == waiting.code.len() && given.bytes().zip(waiting.code.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0;
    if !same {
        waiting.tries += 1;
        if waiting.tries >= TRIES {
            *pairing = None;
            drop(pairing);
            app.hub.send(json!({ "kind": "devices" }));
            return Err((StatusCode::UNAUTHORIZED, "too many wrong codes; make a new one at home".into()));
        }
        return Err((StatusCode::UNAUTHORIZED, "that code isn't right".into()));
    }
    *pairing = None;
    drop(pairing);
    let name = name.trim();
    let name = if name.is_empty() { "A device" } else { name };
    let name: String = name.chars().take(60).collect();
    let key = new_key();
    let device = app.try_store(|store| store.add_device(&name, &key_hash(&key))).map_err(fail)?;
    eprintln!("catbot: paired {} ({})", device.name, device.id);
    app.hub.send(json!({ "kind": "devices", "paired": device.id }));
    Ok((device, key))
}

/// Catbot's public address, for the QR code: `CATBOT_PUBLIC_URL`, else this machine's
/// Tailscale name (`tailscale status --json`, Self.DNSName), as `https://…`.
pub async fn public_url() -> Option<String> {
    if let Ok(url) = std::env::var("CATBOT_PUBLIC_URL")
        && !url.trim().is_empty()
    {
        return Some(url.trim().trim_end_matches('/').to_string());
    }
    let run = tokio::process::Command::new("tailscale").args(["status", "--json"]).kill_on_drop(true).output();
    let output = tokio::time::timeout(Duration::from_secs(3), run).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let status: Value = serde_json::from_slice(&output.stdout).ok()?;
    let name = status["Self"]["DNSName"].as_str()?.trim_end_matches('.');
    (!name.is_empty()).then(|| format!("https://{name}"))
}

fn device_wire(device: &Device) -> Value {
    json!({
        "id": device.id,
        "name": device.name,
        "createdAt": device.created_at,
        "lastSeenAt": device.last_seen_at,
    })
}

/// What the home interface shows: the devices, the waiting code, and the addresses.
async fn overview(app: &Arc<App>) -> Answer {
    let devices = app.try_store(|store| store.devices()).map_err(fail)?;
    let waiting = {
        let pairing = app.pairing.lock().unwrap_or_else(|e| e.into_inner());
        pairing.as_ref().filter(|p| p.live()).map(|p| {
            json!({ "code": p.code, "secondsLeft": p.until.saturating_duration_since(Instant::now()).as_secs(), "triesLeft": TRIES - p.tries })
        })
    };
    Ok((
        StatusCode::OK,
        json!({
            "devices": devices.iter().map(device_wire).collect::<Vec<_>>(),
            "pairing": waiting,
            "publicUrl": public_url().await,
            // The home-network address, while Same Wi-Fi is on (`reach.rs`).
            "lanUrl": super::reach::lan_url(),
            "remote": app.remote.get().map(|addr| addr.to_string()),
        }),
    ))
}

/// The home door's device routes; `rest` is what follows `/api/devices`.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str]) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => overview(app).await,
        (&Method::POST, ["pairing"]) => {
            if app.remote.get().is_none() {
                return Err((StatusCode::CONFLICT, "the remote door is off (catbot serve --remote off)".into()));
            }
            *app.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pairing::fresh());
            app.hub.send(json!({ "kind": "devices" }));
            overview(app).await
        }
        (&Method::DELETE, ["pairing"]) => {
            *app.pairing.lock().unwrap_or_else(|e| e.into_inner()) = None;
            app.hub.send(json!({ "kind": "devices" }));
            overview(app).await
        }
        (&Method::DELETE, [id]) => {
            let removed = app.try_store(|store| store.remove_device(id)).map_err(fail)?;
            if !removed {
                return Err((StatusCode::NOT_FOUND, "no such device".into()));
            }
            eprintln!("catbot: unpaired device {id}");
            // Ends that device's open streams (remote.rs watches for this) and updates the list.
            app.hub.send(json!({ "kind": "devices", "removed": id }));
            overview(app).await
        }
        _ => Err((StatusCode::NOT_FOUND, "no such route".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_keys() {
        let a = code();
        assert_eq!(a.len(), CODE_LEN);
        assert!(a.bytes().all(|b| ALPHABET.contains(&b)));
        assert_ne!(a, code());
        assert_eq!(normalize("abcd-efgh 23"), "ABCDEFGH23");
        let key = new_key();
        assert_eq!(key.len(), 64);
        assert_ne!(key, new_key());
        assert_eq!(key_hash(&key), key_hash(&key));
        assert_ne!(key_hash(&key), key);
    }
}
