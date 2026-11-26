//! The tray (stage 29.2): on the Windows app and the Linux packages, which run their own Catbot,
//! closing the window hides it and the bots keep working. A Catbot icon by the clock has:
//!
//! - **Open Catbot**: the window again (and the desktop pets go home);
//! - **Start when I log in**: Catbot starts hidden, in the tray, when the person logs in;
//! - **Quit Catbot**: the app and its Catbot stop (scheduled bots, email and phones with them).
//!
//! Only one app runs at a time: opening Catbot again (the menu, a desktop pet, the Start menu)
//! asks the one already running to show its window, through a door on this computer only
//! (127.0.0.1:8787), and ends.
//!
//! Moo's choice, 2026-11-25: "Keep running in the tray".

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dioxus::desktop::trayicon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::{DioxusTrayIcon, init_tray_icon};
use dioxus::prelude::*;

/// Where the running app listens for a second one.
const APP_DOOR: &str = "127.0.0.1:8787";
/// Set when a second app asked for the window.
static SHOW: AtomicBool = AtomicBool::new(false);

/// Whether this app keeps to the tray: when it runs its own Catbot.
pub fn wanted() -> bool {
    crate::link::has_own()
}

/// Started at login: hidden, in the tray.
pub fn hidden_start() -> bool {
    std::env::args().any(|a| a == "--hidden")
}

/// Another Catbot app is open: asks it to show its window. True if one answered.
pub fn hand_to_running() -> bool {
    let Ok(addr) = APP_DOOR.parse() else { return false };
    match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
        Ok(mut door) => {
            let _ = door.write_all(b"show\n");
            true
        }
        Err(_) => false,
    }
}

/// Listens for later launches, for as long as the app runs.
pub fn listen() {
    let Ok(listener) = std::net::TcpListener::bind(APP_DOOR) else {
        eprintln!("catbot: couldn't open {APP_DOOR}, so opening Catbot again may start a second window");
        return;
    };
    std::thread::spawn(move || {
        for door in listener.incoming().flatten() {
            let mut line = String::new();
            let _ = BufReader::new(door.take(16)).read_line(&mut line);
            if line.trim() == "show" {
                SHOW.store(true, Ordering::Relaxed);
            }
        }
    });
}

/// The tray icon and its menu, for the life of the app.
pub fn use_tray() {
    let window = use_hook(dioxus::desktop::window);
    let login = use_hook(|| {
        let menu = Menu::new();
        let open = MenuItem::with_id("catbot-open", "Open Catbot", true, None);
        let login = CheckMenuItem::with_id("catbot-login", "Start when I log in", true, autostart::is_on(), None);
        let quit = MenuItem::with_id("catbot-quit", "Quit Catbot", true, None);
        if let Err(err) = menu.append_items(&[&open, &login, &PredefinedMenuItem::separator(), &quit]) {
            eprintln!("catbot: the tray menu: {err}");
        }
        let icon = dioxus::desktop::icon_from_memory::<DioxusTrayIcon>(include_bytes!("../assets/icon/catbot-64.png")).ok();
        init_tray_icon(menu, icon);
        login
    });
    let show = {
        let window = window.clone();
        move || {
            window.set_visible(true);
            window.set_minimized(false);
            window.set_focus();
            crate::pet_window::bring_home();
        }
    };
    let on_menu = show.clone();
    dioxus::desktop::use_tray_menu_event_handler(move |event| match event.id.0.as_str() {
        "catbot-open" => on_menu(),
        "catbot-login" => {
            let on = !autostart::is_on();
            if let Err(err) = autostart::set(on) {
                eprintln!("catbot: couldn't change starting at login: {err}");
            }
            login.set_checked(autostart::is_on());
        }
        "catbot-quit" => {
            crate::pet_window::bring_home();
            std::process::exit(0);
        }
        _ => {}
    });
    // A second launch asked for the window.
    use_future(move || {
        let show = show.clone();
        async move {
            loop {
                crate::events::pause(400).await;
                if SHOW.swap(false, Ordering::Relaxed) {
                    show();
                }
            }
        }
    });
}

/// Starting at login: the desktop's autostart entry on Linux, the Run key on Windows. Either
/// starts Catbot with `--hidden`.
pub mod autostart {
    use std::path::PathBuf;

    /// What starts Catbot: the AppImage file itself, the Windows launcher, or this program.
    fn program() -> Option<PathBuf> {
        if let Some(image) = std::env::var_os("APPIMAGE").filter(|a| !a.is_empty()) {
            return Some(image.into());
        }
        if let Some(dir) = std::env::var_os("CATBOT_INSTALL_DIR").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir).join("Catbot.exe"));
        }
        std::env::current_exe().ok()
    }

    #[cfg(not(windows))]
    fn entry() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(PathBuf::from);
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(config.unwrap_or_else(|| home.join(".config")).join("autostart").join("catbot.desktop"))
    }

    #[cfg(not(windows))]
    pub fn is_on() -> bool {
        entry().is_some_and(|e| e.is_file())
    }

    #[cfg(not(windows))]
    pub fn set(on: bool) -> std::io::Result<()> {
        let entry = entry().ok_or_else(|| std::io::Error::other("no home folder"))?;
        if !on {
            return match std::fs::remove_file(&entry) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            };
        }
        let program = program().ok_or_else(|| std::io::Error::other("can't tell where Catbot is"))?;
        if let Some(dir) = entry.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Quoted for the Exec line; % is the entry format's escape.
        let exec = program.to_string_lossy().replace('%', "%%").replace('"', "\\\"");
        std::fs::write(
            &entry,
            format!("[Desktop Entry]\nType=Application\nName=Catbot\nComment=Starts Catbot in the tray\nExec=\"{exec}\" --hidden\nIcon=catbot\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"),
        )
    }

    #[cfg(windows)]
    const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

    #[cfg(windows)]
    fn reg(args: &[&str]) -> std::io::Result<bool> {
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("reg").args(args).creation_flags(0x0800_0000).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status()?;
        Ok(status.success())
    }

    #[cfg(windows)]
    pub fn is_on() -> bool {
        reg(&["query", RUN, "/v", "Catbot"]).unwrap_or(false)
    }

    #[cfg(windows)]
    pub fn set(on: bool) -> std::io::Result<()> {
        if !on {
            reg(&["delete", RUN, "/v", "Catbot", "/f"])?;
            return Ok(());
        }
        let program = program().ok_or_else(|| std::io::Error::other("can't tell where Catbot is"))?;
        let value = format!("\"{}\" --hidden", program.display());
        if reg(&["add", RUN, "/v", "Catbot", "/t", "REG_SZ", "/d", &value, "/f"])? { Ok(()) } else { Err(std::io::Error::other("reg add refused")) }
    }

    #[cfg(all(test, not(windows)))]
    mod tests {
        #[test]
        fn starting_at_login_is_an_autostart_entry() {
            let dir = tempfile_dir();
            // SAFETY: this test alone uses XDG_CONFIG_HOME.
            unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
            assert!(!super::is_on());
            super::set(true).unwrap();
            assert!(super::is_on());
            let entry = std::fs::read_to_string(dir.join("autostart/catbot.desktop")).unwrap();
            assert!(entry.contains("--hidden") && entry.contains("Exec=\""), "{entry}");
            super::set(false).unwrap();
            assert!(!super::is_on());
            super::set(false).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
        }

        fn tempfile_dir() -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!("catbot-autostart-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }
    }
}
