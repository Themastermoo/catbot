//! The Team map (stage 19.6): every bot in its department's box on a canvas you pan and zoom,
//! as OpenMausBot's `TeamMapPage.tsx` and `TeamCanvas.tsx` (Apache-2.0, see NOTICE).
//!
//! Drag the background to pan (the wheel zooms around the pointer, as Moo asked on 2026-11-27;
//! Shift+wheel and a touchpad's sideways swipe pan), drag a box's heading to put it
//! somewhere else (kept on the server, so every device sees the same map), drag a bot into
//! another box to move it there (after a confirm: it changes its department, not just its
//! place). A box's menu has the department's shared instructions, memory, bots and lead. Under
//! the map, who has handed work to whom; "Show screens" puts each bot's live computer screen on
//! its card.

use std::collections::HashMap;

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::bots::{Activity, Bot};
use crate::departments::{self, Depts, GENERAL, Open};
use crate::live::State;
use crate::mascots::BotMark;
use crate::settings::Ui;
use crate::sidebar::{Icon, MENU_ROW, MENU_SHEET};

pub const CARD_W: f64 = 236.0;
pub const CARD_H: f64 = 112.0;
/// A card with its bot's screen under it.
pub const SCREEN_H: f64 = 138.0;
pub const GAP: f64 = 16.0;
pub const PAD: f64 = 20.0;
pub const HEADER: f64 = 56.0;
const COLUMN_GAP: f64 = 56.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl Default for View {
    fn default() -> Self {
        View { x: 40.0, y: 40.0, scale: 1.0 }
    }
}

/// A department's box: its bots, leads in a column of their own beside the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub key: String,
    pub leads: Vec<Bot>,
    pub members: Vec<Bot>,
}

impl Section {
    fn bots(&self) -> impl Iterator<Item = &Bot> {
        self.leads.iter().chain(self.members.iter())
    }
}

/// The boxes: each department in order (empty ones too), then General. In General the Chief of
/// Staff stands as its lead; in a department, the department's own.
pub fn sections(bots: &[Bot], depts: &Depts) -> Vec<Section> {
    let bots: Vec<Bot> = bots.iter().filter(|b| b.channel.is_none()).cloned().collect();
    departments::groups(&bots, depts)
        .into_iter()
        .map(|(key, list)| {
            let general = key == GENERAL;
            let (leads, members) = list.into_iter().partition(|b| if general { b.chief } else { b.department_chief });
            Section { key, leads, members }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Tile {
    fn holds(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

fn card_h(bot: &Bot, screens: &HashMap<String, String>) -> f64 {
    if screens.contains_key(&bot.id) { CARD_H + SCREEN_H } else { CARD_H }
}

fn column_h(bots: &[Bot], screens: &HashMap<String, String>) -> f64 {
    bots.iter().map(|b| card_h(b, screens) + GAP).sum::<f64>() - if bots.is_empty() { 0.0 } else { GAP }
}

/// A box's size (their `teamSize`): two columns when it has a lead and others.
pub fn size(section: &Section, screens: &HashMap<String, String>) -> (f64, f64) {
    let two = !section.leads.is_empty() && !section.members.is_empty();
    let body = if two {
        column_h(&section.leads, screens).max(column_h(&section.members, screens))
    } else {
        let all: Vec<Bot> = section.bots().cloned().collect();
        column_h(&all, screens).max(CARD_H * 0.6)
    };
    (CARD_W * if two { 2.0 } else { 1.0 } + PAD * 2.0 + if two { 40.0 } else { 0.0 }, HEADER + PAD + body)
}

/// Where the boxes go (their `layoutTeams`): where they were put, else two columns.
pub fn layout(sections: &[Section], places: &HashMap<String, (f64, f64)>, screens: &HashMap<String, String>) -> Vec<Tile> {
    let sizes: Vec<(f64, f64)> = sections.iter().map(|s| size(s, screens)).collect();
    let columns = 2usize;
    let mut column_x = vec![40.0];
    for column in 1..columns {
        let widest = sizes.iter().enumerate().filter(|(i, _)| i % columns == column - 1).map(|(_, s)| s.0).fold(0.0, f64::max);
        column_x.push(column_x[column - 1] + widest + COLUMN_GAP);
    }
    let mut y = 40.0;
    sections
        .iter()
        .enumerate()
        .map(|(i, section)| {
            if i > 0 && i % columns == 0 {
                y += sizes[i - columns..i].iter().map(|s| s.1).fold(0.0, f64::max) + COLUMN_GAP;
            }
            let (x0, y0) = places.get(&section.key).copied().unwrap_or((column_x[i % columns], y));
            Tile { x: x0, y: y0, w: sizes[i].0, h: sizes[i].1 }
        })
        .collect()
}

/// The view that shows every box (their `fitTeams`).
pub fn fit(tiles: &[Tile], width: f64, height: f64) -> View {
    if tiles.is_empty() || width <= 0.0 || height <= 0.0 {
        return View::default();
    }
    let left = tiles.iter().map(|t| t.x).fold(f64::INFINITY, f64::min);
    let top = tiles.iter().map(|t| t.y).fold(f64::INFINITY, f64::min);
    let right = tiles.iter().map(|t| t.x + t.w).fold(f64::NEG_INFINITY, f64::max);
    let bottom = tiles.iter().map(|t| t.y + t.h).fold(f64::NEG_INFINITY, f64::max);
    let (cw, ch) = (right - left, bottom - top);
    let scale = ((width - 80.0) / cw).min((height - 80.0) / ch).clamp(0.3, 1.0);
    View { x: (width - cw * scale) / 2.0 - left * scale, y: (height - ch * scale) / 2.0 - top * scale, scale }
}

/// Zooms keeping a point of the viewport still.
/// A wheel's turn in pixels, whatever it counted in (a mouse wheel often counts lines).
fn wheel_pixels(delta: dioxus::html::geometry::WheelDelta) -> (f64, f64) {
    use dioxus::html::geometry::WheelDelta;
    match delta {
        WheelDelta::Pixels(v) => (v.x, v.y),
        WheelDelta::Lines(v) => (v.x * 40.0, v.y * 40.0),
        WheelDelta::Pages(v) => (v.x * 400.0, v.y * 400.0),
    }
}

/// How much a wheel's turn zooms: about a fifth per notch, in for a turn away from you.
pub fn wheel_zoom(dy: f64) -> f64 {
    (-dy * 0.002).exp()
}

pub fn zoom_at(view: View, scale: f64, px: f64, py: f64) -> View {
    let scale = scale.clamp(0.3, 1.5);
    let ratio = scale / view.scale;
    View { x: px - (px - view.x) * ratio, y: py - (py - view.y) * ratio, scale }
}

fn places_from(layout: &Value) -> HashMap<String, (f64, f64)> {
    layout["departments"]
        .as_object()
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), (v["x"].as_f64()?, v["y"].as_f64()?)))).collect())
        .unwrap_or_default()
}

