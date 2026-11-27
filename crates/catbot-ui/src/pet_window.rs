//! The desktop pets themselves: with the Linux or Windows app's window closed, each bot ticked as
//! a pet is a small transparent window that walks about the desktop ([[Catbot Desktop Pet]]).
//!
//! - **When**: closing the main window with pets ticked starts this same program again with
//!   `--pets` (`come_out`), then the app closes as usual. Opening Catbot again (from a pet, or
//!   the app's menu entry) sends them home: the new app ends the pet program it finds in
//!   `pets.pid` (`bring_home`).
//! - **Where**: an ordinary Wayland window can't place itself, so on a Wayland desktop the pet
//!   program runs under X11 (XWayland, which KDE Plasma and GNOME include), where it can. On
//!   Windows and X11 desktops it runs as it is.
//! - **What it does**: walks along the bottom of the screen above the taskbar, stops, sits, now
//!   and then lies down to sleep; stands and bobs while its bot works and hops when the bot
//!   needs the person. Drag it to put it somewhere (it drops back to the ground). Click it for a
//!   small chat beside it (it waves hello); right-click for Chat, Open Catbot and Send home.
//! - **Stage 29.8**: at work it sits at what its bot is doing (a laptop on a desk for code, a
//!   drafting table for design, a book for looking things up); a job done gets a happy hop;
//!   sitting, it washes its face now and then; it yawns before a nap and stretches and yawns on
//!   waking; and when the mouse pointer rests on the ground near it, it crouches, wiggles and
//!   pounces (KDE Plasma and Windows, where the pet can see the pointer).
//! - **Stage 31**: left with nothing to do for a long while it curls up for a proper nap, and
//!   wakes when its bot starts work (or after a good sleep); carried, it dangles by the scruff
//!   and swings behind the hand; standing about, now and then it chases its tail; sitting, it
//!   looks around.
//!
//! One program holds every pet, each its own window; sending the last one home ends it.

use std::path::PathBuf;
use std::time::Duration;

use dioxus::desktop::tao::dpi::{LogicalSize, PhysicalPosition};
use dioxus::desktop::{Config, WindowBuilder};
use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;

/// The pet's window, in logical pixels: the cat and a little air above it.
const PET: (f64, f64) = (104.0, 96.0);
/// The window with its chat open.
const CHAT: (f64, f64) = (340.0, 430.0);
/// Room left at the bottom of the screen for a taskbar or panel (an app can't ask where it is).
const TASKBAR: f64 = 48.0;
/// Walking speed, in logical pixels a second: the art's stride (33 of its 96 units a 0.8 s step)
/// at its drawn size, so the paws don't slide.
const WALK: f64 = 26.0;
/// How often the pet moves, and how often it asks after its bot.
const TICK: Duration = Duration::from_millis(33);
const ASK_EVERY: Duration = Duration::from_secs(4);

/// The styles the pet windows need (the same files as the app's).
const STYLES: Asset = asset!("/assets/tailwind.css");
const MASCOT_STYLES: Asset = asset!("/assets/mascots.css");
const PET_STYLES: Asset = asset!("/assets/pet.css");

/// One pet, as `GET /api/pets` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct PetInfo {
    pub bot_id: String,
    pub name: String,
    pub color: String,
    pub mark: String,
}

impl PetInfo {
    fn from_wire(wire: &Value) -> Option<PetInfo> {
        Some(PetInfo {
            bot_id: wire["botId"].as_str()?.to_string(),
            name: wire["name"].as_str().unwrap_or("Bot").to_string(),
            color: wire["color"].as_str().unwrap_or("blue").to_string(),
            mark: wire["mark"].as_str().unwrap_or("cat-head").to_string(),
        })
    }
}

/// Where the pet program writes its process id, beside the app's other files.
fn pid_file() -> Option<PathBuf> {
    crate::drafts::file()?.parent().map(|dir| dir.join("pets.pid"))
}

/// Lets the pets out: starts this program again with `--pets`. Under Wayland it runs on X11
/// (XWayland), where a window may place itself.
pub fn come_out() {
    let Ok(program) = std::env::current_exe() else { return };
    let mut command = std::process::Command::new(program);
    command.arg("--pets");
    if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        command.env("GDK_BACKEND", "x11");
    }
    if let Err(err) = command.spawn() {
        eprintln!("catbot: couldn't let the pets out: {err}");
    }
}

/// Sends the pets home: ends the pet program, if one is running.
pub fn bring_home() {
    let Some(file) = pid_file() else { return };
    let Some(pid) = std::fs::read_to_string(&file).ok().and_then(|p| p.trim().parse::<u32>().ok()) else { return };
    let _ = std::fs::remove_file(&file);
    if pid == std::process::id() {
        return;
    }
    let ended = if cfg!(windows) {
        std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output()
    } else {
        std::process::Command::new("kill").arg(pid.to_string()).output()
    };
    if ended.is_ok_and(|o| o.status.success()) {
        // Its own Catbot (the Windows app's) goes with it; give it a moment.
        std::thread::sleep(Duration::from_millis(400));
    }
}

/// Opens Catbot's main window again (a fresh app) and ends the pets.
fn open_catbot() {
    if let Ok(program) = std::env::current_exe() {
        let mut command = std::process::Command::new(program);
        command.env_remove("GDK_BACKEND");
        let _ = command.spawn();
    }
    if let Some(file) = pid_file() {
        let _ = std::fs::remove_file(file);
    }
    std::process::exit(0);
}

/// A pet window's settings: small, see-through, undecorated, above the other windows and out
/// of the taskbar.
fn window_config() -> Config {
    let mut window = WindowBuilder::new()
        .with_title("Catbot pet")
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top(true)
        // Resizable, as GTK sizes a fixed-size window to the webview's natural size (200 pixels a
        // side); held to its size by its limits instead (and it has no edges to drag).
        .with_resizable(true)
        .with_inner_size(LogicalSize::new(PET.0, PET.1))
        .with_min_inner_size(LogicalSize::new(PET.0, PET.1))
        .with_max_inner_size(LogicalSize::new(PET.0, PET.1));
    #[cfg(target_os = "linux")]
    {
        use dioxus::desktop::tao::platform::unix::WindowBuilderExtUnix;
        window = window.with_skip_taskbar(true);
    }
    #[cfg(windows)]
    {
        use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
        window = window.with_skip_taskbar(true);
    }
    Config::new()
        .with_window(window)
        .with_menu(None)
        .with_background_color((0, 0, 0, 0))
        .with_custom_head("<style>html,body,#main{background:transparent!important;margin:0;overflow:hidden;user-select:none}</style>".into())
}

