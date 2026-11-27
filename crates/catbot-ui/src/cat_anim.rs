//! Small cat animations inside the Catbot window (stage 31): the robot cat typing in the
//! "is working" pill, waving hello in an empty chat, and the paw that passes along a hand-off.
//!
//! The cats are the desktop pet's sprite sheets (`assets/pet3d/`, made by
//! `art/pet3d/build_pet.py`), drawn smaller and cropped to the cat: the same three layers as the
//! pet (the cat with a grey shell, the shell's mask filled with the bot's colour, the gloss on
//! top). `assets/mascots.css` steps through the strip, so nothing here runs per frame; Rust only
//! hears when a strip ends or comes round. With reduced motion asked for a strip holds one frame,
//! and while the window isn't the one in use it pauses, as the bot pictures do.
//!
//! A bot whose picture is a dog (`mascots::is_dog`) gets the robot dog here instead: the same
//! sheets of the dog, in `assets/pet3d/dog/`, and a dog's paw print on its hand-offs.

use dioxus::prelude::*;

/// The pet's sprite sheets (also what the pet windows show).
pub const PET3D: Asset = asset!("/assets/pet3d", AssetOptions::folder());

/// One frame of a sheet, in CSS pixels at its natural size.
const FRAME: (f64, f64) = (96.0, 80.0);

/// A sheet shown in the window: its name, how many frames, how many a second, and whether it
/// goes round. The same numbers as `assets/pet3d/manifest.json` (a test keeps them so).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sheet {
    name: &'static str,
    frames: u32,
    fps: u32,
    looped: bool,
    /// The frame shown when nothing may move.
    still: u32,
}

const CODING: Sheet = Sheet { name: "coding", frames: 24, fps: 12, looped: true, still: 2 };
const WAVE: Sheet = Sheet { name: "wave", frames: 18, fps: 12, looped: false, still: 8 };
const IDLE: Sheet = Sheet { name: "idle", frames: 36, fps: 12, looped: true, still: 0 };

/// The part of a frame to show (left, top, right, bottom, in the frame's own 96 x 80), and how
/// large to draw it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Crop {
    part: (f64, f64, f64, f64),
    scale: f64,
}

/// The cat at its desk, small enough for a pill.
const AT_DESK: Crop = Crop { part: (20.0, 26.0, 96.0, 80.0), scale: 0.5 };
/// The whole standing cat, a paw's reach above it, for the empty chat.
const STANDING: Crop = Crop { part: (10.0, 8.0, 88.0, 80.0), scale: 1.5 };
/// The dog at its desk: its tail lies out behind it, so it shows from further left.
const DOG_AT_DESK: Crop = Crop { part: (6.0, 26.0, 96.0, 80.0), scale: 0.5 };

/// Where an animal's sheets are: the cat's in the pet's folder itself, the dog's in `dog/`.
fn folder(dog: bool) -> String {
    if dog { format!("{PET3D}/dog") } else { PET3D.to_string() }
}

/// The CSS variables one strip needs (read by `.cat-strip-box` in `mascots.css`).
fn strip_style(sheet: Sheet, crop: Crop, color: &str, folder: &str) -> String {
    let (left, top, right, bottom) = crop.part;
    let s = crop.scale;
    let name = sheet.name;
    format!(
        "--bw:{:.1}px;--bh:{:.1}px;--fw:{:.1}px;--fh:{:.1}px;--ox:{:.1}px;--oy:{:.1}px;--frames:{};--still:{};--dur:{:.3}s;--tint:var(--identity-{color});--base:url({folder}/base_{name}.png);--mask:url({folder}/mask_{name}.png);--gloss:url({folder}/gloss_{name}.png)",
        (right - left) * s,
        (bottom - top) * s,
        FRAME.0 * s,
        FRAME.1 * s,
        -left * s,
        -top * s,
        sheet.frames,
        sheet.still,
        sheet.frames as f64 / sheet.fps as f64,
    )
}

