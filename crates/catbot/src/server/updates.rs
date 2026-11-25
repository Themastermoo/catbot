//! Updates with rollback (stage 28.2): getting a newer Catbot from its GitHub releases,
//! installing it, and going back.
//!
//! Moo's choices (2026-11-25):
//! * Releases come from GitHub (`version.rs` checks; `scripts/release/` makes them).
//! * Installing asks first. One click, and Catbot waits until no bot is in the middle of
//!   answering, then restarts into it.
//! * Going back keeps today's chats. Only when the newer Catbot changed how chats are stored
//!   (`store::STORE_VERSION`) is the copy taken just before the update offered, and today's
//!   data is set aside beside it, not deleted.
//! * A new version that won't start is never undone by itself: the launcher asks (28.3).
//!
//! Where this applies:
//! * **Windows** keeps versions side by side in the install folder: `versions/<v>/` and
//!   `versions.json` (which one runs, which one is being tried, each one's storage format). The
//!   launcher (`Catbot.exe` at the top) starts the current one.
//! * **The phone** downloads the APK and hands it to Android's installer (28.5).
//! * **The home PC** builds itself from its repo through the Claude helper, so it only checks.
//!
//! Routes, beside `version.rs`'s:
//! * `POST /api/version/download`  fetch the offered release for this device, check it
//! * `POST /api/version/install`   install it when no bot is working; `{ waiting }` until then
//! * `POST /api/version/back`      `{ to, useSnapshot }` go back to a kept version
//!
//! Frames: `{ kind: "update", update }` as it changes, with `restart: true` when the app should
//! close so the launcher can start the version now current.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use hyper::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Answer, App, bad, fail};
use super::version::{VERSION, newer};
use crate::store::STORE_VERSION;

/// How many versions Windows keeps, the running one included.
const KEPT: usize = 3;
/// How long a new version runs before it counts as started (the launcher watches until then).
const SETTLED: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Android,
    /// The home PC: built from its repo by the helper.
    Home,
}

impl Platform {
    fn name(self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::Android => "android",
            Platform::Home => "home",
        }
    }
}

/// Where this Catbot runs, for updates.
#[derive(Clone, Debug)]
pub struct Place {
    pub platform: Platform,
    /// Windows: the install folder, holding `versions/` and `versions.json`.
    pub dir: Option<PathBuf>,
    /// GitHub's API.
    pub api: String,
    /// The phone app's version code (Android refuses an APK whose code isn't higher).
    pub code: Option<u64>,
}

impl Place {
    /// This device: worked out from how Catbot was built and started.
    pub fn here() -> Place {
        let platform = if cfg!(target_os = "android") {
            Platform::Android
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Home
        };
        // The launcher says where it installed Catbot; otherwise the program's own folder tells,
        // when it's `<dir>/versions/<v>/`.
        let dir = std::env::var_os("CATBOT_INSTALL_DIR").map(PathBuf::from).or_else(|| {
            let exe = std::env::current_exe().ok()?;
            let version_dir = exe.parent()?;
            let versions = version_dir.parent()?;
            (versions.file_name()? == "versions").then(|| versions.parent().map(Path::to_path_buf)).flatten()
        });
        Place {
            platform,
            dir: if platform == Platform::Windows { dir } else { None },
            api: "https://api.github.com".into(),
            code: option_env!("CATBOT_ANDROID_CODE").and_then(|c| c.parse().ok()),
        }
    }
}

/// What's going on with an update right now; `wire` shows it.
#[derive(Default)]
struct Now {
    /// Downloading: `(version, got, of)`.
    downloading: Option<(String, u64, u64)>,
    /// Waiting for the bots to finish before installing.
    waiting: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    place: OnceLock<Place>,
    now: Mutex<Now>,
}

impl Updates {
    pub fn place(&self) -> &Place {
        self.place.get_or_init(Place::here)
    }

