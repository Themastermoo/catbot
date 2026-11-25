//! Catbot's bot pictures: three original cats, each filled with the bot's identity colour shaded
//! light to dark, and the letter circle. Moo kept all four as choices (the Codex note
//! "2026-11-14 Catbot bots choose their mascot").
//!
//! The cats were drawn for Catbot (viewBox 0 0 64 64, legible from 16px up). None of them uses
//! OpenMausBot's mascot shapes.
//!
//! They are alive: the body moves by state, the eyes blink, glance about and follow the pointer.
//! All of that runs in the page (`assets/mascots.css` and `MASCOT_LIFE` below), never as a Rust
//! render per frame. The motion approach is after OpenMausBot's mascot-motion.ts, CursorAvatar.tsx
//! and Avatar.tsx (Apache-2.0, see NOTICE); the groups here (`cat-blink`, `cat-look`,
//! `cat-glance`) only give that motion something to hold.

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::prelude::*;

/// The four pictures, as stored on a bot.
pub const MARKS: [(&str, &str); 6] =
    [("cat-head", "Cat head"), ("cat-sitting", "Sitting cat"), ("cat-visor", "Visor cat"), ("robot-head", "Robot cat head"), ("robot-full", "Robot cat"), ("letter", "Letter")];

/// The colours a bot can wear, as the stylesheet names them.
pub const COLORS: [&str; 12] = ["green", "blue", "red", "orange", "purple", "cyan", "pink", "yellow", "teal", "coral", "silver", "chrome"];

/// Each picture on the page needs its own gradient id.
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

const INK: &str = "#14171d";

