//! AI on this computer (stage 24): finding Ollama and LM Studio, adding them in one click,
//! starting Ollama, and getting and removing its models.
//!
//! Moo's choices (2026-11-25): when one is found running, Catbot offers it ("Use it") and
//! changes nothing until he clicks; installed but stopped, Catbot starts it, and not installed it
//! links to the download with the command to paste (installing needs his password); Catbot gets
//! and removes Ollama's models, from a short list sized to his graphics card or any name; a
//! thinking model's thinking is a per-bot Off/On (`engines::effort_levels`).
//!
//! - `GET  /api/local-ai`: both, with what's found, the models, sizes, the suggestions and how
//!   far a download has got.
//! - `POST /api/local-ai/:id/use`: adds it to the AIs bots can choose.
//! - `POST /api/local-ai/:id/start`: starts it (Ollama's service or `ollama serve`; LM Studio's
//!   `lms server start`).
//! - `POST /api/local-ai/ollama/pull {model}`, `DELETE /api/local-ai/ollama/pull?model=`:
//!   gets a model (with `local-ai` frames for progress), or stops getting it.
//! - `POST /api/local-ai/ollama/remove {model}`: removes one.
//!
//! The rest of the time an engine on this computer is like any other (`engines.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::engines;

const ADDED: &str = "local_ai_added";

/// Models suggested for Ollama (as of November 2026): the name to get, what it's like, and about
/// how much memory it needs to run quickly, in GB.
pub const SUGGESTED: [(&str, &str, f64); 6] = [
    ("qwen3.5:4b", "Small and quick; thinks before answering", 4.0),
    ("llama3.1:8b", "A well-known all-rounder", 6.0),
    ("deepseek-r1:8b", "Reasons step by step", 6.0),
    ("qwen3.5:9b", "The strongest that fits an 8 GB graphics card; thinks", 7.0),
    ("gemma4:12b", "Bigger and better at writing; tight on 8 GB", 9.0),
    ("gpt-oss:20b", "OpenAI's open model; thinking levels", 14.0),
];

fn engine(id: &str) -> Result<&'static engines::Engine, (StatusCode, String)> {
    engines::find(id).filter(|e| e.kind == engines::Kind::Api && !e.needs_key).ok_or_else(|| (StatusCode::NOT_FOUND, format!("{id} isn't an AI on this computer")))
}

/// Where it answers, without `/v1` (Ollama's own routes are beside the OpenAI ones).
fn base(app: &App, id: &str) -> String {
    let saved = app.with_store(|store| store.engine_key(id)).flatten();
    let url = engines::find(id).and_then(|e| engines::access(e, saved.as_ref())).map(|a| a.url).unwrap_or_default();
    url.trim_end_matches('/').trim_end_matches("/v1").to_string()
}

/// Whether it's offered to bots: the person added it, or a bot already uses it.
pub fn is_added(app: &App, id: &str) -> bool {
    let added = app.with_store(|store| store.setting(ADDED)).flatten();
    added.as_ref().and_then(Value::as_array).is_some_and(|ids| ids.iter().any(|v| v == id))
        || app.with_store(|store| store.uses_engine(id)).unwrap_or_default()
}

fn add(app: &App, id: &str) -> Result<(), (StatusCode, String)> {
    app.try_store(|store| {
        let mut added = store.setting(ADDED)?.and_then(|v| v.as_array().cloned()).unwrap_or_default();
        if !added.iter().any(|v| v == id) {
            added.push(json!(id));
            store.set_setting(ADDED, &json!(added))?;
        }
        Ok(())
    }).map_err(fail)
}