/// One strip, playing. `ended` hears a one-shot finish, `round` each time a loop comes round.
#[component]
fn Strip(sheet: Sheet, crop: Crop, color: String, #[props(default)] dog: bool, #[props(default)] ended: EventHandler<()>, #[props(default)] round: EventHandler<()>) -> Element {
    let style = strip_style(sheet, crop, &color, &folder(dog));
    let play = if sheet.looped { "loop" } else { "once" };
    rsx! {
        span { class: "cat-strip-box", "aria-hidden": "true", "data-cat-strip": "{sheet.name}", "data-strip-kind": if dog { "dog" } else { "cat" }, style: "{style}",
            // Keyed, so another sheet (or the other animal's) starts from its first frame.
            for shown in [format!("{}{}", if dog { "dog-" } else { "" }, sheet.name)] {
                span { key: "{shown}", class: "cat-strip", "data-play": play,
                    onanimationend: move |_| ended.call(()),
                    onanimationiteration: move |_| round.call(()),
                    span { class: "cat-strip-base" }
                    span { class: "cat-strip-tint" }
                    span { class: "cat-strip-gloss" }
                }
            }
        }
    }
}

/// The working pill's cat: the robot cat at its laptop, typing, in the bot's colour. With a
/// dog's picture as `mark`, the robot dog at the same laptop.
#[component]
pub fn TypingCat(color: String, #[props(default)] mark: String) -> Element {
    let dog = crate::mascots::is_dog(&mark);
    rsx! { Strip { sheet: CODING, crop: if dog { DOG_AT_DESK } else { AT_DESK }, color, dog } }
}

/// How many times the standing cat's loop goes round between waves (3 seconds a round).
const WAVE_EVERY: u32 = 3;

/// What the waving cat plays next: after the wave it stands; after enough rounds of standing
/// it waves again.
fn after(now: &str, rounds: u32) -> &'static str {
    match now {
        "wave" => "idle",
        _ if rounds >= WAVE_EVERY => "wave",
        _ => "idle",
    }
}

/// The empty chat's cat: the robot cat in the bot's colour waves hello, stands a while, and
/// waves again. With a dog's picture as `mark`, the robot dog does.
#[component]
pub fn WavingCat(color: String, #[props(default)] mark: String) -> Element {
    let dog = crate::mascots::is_dog(&mark);
    let mut playing = use_signal(|| "wave");
    let mut rounds = use_signal(|| 0u32);
    let sheet = if playing() == "wave" { WAVE } else { IDLE };
    rsx! {
        Strip { sheet, crop: STANDING, color, dog,
            ended: move |_| {
                rounds.set(0);
                playing.set(after("wave", 0));
            },
            round: move |_| {
                let so_far = *rounds.peek() + 1;
                let next = after("idle", so_far);
                rounds.set(if next == "wave" { 0 } else { so_far });
                if next != *playing.peek() {
                    playing.set(next);
                }
            },
        }
    }
}

/// The hand-off chip's arrow as a paw print passing from one bot to the next: it keeps going
/// across while the hand-off is still at work, and crosses once and rests when it's done.
/// A dog's print (`dog`, when the bot handing over is one) has its claws showing.
#[component]
pub fn PawPass(going: bool, #[props(default)] color: Option<String>, #[props(default)] dog: bool) -> Element {
    let tint = color.map(|c| format!("color:var(--identity-{c})")).unwrap_or_default();
    rsx! {
        span { class: "paw-pass", role: "img", "aria-label": "handed this to", "data-going": if going { "true" } else { "false" }, "data-paw": if dog { "dog" } else { "cat" }, style: "{tint}",
            svg { class: "paw-pass-paw", "aria-hidden": "true", width: "12", height: "12", view_box: "0 0 24 24", fill: "currentColor",
                ellipse { cx: "12", cy: "16.2", rx: "5.6", ry: "4.6" }
                ellipse { cx: "4.6", cy: "10.4", rx: "2.3", ry: "2.9" }
                ellipse { cx: "9.3", cy: "5.6", rx: "2.4", ry: "3.1" }
                ellipse { cx: "14.7", cy: "5.6", rx: "2.4", ry: "3.1" }
                ellipse { cx: "19.4", cy: "10.4", rx: "2.3", ry: "2.9" }
                if dog {
                    // A claw mark ahead of each toe.
                    path { d: "M3.4 7.4L4.2 4.6L5.4 7.2ZM8.2 2.5L9.2 0L10.4 2.5ZM13.6 2.5L14.8 0L15.8 2.5ZM18.6 7.2L19.8 4.6L20.6 7.4Z" }
                }
            }
        }
    }
}

