//! Voice (stage 22): talking to bots, and bots speaking their replies.
//!
//! Moo's choices (2026-11-24): two switches, "Talk to bots" and "Turn on voice reply", each run on
//! this computer or by an online service. On this computer: Whisper listens, Kokoro or Piper
//! speaks, run by sherpa-onnx's two programs. Online: OpenAI for both, ElevenLabs as another reply
//! voice. A mic setting picks between checking the words first and sending them at once; replies
//! are read aloud; each bot can have its own voice.
//!
//! Catbot's server does the listening and the speaking, so the desktop app, the browser and the
//! phone all use the same settings, and no engine runs inside a page.
//!
//! - `GET  /api/voice`: the settings, which engines are ready, the voices, keys as last four.
//! - `PUT  /api/voice`: change the settings (any part of them).
//! - `PUT  /api/voice/keys/:service {key, url?}`, `DELETE …`: OpenAI's or ElevenLabs' key (at home).
//! - `POST /api/voice/get {part}`: download what this computer needs ("program", "listen",
//!   "kokoro", "piper"), in the background, with `voice` frames saying how far it's got.
//! - `POST /api/voice/listen` (a WAV, or another recording an online service takes): `{text}`.
//! - `POST /api/voice/speak {text, botId?}`: the words as audio (WAV or MP3).
//!
//! On this computer the programs come from sherpa-onnx's own packages on PyPI (pinned, checked
//! against their SHA-256) and the models from Hugging Face (and Piper's word list from
//! sherpa-onnx's GitHub releases). Nothing is downloaded until the person asks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;

use super::{Answer, App, bad, fail, json_body, respond};

const KEY: &str = "voice";
/// How long a recording may be, in bytes (about ten minutes of 16 kHz WAV).
const AUDIO_MAX: usize = 24 * 1024 * 1024;
/// How many characters of a reply are spoken.
const SPEAK_MAX: usize = 4000;