/// The program, if it's on this computer (`CATBOT_OLLAMA_PROGRAM` stands in for it in tests).
fn program(id: &str) -> Option<std::path::PathBuf> {
    if id == "ollama"
        && let Some(given) = std::env::var_os("CATBOT_OLLAMA_PROGRAM")
    {
        return Some(given.into());
    }
    let (name, also): (&str, Vec<std::path::PathBuf>) = match id {
        "ollama" => (
            "ollama",
            std::env::var_os("LOCALAPPDATA").map(|l| vec![std::path::PathBuf::from(l).join("Programs/Ollama/ollama.exe")]).unwrap_or_default(),
        ),
        _ => ("lms", vec![crate::store::user_home().join(".lmstudio/bin").join(if cfg!(windows) { "lms.exe" } else { "lms" })]),
    };
    engines::find_program(name).or_else(|| also.into_iter().find(|p| p.is_file()))
}

/// Cached `(model_name, digest) -> thinks` so discovery doesn't hammer `/api/show` on every refresh.
static THINK_CACHE: OnceLock<Mutex<HashMap<(String, String), bool>>> = OnceLock::new();

fn think_cache() -> &'static Mutex<HashMap<(String, String), bool>> {
    THINK_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What one says about itself: running (and its version), and its models.
async fn look(app: &App, id: &str) -> Value {
    let base = base(app, id);
    let http = crate::http::client();
    let quick = Duration::from_secs(3);
    if id == "ollama" {
        let Ok(answer) = http.get(format!("{base}/api/version")).timeout(quick).send().await else {
            return json!({ "running": false });
        };
        let version: Value = answer.json().await.unwrap_or(Value::Null);
        let tags: Value = match http.get(format!("{base}/api/tags")).timeout(quick).send().await {
            Ok(answer) => answer.json().await.unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        let raw_models: Vec<Value> = tags["models"].as_array().cloned().unwrap_or_default();
        // Probe uncached models concurrently; reuse cached capability when `(name, digest)` matches.
        let lookups = raw_models.iter().map(|m| {
            let base = base.clone();
            let name = m["name"].as_str().unwrap_or_default().to_string();
            let digest = m["digest"].as_str().unwrap_or_default().to_string();
            async move {
                if !digest.is_empty()
                    && let Some(&cached) = think_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&(name.clone(), digest.clone()))
                {
                    return (name, cached);
                }
                let shown: Value = match http.post(format!("{base}/api/show")).timeout(quick).json(&json!({ "model": name })).send().await {
                    Ok(answer) => answer.json().await.unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                };
                let thinks = shown["capabilities"].as_array().is_some_and(|c| c.iter().any(|x| x == "thinking"));
                if !digest.is_empty() && !shown.is_null() {
                    let mut cache = think_cache().lock().unwrap_or_else(|e| e.into_inner());
                    if cache.len() > 256 {
                        cache.clear();
                    }
                    cache.insert((name.clone(), digest), thinks);
                }
                (name, thinks)
            }
        });
        let thinkers: Vec<(String, bool)> = futures_util::future::join_all(lookups).await;
        let models: Vec<Value> = raw_models
            .iter()
            .zip(thinkers.iter())
            .map(|(m, (name, thinks))| {
                json!({
                    "name": name,
                    "size": m["size"],
                    "parameters": m["details"]["parameter_size"],
                    "quantization": m["details"]["quantization_level"],
                    "family": m["details"]["family"],
                    "thinks": thinks,
                })
            })
            .collect();
        engines::note_thinkers(thinkers);
        json!({ "running": true, "version": version["version"], "models": models })
    } else {
        let Ok(answer) = http.get(format!("{base}/v1/models")).timeout(quick).send().await else {
            return json!({ "running": false });
        };
        let listed: Value = answer.json().await.unwrap_or(Value::Null);
        let models: Vec<Value> = listed["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).map(|id| json!({ "name": id })).collect();
        json!({ "running": true, "models": models })
    }
}

/// Static hardware memory cached once per process so discovery doesn't spawn `nvidia-smi` repeatedly.
static HARDWARE_MEM: OnceLock<(Option<f64>, Option<f64>)> = OnceLock::new();

async fn cached_memory() -> (Option<f64>, Option<f64>) {
    if let Some(&mem) = HARDWARE_MEM.get() {
        return mem;
    }
    let mem = tokio::task::spawn_blocking(memory).await.unwrap_or((None, None));
    *HARDWARE_MEM.get_or_init(|| mem)
}

/// The graphics card's memory and the computer's, in GB, where they can be told.
fn memory() -> (Option<f64>, Option<f64>) {
    let mut command = std::process::Command::new("nvidia-smi");
    command.args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"]).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let gpu = command
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse::<f64>().ok()).reduce(f64::max))
        .map(|mib| (mib / 1024.0 * 10.0).round() / 10.0);
    let ram = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|info| info.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|kb| kb.parse::<f64>().ok()))
        .map(|kb| (kb / 1_048_576.0).round());
    (gpu, ram)
}