/// Whether a reply sent at `at` has only just arrived (both in ms since the epoch): within the
/// last few seconds, give or take a clock that runs a little off. A reply that has just arrived
/// gets its speaker's hop; one scrolled back to, or loaded with the chat, doesn't.
pub fn just_arrived(at: f64, now: f64) -> bool {
    (now - at).abs() < 4000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strips_match_the_pets_sheets() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../assets/pet3d/manifest.json")).unwrap();
        assert_eq!((manifest["frame"]["css_width"].as_f64(), manifest["frame"]["css_height"].as_f64()), (Some(FRAME.0), Some(FRAME.1)));
        for sheet in [CODING, WAVE, IDLE] {
            let listed = &manifest["animations"][sheet.name];
            assert_eq!(listed["frames"].as_u64(), Some(sheet.frames as u64), "{}", sheet.name);
            assert_eq!(listed["fps"].as_u64(), Some(sheet.fps as u64), "{}", sheet.name);
            assert_eq!(listed["loop"].as_bool(), Some(sheet.looped), "{}", sheet.name);
            assert!(sheet.still < sheet.frames);
        }
    }

    #[test]
    fn the_dogs_strips_match_its_own_sheets() {
        // The same three sheets, frame for frame, so one set of numbers plays either animal.
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../assets/pet3d/dog/manifest.json")).unwrap();
        assert_eq!(manifest["species"].as_str(), Some("dog"));
        assert_eq!((manifest["frame"]["css_width"].as_f64(), manifest["frame"]["css_height"].as_f64()), (Some(FRAME.0), Some(FRAME.1)));
        for sheet in [CODING, WAVE, IDLE] {
            let listed = &manifest["animations"][sheet.name];
            assert_eq!(listed["frames"].as_u64(), Some(sheet.frames as u64), "{}", sheet.name);
            assert_eq!(listed["fps"].as_u64(), Some(sheet.fps as u64), "{}", sheet.name);
            assert_eq!(listed["loop"].as_bool(), Some(sheet.looped), "{}", sheet.name);
            for pass in ["base", "mask", "gloss"] {
                assert_eq!(listed["sheets"][pass].as_str(), Some(format!("{pass}_{}.png", sheet.name).as_str()));
            }
        }
        // They are in their own folder, so the cat's are never touched.
        assert!(folder(true).ends_with("/dog") && !folder(false).contains("dog"));
        let style = strip_style(CODING, DOG_AT_DESK, "orange", &folder(true));
        assert!(style.contains("/dog/base_coding.png") && style.contains("--bw:45.0px") && style.contains("--ox:-3.0px"), "{style}");
    }

    #[test]
    fn a_strip_is_cropped_and_sized_by_its_variables() {
        let style = strip_style(CODING, AT_DESK, "purple", "/assets/pet3d");
        // Half size: a 48 x 40 frame, of which the 38 x 27 with the cat and its desk shows.
        for part in ["--bw:38.0px", "--bh:27.0px", "--fw:48.0px", "--fh:40.0px", "--ox:-10.0px", "--oy:-13.0px", "--frames:24", "--dur:2.000s",
                     "--tint:var(--identity-purple)", "--base:url(/assets/pet3d/base_coding.png)", "--mask:url(/assets/pet3d/mask_coding.png)"] {
            assert!(style.contains(part), "{part} in {style}");
        }
        // The crop never reaches past its frame, where the next frame would show.
        for crop in [AT_DESK, STANDING, DOG_AT_DESK] {
            let (left, top, right, bottom) = crop.part;
            assert!(0.0 <= left && left < right && right <= FRAME.0 && 0.0 <= top && top < bottom && bottom <= FRAME.1);
        }
    }

    #[test]
    fn the_empty_chats_cat_waves_stands_and_waves_again() {
        assert_eq!(after("wave", 0), "idle");
        assert_eq!(after("idle", 1), "idle");
        assert_eq!(after("idle", WAVE_EVERY - 1), "idle");
        assert_eq!(after("idle", WAVE_EVERY), "wave");
    }

    #[test]
    fn only_a_fresh_reply_hops() {
        let now = 1_795_000_000_000.0;
        assert!(just_arrived(now - 500.0, now));
        assert!(just_arrived(now + 800.0, now), "a clock a little ahead");
        assert!(!just_arrived(now - 60_000.0, now), "an old reply, scrolled back to");
    }
}