// ---- Settings ----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub listen: Listen,
    pub reply: Reply,
    /// Each bot's own voice, per engine ("kokoro", "piper", "openai", "elevenlabs").
    pub bots: HashMap<String, HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Listen {
    /// "Talk to bots": the microphone button.
    pub on: bool,
    /// "local" (this computer) or "online".
    #[serde(rename = "where")]
    pub place: String,
    /// The Whisper model on this computer: "base.en", "small.en" or "turbo".
    pub model: String,
    /// OpenAI's model for listening.
    pub online_model: String,
    /// Send the words as soon as they're written out, instead of putting them in the box.
    pub send_at_once: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reply {
    /// "Turn on voice reply".
    pub on: bool,
    #[serde(rename = "where")]
    pub place: String,
    /// On this computer: "kokoro" or "piper".
    pub local: String,
    /// Online: "openai" or "elevenlabs".
    pub online: String,
    /// The default voice for each engine.
    pub voices: HashMap<String, String>,
    pub openai_model: String,
    pub elevenlabs_model: String,
}

impl Default for Listen {
    fn default() -> Self {
        Listen { on: false, place: "local".into(), model: "base.en".into(), online_model: "gpt-4o-mini-transcribe".into(), send_at_once: false }
    }
}

impl Default for Reply {
    fn default() -> Self {
        Reply {
            on: false,
            place: "local".into(),
            local: "kokoro".into(),
            online: "openai".into(),
            voices: HashMap::new(),
            openai_model: "gpt-4o-mini-tts".into(),
            elevenlabs_model: "eleven_flash_v2_5".into(),
        }
    }
}

pub fn settings(app: &App) -> Settings {
    app.with_store(|store| store.setting(KEY)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save(app: &App, settings: &Settings) -> Result<(), (StatusCode, String)> {
    app.try_store(|store| store.set_setting(KEY, &json!(settings))).map_err(fail)
}

/// Merges a change (any part of the settings) into them, checking the choices.
fn changed(mut now: Settings, change: &Value) -> Result<Settings, String> {
    let mut merged = json!(now);
    merge(&mut merged, change);
    now = serde_json::from_value(merged).map_err(|e| format!("that isn't a voice setting: {e}"))?;
    let ok = |v: &str, allowed: &[&str], what: &str| if allowed.contains(&v) { Ok(()) } else { Err(format!("{what} can't be \"{v}\"")) };
    ok(&now.listen.place, &["local", "online"], "where listening runs")?;
    ok(&now.reply.place, &["local", "online"], "where speaking runs")?;
    ok(&now.listen.model, &LISTEN_MODELS.iter().map(|m| m.id).collect::<Vec<_>>(), "the listening model")?;
    ok(&now.reply.local, &["kokoro", "piper"], "the voice on this computer")?;
    ok(&now.reply.online, &["openai", "elevenlabs"], "the online voice")?;
    for model in [&now.listen.online_model, &now.reply.openai_model, &now.reply.elevenlabs_model] {
        if model.trim().is_empty() || model.len() > 100 || model.contains(char::is_whitespace) {
            return Err("a model name is one word".into());
        }
    }
    // Empty voices mean "the default"; they're dropped.
    now.reply.voices.retain(|_, v| !v.trim().is_empty());
    for voices in now.bots.values_mut() {
        voices.retain(|_, v| !v.trim().is_empty());
    }
    now.bots.retain(|_, v| !v.is_empty());
    Ok(now)
}

fn merge(into: &mut Value, change: &Value) {
    match (into, change) {
        (Value::Object(into), Value::Object(change)) => {
            for (key, value) in change {
                // The bots' and default voices are replaced per key, so one can be taken off.
                match into.get_mut(key) {
                    Some(slot) if value.is_object() && slot.is_object() => merge(slot, value),
                    _ => {
                        into.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (into, change) => *into = change.clone(),
    }
}

// ---- What this computer has ------------------------------------------------------------------

/// A file to fetch: where from, its name here, and (for the programs) its SHA-256.
struct Remote {
    url: &'static str,
    name: &'static str,
    sha256: Option<&'static str>,
}

/// A Whisper model this computer can listen with.
pub struct ListenModel {
    pub id: &'static str,
    pub label: &'static str,
    pub size: &'static str,
    files: [Remote; 3],
}

macro_rules! hf {
    ($repo:literal, $file:literal) => {
        concat!("https://huggingface.co/csukuangfj/", $repo, "/resolve/main/", $file)
    };
}

pub const LISTEN_MODELS: [ListenModel; 3] = [
    ListenModel {
        id: "base.en",
        label: "Quick (Whisper base)",
        size: "160 MB",
        files: [
            Remote { url: hf!("sherpa-onnx-whisper-base.en", "base.en-encoder.int8.onnx"), name: "encoder.onnx", sha256: None },
            Remote { url: hf!("sherpa-onnx-whisper-base.en", "base.en-decoder.int8.onnx"), name: "decoder.onnx", sha256: None },
            Remote { url: hf!("sherpa-onnx-whisper-base.en", "base.en-tokens.txt"), name: "tokens.txt", sha256: None },
        ],
    },
    ListenModel {
        id: "small.en",
        label: "Better (Whisper small)",
        size: "380 MB",
        files: [
            Remote { url: hf!("sherpa-onnx-whisper-small.en", "small.en-encoder.int8.onnx"), name: "encoder.onnx", sha256: None },
            Remote { url: hf!("sherpa-onnx-whisper-small.en", "small.en-decoder.int8.onnx"), name: "decoder.onnx", sha256: None },
            Remote { url: hf!("sherpa-onnx-whisper-small.en", "small.en-tokens.txt"), name: "tokens.txt", sha256: None },
        ],
    },
    ListenModel {
        id: "turbo",
        label: "Best (Whisper large turbo, slower without a graphics card)",
        size: "1 GB",
        files: [
            Remote { url: hf!("sherpa-onnx-whisper-turbo", "turbo-encoder.int8.onnx"), name: "encoder.onnx", sha256: None },
            Remote { url: hf!("sherpa-onnx-whisper-turbo", "turbo-decoder.int8.onnx"), name: "decoder.onnx", sha256: None },
            Remote { url: hf!("sherpa-onnx-whisper-turbo", "turbo-tokens.txt"), name: "tokens.txt", sha256: None },
        ],
    },
];

const KOKORO: [Remote; 4] = [
    Remote { url: hf!("kokoro-multi-lang-v1_0", "model.onnx"), name: "model.onnx", sha256: None },
    Remote { url: hf!("kokoro-multi-lang-v1_0", "voices.bin"), name: "voices.bin", sha256: None },
    Remote { url: hf!("kokoro-multi-lang-v1_0", "tokens.txt"), name: "tokens.txt", sha256: None },
    Remote { url: hf!("kokoro-multi-lang-v1_0", "lexicon-us-en.txt"), name: "lexicon-us-en.txt", sha256: None },
];

const PIPER: [Remote; 3] = [
    Remote { url: hf!("vits-piper-en_US-amy-low", "en_US-amy-low.onnx"), name: "model.onnx", sha256: None },
    Remote { url: hf!("vits-piper-en_US-amy-low", "tokens.txt"), name: "tokens.txt", sha256: None },
    Remote { url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/lexicon-en-us.txt", name: "lexicon-en-us.txt", sha256: None },
];

/// sherpa-onnx 2.0.3's programs (`sherpa-onnx-bin`) and its ONNX runtime (`sherpa-onnx-core`).
#[cfg(windows)]
const PROGRAM: [Remote; 2] = [
    Remote {
        url: "https://files.pythonhosted.org/packages/6f/2e/9ccd920ab006f9ae7fb6322eda5199033b934b3d6ae0582142af2625bd8d/sherpa_onnx_bin-2.0.3-py3-none-win_amd64.whl",
        name: "bin.whl",
        sha256: Some("c4b04f53c8a75a993ae9c4fd9edca846f9cdd81bcb932299e937d803e344dd38"),
    },
    Remote {
        url: "https://files.pythonhosted.org/packages/86/60/267823e104a5856b38601784ae6ca388cd00f219b49c8e78de380ceb63b0/sherpa_onnx_core-2.0.3-py3-none-win_amd64.whl",
        name: "core.whl",
        sha256: Some("798ba3803b6461bac9eca5a7f3927f04e7a54173155e3f0479e6abd89bb4bc59"),
    },
];
#[cfg(not(windows))]
const PROGRAM: [Remote; 2] = [
    Remote {
        url: "https://files.pythonhosted.org/packages/c1/5b/c9b1cb6bdeb9e57f85082d26f0347a8859d4aa7502e5fb1142b6f8e20059/sherpa_onnx_bin-2.0.3-py3-none-manylinux2014_x86_64.whl",
        name: "bin.whl",
        sha256: Some("bdcf5affe58057adf8bff7a77dccfaa82cc89272156602a8ca87f16957575eca"),
    },
    Remote {
        url: "https://files.pythonhosted.org/packages/44/a8/339ae6e5edd700c0e3a5a5dbfa64136d186bf1c1c795295fab0df2b57bf7/sherpa_onnx_core-2.0.3-py3-none-manylinux2014_x86_64.whl",
        name: "core.whl",
        sha256: Some("3c4e54ed9ac03ef8cc9b7340dc18a1203017bdd0e4ecc9923562560006d881e0"),
    },
];

/// The files taken out of the two packages: (the end of the path in the package, its name here).
#[cfg(windows)]
const UNPACKED: [(&str, &str); 3] = [
    ("/Scripts/sherpa-onnx-offline.exe", "sherpa-onnx-offline.exe"),
    ("/Scripts/sherpa-onnx-offline-tts.exe", "sherpa-onnx-offline-tts.exe"),
    ("sherpa_onnx/lib/onnxruntime.dll", "onnxruntime.dll"),
];
#[cfg(not(windows))]
const UNPACKED: [(&str, &str); 3] = [
    ("/bin/sherpa-onnx-offline", "sherpa-onnx-offline"),
    ("/bin/sherpa-onnx-offline-tts", "sherpa-onnx-offline-tts"),
    ("sherpa_onnx/lib/libonnxruntime.so", "libonnxruntime.so"),
];

/// Kokoro's English voices (its speaker numbers in the v1.0 voice table).
pub const KOKORO_VOICES: [(&str, &str); 28] = [
    ("0", "Alloy (US, woman)"),
    ("1", "Aoede (US, woman)"),
    ("2", "Bella (US, woman)"),
    ("3", "Heart (US, woman)"),
    ("4", "Jessica (US, woman)"),
    ("5", "Kore (US, woman)"),
    ("6", "Nicole (US, woman)"),
    ("7", "Nova (US, woman)"),
    ("8", "River (US, woman)"),
    ("9", "Sarah (US, woman)"),
    ("10", "Sky (US, woman)"),
    ("11", "Adam (US, man)"),
    ("12", "Echo (US, man)"),
    ("13", "Eric (US, man)"),
    ("14", "Fenrir (US, man)"),
    ("15", "Liam (US, man)"),
    ("16", "Michael (US, man)"),
    ("17", "Onyx (US, man)"),
    ("18", "Puck (US, man)"),
    ("19", "Santa (US, man)"),
    ("20", "Alice (UK, woman)"),
    ("21", "Emma (UK, woman)"),
    ("22", "Isabella (UK, woman)"),
    ("23", "Lily (UK, woman)"),
    ("24", "Daniel (UK, man)"),
    ("25", "Fable (UK, man)"),
    ("26", "George (UK, man)"),
    ("27", "Lewis (UK, man)"),
];
const KOKORO_DEFAULT: &str = "3";

pub const OPENAI_VOICES: [&str; 11] = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"];
const OPENAI_DEFAULT: &str = "coral";

fn dir(app: &App) -> PathBuf {
    app.with_store(|store| Ok(store.home().join("voice"))).unwrap_or_else(|| PathBuf::from("voice"))
}

fn program(app: &App, name: &str) -> PathBuf {
    let name = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    dir(app).join("sherpa").join(name)
}

fn part_dir(app: &App, part: &str) -> PathBuf {
    dir(app).join("models").join(part)
}

fn have(app: &App, part: &str) -> bool {
    match part {
        "program" => UNPACKED.iter().all(|(_, name)| dir(app).join("sherpa").join(name).is_file()),
        "kokoro" => KOKORO.iter().all(|f| part_dir(app, "kokoro").join(f.name).is_file()),
        "piper" => PIPER.iter().all(|f| part_dir(app, "piper").join(f.name).is_file()),
        model => LISTEN_MODELS
            .iter()
            .find(|m| m.id == model)
            .is_some_and(|m| m.files.iter().all(|f| part_dir(app, &format!("whisper-{}", m.id)).join(f.name).is_file())),
    }
}

// ---- Downloading -------------------------------------------------------------------------------

/// How far a download has got: bytes done, bytes in all, the problem if it stopped.
type Progress = (u64, u64, Option<String>);

/// What's being fetched, by part.
static GETTING: Mutex<Option<HashMap<String, Progress>>> = Mutex::new(None);

fn getting() -> HashMap<String, Progress> {
    GETTING.lock().expect("voice lock").clone().unwrap_or_default()
}

fn set_getting(part: &str, value: Option<Progress>) {
    let mut all = GETTING.lock().expect("voice lock");
    let map = all.get_or_insert_with(HashMap::new);
    match value {
        Some(v) => {
            map.insert(part.to_string(), v);
        }
        None => {
            map.remove(part);
        }
    }
}

/// Where a file really comes from: `CATBOT_VOICE_MIRROR` stands in for the internet (tests).
fn source(url: &str) -> String {
    match std::env::var("CATBOT_VOICE_MIRROR").ok().filter(|m| !m.trim().is_empty()) {
        Some(mirror) => format!("{}/{}", mirror.trim_end_matches('/'), url.trim_start_matches("https://")),
        None => url.to_string(),
    }
}

fn files_for(part: &str) -> Option<(Vec<&'static Remote>, String)> {
    match part {
        "program" => Some((PROGRAM.iter().collect(), "sherpa".into())),
        "kokoro" => Some((KOKORO.iter().collect(), "models/kokoro".into())),
        "piper" => Some((PIPER.iter().collect(), "models/piper".into())),
        model => LISTEN_MODELS.iter().find(|m| m.id == model).map(|m| (m.files.iter().collect(), format!("models/whisper-{}", m.id))),
    }
}

fn start_getting(app: &Arc<App>, part: &str) -> Answer {
    let (files, into) = files_for(part).ok_or_else(|| bad(format!("there's nothing called \"{part}\" to get")))?;
    if getting().get(part).is_some_and(|g| g.2.is_none()) {
        return Ok((StatusCode::ACCEPTED, json!({ "ok": true, "already": true })));
    }
    set_getting(part, Some((0, 0, None)));
    app.hub.send(json!({ "kind": "voice" }));
    let (app, part, into) = (app.clone(), part.to_string(), dir(app).join(into));
    tokio::spawn(async move {
        let result = fetch_all(&app, &part, &files, &into).await;
        match result {
            Ok(()) => set_getting(&part, None),
            Err(why) => {
                eprintln!("catbot: voice: getting {part}: {why}");
                set_getting(&part, Some((0, 0, Some(why))));
            }
        }
        app.hub.send(json!({ "kind": "voice" }));
    });
    Ok((StatusCode::ACCEPTED, json!({ "ok": true })))
}

async fn fetch_all(app: &Arc<App>, part: &str, files: &[&Remote], into: &Path) -> Result<(), String> {
    let client = crate::http::stream_client();
    let work = into.with_extension("part");
    let _ = tokio::fs::remove_dir_all(&work).await;
    tokio::fs::create_dir_all(&work).await.map_err(|e| format!("couldn't make {}: {e}", work.display()))?;
    // How big it all is, to say how far it's got.
    let mut total = 0u64;
    for file in files {
        // The header itself: a HEAD answer's body is always empty.
        if let Ok(head) = crate::http::client().head(source(file.url)).timeout(Duration::from_secs(30)).send().await
            && let Some(len) = head.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok())
        {
            total += len;
        }
    }
    let mut done = 0u64;
    let mut last_said = std::time::Instant::now();
    for file in files {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        let answer = client.get(source(file.url)).send().await.map_err(|e| format!("couldn't reach {}: {e}", host(file.url)))?;
        if !answer.status().is_success() {
            return Err(format!("{} answered {} for {}", host(file.url), answer.status(), file.name));
        }
        let path = work.join(file.name);
        let mut out = tokio::fs::File::create(&path).await.map_err(|e| e.to_string())?;
        let mut hash = sha2::Sha256::new();
        let mut body = answer.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| format!("the download of {} broke off: {e}", file.name))?;
            hash.update(&chunk);
            out.write_all(&chunk).await.map_err(|e| format!("couldn't save {}: {e}", file.name))?;
            done += chunk.len() as u64;
            if last_said.elapsed() > Duration::from_millis(500) {
                last_said = std::time::Instant::now();
                set_getting(part, Some((done, total.max(done), None)));
                app.hub.send(json!({ "kind": "voice", "getting": part, "done": done, "total": total.max(done) }));
            }
        }
        out.flush().await.map_err(|e| e.to_string())?;
        if let Some(wanted) = file.sha256 {
            let got = hex(&hash.finalize());
            if got != wanted {
                return Err(format!("{} didn't match the copy Catbot expects, so it wasn't used", file.name));
            }
        }
    }
    if part == "program" {
        let unpack_dir = work.clone();
        tokio::task::spawn_blocking(move || unpack(&unpack_dir))
            .await
            .map_err(|e| e.to_string())??;
    }
    let _ = tokio::fs::remove_dir_all(into).await;
    tokio::fs::rename(&work, into).await.map_err(|e| format!("couldn't put the files in place: {e}"))?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn host(url: &str) -> &str {
    url.trim_start_matches("https://").split('/').next().unwrap_or(url)
}

/// Takes the two programs and the ONNX runtime out of the packages, beside each other.
fn unpack(work: &Path) -> Result<(), String> {
    for wheel in ["bin.whl", "core.whl"] {
        let file = std::fs::File::open(work.join(wheel)).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{wheel} isn't a package: {e}"))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let Some((_, name)) = UNPACKED.iter().find(|(end, _)| entry.name().ends_with(end)) else { continue };
            let path = work.join(name);
            let mut out = std::fs::File::create(&path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
            }
        }
        let _ = std::fs::remove_file(work.join(wheel));
    }
    match UNPACKED.iter().find(|(_, name)| !work.join(name).is_file()) {
        Some((_, missing)) => Err(format!("the package had no {missing}")),
        None => Ok(()),
    }
}

// ---- Keys ---------------------------------------------------------------------------------------

/// OpenAI's address and key: the same as its chat engine's (Settings › AI), or its own.
fn openai(app: &App) -> Option<(String, String)> {
    let engine = crate::engines::find("openai")?;
    let saved = app.with_store(|store| store.engine_key("openai")).flatten();
    let access = crate::engines::access(engine, saved.as_ref())?;
    Some((access.url, access.key?))
}

fn elevenlabs(app: &App) -> Option<(String, String)> {
    let saved = app.with_store(|store| store.engine_key("elevenlabs")).flatten();
    let url = saved.as_ref().and_then(|s| s.url.clone()).filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "https://api.elevenlabs.io/v1".into());
    let key = saved.map(|s| s.key).filter(|k| !k.is_empty()).or_else(|| std::env::var("ELEVENLABS_API_KEY").ok().filter(|k| !k.trim().is_empty()))?;
    Some((url.trim_end_matches('/').to_string(), key))
}

fn key_shown(app: &App, service: &str) -> Value {
    let found = match service {
        "openai" => openai(app).map(|(_, k)| k),
        _ => elevenlabs(app).map(|(_, k)| k),
    };
    json!({ "saved": found.is_some(), "hint": found.map(|k| crate::engines::key_hint(&k)) })
}

// ---- Listening ---------------------------------------------------------------------------------

/// The words in a recording.
pub async fn listen(app: &Arc<App>, audio: Bytes) -> Result<String, (StatusCode, String)> {
    if audio.is_empty() {
        return Err(bad("the recording is empty"));
    }
    if audio.len() > AUDIO_MAX {
        return Err(bad("the recording is too long; keep it under about ten minutes"));
    }
    let settings = settings(app);
    let text = if settings.listen.place == "online" {
        listen_online(app, &settings, audio).await?
    } else {
        listen_here(app, &settings, audio).await?
    };
    Ok(text.trim().to_string())
}

async fn listen_here(app: &Arc<App>, settings: &Settings, audio: Bytes) -> Result<String, (StatusCode, String)> {
    let model = &settings.listen.model;
    if !have(app, "program") || !have(app, model) {
        return Err((StatusCode::CONFLICT, "Listening on this computer isn't set up yet: get its files in Settings › Voice.".into()));
    }
    if !audio.starts_with(b"RIFF") {
        return Err(bad("listening on this computer takes a WAV recording"));
    }
    let models = part_dir(app, &format!("whisper-{model}"));
    let wav = scratch(app, "wav");
    let _cleanup = TempFileGuard(wav.clone());
    tokio::fs::write(&wav, &audio).await.map_err(fail)?;
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(1, 8)).unwrap_or(2);
    let args = vec![
        format!("--whisper-encoder={}", models.join("encoder.onnx").display()),
        format!("--whisper-decoder={}", models.join("decoder.onnx").display()),
        format!("--tokens={}", models.join("tokens.txt").display()),
        format!("--num-threads={threads}"),
        wav.display().to_string(),
    ];
    let ran = run(&program(app, "sherpa-onnx-offline"), &args, Duration::from_secs(180)).await;
    let (stdout, stderr) = ran.map_err(|why| (StatusCode::BAD_GATEWAY, why))?;
    // sherpa-onnx prints each recording's result as one line of JSON on stdout.
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find_map(|v| v["text"].as_str().map(str::to_string))
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, format!("Whisper gave no words back. {}", last_line(&stderr))))
}

async fn listen_online(app: &Arc<App>, settings: &Settings, audio: Bytes) -> Result<String, (StatusCode, String)> {
    let (url, key) = openai(app).ok_or((StatusCode::CONFLICT, "Listening online needs an OpenAI key: add one in Settings › Voice.".to_string()))?;
    let (name, mime) = kind_of(&audio);
    let boundary = format!("catbot{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::with_capacity(audio.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{}\r\n", settings.listen.online_model).as_bytes());
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(&audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let answer = client()
        .post(format!("{url}/audio/transcriptions"))
        .timeout(Duration::from_secs(180))
        .bearer_auth(key)
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("couldn't reach OpenAI: {e}")))?;
    let status = answer.status();
    let said: Value = answer.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err((StatusCode::BAD_GATEWAY, format!("OpenAI said no ({status}): {}", said["error"]["message"].as_str().unwrap_or("no reason given"))));
    }
    record(app, "openaiListenSeconds", seconds_of(&audio));
    Ok(said["text"].as_str().unwrap_or_default().to_string())
}