/// How well a model that needs `need` GB runs here.
pub fn fit(need: f64, gpu: Option<f64>, ram: Option<f64>) -> &'static str {
    match (gpu, ram) {
        (Some(g), _) if need <= g => "fits",
        (Some(g), Some(r)) if need <= g + r / 2.0 => "partly",
        (None, Some(r)) if need <= r / 2.0 => "slow",
        (None, None) | (Some(_), None) => "unknown",
        _ => "too big",
    }
}

// ---- Getting models ----------------------------------------------------------------------------

/// Downloads under way: model → (bytes done, bytes in all, what Ollama says, the problem).
type Pull = (u64, u64, String, Option<String>);
static PULLS: Mutex<Option<HashMap<String, Pull>>> = Mutex::new(None);
static TASKS: Mutex<Option<HashMap<String, (u64, tokio::task::JoinHandle<()>)>>> = Mutex::new(None);
static PULL_SEQ: AtomicU64 = AtomicU64::new(1);

fn pulls() -> HashMap<String, Pull> {
    PULLS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

fn set_pull(model: &str, pull: Option<Pull>) {
    let mut all = PULLS.lock().unwrap_or_else(|e| e.into_inner());
    let map = all.get_or_insert_with(HashMap::new);
    match pull {
        Some(p) => {
            map.insert(model.to_string(), p);
        }
        None => {
            map.remove(model);
        }
    }
}

/// A model's name as Ollama takes it: letters, digits and `. _ - : /`.
fn check_model(name: &str) -> Result<String, (StatusCode, String)> {
    let name = name.trim();
    if name.is_empty() || name.len() > 120 || !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-:/".contains(c)) {
        return Err(bad("a model's name is like qwen3.5:9b"));
    }
    Ok(name.to_string())
}

fn pull(app: &Arc<App>, model: String) -> Answer {
    {
        let mut all = PULLS.lock().unwrap_or_else(|e| e.into_inner());
        let map = all.get_or_insert_with(HashMap::new);
        if map.get(&model).is_some_and(|p| p.3.is_none()) {
            return Ok((StatusCode::ACCEPTED, json!({ "already": true })));
        }
        map.insert(model.clone(), (0, 0, "starting".into(), None));
    }
    app.hub.send(json!({ "kind": "local-ai" }));
    let token = PULL_SEQ.fetch_add(1, Ordering::Relaxed);
    let (app2, name) = (app.clone(), model.clone());
    let task = tokio::spawn(async move {
        let app = app2;
        let result = pull_stream(&app, &name).await;
        match result {
            Ok(()) => {
                set_pull(&name, None);
                // What it can do (thinking) is read with the list.
                let _ = look(&app, "ollama").await;
            }
            Err(why) => set_pull(&name, Some((0, 0, String::new(), Some(why)))),
        }
        {
            let mut tasks = TASKS.lock().unwrap_or_else(|e| e.into_inner());
            let map = tasks.get_or_insert_with(HashMap::new);
            if map.get(&name).is_some_and(|(t, _)| *t == token) {
                map.remove(&name);
            }
        }
        app.hub.send(json!({ "kind": "local-ai" }));
        app.hub.send(json!({ "kind": "engines" }));
    });
    if let Some((_, prev)) = TASKS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(model, (token, task)) {
        prev.abort();
    }
    Ok((StatusCode::ACCEPTED, json!({ "ok": true })))
}

