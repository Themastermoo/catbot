//! Where the desktop pets can stand besides the bottom of the screen: the tops of the open
//! windows ([[Catbot Desktop Pet]]).
//!
//! - **KDE Plasma** (Wayland or X11): an app can't see other windows under Wayland, so the pet
//!   program loads a small KWin script (over D-Bus, `org.kde.KWin /Scripting`) that sends it
//!   every normal window's frame on the current desktop whenever one moves, opens, closes or is
//!   activated, by calling `Windows` on the pet program's own D-Bus name
//!   (`org.catbot.Pets.p<pid>`). Its coordinates are KWin's (logical); the pet program runs
//!   under X11, whose may be scaled, so they're scaled by the ratio of the two screens' widths.
//! - **Windows**: the system lists the top-level windows itself (`EnumWindows`).
//! - Elsewhere (another Linux desktop): no window tops, only the bottom of the screen.
//!
//! A surface is a window's top edge where nothing above it in the stacking order covers it.

use std::sync::Mutex;

/// A stretch of a window's top edge a pet can stand on, in the pet window's screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    /// The window's own id, to follow it when it moves.
    pub id: String,
    pub left: i32,
    pub right: i32,
    /// The edge's height on the screen (the pet's feet go here).
    pub y: i32,
}

/// A window's frame, bottom of the stacking order first, as a backend reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The latest frames, and the width of the screen they're measured on (0: the pet's own).
static FRAMES: Mutex<Option<(Vec<Frame>, f64)>> = Mutex::new(None);

fn keep(frames: Vec<Frame>, width: f64) {
    *FRAMES.lock().unwrap_or_else(|e| e.into_inner()) = Some((frames, width));
}

/// The surfaces now, for a screen `screen_width` pixels wide (the pet program's), or none if
/// this desktop can't say where its windows are.
pub fn surfaces(screen_width: f64) -> Vec<Surface> {
    #[cfg(windows)]
    windows::refresh();
    let Some((frames, width)) = FRAMES.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return Vec::new() };
    let scale = if width > 0.0 && screen_width > 0.0 { screen_width / width } else { 1.0 };
    let frames: Vec<Frame> = frames
        .into_iter()
        .map(|f| Frame { x: (f.x as f64 * scale) as i32, y: (f.y as f64 * scale) as i32, w: (f.w as f64 * scale) as i32, h: (f.h as f64 * scale) as i32, ..f })
        .collect();
    open_tops(&frames)
}

/// Each window's top edge, less what the windows above it cover; pieces too short for a cat
/// (60 pixels) are left out, and so are edges at the very top of the screen.
pub fn open_tops(frames: &[Frame]) -> Vec<Surface> {
    let mut out = Vec::new();
    for (n, frame) in frames.iter().enumerate() {
        if frame.w < 60 || frame.h < 40 || frame.y < 40 {
            continue;
        }
        let mut pieces = vec![(frame.x, frame.x + frame.w)];
        for above in &frames[n + 1..] {
            // A window above covers the edge where it spans it (with room for the cat above).
            if above.y > frame.y || above.y + above.h < frame.y - 60 {
                continue;
            }
            let (a, b) = (above.x, above.x + above.w);
            pieces = pieces
                .into_iter()
                .flat_map(|(l, r)| {
                    if b <= l || a >= r {
                        vec![(l, r)]
                    } else {
                        [(l, a.max(l)), (b.min(r), r)].into_iter().filter(|(x, y)| y > x).collect()
                    }
                })
                .collect();
        }
        for (left, right) in pieces {
            if right - left >= 60 {
                out.push(Surface { id: frame.id.clone(), left, right, y: frame.y });
            }
        }
    }
    out
}

/// Starts listening for where the windows are, if this desktop can say. (`CATBOT_PET_TOPS`, a
/// report as the KWin script sends it, stands in for the desktop: for trying the pets out.)
pub fn start() {
    #[cfg(target_os = "linux")]
    {
        if let Some((frames, width)) = std::env::var("CATBOT_PET_TOPS").ok().and_then(|r| kde::read(&r)) {
            keep(frames, width);
            return;
        }
        kde::start();
    }
}