/// A recording's file name and type, from its first bytes.
fn kind_of(audio: &[u8]) -> (&'static str, &'static str) {
    if audio.starts_with(b"RIFF") {
        ("speech.wav", "audio/wav")
    } else if audio.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        ("speech.webm", "audio/webm")
    } else if audio.starts_with(b"OggS") {
        ("speech.ogg", "audio/ogg")
    } else if audio.get(4..8) == Some(b"ftyp") {
        ("speech.mp4", "audio/mp4")
    } else {
        ("speech.mp3", "audio/mpeg")
    }
}

// ---- Speaking ----------------------------------------------------------------------------------

/// A bot's reply as audio: its bytes and their type.
pub async fn speak(app: &Arc<App>, text: &str, bot: Option<&str>) -> Result<(Vec<u8>, &'static str), (StatusCode, String)> {
    let words = speakable(text);
    if words.trim().is_empty() {
        return Err(bad("there's nothing to say"));
    }
    let settings = settings(app);
    let engine = if settings.reply.place == "online" { settings.reply.online.clone() } else { settings.reply.local.clone() };
    let voice = bot
        .and_then(|b| settings.bots.get(b))
        .and_then(|v| v.get(&engine))
        .or_else(|| settings.reply.voices.get(&engine))
        .cloned()
        .unwrap_or_default();
    match engine.as_str() {
        "openai" => speak_openai(app, &settings, &words, &voice).await,
        "elevenlabs" => speak_elevenlabs(app, &settings, &words, &voice).await,
        local => speak_here(app, local, &words, &voice).await,
    }
}

