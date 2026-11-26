//! Installing an AI program and signing it in without a terminal (stage 29.7).
//!
//! The install is the maker's own line (`npm install -g …`, `curl … | bash`), run by Catbot with
//! the folders installers use on its PATH. The sign-in is the program's own login command
//! (`kimi login`, `claude auth login`): it prints the page to open and, for a device sign-in, a
//! code; Catbot opens the page in the person's browser, where they're likely already signed in,
//! shows the code, and waits for the program to say it's done.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::mpsc,
};

/// How long a sign-in may wait for the person, and an install for its download.
const SIGN_IN_WAIT: Duration = Duration::from_secs(600);
const INSTALL_WAIT: Duration = Duration::from_secs(900);
/// How long to look for the page to open before showing what there is.
const LINK_WAIT: Duration = Duration::from_secs(20);

/// The folders installers put programs in, besides the PATH: Catbot started from the desktop
/// often has a short PATH that leaves them out.
pub fn tool_dirs() -> Vec<PathBuf> {
    let home = crate::store::user_home();
    let mut dirs = vec![home.join(".local/bin"), home.join(".npm-global/bin"), home.join("bin"), home.join(".bun/bin"), home.join(".cargo/bin")];
    // nvm's newest Node, where `npm install -g` puts programs.
    if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
        let mut found: Vec<PathBuf> = versions.flatten().map(|e| e.path().join("bin")).filter(|p| p.is_dir()).collect();
        found.sort_by_key(|p| version_of(p));
        if let Some(newest) = found.pop() {
            dirs.push(newest);
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    dirs
}

/// `…/v22.11.0/bin` → (22, 11, 0), to find the newest.
fn version_of(bin: &Path) -> (u32, u32, u32) {
    let name = bin.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or_default();
    let mut parts = name.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// The PATH for an install or a sign-in: the one Catbot has, then the installers' folders.
pub fn tool_path() -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for dir in tool_dirs() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Opens a web page in the person's browser. Only https pages (a sign-in page). `CATBOT_OPEN_WITH`
/// names another program to hand it to (tests use a stand-in, so nothing opens).
pub fn open_in_browser(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let mut command = match std::env::var_os("CATBOT_OPEN_WITH").filter(|p| !p.is_empty()) {
        Some(program) => std::process::Command::new(program),
        None if cfg!(windows) => {
            let mut c = std::process::Command::new("rundll32");
            c.arg("url.dll,FileProtocolHandler");
            c
        }
        None if cfg!(target_os = "macos") => std::process::Command::new("open"),
        None => std::process::Command::new("xdg-open"),
    };
    let _ = command.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// What a sign-in shows the person: the page, and the code to type there when it gives one.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Shown {
    pub url: Option<String>,
    pub code: Option<String>,
}

/// The first https address in some text, without the punctuation around it.
pub fn link_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .filter_map(|word| word.find("https://").map(|at| &word[at..]))
        .map(|word| word.trim_end_matches(|c: char| ".,;:)]}>\"'".contains(c)).to_string())
        .find(|url| url.len() > "https://".len())
}

/// A device sign-in's code: the word after "code" ("Enter code ABCD-1234", "User code: WXYZ-5678"),
/// or the address's own `user_code`.
pub fn code_in(text: &str, url: Option<&str>) -> Option<String> {
    let from_link = url.and_then(|u| reqwest::Url::parse(u).ok()).and_then(|u| u.query_pairs().find(|(k, _)| k == "user_code").map(|(_, v)| v.into_owned()));
    let looks = |w: &str| (4..=16).contains(&w.len()) && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && w.chars().any(|c| c.is_ascii_digit() || c == '-');
    let from_text = || {
        let words: Vec<&str> = text.split_whitespace().collect();
        words.windows(2).find_map(|w| {
            let label = w[0].trim_end_matches(':').to_lowercase();
            let value = w[1].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            (label == "code" && looks(value)).then(|| value.to_string())
        })
    };
    from_link.or_else(from_text)
}

/// Reads a stream's lines into `tx`.
fn pump(stream: impl AsyncRead + Unpin + Send + 'static, tx: mpsc::UnboundedSender<String>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
}

/// The last thing a program said, for when it failed.
fn last_words(lines: &[String]) -> String {
    let said: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    said.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" · ").chars().take(400).collect()
}