async fn pull_stream(app: &App, model: &str) -> Result<(), String> {
    use futures_util::StreamExt;
    let answer = crate::http::stream_client()
        .post(format!("{}/api/pull", base(app, "ollama")))
        .json(&json!({ "model": model, "stream": true }))
        .send()
        .await
        .map_err(|_| "Ollama isn't answering; start it first".to_string())?;
    if !answer.status().is_success() {
        let said: Value = answer.json().await.unwrap_or(Value::Null);
        return Err(said["error"].as_str().unwrap_or("Ollama said no").to_string());
    }
    let mut body = answer.bytes_stream();
    let mut lines = crate::stream_lines::StreamLines::default();
    let mut last_said = std::time::Instant::now() - Duration::from_secs(1);
    let mut finished = false;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| format!("the download broke off: {e}"))?;
        for line in lines.push(&chunk) {
            let Ok(event) = serde_json::from_slice::<Value>(&line) else { continue };
            if let Some(error) = event["error"].as_str() {
                return Err(if error.contains("file does not exist") { format!("Ollama has no model called {model}") } else { error.to_string() });
            }
            let status = event["status"].as_str().unwrap_or_default().to_string();
            finished |= status == "success";
            let (done, total) = (event["completed"].as_u64().unwrap_or(0), event["total"].as_u64().unwrap_or(0));
            set_pull(model, Some((done, total, status.clone(), None)));
            if last_said.elapsed() > Duration::from_millis(500) {
                last_said = std::time::Instant::now();
                app.hub.send(json!({ "kind": "local-ai", "pulling": model, "done": done, "total": total, "status": status }));
            }
        }
    }
    if finished { Ok(()) } else { Err("the download stopped before it finished".into()) }
}

fn stop_pull(model: &str) {
    if let Some((_, task)) = TASKS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).remove(model) {
        task.abort();
    }
    set_pull(model, None);
}

// ---- Starting --------------------------------------------------------------------------------

/// Starts it, without waiting: Ollama's own service where it has one (Linux asks for the
/// person's password in its own window), else `ollama serve`; LM Studio's `lms server start`.
fn start(id: &str) -> Result<String, String> {
    let program = program(id).ok_or_else(|| format!("{} isn't installed on this computer", if id == "ollama" { "Ollama" } else { "LM Studio's lms" }))?;
    let mut command = if id == "ollama" {
        if cfg!(target_os = "linux") && std::path::Path::new("/etc/systemd/system/ollama.service").is_file() && std::env::var_os("CATBOT_OLLAMA_PROGRAM").is_none() {
            let mut c = std::process::Command::new("systemctl");
            c.args(["start", "ollama"]);
            c
        } else if cfg!(windows) && program.with_file_name("ollama app.exe").is_file() {
            std::process::Command::new(program.with_file_name("ollama app.exe"))
        } else {
            let mut c = std::process::Command::new(&program);
            c.arg("serve");
            c
        }
    } else {
        let mut c = std::process::Command::new(&program);
        c.args(["server", "start"]);
        c
    };
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No window, and not ended with Catbot.
        command.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    command.spawn().map_err(|e| format!("couldn't start it: {e}"))?;
    Ok(program.display().to_string())
}

// ---- Routes ------------------------------------------------------------------------------------

