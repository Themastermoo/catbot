//! The Windows app's own Catbot: `catbot-server.exe`, beside the app, started when the app
//! opens (unless one is already answering) and stopped when it closes.
//!
//! The server is started with `--stop-with-input` and its input piped from here: when the app
//! ends, for any reason, the pipe closes and the server stops. Its messages go to
//! `server.log` in Catbot's data folder.

#![cfg_attr(not(feature = "desktop"), allow(dead_code))]

use std::{
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Where this app's own Catbot listens.
const ADDRESS: &str = "127.0.0.1:8788";

/// The phone app's own Catbot (stage 26): the same server, run inside the app on its own thread,
/// keeping its data in the app's private storage. Only the chat APIs work there
/// (`CATBOT_PHONE`); the AIs that are programs, and the remote door, stay on the PC.
#[cfg(target_os = "android")]
pub fn start_on_phone() {
    if answering() {
        return;
    }
    let files = std::path::PathBuf::from("/data/data/com.moo.catbot/files");
    // SAFETY: set once, before the server thread starts and before anything reads them.
    unsafe {
        std::env::set_var("CATBOT_PHONE", "1");
        std::env::set_var("HOME", &files);
    }
    let options = catbot::server::Options {
        listen: ADDRESS.parse().expect("a fixed address"),
        upstream: "http://127.0.0.1:9".into(),
        home: files.join("catbot"),
        codex: files.join("VaultCodex"),
        memory: Some(files.join("CatbotMemory")),
        cli: vec!["claude".into()],
        record: None,
        ui: None,
        remote: None,
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build();
        match runtime {
            Ok(runtime) => {
                if let Err(err) = runtime.block_on(catbot::server::serve(options)) {
                    eprintln!("catbot: the phone's own Catbot stopped: {err}");
                }
            }
            Err(err) => eprintln!("catbot: couldn't start the phone's own Catbot: {err}"),
        }
    });
    let since = Instant::now();
    while !answering() && since.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Kept for the life of the app: holding it keeps the server's input open.
static SERVER: Mutex<Option<Child>> = Mutex::new(None);

/// Starts this PC's Catbot if the app runs one and none is answering yet, then waits (a few
/// seconds at most) until it answers, so the window opens onto a working Catbot.
pub fn start() {
    if !crate::link::has_own() || answering() {
        return;
    }
    let Some(program) = server_program() else {
        eprintln!("catbot: no catbot-server beside the app, so this PC's Catbot can't start");
        return;
    };
    match spawn(&program) {
        Ok(child) => {
            *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
            let since = Instant::now();
            while !answering() && since.elapsed() < Duration::from_secs(8) {
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        Err(err) => eprintln!("catbot: couldn't start {}: {err}", program.display()),
    }
}

fn answering() -> bool {
    let addr: SocketAddr = ADDRESS.parse().expect("a fixed address");
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// `catbot-server(.exe)` in the app's own folder.
fn server_program() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let name = if cfg!(windows) { "catbot-server.exe" } else { "catbot-server" };
    let program = dir.join(name);
    program.is_file().then_some(program)
}

fn spawn(program: &PathBuf) -> std::io::Result<Child> {
    let dir = program.parent().map(PathBuf::from).unwrap_or_default();
    let log = log_file()?;
    let mut command = Command::new(program);
    command
        .args(["serve", "--listen", ADDRESS, "--remote", "off", "--stop-with-input", "--ui"])
        .arg(dir.join("ui"))
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(log.try_clone()?)
        .stderr(log);
    // No console window for it.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn()
}

fn log_file() -> std::io::Result<std::fs::File> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Catbot");
    std::fs::create_dir_all(&dir)?;
    std::fs::OpenOptions::new().create(true).append(true).open(dir.join("server.log"))
}