/// The cats' eyes, run in the page: one small script for the whole document, so it holds for
/// cats added later and costs Rust nothing. Installed once by `App`. After OpenMausBot's
/// CursorAvatar.tsx (`BLINK`, the look-around) and Avatar.tsx (`onPointerMove`, `POINTER_GAZE`),
/// Apache-2.0, see NOTICE.
///
/// - Blinks and glances: four times a second it looks over the cats on the page and starts
///   whatever is due on each one's own random clock, by setting `data-blink` on the picture or
///   `--glance-x` / `--glance-y` (-1..1) on its `.cat-glance` groups. Glances are for idle cats:
///   a working cat scans (CSS) and a waiting cat keeps its eyes on the person.
/// - Following the pointer: the cat under the pointer, or the one inside the `[data-cat-follow]`
///   area under it (a sidebar row, a picker tile), gets `data-look`, and `--look-x` / `--look-y`
///   (-1..1) on its `.cat-look` groups.
///
/// - Resting: while the window isn't the one in use, `data-cats-rest` on `<html>`; the cats hold
///   still (no breathing, blinks or glances) until it is again. Their eyes still turn to a
///   pointer passing over them, since that only moves when the person does.
///
/// `mascots.css` turns those into motion. With reduced motion asked for, the script does nothing.
pub const MASCOT_LIFE: &str = r#"
(() => {
    if (window.__catbotMascotLife) return;
    window.__catbotMascotLife = true;
    const still = matchMedia("(prefers-reduced-motion: reduce)");
    const between = (low, high) => low + Math.random() * (high - low);
    const set = (cat, part, values) => {
        for (const el of cat.querySelectorAll(part)) {
            for (const [name, value] of Object.entries(values)) {
                if (value === null) el.style.removeProperty(name); else el.style.setProperty(name, value);
            }
        }
    };

    // Blinks and glances. How often a cat blinks, by state, in ms (their BLINK table).
    const BLINK = { idle: [5000, 12000], working: [2800, 5500], waiting: [2000, 4000] };
    const BLINK_MS = 320;
    const cats = document.getElementsByClassName("catbot-mark");
    const clocks = new WeakMap();
    const centre = (cat, clock) => {
        if (!clock.glancing) return;
        clock.glancing = false;
        set(cat, ".cat-glance", { "--glance-x": null, "--glance-y": null });
    };
    // Resting: while the window isn't the one in use (another window has focus, or it is
    // minimised or on another desktop) the cats hold still. `mascots.css` pauses the body under
    // data-cats-rest; here a blink in progress opens and a glance comes back to centre.
    const root = document.documentElement;
    const resting = () => root.hasAttribute("data-cats-rest");
    const rest = () => {
        if (resting()) return;
        root.setAttribute("data-cats-rest", "");
        for (const cat of cats) {
            cat.removeAttribute("data-blink");
            const clock = clocks.get(cat);
            if (clock) { clock.openAt = 0; centre(cat, clock); }
        }
    };
    const wake = () => {
        if (document.hidden || !document.hasFocus()) return;
        root.removeAttribute("data-cats-rest");
        // A blink or a glance that came due while resting waits a moment rather than all at once.
        const now = performance.now();
        for (const cat of cats) {
            const clock = clocks.get(cat);
            if (!clock) continue;
            clock.blinkAt = Math.max(clock.blinkAt, now + between(400, 3000));
            clock.glanceAt = Math.max(clock.glanceAt, now + between(1000, 4000));
        }
    };
    window.addEventListener("blur", rest);
    window.addEventListener("focus", wake);
    document.addEventListener("visibilitychange", () => (document.hidden ? rest() : wake()));
    if (document.hidden || !document.hasFocus()) rest();

    const tick = () => {
        if (document.hidden || resting()) return;
        const now = performance.now();
        for (const cat of cats) {
            if (cat.dataset.mark === "letter") continue;
            let clock = clocks.get(cat);
            if (!clock) {
                clock = { blinkAt: now + between(500, 9000), openAt: 0, glanceAt: now + between(1500, 8000), backAt: 0, glancing: false };
                clocks.set(cat, clock);
            }
            if (still.matches) {
                cat.removeAttribute("data-blink");
                centre(cat, clock);
                continue;
            }
            const state = cat.dataset.state;
            if (clock.openAt && now >= clock.openAt) {
                clock.openAt = 0;
                cat.removeAttribute("data-blink");
            } else if (!clock.openAt && now >= clock.blinkAt) {
                cat.setAttribute("data-blink", "");
                clock.openAt = now + BLINK_MS + 40;
                const [low, high] = BLINK[state] || BLINK.idle;
                // Now and then, a double blink.
                clock.blinkAt = Math.random() < 0.15 ? clock.openAt + 150 : now + between(low, high);
            }
            if (state !== "idle" || cat.hasAttribute("data-look")) {
                centre(cat, clock);
                clock.glanceAt = Math.max(clock.glanceAt, now + 1500);
            } else if (clock.glancing && now >= clock.backAt) {
                centre(cat, clock);
            } else if (!clock.glancing && now >= clock.glanceAt) {
                const side = Math.random() < 0.5 ? -1 : 1;
                set(cat, ".cat-glance", { "--glance-x": (side * between(0.55, 1)).toFixed(2), "--glance-y": between(-0.8, 0.5).toFixed(2) });
                clock.glancing = true;
                clock.backAt = now + between(700, 2200);
                // Often a second look elsewhere soon after, then a longer rest.
                clock.glanceAt = clock.backAt + (Math.random() < 0.3 ? between(300, 900) : between(3500, 9000));
            }
        }
    };
    setInterval(tick, 250);

    // Following the pointer.
    let cat = null, frame = 0, x = 0, y = 0, under = null;
    const release = () => {
        if (!cat) return;
        set(cat, ".cat-look", { "--look-x": null, "--look-y": null });
        cat.removeAttribute("data-look");
        cat = null;
    };
    const find = () => {
        if (still.matches || !(under instanceof Element)) return null;
        const direct = under.closest("svg.catbot-mark");
        if (direct) return direct;
        const area = under.closest("[data-cat-follow]");
        return area ? area.querySelector("svg.catbot-mark") : null;
    };
    const update = () => {
        frame = 0;
        const next = find();
        if (next !== cat) release();
        if (!next) return;
        cat = next;
        const box = cat.getBoundingClientRect();
        const half = Math.max(box.width / 2, 1);
        const dx = x - (box.left + box.width / 2), dy = y - (box.top + box.height / 2);
        // Inside the picture the eyes turn in proportion; beyond it they look fully that way.
        const reach = Math.max(half, Math.hypot(dx, dy));
        set(cat, ".cat-look", { "--look-x": (dx / reach).toFixed(3), "--look-y": (dy / reach).toFixed(3) });
        cat.setAttribute("data-look", "");
    };
    const moved = (event) => {
        if (event.pointerType === "touch") return;
        x = event.clientX; y = event.clientY; under = event.target;
        if (!frame) frame = requestAnimationFrame(update);
    };
    const left = () => { under = null; if (!frame) frame = requestAnimationFrame(update); };
    document.addEventListener("pointermove", moved, { passive: true });
    document.documentElement.addEventListener("pointerleave", left);
    window.addEventListener("blur", left);
})();
"#;