async fn shown(app: &App) -> Value {
    let ((gpu, ram), (ollama, lmstudio)) = tokio::join!(cached_memory(), async { tokio::join!(look(app, "ollama"), look(app, "lmstudio")) });
    let now = pulls();
    let one = |id: &str, mut seen: Value| {
        seen["id"] = json!(id);
        seen["installed"] = json!(program(id).is_some() || seen["running"] == true);
        seen["added"] = json!(is_added(app, id));
        seen["url"] = json!(base(app, id));
        seen
    };
    let ollama = one("ollama", ollama);
    let have: Vec<String> = ollama["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(str::to_string)).collect();
    json!({
        "ollama": ollama,
        "lmstudio": one("lmstudio", lmstudio),
        "memory": { "gpu": gpu, "ram": ram },
        "suggested": SUGGESTED.iter().map(|(name, about, need)| json!({
            "name": name, "about": about, "needs": need, "fit": fit(*need, gpu, ram),
            "have": have.iter().any(|h| h == name),
        })).collect::<Vec<_>>(),
        "pulling": now.iter().map(|(model, (done, total, status, problem))| json!({ "model": model, "done": done, "total": total, "status": status, "problem": problem })).collect::<Vec<_>>(),
        "install": {
            "ollama": if cfg!(windows) { "https://ollama.com/download/windows" } else { "curl -fsSL https://ollama.com/install.sh | sh" },
            "ollamaPage": "https://ollama.com/download",
            "lmstudioPage": "https://lmstudio.ai/download",
        },
    })
}

pub async fn route(app: &Arc<App>, method: &axum::http::Method, rest: &[&str], query: Option<String>, body: Value, at_home: bool) -> Answer {
    use axum::http::Method;
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, shown(app).await)),
        (_, [_, "start" | "use"] | ["ollama", "pull" | "remove"]) if !at_home => Err((StatusCode::FORBIDDEN, "AI on this computer is managed at home".into())),
        (&Method::POST, [id, "use"]) => {
            engine(id)?;
            add(app, id)?;
            app.hub.send(json!({ "kind": "engines" }));
            app.hub.send(json!({ "kind": "local-ai" }));
            Ok((StatusCode::OK, shown(app).await))
        }
        (&Method::POST, [id, "start"]) => {
            engine(id)?;
            let started = start(id).map_err(|why| (StatusCode::CONFLICT, why))?;
            // A moment for it to answer, so the page can say it's running (cheap ping only).
            let probe = format!("{}{}", base(app, id), if *id == "ollama" { "/api/version" } else { "/v1/models" });
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if crate::http::client().get(&probe).timeout(Duration::from_secs(2)).send().await.is_ok() {
                    break;
                }
            }
            app.hub.send(json!({ "kind": "local-ai" }));
            let mut answer = shown(app).await;
            answer["started"] = json!(started);
            Ok((StatusCode::OK, answer))
        }
        (&Method::POST, ["ollama", "pull"]) => pull(app, check_model(body["model"].as_str().unwrap_or_default())?),
        (&Method::DELETE, ["ollama", "pull"]) => {
            stop_pull(&check_model(query.as_deref().unwrap_or_default())?);
            app.hub.send(json!({ "kind": "local-ai" }));
            Ok((StatusCode::OK, json!({ "ok": true })))
        }
        (&Method::POST, ["ollama", "remove"]) => {
            let model = check_model(body["model"].as_str().unwrap_or_default())?;
            let answer = crate::http::client()
                .delete(format!("{}/api/delete", base(app, "ollama")))
                .timeout(Duration::from_secs(30))
                .json(&json!({ "model": model }))
                .send()
                .await
                .map_err(|_| (StatusCode::BAD_GATEWAY, "Ollama isn't answering".to_string()))?;
            if !answer.status().is_success() {
                let said: Value = answer.json().await.unwrap_or(Value::Null);
                return Err((StatusCode::BAD_GATEWAY, said["error"].as_str().unwrap_or("Ollama couldn't remove it").to_string()));
            }
            app.hub.send(json!({ "kind": "local-ai" }));
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, shown(app).await))
        }
        _ => Err((StatusCode::NOT_FOUND, "local AI doesn't do that".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_fits_or_doesnt() {
        assert_eq!(fit(6.0, Some(8.0), Some(32.0)), "fits");
        assert_eq!(fit(14.0, Some(8.0), Some(32.0)), "partly");
        assert_eq!(fit(40.0, Some(8.0), Some(32.0)), "too big");
        assert_eq!(fit(6.0, None, Some(16.0)), "slow");
        assert_eq!(fit(6.0, None, None), "unknown");
    }

    #[test]
    fn model_names_are_checked() {
        assert_eq!(check_model(" qwen3.5:9b ").unwrap(), "qwen3.5:9b");
        assert!(check_model("hf.co/someone/model:Q4_K_M").is_ok());
        assert!(check_model("bad name; rm -rf").is_err());
    }
}
