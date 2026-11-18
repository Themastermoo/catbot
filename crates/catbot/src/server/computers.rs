//! A computer for each bot (stage 17.1): a small Linux desktop with a browser, in a container on
//! this computer (Podman or Docker), that the bot uses as its own.
//!
//! Moo's choices (2026-11-17): both kinds, a container here by default and a Boat cloud computer
//! when he chooses (stage 17.2); and "anything inside its box" is free: running commands,
//! clicking, typing and browsing on its own computer never asks. Sending, posting and using his
//! apps still ask, as before. Nothing of his is put inside: no folder of his is mounted.
//!
//! The image is built from `computer/Dockerfile` the first time a computer starts (a few
//! minutes). Each bot's container keeps its files between starts; Reset throws it away. A
//! computer nobody has used or watched for a while is stopped (`CATBOT_COMPUTER_IDLE_MINUTES`,
//! 30 unless set; 0 never).
//!
//! The person watches (or takes over) through noVNC. Its files come from the image, copied out
//! once when it's built, and Catbot serves them itself at `/computer/view/`: nothing a bot does
//! inside its box can change the page that runs at Catbot's address. Only the screen's
//! websocket (`/computer/<bot>/websockify`) is carried to the box.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bot_tools::Grant;
use super::{Answer, App, bad};

const SETTING: &str = "computers";
/// The image's name; the number goes up when the Dockerfile changes.
pub const IMAGE: &str = "catbot-computer:1";
const DOCKERFILE: &str = include_str!("../../computer/Dockerfile");
const START: &str = include_str!("../../computer/start.sh");
/// The live view's files, copied out of the image (the number follows the image's).
const VIEW_DIR: &str = "novnc-1";
/// The screen's size.
pub const WIDTH: i64 = 1280;
pub const HEIGHT: i64 = 800;
/// How much of a command's output a bot reads back, from the end.
const MOST_OUTPUT: usize = 30_000;
const TOOLS: [&str; 3] = ["computer_run", "computer_screen", "computer_open"];

/// A bot's computer, kept in the settings as `computers`: bot id → this.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    /// "container" (stage 17.2 adds "boat").
    pub kind: String,
    pub since: String,
}

pub fn entries(app: &App) -> BTreeMap<String, Entry> {
    app.with_store(|store| Ok(store.setting(SETTING)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())).unwrap_or_default()
}

fn keep(app: &App, all: &BTreeMap<String, Entry>) {
    app.with_store(|store| store.set_setting(SETTING, &json!(all)));
    changed(app);
}

fn changed(app: &App) {
    app.hub.send(json!({ "kind": "computers" }));
}

pub fn has(app: &App, bot_id: &str) -> bool {
    entries(app).contains_key(bot_id)
}

/// What Catbot knows of the computers while it runs.
#[derive(Default)]
pub struct Computers {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The image is being built, or its view copied out.
    building: bool,
    /// Why the last build failed.
    image_problem: Option<String>,
    boxes: std::collections::HashMap<String, Live>,
}

#[derive(Clone, Default)]
struct Live {
    /// "setting up", "starting" or "stopping", while that's happening.
    busy: Option<&'static str>,
    port: Option<u16>,
    problem: Option<String>,
    last_used: Option<Instant>,
    viewers: usize,
}

impl Computers {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        f(&mut self.inner.lock().expect("computers lock"))
    }

    fn with_box<T>(&self, bot_id: &str, f: impl FnOnce(&mut Live) -> T) -> T {
        self.with(|inner| f(inner.boxes.entry(bot_id.to_string()).or_default()))
    }
}

// ---- the container program ----------------------------------------------------------------

/// The program that runs containers: `CATBOT_CONTAINER` if set, else Podman, else Docker.
pub fn engine() -> Option<Vec<String>> {
    if let Ok(set) = std::env::var("CATBOT_CONTAINER") {
        let words: Vec<String> = set.split_whitespace().map(String::from).collect();
        if !words.is_empty() {
            return Some(words);
        }
    }
    ["podman", "docker"].into_iter().find(|name| on_path(name)).map(|name| vec![name.to_string()])
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

fn engine_name(words: &[String]) -> String {
    let program = words.iter().rev().find(|w| !w.starts_with('-')).cloned().unwrap_or_default();
    let base = Path::new(&program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(program);
    if base.contains("podman") {
        "Podman".into()
    } else if base.contains("docker") {
        "Docker".into()
    } else {
        base
    }
}

/// What a run of the container program gave back.
struct Ran {
    code: Option<i32>,
    out: Vec<u8>,
    err: String,
}

impl Ran {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out).trim().to_string()
    }

    fn why(&self) -> String {
        let err = self.err.trim();
        if err.is_empty() { format!("it stopped with code {}", self.code.map_or("?".into(), |c| c.to_string())) } else { last_lines(err, 6) }
    }
}

