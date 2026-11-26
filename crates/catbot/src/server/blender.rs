//! Blender (stage 25): bots see and work in the Blender open on this computer, through Catbot's
//! own add-on (`crates/catbot/blender/catbot_bridge.py`).
//!
//! Moo's choices (2026-11-25): an add-on of Catbot's own rather than someone else's Blender MCP;
//! the person installs it from Settings › Apps › Blender and turns it on once in Blender; on his
//! home Kubuntu and his Windows work PC alike. Bots get all four kinds of tool: looking (the
//! scene, an object, a picture of the viewport or the last render), making and changing objects,
//! running Python in Blender, and rendering and saving. As for every app, reading runs at once
//! and the rest shows the person a card with exactly what would be done (a script in full) and
//! runs when they approve it.
//!
//! The add-on listens on 127.0.0.1 only and answers only requests carrying the key Catbot wrote
//! beside it (`catbot_bridge.json`). One request per connection: a line of JSON in, one out.
//! Kept in the settings as `blender`. `CATBOT_BLENDER_CONFIG` stands in for Blender's settings
//! folder in tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::bot_tools::Grant;
use super::{Answer, App, bad};
use crate::store::Role;

const SETTING: &str = "blender";
/// The add-on, as Catbot installs it.
pub const ADDON: &str = include_str!("../../blender/catbot_bridge.py");
const ADDON_FILE: &str = "catbot_bridge.py";
const CONFIG_FILE: &str = "catbot_bridge.json";
/// The add-on's version this Catbot carries (its `BRIDGE_VERSION`).
const BRIDGE_VERSION: u64 = 2;
pub const DEFAULT_PORT: u16 = 9871;
/// The longest script a card shows, so the person can read all of it.
const LONGEST_SCRIPT: usize = 12_000;
const MOST_STEPS: usize = 100;
/// What a bot is shown of an answer.
const MOST_ANSWER: usize = 30_000;
const EXPORTS: [&str; 6] = ["glb", "gltf", "fbx", "obj", "stl", "usd"];

const TOOLS: [&str; 7] = ["blender_scene", "blender_object", "blender_look", "blender_change", "blender_python", "blender_render", "blender_save"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Blender {
    pub enabled: bool,
    /// The bots that get Blender (none until the person ticks them).
    pub bots: Vec<String>,
    pub port: u16,
    /// Written beside the add-on; never goes to a bot.
    pub key: String,
    /// Where the add-on was put (Blender's version folders).
    pub installed: Vec<String>,
}

impl Default for Blender {
    fn default() -> Self {
        Blender { enabled: false, bots: Vec::new(), port: DEFAULT_PORT, key: String::new(), installed: Vec::new() }
    }
}

pub fn settings(app: &App) -> Blender {
    app.with_store(|store| Ok(store.setting(SETTING)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())).unwrap_or_default()
}

fn keep(app: &App, b: &Blender) {
    app.with_store(|store| store.set_setting(SETTING, &json!(b)));
    app.hub.send(json!({ "kind": "apps" }));
}

/// Catbot's own Blender folder: renders go in `renders`, saved files with no path in `files`.
fn folder(app: &App, part: &str) -> PathBuf {
    app.with_store(|store| Ok(store.home().to_path_buf())).unwrap_or_else(std::env::temp_dir).join("blender").join(part)
}

// ---- Finding Blender's settings folders ------------------------------------------------------

/// The folders Blender keeps its settings in on this computer (each holds a folder per version).
fn config_roots() -> Vec<PathBuf> {
    if let Ok(given) = std::env::var("CATBOT_BLENDER_CONFIG") {
        return vec![PathBuf::from(given)];
    }
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Ok(appdata) = std::env::var("APPDATA") {
            out.push(Path::new(&appdata).join("Blender Foundation").join("Blender"));
        }
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if cfg!(target_os = "macos") {
            out.push(home.join("Library/Application Support/Blender"));
        } else {
            let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
            out.push(config.join("blender"));
            // Flatpak and Snap keep their own.
            out.push(home.join(".var/app/org.blender.Blender/config/blender"));
            out.push(home.join("snap/blender/current/.config/blender"));
        }
    }
    out
}