/// The pet program: one window per pet. Started by `come_out`.
pub fn launch() {
    if let Some(file) = pid_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, std::process::id().to_string());
    }
    // The Windows app's own Catbot stops with the app that closed; this program starts its own
    // once that one has gone.
    if crate::link::has_own() {
        std::thread::sleep(Duration::from_millis(1500));
        crate::local::start();
    }
    crate::pet_tops::start();
    dioxus::LaunchBuilder::desktop().with_cfg(window_config()).launch(Pets);
}

/// The first window: finds the pets, opens a window for each but the first, and is the first.
#[component]
fn Pets() -> Element {
    let pets = use_resource(|| async {
        let found = api::get("/api/pets").await.ok()?;
        Some(found["pets"].as_array()?.iter().filter_map(PetInfo::from_wire).collect::<Vec<_>>())
    });
    let found = pets.read().clone();
    match found {
        None => rsx! {},
        Some(Some(all)) if !all.is_empty() => {
            let first = all[0].clone();
            let others = all[1..].to_vec();
            rsx! { Opener { others } Pet { pet: first, index: 0 } }
        }
        // No pets (or no Catbot to ask): nothing to do.
        Some(_) => std::process::exit(0),
    }
}

/// Opens the other pets' windows, once.
#[component]
fn Opener(others: Vec<PetInfo>) -> Element {
    use_hook(move || {
        let desktop = dioxus::desktop::window();
        for (n, pet) in others.into_iter().enumerate() {
            let dom = VirtualDom::new_with_props(Pet, PetProps { pet, index: n + 1 });
            let _ = desktop.new_window(dom, window_config());
        }
    });
    rsx! {}
}

/// What the pet is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Walk,
    Idle,
    Sit,
    Sleep,
    /// The long nap (stage 31): curled up after a long while with nothing to do, until its bot
    /// starts work or it has slept enough.
    Nap,
    /// Falling back to the ground after being put down, at this speed (px/s, physical).
    Fall(f64),
    /// Being carried: where it was last tick, and since when it's held still (seconds).
    Carried(i32, i32, f64),
    /// Leaping up onto a window (or across), from one window position to another, `t` of the
    /// way (0..1).
    Leap((i32, i32), (i32, i32), f64),
    /// Landing, for this much longer (seconds).
    Land(f64),
    Chat,
    /// Ready to pounce on the mouse pointer (stage 29.8): this much longer, then at `x`
    /// (physical pixels, where the pointer stands).
    Crouch(f64, i32),
    /// Pouncing, from one window position to another, `t` of the way.
    Pounce((i32, i32), (i32, i32), f64),
}

/// How long a pounce takes, and how often a pet may pounce (seconds).
const POUNCE: f64 = 0.45;
const POUNCE_EVERY: f64 = 25.0;

/// How long a pet is left with nothing to do before the long nap, and how long the nap lasts
/// at least and at most when nothing wakes it (seconds).
const NAP_AFTER: f64 = 300.0;
const NAP_FOR: (f64, f64) = (240.0, 480.0);

/// Whether the long nap is due: its bot idle, nothing playing, on its feet or resting, and
/// `quiet` seconds gone by since anything last happened.
fn nap_due(quiet: f64, now: Act, busy: bool, playing: bool) -> bool {
    !busy && !playing && quiet >= NAP_AFTER && matches!(now, Act::Walk | Act::Idle | Act::Sit | Act::Sleep)
}

/// The little extra a pet does instead of its next move, by `roll` (0..1): sitting, it washes
/// its face or looks around; standing about, now and then it chases its tail.
fn aside(now: Act, roll: f64) -> Option<&'static str> {
    match now {
        Act::Sit if roll < 0.3 => Some("groom"),
        Act::Sit if roll < 0.6 => Some("look"),
        Act::Idle | Act::Walk if roll < 0.12 => Some("chase"),
        _ => None,
    }
}

/// Which way a carried pet swings: -1 when the hand moves it left, 1 right, 0 when it has moved
/// less than `least` pixels since the last look.
fn swing(dx: i32, least: i32) -> i8 {
    if dx.abs() < least.max(1) { 0 } else { dx.signum() as i8 }
}

/// How the swing shows on the picture (`data-lean`): the body trails behind the hand, so going
/// the way the cat faces it leans "back", going the other way "fore".
fn lean_name(swing: i8, facing_left: bool) -> &'static str {
    match (swing, facing_left) {
        (0, _) => "",
        (1, false) | (-1, true) => "back",
        _ => "fore",
    }
}

/// How long a one-shot pose plays: its way in, or two rounds of a loop (the happy hop).
fn play_seconds(dog: bool, pose: &str) -> f64 {
    let first = first_of(dog, pose);
    match anim(dog, &first) {
        Some(a) if a.looped => a.seconds * 2.0,
        Some(a) => a.seconds,
        None => 0.0,
    }
}

/// How long a leap takes, and a landing.
const LEAP: f64 = 0.7;
const LANDING: f64 = 0.71;

/// Where a leap is `t` of the way: straight across, up and over in an arc at least `lift` high.
fn arc(from: (i32, i32), to: (i32, i32), t: f64, lift: f64) -> (i32, i32) {
    let t = t.clamp(0.0, 1.0);
    let x = from.0 as f64 + (to.0 - from.0) as f64 * t;
    let rise = (from.1 - to.1).max(0) as f64 * 0.3 + lift;
    let y = from.1 as f64 + (to.1 - from.1) as f64 * t - 4.0 * rise * t * (1.0 - t);
    (x.round() as i32, y.round() as i32)
}

/// Where a pet stands: a window's top edge (by the window's id), or the bottom of the screen.
#[derive(Clone, Debug, PartialEq)]
struct Perch {
    id: String,
    left: i32,
    right: i32,
    /// Where its feet go.
    feet: i32,
}

/// The first surface under a pet's feet at `x` (its middle), from `feet` down, if any.
fn below(surfaces: &[crate::pet_tops::Surface], x: i32, feet: i32) -> Option<Perch> {
    surfaces
        .iter()
        .filter(|s| s.left <= x && x <= s.right && s.y >= feet)
        .min_by_key(|s| s.y)
        .map(|s| Perch { id: s.id.clone(), left: s.left, right: s.right, feet: s.y })
}

/// A window top within a leap of a pet whose middle is at `x`, feet at `feet`: up to `reach`
/// across and at least 40 up, picked by `roll` (0..1). Gives where to land (the pet's middle) and
/// the surface.
fn leap_to(surfaces: &[crate::pet_tops::Surface], x: i32, feet: i32, reach: i32, half: i32, roll: f64) -> Option<(i32, Perch)> {
    let near: Vec<&crate::pet_tops::Surface> = surfaces
        .iter()
        .filter(|s| s.right - s.left >= half * 2 + 10 && s.y < feet - 40 && feet - s.y < reach * 2 && (x.clamp(s.left + half, s.right - half) - x).abs() <= reach)
        .collect();
    let s = near.get(((near.len() as f64) * roll) as usize % near.len().max(1))?;
    let land = x.clamp(s.left + half, s.right - half);
    Some((land, Perch { id: s.id.clone(), left: s.left, right: s.right, feet: s.y }))
}

