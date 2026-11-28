//! ComfyUI (stage 26): bots make pictures and videos with the ComfyUI running on this computer.
//!
//! Moo's choices (2026-11-25): ComfyUI on his home PC (127.0.0.1:8188 unless changed); bots may
//! see its models, nodes and queue, run his saved workflows (changing their prompts, seeds, sizes
//! and input pictures), and build workflows of their own; every run asks him first, on a card
//! that shows what will run; what comes out goes into the chat, ComfyUI is one of the choices
//! `/image` offers, and bots can hand its pictures to Blender (by their path).
//!
//! Saved workflows are read from ComfyUI's own user folder (`/userdata`, where its editor saves
//! them) and turned into the form ComfyUI runs (`comfy_graph.rs`). On approval the job is queued
//! and Catbot waits for it in the background, so a long job doesn't hang on an open request; the
//! results come into the chat and the bot hears where they were saved.
//!
//! Kept in the settings as `comfyui`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::bot_tools::Grant;
use super::comfy_graph;
use super::{Answer, App, bad, turns};
use crate::store::{Bot, Role};

const SETTING: &str = "comfyui";
pub const DEFAULT_URL: &str = "http://127.0.0.1:8188";
const TOOLS: [&str; 7] = ["comfy_status", "comfy_models", "comfy_nodes", "comfy_workflows", "comfy_run", "comfy_cancel", "comfy_result"];
/// What a bot is shown of an answer.
const MOST_ANSWER: usize = 30_000;
/// Longest a job is waited for.
const LONGEST_JOB: Duration = Duration::from_secs(60 * 60);
/// Most files kept from one job.
const MOST_FILES: usize = 24;
/// Inputs that hold a seed, set afresh for each `/image` picture.
const SEEDS: [&str; 2] = ["seed", "noise_seed"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComfyUi {
    pub enabled: bool,
    pub url: String,
    /// The bots that get ComfyUI (none until the person ticks them).
    pub bots: Vec<String>,
    /// The saved workflow `/image` runs, and the "node.input" its picture's description goes in
    /// (empty: the first prompt that isn't a negative one).
    pub image_workflow: String,
    pub image_prompt: String,
}

impl Default for ComfyUi {
    fn default() -> Self {
        ComfyUi { enabled: false, url: DEFAULT_URL.into(), bots: Vec::new(), image_workflow: String::new(), image_prompt: String::new() }
    }
}

pub fn settings(app: &App) -> ComfyUi {
    app.with_store(|store| Ok(store.setting(SETTING)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())).unwrap_or_default()
}

fn keep(app: &App, c: &ComfyUi) {
    app.with_store(|store| store.set_setting(SETTING, &json!(c)));
    app.hub.send(json!({ "kind": "apps" }));
}

// ---- Talking to ComfyUI ----------------------------------------------------------------------

const MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 100 * 1024 * 1024;

fn not_open(url: &str) -> String {
    format!("ComfyUI isn't answering at {url}: is it running?")
}

async fn get(c: &ComfyUi, path: &str) -> Result<Value, String> {
    let resp = crate::http::client().get(format!("{}{path}", c.url)).timeout(Duration::from_secs(60)).send().await.map_err(|_| not_open(&c.url))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(Value::Null);
    }
    if !status.is_success() {
        return Err(format!("ComfyUI said {status}: {}", text.chars().take(300).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|_| "ComfyUI's answer didn't read".to_string())
}

async fn post(c: &ComfyUi, path: &str, body: &Value) -> Result<Value, String> {
    let resp = crate::http::client().post(format!("{}{path}", c.url)).timeout(Duration::from_secs(60)).json(body).send().await.map_err(|_| not_open(&c.url))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let found: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(refusal(&found).unwrap_or_else(|| format!("ComfyUI said {status}: {}", text.chars().take(300).collect::<String>())));
    }
    Ok(found)
}

/// Why ComfyUI refused a workflow, in words: its own message and each node's complaints.
fn refusal(found: &Value) -> Option<String> {
    let top = found["error"]["message"].as_str()?;
    let mut said = vec![top.to_string()];
    for (id, node) in found["node_errors"].as_object().into_iter().flatten() {
        for e in node["errors"].as_array().into_iter().flatten() {
            let detail = e["details"].as_str().filter(|d| !d.is_empty()).or(e["message"].as_str()).unwrap_or_default();
            said.push(format!("node {id} ({}): {detail}", node["class_type"].as_str().unwrap_or("?")));
        }
    }
    Some(said.join("; "))
}

/// `/object_info`, kept a few minutes (it's large).
type Kept = Option<(String, Instant, Arc<Value>)>;
static INFO: LazyLock<Mutex<Kept>> = LazyLock::new(Default::default);

async fn object_info(c: &ComfyUi) -> Result<Arc<Value>, String> {
    if let Some((url, at, info)) = INFO.lock().unwrap_or_else(|p| p.into_inner()).clone()
        && url == c.url
        && at.elapsed() < Duration::from_secs(300)
    {
        return Ok(info);
    }
    let info = Arc::new(get(c, "/object_info").await?);
    if !info.is_object() {
        return Err("ComfyUI didn't describe its nodes".into());
    }
    *INFO.lock().unwrap_or_else(|p| p.into_inner()) = Some((c.url.clone(), Instant::now(), info.clone()));
    Ok(info)
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// The person's saved workflows, by name ("portrait", "tests/quick").
async fn workflow_names(c: &ComfyUi) -> Result<Vec<String>, String> {
    let found = get(c, "/userdata?dir=workflows&recurse=true&split=false&full_info=false").await?;
    let mut names: Vec<String> = found
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().or(v["path"].as_str()))
        .filter(|p| p.ends_with(".json"))
        .map(|p| p.trim_end_matches(".json").to_string())
        .collect();
    names.sort();
    Ok(names)
}

/// A saved workflow, as it's saved.
async fn saved_workflow(c: &ComfyUi, name: &str) -> Result<Value, String> {
    if name.contains("..") || name.starts_with('/') {
        return Err("that isn't a workflow's name".into());
    }
    let found = get(c, &format!("/userdata/{}", encode(&format!("workflows/{name}.json")))).await?;
    if found.is_null() {
        return Err(format!("ComfyUI has no saved workflow called {name:?} (comfy_workflows lists them)"));
    }
    Ok(found)
}

/// A saved workflow in the form ComfyUI runs.
async fn runnable(c: &ComfyUi, name: &str) -> Result<Value, String> {
    let saved = saved_workflow(c, name).await?;
    let info = object_info(c).await?;
    comfy_graph::to_run_form(&saved, &info).map_err(|e| format!("{name} can't be run as it's saved: {e}"))
}

/// Verifies that `path` is a regular image file inside `bot_folder` and within size limits.
fn check_picture_path(path: &Path, bot_folder: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("give the full path of a picture in your folder".into());
    }
    let home = bot_folder.canonicalize().map_err(|_| "the bot's folder isn't readable".to_string())?;
    let real = path.canonicalize().map_err(|_| format!("there's no picture at {}", path.display()))?;
    if !real.starts_with(&home) {
        return Err("only pictures inside your own folder can be uploaded to ComfyUI".into());
    }
    let meta = std::fs::metadata(&real).map_err(|e| format!("couldn't read {}: {e}", real.display()))?;
    if !meta.is_file() {
        return Err(format!("{} isn't a file", real.display()));
    }
    if !mime_of(&real.to_string_lossy()).starts_with("image/") {
        return Err(format!("{} isn't a supported picture file", real.display()));
    }
    if meta.len() > MAX_UPLOAD_BYTES {
        return Err(format!("{} is too large to upload to ComfyUI", real.display()));
    }
    Ok(real)
}

