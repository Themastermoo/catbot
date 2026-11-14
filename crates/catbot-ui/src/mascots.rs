//! Catbot's bot pictures: three original cats, each filled with the bot's identity colour shaded
//! light to dark, and the letter circle. Moo kept all four as choices (the Codex note
//! "2026-11-14 Catbot bots choose their mascot").
//!
//! The cats were drawn for Catbot (viewBox 0 0 64 64, legible from 16px up). None of them uses
//! OpenMausBot's mascot shapes.

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::prelude::*;

/// The four pictures, as stored on a bot.
pub const MARKS: [(&str, &str); 4] = [("cat-head", "Cat head"), ("cat-sitting", "Sitting cat"), ("cat-visor", "Visor cat"), ("letter", "Letter")];

/// The colours a bot can wear, as the stylesheet names them.
pub const COLORS: [&str; 12] = ["green", "blue", "red", "orange", "purple", "cyan", "pink", "yellow", "teal", "coral", "silver", "chrome"];

/// Each picture on the page needs its own gradient id.
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

const INK: &str = "#14171d";

/// A bot's picture. `mark` is one of `MARKS` (anything else draws the cat head); `state` is
/// "idle", "working" or "waiting" and changes the cat head's eyes.
#[component]
pub fn BotMark(
    color: String,
    initial: String,
    size: u32,
    #[props(default = "cat-head".to_string())] mark: String,
    #[props(default = "idle".to_string())] state: String,
) -> Element {
    match mark.as_str() {
        "letter" => rsx! { LetterMark { color, initial, size } },
        "cat-sitting" => rsx! { CatSitting { color, size } },
        "cat-visor" => rsx! { CatVisor { color, size } },
        _ => rsx! { CatHead { color, size, state } },
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

fn new_id(kind: &str) -> String {
    format!("{kind}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// The letter in a circle: the stand-in from before the cats, kept as a choice.
#[component]
fn LetterMark(color: String, initial: String, size: u32) -> Element {
    let (light, fill, dark) = stops(&color);
    let background = format!("linear-gradient(160deg, {light}, {fill} 55%, {dark})");
    let font = size * 2 / 5;
    rsx! {
        span {
            "aria-hidden": "true",
            class: "flex shrink-0 select-none items-center justify-center rounded-full font-semibold text-white",
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
    let id = use_hook(|| new_id("cathead"));
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
        svg { "aria-hidden": "true", class: "shrink-0 select-none", width: "{size}", height: "{size}", view_box: "0 0 64 64",
            Gradient { id: id.clone(), color }
            path { fill: "url(#{id})", d: HEAD }
            path { fill: "#000000", fill_opacity: "0.2", d: INNER_EARS }
            ellipse { cx: "{left_x}", cy: "{eye_y}", rx: "{eye_rx}", ry: "{eye_ry}", fill: "#ffffff" }
            ellipse { cx: "{right_x}", cy: "{eye_y}", rx: "{eye_rx}", ry: "{eye_ry}", fill: "#ffffff" }
            ellipse { cx: "{left_x + dx}", cy: "{eye_y + dy}", rx: "{pupil_rx}", ry: "{pupil_ry}", fill: INK }
            ellipse { cx: "{right_x + dx}", cy: "{eye_y + dy}", rx: "{pupil_rx}", ry: "{pupil_ry}", fill: INK }
            path { fill: INK, d: NOSE }
        }
    }
}

/// Concept B: a whole cat sitting, tail curled round.
#[component]
fn CatSitting(color: String, size: u32) -> Element {
    let id = use_hook(|| new_id("catsit"));
    rsx! {
        svg { "aria-hidden": "true", class: "shrink-0 select-none", width: "{size}", height: "{size}", view_box: "0 0 64 64",
            Gradient { id: id.clone(), color }
            path { fill: "none", stroke: "url(#{id})", stroke_width: "7.5", stroke_linecap: "round", d: "M42 56Q57 58 57.5 46Q57.8 40 53 37" }
            ellipse { cx: "28", cy: "45", rx: "19", ry: "16.5", fill: "url(#{id})" }
            path { fill: "url(#{id})", d: "M12.5 5.5Q12.2 2.5 15.2 3.5L24.5 8Q28 7 31.5 8L40.8 3.5Q43.8 2.5 43.5 5.5L43.8 16Q45 19 45 22Q45 36 28 36Q11 36 11 22Q11 19 12.2 16Z" }
            ellipse { cx: "28", cy: "49", rx: "9.5", ry: "10.5", fill: "#ffffff", fill_opacity: "0.25" }
            ellipse { cx: "20.5", cy: "22.5", rx: "3.6", ry: "4.6", fill: INK }
            ellipse { cx: "35.5", cy: "22.5", rx: "3.6", ry: "4.6", fill: INK }
            circle { cx: "21.7", cy: "20.7", r: "1.3", fill: "#ffffff" }
            circle { cx: "36.7", cy: "20.7", r: "1.3", fill: "#ffffff" }
        }
    }
}

/// Concept C: a rounded-square head whose corners rise into ears, with a dark visor.
#[component]
fn CatVisor(color: String, size: u32) -> Element {
    let id = use_hook(|| new_id("catvisor"));
    rsx! {
        svg { "aria-hidden": "true", class: "shrink-0 select-none", width: "{size}", height: "{size}", view_box: "0 0 64 64",
            Gradient { id: id.clone(), color }
            path { fill: "url(#{id})", d: "M7 9Q7 4 11.5 6.2L24 13L40 13L52.5 6.2Q57 4 57 9L57 46Q57 60 43 60L21 60Q7 60 7 46Z" }
            rect { x: "13", y: "24", width: "38", height: "24", rx: "10", fill: INK }
            rect { x: "20.5", y: "30", width: "6", height: "11", rx: "3", fill: "#ffffff" }
            rect { x: "37.5", y: "30", width: "6", height: "11", rx: "3", fill: "#ffffff" }
            path { fill: "#ffffff", d: "M30 40.5L34 40.5Q34.8 40.7 34.3 41.5L32.6 43.3Q32 43.8 31.4 43.3L29.7 41.5Q29.2 40.7 30 40.5Z" }
        }
    }
}