/// "4.2", "5.3": a Blender version folder's name.
fn is_version(name: &str) -> bool {
    let mut parts = name.split('.');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else { return false };
    !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit())
}

fn version_key(name: &str) -> (u32, u32) {
    let mut parts = name.split('.').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// Blender's version folders here, newest first.
fn version_folders() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = config_roots()
        .into_iter()
        .filter_map(|root| std::fs::read_dir(root).ok())
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name().to_str().is_some_and(is_version))
        .map(|e| e.path())
        .collect();
    found.sort_by_key(|p| std::cmp::Reverse(version_key(p.file_name().and_then(|n| n.to_str()).unwrap_or(""))));
    found
}

fn addons_folder(version: &Path) -> PathBuf {
    version.join("scripts").join("addons")
}

/// Puts the add-on and its key into every Blender version folder here.
fn install(b: &mut Blender) -> Result<Vec<String>, String> {
    let versions = version_folders();
    if versions.is_empty() {
        return Err("Catbot can't find Blender's settings folder. Open Blender once (so it makes it), close it, and press Install again".into());
    }
    if b.key.len() < 32 {
        b.key = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    }
    let config = serde_json::to_string_pretty(&json!({ "port": b.port, "key": b.key })).unwrap_or_default();
    let mut put = Vec::new();
    for version in versions {
        let addons = addons_folder(&version);
        std::fs::create_dir_all(&addons).map_err(|e| format!("couldn't make {}: {e}", addons.display()))?;
        std::fs::write(addons.join(ADDON_FILE), ADDON).map_err(|e| format!("couldn't write the add-on into {}: {e}", addons.display()))?;
        let key_file = addons.join(CONFIG_FILE);
        std::fs::write(&key_file, &config).map_err(|e| format!("couldn't write {}: {e}", key_file.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600));
        }
        put.push(version.display().to_string());
    }
    b.installed = put.clone();
    Ok(put)
}

/// Takes the add-on out of every folder it was put in.
fn uninstall(b: &mut Blender) {
    for version in &b.installed {
        let addons = addons_folder(Path::new(version));
        let _ = std::fs::remove_file(addons.join(ADDON_FILE));
        let _ = std::fs::remove_file(addons.join(CONFIG_FILE));
    }
    b.installed.clear();
}

// ---- Talking to the add-on -------------------------------------------------------------------

fn not_open() -> String {
    "Blender isn't open, or the Catbot Bridge add-on isn't turned on in it (Edit › Preferences › Add-ons › Catbot Bridge)".into()
}

/// Asks the add-on for `op`; its result, or what went wrong in words.
async fn ask(b: &Blender, op: &str, args: Value, wait: Duration) -> Result<Value, String> {
    if b.key.is_empty() {
        return Err("the Blender add-on isn't installed yet (Settings › Apps › Blender)".into());
    }
    let talk = async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", b.port)).await.map_err(|_| not_open())?;
        let mut line = serde_json::to_vec(&json!({ "key": b.key, "op": op, "args": args })).map_err(|e| e.to_string())?;
        line.push(b'\n');
        stream.write_all(&line).await.map_err(|e| format!("couldn't reach Blender: {e}"))?;
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).await.map_err(|e| format!("Blender's answer didn't come: {e}"))?;
        if answer.trim().is_empty() {
            return Err("Blender closed the connection without answering (is something else using its port?)".into());
        }
        serde_json::from_str::<Value>(&answer).map_err(|_| "Blender's answer didn't read (is something else using its port?)".to_string())
    };
    let reply = tokio::time::timeout(wait, talk).await.map_err(|_| format!("Blender didn't answer within {} seconds", wait.as_secs()))??;
    if reply["ok"] == true {
        return Ok(reply["result"].clone());
    }
    let why = reply["error"].as_str().unwrap_or("it failed").to_string();
    if why == "wrong key" {
        return Err("Blender's add-on has a different key from Catbot's: press Install again in Settings › Apps › Blender, then turn the add-on off and on in Blender".into());
    }
    Err(why)
}

