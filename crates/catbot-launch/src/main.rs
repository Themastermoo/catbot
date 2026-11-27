//! `Catbot.exe` on Windows (stage 28.3): the launcher the Start menu and desktop shortcuts open.
//!
//! Catbot keeps its versions side by side in the install folder:
//!
//! ```text
//! %LOCALAPPDATA%\Programs\Catbot\
//!     Catbot.exe          this launcher
//!     versions.json       which version is current, which is being tried, each one's storage format
//!     versions\0.2.0\     a whole app: Catbot.exe, catbot-server.exe, assets, ui …
//!     versions\0.3.0\
//! ```
//!
//! The launcher starts the current version and waits for it.
//! * When the app closes with code 75, it was asked to restart into another version (an update,
//!   or going back), so the launcher starts whichever version is now current.
//! * When a version that's being tried (just installed, not yet settled) closes with an error,
//!   the launcher asks before doing anything (Moo's choice, 2026-11-25): go back to the version
//!   before, try again, or close. Going back keeps today's chats, unless the older version can't
//!   read how they're stored now. Then it says so and asks Catbot for the copy taken before the
//!   update (`updates/restore.json`), which Catbot puts in place as it starts, setting today's
//!   chats aside rather than deleting them.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// The app's exit code for "start whichever version is current now".
pub const RESTART: i32 = 75;

#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    GoBack,
    TryAgain,
    Close,
}

/// How the launcher talks to the person: Windows' message boxes, or stand-ins in tests.
pub trait Ask {
    fn go_back(&mut self, text: &str) -> Answer;
    fn tell(&mut self, text: &str);
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    // PowerShell can write a byte-order mark first.
    serde_json::from_slice(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).ok()
}

fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("json.part");
    std::fs::write(&part, serde_json::to_vec_pretty(value).unwrap_or_default())?;
    std::fs::rename(&part, path)
}

fn version_numbers(v: &str) -> Vec<u64> {
    let mut n: Vec<u64> = v.trim_start_matches(['v', 'V']).split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    n.resize(3, 0);
    n
}

/// The version to start: `versions.json`'s current one if it's there, else the newest folder.
fn current(dir: &Path, kept: &Value) -> Option<String> {
    let runnable = |v: &str| dir.join("versions").join(v).join("Catbot.exe").is_file();
    if let Some(v) = kept["current"].as_str().filter(|v| runnable(v)) {
        return Some(v.to_string());
    }
    let mut found: Vec<String> = std::fs::read_dir(dir.join("versions"))
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|v| runnable(v))
        .collect();
    found.sort_by_key(|v| version_numbers(v));
    found.pop()
}

/// What a failed start says: Windows' crash codes read best in hex.
fn why(code: Option<i32>, err: Option<&std::io::Error>) -> String {
    match (code, err) {
        (_, Some(err)) => format!("Windows couldn't start it: {err}"),
        (Some(code), _) if !(0..=0xFFFF).contains(&code) => format!("it stopped with error 0x{:08X}", code as u32),
        (Some(code), _) => format!("it closed with code {code}"),
        (None, _) => "it was stopped".to_string(),
    }
}

/// The copy of the chats to go back with, when `previous` can't read how `failed` stores them:
/// the newest copy taken in a format `previous` reads. `Some(None)` means one is needed but
/// there's none.
fn copy_needed(home: &Path, kept: &Value, failed: &str, previous: &str) -> Option<Option<Value>> {
    let theirs = kept["kept"][previous]["store"].as_i64()?;
    let ours = kept["kept"][failed]["store"].as_i64()?;
    if theirs >= ours {
        return None;
    }
    let mut copies: Vec<Value> = std::fs::read_dir(home.join("updates").join("snapshots"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_json(&e.path()))
        .filter(|c| c["store"].as_i64().is_some_and(|s| s <= theirs) && c["file"].as_str().is_some_and(|f| Path::new(f).is_file()))
        .collect();
    copies.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
    Some(copies.pop())
}

/// When a copy was taken, as the person reads it: "Nov 25 at 3:10 PM".
fn when(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| t.with_timezone(&chrono::Local).format("%b %-d at %-I:%M %p").to_string())
        .unwrap_or_else(|_| at.to_string())
}

/// Starts the current version until it closes for good; returns the exit code. `args` go to the
/// first start only (`--hidden` from starting at login, stage 29.2); a restart into another
/// version opens its window.
/// Starts the program and waits for it. On Linux a program file just written can be "busy" for
/// a moment (another program starting then holds it open), so that is tried again, briefly.
fn start_and_wait(command: impl Fn() -> Command) -> std::io::Result<std::process::ExitStatus> {
    for _ in 0..20 {
        match command().status() {
            Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy => std::thread::sleep(std::time::Duration::from_millis(50)),
            other => return other,
        }
    }
    command().status()
}