    /// Sets where this Catbot runs (tests use a stand-in GitHub and install folder).
    pub fn set_place(&self, place: Place) {
        let _ = self.place.set(place);
    }

    fn now(&self) -> std::sync::MutexGuard<'_, Now> {
        self.now.lock().expect("updates lock")
    }
}

// ---- Files ----

fn folder(app: &App) -> PathBuf {
    app.with_store(|store| Ok(store.home().join("updates"))).unwrap_or_else(|| PathBuf::from("updates"))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = path.with_extension("json.part");
    std::fs::write(&part, serde_json::to_vec_pretty(value).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}

/// Windows' `versions.json`, or what it would say for a Catbot installed before stage 28.
fn versions(dir: &Path) -> Value {
    let mut kept = read_json(&dir.join("versions.json")).unwrap_or_else(|| json!({}));
    if kept["current"].is_null() {
        kept["current"] = json!(VERSION);
    }
    if !kept["kept"].is_object() {
        kept["kept"] = json!({});
    }
    let current = kept["current"].as_str().unwrap_or(VERSION).to_string();
    if current == VERSION && kept["kept"][VERSION].is_null() {
        kept["kept"][VERSION] = json!({ "store": STORE_VERSION });
    }
    kept
}

fn save_versions(dir: &Path, kept: &Value) -> Result<(), String> {
    write_json(&dir.join("versions.json"), kept)
}

/// The settings kept under "updates" (shared with `version.rs`).
fn saved(app: &App) -> Value {
    app.with_store(|store| store.setting("updates")).flatten().unwrap_or_else(|| json!({}))
}

fn save(app: &App, change: impl FnOnce(&mut Value)) -> Result<(), String> {
    let mut kept = saved(app);
    change(&mut kept);
    app.try_store(|store| store.set_setting("updates", &kept)).map_err(|e| e.to_string())
}

/// Tells every window what's going on.
fn tell(app: &App, extra: Value) {
    let mut frame = json!({ "kind": "update", "update": wire(app) });
    if let (Some(frame), Some(extra)) = (frame.as_object_mut(), extra.as_object()) {
        frame.extend(extra.clone());
    }
    app.hub.send(frame);
}

// ---- What a release offers this device ----

/// GitHub's latest release, read for this device: whether it can be installed here, and why not.
/// `release` is GitHub's answer; `found` is its `release.json`, when there is one.
pub fn offer(app: &App, release: &Value, found: Option<&Value>) -> Value {
    let place = app.updates.place();
    let tag = release["tag_name"].as_str().unwrap_or_default();
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let mut offer = json!({
        "version": version,
        "notes": release["body"].as_str().filter(|b| !b.trim().is_empty()),
        "url": release["html_url"],
    });
    let why_not = |why: &str| Some(why.to_string());
    let why: Option<String> = match (place.platform, found) {
        (Platform::Home, _) => why_not("This computer builds Catbot from its repo; the Claude helper updates it."),
        (_, None) => why_not("This release has no release.json, so Catbot can't check its files."),
        (platform, Some(found)) => {
            offer["store"] = found["store"].clone();
            let entry = &found[platform.name()];
            if entry.is_null() {
                why_not("This release has nothing for this device.")
            } else if platform == Platform::Windows && place.dir.is_none() {
                why_not("This Catbot wasn't installed by the launcher, so it can't swap versions. Install it once from the zip.")
            } else if platform == Platform::Android && entry["code"].as_u64().zip(place.code).is_some_and(|(theirs, ours)| theirs <= ours) {
                why_not("This phone went back from this version; the next release will update it.")
            } else {
                offer["bytes"] = entry["bytes"].clone();
                None
            }
        }
    };
    offer["canInstall"] = json!(why.is_none());
    offer["whyNot"] = json!(why);
    offer
}

/// A release's asset by name: its download address.
fn asset<'a>(release: &'a Value, name: &str) -> Option<&'a str> {
    release["assets"].as_array()?.iter().find(|a| a["name"] == name)?["browser_download_url"].as_str()
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .user_agent(format!("Catbot/{VERSION}"))
        .build()
        .map_err(|e| e.to_string())
}