/// Puts a picture into ComfyUI's input folder; the name to give LoadImage.
async fn upload(c: &ComfyUi, path: &Path, bot_folder: &Path) -> Result<String, String> {
    let real = check_picture_path(path, bot_folder)?;
    let bytes = tokio::fs::read(&real).await.map_err(|e| format!("couldn't read {}: {e}", real.display()))?;
    let name = real.file_name().and_then(|n| n.to_str()).unwrap_or("picture.png").replace(['"', '\r', '\n'], "_");
    let boundary = format!("catbot{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::new();
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend(&bytes);
    body.extend(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"overwrite\"\r\n\r\ntrue\r\n--{boundary}--\r\n").as_bytes());
    let resp = crate::http::client()
        .post(format!("{}/upload/image", c.url))
        .timeout(Duration::from_secs(120))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .map_err(|_| not_open(&c.url))?;
    let found: Value = resp.json().await.map_err(|_| "ComfyUI didn't take the picture".to_string())?;
    let file = found["name"].as_str().ok_or("ComfyUI didn't take the picture")?;
    Ok(match found["subfolder"].as_str().filter(|s| !s.is_empty()) {
        Some(sub) => format!("{sub}/{file}"),
        None => file.to_string(),
    })
}

/// Queues a workflow; its job id.
async fn queue(c: &ComfyUi, graph: &Value) -> Result<String, String> {
    let made = post(c, "/prompt", &json!({ "prompt": graph, "client_id": "catbot" })).await?;
    if let Some(why) = refusal(&made) {
        return Err(why);
    }
    made["prompt_id"].as_str().map(str::to_string).ok_or_else(|| "ComfyUI didn't queue it".into())
}