async fn speak_here(app: &Arc<App>, engine: &str, words: &str, voice: &str) -> Result<(Vec<u8>, &'static str), (StatusCode, String)> {
    if !have(app, "program") || !have(app, engine) {
        return Err((StatusCode::CONFLICT, "Speaking on this computer isn't set up yet: get its files in Settings › Voice.".into()));
    }
    let models = part_dir(app, engine);
    let out = scratch(app, "wav");
    let _cleanup = TempFileGuard(out.clone());
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(1, 4)).unwrap_or(2);
    let mut args = match engine {
        "piper" => vec![
            format!("--vits-model={}", models.join("model.onnx").display()),
            format!("--vits-tokens={}", models.join("tokens.txt").display()),
            format!("--vits-lexicon={}", models.join("lexicon-en-us.txt").display()),
            "--sid=0".to_string(),
        ],
        _ => {
            let sid = KOKORO_VOICES.iter().find(|(id, _)| *id == voice).map(|(id, _)| *id).unwrap_or(KOKORO_DEFAULT);
            vec![
                format!("--kokoro-model={}", models.join("model.onnx").display()),
                format!("--kokoro-voices={}", models.join("voices.bin").display()),
                format!("--kokoro-tokens={}", models.join("tokens.txt").display()),
                format!("--kokoro-lexicon={}", models.join("lexicon-us-en.txt").display()),
                format!("--sid={sid}"),
            ]
        }
    };
    args.push(format!("--num-threads={threads}"));
    args.push(format!("--output-filename={}", out.display()));
    // Prevent sherpa-onnx from parsing leading `-` in spoken text as CLI options.
    args.push(if words.starts_with('-') { format!(" {words}") } else { words.to_string() });
    let ran = run(&program(app, "sherpa-onnx-offline-tts"), &args, Duration::from_secs(300)).await;
    let audio = tokio::fs::read(&out).await;
    let (_, stderr) = ran.map_err(|why| (StatusCode::BAD_GATEWAY, why))?;
    match audio {
        Ok(bytes) if bytes.starts_with(b"RIFF") => Ok((bytes, "audio/wav")),
        _ => Err((StatusCode::BAD_GATEWAY, format!("The voice made no sound. {}", last_line(&stderr)))),
    }
}