/// A release's `release.json`, read from its assets.
pub async fn release_json(release: &Value) -> Result<Option<Value>, String> {
    let Some(url) = asset(release, "release.json") else { return Ok(None) };
    let answer = client()?.get(url).send().await.map_err(|e| format!("couldn't read release.json: {e}"))?;
    if !answer.status().is_success() {
        return Err(format!("release.json answered {}", answer.status()));
    }
    answer.json().await.map(Some).map_err(|e| format!("release.json isn't readable: {e}"))
}

/// Downloads `url` into `into`, no bigger than `bytes`, and checks its SHA-256.
async fn fetch(url: &str, into: &Path, bytes: u64, sha: &str, progress: impl Fn(u64)) -> Result<(), String> {
    let answer = client()?.get(url).send().await.map_err(|e| format!("couldn't download it: {e}"))?;
    if !answer.status().is_success() {
        return Err(format!("the download answered {}", answer.status()));
    }
    if let Some(dir) = into.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut out = std::fs::File::create(into).map_err(|e| format!("couldn't save it: {e}"))?;
    let mut hash = Sha256::new();
    let (mut got, mut told) = (0u64, 0u64);
    let mut stream = answer.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("the download broke off: {e}"))?;
        got += chunk.len() as u64;
        if got > bytes {
            drop(out);
            let _ = std::fs::remove_file(into);
            return Err("the download is bigger than the release says, so it wasn't used".into());
        }
        hash.update(&chunk);
        std::io::Write::write_all(&mut out, &chunk).map_err(|e| format!("couldn't save it: {e}"))?;
        if got - told > 2 * 1024 * 1024 {
            told = got;
            progress(got);
        }
    }
    progress(got);
    if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(sha) {
        let _ = std::fs::remove_file(into);
        return Err("the download isn't the file the release names (its checksum doesn't match), so it wasn't used".into());
    }
    Ok(())
}

// ---- Downloading ----

async fn download(app: &std::sync::Arc<App>) -> Answer {
    let place = app.updates.place().clone();
    if place.platform == Platform::Home {
        return Err(bad("this computer builds Catbot from its repo; the Claude helper updates it"));
    }
    if app.updates.now().downloading.is_some() {
        return Ok((StatusCode::ACCEPTED, wire(app)));
    }
    let repo = super::version::repo(app).ok_or_else(|| bad("name Catbot's GitHub repository first"))?;
    let release = super::version::latest_release(app, &repo).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let found = release_json(&release).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let offer = offer(app, &release, found.as_ref());
    if offer["canInstall"] != true {
        return Err(bad(offer["whyNot"].as_str().unwrap_or("there's nothing to install")));
    }
    let version = offer["version"].as_str().unwrap_or_default().to_string();
    if !newer(&version, VERSION) {
        return Err(bad(format!("this is already Catbot {VERSION}, and the latest release is {version}")));
    }
    let found = found.unwrap_or_default();
    let entry = found[place.platform.name()].clone();
    let name = entry["file"].as_str().unwrap_or_default().to_string();
    let url = asset(&release, &name).ok_or_else(|| (StatusCode::BAD_GATEWAY, format!("the release names {name} but doesn't have it")))?.to_string();
    let (bytes, sha) = (entry["bytes"].as_u64().unwrap_or(0), entry["sha256"].as_str().unwrap_or_default().to_string());
    {
        let mut now = app.updates.now();
        now.downloading = Some((version.clone(), 0, bytes));
        now.error = None;
    }
    tell(app, json!({}));
    let answer = wire(app);
    let app = app.clone();
    let store = found["store"].as_i64().unwrap_or(STORE_VERSION);
    tokio::spawn(async move {
        let done = fetch_and_keep(&app, &place, &version, store, &url, &name, bytes, &sha).await;
        {
            let mut now = app.updates.now();
            now.downloading = None;
            now.error = done.as_ref().err().cloned();
        }
        if let Err(err) = &done {
            eprintln!("catbot: the update to {version} didn't download: {err}");
        }
        tell(&app, json!({}));
    });
    Ok((StatusCode::ACCEPTED, answer))
}