async fn run(args: &[&str], wait: Duration) -> Result<Ran, String> {
    let words = engine().ok_or("there's no Podman or Docker on this computer")?;
    let (program, leading) = words.split_first().ok_or("no container program")?;
    let mut command = tokio::process::Command::new(program);
    command.args(leading).args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(wait, command.output())
        .await
        .map_err(|_| format!("{} took too long", engine_name(&words)))?
        .map_err(|e| format!("couldn't run {program}: {e}"))?;
    Ok(Ran { code: output.status.code(), out: output.stdout, err: String::from_utf8_lossy(&output.stderr).into_owned() })
}

/// Runs it, and says what went wrong when it didn't work.
async fn must(args: &[&str], wait: Duration) -> Result<String, String> {
    let ran = run(args, wait).await?;
    if ran.ok() { Ok(ran.text()) } else { Err(ran.why()) }
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// A bot's container's name: `catbot-computer-<its id>`.
pub fn container(bot_id: &str) -> String {
    let id: String = bot_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    format!("catbot-computer-{}", id.to_lowercase())
}

/// Whether a container runs: None when there's no such container.
async fn running(name: &str) -> Option<bool> {
    let ran = run(&["inspect", "--format", "{{.State.Running}}", name], Duration::from_secs(20)).await.ok()?;
    ran.ok().then(|| ran.text() == "true")
}

/// The port on this computer that carries the box's screen: from `port <name> 6080/tcp`.
pub fn parse_port(text: &str) -> Option<u16> {
    text.lines().find_map(|line| line.trim().rsplit(':').next()?.trim().parse().ok())
}

// ---- the image -----------------------------------------------------------------------------

fn home(app: &App) -> Option<PathBuf> {
    app.with_store(|store| Ok(store.home().join("computer")))
}

fn view_dir(app: &App) -> Option<PathBuf> {
    home(app).map(|dir| dir.join(VIEW_DIR))
}

async fn image_ready(app: &App) -> bool {
    let view = view_dir(app).is_some_and(|dir| dir.join("vnc.html").is_file());
    view && run(&["image", "inspect", "--format", "{{.Id}}", IMAGE], Duration::from_secs(20)).await.is_ok_and(|r| r.ok())
}

/// Builds the image if it isn't there (one build at a time), and copies its live view out.
async fn ensure_image(app: &Arc<App>) -> Result<(), String> {
    loop {
        if image_ready(app).await {
            return Ok(());
        }
        let mine = app.computers.with(|inner| !std::mem::replace(&mut inner.building, true));
        if mine {
            break;
        }
        // Someone else is building it: wait for them.
        while app.computers.with(|inner| inner.building) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if let Some(problem) = app.computers.with(|inner| inner.image_problem.clone()) {
            return Err(problem);
        }
    }
    changed(app);
    let made = build(app).await;
    app.computers.with(|inner| {
        inner.building = false;
        inner.image_problem = made.as_ref().err().cloned();
    });
    changed(app);
    made
}

async fn build(app: &App) -> Result<(), String> {
    let dir = home(app).ok_or("no place for the computer's files")?;
    let has_image = run(&["image", "inspect", "--format", "{{.Id}}", IMAGE], Duration::from_secs(20)).await.is_ok_and(|r| r.ok());
    if !has_image {
        let context = dir.join("build");
        std::fs::create_dir_all(&context).map_err(|e| format!("couldn't make {}: {e}", context.display()))?;
        std::fs::write(context.join("Dockerfile"), DOCKERFILE).map_err(|e| e.to_string())?;
        std::fs::write(context.join("start.sh"), START).map_err(|e| e.to_string())?;
        let context = context.to_string_lossy().into_owned();
        must(&["build", "-t", IMAGE, &context], Duration::from_secs(45 * 60)).await.map_err(|e| format!("the computer's image didn't build: {e}"))?;
    }
    // The live view's files, out of the image (not a running box, which a bot could change).
    let copy = "catbot-computer-view";
    let _ = run(&["rm", "-f", copy], Duration::from_secs(30)).await;
    must(&["create", "--name", copy, IMAGE], Duration::from_secs(60)).await.map_err(|e| format!("couldn't open the image: {e}"))?;
    let fresh = dir.join(format!("{VIEW_DIR}.new"));
    let _ = std::fs::remove_dir_all(&fresh);
    let into = fresh.to_string_lossy().into_owned();
    let copied = must(&["cp", &format!("{copy}:/usr/share/novnc"), &into], Duration::from_secs(60)).await;
    let _ = run(&["rm", "-f", copy], Duration::from_secs(30)).await;
    copied.map_err(|e| format!("couldn't copy the live view out of the image: {e}"))?;
    let view = dir.join(VIEW_DIR);
    let _ = std::fs::remove_dir_all(&view);
    std::fs::rename(&fresh, &view).map_err(|e| e.to_string())?;
    Ok(())
}

// ---- starting and stopping -----------------------------------------------------------------

/// Starts a bot's computer, making it first if need be; waits until its screen answers.
pub async fn start(app: &Arc<App>, bot_id: &str) -> Result<u16, String> {
    // One start at a time per bot: a second waits for the first.
    loop {
        let busy = app.computers.with_box(bot_id, |live| live.busy);
        match busy {
            None => break,
            Some("stopping") => tokio::time::sleep(Duration::from_millis(300)).await,
            Some(_) => {
                while app.computers.with_box(bot_id, |live| live.busy.is_some()) {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                return match app.computers.with_box(bot_id, |live| (live.port, live.problem.clone())) {
                    (Some(port), None) => Ok(port),
                    (_, Some(problem)) => Err(problem),
                    _ => Err("it didn't start".into()),
                };
            }
        }
    }
    let setting_up = !image_ready(app).await;
    app.computers.with_box(bot_id, |live| {
        live.busy = Some(if setting_up { "setting up" } else { "starting" });
        live.problem = None;
    });
    changed(app);
    let started = bring_up(app, bot_id).await;
    app.computers.with_box(bot_id, |live| {
        live.busy = None;
        match &started {
            Ok(port) => {
                live.port = Some(*port);
                live.last_used = Some(Instant::now());
            }
            Err(problem) => {
                live.port = None;
                live.problem = Some(problem.clone());
            }
        }
    });
    changed(app);
    started
}

async fn bring_up(app: &Arc<App>, bot_id: &str) -> Result<u16, String> {
    if !has(app, bot_id) {
        return Err("this bot has no computer".into());
    }
    ensure_image(app).await?;
    let name = container(bot_id);
    match running(&name).await {
        Some(true) => {}
        Some(false) => {
            must(&["start", &name], Duration::from_secs(60)).await.map_err(|e| format!("it didn't start: {e}"))?;
        }
        None => {
            let label = format!("catbot.bot={bot_id}");
            let args = ["run", "-d", "--name", &name, "--hostname", "computer", "--label", &label, "-p", "127.0.0.1::6080", "--shm-size", "512m", IMAGE];
            must(&args, Duration::from_secs(120)).await.map_err(|e| format!("it didn't start: {e}"))?;
        }
    }
    let port = parse_port(&must(&["port", &name, "6080/tcp"], Duration::from_secs(20)).await?).ok_or("its screen has no port")?;
    // Its screen answers once the desktop is up.
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(300)).await;
            return Ok(port);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("it started, but its screen didn't answer".into())
}

pub async fn stop(app: &Arc<App>, bot_id: &str) -> Result<(), String> {
    app.computers.with_box(bot_id, |live| live.busy = Some("stopping"));
    changed(app);
    let name = container(bot_id);
    let stopped = match running(&name).await {
        Some(true) => must(&["stop", "-t", "5", &name], Duration::from_secs(60)).await.map(|_| ()),
        _ => Ok(()),
    };
    app.computers.with_box(bot_id, |live| {
        live.busy = None;
        live.port = None;
        live.problem = stopped.as_ref().err().cloned();
    });
    changed(app);
    stopped
}

/// Throws a bot's computer away, files and all; the next start makes a fresh one.
async fn reset(app: &Arc<App>, bot_id: &str) -> Result<(), String> {
    app.computers.with_box(bot_id, |live| live.busy = Some("stopping"));
    changed(app);
    let removed = must(&["rm", "-f", &container(bot_id)], Duration::from_secs(60)).await.map(|_| ());
    app.computers.with(|inner| {
        inner.boxes.remove(bot_id);
    });
    changed(app);
    removed
}

/// The minutes a computer may sit unused before it's stopped; 0 never.
fn idle_minutes() -> u64 {
    std::env::var("CATBOT_COMPUTER_IDLE_MINUTES").ok().and_then(|m| m.trim().parse().ok()).unwrap_or(30)
}

/// Stops computers nobody has used or watched for a while: checked once a minute.
pub async fn keep_tidy(app: Arc<App>) {
    let minutes = idle_minutes();
    if minutes == 0 {
        return;
    }
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        if engine().is_none() {
            continue;
        }
        for (bot_id, _) in entries(&app) {
            let (busy, viewers, last_used) = app.computers.with_box(&bot_id, |live| (live.busy, live.viewers, live.last_used));
            if busy.is_some() || viewers > 0 {
                continue;
            }
            if running(&container(&bot_id)).await != Some(true) {
                continue;
            }
            match last_used {
                // Running from before Catbot started: its time starts now.
                None => app.computers.with_box(&bot_id, |live| live.last_used = Some(Instant::now())),
                Some(at) if at.elapsed() > Duration::from_secs(minutes * 60) => {
                    let _ = stop(&app, &bot_id).await;
                }
                Some(_) => {}
            }
        }
    }
}