const QUICK: Duration = Duration::from_secs(30);
const CHANGE: Duration = Duration::from_secs(5 * 60);
const SLOW: Duration = Duration::from_secs(60 * 60);

fn cut(text: String) -> String {
    if text.chars().count() > MOST_ANSWER { text.chars().take(MOST_ANSWER).collect::<String>() + "\n(…cut short)" } else { text }
}

// ---- The bots' side --------------------------------------------------------------------------

fn allowed(b: &Blender, bot_id: &str) -> bool {
    b.enabled && !b.key.is_empty() && b.bots.iter().any(|x| x == bot_id)
}

pub fn knows(name: &str) -> bool {
    TOOLS.contains(&name)
}

const ASKS: &str = " The person sees exactly what you asked for and approves it first; you hear how it went with their next message.";

pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    if !allowed(&settings(app), bot_id) {
        return Vec::new();
    }
    let vec3 = json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 });
    let step = json!({
        "type": "object",
        "description": "One change. action is one of: add (kind: cube, sphere, ico_sphere, cylinder, cone, plane, torus, monkey, empty, camera, light, text, image; with name, size, location, rotation in degrees, scale, collection; a light takes light: point/sun/spot/area, power, color; a camera takes lens, active: true; text takes text; image is a flat card showing the picture at path, a full path such as a ComfyUI result), transform (name, location, rotation, scale), delete (name), rename (name, to), duplicate (name, to, linked, location…), material (object, name, color as RGB(A) 0–1, texture: a picture's full path, metallic, roughness, emission, emissionStrength), parent (name, parent, or none to unparent), hide (name, hidden, render), frame (start, end, current), keyframe (name, at, what: data paths, location…), select (names).",
        "properties": {
            "action": { "type": "string", "enum": ["add", "transform", "delete", "rename", "duplicate", "material", "parent", "hide", "frame", "keyframe", "select"] },
            "kind": { "type": "string" },
            "name": { "type": "string" },
            "location": vec3,
            "rotation": vec3,
        },
        "required": ["action"],
    });
    vec![
        json!({
            "name": "blender_scene",
            "description": "What's open in Blender on this computer: the file, the scene's settings (frames, engine, resolution, camera) and its objects with where they are and their materials. Runs at once.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "blender_object",
            "description": "One Blender object in detail: transforms, size, bounds, mesh counts, modifiers, materials, light or camera settings. Runs at once.",
            "inputSchema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] },
        }),
        json!({
            "name": "blender_look",
            "description": "A picture: what Blender's 3D viewport shows now (what: viewport), or the last picture you rendered (what: last_render). Runs at once.",
            "inputSchema": { "type": "object", "properties": { "what": { "type": "string", "enum": ["viewport", "last_render"] } } },
        }),
        json!({
            "name": "blender_change",
            "description": format!("Make and change objects in Blender: a list of steps done in order, as one change the person can undo with Ctrl+Z. Up to {MOST_STEPS} steps. Say in why what it's for.{ASKS}"),
            "inputSchema": { "type": "object", "properties": {
                "steps": { "type": "array", "items": step },
                "why": { "type": "string" },
            }, "required": ["steps"] },
        }),
        json!({
            "name": "blender_python",
            "description": format!("Run a Python script in Blender (bpy, mathutils, C = context, D = data are ready). print() and a variable named result come back to you. For what blender_change can't do. Under {LONGEST_SCRIPT} characters, written plainly so the person can read it.{ASKS}"),
            "inputSchema": { "type": "object", "properties": {
                "code": { "type": "string" },
                "why": { "type": "string" },
            }, "required": ["code"] },
        }),
        json!({
            "name": "blender_render",
            "description": format!("Render a picture from the scene's camera with its own render settings (Catbot keeps the picture; see it with blender_look what: last_render). frame picks the frame; percent shrinks it (e.g. 50 for a quick look).{ASKS}"),
            "inputSchema": { "type": "object", "properties": {
                "frame": { "type": "integer" },
                "percent": { "type": "integer", "minimum": 1, "maximum": 100 },
            } },
        }),
        json!({
            "name": "blender_save",
            "description": format!("Save the .blend (kind: blend; with no path it saves where it already is), or export to glb, gltf, fbx, obj, stl or usd. path is a full path on this computer; with none, Catbot picks one in its Blender folder. selectedOnly exports only what's selected.{ASKS}"),
            "inputSchema": { "type": "object", "properties": {
                "kind": { "type": "string", "enum": ["blend", "glb", "gltf", "fbx", "obj", "stl", "usd"] },
                "path": { "type": "string" },
                "selectedOnly": { "type": "boolean" },
            }, "required": ["kind"] },
        }),
    ]
}