#[allow(clippy::too_many_arguments)]
async fn fetch_and_keep(app: &App, place: &Place, version: &str, store: i64, url: &str, name: &str, bytes: u64, sha: &str) -> Result<(), String> {
    let file = folder(app).join("downloads").join(name);
    let progress = |got| {
        if let Some((_, have, _)) = app.updates.now().downloading.as_mut() {
            *have = got;
        }
        tell(app, json!({}));
    };
    fetch(url, &file, bytes, sha, progress).await?;
    let ready = match place.platform {
        Platform::Windows => {
            let dir = place.dir.clone().ok_or("no install folder")?;
            let into = dir.join("versions").join(version);
            let unpacked = dir.join("versions").join(format!("{version}.part"));
            let (from, to) = (file.clone(), unpacked.clone());
            let _ = std::fs::remove_dir_all(&unpacked);
            tokio::task::spawn_blocking(move || crate::agents::unzip(&from, &to)).await.map_err(|e| e.to_string())??;
            let _ = std::fs::remove_file(&file);
            // The zip holds catbot-windows/app/…: that folder is the version.
            let app_dir = unpacked.join("catbot-windows").join("app");
            if !app_dir.join("Catbot.exe").is_file() {
                let _ = std::fs::remove_dir_all(&unpacked);
                return Err("the download has no Catbot.exe in it".into());
            }
            let _ = std::fs::remove_dir_all(&into);
            std::fs::rename(&app_dir, &into).map_err(|e| format!("couldn't put it in place: {e}"))?;
            let _ = std::fs::remove_dir_all(&unpacked);
            let mut kept = versions(&dir);
            kept["kept"][version] = json!({ "store": store, "at": chrono::Utc::now().to_rfc3339() });
            save_versions(&dir, &kept)?;
            json!({ "version": version, "store": store })
        }
        Platform::Android => json!({ "version": version, "store": store, "apk": file.to_string_lossy() }),
        Platform::Home => return Err("nothing to install here".into()),
    };
    save(app, |kept| kept["ready"] = ready)
}

// ---- Installing ----