/// What makes one picture's body move unlike the next: a tempo and a starting point, as CSS
/// variables (read by `mascots.css`). Fixed for the picture's life, so the `style` they go in
/// never changes after the first render. (Blinks and glances keep their own random clocks.)
fn vary(n: usize) -> String {
    // Fractions of n times unrelated irrationals: well spread, and the same on every run.
    let part = |by: f64| (n as f64 * by).fract();
    format!("--cat-k:{:.3};--cat-phase:-{:.2}s", 0.85 + 0.35 * part(0.618_034), 4.0 * part(0.754_878))
}

/// A bot's picture. `mark` is one of `MARKS` (anything else draws the cat head); `state` is
/// "idle", "working" or "waiting": it sets how the picture moves, and the cat head's eyes.
#[component]
pub fn BotMark(
    color: String,
    initial: String,
    size: u32,
    #[props(default = "cat-head".to_string())] mark: String,
    #[props(default = "idle".to_string())] state: String,
    /// The bot's own picture (stage 23): shown instead of its mark.
    #[props(default)]
    picture: Option<Picture>,
) -> Element {
    if let Some(picture) = picture {
        return rsx! { PictureMark { picture, size, color } };
    }
    match mark.as_str() {
        "letter" => rsx! { LetterMark { color, initial, size, state } },
        "cat-sitting" => rsx! { CatSitting { color, size, state } },
        "cat-visor" => rsx! { CatVisor { color, size, state } },
        "channel" => rsx! { ChannelMark { color, size, state } },
        "robot-head" | "robot-full" => rsx! { RobotMark { mark, color, size, state } },
        _ => rsx! { CatHead { color, size, state } },
    }
}

/// The robot cat's pictures (stage 25.3): Fable's Blender model, its head or the whole cat sitting,
/// rendered by `art/pet3d/build_pet.py` (marks mode) in three passes like the desktop pet: the
/// cat with a grey shell, the shell's mask (the bot's colour, multiplied in) and its gloss
/// (screened back on top). Small pictures use the 64-pixel renders, drawn simpler to read small.
const ROBOT_MARKS: Asset = asset!("/assets/marks", AssetOptions::folder());

/// The robot-cat picture for `mark` at `size`: which renders, by name.
fn robot_files(mark: &str, size: u32) -> (String, String, String) {
    let kind = if mark == "robot-full" { "full" } else { "head" };
    let px = if size <= 40 { 64 } else { 256 };
    let at = |pass: &str| format!("{ROBOT_MARKS}/mark_{kind}_{pass}_{px}.png");
    (at("base"), at("mask"), at("gloss"))
}

#[component]
fn RobotMark(mark: String, color: String, size: u32, state: String) -> Element {
    let vary = use_hook(|| new_id("robot").1);
    let (base, mask, gloss) = robot_files(&mark, size);
    let layer = "position:absolute;inset:0;background-size:100% 100%";
    // Keyed by the renders, so another picture is a new element: a changed mask on a blended
    // layer isn't always repainted.
    let shown = base.clone();
    rsx! {
        for _ in [()] { div {
            key: "{shown}",
            "aria-hidden": "true",
            class: "catbot-mark catbot-robot relative shrink-0 select-none",
            "data-mark": "{mark}",
            "data-state": "{state}",
            style: "{vary};width:{size}px;height:{size}px;isolation:isolate;-webkit-mask:url({base}) 0 0/100% 100% no-repeat;mask:url({base}) 0 0/100% 100% no-repeat",
            div { style: "{layer};background-image:url({base})" }
            div { style: "{layer};background:var(--identity-{color});mix-blend-mode:multiply;-webkit-mask:url({mask}) 0 0/100% 100% no-repeat;mask:url({mask}) 0 0/100% 100% no-repeat" }
            div { style: "{layer};background-image:url({gloss});mix-blend-mode:screen" }
        } }
    }
}