pub fn run(dir: &Path, home: &Path, args: &[String], ask: &mut impl Ask) -> i32 {
    for attempt in 0..10 {
        let kept = read_json(&dir.join("versions.json")).unwrap_or_else(|| json!({}));
        let Some(version) = current(dir, &kept) else {
            ask.tell(&format!("Catbot isn't installed properly: there's no version to start in {}.\n\nInstall it again from the zip (Install.cmd).", dir.join("versions").display()));
            return 1;
        };
        let program = dir.join("versions").join(&version).join("Catbot.exe");
        let first: &[String] = if attempt == 0 { args } else { &[] };
        let started = start_and_wait(|| {
            let mut command = Command::new(&program);
            command.args(first).current_dir(program.parent().unwrap_or(dir)).env("CATBOT_INSTALL_DIR", dir);
            command
        });
        let (code, err) = match &started {
            Ok(status) => (status.code(), None),
            Err(err) => (None, Some(err)),
        };
        if code == Some(RESTART) {
            continue;
        }
        let mut kept = read_json(&dir.join("versions.json")).unwrap_or_else(|| json!({}));
        let trying = kept["trying"]["version"] == json!(version);
        if code == Some(0) || !trying {
            return code.unwrap_or(1);
        }
        // A new version didn't start. Ask before going back.
        let failed = why(code, err);
        let previous = kept["previous"].as_str().filter(|p| *p != version && dir.join("versions").join(p).join("Catbot.exe").is_file()).map(str::to_string);
        let Some(previous) = previous else {
            ask.tell(&format!("Catbot {version} didn't start ({failed}), and there's no earlier version on this PC to go back to.\n\nInstall Catbot again from the zip."));
            return 1;
        };
        let copy = copy_needed(home, &kept, &version, &previous);
        let chats = match &copy {
            None => "Your chats stay as they are.".to_string(),
            Some(Some(copy)) => format!(
                "Catbot {previous} can't read how {version} stores chats, so it goes back to them as they were on {}. Anything since is set aside, not deleted.",
                when(copy["at"].as_str().unwrap_or_default())
            ),
            Some(None) => format!("Catbot {version} changed how chats are stored, and there's no copy from before, so {previous} may not be able to open them."),
        };
        let text = format!("Catbot {version} didn't start: {failed}.\n\nGo back to Catbot {previous}? {chats}\n\nYes: go back to {previous}.\nNo: try {version} again.\nCancel: close Catbot for now.");
        match ask.go_back(&text) {
            Answer::Close => return 1,
            Answer::TryAgain => continue,
            Answer::GoBack => {
                if let Some(Some(copy)) = copy {
                    let restore = json!({ "file": copy["file"], "at": copy["at"], "asked": chrono::Utc::now().to_rfc3339(), "from": version });
                    if let Err(err) = write_json(&home.join("updates").join("restore.json"), &restore) {
                        ask.tell(&format!("Couldn't ask for the copy of the chats ({err}), so Catbot stays on {version} for now."));
                        return 1;
                    }
                }
                kept["current"] = json!(previous);
                kept["previous"] = json!(version);
                kept["trying"] = Value::Null;
                kept["failed"] = json!({ "version": version, "why": failed, "at": chrono::Utc::now().to_rfc3339() });
                if let Err(err) = write_json(&dir.join("versions.json"), &kept) {
                    ask.tell(&format!("Couldn't switch back to {previous}: {err}"));
                    return 1;
                }
            }
        }
    }
    ask.tell("Catbot keeps closing as it starts, so the launcher stopped trying. Open Catbot again to try once more.");
    1
}

/// Catbot's data folder, where `updates/` lives (the server's `store::default_home`).
fn home() -> PathBuf {
    if let Some(home) = std::env::var_os("CATBOT_HOME") {
        return home.into();
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
        return PathBuf::from(local).join("Catbot");
    }
    let user = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| user.join(".local/share")).join("catbot")
}

#[cfg(windows)]
mod dialogs {
    use super::{Answer, Ask};
    use std::ffi::c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn show(text: &str, kind: u32) -> i32 {
        let (text, caption) = (wide(text), wide("Catbot"));
        // SAFETY: both strings are NUL-terminated UTF-16 and live for the call.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), kind) }
    }

    pub struct Boxes;

    impl Ask for Boxes {
        fn go_back(&mut self, text: &str) -> Answer {
            // MB_YESNOCANCEL | MB_ICONWARNING; IDYES = 6, IDNO = 7.
            match show(text, 0x3 | 0x30) {
                6 => Answer::GoBack,
                7 => Answer::TryAgain,
                _ => Answer::Close,
            }
        }

        fn tell(&mut self, text: &str) {
            // MB_OK | MB_ICONERROR
            show(text, 0x10);
        }
    }
}