/// Runs a program's own login command. Gives back what to show once its page is known (or once
/// it has waited a while: some programs open the browser themselves and print nothing), and the
/// task that waits for it to finish; `finished` hears how it ended. With `open`, Catbot opens the
/// page itself.
pub async fn sign_in(
    program: &Path,
    args: &[&str],
    env: Vec<(String, String)>,
    open: bool,
    finished: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(Shown, tokio::task::JoinHandle<()>), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env)
        .env("PATH", tool_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|e| format!("couldn't start {}: {e}", program.display()))?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    pump(child.stdout.take().expect("stdout is piped"), tx.clone());
    pump(child.stderr.take().expect("stderr is piped"), tx);

    let mut said: Vec<String> = Vec::new();
    let mut shown = Shown::default();
    let looking = tokio::time::sleep(LINK_WAIT);
    tokio::pin!(looking);
    // The page, and a moment more for a code printed on the next line.
    let mut grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    let mut ended = None;
    loop {
        tokio::select! {
            line = rx.recv() => match line {
                Some(line) => {
                    said.push(line);
                    let text = said.join("\n");
                    if shown.url.is_none() {
                        shown.url = link_in(&text);
                        if shown.url.is_some() {
                            grace = Some(Box::pin(tokio::time::sleep(Duration::from_millis(1500))));
                        }
                    }
                    shown.code = code_in(&text, shown.url.as_deref());
                    if shown.url.is_some() && shown.code.is_some() {
                        break;
                    }
                }
                None => {
                    ended = Some(child.wait().await);
                    break;
                }
            },
            _ = async { grace.as_mut().expect("set").await }, if grace.is_some() => break,
            _ = &mut looking => break,
        }
    }
    if open && let Some(url) = &shown.url {
        open_in_browser(url);
    }
    let task = tokio::spawn(async move {
        let status = match ended {
            Some(status) => status,
            None => {
                // Keep reading, so it never blocks on a full pipe, and keep its last words.
                let reading = async {
                    while let Some(line) = rx.recv().await {
                        said.push(line);
                    }
                    child.wait().await
                };
                match tokio::time::timeout(SIGN_IN_WAIT, reading).await {
                    Ok(status) => status,
                    Err(_) => {
                        finished(Err("it waited ten minutes without a sign-in".into()));
                        return;
                    }
                }
            }
        };
        finished(match status {
            Ok(s) if s.success() => Ok(()),
            Ok(_) => Err(last_words(&said)).map_err(|w| if w.is_empty() { "it stopped without signing in".to_string() } else { w }),
            Err(e) => Err(e.to_string()),
        });
    });
    Ok((shown, task))
}

/// Runs an install line from the program's maker in a shell; `progress` hears each line it prints
/// and `finished` how it ended.
pub fn install(
    line: &str,
    progress: impl Fn(&str) + Send + Sync + 'static,
    finished: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", line]);
        c
    } else {
        let mut c = Command::new("bash");
        c.args(["-c", line]);
        c
    };
    command.env("PATH", tool_path()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|e| format!("couldn't run the installer: {e}"))?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    pump(child.stdout.take().expect("stdout is piped"), tx.clone());
    pump(child.stderr.take().expect("stderr is piped"), tx);
    Ok(tokio::spawn(async move {
        let mut said = Vec::new();
        let run = async {
            while let Some(line) = rx.recv().await {
                progress(&line);
                said.push(line);
            }
            child.wait().await
        };
        finished(match tokio::time::timeout(INSTALL_WAIT, run).await {
            Err(_) => Err("the installer took more than fifteen minutes".into()),
            Ok(Ok(s)) if s.success() => Ok(()),
            Ok(Ok(_)) => Err(match last_words(&said) {
                w if w.is_empty() => "the installer stopped with an error".into(),
                w => format!("the installer stopped: {w}"),
            }),
            Ok(Err(e)) => Err(e.to_string()),
        });
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_and_code_are_read_from_what_it_prints() {
        let text = "Please visit https://auth.kimi.com/device?x=1, then\nenter code: ABCD-1234.";
        assert_eq!(link_in(text).as_deref(), Some("https://auth.kimi.com/device?x=1"));
        assert_eq!(code_in(text, None).as_deref(), Some("ABCD-1234"));
        assert_eq!(code_in("Your one-time code WXYZ-9876", None).as_deref(), Some("WXYZ-9876"));
        assert_eq!(code_in("", Some("https://x.ai/device?user_code=QQ-77")).as_deref(), Some("QQ-77"));
        assert_eq!(code_in("Open the page to sign in with your code", None), None, "a plain word isn't a code");
        assert_eq!(link_in("Opening (https://claude.ai/oauth/authorize?a=b)"), Some("https://claude.ai/oauth/authorize?a=b".into()));
        assert_eq!(link_in("http://localhost:1/x only"), None);
    }

    #[test]
    fn the_newest_node_is_found() {
        assert!(version_of(Path::new("/h/.nvm/versions/node/v22.11.0/bin")) > version_of(Path::new("/h/.nvm/versions/node/v9.99.0/bin")));
    }
}