/// The identity colour as three gradient stops, light to dark, as their room avatars shade it.
fn stops(color: &str) -> (String, String, String) {
    let fill = format!("var(--identity-{color})");
    (format!("color-mix(in srgb, {fill} 82%, #ffffff)"), fill.clone(), format!("color-mix(in srgb, {fill} 82%, #000000)"))
}

#[component]
fn Gradient(id: String, color: String) -> Element {
    let (light, fill, dark) = stops(&color);
    rsx! {
        defs {
            linearGradient { id: "{id}", gradient_units: "userSpaceOnUse", x1: "42", y1: "2", x2: "22", y2: "62",
                stop { offset: "0", stop_color: "{light}" }
                stop { offset: "0.55", stop_color: "{fill}" }
                stop { offset: "1", stop_color: "{dark}" }
            }
        }
    }
}

/// A picture's gradient id, and its own motion variables.
fn new_id(kind: &str) -> (String, String) {
    let n = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    (format!("{kind}-{n}"), vary(n))
}

/// A channel's picture: a "#" on a rounded square in its colour.
#[component]
fn ChannelMark(color: String, size: u32, state: String) -> Element {
    let (light, fill, dark) = stops(&color);
    let background = format!("linear-gradient(160deg, {light}, {fill} 55%, {dark})");
    let font = size / 2;
    let corner = size / 4;
    rsx! {
        span {
            "aria-hidden": "true",
            class: "catbot-mark flex shrink-0 select-none items-center justify-center font-bold text-white",
            "data-mark": "channel",
            "data-state": "{state}",
            width: "{size}px",
            height: "{size}px",
            font_size: "{font}px",
            border_radius: "{corner}px",
            background: "{background}",
            "#"
        }
    }
}

/// The letter in a circle: the stand-in from before the cats, kept as a choice.
#[component]
fn LetterMark(color: String, initial: String, size: u32, state: String) -> Element {
    let (light, fill, dark) = stops(&color);
    let background = format!("linear-gradient(160deg, {light}, {fill} 55%, {dark})");
    let font = size * 2 / 5;
    rsx! {
        span {
            "aria-hidden": "true",
            class: "catbot-mark flex shrink-0 select-none items-center justify-center rounded-full font-semibold text-white",
            "data-mark": "letter",
            "data-state": "{state}",
            // One property each: Dioxus updates a changed property in place, where a changed
            // `style` string left the old background cleared and no new one set.
            width: "{size}px",
            height: "{size}px",
            font_size: "{font}px",
            background: "{background}",
            "{initial}"
        }
    }
}