/// The newest picture in Catbot's renders folder.
fn last_render(app: &App) -> Option<PathBuf> {
    std::fs::read_dir(folder(app, "renders"))
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
}

/// A bot's call: `{ text, image?, isError? }` for `/api/bot-tools/call`.
pub async fn call(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Value {
    match use_blender(app, grant, name, args).await {
        Ok((text, Some(image))) => json!({ "text": text, "image": image }),
        Ok((text, None)) => json!({ "text": text }),
        Err(text) => json!({ "text": text, "isError": true }),
    }
}

async fn use_blender(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Result<(String, Option<Value>), String> {
    let b = settings(app);
    if !allowed(&b, &grant.bot_id) {
        return Err("Blender isn't turned on for you".into());
    }
    let pretty = |v: &Value| cut(serde_json::to_string_pretty(v).unwrap_or_default());
    match name {
        "blender_scene" => ask(&b, "scene", json!({}), QUICK).await.map(|v| (pretty(&v), None)),
        "blender_object" => ask(&b, "object", json!({ "name": args["name"] }), QUICK).await.map(|v| (pretty(&v), None)),
        "blender_look" => {
            let what = args["what"].as_str().unwrap_or("viewport");
            let (asked, said) = if what == "last_render" {
                let path = last_render(app).ok_or("nothing has been rendered yet (blender_render)")?;
                (json!({ "path": path.display().to_string() }), format!("The last render ({}).", path.display()))
            } else {
                (json!({}), "Blender's 3D viewport now.".to_string())
            };
            let pic = ask(&b, "look", asked, QUICK).await.map_err(|e| if what == "viewport" { format!("couldn't take a picture of the viewport: {e}") } else { e })?;
            Ok((said, Some(json!({ "data": pic["data"], "mimeType": pic["mimeType"] }))))
        }
        "blender_change" => {
            let steps = args["steps"].as_array().filter(|s| !s.is_empty()).ok_or("give steps: a list of changes")?;
            if steps.len() > MOST_STEPS {
                return Err(format!("at most {MOST_STEPS} steps at once"));
            }
            let lines: Vec<String> = steps.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, describe_step(s))).collect();
            propose(app, grant, "change things in Blender", &format!("Blender › {} change{}", steps.len(), if steps.len() == 1 { "" } else { "s" }), args["why"].as_str(), lines, json!({ "op": "change", "args": { "steps": steps } }))
        }
        "blender_python" => {
            let code = args["code"].as_str().unwrap_or_default();
            if code.trim().is_empty() {
                return Err("there's no code".into());
            }
            if code.chars().count() > LONGEST_SCRIPT {
                return Err(format!("keep a script under {LONGEST_SCRIPT} characters so the person can read it; split it up"));
            }
            propose(app, grant, "run a script in Blender", "Blender › Python script", args["why"].as_str(), vec![code.to_string()], json!({ "op": "python", "args": { "code": code } }))
        }
        "blender_render" => {
            let path = folder(app, "renders").join(format!("{}.png", stamp()));
            let mut asked = json!({ "path": path.display().to_string() });
            let mut lines = vec![format!("Saved to {}", path.display())];
            if let Some(f) = args["frame"].as_i64() {
                asked["frame"] = json!(f);
                lines.push(format!("Frame {f}"));
            }
            if let Some(p) = args["percent"].as_i64() {
                asked["percent"] = json!(p.clamp(1, 100));
                lines.push(format!("At {}% size", p.clamp(1, 100)));
            }
            propose(app, grant, "render in Blender", "Blender › Render", None, lines, json!({ "op": "render", "args": asked }))
        }
        "blender_save" => {
            let kind = args["kind"].as_str().unwrap_or("blend").to_lowercase();
            if kind != "blend" && !EXPORTS.contains(&kind.as_str()) {
                return Err(format!("kind is blend or one of {}", EXPORTS.join(", ")));
            }
            let given = args["path"].as_str().map(str::trim).filter(|p| !p.is_empty());
            let path = match given {
                Some(p) => {
                    let p = PathBuf::from(p);
                    if !p.is_absolute() {
                        return Err("give a full path (from the top of the drive), or none".into());
                    }
                    if p.extension().and_then(|x| x.to_str()).map(str::to_lowercase).as_deref() != Some(kind.as_str()) {
                        return Err(format!("a {kind} file's name ends .{kind}"));
                    }
                    Some(p)
                }
                // A .blend with no path saves where it is; an export goes in Catbot's folder.
                None if kind == "blend" => None,
                None => Some(folder(app, "files").join(format!("{}.{kind}", stamp()))),
            };
            let mut asked = json!({ "kind": kind });
            if let Some(p) = &path {
                asked["path"] = json!(p.display().to_string());
            }
            if args["selectedOnly"] == true {
                asked["selectedOnly"] = json!(true);
            }
            let what = if kind == "blend" { "Save the .blend".to_string() } else { format!("Export as {kind}") };
            let mut lines = vec![match &path {
                Some(p) => format!("{what} to {}", p.display()),
                None => format!("{what} where it already is"),
            }];
            if args["selectedOnly"] == true {
                lines.push("Only what's selected".into());
            }
            if path.as_ref().is_some_and(|p| p.exists()) {
                lines.push("That file is there already and will be replaced".into());
            }
            propose(app, grant, if kind == "blend" { "save in Blender" } else { "export from Blender" }, &format!("Blender › {what}"), None, lines, json!({ "op": "save", "args": asked }))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

/// One change, in words, for the card.
fn describe_step(s: &Value) -> String {
    let action = s["action"].as_str().unwrap_or("?");
    let mut rest = s.as_object().cloned().unwrap_or_default();
    rest.remove("action");
    let rest = serde_json::to_string(&Value::Object(rest)).unwrap_or_default();
    let rest: String = if rest.chars().count() > 400 { rest.chars().take(400).collect::<String>() + "…" } else { rest };
    format!("{action} {rest}")
}

/// Shows the person a card, and tells the bot it's waiting.
fn propose(app: &Arc<App>, grant: &Grant, wants: &str, title: &str, why: Option<&str>, mut lines: Vec<String>, body: Value) -> Result<(String, Option<Value>), String> {
    let bot = app.with_store(|store| store.bot_by_id(&grant.bot_id)).flatten().ok_or("the bot is gone")?;
    if let Some(why) = why.map(str::trim).filter(|w| !w.is_empty()) {
        lines.insert(0, why.chars().take(300).collect());
    }
    let mut body = body;
    body["blender"] = json!(true);
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
    super::turns::save_and_send(app, &grant.thread, Role::Step, title, Some(&detail));
    Ok(("It's waiting for the person's approval in the chat; they see exactly what you asked for. You'll hear how it went with their next message. Don't ask again.".into(), None))
}

/// Runs an approved Blender call (from `apps::run_approved`); what came of it, for the card.
pub async fn run_approved(app: &App, body: &Value) -> Result<String, String> {
    let b = settings(app);
    if !b.enabled {
        return Err("Blender is turned off in Catbot".into());
    }
    let op = body["op"].as_str().unwrap_or_default();
    if !matches!(op, "change" | "python" | "render" | "save") {
        return Err(format!("{op} isn't a Blender change Catbot knows"));
    }
    let wait = if op == "render" { SLOW } else { CHANGE };
    let done = ask(&b, op, body["args"].clone(), wait).await?;
    let short = |s: String| -> String { if s.chars().count() > 1500 { s.chars().take(1500).collect::<String>() + "…" } else { s } };
    Ok(match op {
        "change" => short(format!("Done: {}.", done["done"].as_array().map(|d| d.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("; ")).unwrap_or_default())),
        "python" => {
            let printed = done["printed"].as_str().unwrap_or_default().trim().to_string();
            let mut said = String::from("Ran.");
            if !printed.is_empty() {
                said.push_str(&format!(" It printed: {printed}"));
            }
            if !done["result"].is_null() {
                said.push_str(&format!(" result = {}", done["result"]));
            }
            short(said)
        }
        "render" => format!("Rendered in {} seconds ({}) to {}. See it with blender_look (what: last_render).", done["seconds"], done["engine"].as_str().unwrap_or("?"), done["path"].as_str().unwrap_or("?")),
        _ => format!("Saved to {}.", done["saved"].as_str().or(done["exported"].as_str()).unwrap_or("?")),
    })
}

// ---- The person's side: Settings › Apps › Blender ----------------------------------------------

async fn shown(app: &App, b: &Blender) -> Value {
    let names: Vec<String> = b.bots.iter().filter_map(|id| app.with_store(|store| store.bot_by_id(id)).flatten().map(|x| x.name)).collect();
    let found: Vec<String> = version_folders().iter().map(|p| p.display().to_string()).collect();
    let (open, problem) = if b.key.is_empty() {
        (Value::Null, None)
    } else {
        match ask(b, "info", json!({}), Duration::from_secs(3)).await {
            Ok(info) => (info, None),
            Err(why) => (Value::Null, Some(why)),
        }
    };
    let outdated = open["bridge"].as_u64().is_some_and(|v| v < BRIDGE_VERSION);
    json!({
        "enabled": b.enabled,
        "bots": b.bots,
        "botNames": names,
        "port": b.port,
        "installed": b.installed,
        "found": found,
        "open": !open.is_null(),
        "blender": open["blender"],
        "file": open["file"],
        "outdated": outdated,
        "problem": problem,
        "renders": folder(app, "renders").display().to_string(),
        "files": folder(app, "files").display().to_string(),
    })
}

/// `GET`/`PUT /api/blender` `{enabled, bots, port}`, `POST /api/blender/install`, `DELETE
/// /api/blender/install`.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or_default();
    let mut b = settings(app);
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, shown(app, &b).await)),
        (&Method::PUT, []) => {
            if let Some(on) = body["enabled"].as_bool() {
                b.enabled = on;
            }
            if let Some(bots) = body["bots"].as_array() {
                b.bots = bots.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            }
            if let Some(port) = body["port"].as_u64() {
                let port = u16::try_from(port).ok().filter(|p| *p >= 1024).ok_or_else(|| bad("a port from 1024 to 65535"))?;
                if port != b.port {
                    b.port = port;
                    // The add-on reads its port from the file beside it.
                    if !b.installed.is_empty() {
                        install(&mut b).map_err(bad)?;
                    }
                }
            }
            keep(app, &b);
            Ok((StatusCode::OK, shown(app, &b).await))
        }
        (&Method::POST, ["install"]) => {
            install(&mut b).map_err(bad)?;
            b.enabled = true;
            keep(app, &b);
            Ok((StatusCode::OK, shown(app, &b).await))
        }
        (&Method::DELETE, ["install"]) => {
            uninstall(&mut b);
            b.enabled = false;
            b.key.clear();
            keep(app, &b);
            Ok((StatusCode::OK, shown(app, &b).await))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such Blender route".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_folders_are_told_apart() {
        assert!(is_version("5.3"));
        assert!(is_version("4.10"));
        assert!(!is_version("config"));
        assert!(!is_version("5"));
        assert!(!is_version("5.3.1"));
        assert!(version_key("4.10") > version_key("4.2"));
    }

    #[test]
    fn the_addon_matches_this_version() {
        assert!(ADDON.contains(&format!("BRIDGE_VERSION = {BRIDGE_VERSION}")));
        assert!(ADDON.contains("\"127.0.0.1\""));
    }
}
