//! Settings › AI's terminal and program-path tools (stage 19.7.2), after MausBot's "Open install
//! in Terminal" and "CLI path":
//!
//! * `POST /api/engines/:id/terminal` `{ what: "install" | "signIn" }` opens a terminal window on
//!   this computer running that AI's own install or sign-in command. Only from home, and only
//!   the command Catbot knows for that AI: the request names which, never what to run.
//! * `PUT /api/engines/:id/path` `{ path, force? }` points Catbot at the AI's program when it
//!   isn't found by name; it's tried with `--version` first (`force` keeps a path that fails
//!   the try). `{ path: null }` goes back to finding it by name. Kept in the settings,
//!   `engine-paths`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hyper::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::engines::{self, Engine, Kind};

const PATHS: &str = "engine-paths";

/// Loads the chosen paths at start.
pub fn load(app: &App) {
    let saved: HashMap<String, PathBuf> = app.with_store(|store| store.setting(PATHS)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    engines::set_chosen_paths(saved);
}

/// The terminals tried, in order, and how each is told to run a command and stay open.
fn terminals(command: &str) -> Vec<(String, Vec<String>)> {
    if cfg!(target_os = "windows") {
        return vec![("cmd".into(), vec!["/C".into(), "start".into(), "Catbot".into(), "cmd".into(), "/K".into(), command.into()])];
    }
    if cfg!(target_os = "macos") {
        let quoted = command.replace('\\', "\\\\").replace('"', "\\\"");
        return vec![("osascript".into(), vec!["-e".into(), format!("tell application \"Terminal\" to do script \"{quoted}\""), "-e".into(), "tell application \"Terminal\" to activate".into()])];
    }
    // Stays open after the command, so its output (and any sign-in prompt) can be read.
    let held = format!("{command}; echo; echo 'Done. You can close this window.'; exec bash");
    let bash = |prefix: &[&str]| prefix.iter().map(|s| s.to_string()).chain(["bash".into(), "-lc".into(), held.clone()]).collect::<Vec<String>>();
    let mut list = Vec::new();
    if let Ok(own) = std::env::var("TERMINAL")
        && !own.trim().is_empty()
    {
        list.push((own.trim().to_string(), bash(&["-e"])));
    }
    list.extend([
        ("konsole".to_string(), bash(&["-e"])),
        ("gnome-terminal".to_string(), bash(&["--"])),
        ("kgx".to_string(), bash(&["--"])),
        ("xfce4-terminal".to_string(), vec!["-x".into(), "bash".into(), "-lc".into(), held.clone()]),
        ("x-terminal-emulator".to_string(), bash(&["-e"])),
        ("alacritty".to_string(), bash(&["-e"])),
        ("kitty".to_string(), bash(&[])),
        ("xterm".to_string(), bash(&["-e"])),
    ]);
    list
}

/// Opens a terminal running `command`; the terminal's name, or why it couldn't.
pub fn open_terminal(command: &str) -> Result<String, String> {
    for (program, args) in terminals(command) {
        let Some(found) = engines::find_program(&program) else { continue };
        let mut run = std::process::Command::new(&found);
        run.args(&args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        match run.spawn() {
            Ok(mut child) => {
                // Not waited on: the terminal outlives the request.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(program);
            }
            Err(err) => eprintln!("catbot: couldn't open {program}: {err}"),
        }
    }
    Err("no terminal program was found on this computer (set TERMINAL to yours)".into())
}

/// The command an AI's install or sign-in is, on this computer.
fn command_for(engine: &Engine, what: &str) -> Result<String, (StatusCode, String)> {
    let line = match what {
        "install" => engine.install,
        "signIn" => engine.sign_in,
        _ => return Err(bad("open the install or the sign-in (\"what\": \"install\" or \"signIn\")")),
    };
    let command = engines::terminal_command(line).ok_or_else(|| bad(format!("{} has no {} command", engine.name, if what == "install" { "install" } else { "sign-in" })))?;
    if !engines::runs_here(&command) {
        return Err(bad(format!("there isn't a one-line installer for {} on Windows; its page says how: {}", engine.name, engine.key_page)));
    }
    Ok(command)
}

pub fn terminal(engine: &Engine, body: &Value) -> Answer {
    if !matches!(engine.kind, Kind::Agent | Kind::Codex | Kind::Pi | Kind::Claude) {
        return Err(bad(format!("{} doesn't install in a terminal", engine.name)));
    }
    let what = body["what"].as_str().unwrap_or("install");
    let command = command_for(engine, what)?;
    let opened = open_terminal(&command).map_err(|why| (StatusCode::SERVICE_UNAVAILABLE, why))?;
    eprintln!("catbot: opened {opened} to run {}'s {what}", engine.name);
    Ok((StatusCode::OK, json!({ "opened": opened, "command": command })))
}

/// Tries a program with `--version`: its first line, or why it didn't answer.
async fn try_program(path: &Path) -> Result<String, String> {
    let run = tokio::process::Command::new(path).arg("--version").stdin(std::process::Stdio::null()).kill_on_drop(true).output();
    match tokio::time::timeout(Duration::from_secs(10), run).await {
        Err(_) => Err("it didn't answer --version in 10 seconds".into()),
        Ok(Err(err)) => Err(format!("it didn't start: {err}")),
        Ok(Ok(out)) => {
            let text = String::from_utf8_lossy(if out.stdout.is_empty() { &out.stderr } else { &out.stdout }).to_string();
            let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().chars().take(120).collect::<String>();
            if out.status.success() { Ok(if line.is_empty() { "it ran".into() } else { line }) } else { Err(if line.is_empty() { format!("it exited with {}", out.status) } else { line }) }
        }
    }
}

pub async fn set_path(app: &App, engine: &Engine, body: &Value) -> Answer {
    if !matches!(engine.kind, Kind::Agent | Kind::Codex | Kind::Pi) {
        return Err(bad(format!("{}'s program is set when Catbot starts (catbot serve --cli)", engine.name)));
    }
    let mut saved: HashMap<String, PathBuf> = app.with_store(|store| store.setting(PATHS)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let version = match body["path"].as_str().map(str::trim).filter(|p| !p.is_empty()) {
        None => {
            saved.remove(engine.id);
            None
        }
        Some(given) => {
            let expanded = given.strip_prefix("~/").map(|rest| crate::store::user_home().join(rest)).unwrap_or_else(|| PathBuf::from(given));
            if !expanded.is_absolute() {
                return Err(bad("give the program's full path, like /home/you/.local/bin/grok"));
            }
            if !expanded.is_file() {
                return Err(bad(format!("there's no file at {}", expanded.display())));
            }
            let tried = try_program(&expanded).await;
            if let Err(why) = &tried
                && body["force"] != true
            {
                return Err((StatusCode::UNPROCESSABLE_ENTITY, format!("Test failed: {why}.")));
            }
            saved.insert(engine.id.to_string(), expanded);
            Some(tried.unwrap_or_else(|why| format!("kept, though the test failed: {why}")))
        }
    };
    app.try_store(|store| store.set_setting(PATHS, &json!(saved))).map_err(fail)?;
    engines::set_chosen_paths(saved);
    app.hub.send(json!({ "kind": "engines" }));
    Ok((StatusCode::OK, json!({ "version": version, "program": engines::agent_program(engine), "chosen": engines::chosen_path(engine.id) })))
}