/// Concept A: a chunky cat head with slit-pupil eyes that look aside while working and go wide
/// while waiting for the person.
#[component]
fn CatHead(color: String, size: u32, state: String) -> Element {
    let (id, vary) = use_hook(|| new_id("cathead"));
    const HEAD: &str = "M8 11Q8 6 12.5 8L25 15.5Q32 13.5 39 15.5L51.5 8Q56 6 56 11L57 30Q60 37 58 44Q54 59 32 59Q10 59 6 44Q4 37 7 30Z";
    const INNER_EARS: &str = "M12 13.5L20.5 18.5Q15.5 21 13 25.5ZM52 13.5L43.5 18.5Q48.5 21 51 25.5Z";
    const NOSE: &str = "M29.2 47.2Q32 46.2 34.8 47.2Q35.6 47.7 35 48.6L32.8 51Q32 51.7 31.2 51L29 48.6Q28.4 47.7 29.2 47.2Z";
    // (eye rx, eye ry, pupil dx, pupil dy, pupil rx, pupil ry)
    let (eye_rx, eye_ry, dx, dy, pupil_rx, pupil_ry) = match state.as_str() {
        "working" => (7.0, 8.0, 3.0, 1.5, 2.8, 5.6),
        "waiting" => (7.4, 8.8, 0.0, 0.0, 4.6, 5.8),
        _ => (7.0, 8.0, 0.0, 0.0, 2.8, 5.6),
    };
    let (left_x, right_x, eye_y) = (20.5, 43.5, 37.0);
    rsx! {
        svg { "aria-hidden": "true", class: "catbot-mark shrink-0 select-none", "data-mark": "cat-head", "data-state": "{state}", style: "{vary}", width: "{size}", height: "{size}", view_box: "0 0 64 64",
            Gradient { id: id.clone(), color }
            path { fill: "url(#{id})", d: HEAD }
            path { fill: "#000000", fill_opacity: "0.2", d: INNER_EARS }
            // Whites and pupils blink together; only the pupils look about, within the whites.
            g { class: "cat-blink",
                ellipse { cx: "{left_x}", cy: "{eye_y}", rx: "{eye_rx}", ry: "{eye_ry}", fill: "#ffffff" }
                ellipse { cx: "{right_x}", cy: "{eye_y}", rx: "{eye_rx}", ry: "{eye_ry}", fill: "#ffffff" }
                g { class: "cat-look", g { class: "cat-glance",
                    ellipse { cx: "{left_x + dx}", cy: "{eye_y + dy}", rx: "{pupil_rx}", ry: "{pupil_ry}", fill: INK }
                    ellipse { cx: "{right_x + dx}", cy: "{eye_y + dy}", rx: "{pupil_rx}", ry: "{pupil_ry}", fill: INK }
                } }
            }
            path { fill: INK, d: NOSE }
        }
    }
}

/// Concept B: a whole cat sitting, tail curled round.
#[component]
fn CatSitting(color: String, size: u32, state: String) -> Element {
    let (id, vary) = use_hook(|| new_id("catsit"));
    rsx! {
        svg { "aria-hidden": "true", class: "catbot-mark shrink-0 select-none", "data-mark": "cat-sitting", "data-state": "{state}", style: "{vary}", width: "{size}", height: "{size}", view_box: "0 0 64 64",
            Gradient { id: id.clone(), color }
            path { fill: "none", stroke: "url(#{id})", stroke_width: "7.5", stroke_linecap: "round", d: "M42 56Q57 58 57.5 46Q57.8 40 53 37" }
            ellipse { cx: "28", cy: "45", rx: "19", ry: "16.5", fill: "url(#{id})" }
            path { fill: "url(#{id})", d: "M12.5 5.5Q12.2 2.5 15.2 3.5L24.5 8Q28 7 31.5 8L40.8 3.5Q43.8 2.5 43.5 5.5L43.8 16Q45 19 45 22Q45 36 28 36Q11 36 11 22Q11 19 12.2 16Z" }
            ellipse { cx: "28", cy: "49", rx: "9.5", ry: "10.5", fill: "#ffffff", fill_opacity: "0.25" }
            // The dot eyes, glints and all, shift a little and blink.
            g { class: "cat-look", g { class: "cat-glance", g { class: "cat-blink",
                ellipse { cx: "20.5", cy: "22.5", rx: "3.6", ry: "4.6", fill: INK }
                ellipse { cx: "35.5", cy: "22.5", rx: "3.6", ry: "4.6", fill: INK }
                circle { cx: "21.7", cy: "20.7", r: "1.3", fill: "#ffffff" }
                circle { cx: "36.7", cy: "20.7", r: "1.3", fill: "#ffffff" }
            } } }
        }
    }
}