// ---- the bot's tools -----------------------------------------------------------------------

const ABOUT: &str = "your own computer: a Debian Linux desktop (1280×800 screen, Chromium, user `bot` with sudo). It's yours alone: none of the person's files are on it, and nothing you do on it needs asking. The person can watch it live.";

pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    if !has(app, bot_id) {
        return Vec::new();
    }
    vec![
        json!({
            "name": "computer_run",
            "description": format!("Run a shell command (bash) on {ABOUT} It starts in /home/bot; what you make there stays between runs. Output is cut to its last {MOST_OUTPUT} characters."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command, run with bash -lc" },
                    "timeout_seconds": { "type": "integer", "description": "Stop it after this many seconds (default 120, at most 1800)" },
                },
                "required": ["command"],
            },
        }),
        json!({
            "name": "computer_screen",
            "description": format!("See and use the screen of {ABOUT} Take a screenshot first, then click, type, press keys or scroll; screenshot again to see what happened. x runs 0–{} left to right, y 0–{} top to bottom.", WIDTH - 1, HEIGHT - 1),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["screenshot", "click", "double_click", "right_click", "move", "drag", "type", "key", "scroll"] },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "to_x": { "type": "integer", "description": "Where a drag ends" },
                    "to_y": { "type": "integer", "description": "Where a drag ends" },
                    "text": { "type": "string", "description": "What to type" },
                    "keys": { "type": "string", "description": "Keys to press, xdotool's names, one press per word: \"ctrl+l\", \"Return\", \"ctrl+shift+t Tab\"" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "amount": { "type": "integer", "description": "How many notches to scroll (default 3)" },
                },
                "required": ["action"],
            },
        }),
        json!({
            "name": "computer_open",
            "description": format!("Open a web address in the browser on {ABOUT}"),
            "inputSchema": {
                "type": "object",
                "properties": { "url": { "type": "string", "description": "An http or https address" } },
                "required": ["url"],
            },
        }),
    ]
}