async fn speak_openai(app: &Arc<App>, settings: &Settings, words: &str, voice: &str) -> Result<(Vec<u8>, &'static str), (StatusCode, String)> {
    let (url, key) = openai(app).ok_or((StatusCode::CONFLICT, "Speaking online needs an OpenAI key: add one in Settings › Voice.".to_string()))?;
    let voice = if voice.is_empty() { OPENAI_DEFAULT } else { voice };
    let body = json!({ "model": settings.reply.openai_model, "voice": voice, "input": words, "response_format": "mp3" });
    let answer = client()
        .post(format!("{url}/audio/speech"))
        .timeout(Duration::from_secs(180))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("couldn't reach OpenAI: {e}")))?;
    let audio = audio_from("OpenAI", answer).await?;
    record(app, "openaiSpeakLetters", words.chars().count() as f64);
    Ok(audio)
}

async fn speak_elevenlabs(app: &Arc<App>, settings: &Settings, words: &str, voice: &str) -> Result<(Vec<u8>, &'static str), (StatusCode, String)> {
    let (url, key) = elevenlabs(app).ok_or((StatusCode::CONFLICT, "ElevenLabs needs its key: add one in Settings › Voice.".to_string()))?;
    let voice = if voice.is_empty() {
        // None chosen yet: the first of the account's voices.
        elevenlabs_voices(app).await?.first().and_then(|v| v["id"].as_str().map(str::to_string)).ok_or((StatusCode::BAD_GATEWAY, "ElevenLabs lists no voices for this key".to_string()))?
    } else {
        voice.to_string()
    };
    if voice.is_empty() || voice.len() > 128 || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(bad("that isn't an ElevenLabs voice"));
    }
    let body = json!({ "text": words, "model_id": settings.reply.elevenlabs_model });
    let answer = client()
        .post(format!("{url}/text-to-speech/{voice}?output_format=mp3_44100_128"))
        .timeout(Duration::from_secs(180))
        .header("xi-api-key", key)
        .json(&body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("couldn't reach ElevenLabs: {e}")))?;
    let audio = audio_from("ElevenLabs", answer).await?;
    // Flash and Turbo models take half a credit a letter.
    let cheap = settings.reply.elevenlabs_model.contains("flash") || settings.reply.elevenlabs_model.contains("turbo");
    record(app, if cheap { "elevenFlashLetters" } else { "elevenLetters" }, words.chars().count() as f64);
    Ok(audio)
}