/// Off Windows (trying the launcher on Linux): says it on the terminal, and answers with
/// `CATBOT_LAUNCH_ANSWER` (back, again), else closes.
#[cfg(not(windows))]
mod dialogs {
    use super::{Answer, Ask};

    pub struct Boxes;

    impl Ask for Boxes {
        fn go_back(&mut self, text: &str) -> Answer {
            eprintln!("{text}");
            match std::env::var("CATBOT_LAUNCH_ANSWER").as_deref() {
                Ok("back") => Answer::GoBack,
                Ok("again") => Answer::TryAgain,
                _ => Answer::Close,
            }
        }

        fn tell(&mut self, text: &str) {
            eprintln!("{text}");
        }
    }
}

/// `Catbot.exe --register <version>` (stage 29.5): the installer has put `versions\<version>\` in
/// place; make it current, the one it replaces the one to go back to, and note its storage
/// format (from its `version.json`). Versions whose folders are gone are forgotten.
pub fn register(dir: &Path, version: &str) -> Result<(), String> {
    let folder = dir.join("versions").join(version);
    if !folder.join("Catbot.exe").is_file() {
        return Err(format!("{} has no Catbot.exe", folder.display()));
    }
    let about = read_json(&folder.join("version.json")).unwrap_or_else(|| json!({}));
    let mut kept = read_json(&dir.join("versions.json")).unwrap_or_else(|| json!({}));
    if !kept["kept"].is_object() {
        kept["kept"] = json!({});
    }
    let old = kept["current"].as_str().map(str::to_string);
    if let Some(old) = old.filter(|o| o != version && dir.join("versions").join(o).join("Catbot.exe").is_file()) {
        kept["previous"] = json!(old);
    }
    kept["current"] = json!(version);
    kept["trying"] = Value::Null;
    kept["failed"] = Value::Null;
    kept["kept"][version] = json!({ "store": about["store"], "at": chrono::Utc::now().to_rfc3339() });
    if let Some(map) = kept["kept"].as_object_mut() {
        map.retain(|v, _| dir.join("versions").join(v).join("Catbot.exe").is_file());
    }
    write_json(&dir.join("versions.json"), &kept).map_err(|e| e.to_string())
}