pub fn knows(name: &str) -> bool {
    TOOLS.contains(&name)
}

/// A tool call: `{ text, image?, isError? }` for `/api/bot-tools/call`.
pub async fn call(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Value {
    match use_computer(app, grant, name, args).await {
        Ok((text, Some(jpeg))) => {
            use base64::Engine;
            json!({ "text": text, "image": { "data": base64::engine::general_purpose::STANDARD.encode(jpeg), "mimeType": "image/jpeg" } })
        }
        Ok((text, None)) => json!({ "text": text }),
        Err(text) => json!({ "text": text, "isError": true }),
    }
}

async fn use_computer(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Result<(String, Option<Vec<u8>>), String> {
    let bot_id = &grant.bot_id;
    if engine().is_none() {
        return Err("There's no Podman or Docker on the person's computer, so your computer can't run. They can install Podman (sudo apt install podman) and try again.".into());
    }
    // Checked before anything starts: a wrong call doesn't wake the computer.
    let work = plan(name, args)?;
    if !image_ready(app).await {
        let app = app.clone();
        let bot_id = bot_id.clone();
        tokio::spawn(async move {
            let _ = start(&app, &bot_id).await;
        });
        return Err("Your computer is being set up for the first time; that takes a few minutes. Try again shortly.".into());
    }
    let on = running(&container(bot_id)).await == Some(true) && app.computers.with_box(bot_id, |live| live.port.is_some() && live.busy.is_none());
    if !on {
        start(app, bot_id).await.map_err(|e| format!("Your computer didn't start: {e}"))?;
    }
    app.computers.with_box(bot_id, |live| live.last_used = Some(Instant::now()));
    let name = container(bot_id);
    match work {
        Work::Shell { command, seconds } => {
            let secs = seconds.to_string();
            let mut args = vec!["exec", "-u", "bot", "-w", "/home/bot", "-e", "DISPLAY=:1", &name, "timeout", "-k", "5", &secs, "bash", "-lc"];
            args.push(&command);
            let ran = run(&args, Duration::from_secs(seconds + 30)).await?;
            Ok((shell_report(&ran, seconds), None))
        }
        Work::Screen { args: xdotool, said } => {
            let mut args = vec!["exec", "-u", "bot", "-e", "DISPLAY=:1", &name, "xdotool"];
            args.extend(xdotool.iter().map(String::as_str));
            let ran = run(&args, Duration::from_secs(60)).await?;
            if !ran.ok() {
                return Err(format!("That didn't work: {}", ran.why()));
            }
            Ok((said, None))
        }
        Work::Screenshot => {
            let ran = run(&["exec", "-u", "bot", "-e", "DISPLAY=:1", &name, "import", "-window", "root", "-quality", "70", "jpg:-"], Duration::from_secs(30)).await?;
            if !ran.ok() || ran.out.is_empty() {
                return Err(format!("The screenshot didn't work: {}", ran.why()));
            }
            Ok((format!("The screen ({WIDTH}×{HEIGHT})."), Some(ran.out)))
        }
        Work::Open(url) => {
            let args = ["exec", "-d", "-u", "bot", "-e", "DISPLAY=:1", &name, "chromium", "--no-sandbox", "--test-type", "--no-first-run", "--no-default-browser-check", "--hide-crash-restore-bubble", &url];
            must(&args, Duration::from_secs(30)).await.map_err(|e| format!("The browser didn't open it: {e}"))?;
            Ok((format!("Opened {url} in the browser. Take a screenshot to see it."), None))
        }
    }
}

/// What a tool call does, checked.
#[derive(Debug, PartialEq)]
pub enum Work {
    Shell { command: String, seconds: u64 },
    /// xdotool's arguments, and what to tell the bot.
    Screen { args: Vec<String>, said: String },
    Screenshot,
    Open(String),
}

pub fn plan(name: &str, args: &Value) -> Result<Work, String> {
    match name {
        "computer_run" => {
            let command = args["command"].as_str().unwrap_or_default().trim().to_string();
            if command.is_empty() {
                return Err("Say what command to run.".into());
            }
            let seconds = args["timeout_seconds"].as_u64().unwrap_or(120).clamp(1, 1800);
            Ok(Work::Shell { command, seconds })
        }
        "computer_open" => {
            let url = args["url"].as_str().unwrap_or_default().trim().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) || url.chars().any(char::is_whitespace) {
                return Err("Give an http:// or https:// address.".into());
            }
            Ok(Work::Open(url))
        }
        "computer_screen" => screen(args),
        _ => Err(format!("unknown tool {name}")),
    }
}

fn point(args: &Value, x: &str, y: &str) -> Result<(i64, i64), String> {
    match (args[x].as_i64(), args[y].as_i64()) {
        (Some(px), Some(py)) if (0..WIDTH).contains(&px) && (0..HEIGHT).contains(&py) => Ok((px, py)),
        (Some(_), Some(_)) => Err(format!("({x}, {y}) is off the screen: it's {WIDTH}×{HEIGHT}.")),
        _ => Err(format!("Say where: {x} and {y}.")),
    }
}

fn screen(args: &Value) -> Result<Work, String> {
    let action = args["action"].as_str().unwrap_or_default();
    let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let at = |x: i64, y: i64| vec!["mousemove".to_string(), "--sync".into(), x.to_string(), y.to_string()];
    Ok(match action {
        "screenshot" => Work::Screenshot,
        "click" | "double_click" | "right_click" | "move" => {
            let (x, y) = point(args, "x", "y")?;
            let mut a = at(x, y);
            let said = match action {
                "click" => {
                    a.extend(s(&["click", "1"]));
                    format!("Clicked at ({x}, {y}).")
                }
                "double_click" => {
                    a.extend(s(&["click", "--repeat", "2", "--delay", "80", "1"]));
                    format!("Double-clicked at ({x}, {y}).")
                }
                "right_click" => {
                    a.extend(s(&["click", "3"]));
                    format!("Right-clicked at ({x}, {y}).")
                }
                _ => format!("Moved the pointer to ({x}, {y})."),
            };
            Work::Screen { args: a, said }
        }
        "drag" => {
            let (x, y) = point(args, "x", "y")?;
            let (tx, ty) = point(args, "to_x", "to_y")?;
            let mut a = at(x, y);
            a.extend(s(&["mousedown", "1"]));
            a.extend(at(tx, ty));
            a.extend(s(&["mouseup", "1"]));
            Work::Screen { args: a, said: format!("Dragged from ({x}, {y}) to ({tx}, {ty}).") }
        }
        "type" => {
            let text = args["text"].as_str().unwrap_or_default();
            if text.is_empty() {
                return Err("Say what to type.".into());
            }
            Work::Screen { args: vec!["type".into(), "--delay".into(), "12".into(), "--".into(), text.to_string()], said: format!("Typed {} characters.", text.chars().count()) }
        }
        "key" => {
            let keys: Vec<String> = args["keys"].as_str().unwrap_or_default().split_whitespace().map(String::from).collect();
            if keys.is_empty() {
                return Err("Say which keys, such as \"ctrl+l\" or \"Return\".".into());
            }
            if keys.iter().any(|k| !k.chars().all(|c| c.is_ascii_alphanumeric() || "+_-".contains(c))) {
                return Err("Keys are xdotool's names joined by +, such as ctrl+shift+t or Return.".into());
            }
            let said = format!("Pressed {}.", keys.join(" "));
            let mut a = vec!["key".to_string(), "--delay".into(), "60".into(), "--".into()];
            a.extend(keys);
            Work::Screen { args: a, said }
        }
        "scroll" => {
            let (x, y) = point(args, "x", "y")?;
            let direction = args["direction"].as_str().unwrap_or("down");
            let button = match direction {
                "up" => "4",
                "down" => "5",
                "left" => "6",
                "right" => "7",
                _ => return Err("Scroll up, down, left or right.".into()),
            };
            let amount = args["amount"].as_u64().unwrap_or(3).clamp(1, 30);
            let mut a = at(x, y);
            a.extend(s(&["click", "--repeat", &amount.to_string(), "--delay", "40", button]));
            Work::Screen { args: a, said: format!("Scrolled {direction} {amount} at ({x}, {y}).") }
        }
        "" => return Err("Say what to do: screenshot, click, type, key, scroll…".into()),
        other => return Err(format!("There's no screen action called {other}.")),
    })
}

/// A command's result, for the bot: its exit, then its output and errors, each cut from the end.
fn shell_report(ran: &Ran, seconds: u64) -> String {
    let out = tail(&String::from_utf8_lossy(&ran.out), MOST_OUTPUT);
    let err = tail(&ran.err, MOST_OUTPUT / 3);
    let exit = match ran.code {
        Some(124) | Some(137) => format!("stopped after {seconds} seconds"),
        Some(code) => format!("exit {code}"),
        None => "stopped".into(),
    };
    let mut report = exit;
    if !out.trim().is_empty() {
        report.push_str("\n--- output ---\n");
        report.push_str(out.trim_end());
    }
    if !err.trim().is_empty() {
        report.push_str("\n--- errors ---\n");
        report.push_str(err.trim_end());
    }
    report
}

/// The last `most` bytes of a text (on a character edge), saying so when it was cut.
pub fn tail(text: &str, most: usize) -> String {
    if text.len() <= most {
        return text.to_string();
    }
    let mut start = text.len() - most;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("… ({} characters before this were cut)\n{}", start, &text[start..])
}

// ---- the routes ----------------------------------------------------------------------------

async fn shown(app: &App, bot_id: &str, entry: &Entry) -> Value {
    let live = app.computers.with_box(bot_id, |live| live.clone());
    let state = match live.busy {
        Some(busy) => busy,
        None if running(&container(bot_id)).await == Some(true) => "on",
        None => "off",
    };
    json!({ "botId": bot_id, "kind": entry.kind, "since": entry.since, "state": state, "problem": live.problem, "watching": live.viewers })
}

/// `/api/computers`: `GET` all; `PUT /:bot {on}` gives a bot a computer or takes it away;
/// `POST /:bot/start|stop|reset`.
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>) -> Answer {
    let body = body.unwrap_or_default();
    match (method, rest) {
        (&Method::GET, []) => {
            let words = engine();
            let ready = words.is_some() && image_ready(app).await;
            let (building, problem) = app.computers.with(|inner| (inner.building, inner.image_problem.clone()));
            let image = if ready {
                "ready"
            } else if building {
                "building"
            } else if problem.is_some() {
                "failed"
            } else {
                "missing"
            };
            let mut bots = Vec::new();
            for (bot_id, entry) in entries(app) {
                bots.push(shown(app, &bot_id, &entry).await);
            }
            Ok((StatusCode::OK, json!({ "engine": words.as_deref().map(engine_name), "image": image, "imageProblem": problem, "idleMinutes": idle_minutes(), "bots": bots })))
        }
        (&Method::PUT, [bot_id]) => {
            if app.with_store(|store| store.bot_by_id(bot_id)).flatten().is_none() {
                return Err((StatusCode::NOT_FOUND, "no such bot".into()));
            }
            let mut all = entries(app);
            if body["on"] == true {
                all.entry(bot_id.to_string()).or_insert_with(|| Entry { kind: "container".into(), since: chrono::Utc::now().to_rfc3339() });
                keep(app, &all);
            } else if all.remove(*bot_id).is_some() {
                // Taken away: the box and everything on it go.
                if engine().is_some() {
                    let _ = run(&["rm", "-f", &container(bot_id)], Duration::from_secs(60)).await;
                }
                app.computers.with(|inner| inner.boxes.remove(*bot_id));
                keep(app, &all);
            }
            let entry = all.get(*bot_id).cloned();
            let shown = match entry {
                Some(entry) => shown(app, bot_id, &entry).await,
                None => Value::Null,
            };
            Ok((StatusCode::OK, json!({ "computer": shown })))
        }
        (&Method::POST, [bot_id, action]) => {
            let entry = entries(app).get(*bot_id).cloned().ok_or_else(|| bad("this bot has no computer"))?;
            if engine().is_none() {
                return Err(bad("there's no Podman or Docker on this computer"));
            }
            match *action {
                // Starting can take minutes the first time: it goes on, and the list follows it.
                "start" => {
                    let (app2, bot) = (app.clone(), bot_id.to_string());
                    tokio::spawn(async move {
                        let _ = start(&app2, &bot).await;
                    });
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                "stop" => stop(app, bot_id).await.map_err(bad)?,
                "reset" => reset(app, bot_id).await.map_err(bad)?,
                _ => return Err((StatusCode::NOT_FOUND, "no such computer route".into())),
            }
            Ok((StatusCode::OK, json!({ "computer": shown(app, bot_id, &entry).await })))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such computer route".into())),
    }
}

/// `GET /computer/view/<file>`: the live view's own files, as copied out of the image.
pub async fn serve_view(app: &App, rest: &[&str]) -> Response {
    let not_found = || Response::builder().status(StatusCode::NOT_FOUND).body(Body::from("not here")).expect("response");
    let Some(dir) = view_dir(app).and_then(|d| d.canonicalize().ok()) else { return not_found() };
    let rel = if rest.is_empty() || rest == [""] { "vnc.html".to_string() } else { rest.join("/") };
    let rel_path = Path::new(&rel);
    if rel_path.components().any(|c| !matches!(c, Component::Normal(_))) || rel.split('/').any(|p| p.is_empty() || p.starts_with('.')) {
        return not_found();
    }
    let Ok(path) = dir.join(rel_path).canonicalize() else { return not_found() };
    if !path.starts_with(&dir) || !path.is_file() {
        return not_found();
    }
    let Ok(bytes) = tokio::fs::read(&path).await else { return not_found() };
    let kind = super::pages::content_type(&path).unwrap_or("application/octet-stream");
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    response
}

/// Whether a request comes from Catbot's own page (or from no page at all): a website open
/// elsewhere in the person's browser mustn't reach a bot's screen.
pub fn same_origin(req: &Request) -> bool {
    let Some(origin) = req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok()) else { return true };
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    origin.split_once("://").is_some_and(|(_, rest)| !host.is_empty() && rest.trim_end_matches('/') == host)
}

/// `/computer/<bot>/websockify`: the screen's websocket, carried to the bot's box.
pub async fn view_socket(app: &Arc<App>, bot_id: &str, mut req: Request) -> Response {
    use http_body_util::Empty;
    use hyper_util::rt::TokioIo;

    let answer = |status: StatusCode, text: &str| Response::builder().status(status).body(Body::from(text.to_string())).expect("response");
    if !same_origin(&req) {
        return answer(StatusCode::FORBIDDEN, "not from Catbot's page");
    }
    if !has(app, bot_id) {
        return answer(StatusCode::NOT_FOUND, "this bot has no computer");
    }
    let port = match app.computers.with_box(bot_id, |live| live.port) {
        Some(port) => port,
        None => match running(&container(bot_id)).await {
            // On from before Catbot started: find its port.
            Some(true) => match must(&["port", &container(bot_id), "6080/tcp"], Duration::from_secs(20)).await.ok().as_deref().and_then(parse_port) {
                Some(port) => {
                    app.computers.with_box(bot_id, |live| live.port = Some(port));
                    port
                }
                None => return answer(StatusCode::SERVICE_UNAVAILABLE, "its screen has no port"),
            },
            _ => return answer(StatusCode::SERVICE_UNAVAILABLE, "the computer is off"),
        },
    };
    let Ok(stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else { return answer(StatusCode::BAD_GATEWAY, "its screen didn't answer") };
    let Ok((mut sender, conn)) = hyper::client::conn::http1::handshake::<_, Empty<bytes::Bytes>>(TokioIo::new(stream)).await else {
        return answer(StatusCode::BAD_GATEWAY, "its screen didn't answer");
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let mut upstream = hyper::Request::builder().method(hyper::Method::GET).uri("/websockify").header(header::HOST, format!("127.0.0.1:{port}"));
    for name in [header::UPGRADE, header::CONNECTION, header::SEC_WEBSOCKET_KEY, header::SEC_WEBSOCKET_VERSION, header::SEC_WEBSOCKET_PROTOCOL] {
        if let Some(value) = req.headers().get(&name) {
            upstream = upstream.header(name, value);
        }
    }
    let Ok(upstream) = upstream.body(Empty::new()) else { return answer(StatusCode::BAD_REQUEST, "bad request") };
    let Ok(mut reply) = sender.send_request(upstream).await else { return answer(StatusCode::BAD_GATEWAY, "its screen didn't answer") };
    if reply.status() != StatusCode::SWITCHING_PROTOCOLS {
        return answer(StatusCode::BAD_GATEWAY, "its screen turned the view away");
    }
    let mut response = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for name in [header::UPGRADE, header::CONNECTION, header::SEC_WEBSOCKET_ACCEPT, header::SEC_WEBSOCKET_PROTOCOL] {
        if let Some(value) = reply.headers().get(&name) {
            response = response.header(name, value);
        }
    }
    let theirs = hyper::upgrade::on(&mut reply);
    let ours = hyper::upgrade::on(&mut req);
    let (app, bot_id) = (app.clone(), bot_id.to_string());
    tokio::spawn(async move {
        let (Ok(ours), Ok(theirs)) = tokio::join!(ours, theirs) else { return };
        app.computers.with_box(&bot_id, |live| live.viewers += 1);
        changed(&app);
        let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(ours), &mut TokioIo::new(theirs)).await;
        app.computers.with_box(&bot_id, |live| {
            live.viewers = live.viewers.saturating_sub(1);
            live.last_used = Some(Instant::now());
        });
        changed(&app);
    });
    response.body(Body::empty()).unwrap_or_else(|_| answer(StatusCode::BAD_GATEWAY, "bad reply"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_ports() {
        assert_eq!(container("0b1A-x/../y"), "catbot-computer-0b1a-xy");
        assert_eq!(parse_port("127.0.0.1:32768\n"), Some(32768));
        assert_eq!(parse_port("0.0.0.0:41000\n[::]:41000"), Some(41000));
        assert_eq!(parse_port(""), None);
        assert_eq!(engine_name(&["/usr/bin/podman".into()]), "Podman");
        assert_eq!(engine_name(&["sudo".into(), "docker".into()]), "Docker");
    }

    #[test]
    fn screen_actions() {
        let click = plan("computer_screen", &json!({ "action": "click", "x": 10, "y": 20 })).unwrap();
        assert_eq!(click, Work::Screen { args: ["mousemove", "--sync", "10", "20", "click", "1"].map(String::from).to_vec(), said: "Clicked at (10, 20).".into() });
        assert!(plan("computer_screen", &json!({ "action": "click", "x": 1280, "y": 20 })).unwrap_err().contains("off the screen"));
        assert!(plan("computer_screen", &json!({ "action": "click" })).unwrap_err().contains("Say where"));
        let Work::Screen { args, .. } = plan("computer_screen", &json!({ "action": "type", "text": "-rf --hi" })).unwrap() else { panic!() };
        assert_eq!(args, ["type", "--delay", "12", "--", "-rf --hi"]);
        let Work::Screen { args, .. } = plan("computer_screen", &json!({ "action": "key", "keys": "ctrl+l Return" })).unwrap() else { panic!() };
        assert_eq!(args, ["key", "--delay", "60", "--", "ctrl+l", "Return"]);
        assert!(plan("computer_screen", &json!({ "action": "key", "keys": "ctrl+l;rm" })).is_err());
        let Work::Screen { args, .. } = plan("computer_screen", &json!({ "action": "scroll", "x": 5, "y": 5, "direction": "up", "amount": 99 })).unwrap() else { panic!() };
        assert_eq!(args[4..], ["click", "--repeat", "30", "--delay", "40", "4"].map(String::from));
        assert_eq!(plan("computer_screen", &json!({ "action": "screenshot" })).unwrap(), Work::Screenshot);
        assert!(plan("computer_screen", &json!({ "action": "dance" })).is_err());
    }

    #[test]
    fn commands_and_addresses() {
        assert_eq!(plan("computer_run", &json!({ "command": " ls ", "timeout_seconds": 99999 })).unwrap(), Work::Shell { command: "ls".into(), seconds: 1800 });
        assert!(plan("computer_run", &json!({})).is_err());
        assert_eq!(plan("computer_open", &json!({ "url": "https://example.com" })).unwrap(), Work::Open("https://example.com".into()));
        assert!(plan("computer_open", &json!({ "url": "file:///etc/passwd" })).is_err());
        assert!(plan("computer_open", &json!({ "url": "https://a b" })).is_err());
    }

    #[test]
    fn cut_from_the_end() {
        assert_eq!(tail("short", 10), "short");
        let cut = tail("ééééé", 4);
        assert!(cut.ends_with("éé"), "{cut}");
        let ran = Ran { code: Some(124), out: b"partial".to_vec(), err: String::new() };
        assert_eq!(shell_report(&ran, 5), "stopped after 5 seconds\n--- output ---\npartial");
        let ran = Ran { code: Some(2), out: Vec::new(), err: "ls: nope\n".into() };
        assert_eq!(shell_report(&ran, 5), "exit 2\n--- errors ---\nls: nope");
    }

    #[test]
    fn only_catbots_own_page() {
        let req = |origin: Option<&str>| {
            let mut r = Request::builder().uri("/computer/x/websockify").header(header::HOST, "127.0.0.1:8788");
            if let Some(o) = origin {
                r = r.header(header::ORIGIN, o);
            }
            r.body(Body::empty()).unwrap()
        };
        assert!(same_origin(&req(None)));
        assert!(same_origin(&req(Some("http://127.0.0.1:8788"))));
        assert!(!same_origin(&req(Some("https://evil.example"))));
        assert!(!same_origin(&req(Some("http://127.0.0.1:8788.evil.example"))));
    }
}