async fn audio_from(who: &str, answer: reqwest::Response) -> Result<(Vec<u8>, &'static str), (StatusCode, String)> {
    let status = answer.status();
    let bytes = answer.bytes().await.map_err(|e| (StatusCode::BAD_GATEWAY, format!("{who}'s answer broke off: {e}")))?;
    if !status.is_success() {
        let said: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let why = said["error"]["message"].as_str().or(said["detail"]["message"].as_str()).unwrap_or("no reason given");
        return Err((StatusCode::BAD_GATEWAY, format!("{who} said no ({status}): {why}")));
    }
    Ok((bytes.to_vec(), "audio/mpeg"))
}

async fn elevenlabs_voices(app: &Arc<App>) -> Result<Vec<Value>, (StatusCode, String)> {
    let (url, key) = elevenlabs(app).ok_or((StatusCode::CONFLICT, "no ElevenLabs key yet".to_string()))?;
    let answer = client()
        .get(format!("{url}/voices"))
        .timeout(Duration::from_secs(30))
        .header("xi-api-key", key)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("couldn't reach ElevenLabs: {e}")))?;
    let said: Value = answer.json().await.unwrap_or(Value::Null);
    Ok(said["voices"]
        .as_array()
        .map(|list| list.iter().filter_map(|v| Some(json!({ "id": v["voice_id"].as_str()?, "name": v["name"].as_str().unwrap_or("Voice") }))).collect())
        .unwrap_or_default())
}

/// A reply as it's read aloud: no code, links or markdown marks; at most a few thousand letters.
pub fn speakable(text: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if !in_code {
                out.push_str("(There's some code here.)\n");
            }
            in_code = !in_code;
            continue;
        }
        if in_code || trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.chars().all(|c| "|-: ".contains(c)) {
            continue;
        }
        let line = trimmed.trim_start_matches(['#', '>', ' ']).trim_start_matches("- ").trim_start_matches("* ");
        let mut plain = String::new();
        let mut rest = line;
        // [words](address) → words; bare addresses → "a link".
        while let Some(open) = rest.find('[') {
            let (before, after) = rest.split_at(open);
            plain.push_str(before);
            match (after.find("]("), after.find(')')) {
                (Some(mid), Some(close)) if mid < close => {
                    plain.push_str(&after[1..mid]);
                    rest = &after[close + 1..];
                }
                _ => {
                    plain.push('[');
                    rest = &after[1..];
                }
            }
        }
        plain.push_str(rest);
        let words: Vec<String> = plain
            .split_whitespace()
            .map(|w| if w.starts_with("http://") || w.starts_with("https://") { "a link".to_string() } else { w.replace(['*', '`', '|'], "").replace("__", "") })
            .filter(|w| !w.is_empty())
            .collect();
        if !words.is_empty() {
            out.push_str(&words.join(" "));
            out.push('\n');
        }
    }
    let out = out.trim().to_string();
    if out.chars().count() <= SPEAK_MAX {
        return out;
    }
    let cut: String = out.chars().take(SPEAK_MAX).collect();
    let end = cut.rfind(['.', '!', '?']).map(|i| i + 1).unwrap_or(cut.len());
    format!("{} The rest is in the chat.", &cut[..end])
}