fn main() {
    let dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, version] = args.as_slice()
        && flag == "--register"
    {
        let code = match register(&dir, version) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("catbot: {err}");
                1
            }
        };
        std::process::exit(code);
    }
    let code = run(&dir, &home(), &args, &mut dialogs::Boxes);
    std::process::exit(code);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Answers in order, and remembers what it was asked.
    struct Script {
        answers: Vec<Answer>,
        asked: Vec<String>,
    }

    impl Ask for Script {
        fn go_back(&mut self, text: &str) -> Answer {
            self.asked.push(text.to_string());
            if self.answers.is_empty() { Answer::Close } else { self.answers.remove(0) }
        }
        fn tell(&mut self, text: &str) {
            self.asked.push(text.to_string());
        }
    }

    /// A stand-in app: a script that appends its version to `ran` and exits with the next code
    /// in its `codes` file (0 once they run out).
    fn version(dir: &Path, v: &str, codes: &[i32]) {
        use std::os::unix::fs::PermissionsExt;
        let folder = dir.join("versions").join(v);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("codes"), codes.iter().map(|c| format!("{c}\n")).collect::<String>()).unwrap();
        let script = format!(
            "#!/bin/sh\necho {v} >> \"$CATBOT_INSTALL_DIR/ran\"\ncode=$(head -n1 codes)\ntail -n +2 codes > codes.next; mv codes.next codes\nexit ${{code:-0}}\n"
        );
        let program = folder.join("Catbot.exe");
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn ran(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("ran")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn kept(dir: &Path) -> Value {
        read_json(&dir.join("versions.json")).unwrap()
    }

    #[test]
    fn it_starts_the_current_version_and_restarts_into_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        version(dir.path(), "0.2.0", &[RESTART]);
        version(dir.path(), "0.3.0", &[0]);
        write_json(&dir.path().join("versions.json"), &json!({ "current": "0.2.0" })).unwrap();
        // As Catbot does when installing: 0.2.0 makes 0.3.0 current, then closes to restart.
        let mut ask = Script { answers: vec![], asked: vec![] };
        let switch = dir.path().join("versions/0.2.0/Catbot.exe");
        let script = std::fs::read_to_string(&switch).unwrap().replace("exit ", "printf '{\"current\":\"0.3.0\"}' > \"$CATBOT_INSTALL_DIR/versions.json\"\nexit ");
        std::fs::write(&switch, script).unwrap();
        assert_eq!(run(dir.path(), home.path(), &[], &mut ask), 0);
        assert_eq!(ran(dir.path()), ["0.2.0", "0.3.0"]);
        assert!(ask.asked.is_empty());
    }

    #[test]
    fn the_installer_registers_its_version() {
        let dir = tempfile::tempdir().unwrap();
        version(dir.path(), "0.2.0", &[]);
        version(dir.path(), "0.3.0", &[]);
        std::fs::write(dir.path().join("versions/0.3.0/version.json"), r#"{ "version": "0.3.0", "store": 13 }"#).unwrap();
        write_json(&dir.path().join("versions.json"), &json!({ "current": "0.2.0", "kept": { "0.2.0": { "store": 12 }, "0.1.0": { "store": 11 } } })).unwrap();
        register(dir.path(), "0.3.0").unwrap();
        let now = kept(dir.path());
        assert_eq!((now["current"].as_str(), now["previous"].as_str()), (Some("0.3.0"), Some("0.2.0")), "{now}");
        assert_eq!(now["kept"]["0.3.0"]["store"], 13);
        assert!(now["kept"]["0.1.0"].is_null(), "its folder is gone");
        assert!(register(dir.path(), "9.9.9").is_err());
    }

    #[test]
    fn a_settled_version_that_crashes_just_closes() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        version(dir.path(), "0.2.0", &[3]);
        let mut ask = Script { answers: vec![], asked: vec![] };
        assert_eq!(run(dir.path(), home.path(), &[], &mut ask), 3, "no versions.json: the newest folder runs");
        assert!(ask.asked.is_empty(), "nothing being tried, nothing to ask");
    }

    #[test]
    fn a_new_version_that_wont_start_asks_before_going_back() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        version(dir.path(), "0.2.0", &[0]);
        version(dir.path(), "0.3.0", &[1, 1]);
        let tried = json!({ "current": "0.3.0", "previous": "0.2.0", "trying": { "version": "0.3.0" }, "kept": { "0.2.0": { "store": 12 }, "0.3.0": { "store": 12 } } });
        write_json(&dir.path().join("versions.json"), &tried).unwrap();

        // Try again, then close: nothing changes.
        let mut ask = Script { answers: vec![Answer::TryAgain, Answer::Close], asked: vec![] };
        assert_eq!(run(dir.path(), home.path(), &[], &mut ask), 1);
        assert_eq!(ran(dir.path()), ["0.3.0", "0.3.0"]);
        assert!(ask.asked[0].contains("Catbot 0.3.0 didn't start: it closed with code 1"), "{}", ask.asked[0]);
        assert!(ask.asked[0].contains("Your chats stay as they are."), "same storage format");
        assert_eq!(kept(dir.path())["current"], "0.3.0");

        // Go back: 0.2.0 is current, and the failure is noted for Settings.
        std::fs::write(dir.path().join("versions/0.3.0/codes"), "1\n").unwrap();
        let mut ask = Script { answers: vec![Answer::GoBack], asked: vec![] };
        assert_eq!(run(dir.path(), home.path(), &[], &mut ask), 0);
        assert_eq!(ran(dir.path())[2..], ["0.3.0", "0.2.0"]);
        let now = kept(dir.path());
        assert_eq!((now["current"].as_str(), now["previous"].as_str(), now["trying"].is_null()), (Some("0.2.0"), Some("0.3.0"), true), "{now}");
        assert_eq!(now["failed"]["version"], "0.3.0");
        assert!(!home.path().join("updates/restore.json").exists(), "today's chats stay");
    }

    #[test]
    fn going_back_past_a_storage_change_asks_for_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        version(dir.path(), "0.2.0", &[0]);
        version(dir.path(), "0.3.0", &[1]);
        write_json(
            &dir.path().join("versions.json"),
            &json!({ "current": "0.3.0", "previous": "0.2.0", "trying": { "version": "0.3.0" }, "kept": { "0.2.0": { "store": 12 }, "0.3.0": { "store": 13 } } }),
        )
        .unwrap();
        let copies = home.path().join("updates/snapshots");
        std::fs::create_dir_all(&copies).unwrap();
        std::fs::write(copies.join("a.db"), "before").unwrap();
        write_json(&copies.join("a.json"), &json!({ "file": copies.join("a.db"), "store": 12, "at": "2026-11-25T21:10:00Z" })).unwrap();
        let mut ask = Script { answers: vec![Answer::GoBack], asked: vec![] };
        assert_eq!(run(dir.path(), home.path(), &[], &mut ask), 0);
        assert!(ask.asked[0].contains("set aside, not deleted"), "{}", ask.asked[0]);
        let restore = read_json(&home.path().join("updates/restore.json")).expect("asked for the copy");
        assert_eq!(restore["file"], json!(copies.join("a.db")));
        assert_eq!(kept(dir.path())["current"], "0.2.0");
    }
}