/// The pet's pose for its picture (`data-pet` on Fable's cat; the stand-in cat maps it). At
/// work it sits at what its bot is doing (stage 29.8): a laptop, a drafting table or a book.
fn pose(act: Act, mood: &str, doing: Option<&str>) -> &'static str {
    match act {
        Act::Crouch(..) => "crouch",
        Act::Pounce(..) => "jump",
        Act::Walk => "walk",
        Act::Sit => "sit",
        Act::Sleep => "sleep",
        Act::Nap => "nap",
        Act::Leap(..) => "jump",
        Act::Fall(_) => "air",
        Act::Land(_) => "land",
        // Carried by the scruff, swinging (stage 31; `drag` was the quieter hang before it).
        Act::Carried(..) => "dangle",
        Act::Chat | Act::Idle => match mood {
            "working" => match doing {
                Some("coding") => "coding",
                Some("designing") => "designing",
                Some("researching") => "researching",
                _ => "working",
            },
            "waiting" => "waiting",
            _ => "idle",
        },
    }
}

/// A small random number, 0..1, from a running seed (xorshift).
fn chance(seed: &mut u64) -> f64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed % 10_000) as f64 / 10_000.0
}

/// The bottom of the screen the window is on, above the taskbar: (left, right, ground), in
/// physical pixels, the ground being where the window's top goes (for the window as big as it
/// actually is).
fn screen(desktop: &dioxus::desktop::DesktopContext) -> (i32, i32, i32) {
    let window = &desktop.window;
    let monitor = window.current_monitor().or_else(|| window.primary_monitor());
    let Some(monitor) = monitor else { return (0, 1280, 700) };
    let (pos, size, scale) = (monitor.position(), monitor.size(), monitor.scale_factor());
    let ground = pos.y + size.height as i32 - (TASKBAR * scale) as i32 - window.outer_size().height as i32;
    (pos.x, pos.x + size.width as i32, ground)
}

/// How wide the desktop is, across all its screens (what the window tops are measured against).
fn span(desktop: &dioxus::desktop::DesktopContext) -> f64 {
    let monitors: Vec<_> = desktop.window.available_monitors().collect();
    let left = monitors.iter().map(|m| m.position().x).min().unwrap_or(0);
    let right = monitors.iter().map(|m| m.position().x + m.size().width as i32).max().unwrap_or(0);
    (right - left).max(0) as f64
}

/// Sets the window's size, its limits moving with it.
fn resize(desktop: &dioxus::desktop::DesktopContext, size: (f64, f64)) {
    let window = &desktop.window;
    let size = LogicalSize::new(size.0, size.1);
    window.set_min_inner_size(Some(size));
    window.set_max_inner_size(Some(size));
    window.set_inner_size(size);
}