/// A hand-off the map lists.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub thread: String,
    pub running: bool,
    pub last_at: f64,
}

fn edges_from(answer: &Value) -> Vec<Edge> {
    answer["edges"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|e| {
                    Some(Edge {
                        from: e["from"].as_str()?.to_string(),
                        to: e["to"].as_str()?.to_string(),
                        thread: e["threadId"].as_str().unwrap_or_default().to_string(),
                        running: e["state"] == "running",
                        last_at: e["lastAt"].as_f64().unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    Pan { start: (f64, f64), view: View },
    Box { key: String, start: (f64, f64), origin: (f64, f64), moved: bool },
    Bot { bot: Box<Bot>, start: (f64, f64), moved: bool },
}

/// The Team map page, in place of the chat.
#[component]
pub fn TeamMap() -> Element {
    let state = use_context::<Signal<State>>();
    let depts = use_context::<Signal<Depts>>();
    let ui = use_context::<Ui>();
    let mut view = use_signal(View::default);
    let mut places = use_signal(HashMap::<String, (f64, f64)>::new);
    let mut edges = use_signal(Vec::<Edge>::new);
    let mut loaded = use_signal(|| false);
    let mut fitted = use_signal(|| false);
    let mut gesture = use_signal(|| None::<Gesture>);
    let mut dragged = use_signal(|| None::<(Bot, f64, f64)>);
    let mut over = use_signal(|| None::<String>);
    let mut asking = use_signal(|| None::<(Bot, String)>);
    let mut dialog = use_signal(|| None::<Open>);
    let mut menu = use_signal(|| None::<String>);
    let mut show_screens = use_signal(|| false);
    let mut screens = use_signal(HashMap::<String, String>::new);
    let mut rect = use_signal(|| (0.0f64, 0.0f64, 0.0f64, 0.0f64));
    let mut mounted = use_signal(|| None::<std::rc::Rc<MountedData>>);
    // The bots a message being typed below will go to: lit on the map.
    let tagged = use_context_provider(|| Signal::new(Vec::<String>::new()));

    // The boxes' places and who handed work to whom: now, and again when a hand-off starts or ends.
    use_effect(move || {
        let _ = state.read().map_changed;
        spawn(async move {
            if let Ok(answer) = api::get("/api/team-map").await {
                if !*loaded.peek() {
                    places.set(places_from(&answer["layout"]));
                    loaded.set(true);
                }
                edges.set(edges_from(&answer));
            }
        });
    });
    // Live screens: each bot whose computer is on shows it on its card.
    use_effect(move || {
        let on = show_screens();
        let _ = state.read().computers_changed;
        spawn(async move {
            if !on {
                screens.set(HashMap::new());
                return;
            }
            let Ok(list) = api::get("/api/computers").await else { return };
            let mut found = HashMap::new();
            for entry in list["bots"].as_array().into_iter().flatten().filter(|b| b["state"] == "on") {
                let Some(id) = entry["botId"].as_str() else { continue };
                if entry["kind"] == "boat" {
                    continue; // Boat's view opens on its own site.
                }
                if let Ok(view) = api::get(&format!("/api/computers/{id}/view/watch")).await
                    && let Some(url) = view["url"].as_str().and_then(crate::computers::view_url)
                {
                    found.insert(id.to_string(), url);
                }
            }
            screens.set(found);
        });
    });

    let bots = state.read().bots.clone();
    let sections = sections(&bots, &depts.read());
    let tiles = layout(&sections, &places.read(), &screens.read());
    let v = view();
    let count = bots.iter().filter(|b| b.channel.is_none()).count();
    let count_label = if count == 1 { "1 bot".to_string() } else { format!("{count} bots") };

    // Fits once the places are in and the canvas has a size.
    let fit_now = {
        let tiles = tiles.clone();
        move || {
            let (_, _, w, h) = *rect.peek();
            view.set(fit(&tiles, w, h - 96.0));
        }
    };
    if loaded() && !fitted() && rect.read().2 > 0.0 {
        fitted.set(true);
        let mut fit_now = fit_now.clone();
        fit_now();
    }
    let measure = move || {
        spawn(async move {
            let Some(el) = mounted.peek().clone() else { return };
            if let Ok(r) = el.get_client_rect().await {
                rect.set((r.origin.x, r.origin.y, r.size.width, r.size.height));
            }
        });
    };
    let world_at = move |cx: f64, cy: f64| {
        let (left, top, _, _) = *rect.peek();
        let v = *view.peek();
        ((cx - left - v.x) / v.scale, (cy - top - v.y) / v.scale)
    };
    let save_places = move |next: HashMap<String, (f64, f64)>| {
        let body = json!({ "departments": next.iter().map(|(k, (x, y))| (k.clone(), json!({ "x": x, "y": y }))).collect::<serde_json::Map<String, Value>>() });
        spawn(async move {
            let _ = api::call("PUT", "/api/team-map/layout", Some(&body)).await;
        });
    };
    let tiles_for_drop = tiles.clone();
    let keys: Vec<String> = sections.iter().map(|s| s.key.clone()).collect();
    let keys_for_drop = keys.clone();
    let drop_at = move |cx: f64, cy: f64| -> Option<String> {
        let (wx, wy) = world_at(cx, cy);
        tiles_for_drop.iter().zip(keys_for_drop.iter()).find(|(t, _)| t.holds(wx, wy)).map(|(_, k)| k.clone())
    };
    let tiles_for_box = tiles.clone();
    let keys_for_box = keys.clone();

    let drop_on_up = drop_at.clone();
    let on_move = move |e: PointerEvent| {
        let p = e.client_coordinates();
        let (cx, cy) = (p.x, p.y);
        let Some(g) = gesture.peek().clone() else { return };
        match g {
            Gesture::Pan { start, view: from } => view.set(View { x: from.x + cx - start.0, y: from.y + cy - start.1, scale: from.scale }),
            Gesture::Box { key, start, origin, moved } => {
                let (dx, dy) = (cx - start.0, cy - start.1);
                if !moved && dx.hypot(dy) < 5.0 {
                    return;
                }
                let scale = view.peek().scale;
                places.write().insert(key.clone(), (origin.0 + dx / scale, origin.1 + dy / scale));
                gesture.set(Some(Gesture::Box { key, start, origin, moved: true }));
            }
            Gesture::Bot { bot, start, moved } => {
                if !moved && (cx - start.0).hypot(cy - start.1) < 5.0 {
                    return;
                }
                let (left, top, _, _) = *rect.peek();
                dragged.set(Some(((*bot).clone(), cx - left, cy - top)));
                over.set(drop_at(cx, cy));
                gesture.set(Some(Gesture::Bot { bot, start, moved: true }));
            }
        }
    };
    let mut on_up = move |cx: f64, cy: f64| {
        let Some(g) = gesture.take() else { return };
        dragged.set(None);
        over.set(None);
        match g {
            Gesture::Box { moved: true, .. } => save_places(places.peek().clone()),
            Gesture::Bot { bot, moved: true, .. } => {
                if let Some(to) = drop_on_up(cx, cy) {
                    let from = if bot.department.is_empty() { GENERAL.to_string() } else { bot.department.clone() };
                    if to != from {
                        asking.set(Some((*bot, to)));
                    }
                }
            }
            Gesture::Bot { bot, moved: false, .. } => open_chat(state, ui, &bot.id),
            _ => {}
        }
    };

    let edge_rows: Vec<(String, String, Edge)> = edges
        .read()
        .iter()
        .filter_map(|e| {
            let from = bots.iter().find(|b| b.id == e.from)?.name.clone();
            let to = bots.iter().find(|b| b.id == e.to)?.name.clone();
            Some((from, to, e.clone()))
        })
        .take(12)
        .collect();
    let tagged_now = tagged.read().clone();
    let screens_now = screens.read().clone();
    let world_style = format!("transform: translate({}px, {}px) scale({}); visibility: {};", v.x, v.y, v.scale, if fitted() { "visible" } else { "hidden" });
    let grid_style = format!(
        "background-image: radial-gradient(circle, color-mix(in srgb, var(--color-ink-secondary) 18%, transparent) 1px, transparent 1px); background-size: {s}px {s}px; background-position: {x}px {y}px;",
        s = 24.0 * v.scale,
        x = v.x,
        y = v.y
    );
    let fit_key = fit_now.clone();
    let screens_label: &'static str = if show_screens() { "Hide screens" } else { "Show screens" };
    let knob = if show_screens() { "absolute top-0.5 left-3.5 size-3 rounded-full bg-white shadow-sm transition-[left]" } else { "absolute top-0.5 left-0.5 size-3 rounded-full bg-white shadow-sm transition-[left]" };
    let track = if show_screens() { "relative h-4 w-7 shrink-0 rounded-full bg-accent transition-colors" } else { "relative h-4 w-7 shrink-0 rounded-full bg-control transition-colors" };
    rsx! {
        main { "data-team-map": true, class: "flex h-full min-w-0 flex-1 flex-col overflow-hidden bg-app text-ink",
            header { class: "flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-hairline/40 px-6 py-4",
                div {
                    div { class: "flex items-center gap-2.5",
                        span { class: "text-ink-secondary", Icon { name: "users", size: 18 } }
                        h1 { class: "text-[17px] font-semibold", "Team map" }
                        span { class: "ml-1 text-[11px] text-ink-secondary", "{count_label}" }
                    }
                    p { class: "mt-1 text-[12px] text-ink-secondary", "Your bots, their departments, and how they work together." }
                }
                div { class: "flex items-center gap-2",
                    button { r#type: "button", "aria-pressed": "{show_screens()}", title: "Show each bot's live computer screen", "data-map-screens": true,
                        class: "flex items-center gap-2 rounded-lg border border-hairline/60 bg-panel px-3 py-2 text-[12px] font-medium text-ink-secondary hover:bg-control hover:text-ink",
                        onclick: move |_| show_screens.toggle(),
                        span { "aria-hidden": "true", class: track, span { class: knob } }
                        "{screens_label}"
                    }
                    button { r#type: "button", "data-map-new-department": true,
                        class: "flex items-center gap-1.5 rounded-lg border border-hairline/60 bg-panel px-3 py-2 text-[12px] font-medium hover:bg-control",
                        onclick: move |_| dialog.set(Some(Open::New(Vec::new()))),
                        Icon { name: "plus", size: 14 }
                        "Department"
                    }
                }
            }
            div { class: "relative flex min-h-0 flex-1",
                div {
                    role: "region",
                    "aria-label": "Team canvas",
                    tabindex: 0,
                    "data-team-canvas": true,
                    class: "relative min-h-0 flex-1 touch-none select-none overflow-hidden outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/40",
                    style: "{grid_style}",
                    onmounted: move |e| {
                        mounted.set(Some(e.data()));
                        measure();
                    },
                    onresize: move |_| measure(),
                    onpointerdown: move |e| {
                        measure();
                        if gesture.peek().is_none() && e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Primary) {
                            let p = e.client_coordinates();
                            gesture.set(Some(Gesture::Pan { start: (p.x, p.y), view: *view.peek() }));
                        }
                    },
                    onpointermove: on_move,
                    onpointerup: move |e| { let p = e.client_coordinates(); on_up(p.x, p.y); },
                    onpointerleave: move |_| { gesture.set(None); dragged.set(None); over.set(None); },
                    onwheel: move |e| {
                        e.prevent_default();
                        let (dx, dy) = wheel_pixels(e.delta());
                        let (left, top, _, _) = *rect.peek();
                        let p = e.client_coordinates();
                        let now = *view.peek();
                        if e.modifiers().shift() || dx.abs() > dy.abs() {
                            view.set(View { x: now.x - dx, y: now.y - dy, scale: now.scale });
                        } else {
                            view.set(zoom_at(now, now.scale * wheel_zoom(dy), p.x - left, p.y - top));
                        }
                    },
                    onkeydown: move |e| {
                        let now = *view.peek();
                        let (_, _, w, h) = *rect.peek();
                        match e.key() {
                            Key::ArrowLeft => view.set(View { x: now.x + 60.0, ..now }),
                            Key::ArrowRight => view.set(View { x: now.x - 60.0, ..now }),
                            Key::ArrowUp => view.set(View { y: now.y + 60.0, ..now }),
                            Key::ArrowDown => view.set(View { y: now.y - 60.0, ..now }),
                            Key::Character(c) if c == "+" || c == "=" => view.set(zoom_at(now, now.scale * 1.2, w / 2.0, h / 2.0)),
                            Key::Character(c) if c == "-" => view.set(zoom_at(now, now.scale / 1.2, w / 2.0, h / 2.0)),
                            Key::Character(c) if c == "0" => { let mut f = fit_key.clone(); f(); }
                            Key::Escape => { gesture.set(None); dragged.set(None); menu.set(None); }
                            _ => {}
                        }
                    },
                    div { "data-canvas-world": true, class: "absolute left-0 top-0 origin-top-left", style: "{world_style}",
                        for (section, tile) in sections.iter().cloned().zip(tiles.iter().copied()) {
                            BoxView {
                                key: "{section.key}",
                                section: section.clone(),
                                tile,
                                dropping: over.read().as_ref() == Some(&section.key) && dragged.read().as_ref().is_some_and(|(b, _, _)| (if b.department.is_empty() { GENERAL } else { b.department.as_str() }) != section.key),
                                moving: dragged.read().as_ref().map(|(b, _, _)| b.id.clone()),
                                tagged: tagged_now.clone(),
                                screens: screens_now.clone(),
                                menu_open: menu.read().as_ref() == Some(&section.key),
                                on_menu: move |key: Option<String>| menu.set(key),
                                on_open: move |o| { menu.set(None); dialog.set(Some(o)); },
                                on_grab_box: {
                                    let tiles = tiles_for_box.clone();
                                    let keys = keys_for_box.clone();
                                    move |(key, cx, cy): (String, f64, f64)| {
                                        let at = keys.iter().position(|k| *k == key).and_then(|i| tiles.get(i)).map(|t| (t.x, t.y)).unwrap_or_default();
                                        gesture.set(Some(Gesture::Box { key, start: (cx, cy), origin: at, moved: false }));
                                    }
                                },
                                on_grab_bot: move |(bot, cx, cy): (Bot, f64, f64)| gesture.set(Some(Gesture::Bot { bot: Box::new(bot), start: (cx, cy), moved: false })),
                            }
                        }
                    }
                    if let Some((bot, x, y)) = dragged() {
                        div { "aria-hidden": "true", class: "pointer-events-none absolute z-30 flex items-center gap-2 rounded-xl border border-accent bg-panel px-3 py-2 shadow-lg",
                            style: "left: {x + 8.0}px; top: {y + 8.0}px;",
                            BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 24, mark: bot.mark.clone() }
                            span { class: "text-[13px] font-medium", "{bot.name}" }
                        }
                    }
                    div { class: "absolute bottom-24 right-4 z-20 flex flex-col gap-1 rounded-xl border border-hairline/50 bg-panel p-1 shadow-sm",
                        button { r#type: "button", "aria-label": "Zoom in", title: "Zoom in", class: ZOOM, onclick: move |_| { let now = *view.peek(); let (_, _, w, h) = *rect.peek(); view.set(zoom_at(now, now.scale * 1.2, w / 2.0, h / 2.0)); }, Icon { name: "plus", size: 15 } }
                        button { r#type: "button", "aria-label": "Zoom out", title: "Zoom out", class: ZOOM, onclick: move |_| { let now = *view.peek(); let (_, _, w, h) = *rect.peek(); view.set(zoom_at(now, now.scale / 1.2, w / 2.0, h / 2.0)); }, Icon { name: "minus", size: 15 } }
                        button { r#type: "button", "aria-label": "Fit departments to view (0)", title: "Fit departments to view (0)", "data-map-fit": true, class: ZOOM, onclick: { let fit_now = fit_now.clone(); move |_| { let mut f = fit_now.clone(); f(); } }, Icon { name: "maximize", size: 14 } }
                    }
                    if menu.read().is_some() {
                        div { class: "fixed inset-0 z-10", onclick: move |_| menu.set(None) }
                    }
                    crate::mapcomposer::MapComposer {}
                }
            }
            if !edge_rows.is_empty() {
                details { class: "shrink-0 border-t border-hairline/40 bg-panel px-6 py-3", "data-map-handoffs": true,
                    summary { class: "cursor-pointer text-[12px] text-ink-secondary", "Who hands work to whom · {edge_rows.len()}" }
                    div { class: "mt-3 max-h-48 space-y-2 overflow-y-auto",
                        for (from, to, edge) in edge_rows {
                            EdgeRow { key: "{edge.from}-{edge.to}", from, to, edge }
                        }
                    }
                }
            }
            if let Some((bot, to)) = asking() {
                MoveConfirm { bot, to, on_close: move |_| asking.set(None) }
            }
            departments::Dialogs { open: dialog }
        }
    }
}

const ZOOM: &str = "flex size-8 items-center justify-center rounded-lg text-ink-secondary hover:bg-control hover:text-ink";

/// Opens a bot's chat, leaving the map.
fn open_chat(state: Signal<State>, mut ui: Ui, bot_id: &str) {
    let thread = state.peek().bots.iter().find(|b| b.id == bot_id).map(|b| b.thread_id.clone()).unwrap_or_default();
    ui.map.set(false);
    crate::chats::open(state, ui, bot_id, &thread);
}

#[component]
fn BoxView(
    section: Section,
    tile: Tile,
    dropping: bool,
    moving: Option<String>,
    tagged: Vec<String>,
    screens: HashMap<String, String>,
    menu_open: bool,
    on_menu: EventHandler<Option<String>>,
    on_open: EventHandler<Open>,
    on_grab_box: EventHandler<(String, f64, f64)>,
    on_grab_bot: EventHandler<(Bot, f64, f64)>,
) -> Element {
    let general = section.key == GENERAL;
    let two = !section.leads.is_empty() && !section.members.is_empty();
    let count = section.leads.len() + section.members.len();
    let ring = if dropping { "border-accent ring-2 ring-accent/25" } else { "border-hairline/50" };
    let z = if menu_open { "z-20" } else { "" };
    let key = section.key.clone();
    let (k1, k2, k3, k4, k5) = (key.clone(), key.clone(), key.clone(), key.clone(), key.clone());
    let lead_label = if general { "Chief of Staff" } else { "Lead" };
    rsx! {
        section { "data-team-key": "{section.key}", "aria-label": "{section.key} department",
            class: "absolute rounded-2xl border bg-panel/90 shadow-sm {ring} {z}",
            style: "left: {tile.x}px; top: {tile.y}px; width: {tile.w}px; height: {tile.h}px;",
            header { class: "flex h-14 items-center gap-2 px-5",
                div { "data-arrange-team": "{section.key}", title: "Drag to arrange",
                    class: "min-w-0 flex-1 cursor-grab rounded-md py-2 active:cursor-grabbing",
                    onpointerdown: move |e| {
                        if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Primary) {
                            let p = e.client_coordinates();
                            on_grab_box.call((key.clone(), p.x, p.y));
                        }
                    },
                    h2 { class: "truncate text-[13px] font-semibold tracking-tight", "{section.key}" }
                }
                span { class: "text-[11px] tabular-nums text-ink-secondary", "{count}" }
                if !general {
                    div { class: "relative",
                        button { r#type: "button", "aria-label": "Manage {section.key} department", "aria-expanded": "{menu_open}", "data-map-box-menu": "{section.key}",
                            class: "flex size-8 items-center justify-center rounded-lg text-ink-secondary hover:bg-control hover:text-ink",
                            onpointerdown: move |e| e.stop_propagation(),
                            onclick: move |_| on_menu.call(if menu_open { None } else { Some(k1.clone()) }),
                            Icon { name: "more", size: 15 }
                        }
                        if menu_open {
                            div { role: "menu", class: "absolute right-0 top-full z-30 mt-1 w-[230px] {MENU_SHEET}", onpointerdown: move |e| e.stop_propagation(),
                                button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: move |_| on_open.call(Open::Instructions(k2.clone())), "Shared instructions…" }
                                button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: move |_| on_open.call(Open::Memory(k3.clone())), "Department memory" }
                                button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: move |_| on_open.call(Open::Members(k4.clone())), "Bots and lead…" }
                                button { r#type: "button", role: "menuitem", class: MENU_ROW, onclick: { let k = k5.clone(); move |_| on_open.call(Open::Rename(k.clone())) }, "Rename…" }
                                button { r#type: "button", role: "menuitem", class: "{MENU_ROW} text-danger", onclick: move |_| on_open.call(Open::Delete(k5.clone())), "Delete department…" }
                            }
                        }
                    }
                }
            }
            if count == 0 {
                p { class: "px-5 text-[12px] leading-relaxed text-ink-tertiary", "Drag bots here, or add them from the department's menu." }
            } else if two {
                div { class: "flex gap-10 px-5",
                    div { class: "flex flex-col gap-4",
                        span { class: "sr-only", "{lead_label}" }
                        for bot in section.leads.iter().cloned() {
                            Card { key: "{bot.id}", moving: moving.as_ref() == Some(&bot.id), tagged: tagged.contains(&bot.id), screen: screens.get(&bot.id).cloned(), bot, on_grab: on_grab_bot }
                        }
                    }
                    div { class: "flex flex-col gap-4",
                        for bot in section.members.iter().cloned() {
                            Card { key: "{bot.id}", moving: moving.as_ref() == Some(&bot.id), tagged: tagged.contains(&bot.id), screen: screens.get(&bot.id).cloned(), bot, on_grab: on_grab_bot }
                        }
                    }
                }
            } else {
                div { class: "flex flex-col gap-4 px-5",
                    for bot in section.bots().cloned() {
                        Card { key: "{bot.id}", moving: moving.as_ref() == Some(&bot.id), tagged: tagged.contains(&bot.id), screen: screens.get(&bot.id).cloned(), bot, on_grab: on_grab_bot }
                    }
                }
            }
        }
    }
}

/// A bot on the map (their `BotCard`): picture, name and crown, what it's doing, its model;
/// grab it to move it, click it (or the bubble) to open its chat.
#[component]
fn Card(bot: Bot, moving: bool, tagged: bool, screen: Option<String>, on_grab: EventHandler<(Bot, f64, f64)>) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let (status, tone) = match bot.activity {
        Activity::WaitingOnYou => ("Waiting for you", "bg-warning"),
        Activity::Working => ("Working", "bg-success"),
        Activity::Idle => ("", ""),
    };
    let role = if bot.chief {
        "Chief of Staff".to_string()
    } else if bot.department_chief {
        format!("Leads {}", bot.department)
    } else if !bot.title.is_empty() {
        // Its role, when it has one (stage 23).
        bot.title.clone()
    } else {
        let first: String = bot.soul.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
        if first.is_empty() { "Bot".into() } else { first }
    };
    let model = if bot.model.is_empty() {
        let mut engine = bot.engine.chars();
        engine.next().map(|c| c.to_uppercase().chain(engine).collect::<String>()).unwrap_or_default() + " default"
    } else {
        bot.model.clone()
    };
    let mut border = if tagged { "border-accent ring-4 ring-accent/25" } else { "border-hairline/50 hover:border-ink-secondary/40" }.to_string();
    if moving {
        border.push_str(" opacity-35");
    }
    let grabbed = bot.clone();
    let id = bot.id.clone();
    rsx! {
        article { "data-bot-id": "{bot.id}", "data-tagged": tagged,
            class: "relative w-[236px] shrink-0 rounded-xl border bg-card shadow-sm transition-colors {border}",
            div { class: "flex h-[70px] cursor-grab items-center gap-3 rounded-t-xl px-4 active:cursor-grabbing",
                title: "Drag to another department to move {bot.name}",
                onpointerdown: move |e| {
                    if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Primary) {
                        let p = e.client_coordinates();
                        on_grab.call((grabbed.clone(), p.x, p.y));
                    }
                },
                BotMark { picture: bot.picture.clone(), color: bot.color.clone(), initial: bot.initial(), size: 38, mark: bot.mark.clone(), state: bot.activity.mood() }
                span { class: "min-w-0 flex-1",
                    span { class: "flex items-center gap-1.5",
                        span { class: "truncate text-[14px] font-semibold", "{bot.name}" }
                        if bot.chief {
                            span { class: "shrink-0 text-warning", role: "img", "aria-label": "Chief of Staff", Icon { name: "crown", size: 12 } }
                        } else if bot.department_chief {
                            span { class: "shrink-0 text-ink-secondary", role: "img", "aria-label": "Leads {bot.department}", Icon { name: "crown", size: 12 } }
                        }
                    }
                    span { class: "mt-1 block truncate text-[11px] text-ink-secondary", "{role}" }
                }
            }
            div { class: "flex h-[42px] items-center gap-1 border-t border-hairline/30 px-2",
                button { r#type: "button", "aria-label": "Open chat with {bot.name}", title: "Open chat", "data-map-open-chat": "{bot.id}",
                    class: "flex size-8 items-center justify-center rounded-lg text-ink-secondary hover:bg-control hover:text-ink",
                    onpointerdown: move |e| e.stop_propagation(),
                    onclick: move |_| open_chat(state, ui, &id),
                    Icon { name: "message", size: 13 }
                }
                if !status.is_empty() {
                    span { class: "flex items-center gap-1.5 text-[10px] text-ink-secondary", title: "{status}",
                        span { class: "size-1.5 rounded-full {tone}" }
                        "{status}"
                    }
                }
                span { class: "ml-auto min-w-0 max-w-[130px] truncate px-2 text-[10px] text-ink-secondary", title: "Model: {model}", "{model}" }
            }
            if let Some(url) = screen {
                div { class: "border-t border-hairline/30 p-2",
                    iframe { title: "{bot.name}'s screen", src: "{url}", tabindex: "-1", class: "pointer-events-none h-[122px] w-full rounded-md border border-hairline/40 bg-black" }
                }
            }
        }
    }
}

#[component]
fn EdgeRow(from: String, to: String, edge: Edge) -> Element {
    let state = use_context::<Signal<State>>();
    let ui = use_context::<Ui>();
    let when = if edge.running { "Running".to_string() } else { crate::chat::short_when(edge.last_at) };
    let badge = if edge.running { "rounded-full bg-success/15 px-2 py-0.5 text-[11px] font-medium text-success" } else { "rounded-full bg-control px-2 py-0.5 text-[11px] font-medium text-ink-secondary" };
    let arrow = if edge.running { "shrink-0 text-accent" } else { "shrink-0 text-ink-secondary" };
    // A hand-off happens in the chat of the bot that handed on.
    let (owner, thread) = (edge.from.clone(), edge.thread.clone());
    rsx! {
        button { r#type: "button", "data-map-edge": true,
            class: "flex w-full items-center gap-3 rounded-xl border border-hairline/40 bg-card px-3 py-2.5 text-left transition hover:bg-raised/50",
            onclick: move |_| {
                let mut ui = ui;
                ui.map.set(false);
                crate::chats::open(state, ui, &owner, &thread);
            },
            div { class: "flex min-w-0 flex-1 items-center gap-2",
                span { class: "truncate text-[13px] font-medium text-ink", "{from}" }
                span { class: arrow, Icon { name: "arrow-right", size: 13 } }
                span { class: "truncate text-[13px] font-medium text-ink", "{to}" }
            }
            span { class: badge, "{when}" }
        }
    }
}

/// Moving a bot to another department changes what it reads and who it works with: asked first.
#[component]
fn MoveConfirm(bot: Bot, to: String, on_close: EventHandler<()>) -> Element {
    let depts = use_context::<Signal<Depts>>();
    let id = bot.id.clone();
    let target = to.clone();
    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4", onclick: move |_| on_close.call(()),
            div { role: "alertdialog", "aria-label": "Move {bot.name} to {to}?", "data-map-move-confirm": true,
                class: "w-full max-w-[420px] rounded-2xl bg-panel p-5 shadow-2xl", onclick: move |e| e.stop_propagation(),
                h2 { class: "text-[17px] font-semibold text-ink", "Move {bot.name} to {to}?" }
                p { class: "mt-2 text-[13px] leading-relaxed text-ink-secondary", "This changes the bot's department and the shared instructions it reads, not just where it sits. Its chats and model stay with it." }
                div { class: "mt-4 flex justify-end gap-2",
                    button { r#type: "button", class: "rounded-lg px-3 py-2 text-[13px] text-ink-secondary hover:bg-control", onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "button", "data-map-move": true, class: "ui-button-primary rounded-lg bg-accent px-3 py-2 text-[13px] font-medium text-accent-ink hover:brightness-110",
                        onclick: move |_| {
                            departments::move_bots(depts, &target, vec![id.clone()]);
                            on_close.call(());
                        },
                        "Move bot"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(id: &str, dept: &str, lead: bool, chief: bool) -> Bot {
        let mut b = Bot::from_wire(&json!({ "id": id, "name": id, "chief": chief })).unwrap();
        b.department = dept.into();
        b.department_chief = lead;
        b
    }

    #[test]
    fn the_wheel_zooms_about_a_fifth_a_notch_around_the_pointer() {
        // A notch towards you (100 px down) zooms out; away, in; by the same step.
        let out = wheel_zoom(100.0);
        assert!((0.8..0.85).contains(&out), "{out}");
        assert!((wheel_zoom(-100.0) * out - 1.0).abs() < 1e-9);
        // The point under the pointer stays put.
        let v = View { x: 10.0, y: 20.0, scale: 1.0 };
        let z = zoom_at(v, v.scale * wheel_zoom(-100.0), 300.0, 200.0);
        assert!(((300.0 - z.x) / z.scale - (300.0 - v.x) / v.scale).abs() < 1e-9);
        assert_eq!(wheel_pixels(dioxus::html::geometry::WheelDelta::lines(0.0, 3.0, 0.0)), (0.0, 120.0));
    }

    #[test]
    fn boxes_put_leads_beside_the_rest() {
        let depts = Depts::from_wire(&json!({ "departments": [{ "name": "Garden" }, { "name": "Empty" }] }));
        let all = [bot("k", "", false, true), bot("q", "Garden", true, false), bot("s", "Garden", false, false), bot("x", "", false, false)];
        let found = sections(&all, &depts);
        let shape: Vec<(String, usize, usize)> = found.iter().map(|s| (s.key.clone(), s.leads.len(), s.members.len())).collect();
        assert_eq!(shape, [("Garden".into(), 1, 1), ("Empty".into(), 0, 0), ("General".into(), 1, 1)]);
        let none = HashMap::new();
        let (w, _) = size(&found[0], &none);
        assert_eq!(w, CARD_W * 2.0 + PAD * 2.0 + 40.0, "a lead stands in its own column");
        let (w, _) = size(&found[1], &none);
        assert_eq!(w, CARD_W + PAD * 2.0);
        let mut screens = HashMap::new();
        screens.insert("s".to_string(), "/x".to_string());
        assert!(size(&found[0], &screens).1 > size(&found[0], &none).1, "a screen makes room");
    }

    #[test]
    fn boxes_lay_out_in_two_columns_unless_placed() {
        let depts = Depts::from_wire(&json!({ "departments": [{ "name": "A" }, { "name": "B" }] }));
        let found = sections(&[], &depts);
        let none = HashMap::new();
        let tiles = layout(&found, &HashMap::new(), &none);
        assert_eq!((tiles[0].x, tiles[0].y), (40.0, 40.0));
        assert!(tiles[1].x > tiles[0].x && tiles[1].y == 40.0);
        assert!(tiles[2].y > 40.0 && tiles[2].x == 40.0, "General starts the next row");
        let mut placed = HashMap::new();
        placed.insert("B".to_string(), (500.0, 600.0));
        assert_eq!((layout(&found, &placed, &none)[1].x, layout(&found, &placed, &none)[1].y), (500.0, 600.0));
        let v = fit(&tiles, 1000.0, 700.0);
        assert!(v.scale <= 1.0 && v.scale >= 0.3);
        let z = zoom_at(View::default(), 9.0, 0.0, 0.0);
        assert_eq!(z.scale, 1.5, "zoom stops at 150%");
    }
}