/// A file ComfyUI made.
#[derive(Clone, Debug)]
struct Made {
    filename: String,
    subfolder: String,
    kind: String,
}

/// Waits for a job: the files it made, or why it failed.
async fn finished(c: &ComfyUi, job: &str, longest: Duration) -> Result<Vec<Made>, String> {
    let started = Instant::now();
    let mut transient_errors = 0u32;
    let mut last_queue_check = Instant::now();
    loop {
        let history = match get(c, &format!("/history/{job}")).await {
            Ok(h) => {
                transient_errors = 0;
                h
            }
            Err(why) => {
                transient_errors += 1;
                if transient_errors >= 3 || started.elapsed() > longest {
                    return Err(why);
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        let entry = &history[job];
        let status = &entry["status"];
        if status["status_str"] == "error" {
            let why = status["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| m[0] == "execution_error")
                .map(|m| format!("{} ({}): {}", m[1]["node_type"].as_str().unwrap_or("a node"), m[1]["node_id"].as_str().unwrap_or("?"), m[1]["exception_message"].as_str().unwrap_or("it failed").trim()))
                .unwrap_or_else(|| "it failed".into());
            return Err(format!("ComfyUI couldn't finish it: {why}"));
        }
        if status["completed"] == true || status["status_str"] == "success" {
            let mut made = Vec::new();
            for out in entry["outputs"].as_object().into_iter().flat_map(|o| o.values()) {
                for list in out.as_object().into_iter().flat_map(|o| o.values()) {
                    for f in list.as_array().into_iter().flatten() {
                        if let Some(filename) = f["filename"].as_str() {
                            made.push(Made { filename: filename.into(), subfolder: f["subfolder"].as_str().unwrap_or_default().into(), kind: f["type"].as_str().unwrap_or("output").into() });
                        }
                    }
                }
            }
            // Previews only when nothing was saved.
            if made.iter().any(|m| m.kind == "output") {
                made.retain(|m| m.kind == "output");
            }
            made.truncate(MOST_FILES);
            return Ok(made);
        }
        // If the job disappeared from ComfyUI (e.g. cancelled externally or crashed without history),
        // notice instead of polling for the full hour.
        if entry.is_null() && started.elapsed() > Duration::from_secs(60) && last_queue_check.elapsed() > Duration::from_secs(30) {
            last_queue_check = Instant::now();
            if let Ok(q) = get(c, "/queue").await {
                let in_queue = q["queue_running"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(q["queue_pending"].as_array().into_iter().flatten())
                    .any(|item| item[1].as_str() == Some(job));
                if !in_queue {
                    return Err("the job is no longer in ComfyUI's queue or history (it may have been cancelled)".into());
                }
            }
        }
        if started.elapsed() > longest {
            return Err(format!("it's still going after {} minutes; Catbot stopped waiting (it may still finish in ComfyUI)", longest.as_secs() / 60));
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}

async fn fetch(c: &ComfyUi, m: &Made) -> Result<Vec<u8>, String> {
    use futures_util::StreamExt;
    let url = format!("{}/view?filename={}&subfolder={}&type={}", c.url, encode(&m.filename), encode(&m.subfolder), encode(&m.kind));
    let resp = crate::http::stream_client().get(url).timeout(Duration::from_secs(300)).send().await.map_err(|_| not_open(&c.url))?;
    if !resp.status().is_success() {
        return Err(format!("ComfyUI didn't give back {}", m.filename));
    }
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if out.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            return Err(format!("{} is too big to save (> {} MB)", m.filename, MAX_DOWNLOAD_BYTES / 1024 / 1024));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("glb") => "model/gltf-binary",
        _ => "application/octet-stream",
    }
}

fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..).map(|n| dir.join(format!("{stem} ({n}).{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Saves what a job made in the bot's folder and shows it in the chat; the saved paths.
async fn bring_home(app: &App, c: &ComfyUi, bot: &Bot, thread: &str, title: &str, made: &[Made]) -> Result<Vec<PathBuf>, String> {
    let dir = bot.folder.join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut shown = Vec::new();
    let mut paths = Vec::new();
    for m in made {
        let bytes = fetch(c, m).await?;
        let safe_name = super::attachments::clean(&m.filename);
        let path = free_path(&dir, &format!("{}-comfyui-{safe_name}", chrono::Local::now().format("%Y-%m-%d")));
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        let attachment = app.try_store(|store| store.add_attachment(&bot.id, &safe_name, &path, mime_of(&safe_name), bytes.len() as u64)).map_err(|e| e.to_string())?;
        shown.push(super::attachments::describe(&attachment));
        paths.push(path);
    }
    let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let text = if names.is_empty() {
        format!("ComfyUI finished {title}, but it saved nothing (does the workflow end in a Save node?).")
    } else {
        format!("ComfyUI finished {title}. Saved in my folder: {}", names.join(", "))
    };
    turns::save_and_send(app, thread, Role::Bot, &text, Some(&json!({ "attachments": shown, "picture": true })));
    Ok(paths)
}

fn cut(text: String) -> String {
    if text.chars().count() > MOST_ANSWER { text.chars().take(MOST_ANSWER).collect::<String>() + "\n(…cut short)" } else { text }
}

fn pretty(v: &Value) -> String {
    cut(serde_json::to_string_pretty(v).unwrap_or_default())
}

// ---- The bots' side --------------------------------------------------------------------------

fn allowed(c: &ComfyUi, bot_id: &str) -> bool {
    c.enabled && c.bots.iter().any(|b| b == bot_id)
}

pub fn knows(name: &str) -> bool {
    TOOLS.contains(&name)
}

const ASKS: &str = " The person sees exactly what will run and approves it first; what it makes comes into the chat, and you hear where it was saved.";

pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    if !allowed(&settings(app), bot_id) {
        return Vec::new();
    }
    vec![
        json!({
            "name": "comfy_status",
            "description": "ComfyUI on this computer: its version, the graphics card and free memory, and its queue (what's running and waiting, by job id). Runs at once.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "comfy_models",
            "description": "The model files ComfyUI has. With no folder, the kinds of model (checkpoints, loras, vae, diffusion_models, text_encoders, upscale_models, controlnet…); with one, its files. Runs at once.",
            "inputSchema": { "type": "object", "properties": { "folder": { "type": "string" } } },
        }),
        json!({
            "name": "comfy_nodes",
            "description": "ComfyUI's node types, for building a workflow: query finds them by name or category; name gives one node's inputs (with their kinds, defaults and choices) and outputs. Runs at once.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "name": { "type": "string" } } },
        }),
        json!({
            "name": "comfy_workflows",
            "description": "The person's saved ComfyUI workflows. With a name: what it does, node by node (its ids), and the values you can set (\"node id.input\": prompts, seeds, sizes, steps, models, input pictures). Runs at once.",
            "inputSchema": { "type": "object", "properties": { "name": { "type": "string" } } },
        }),
        json!({
            "name": "comfy_run",
            "description": format!("Run a workflow in ComfyUI: a saved one by name (workflow) with any values changed (set: {{\"6.text\": \"a red barn\", \"3.seed\": 42}}), or one you built (graph: ComfyUI's API form, node id to {{class_type, inputs}}, a wire being [node id, output index]; check node inputs with comfy_nodes). pictures puts files on this computer into LoadImage inputs: {{\"10.image\": \"/full/path.png\"}}, such as a Blender render or an earlier result. Say in why what it's for.{ASKS}"),
            "inputSchema": { "type": "object", "properties": {
                "workflow": { "type": "string" },
                "set": { "type": "object" },
                "graph": { "type": "object" },
                "pictures": { "type": "object" },
                "why": { "type": "string" },
            } },
        }),
        json!({
            "name": "comfy_cancel",
            "description": format!("Stop a ComfyUI job (running or waiting), by its job id from comfy_status.{ASKS}"),
            "inputSchema": { "type": "object", "properties": { "job": { "type": "string" } }, "required": ["job"] },
        }),
        json!({
            "name": "comfy_result",
            "description": "Look at a picture a ComfyUI job made (or any picture in your folder), by its path. Runs at once.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] },
        }),
    ]
}

/// A bot's call: `{ text, image?, isError? }` for `/api/bot-tools/call`.
pub async fn call(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Value {
    match use_comfy(app, grant, name, args).await {
        Ok((text, Some(image))) => json!({ "text": text, "image": image }),
        Ok((text, None)) => json!({ "text": text }),
        Err(text) => json!({ "text": text, "isError": true }),
    }
}

/// One node type, trimmed for a bot: its inputs' kinds, defaults and (some) choices.
fn node_summary(name: &str, def: &Value) -> Value {
    let mut inputs = Map::new();
    for section in ["required", "optional"] {
        for (input, spec) in def["input"][section].as_object().into_iter().flatten() {
            let mut shown = json!({ "kind": match &spec[0] { Value::Array(_) => json!("COMBO"), other => other.clone() }, "required": section == "required" });
            let choices = spec[0].as_array().or(spec[1]["options"].as_array());
            if let Some(choices) = choices {
                let list: Vec<Value> = choices.iter().take(40).map(|c| if c.is_object() { c["key"].clone() } else { c.clone() }).collect();
                shown["choices"] = json!(list);
                if choices.len() > 40 {
                    shown["moreChoices"] = json!(choices.len() - 40);
                }
            }
            for key in ["default", "min", "max", "tooltip", "forceInput"] {
                if let Some(v) = spec[1].get(key) {
                    shown[key] = v.clone();
                }
            }
            inputs.insert(input.clone(), shown);
        }
    }
    json!({ "name": name, "displayName": def["display_name"], "category": def["category"], "description": def["description"], "inputs": inputs, "outputs": def["output"], "outputNames": def["output_name"], "isOutput": def["output_node"] })
}

/// A workflow in the running form, node by node, for a bot or a card.
fn outline(graph: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for (id, node) in graph.as_object().into_iter().flatten() {
        let values: Vec<String> = node["inputs"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| match v.as_array() {
                Some(w) if w.len() == 2 => format!("{k} ← {}", w[0].as_str().unwrap_or("?")),
                _ => {
                    let s = match v {
                        Value::String(s) => format!("{s:?}"),
                        other => other.to_string(),
                    };
                    let s: String = if s.chars().count() > 120 { s.chars().take(120).collect::<String>() + "…" } else { s };
                    format!("{k}={s}")
                }
            })
            .collect();
        lines.push(format!("{id} {}: {}", node["class_type"].as_str().unwrap_or("?"), values.join(", ")));
    }
    lines
}

async fn use_comfy(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Result<(String, Option<Value>), String> {
    let c = settings(app);
    if !allowed(&c, &grant.bot_id) {
        return Err("ComfyUI isn't turned on for you".into());
    }
    match name {
        "comfy_status" => {
            let stats = get(&c, "/system_stats").await?;
            let queue = get(&c, "/queue").await?;
            let job = |q: &Value| json!({ "job": q[1], "nodes": q[2].as_object().map(|g| g.len()) });
            let devices: Vec<Value> = stats["devices"].as_array().into_iter().flatten().map(|d| json!({ "name": d["name"], "vramTotalMB": d["vram_total"].as_u64().map(|v| v / 1_048_576), "vramFreeMB": d["vram_free"].as_u64().map(|v| v / 1_048_576) })).collect();
            Ok((pretty(&json!({
                "comfyui": stats["system"]["comfyui_version"],
                "devices": devices,
                "running": queue["queue_running"].as_array().map(|q| q.iter().map(job).collect::<Vec<_>>()),
                "waiting": queue["queue_pending"].as_array().map(|q| q.iter().map(job).collect::<Vec<_>>()),
            })), None))
        }
        "comfy_models" => match args["folder"].as_str().map(str::trim).filter(|f| !f.is_empty()) {
            None => Ok((pretty(&get(&c, "/models").await?), None)),
            Some(folder) => {
                if !folder.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                    return Err("a folder is one word, like checkpoints or loras".into());
                }
                let found = get(&c, &format!("/models/{folder}")).await?;
                if found.is_null() {
                    return Err(format!("ComfyUI has no model folder {folder}; comfy_models with no folder lists them"));
                }
                Ok((pretty(&found), None))
            }
        },
        "comfy_nodes" => {
            let info = object_info(&c).await?;
            if let Some(n) = args["name"].as_str().filter(|n| !n.is_empty()) {
                let def = info.get(n).ok_or_else(|| format!("ComfyUI has no node {n}; find one with query"))?;
                return Ok((pretty(&node_summary(n, def)), None));
            }
            let q = args["query"].as_str().unwrap_or_default().to_lowercase();
            let found: Vec<Value> = info
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(n, d)| q.is_empty() || n.to_lowercase().contains(&q) || d["display_name"].as_str().is_some_and(|x| x.to_lowercase().contains(&q)) || d["category"].as_str().is_some_and(|x| x.to_lowercase().contains(&q)))
                .take(60)
                .map(|(n, d)| json!({ "name": n, "displayName": d["display_name"], "category": d["category"] }))
                .collect();
            Ok((pretty(&json!({ "found": found })), None))
        }
        "comfy_workflows" => match args["name"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
            None => {
                let names = workflow_names(&c).await?;
                Ok((if names.is_empty() { "ComfyUI has no saved workflows yet (they're saved from its editor, Workflow › Save).".into() } else { pretty(&json!({ "workflows": names })) }, None))
            }
            Some(n) => {
                let graph = runnable(&c, n).await?;
                Ok((pretty(&json!({ "workflow": n, "nodes": outline(&graph), "settable": comfy_graph::settable(&graph) })), None))
            }
        },
        "comfy_run" => propose_run(app, grant, &c, args).await,
        "comfy_cancel" => {
            let job = args["job"].as_str().map(str::trim).filter(|j| !j.is_empty()).ok_or("which job? (comfy_status lists them)")?;
            propose(app, grant, "stop a ComfyUI job", "ComfyUI › Stop a job", None, vec![format!("Job {job}")], json!({ "op": "cancel", "job": job }))
        }
        "comfy_result" => {
            let path = PathBuf::from(args["path"].as_str().unwrap_or_default());
            let bot = app.with_store(|store| store.bot_by_id(&grant.bot_id)).flatten().ok_or("the bot is gone")?;
            let inside = path.canonicalize().ok().zip(bot.folder.canonicalize().ok()).is_some_and(|(p, f)| p.starts_with(f));
            if !inside {
                return Err("give the full path of a picture in your folder (as ComfyUI's result said)".into());
            }
            let mime = mime_of(&path.to_string_lossy());
            if !mime.starts_with("image/") {
                return Err("that isn't a picture".into());
            }
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err("that picture is too big to look at here".into());
            }
            use base64::Engine as _;
            Ok((format!("{}.", path.display()), Some(json!({ "data": base64::engine::general_purpose::STANDARD.encode(&bytes), "mimeType": mime }))))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

/// Builds what would run (a saved workflow with its changes, or the bot's own) and shows the card.
async fn propose_run(app: &Arc<App>, grant: &Grant, c: &ComfyUi, args: &Value) -> Result<(String, Option<Value>), String> {
    let (mut graph, from) = match (args["workflow"].as_str().map(str::trim).filter(|w| !w.is_empty()), args.get("graph").filter(|g| g.is_object())) {
        (Some(_), Some(_)) => return Err("give a saved workflow or a graph, not both".into()),
        (Some(name), None) => (runnable(c, name).await?, format!("Saved workflow: {name}")),
        (None, Some(graph)) => {
            if !comfy_graph::is_run_form(graph) {
                return Err("graph is ComfyUI's API form: node id to {class_type, inputs}".into());
            }
            let info = object_info(c).await?;
            if let Some((id, n)) = graph.as_object().into_iter().flatten().find(|(_, n)| info.get(n["class_type"].as_str().unwrap_or_default()).is_none()) {
                return Err(format!("node {id}: ComfyUI has no node {} (comfy_nodes finds them)", n["class_type"].as_str().unwrap_or("?")));
            }
            (graph.clone(), format!("A workflow the bot built: {} nodes", graph.as_object().map(|g| g.len()).unwrap_or(0)))
        }
        (None, None) => return Err("give workflow (a saved one's name) or graph".into()),
    };
    let mut lines = vec![from.clone()];
    if let Some(set) = args["set"].as_object().filter(|s| !s.is_empty()) {
        for line in comfy_graph::set_values(&mut graph, set)? {
            lines.push(format!("Set {line}"));
        }
    }
    let bot = app.with_store(|store| store.bot_by_id(&grant.bot_id)).flatten().ok_or("the bot is gone")?;
    let mut uploads = Vec::new();
    for (at, path) in args["pictures"].as_object().into_iter().flatten() {
        let path = PathBuf::from(path.as_str().unwrap_or_default());
        let real = check_picture_path(&path, &bot.folder).map_err(|why| format!("{at}: {why}"))?;
        let (id, input) = at.split_once('.').ok_or_else(|| format!("{at:?} isn't \"node id.input\""))?;
        if graph.get(id).is_none() {
            return Err(format!("the workflow has no node {id}"));
        }
        lines.push(format!("Picture {} into {at}", real.display()));
        uploads.push(json!({ "node": id, "input": input, "path": real.display().to_string() }));
    }
    if args.get("graph").is_some() {
        lines.extend(outline(&graph));
    }
    let title = args["workflow"].as_str().map(|w| format!("ComfyUI › {w}")).unwrap_or_else(|| "ComfyUI › a new workflow".into());
    propose(app, grant, "run a workflow in ComfyUI", &title, args["why"].as_str(), lines, json!({ "op": "run", "graph": graph, "uploads": uploads, "title": title, "thread": grant.thread }))
}

/// Shows the person a card, and tells the bot it's waiting.
fn propose(app: &Arc<App>, grant: &Grant, wants: &str, title: &str, why: Option<&str>, mut lines: Vec<String>, mut body: Value) -> Result<(String, Option<Value>), String> {
    let bot = app.with_store(|store| store.bot_by_id(&grant.bot_id)).flatten().ok_or("the bot is gone")?;
    if let Some(why) = why.map(str::trim).filter(|w| !w.is_empty()) {
        lines.insert(0, why.chars().take(300).collect());
    }
    body["comfyui"] = json!(true);
    body["thread"] = json!(grant.thread);
    let detail = json!({
        "kind": "team-change",
        "heading": format!("{} wants to {wants}", bot.name),
        "askedBy": bot.name,
        "title": title,
        "lines": lines,
        "action": "app_call",
        "target": bot.id,
        "body": body,
        "state": "asking",
    });
    turns::save_and_send(app, &grant.thread, Role::Step, title, Some(&detail));
    Ok(("It's waiting for the person's approval in the chat; they see exactly what will run. You'll hear how it went. Don't ask again.".into(), None))
}

/// Puts the pictures in and queues the job.
async fn start(c: &ComfyUi, body: &Value, bot_folder: &Path) -> Result<String, String> {
    let mut graph = body["graph"].clone();
    for u in body["uploads"].as_array().into_iter().flatten() {
        let name = upload(c, Path::new(u["path"].as_str().unwrap_or_default()), bot_folder).await?;
        let (id, input) = (u["node"].as_str().unwrap_or_default(), u["input"].as_str().unwrap_or("image"));
        graph[id]["inputs"][input] = json!(name);
    }
    queue(c, &graph).await
}

/// Runs an approved ComfyUI call (from `apps::run_approved`): queues it, and waits for it in the
/// background.
pub async fn run_approved(app: &Arc<App>, body: &Value, bot_id: &str) -> Result<String, String> {
    let c = settings(app);
    if !c.enabled {
        return Err("ComfyUI is turned off in Catbot".into());
    }
    match body["op"].as_str().unwrap_or_default() {
        "cancel" => {
            let job = body["job"].as_str().unwrap_or_default();
            let queue = get(&c, "/queue").await?;
            let running = queue["queue_running"].as_array().into_iter().flatten().any(|q| q[1] == job);
            if running {
                post(&c, "/interrupt", &json!({ "prompt_id": job })).await?;
            } else {
                post(&c, "/queue", &json!({ "delete": [job] })).await?;
            }
            Ok(format!("Stopped job {job}."))
        }
        "run" => {
            let bot = app.with_store(|store| store.bot_by_id(bot_id)).flatten().ok_or("the bot is gone")?;
            let job = start(&c, body, &bot.folder).await?;
            let thread = body["thread"].as_str().unwrap_or_default().to_string();
            let title = body["title"].as_str().unwrap_or("the workflow").to_string();
            let app = app.clone();
            let queued = format!("Queued in ComfyUI as job {job}. What it makes comes into the chat when it's done.");
            tokio::spawn(async move {
                let said = match finished(&c, &job, LONGEST_JOB).await {
                    Ok(made) => match bring_home(&app, &c, &bot, &thread, &title, &made).await {
                        Ok(paths) if paths.is_empty() => format!("ComfyUI finished job {job} but saved nothing."),
                        Ok(paths) => format!("ComfyUI finished job {job}; saved in your folder: {}.", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
                        Err(why) => format!("ComfyUI finished job {job}, but Catbot couldn't bring the files back: {why}."),
                    },
                    Err(why) => {
                        turns::save_and_send(&app, &thread, Role::Step, &format!("ComfyUI job {job}: {why}"), None);
                        format!("ComfyUI job {job} failed: {why}.")
                    }
                };
                super::team::remember(&app, &thread, &said);
            });
            Ok(queued)
        }
        other => Err(format!("{other} isn't a ComfyUI change Catbot knows")),
    }
}

// ---- `/image` --------------------------------------------------------------------------------

/// The `/image` choice, when a workflow is picked for it.
pub fn image_option(app: &App) -> Option<(String, String)> {
    let c = settings(app);
    (c.enabled && !c.image_workflow.is_empty()).then(|| ("comfyui".to_string(), format!("ComfyUI · {}", c.image_workflow)))
}

/// Where a workflow's description goes: the setting, or the first prompt that isn't negative.
fn prompt_input(graph: &Value, chosen: &str) -> Option<String> {
    if !chosen.is_empty() {
        return Some(chosen.to_string());
    }
    let all = comfy_graph::settable(graph);
    let is_text = |s: &&Value| s["set"].as_str().is_some_and(|p| p.ends_with(".text") || p.ends_with(".prompt") || p.ends_with(".positive")) && s["now"].is_string();
    let negative = |s: &&Value| s["node"].as_str().is_some_and(|t| t.to_lowercase().contains("neg"));
    let found = all.iter().filter(is_text).find(|s| !negative(s)).or_else(|| all.iter().find(is_text));
    found.and_then(|s| s["set"].as_str().map(str::to_string))
}

/// Draws an `/image` picture with the chosen workflow: its first picture's bytes.
pub async fn draw(app: &App, prompt: &str) -> Result<Vec<u8>, String> {
    let c = settings(app);
    if !c.enabled || c.image_workflow.is_empty() {
        return Err("no ComfyUI workflow is chosen for pictures (Settings › Apps › ComfyUI)".into());
    }
    let mut graph = runnable(&c, &c.image_workflow).await?;
    let at = prompt_input(&graph, &c.image_prompt).ok_or_else(|| format!("{} has no prompt to put the description in; pick one in Settings › Apps › ComfyUI", c.image_workflow))?;
    let mut set = Map::new();
    set.insert(at, json!(prompt));
    // A new seed each time, or the same picture comes back.
    for (id, node) in graph.as_object().into_iter().flatten() {
        for seed in SEEDS {
            if node["inputs"][seed].is_number() {
                set.insert(format!("{id}.{seed}"), json!(rand_seed()));
            }
        }
    }
    comfy_graph::set_values(&mut graph, &set)?;
    let job = queue(&c, &graph).await?;
    let made = finished(&c, &job, LONGEST_JOB).await?;
    let first = made.iter().find(|m| mime_of(&m.filename).starts_with("image/")).ok_or("the workflow saved no picture")?;
    fetch(&c, first).await
}

fn rand_seed() -> u64 {
    u64::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap_or_default()) >> 14
}

// ---- The person's side: Settings › Apps › ComfyUI ----------------------------------------------

async fn shown(app: &App, c: &ComfyUi) -> Value {
    let names: Vec<String> = c.bots.iter().filter_map(|id| app.with_store(|store| store.bot_by_id(id)).flatten().map(|b| b.name)).collect();
    let (version, devices, problem) = match get(c, "/system_stats").await {
        Ok(stats) => (stats["system"]["comfyui_version"].clone(), stats["devices"].as_array().map(|d| d.iter().map(|x| x["name"].clone()).collect::<Vec<_>>()).unwrap_or_default(), None),
        Err(why) => (Value::Null, Vec::new(), Some(why)),
    };
    let workflows = if problem.is_none() { workflow_names(c).await.unwrap_or_default() } else { Vec::new() };
    let (prompts, prompt_problem) = if problem.is_none() && !c.image_workflow.is_empty() {
        match runnable(c, &c.image_workflow).await {
            Ok(graph) => (comfy_graph::settable(&graph).into_iter().filter(|s| s["now"].is_string()).collect::<Vec<_>>(), None),
            Err(why) => (Vec::new(), Some(why)),
        }
    } else {
        (Vec::new(), None)
    };
    json!({
        "enabled": c.enabled,
        "url": c.url,
        "bots": c.bots,
        "botNames": names,
        "open": version.is_string(),
        "version": version,
        "devices": devices,
        "problem": problem,
        "workflows": workflows,
        "imageWorkflow": c.image_workflow,
        "imagePrompt": c.image_prompt,
        "prompts": prompts,
        "promptProblem": prompt_problem,
    })
}

/// `GET`/`PUT /api/comfyui` `{enabled, url, bots, imageWorkflow, imagePrompt}`.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or_default();
    let mut c = settings(app);
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, shown(app, &c).await)),
        (&Method::PUT, []) => {
            if let Some(on) = body["enabled"].as_bool() {
                c.enabled = on;
            }
            if let Some(url) = body["url"].as_str() {
                let url = url.trim().trim_end_matches('/');
                let url = if url.is_empty() { DEFAULT_URL } else { url };
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(bad("ComfyUI's address starts http://, like http://127.0.0.1:8188"));
                }
                c.url = url.to_string();
            }
            if let Some(bots) = body["bots"].as_array() {
                c.bots = bots.iter().filter_map(|b| b.as_str().map(str::to_string)).collect();
            }
            if let Some(w) = body["imageWorkflow"].as_str() {
                if w != c.image_workflow {
                    c.image_prompt.clear();
                }
                c.image_workflow = w.to_string();
            }
            if let Some(p) = body["imagePrompt"].as_str() {
                c.image_prompt = p.to_string();
            }
            keep(app, &c);
            Ok((StatusCode::OK, shown(app, &c).await))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such ComfyUI route".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_found() {
        let graph = json!({
            "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "neg stuff" }, "_meta": { "title": "Negative" } },
            "7": { "class_type": "CLIPTextEncode", "inputs": { "text": "a cat" }, "_meta": { "title": "Positive" } },
        });
        assert_eq!(prompt_input(&graph, "").as_deref(), Some("7.text"));
        assert_eq!(prompt_input(&graph, "6.text").as_deref(), Some("6.text"));
    }

    #[test]
    fn names_and_refusals_read() {
        assert_eq!(encode("workflows/my flow.json"), "workflows%2Fmy%20flow.json");
        let said = refusal(&json!({ "error": { "message": "Prompt outputs failed validation" }, "node_errors": { "3": { "class_type": "KSampler", "errors": [{ "message": "Required input is missing", "details": "model" }] } } })).unwrap();
        assert_eq!(said, "Prompt outputs failed validation; node 3 (KSampler): model");
        assert_eq!(mime_of("a.PNG"), "image/png");
        assert!(rand_seed() < (1u64 << 50));
    }
}