// ---- What online voice spends ------------------------------------------------------------------

const SPENT: &str = "voice_spend";
/// About what each costs, in dollars, as listed on 2026-11-24 (an estimate: plans differ).
const PRICES: [(&str, f64, &str); 4] = [
    ("openaiListenSeconds", 0.003 / 60.0, "OpenAI listening"),
    // gpt-4o-mini-tts: about 1.5¢ a minute of speech, about 900 letters.
    ("openaiSpeakLetters", 0.015 / 900.0, "OpenAI speaking"),
    ("elevenLetters", 0.10 / 1000.0, "ElevenLabs"),
    ("elevenFlashLetters", 0.05 / 1000.0, "ElevenLabs (Flash)"),
];

static SPENDING: Mutex<()> = Mutex::new(());

/// Adds to today's count of something online voice used (kept a year, by day).
fn record(app: &App, what: &str, amount: f64) {
    if amount <= 0.0 {
        return;
    }
    let _one = SPENDING.lock().expect("voice spend lock");
    let mut days = app.with_store(|store| store.setting(SPENT)).flatten().unwrap_or_else(|| json!({}));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let was = days[&today][what].as_f64().unwrap_or(0.0);
    days[&today][what] = json!(was + amount);
    if let Some(all) = days.as_object_mut() {
        let year_ago = (chrono::Local::now() - chrono::Duration::days(400)).format("%Y-%m-%d").to_string();
        all.retain(|day, _| *day >= year_ago);
    }
    app.with_store(|store| store.set_setting(SPENT, &days));
}

/// What online voice used between two times (by local day), and about what it cost.
pub fn spent(app: &App, from: chrono::DateTime<chrono::Utc>, to: chrono::DateTime<chrono::Utc>) -> Value {
    let days = app.with_store(|store| store.setting(SPENT)).flatten().unwrap_or_else(|| json!({}));
    let (first, last) = (from.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string(), to.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string());
    let mut lines = Vec::new();
    let mut total = 0.0;
    for (key, price, label) in PRICES {
        let amount: f64 = days.as_object().into_iter().flatten().filter(|(day, _)| **day >= first && **day <= last).filter_map(|(_, v)| v[key].as_f64()).sum();
        if amount > 0.0 {
            let cost = amount * price;
            total += cost;
            let used = if key.ends_with("Seconds") { format!("{:.0} min", (amount / 60.0).ceil()) } else { format!("{} letters", amount.round()) };
            lines.push(json!({ "label": label, "used": used, "costUsd": (cost * 10000.0).round() / 10000.0 }));
        }
    }
    json!({ "lines": lines, "costUsd": (total * 10000.0).round() / 10000.0 })
}

/// How long a WAV recording is, in seconds (0 for anything else).
fn seconds_of(audio: &[u8]) -> f64 {
    if !audio.starts_with(b"RIFF") || audio.len() < 44 {
        return 0.0;
    }
    let bytes_a_second = u32::from_le_bytes([audio[28], audio[29], audio[30], audio[31]]);
    if bytes_a_second == 0 { 0.0 } else { (audio.len() - 44) as f64 / f64::from(bytes_a_second) }
}

// ---- Running the programs ----------------------------------------------------------------------

fn client() -> &'static reqwest::Client {
    crate::http::client()
}

/// Limits concurrent local sherpa-onnx subprocesses so simultaneous voice requests cannot OOM the host.
static LOCAL_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn scratch(app: &App, ext: &str) -> PathBuf {
    let dir = dir(app).join("tmp");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{}.{ext}", uuid::Uuid::new_v4().simple()))
}

fn last_line(text: &str) -> String {
    text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().chars().take(300).collect()
}

/// Runs a program with no shell between (the words are one argument), and waits for it.
async fn run(program: &Path, args: &[String], limit: Duration) -> Result<(String, String), String> {
    let _slot = LOCAL_SLOTS.acquire().await.map_err(|_| "voice runner closed".to_string())?;
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    if let Some(folder) = program.parent() {
        command.current_dir(folder);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(limit, command.output())
        .await
        .map_err(|_| format!("{} took too long", program.file_name().and_then(|n| n.to_str()).unwrap_or("the program")))?
        .map_err(|e| format!("couldn't start {}: {e}", program.display()))?;
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout).to_string(), String::from_utf8_lossy(&output.stderr).to_string());
    if !output.status.success() {
        return Err(format!("{} stopped with an error. {}", program.file_name().and_then(|n| n.to_str()).unwrap_or("The program"), last_line(&stderr)));
    }
    Ok((stdout, stderr))
}

// ---- Routes -------------------------------------------------------------------------------------

fn shown(app: &App) -> Value {
    let settings = settings(app);
    let now = getting();
    let part = |id: &str| {
        let g = now.get(id);
        json!({
            "ready": have(app, id),
            "getting": g.is_some_and(|g| g.2.is_none()),
            "done": g.map(|g| g.0),
            "total": g.map(|g| g.1),
            "problem": g.and_then(|g| g.2.clone()),
        })
    };
    json!({
        "settings": settings,
        "keys": { "openai": key_shown(app, "openai"), "elevenlabs": key_shown(app, "elevenlabs") },
        "local": {
            "program": part("program"),
            "listen": LISTEN_MODELS.iter().map(|m| json!({ "id": m.id, "label": m.label, "size": m.size, "state": part(m.id) })).collect::<Vec<_>>(),
            "kokoro": part("kokoro"),
            "piper": part("piper"),
        },
        "voices": {
            "kokoro": KOKORO_VOICES.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
            "piper": [{ "id": "0", "name": "Amy (US, woman)" }],
            "openai": OPENAI_VOICES.iter().map(|v| json!({ "id": v, "name": format!("{}{}", v[..1].to_uppercase(), &v[1..]) })).collect::<Vec<_>>(),
        },
        "defaults": { "kokoro": KOKORO_DEFAULT, "piper": "0", "openai": OPENAI_DEFAULT },
    })
}