async fn install(app: &std::sync::Arc<App>) -> Answer {
    let ready = saved(app)["ready"].clone();
    if ready.is_null() {
        return Err(bad("there's no downloaded update to install"));
    }
    if app.turns.any_busy() {
        let first = !std::mem::replace(&mut app.updates.now().waiting, true);
        if first {
            let app = app.clone();
            tokio::spawn(async move {
                while app.turns.any_busy() && app.updates.now().waiting {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                if std::mem::take(&mut app.updates.now().waiting) {
                    finish_install(&app);
                }
            });
        }
        tell(app, json!({}));
        return Ok((StatusCode::ACCEPTED, wire(app)));
    }
    finish_install(app);
    Ok((StatusCode::OK, wire(app)))
}

/// Takes the copy of the data, makes the new version current, and asks the app to restart.
fn finish_install(app: &App) {
    match switch_to_new(app) {
        Ok(extra) => tell(app, extra),
        Err(err) => {
            app.updates.now().error = Some(err);
            tell(app, json!({}));
        }
    }
}

fn switch_to_new(app: &App) -> Result<Value, String> {
    let ready = saved(app)["ready"].clone();
    let version = ready["version"].as_str().ok_or("there's no downloaded update")?.to_string();
    let snapshot = take_snapshot(app, &version)?;
    save(app, |kept| kept["installing"] = json!({ "from": VERSION, "to": version, "at": chrono::Utc::now().to_rfc3339(), "snapshot": snapshot }))?;
    let place = app.updates.place();
    match place.platform {
        Platform::Windows => {
            let dir = place.dir.clone().ok_or("no install folder")?;
            let mut kept = versions(&dir);
            kept["previous"] = kept["current"].clone();
            kept["current"] = json!(version);
            kept["trying"] = json!({ "version": version, "at": chrono::Utc::now().to_rfc3339() });
            kept["failed"] = Value::Null;
            save_versions(&dir, &kept)?;
            save(app, |kept| kept["ready"] = Value::Null)?;
            Ok(json!({ "restart": true }))
        }
        // The phone's interface opens Android's installer with it; the app restarts as the new one.
        Platform::Android => Ok(json!({ "openInstaller": ready["apk"] })),
        Platform::Home => Err("nothing to install here".into()),
    }
}

/// A copy of the chats as they are, before changing version: `updates/snapshots/`.
fn take_snapshot(app: &App, to: &str) -> Result<Value, String> {
    let dir = folder(app).join("snapshots");
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let file = dir.join(format!("{stamp}-from-{VERSION}.db"));
    let format = app.try_store(|store| {
        store.copy_to(&file)?;
        store.format()
    }).map_err(|e| format!("couldn't copy the chats first: {e}"))?;
    let meta = json!({ "file": file.to_string_lossy(), "from": VERSION, "to": to, "store": format, "at": chrono::Utc::now().to_rfc3339() });
    write_json(&file.with_extension("json"), &meta)?;
    // Keep the last few.
    let mut all = snapshots(app);
    while all.len() > KEPT {
        let old = all.remove(0);
        if let Some(file) = old["file"].as_str() {
            let _ = std::fs::remove_file(file);
            let _ = std::fs::remove_file(Path::new(file).with_extension("json"));
        }
    }
    Ok(meta)
}

/// The copies taken before updates, oldest first.
fn snapshots(app: &App) -> Vec<Value> {
    let dir = folder(app).join("snapshots");
    let mut found: Vec<Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_json(&e.path()))
        .filter(|m| m["file"].as_str().is_some_and(|f| Path::new(f).is_file()))
        .collect();
    found.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
    found
}

// ---- Going back ----

async fn back(app: &App, body: &Value) -> Answer {
    let place = app.updates.place().clone();
    let to = body["to"].as_str().unwrap_or_default().trim().to_string();
    let use_snapshot = body["useSnapshot"] == true;
    match place.platform {
        Platform::Home => return Err(bad("this computer builds Catbot from its repo; go back with git and the Claude helper")),
        Platform::Android => return Err(bad("going back on the phone comes in a later stage")),
        Platform::Windows => {}
    }
    let dir = place.dir.clone().ok_or_else(|| bad("this Catbot wasn't installed by the launcher"))?;
    let mut kept = versions(&dir);
    if to.is_empty() || to == VERSION {
        return Err(bad("name a kept version other than this one"));
    }
    if kept["kept"][&to].is_null() || !dir.join("versions").join(&to).join("Catbot.exe").is_file() {
        return Err(bad(format!("Catbot {to} isn't kept on this PC")));
    }
    let theirs = kept["kept"][&to]["store"].as_i64().unwrap_or(0);
    let ours = app.try_store(|store| store.format()).map_err(fail)?;
    let mut restore = Value::Null;
    if theirs < ours {
        // The older Catbot can't read today's chats: offer the copy from before the update.
        let snapshot = snapshots(app).into_iter().rev().find(|s| s["store"].as_i64().is_some_and(|f| f <= theirs));
        let Some(snapshot) = snapshot else {
            return Err((StatusCode::CONFLICT, format!("Catbot {to} can't read how chats are stored now, and there's no copy from before the update")));
        };
        if !use_snapshot {
            return Ok((StatusCode::CONFLICT, json!({ "needsSnapshot": true, "snapshot": snapshot, "to": to })));
        }
        restore = json!({ "file": snapshot["file"], "at": snapshot["at"], "asked": chrono::Utc::now().to_rfc3339(), "from": VERSION });
        write_json(&folder(app).join("restore.json"), &restore).map_err(fail)?;
    }
    kept["previous"] = json!(VERSION);
    kept["current"] = json!(to);
    kept["trying"] = Value::Null;
    kept["failed"] = Value::Null;
    save_versions(&dir, &kept).map_err(fail)?;
    save(app, |kept| kept["back"] = json!({ "from": VERSION, "to": to, "at": chrono::Utc::now().to_rfc3339(), "restored": !restore.is_null() })).map_err(fail)?;
    tell(app, json!({ "restart": true }));
    Ok((StatusCode::OK, json!({ "restart": true, "to": to, "restored": !restore.is_null() })))
}