#[cfg(target_os = "linux")]
mod kde {
    use super::{Frame, keep};

    /// The KWin script: sends the windows' frames to the pet program on every change.
    const SCRIPT: &str = r#"
const SERVICE = "%SERVICE%";
function all() { return workspace.windowList ? workspace.windowList() : workspace.clientList(); }
function here(w) {
    if (w.onAllDesktops) return true;
    if (w.desktops !== undefined) return w.desktops.indexOf(workspace.currentDesktop) >= 0;
    return w.desktop === workspace.currentDesktop || w.desktop === -1;
}
function send() {
    const out = [];
    const list = all();
    for (let i = 0; i < list.length; i++) {
        const w = list[i];
        if (!w.normalWindow || w.minimized || w.fullScreen || !here(w)) continue;
        if (String(w.caption) === "Catbot pet") continue;
        const g = w.frameGeometry;
        out.push({ id: String(w.internalId), x: Math.round(g.x), y: Math.round(g.y), w: Math.round(g.width), h: Math.round(g.height), max: w.maximized ? 1 : 0 });
    }
    callDBus(SERVICE, "/", "org.catbot.Pets", "Windows", JSON.stringify({ width: workspace.workspaceWidth, windows: out }));
}
function watch(w) {
    if (!w) return;
    w.frameGeometryChanged.connect(send);
    if (w.minimizedChanged) w.minimizedChanged.connect(send);
}
const first = all();
for (let i = 0; i < first.length; i++) watch(first[i]);
(workspace.windowAdded || workspace.clientAdded).connect(function (w) { watch(w); send(); });
(workspace.windowRemoved || workspace.clientRemoved).connect(send);
(workspace.windowActivated || workspace.clientActivated).connect(send);
workspace.currentDesktopChanged.connect(send);
send();
"#;

    /// The plugin name the script is loaded under (one at a time).
    const PLUGIN: &str = "catbot-pets";

    struct Listener;

    #[zbus::interface(name = "org.catbot.Pets")]
    impl Listener {
        /// The script's report: `{ width, windows: [{ id, x, y, w, h, max }] }`.
        fn windows(&self, report: String) {
            if let Some((frames, width)) = read(&report) {
                keep(frames, width);
            }
        }
    }

    /// The script's report as frames (maximised windows left out: their top is the screen's).
    pub fn read(report: &str) -> Option<(Vec<Frame>, f64)> {
        let value: serde_json::Value = serde_json::from_str(report).ok()?;
        let frames = value["windows"]
            .as_array()?
            .iter()
            .filter(|w| w["max"] != 1)
            .filter_map(|w| Some(Frame { id: w["id"].as_str()?.to_string(), x: w["x"].as_i64()? as i32, y: w["y"].as_i64()? as i32, w: w["w"].as_i64()? as i32, h: w["h"].as_i64()? as i32 }))
            .collect();
        Some((frames, value["width"].as_f64().unwrap_or(0.0)))
    }