/// Everything under `/api/voice`; `at_home` is false at the remote door (no keys, no downloads).
pub async fn route(app: &Arc<App>, method: Method, rest: &[&str], req: Request, at_home: bool) -> Response {
    match (&method, rest) {
        (&Method::POST, ["listen"]) => {
            let audio = match axum::body::to_bytes(req.into_body(), AUDIO_MAX + 1).await {
                Ok(bytes) => bytes,
                Err(_) => return respond(Err(bad("the recording is too long; keep it under about ten minutes"))),
            };
            respond(listen(app, audio).await.map(|text| (StatusCode::OK, json!({ "text": text }))))
        }
        (&Method::POST, ["speak"]) => {
            let body = match json_body(req).await {
                Ok(body) => body,
                Err(err) => return respond(Err(err)),
            };
            match speak(app, body["text"].as_str().unwrap_or_default(), body["botId"].as_str()).await {
                Ok((audio, kind)) => (StatusCode::OK, [(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "no-store")], audio).into_response(),
                Err(err) => respond(Err(err)),
            }
        }
        _ => {
            let body = if matches!(method, Method::PUT | Method::POST) {
                match json_body(req).await {
                    Ok(body) => body,
                    Err(err) => return respond(Err(err)),
                }
            } else {
                Value::Null
            };
            respond(settle(app, &method, rest, body, at_home).await)
        }
    }
}

async fn settle(app: &Arc<App>, method: &Method, rest: &[&str], body: Value, at_home: bool) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, shown(app))),
        (&Method::GET, ["elevenlabs-voices"]) => Ok((StatusCode::OK, json!({ "voices": elevenlabs_voices(app).await? }))),
        (&Method::PUT, []) => {
            let next = changed(settings(app), &body).map_err(bad)?;
            save(app, &next)?;
            app.hub.send(json!({ "kind": "voice" }));
            Ok((StatusCode::OK, shown(app)))
        }
        (&Method::PUT, ["keys", service @ ("openai" | "elevenlabs")]) if at_home => {
            let key = crate::engines::check_key(body["key"].as_str().unwrap_or_default()).map_err(bad)?;
            let url = body["url"].as_str().filter(|u| !u.trim().is_empty()).map(crate::engines::check_url).transpose().map_err(bad)?;
            // OpenAI's key is the one Settings › AI keeps for its chat engine too.
            let kept_url = if *service == "openai" { url.or_else(|| app.with_store(|s| s.engine_key("openai")).flatten().and_then(|k| k.url)) } else { url };
            app.try_store(|store| store.set_engine_key(service, &key, kept_url.as_deref())).map_err(fail)?;
            app.hub.send(json!({ "kind": "voice" }));
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, shown(app)))
        }
        (&Method::DELETE, ["keys", service @ ("openai" | "elevenlabs")]) if at_home => {
            app.try_store(|store| store.remove_engine_key(service)).map_err(fail)?;
            app.hub.send(json!({ "kind": "voice" }));
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, shown(app)))
        }
        (&Method::POST, ["get"]) if at_home => start_getting(app, body["part"].as_str().unwrap_or_default()),
        (_, ["keys", ..] | ["get"]) => Err((StatusCode::FORBIDDEN, "keys and downloads are managed at home".into())),
        _ => Err((StatusCode::NOT_FOUND, "voice doesn't do that".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_read_without_code_links_or_marks() {
        let said = speakable("## Done\n\nI fixed **two** things, see [the notes](https://x.y/z) and https://a.b.\n\n```rust\nfn main() {}\n```\n| a | b |\n|---|---|\n- `cargo test` passes");
        assert_eq!(said, "Done\nI fixed two things, see the notes and a link\n(There's some code here.)\n| a | b |\ncargo test passes".replace("| a | b |", "a b"));
        let long = "Word. ".repeat(2000);
        let cut = speakable(&long);
        assert!(cut.ends_with("The rest is in the chat.") && cut.len() < SPEAK_MAX + 40);
    }

    #[test]
    fn settings_change_part_by_part_and_are_checked() {
        let now = changed(Settings::default(), &json!({ "listen": { "on": true, "where": "online" } })).unwrap();
        assert!(now.listen.on && now.listen.place == "online" && now.listen.model == "base.en");
        assert!(changed(now.clone(), &json!({ "reply": { "local": "festival" } })).is_err());
        assert!(changed(now.clone(), &json!({ "listen": { "model": "huge" } })).is_err());
        let voiced = changed(now, &json!({ "bots": { "b1": { "kokoro": "11" } } })).unwrap();
        assert_eq!(voiced.bots["b1"]["kokoro"], "11");
        let cleared = changed(voiced, &json!({ "bots": { "b1": { "kokoro": "" } } })).unwrap();
        assert!(cleared.bots.is_empty(), "an empty voice goes back to the default");
    }

    #[test]
    fn a_recordings_length_is_read_from_its_header() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\0\0\0\0".to_vec();
        wav.extend(vec![0u8; 64_000]);
        assert_eq!(seconds_of(&wav), 2.0);
        assert_eq!(seconds_of(b"ID3"), 0.0);
    }

    #[test]
    fn recordings_are_told_apart() {
        assert_eq!(kind_of(b"RIFF....WAVE").0, "speech.wav");
        assert_eq!(kind_of(&[0x1a, 0x45, 0xdf, 0xa3, 0]).0, "speech.webm");
        assert_eq!(kind_of(b"\0\0\0\x20ftypM4A ").0, "speech.mp4");
    }
}