/// Before the chats are opened: if going back asked for the copy from before an update, today's
/// data is set aside (not deleted) and the copy put in its place. Returns where it was set aside.
pub fn restore_if_asked(home: &Path) -> Option<PathBuf> {
    let asked = home.join("updates").join("restore.json");
    let restore = read_json(&asked)?;
    let _ = std::fs::remove_file(&asked);
    let copy = PathBuf::from(restore["file"].as_str()?);
    if !copy.is_file() {
        eprintln!("catbot: the copy to go back to ({}) is gone, so today's chats stay", copy.display());
        return None;
    }
    let aside = home.join("updates").join("set-aside").join(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    if let Err(err) = std::fs::create_dir_all(&aside) {
        eprintln!("catbot: couldn't make {} ({err}), so today's chats stay", aside.display());
        return None;
    }
    for name in ["catbot.db", "catbot.db-wal", "catbot.db-shm"] {
        let from = home.join(name);
        if from.exists()
            && let Err(err) = std::fs::rename(&from, aside.join(name))
        {
            eprintln!("catbot: couldn't set {name} aside ({err}), so today's chats stay");
            return None;
        }
    }
    if let Err(err) = std::fs::copy(&copy, home.join("catbot.db")) {
        eprintln!("catbot: couldn't put the copy back ({err}); today's chats are in {}", aside.display());
        for name in ["catbot.db", "catbot.db-wal", "catbot.db-shm"] {
            let _ = std::fs::rename(aside.join(name), home.join(name));
        }
        return None;
    }
    let _ = write_json(&aside.join("about.json"), &json!({ "setAside": chrono::Utc::now().to_rfc3339(), "restored": restore }));
    eprintln!("catbot: went back to the chats as they were at {}; today's are set aside in {}", restore["at"].as_str().unwrap_or("?"), aside.display());
    Some(aside)
}

/// Once the server is up: a new version that keeps running counts as started, and old versions
/// beyond the last few are cleared away.
pub fn started(app: std::sync::Arc<App>) {
    tokio::spawn(async move {
        tokio::time::sleep(SETTLED).await;
        settle(&app);
    });
}

pub fn settle(app: &App) {
    let installing = saved(app)["installing"].clone();
    if installing["to"] == VERSION {
        let _ = save(app, |kept| {
            kept["updated"] = json!({ "from": installing["from"], "to": VERSION, "at": chrono::Utc::now().to_rfc3339() });
            kept["installing"] = Value::Null;
            kept["checked"] = Value::Null;
        });
    }
    let Some(dir) = app.updates.place().dir.clone() else { return };
    let mut kept = versions(&dir);
    if kept["trying"]["version"] == VERSION {
        kept["trying"] = Value::Null;
    }
    // Keep the current version and the newest others, up to KEPT.
    let current = kept["current"].as_str().unwrap_or(VERSION).to_string();
    let mut others: Vec<String> = kept["kept"].as_object().map(|k| k.keys().filter(|v| **v != current).cloned().collect()).unwrap_or_default();
    others.sort_by(|a, b| if newer(a, b) { std::cmp::Ordering::Greater } else if newer(b, a) { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal });
    while others.len() >= KEPT {
        let old = others.remove(0);
        let _ = std::fs::remove_dir_all(dir.join("versions").join(&old));
        if let Some(map) = kept["kept"].as_object_mut() {
            map.remove(&old);
        }
    }
    let _ = save_versions(&dir, &kept);
}

// ---- What the interface sees ----

/// The update's state, for `GET /api/version` and the frames.
pub fn wire(app: &App) -> Value {
    let place = app.updates.place();
    let settings = saved(app);
    let now = app.updates.now();
    let kept = place.dir.as_deref().map(versions);
    let list: Vec<Value> = kept
        .as_ref()
        .and_then(|k| k["kept"].as_object().cloned())
        .map(|map| {
            let mut list: Vec<Value> = map
                .into_iter()
                .filter(|(v, _)| v == VERSION || place.dir.as_deref().is_some_and(|d| d.join("versions").join(v).join("Catbot.exe").is_file()))
                .map(|(v, about)| json!({ "version": v, "store": about["store"], "current": v == VERSION }))
                .collect();
            list.sort_by(|a, b| if newer(a["version"].as_str().unwrap_or(""), b["version"].as_str().unwrap_or("")) { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater });
            list
        })
        .unwrap_or_default();
    json!({
        "platform": place.platform.name(),
        "canSwap": place.platform != Platform::Home && (place.platform != Platform::Windows || place.dir.is_some()),
        "downloading": now.downloading.as_ref().map(|(v, got, of)| json!({ "version": v, "got": got, "of": of })),
        "waiting": now.waiting,
        "error": now.error,
        "ready": settings["ready"],
        "kept": list,
        "failed": kept.as_ref().map(|k| k["failed"].clone()).unwrap_or(Value::Null),
        "updated": settings["updated"],
        "back": settings["back"],
    })
}

pub async fn route(app: &std::sync::Arc<App>, rest: &[&str], body: Value) -> Option<Answer> {
    Some(match rest {
        ["download"] => download(app).await,
        ["install"] => install(app).await,
        ["back"] => back(app, &body).await,
        ["cancel"] => {
            app.updates.now().waiting = false;
            tell(app, json!({}));
            Ok((StatusCode::OK, wire(app)))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn going_back_sets_today_aside_and_puts_the_copy_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("catbot.db"), "today").unwrap();
        std::fs::write(home.join("catbot.db-wal"), "today's log").unwrap();
        let copy = home.join("updates/snapshots/before.db");
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::write(&copy, "before").unwrap();
        assert!(restore_if_asked(home).is_none(), "nothing asked");
        write_json(&home.join("updates/restore.json"), &json!({ "file": copy, "at": "earlier" })).unwrap();
        let aside = restore_if_asked(home).expect("restored");
        assert_eq!(std::fs::read_to_string(home.join("catbot.db")).unwrap(), "before");
        assert!(!home.join("catbot.db-wal").exists(), "today's log goes with today's chats");
        assert_eq!(std::fs::read_to_string(aside.join("catbot.db")).unwrap(), "today");
        assert_eq!(std::fs::read_to_string(aside.join("catbot.db-wal")).unwrap(), "today's log");
        assert!(!home.join("updates/restore.json").exists(), "done once");
        assert!(copy.is_file(), "the copy is kept too");
    }

    #[test]
    fn a_catbot_from_before_stage_28_reads_as_this_version() {
        let dir = tempfile::tempdir().unwrap();
        let kept = versions(dir.path());
        assert_eq!(kept["current"], VERSION);
        assert_eq!(kept["kept"][VERSION]["store"], STORE_VERSION);
    }
}