    /// On KDE Plasma: serves `org.catbot.Pets.p<pid>` and loads the script into KWin. Anything
    /// failing leaves the pets on the bottom of the screen.
    pub fn start() {
        let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_uppercase().contains("KDE")) || std::env::var_os("KDE_FULL_SESSION").is_some();
        if !kde {
            return;
        }
        std::thread::spawn(|| {
            if let Err(err) = zbus::block_on(run()) {
                eprintln!("catbot: the pets can't see the windows (KWin): {err}");
            }
        });
    }

    async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = format!("org.catbot.Pets.p{}", std::process::id());
        let conn = zbus::connection::Builder::session().name(service.clone()).serve_at("/", Listener).build().await?;
        let file = std::env::temp_dir().join(format!("catbot-pets-{}.js", std::process::id()));
        std::fs::write(&file, SCRIPT.replace("%SERVICE%", &service))?;
        // An earlier pet program's script goes first.
        let _ = conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "unloadScript", &(PLUGIN,)).await;
        let path = file.to_string_lossy().to_string();
        let loaded = conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "loadScript", &(path.as_str(), PLUGIN)).await?;
        let id: i32 = loaded.body().deserialize()?;
        // Plasma 6 keeps a script at /Scripting/Script<id>, Plasma 5 at /<id>.
        let mut ran = false;
        for object in [format!("/Scripting/Script{id}"), format!("/{id}")] {
            if conn.call_method(Some("org.kde.KWin"), object.as_str(), Some("org.kde.kwin.Script"), "run", &()).await.is_ok() {
                ran = true;
                break;
            }
        }
        let _ = std::fs::remove_file(&file);
        if !ran {
            return Err("KWin didn't run the script".into());
        }
        // Serving, for as long as the pets are out.
        std::future::pending::<()>().await;
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use super::{Frame, keep};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowRect, GetWindowTextW, IsIconic, IsWindowVisible, IsZoomed, WS_EX_TOOLWINDOW,
    };

    /// Lists the visible top-level windows (top of the stacking order first, as Windows gives
    /// them; kept bottom first).
    pub fn refresh() {
        let mut found: Vec<Frame> = Vec::new();
        unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
            let found = unsafe { &mut *(data as *mut Vec<Frame>) };
            unsafe {
                if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 || IsZoomed(hwnd) != 0 {
                    return 1;
                }
                if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW != 0 {
                    return 1;
                }
                let mut cloaked: u32 = 0;
                if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut _, 4) == 0 && cloaked != 0 {
                    return 1;
                }
                let mut title = [0u16; 64];
                let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
                if n <= 0 || String::from_utf16_lossy(&title[..n as usize]) == "Catbot pet" {
                    return 1;
                }
                let mut rect: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut rect) == 0 {
                    return 1;
                }
                found.push(Frame { id: format!("{:x}", hwnd as usize), x: rect.left, y: rect.top, w: rect.right - rect.left, h: rect.bottom - rect.top });
            }
            1
        }
        unsafe {
            EnumWindows(Some(each), &mut found as *mut Vec<Frame> as LPARAM);
        }
        found.reverse();
        keep(found, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: &str, x: i32, y: i32, w: i32, h: i32) -> Frame {
        Frame { id: id.into(), x, y, w, h }
    }

    #[test]
    fn a_window_above_covers_part_of_the_edge_below() {
        // An editor, and a smaller window over its top edge's middle.
        let tops = open_tops(&[frame("editor", 100, 300, 800, 400), frame("dialog", 400, 250, 200, 200)]);
        assert_eq!(tops, [
            Surface { id: "editor".into(), left: 100, right: 400, y: 300 },
            Surface { id: "editor".into(), left: 600, right: 900, y: 300 },
            Surface { id: "dialog".into(), left: 400, right: 600, y: 250 },
        ]);
        // A window lower down doesn't cover it; tiny windows and the top of the screen don't count.
        assert_eq!(open_tops(&[frame("a", 0, 200, 300, 300), frame("b", 0, 400, 300, 300)]).len(), 2);
        assert!(open_tops(&[frame("tiny", 0, 200, 40, 300), frame("top", 0, 10, 500, 300)]).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_kwin_script_s_report_is_read() {
        let (frames, width) = kde::read(r#"{"width": 1920, "windows": [{"id": "{a}", "x": 10, "y": 200, "w": 600, "h": 400, "max": 0}, {"id": "{b}", "x": 0, "y": 0, "w": 1920, "h": 1040, "max": 1}]}"#).unwrap();
        assert_eq!((frames.len(), width), (1, 1920.0));
        assert_eq!(frames[0], frame("{a}", 10, 200, 600, 400));
    }

    #[test]
    fn frames_are_scaled_to_the_pet_screen() {
        keep(vec![frame("w", 100, 300, 400, 300)], 1000.0);
        assert_eq!(surfaces(2000.0), [Surface { id: "w".into(), left: 200, right: 1000, y: 600 }]);
    }
}