/// Concept C: a rounded-square head whose corners rise into ears, with a dark visor.
#[component]
fn CatVisor(color: String, size: u32, state: String) -> Element {
    let (id, vary) = use_hook(|| new_id("catvisor"));
    rsx! {
        svg { "aria-hidden": "true", class: "catbot-mark shrink-0 select-none", "data-mark": "cat-visor", "data-state": "{state}", style: "{vary}", width: "{size}", height: "{size}", view_box: "0 0 64 64",
            Gradient { id: id.clone(), color }
            path { fill: "url(#{id})", d: "M7 9Q7 4 11.5 6.2L24 13L40 13L52.5 6.2Q57 4 57 9L57 46Q57 60 43 60L21 60Q7 60 7 46Z" }
            rect { x: "13", y: "24", width: "38", height: "24", rx: "10", fill: INK }
            // The face on the visor shifts as one (so the eyes never run into the nose); only
            // the pill eyes blink.
            g { class: "cat-look", g { class: "cat-glance",
                g { class: "cat-blink",
                    rect { x: "20.5", y: "30", width: "6", height: "11", rx: "3", fill: "#ffffff" }
                    rect { x: "37.5", y: "30", width: "6", height: "11", rx: "3", fill: "#ffffff" }
                }
                path { fill: "#ffffff", d: "M30 40.5L34 40.5Q34.8 40.7 34.3 41.5L32.6 43.3Q32 43.8 31.4 43.3L29.7 41.5Q29.2 40.7 30 40.5Z" }
            } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{robot_files, vary};

    #[test]
    fn robot_cats_use_the_small_renders_when_small() {
        let (base, mask, gloss) = robot_files("robot-full", 28);
        assert!(base.ends_with("/mark_full_base_64.png") && mask.ends_with("/mark_full_mask_64.png") && gloss.ends_with("/mark_full_gloss_64.png"));
        assert!(robot_files("robot-head", 64).0.ends_with("/mark_head_base_256.png"));
        for (mark, _) in super::MARKS {
            assert!(!mark.is_empty());
        }
    }

    #[test]
    fn pictures_move_out_of_step() {
        let all: Vec<String> = (0..40).map(vary).collect();
        for (i, one) in all.iter().enumerate() {
            assert!(one.starts_with("--cat-k:") && !one.contains(' '), "{one}");
            let tempo: f64 = one["--cat-k:".len()..one.find(';').unwrap()].parse().unwrap();
            assert!((0.85..=1.2).contains(&tempo), "{one}");
            assert!(!all[..i].contains(one), "picture {i} moves exactly like an earlier one");
        }
    }
}

/// A bot's own picture and how it's framed in its circle (stage 23).
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub url: String,
    /// 1 to 3.
    pub zoom: f64,
    /// What's in the middle, 0 to 1 across and down.
    pub x: f64,
    pub y: f64,
}

impl Picture {
    /// How the image sits in its circle.
    pub fn style(&self) -> String {
        let (x, y) = (self.x * 100.0, self.y * 100.0);
        format!("width:100%;height:100%;object-fit:cover;object-position:{x:.1}% {y:.1}%;transform:scale({:.3});transform-origin:{x:.1}% {y:.1}%", self.zoom)
    }
}

/// Where the picture can be drawn from: its address in a browser; in the apps (which reach Catbot
/// on their own), its bytes, fetched once and kept.
pub async fn picture_source(url: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        Some(url.to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use base64::Engine as _;
        use std::collections::HashMap;
        use std::sync::Mutex;
        static KEPT: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
        if let Some(found) = KEPT.lock().ok()?.as_ref().and_then(|k| k.get(url).cloned()) {
            return Some(found);
        }
        let bytes = crate::api::bytes(url).await.ok()?;
        let kind = if bytes.starts_with(&[0xff, 0xd8]) { "image/jpeg" } else if bytes.starts_with(b"GIF8") { "image/gif" } else if bytes.starts_with(b"RIFF") { "image/webp" } else { "image/png" };
        let data = format!("data:{kind};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
        KEPT.lock().ok()?.get_or_insert_with(HashMap::new).insert(url.to_string(), data.clone());
        Some(data)
    }
}

/// The picture in its circle, on the bot's colour while it loads.
#[component]
fn PictureMark(picture: Picture, size: u32, color: String) -> Element {
    let url = picture.url.clone();
    let source = use_resource(use_reactive!(|url| async move { picture_source(&url).await }));
    let style = picture.style();
    rsx! {
        span { "data-bot-picture": true, class: "inline-flex shrink-0 overflow-hidden rounded-full", style: "width:{size}px;height:{size}px;background:var(--identity-{color})",
            if let Some(Some(src)) = source() {
                img { src: "{src}", alt: "", draggable: "false", style: "{style}" }
            }
        }
    }
}