#[component]
fn Pet(pet: PetInfo, index: usize) -> Element {
    // A bot whose picture is a dog comes out as the robot dog (Dogbot).
    let dog = crate::mascots::is_dog(&pet.mark);
    let desktop = dioxus::desktop::window();
    let mut act = use_signal(|| Act::Walk);
    let mut facing_left = use_signal(|| index % 2 == 1);
    let mut mood = use_signal(|| "idle".to_string());
    let mut last = use_signal(|| None::<String>);
    let mut menu = use_signal(|| false);
    let mut text = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let mut pressed = use_signal(|| None::<(f64, f64)>);
    // What its bot is at while it works (stage 29.8).
    let mut doing = use_signal(|| None::<String>);
    // A one-shot pose played over whatever it's doing (a stretch, a yawn, grooming, a wave, the
    // happy hop), until when; and the ones lined up after it.
    let mut flourish = use_signal(|| None::<(&'static str, std::time::Instant)>);
    let mut lined_up = use_signal(Vec::<&'static str>::new);
    // Which way it swings while carried (-1 left, 1 right), for the picture's lean.
    let mut swung = use_signal(|| 0i8);

    // Start on the ground, spread out along the screen.
    let start = desktop.clone();
    use_hook(move || {
        // The app's look (its skin carries the bots' colours), and the pet's own size.
        crate::looks::apply_skin(&crate::looks::load().skin);
        resize(&start, PET);
        let (left, right, ground) = screen(&start);
        let scale = start.window.scale_factor();
        let x = left + ((120.0 + index as f64 * 180.0) * scale) as i32;
        start.window.set_outer_position(PhysicalPosition::new(x.min(right - (PET.0 * scale) as i32), ground));
    });

    // Asking after the bot: whether it's working or needs the person, and its last reply.
    let bot_id = pet.bot_id.clone();
    use_future(move || {
        let bot_id = bot_id.clone();
        async move {
            loop {
                if let Ok(answer) = api::get(&format!("/api/bots/{bot_id}")).await {
                    let bot = &answer["bot"];
                    let now = match bot["activity"].as_str().unwrap_or("idle") {
                        "working" => "working",
                        "waiting-on-you" => "waiting",
                        _ => "idle",
                    };
                    if *mood.peek() != now {
                        // A job done: the happy hop.
                        if *mood.peek() == "working" && now == "idle" {
                            flourish.set(Some(("happy", std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, "happy")))));
                        }
                        mood.set(now.to_string());
                    }
                    let at = bot["doing"].as_str().map(str::to_string);
                    if *doing.peek() != at {
                        doing.set(at);
                    }
                    let reply = bot["messages"].as_array().and_then(|m| m.iter().rev().find(|m| m["role"] == "bot" && m["kind"] == "text")).and_then(|m| m["text"].as_str()).map(crate::bots::plain);
                    if *last.peek() != reply {
                        last.set(reply);
                    }
                }
                crate::events::pause(ASK_EVERY.as_millis() as u32).await;
            }
        }
    });

    // Moving: a step every tick, and now and then a change of mind. The pet stands on the bottom
    // of the screen or on a window's top edge (`pet_tops`), and follows that window if it moves.
    let mover = desktop.clone();
    use_future(move || {
        let desktop = mover.clone();
        async move {
            let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ (index as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
            let mut left_for = 4.0 + chance(&mut seed) * 6.0;
            // Steps go by the time that actually passed (a busy computer ticks late), so the
            // walk keeps pace with the paws.
            let mut ticked = std::time::Instant::now();
            let mut perch: Option<Perch> = None;
            let mut surfaces = Vec::new();
            // Part-pixels walked, carried to the next step.
            let mut walked = 0.0f64;
            let mut looked = 0.0;
            // Seconds since it last pounced (it starts ready, after a little while).
            let mut pounced_ago = POUNCE_EVERY - 8.0;
            // Where the desktop actually put the window when it last landed, for the floor it
            // was asked to stand on: KWin can hold a window a few pixels off where it's asked
            // (by the panel, or rounding a scaled screen), and the pet then "fell" and landed
            // again every second (Moo's screen recording, 2026-11-27).
            let mut placed: Option<(i32, i32)> = None;
            // Seconds with nothing to do (towards the long nap), and how much longer a swing
            // shows after the hand last moved.
            let mut quiet = 0.0f64;
            let mut swing_left = 0.0f64;
            loop {
                crate::events::pause(TICK.as_millis() as u32).await;
                let dt = ticked.elapsed().as_secs_f64().min(0.2);
                ticked = std::time::Instant::now();
                let window = &desktop.window;
                let scale = window.scale_factor();
                let Ok(at) = window.outer_position() else { continue };
                let size = window.outer_size();
                let (w, h) = (size.width as i32, size.height as i32);
                let (left, right, ground) = screen(&desktop);
                // Where the windows are, twice a second.
                looked -= dt;
                if looked <= 0.0 {
                    looked = 0.5;
                    surfaces = crate::pet_tops::surfaces(span(&desktop));
                }
                let now = *act.peek();
                // A bot at work or waiting holds the pet where it stands.
                let busy = *mood.peek() != "idle";
                quiet = if busy || matches!(now, Act::Carried(..) | Act::Chat | Act::Nap) { 0.0 } else { quiet + dt };
                if !matches!(now, Act::Carried(..)) && *swung.peek() != 0 {
                    swung.set(0);
                }
                // A one-shot pose holds it too while it plays; then the next lined up, if any.
                let current_flourish = *flourish.peek();
                let playing = match current_flourish {
                    Some((_, until)) if std::time::Instant::now() < until => true,
                    Some(_) => {
                        let next = {
                            let mut queue = lined_up.write();
                            if queue.is_empty() { None } else { Some(queue.remove(0)) }
                        };
                        flourish.set(next.map(|p| (p, std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, p)))));
                        next.is_some()
                    }
                    None => false,
                };
                // The window it stands on moved or went: follow it, or fall.
                if let Some(p) = perch.clone() {
                    let middle = at.x + w / 2;
                    let same = surfaces.iter().filter(|s| s.id == p.id).min_by_key(|s| (s.y - p.feet).abs() + (s.left - p.left).abs());
                    match same {
                        Some(s) if !matches!(now, Act::Carried(..) | Act::Chat | Act::Leap(..) | Act::Fall(_)) => {
                            let (dx, dy) = (s.left - p.left, s.y - p.feet);
                            if (dx, dy) != (0, 0) && s.right - s.left == p.right - p.left {
                                window.set_outer_position(PhysicalPosition::new(at.x + dx, at.y + dy));
                            } else if middle < s.left || middle > s.right {
                                perch = None;
                                act.set(Act::Fall(0.0));
                                continue;
                            }
                            perch = Some(Perch { id: s.id.clone(), left: s.left, right: s.right, feet: s.y });
                        }
                        Some(_) => {}
                        None => {
                            perch = None;
                            if !matches!(now, Act::Carried(..) | Act::Chat | Act::Leap(..)) {
                                act.set(Act::Fall(0.0));
                                continue;
                            }
                        }
                    }
                }
                let (lo, hi, top) = match &perch {
                    Some(p) => (p.left, p.right, p.feet - h),
                    None => (left, right, ground),
                };
                match now {
                    Act::Chat => continue,
                    Act::Carried(x, y, still) => {
                        perch = None;
                        let held = if (at.x, at.y) == (x, y) { still + dt } else { 0.0 };
                        // It swings behind the hand, and hangs straight again a moment after.
                        let way = swing(at.x - x, (2.0 * scale) as i32);
                        swing_left = if way != 0 { 0.3 } else { swing_left - dt };
                        if (way != 0 || swing_left <= 0.0) && *swung.peek() != way {
                            swung.set(way);
                        }
                        if held > 0.35 && pressed.peek().is_none() {
                            act.set(Act::Fall(0.0));
                        } else {
                            act.set(Act::Carried(at.x, at.y, held));
                        }
                        continue;
                    }
                    Act::Fall(speed) => {
                        let speed = speed + 2400.0 * scale * dt;
                        let y = at.y + (speed * dt) as i32;
                        // Onto the first window top under it, or the ground.
                        let floor = below(&surfaces, at.x + w / 2, at.y + h).filter(|p| p.feet - h <= ground);
                        let stop = floor.as_ref().map(|p| p.feet - h).unwrap_or(ground);
                        if y >= stop {
                            window.set_outer_position(PhysicalPosition::new(at.x, stop));
                            perch = floor;
                            act.set(Act::Land(LANDING));
                            left_for = 1.5;
                        } else {
                            window.set_outer_position(PhysicalPosition::new(at.x, y));
                            act.set(Act::Fall(speed));
                        }
                        continue;
                    }
                    Act::Leap(from, to, t) => {
                        let t = t + dt / LEAP;
                        let (x, y) = arc(from, to, t, 70.0 * scale);
                        window.set_outer_position(PhysicalPosition::new(x, y));
                        act.set(if t >= 1.0 { Act::Land(LANDING) } else { Act::Leap(from, to, t) });
                        continue;
                    }
                    Act::Land(left) => {
                        placed = Some((top, at.y));
                        act.set(if left - dt <= 0.0 { Act::Idle } else { Act::Land(left - dt) });
                        continue;
                    }
                    Act::Crouch(left, x) => {
                        if left - dt > 0.0 {
                            act.set(Act::Crouch(left - dt, x));
                        } else {
                            let to = (x - w / 2).clamp(lo, hi - w);
                            act.set(Act::Pounce((at.x, at.y), (to, top), 0.0));
                        }
                        continue;
                    }
                    Act::Pounce(from, to, t) => {
                        let t = t + dt / POUNCE;
                        let (x, y) = arc(from, to, t, 26.0 * scale);
                        window.set_outer_position(PhysicalPosition::new(x, y));
                        act.set(if t >= 1.0 { Act::Land(LANDING) } else { Act::Pounce(from, to, t) });
                        continue;
                    }
                    _ => {}
                }
                if playing {
                    continue;
                }
                // Not where it should stand (the screen changed, say): down it goes. Where it
                // last landed on this same floor counts as standing on it.
                let floor_y = match placed {
                    Some((asked, got)) if asked == top => got,
                    _ => top,
                };
                if (at.y - top).abs() > 2 && (at.y - floor_y).abs() > 2 {
                    perch = None;
                    act.set(Act::Fall(0.0));
                    continue;
                }
                // Nothing to do for a long while: a yawn, and it curls up for the long nap.
                if nap_due(quiet, now, busy, playing) {
                    flourish.set(Some(("yawn", std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, "yawn")))));
                    left_for = NAP_FOR.0 + chance(&mut seed) * (NAP_FOR.1 - NAP_FOR.0);
                    act.set(Act::Nap);
                    continue;
                }
                // The pointer, still for a moment on the ground ahead: crouch, wiggle, pounce.
                pounced_ago += dt;
                if !busy && pounced_ago > POUNCE_EVERY && matches!(now, Act::Walk | Act::Idle | Act::Sit)
                    && let Some((cx, cy, still)) = crate::pet_tops::cursor(span(&desktop))
                {
                    let middle = at.x + w / 2;
                    let feet = at.y + h;
                    let off = (cx - middle).abs();
                    let near_floor = cy <= feet + (10.0 * scale) as i32 && cy >= feet - (150.0 * scale) as i32;
                    let in_reach = off >= (40.0 * scale) as i32 && off <= (300.0 * scale) as i32 && cx > lo + w / 2 && cx < hi - w / 2;
                    if near_floor && in_reach && still.as_secs_f64() > 0.6 && still.as_secs_f64() < 4.0 {
                        pounced_ago = 0.0;
                        facing_left.set(cx < middle);
                        act.set(Act::Crouch(1.2, cx));
                        continue;
                    }
                }
                left_for -= dt;
                if now == Act::Walk && !busy {
                    walked += WALK * scale * dt;
                    let step = walked.floor() as i32;
                    walked -= step as f64;
                    let mut x = if *facing_left.peek() { at.x - step } else { at.x + step };
                    let edge = x <= lo || x + w >= hi;
                    if edge && perch.is_some() && chance(&mut seed) < 0.35 {
                        // Off the edge of a window: down it hops.
                        perch = None;
                        act.set(Act::Fall(0.0));
                        continue;
                    }
                    if x <= lo {
                        x = lo;
                        facing_left.set(false);
                    } else if x + w >= hi {
                        x = hi - w;
                        facing_left.set(true);
                    }
                    window.set_outer_position(PhysicalPosition::new(x, floor_y));
                }
                // Its bot has started work, or it has slept enough: up from the long nap.
                if now == Act::Nap && (busy || left_for <= 0.0) {
                    flourish.set(Some(("wake", std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, "wake")))));
                    if !busy {
                        lined_up.set(vec!["stretch"]);
                    }
                    left_for = 2.0 + chance(&mut seed) * 3.0;
                    act.set(Act::Idle);
                    continue;
                }
                if busy && now != Act::Idle {
                    act.set(Act::Idle);
                }
                if left_for <= 0.0 && !busy {
                    // What next: mostly walking, sometimes a rest, now and then up onto a window
                    // (or down from one), rarely a nap.
                    let roll = chance(&mut seed);
                    if roll < 0.15 && now != Act::Sleep {
                        let middle = at.x + w / 2;
                        if let Some((land, onto)) = leap_to(&surfaces, middle, at.y + h, (420.0 * scale) as i32, w / 2, chance(&mut seed)) {
                            facing_left.set(land < middle);
                            act.set(Act::Leap((at.x, at.y), (land - w / 2, onto.feet - h), 0.0));
                            perch = Some(onto);
                            left_for = 3.0 + chance(&mut seed) * 4.0;
                            continue;
                        }
                        if perch.is_some() {
                            perch = None;
                            act.set(Act::Fall(0.0));
                            continue;
                        }
                    }
                    // Sitting, now and then it washes its face instead (stage 29.8) or looks around;
                    // standing about, it may chase its tail (stage 31).
                    if let Some(extra) = aside(now, chance(&mut seed)) {
                        flourish.set(Some((extra, std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, extra)))));
                        left_for = play_seconds(dog, extra) + 1.0 + chance(&mut seed) * 3.0;
                        if now == Act::Walk {
                            act.set(Act::Idle);
                        }
                        continue;
                    }
                    let next = match now {
                        Act::Sleep if roll < 0.7 => Act::Sleep,
                        _ if roll < 0.6 => Act::Walk,
                        _ if roll < 0.82 => Act::Idle,
                        _ if roll < 0.95 => Act::Sit,
                        _ => Act::Sleep,
                    };
                    // A yawn before a nap; on waking, a long stretch and a yawn.
                    if next == Act::Sleep && now != Act::Sleep {
                        flourish.set(Some(("yawn", std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, "yawn")))));
                    } else if now == Act::Sleep && next != Act::Sleep {
                        flourish.set(Some(("stretch", std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, "stretch")))));
                        lined_up.set(vec!["yawn"]);
                    }
                    if next == Act::Walk && chance(&mut seed) < 0.4 {
                        let turned = !*facing_left.peek();
                        facing_left.set(turned);
                    }
                    left_for = match next {
                        Act::Walk => 3.0 + chance(&mut seed) * 7.0,
                        Act::Sleep => 20.0 + chance(&mut seed) * 40.0,
                        _ => 2.0 + chance(&mut seed) * 5.0,
                    };
                    act.set(next);
                }
            }
        }
    });

    let mut open_chat = {
        let desktop = desktop.clone();
        move || {
            let window = &desktop.window;
            let scale = window.scale_factor();
            let Ok(at) = window.outer_position() else { return };
            let (_, right, _) = screen(&desktop);
            let grow = ((CHAT.0 - PET.0) * scale) as i32;
            let up = ((CHAT.1 - PET.1) * scale) as i32;
            // The chat opens to the right of the cat, or to its left near the screen's edge.
            let to_left = at.x + (CHAT.0 * scale) as i32 > right;
            resize(&desktop, CHAT);
            window.set_outer_position(PhysicalPosition::new(if to_left { at.x - grow } else { at.x }, at.y - up));
            facing_left.set(to_left);
            // Hello (up from the long nap first, if it was having one).
            let hello = if *act.peek() == Act::Nap {
                lined_up.set(vec!["wave"]);
                "wake"
            } else {
                "wave"
            };
            act.set(Act::Chat);
            flourish.set(Some((hello, std::time::Instant::now() + Duration::from_secs_f64(play_seconds(dog, hello)))));
        }
    };
    let close_chat = {
        let desktop = desktop.clone();
        move || {
            let window = &desktop.window;
            let scale = window.scale_factor();
            let Ok(at) = window.outer_position() else { return };
            let grow = if *facing_left.peek() { ((CHAT.0 - PET.0) * scale) as i32 } else { 0 };
            let up = ((CHAT.1 - PET.1) * scale) as i32;
            resize(&desktop, PET);
            window.set_outer_position(PhysicalPosition::new(at.x + grow, at.y + up));
            menu.set(false);
            act.set(Act::Idle);
        }
    };

    let send_id = pet.bot_id.clone();
    let mut send = move || {
        let words = text.peek().trim().to_string();
        if words.is_empty() || *sending.peek() {
            return;
        }
        let id = send_id.clone();
        sending.set(true);
        spawn(async move {
            if api::post(&format!("/api/bots/{id}/messages"), &json!({ "text": words })).await.is_ok() {
                text.set(String::new());
                mood.set("working".into());
            }
            sending.set(false);
        });
    };

    let current = act();
    let chatting = current == Act::Chat;
    let shown_pose = match flourish() {
        Some((p, until)) if std::time::Instant::now() < until && !matches!(current, Act::Carried(..) | Act::Fall(_) | Act::Leap(..) | Act::Pounce(..)) => p,
        _ => pose(current, &mood(), doing().as_deref()),
    };
    let flip = if facing_left() { "transform: scaleX(-1)" } else { "" };
    let lean = lean_name(swung(), facing_left());
    let name = pet.name.clone();
    let drag_window = desktop.clone();
    let mut toggle = {
        let mut open_chat = open_chat.clone();
        let mut close_chat = close_chat.clone();
        move || if *act.peek() == Act::Chat { close_chat() } else { open_chat() }
    };
    let mut close_for_menu = close_chat.clone();
    let mut send_for_key = send.clone();
    rsx! {
        document::Stylesheet { href: STYLES }
        document::Stylesheet { href: MASCOT_STYLES }
        document::Stylesheet { href: PET_STYLES }
        div { class: "flex h-screen w-screen flex-col justify-end", "data-pet-window": "{pet.bot_id}",
            if chatting {
                // min-h-0 (here and on the card): a long reply scrolls inside the card instead of
                // growing it past the top of the window, where it was cut off (Moo, 2026-11-27).
                div { class: if facing_left() { "mx-2 mb-1 mt-2 flex min-h-0 flex-1 flex-col justify-end items-end" } else { "mx-2 mb-1 mt-2 flex min-h-0 flex-1 flex-col justify-end items-start" },
                    div { class: "flex max-h-full min-h-0 w-[300px] flex-col overflow-hidden rounded-2xl bg-panel text-ink shadow-xl ring-1 ring-ink/10", "data-pet-chat": true,
                        div { class: "flex shrink-0 items-center gap-2 px-3 pt-2.5",
                            span { class: "min-w-0 flex-1 truncate text-[13px] font-semibold", "{name}" }
                            span { class: "text-[12px] text-ink-tertiary",
                                match mood().as_str() { "working" => "Working…", "waiting" => "Needs you", _ => "" }
                            }
                            button { r#type: "button", "aria-label": "Close", class: "flex size-6 items-center justify-center rounded-full text-ink-secondary hover:bg-ink/[0.06]", onclick: move |_| close_for_menu(), crate::sidebar::Icon { name: "x", size: 13 } }
                        }
                        div { class: "min-h-0 flex-1 overflow-y-auto px-3 py-2 text-[13px] leading-[19px] text-ink whitespace-pre-wrap",
                            match last() {
                                Some(reply) => rsx! { "{reply}" },
                                None => rsx! { span { class: "text-ink-tertiary", "Nothing from {name} yet." } },
                            }
                        }
                        if menu() {
                            div { class: "shrink-0 border-t border-hairline/50 p-1",
                                button { r#type: "button", class: "flex w-full rounded-lg px-2.5 py-1.5 text-left text-[13px] hover:bg-ink/[0.05]", onclick: move |_| open_catbot(), "Open Catbot" }
                                button { r#type: "button", "data-pet-home": true, class: "flex w-full rounded-lg px-2.5 py-1.5 text-left text-[13px] hover:bg-ink/[0.05]", onclick: move |_| dioxus::desktop::window().close(), "Send {name} home" }
                            }
                        }
                        div { class: "flex shrink-0 items-center gap-1.5 border-t border-hairline/50 p-2",
                            input {
                                class: "min-w-0 flex-1 rounded-full bg-ink/[0.05] px-3 py-1.5 text-[13px] text-ink placeholder:text-ink-tertiary focus:outline-none",
                                placeholder: "Message {name}",
                                value: "{text}",
                                autofocus: true,
                                oninput: move |e| text.set(e.value()),
                                onkeydown: move |e| if e.key() == Key::Enter { send_for_key() },
                            }
                            button { r#type: "button", class: "rounded-full bg-accent px-3 py-1.5 text-[12px] font-medium text-white disabled:opacity-40", disabled: sending() || text().trim().is_empty(), onclick: move |_| send(), "Send" }
                            button { r#type: "button", title: "Open Catbot", "aria-label": "Open Catbot", class: "flex size-7 items-center justify-center rounded-full text-ink-secondary hover:bg-ink/[0.06]", onclick: move |_| open_catbot(), crate::sidebar::Icon { name: "monitor", size: 14 } }
                        }
                    }
                }
            }
            div { class: if facing_left() { "flex justify-end" } else { "flex justify-start" },
                div {
                    class: "relative h-[88px] w-[104px] cursor-grab",
                    style: "{flip}",
                    "data-pet-cat": true,
                    title: "{name}",
                    onmousedown: move |e| {
                        if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Primary) {
                            let p = e.screen_coordinates();
                            pressed.set(Some((p.x, p.y)));
                        }
                    },
                    onmousemove: move |e| {
                        let Some((x, y)) = *pressed.peek() else { return };
                        let p = e.screen_coordinates();
                        if (p.x - x).abs() + (p.y - y).abs() > 5.0 && *act.peek() != Act::Chat {
                            pressed.set(None);
                            if let Ok(at) = drag_window.window.outer_position() {
                                act.set(Act::Carried(at.x, at.y, 0.0));
                            }
                            drag_window.drag();
                        }
                    },
                    onmouseup: move |_| {
                        if pressed.peek().is_some() {
                            pressed.set(None);
                            toggle();
                        }
                    },
                    oncontextmenu: move |e| {
                        e.prevent_default();
                        menu.set(true);
                        if *act.peek() != Act::Chat {
                            open_chat();
                        }
                    },
                    // The lean turns the picture about the scruff while it's carried (pet.css).
                    div { class: "pet-lean", "data-lean": "{lean}",
                        PetSprite { color: pet.color.clone(), pose: shown_pose.to_string(), name: pet.name.clone(), dog }
                    }
                    // Asleep for the long nap: three small z's drift up (turned back the right
                    // way round when the cat faces left).
                    if shown_pose == "nap" {
                        div { class: "pet-nap-zzz", "aria-hidden": "true", style: "color: var(--identity-{pet.color}); {flip}",
                            span { "z" } span { "z" } span { "z" }
                        }
                    }
                }
            }
        }
    }
}

/// The pet cat (stage 25): Fable's robot cat, built and rendered in Blender by
/// `art/pet3d/build_pet.py` into sprite sheets (`assets/pet3d/`). Each animation is a strip of
/// frames in three passes: the cat with a grey shell, the shell's mask (coloured with the bot's
/// colour by multiply) and the shell's gloss (screened back on top). `assets/pet.css` steps
/// through the strip.
const PET3D: Asset = crate::cat_anim::PET3D;
const MANIFEST: &str = include_str!("../assets/pet3d/manifest.json");
/// The robot dog (Dogbot): the same script's dog, in `assets/pet3d/dog/`, with its own list. A
/// bot whose picture is a dog (`mascots::is_dog`) walks the desktop as the dog.
const DOG_MANIFEST: &str = include_str!("../assets/pet3d/dog/manifest.json");

/// The sheets' list: the robot cat's, or the robot dog's.
fn manifest(dog: bool) -> &'static Value {
    static CAT: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    static DOG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let (parsed, text) = if dog { (&DOG, DOG_MANIFEST) } else { (&CAT, MANIFEST) };
    parsed.get_or_init(|| serde_json::from_str(text).unwrap_or(Value::Null))
}

/// Where a pet's sheets are: the cat's in the folder itself, the dog's in `dog/` inside it.
fn sheets(dog: bool) -> String {
    if dog { format!("{PET3D}/dog") } else { PET3D.to_string() }
}

/// One animation: how many frames, at what speed, whether it loops, and what follows it.
#[derive(Clone, Debug, PartialEq)]
struct Anim {
    frames: u32,
    seconds: f64,
    looped: bool,
    next: Option<String>,
}

fn anim(dog: bool, name: &str) -> Option<Anim> {
    let a = &manifest(dog)["animations"][name];
    let frames = a["frames"].as_u64()? as u32;
    let fps = a["fps"].as_f64().filter(|f| *f > 0.0)?;
    // Walking: the strip plays at the speed the window moves, so the paws don't slide.
    let seconds = match (name, manifest(dog)["walk"]["stride_css_px"].as_f64()) {
        ("walk", Some(stride)) if stride > 0.0 => stride / WALK,
        _ => frames as f64 / fps,
    };
    Some(Anim { frames, seconds, looped: a["loop"] != false, next: a["next"].as_str().map(str::to_string) })
}

/// What plays when the pose becomes `pose` while `now` is playing: a pose named for its own loop
/// (sit, idle, nap) that is already playing carries on, as when a one-shot (washing, a look
/// around, waking) has just led back into it; anything else starts from its way in.
fn start_of(dog: bool, pose: &str, now: &str) -> String {
    if pose == now && manifest(dog)["poses"][pose]["animation"].as_str() == Some(now) { now.to_string() } else { first_of(dog, pose) }
}

/// What a pose plays first: its one-shot way in, if it has one, else its loop.
fn first_of(dog: bool, pose: &str) -> String {
    let p = &manifest(dog)["poses"][pose];
    p["intro"].as_str().or(p["animation"].as_str()).filter(|n| anim(dog, n).is_some()).unwrap_or(if anim(dog, pose).is_some() { pose } else { "idle" }).to_string()
}

#[component]
fn PetSprite(color: String, pose: String, name: String, #[props(default)] dog: bool) -> Element {
    let mut playing = use_signal(|| first_of(dog, &pose));
    use_effect(use_reactive!(|pose| {
        let next = start_of(dog, &pose, &playing.peek());
        playing.set(next);
    }));
    let now = playing();
    let Some(a) = anim(dog, &now) else { return rsx! {} };
    let folder = sheets(dog);
    let style = format!(
        "--frames: {}; --dur: {:.3}s; --tint: var(--identity-{color}); --base: url({folder}/base_{now}.png); --mask: url({folder}/mask_{now}.png); --gloss: url({folder}/gloss_{now}.png)",
        a.frames, a.seconds
    );
    let strip = if a.looped { "pet3d-strip pet3d-loop" } else { "pet3d-strip pet3d-once" };
    let next = a.next.clone();
    let ground = manifest(dog)["ground_css_y"].as_f64().map(|y| 80.0 - y).unwrap_or(0.0);
    rsx! {
        div {
            class: "pet3d absolute bottom-0 left-1/2",
            style: "{style}; transform: translate(-50%, {ground:.1}px)",
            "data-pet": "{pose}",
            "data-pet-kind": if dog { "dog" } else { "cat" },
            "data-playing": "{now}",
            "aria-label": "{name}",
            // Keyed, so a new animation starts from its first frame.
            for shown in [now.clone()] {
                div {
                    key: "{shown}",
                    class: strip,
                    onanimationend: {
                        let next = next.clone();
                        move |_| if let Some(next) = next.clone() { playing.set(next) }
                    },
                    div { class: "pet3d-base" }
                    div { class: "pet3d-tint" }
                    div { class: "pet3d-gloss" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_long_nap_comes_after_a_long_quiet() {
        assert!(!nap_due(NAP_AFTER - 1.0, Act::Idle, false, false), "not yet");
        for resting in [Act::Walk, Act::Idle, Act::Sit, Act::Sleep] {
            assert!(nap_due(NAP_AFTER, resting, false, false), "{resting:?}");
        }
        assert!(!nap_due(NAP_AFTER * 2.0, Act::Idle, true, false), "never while its bot works or waits");
        assert!(!nap_due(NAP_AFTER * 2.0, Act::Idle, false, true), "a one-shot pose finishes first");
        for held in [Act::Carried(0, 0, 0.0), Act::Chat, Act::Fall(0.0), Act::Nap, Act::Crouch(1.0, 0)] {
            assert!(!nap_due(NAP_AFTER * 2.0, held, false, false), "{held:?}");
        }
        assert_eq!(pose(Act::Nap, "idle", None), "nap");
        // It curls up on the way in, sleeps in a loop, and waking leads to standing.
        assert_eq!(first_of(false, "nap"), "curl_up");
        assert_eq!(anim(false, "curl_up").unwrap().next.as_deref(), Some("nap"));
        assert!(anim(false, "nap").unwrap().looped);
        assert_eq!(anim(false, "wake").unwrap().next.as_deref(), Some("idle"));
        assert!(NAP_FOR.0 < NAP_FOR.1);
    }

    #[test]
    fn a_carried_pet_dangles_and_swings_behind_the_hand() {
        assert_eq!(pose(Act::Carried(10, 10, 0.2), "idle", None), "dangle");
        assert!(anim(false, "dangle").unwrap().looped);
        assert_eq!((swing(9, 4), swing(-9, 4), swing(3, 4), swing(0, 0)), (1, -1, 0, 0));
        // Going the way it faces, the body trails back; the other way, it swings fore.
        assert_eq!(lean_name(1, false), "back");
        assert_eq!(lean_name(-1, true), "back");
        assert_eq!(lean_name(-1, false), "fore");
        assert_eq!(lean_name(1, true), "fore");
        assert_eq!(lean_name(0, true), "");
    }

    #[test]
    fn idle_extras_fit_what_the_pet_is_doing() {
        assert_eq!(aside(Act::Sit, 0.1), Some("groom"));
        assert_eq!(aside(Act::Sit, 0.45), Some("look"));
        assert_eq!(aside(Act::Sit, 0.9), None);
        assert_eq!(aside(Act::Idle, 0.05), Some("chase"));
        assert_eq!(aside(Act::Walk, 0.05), Some("chase"));
        assert_eq!(aside(Act::Idle, 0.5), None);
        assert_eq!(aside(Act::Sleep, 0.0), None);
        assert_eq!(aside(Act::Nap, 0.0), None);
        // Both one-shots lead back to where they began.
        assert_eq!(anim(false, &first_of(false, "chase")).unwrap().next.as_deref(), Some("idle"));
        assert_eq!(anim(false, &first_of(false, "look")).unwrap().next.as_deref(), Some("sit"));
    }

    #[test]
    fn a_pet_does_what_its_bot_does() {
        assert_eq!(pose(Act::Walk, "idle", None), "walk");
        assert_eq!(pose(Act::Idle, "working", None), "working");
        assert_eq!(pose(Act::Chat, "waiting", None), "waiting");
        assert_eq!(pose(Act::Carried(0, 0, 0.0), "working", None), "dangle");
        let mut seed = 7;
        let rolls: Vec<f64> = (0..100).map(|_| chance(&mut seed)).collect();
        assert!(rolls.iter().all(|r| (0.0..1.0).contains(r)));
        assert!(rolls.iter().any(|r| *r < 0.2) && rolls.iter().any(|r| *r > 0.8), "spread out");
        assert_eq!(pose(Act::Leap((0, 0), (1, 1), 0.5), "idle", None), "jump");
        // A leap starts and ends where it should and rises in between.
        assert_eq!(arc((0, 500), (200, 300), 0.0, 70.0), (0, 500));
        assert_eq!(arc((0, 500), (200, 300), 1.0, 70.0), (200, 300));
        assert!(arc((0, 500), (200, 300), 0.5, 70.0).1 < 300, "over the top");
        let tops = vec![
            crate::pet_tops::Surface { id: "a".into(), left: 100, right: 600, y: 400 },
            crate::pet_tops::Surface { id: "b".into(), left: 300, right: 900, y: 200 },
        ];
        assert_eq!(below(&tops, 350, 100).unwrap().id, "b", "the first top under it");
        assert_eq!(below(&tops, 150, 300).unwrap().id, "a");
        assert_eq!(below(&tops, 950, 100), None);
        let (land, onto) = leap_to(&tops, 150, 700, 420, 50, 0.0).unwrap();
        assert_eq!((onto.id.as_str(), land), ("a", 150));
        assert_eq!(leap_to(&tops, 2000, 700, 420, 50, 0.0), None, "too far");
        assert_eq!(pose(Act::Fall(0.0), "idle", None), "air");
        // At work, at what its bot is doing; ready to pounce, crouched (stage 29.8).
        assert_eq!(pose(Act::Idle, "working", Some("coding")), "coding");
        assert_eq!(pose(Act::Chat, "working", Some("researching")), "researching");
        assert_eq!(pose(Act::Idle, "idle", Some("coding")), "idle");
        assert_eq!(pose(Act::Crouch(1.0, 0), "idle", None), "crouch");
        // The robot cat's sheets: every pose has something to play, and one-shots lead on.
        for p in ["idle", "walk", "sit", "sleep", "working", "waiting", "jump", "air", "land", "drag",
                  "happy", "stretch", "groom", "yawn", "wave", "crouch", "coding", "designing", "researching",
                  "nap", "wake", "dangle", "chase", "look"] {
            assert!(anim(false, &first_of(false, p)).is_some(), "{p}");
        }
        assert_eq!(first_of(false, "sit"), "sit_down");
        assert_eq!(anim(false, "sit_down").unwrap().next.as_deref(), Some("sit"));
        assert!(anim(false, "walk").unwrap().looped && !anim(false, "land").unwrap().looped);
        // A one-shot that led back into the loop doesn't sit down (or lie down) all over again.
        assert_eq!(start_of(false, "sit", "sit"), "sit");
        assert_eq!(start_of(false, "sit", "idle"), "sit_down");
        assert_eq!(start_of(false, "look", "sit"), "look_around", "a look around still plays from sitting");
        assert_eq!(start_of(false, "idle", "idle"), "idle");
        assert_eq!(PetInfo::from_wire(&json!({ "botId": "b1", "name": "Kiwi", "color": "purple", "mark": "cat-visor" })).unwrap().mark, "cat-visor");
    }

    #[test]
    fn a_dog_bot_walks_the_desktop_as_the_dog() {
        // Its own sheets, in their own folder, listed by its own manifest.
        assert_eq!(manifest(true)["species"].as_str(), Some("dog"));
        assert!(sheets(true).ends_with("/dog") && !sheets(false).ends_with("/dog"));
        // Every pose the window can ask for has something to play, as the cat's do; one-shots lead on.
        for p in ["idle", "walk", "sit", "sleep", "working", "waiting", "jump", "air", "land", "drag",
                  "happy", "stretch", "groom", "yawn", "wave", "crouch", "coding", "designing", "researching",
                  "nap", "wake", "dangle", "chase", "look"] {
            let first = first_of(true, p);
            let a = anim(true, &first).unwrap_or_else(|| panic!("{p}"));
            if let Some(next) = &a.next {
                assert!(anim(true, next).is_some_and(|n| n.looped), "{p}: {first} leads to {next}");
            }
            assert!(play_seconds(true, p) > 0.0, "{p}");
        }
        // Where the cat chases its tail and washes, the dog bows to play and cocks its head.
        assert_eq!((first_of(true, "chase"), first_of(true, "groom")), ("play_bow".to_string(), "head_tilt".to_string()));
        assert_eq!((first_of(false, "chase"), first_of(false, "groom")), ("tail_chase".to_string(), "groom".to_string()));
        assert_eq!(anim(true, "head_tilt").unwrap().next.as_deref(), Some("sit"));
        // It trots at the speed the window moves, so its paws don't slide either.
        let stride = manifest(true)["walk"]["stride_css_px"].as_f64().unwrap();
        assert!((anim(true, "walk").unwrap().seconds - stride / WALK).abs() < 1e-9);
        // The two have the same frame and stand on the same ground.
        assert_eq!(manifest(true)["frame"], manifest(false)["frame"]);
        let ground = |dog: bool| manifest(dog)["ground_css_y"].as_f64().unwrap();
        assert!((ground(true) - ground(false)).abs() < 0.25, "within a quarter of a pixel");
    }
}
