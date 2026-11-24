//! Voice (stage 22): Settings › Voice, each bot's voice, the microphone, voice chat, and reading
//! replies aloud.
//!
//! Moo's choices (2026-11-24): two switches, "Talk to bots" and "Turn on voice reply", each run on
//! this computer or by an online service; Whisper and Kokoro or Piper here, OpenAI online (and
//! ElevenLabs for replies); a setting for checking the words first or sending them at once;
//! replies read aloud; a voice per bot. And (Moo, the same day) voice chat as the Claude and
//! ChatGPT phone apps have it: Catbot listens, sends what you said when you stop, reads the reply,
//! listens again, with a mute button so it doesn't pick up the room. Catbot's server does the work (`server/voice.rs`); this
//! side only records and plays.

use dioxus::prelude::*;
use serde_json::{Value, json};

use std::collections::{HashSet, VecDeque};

use crate::api;
use crate::live::State;
use crate::sidebar::Icon;

const CARD: &str = "overflow-hidden rounded-xl bg-composer ring-1 ring-inset ring-ink/[0.07]";
const ROW: &str = "flex w-full min-h-[52px] flex-wrap items-center gap-3 px-4 py-3 text-left [&+&]:border-t [&+&]:border-hairline/50";
const SECTION: &str = "px-1 pb-2 pt-6 text-[13px] font-semibold text-ink first:pt-1";
const SELECT: &str = "max-w-[260px] rounded-lg bg-raised px-2 py-1 text-[13px] text-ink ring-1 ring-ink/10 focus:outline-none";
const SMALL_BUTTON: &str = "rounded-lg bg-raised px-2.5 py-1 text-[13px] font-medium text-ink ring-1 ring-ink/10 hover:bg-raised-hover disabled:opacity-40";

/// What the server says about voice (`GET /api/voice`), shared by the settings, the bot panel and
/// the chat. Read at start and again on each `voice` frame.
#[derive(Clone, Copy)]
pub struct VoiceInfo(pub Signal<Value>);

impl VoiceInfo {
    pub fn new() -> Self {
        VoiceInfo(Signal::new(Value::Null))
    }

    pub fn refresh(self) {
        let mut info = self.0;
        spawn(async move {
            if let Ok(found) = api::get("/api/voice").await {
                info.set(found);
            }
        });
    }

    pub fn listen_on(&self) -> bool {
        self.0.read()["settings"]["listen"]["on"] == true
    }

    pub fn reply_on(&self) -> bool {
        self.0.read()["settings"]["reply"]["on"] == true
    }

    pub fn send_at_once(&self) -> bool {
        self.0.read()["settings"]["listen"]["sendAtOnce"] == true
    }

    /// The engine replies are spoken with now: "kokoro", "piper", "openai" or "elevenlabs".
    pub fn reply_engine(&self) -> String {
        let reply = &self.0.read()["settings"]["reply"];
        let key = if reply["where"] == "online" { "online" } else { "local" };
        reply[key].as_str().unwrap_or("kokoro").to_string()
    }
}

/// Changes part of the voice settings.
fn change(info: VoiceInfo, part: Value, mut problem: Signal<Option<String>>) {
    let mut shown = info.0;
    spawn(async move {
        match api::call("PUT", "/api/voice", Some(&part)).await {
            Ok(found) => {
                shown.set(found);
                problem.set(None);
            }
            Err(err) => problem.set(Some(err)),
        }
    });
}

/// Starts downloading a part this computer needs (the programs come along if they're missing).
fn get(info: VoiceInfo, part: &str, mut problem: Signal<Option<String>>) {
    let need_program = info.0.read()["local"]["program"]["ready"] != true;
    let part = part.to_string();
    spawn(async move {
        if need_program && part != "program" && let Err(err) = api::post("/api/voice/get", &json!({ "part": "program" })).await {
            problem.set(Some(err));
            return;
        }
        if let Err(err) = api::post("/api/voice/get", &json!({ "part": part })).await {
            problem.set(Some(err));
        }
    });
}

/// "Ready", a Get button, how far a download has got, or why it stopped.
#[component]
fn PartState(state: Value, size: String, label: String, on_get: EventHandler<()>) -> Element {
    if state["ready"] == true {
        return rsx! { span { class: "text-[13px] text-ink-secondary", "data-voice-ready": true, "Ready" } };
    }
    if state["getting"] == true {
        let (done, total) = (state["done"].as_f64().unwrap_or(0.0), state["total"].as_f64().unwrap_or(0.0));
        let pct = if total > 0.0 { (done / total * 100.0).round() } else { 0.0 };
        let mb = (done / 1_048_576.0).round();
        return rsx! {
            span { class: "flex items-center gap-2 text-[13px] text-ink-secondary", "data-voice-getting": true,
                span { class: "h-1.5 w-24 overflow-hidden rounded-full bg-control",
                    span { class: "block h-full bg-accent", style: "width: {pct}%" }
                }
                if total > 0.0 { "{pct}%" } else { "{mb} MB" }
            }
        };
    }
    rsx! {
        span { class: "flex items-center gap-2",
            if let Some(why) = state["problem"].as_str() {
                span { class: "max-w-[260px] text-[12px] text-danger", "{why}" }
            }
            button { r#type: "button", "data-voice-get": "{label}", class: SMALL_BUTTON, onclick: move |_| on_get.call(()),
                if state["problem"].is_string() { "Try again" } else { "Get ({size})" }
            }
        }
    }
}

/// Two choices side by side ("This computer" / "Online").
#[component]
fn Choice(value: String, options: Vec<(String, String)>, label: String, on_pick: EventHandler<String>) -> Element {
    rsx! {
        div { role: "radiogroup", "aria-label": "{label}", class: "flex rounded-lg bg-raised p-0.5 ring-1 ring-ink/10",
            for (id, name) in options {
                {
                    let on = id == value;
                    let class = if on { "rounded-md bg-panel px-2.5 py-1 text-[13px] font-medium text-ink shadow-sm" } else { "rounded-md px-2.5 py-1 text-[13px] text-ink-secondary hover:text-ink" };
                    let pick = id.clone();
                    rsx! {
                        button { key: "{id}", r#type: "button", role: "radio", "aria-checked": "{on}", "data-voice-choice": "{id}", class, onclick: move |_| on_pick.call(pick.clone()), "{name}" }
                    }
                }
            }
        }
    }
}

/// Settings › Voice.
#[component]
pub fn VoiceSettings() -> Element {
    let info = use_context::<VoiceInfo>();
    let problem = use_signal(|| None::<String>);
    let mut eleven_voices = use_signal(Vec::<Value>::new);
    let at_home = !crate::link::pairs() && !crate::link::has_own();
    let v = info.0.read().clone();
    if v.is_null() {
        return rsx! { p { class: "text-[13px] text-ink-secondary", "Looking…" } };
    }
    let s = &v["settings"];
    let listen_on = s["listen"]["on"] == true;
    let listen_place = s["listen"]["where"].as_str().unwrap_or("local").to_string();
    let reply_on = s["reply"]["on"] == true;
    let reply_place = s["reply"]["where"].as_str().unwrap_or("local").to_string();
    let local_engine = s["reply"]["local"].as_str().unwrap_or("kokoro").to_string();
    let online_engine = s["reply"]["online"].as_str().unwrap_or("openai").to_string();
    let engine = if reply_place == "online" { online_engine.clone() } else { local_engine.clone() };
    let model = s["listen"]["model"].as_str().unwrap_or("base.en").to_string();
    let send_at_once = s["listen"]["sendAtOnce"] == true;
    let models: Vec<Value> = v["local"]["listen"].as_array().cloned().unwrap_or_default();
    let model_state = models.iter().find(|m| m["id"] == model.as_str()).map(|m| m["state"].clone()).unwrap_or(Value::Null);
    let model_size = models.iter().find(|m| m["id"] == model.as_str()).and_then(|m| m["size"].as_str()).unwrap_or("").to_string();
    let voices: Vec<(String, String)> = if engine == "elevenlabs" {
        eleven_voices.read().iter().filter_map(|x| Some((x["id"].as_str()?.to_string(), x["name"].as_str().unwrap_or("Voice").to_string()))).collect()
    } else {
        v["voices"][&engine].as_array().into_iter().flatten().filter_map(|x| Some((x["id"].as_str()?.to_string(), x["name"].as_str().unwrap_or("").to_string()))).collect()
    };
    let chosen = s["reply"]["voices"][&engine].as_str().map(str::to_string).or_else(|| v["defaults"][&engine].as_str().map(str::to_string)).unwrap_or_default();
    let openai_key = v["keys"]["openai"]["hint"].as_str().map(str::to_string);
    let eleven_key = v["keys"]["elevenlabs"]["hint"].as_str().map(str::to_string);
    // ElevenLabs' voices are the account's own: asked for once a key is there.
    let wants_eleven = engine == "elevenlabs" && eleven_key.is_some();
    use_effect(use_reactive!(|wants_eleven| {
        if wants_eleven {
            spawn(async move {
                if let Ok(found) = api::get("/api/voice/elevenlabs-voices").await {
                    eleven_voices.set(found["voices"].as_array().cloned().unwrap_or_default());
                }
            });
        }
    }));
    let engine_for_voice = engine.clone();
    let mut trying = use_signal(|| false);
    rsx! {
        div { class: "flex flex-col", "data-voice-settings": true,
            if let Some(err) = problem() {
                p { role: "alert", class: "mb-2 rounded-lg bg-danger/10 px-3 py-2 text-[13px] text-danger", "{err}" }
            }
            h3 { class: SECTION, "Talk to bots" }
            div { class: CARD,
                div { class: ROW,
                    span { class: "flex min-w-0 flex-1 flex-col",
                        span { class: "text-[14px] text-ink", "Talk to bots" }
                        span { class: "text-[12px] text-ink-secondary", "A microphone beside Send, and voice chat with a mute button." }
                    }
                    crate::settings::Switch { checked: listen_on, disabled: false, label: "Talk to bots", on_toggle: move |_| change(info, json!({ "listen": { "on": !listen_on } }), problem) }
                }
                if listen_on {
                    div { class: ROW,
                        span { class: "min-w-0 flex-1 text-[14px] text-ink", "Listening runs on" }
                        Choice { value: listen_place.clone(), label: "Listening runs on", options: vec![("local".into(), "This computer".into()), ("online".into(), "Online (OpenAI)".into())], on_pick: move |p: String| change(info, json!({ "listen": { "where": p } }), problem) }
                    }
                    if listen_place == "local" {
                        div { class: ROW,
                            span { class: "flex min-w-0 flex-1 flex-col",
                                span { class: "text-[14px] text-ink", "Whisper model" }
                                span { class: "text-[12px] text-ink-secondary", "Private and free: your voice stays on this computer." }
                            }
                            select { class: SELECT, "aria-label": "Whisper model", "data-voice-model": true,
                                onchange: move |e| change(info, json!({ "listen": { "model": e.value() } }), problem),
                                for m in models.iter() {
                                    option { value: "{m[\"id\"].as_str().unwrap_or_default()}", selected: m["id"] == model.as_str(), "{m[\"label\"].as_str().unwrap_or_default()} · {m[\"size\"].as_str().unwrap_or_default()}" }
                                }
                            }
                            PartState { state: model_state.clone(), size: model_size.clone(), label: "listen", on_get: { let model = model.clone(); move |_| get(info, &model, problem) } }
                        }
                    } else {
                        div { class: ROW,
                            span { class: "min-w-0 flex-1 text-[12px] text-ink-secondary", "Your recording goes to OpenAI to be written out (about 0.3¢ a minute). It needs the OpenAI key below." }
                        }
                    }
                    div { class: ROW,
                        span { class: "min-w-0 flex-1 text-[14px] text-ink", "After you speak" }
                        Choice { value: if send_at_once { "send".to_string() } else { "check".to_string() }, label: "After you speak", options: vec![("check".into(), "Put the words in the box".into()), ("send".into(), "Send right away".into())], on_pick: move |p: String| change(info, json!({ "listen": { "sendAtOnce": p == "send" } }), problem) }
                    }
                }
            }
            h3 { class: SECTION, "Voice reply" }
            div { class: CARD,
                div { class: ROW,
                    span { class: "flex min-w-0 flex-1 flex-col",
                        span { class: "text-[14px] text-ink", "Turn on voice reply" }
                        span { class: "text-[12px] text-ink-secondary", "Bots read each new reply aloud; a speaker button on each reply plays it again." }
                    }
                    crate::settings::Switch { checked: reply_on, disabled: false, label: "Turn on voice reply", on_toggle: move |_| change(info, json!({ "reply": { "on": !reply_on } }), problem) }
                }
                div { class: ROW,
                    span { class: "flex min-w-0 flex-1 flex-col",
                        span { class: "text-[14px] text-ink", "Speaking runs on" }
                        span { class: "text-[12px] text-ink-secondary", "Voice chat uses this too, whether or not replies are read aloud." }
                    }
                    Choice { value: reply_place.clone(), label: "Speaking runs on", options: vec![("local".into(), "This computer".into()), ("online".into(), "Online".into())], on_pick: move |p: String| change(info, json!({ "reply": { "where": p } }), problem) }
                }
                if reply_place == "local" {
                    div { class: ROW,
                        span { class: "flex min-w-0 flex-1 flex-col",
                            span { class: "text-[14px] text-ink", "Voice" }
                            span { class: "text-[12px] text-ink-secondary", "Kokoro sounds more natural; Piper is quicker on a slow computer." }
                        }
                        Choice { value: local_engine.clone(), label: "Voice on this computer", options: vec![("kokoro".into(), "Kokoro".into()), ("piper".into(), "Piper".into())], on_pick: move |p: String| change(info, json!({ "reply": { "local": p } }), problem) }
                        PartState { state: v["local"][&local_engine].clone(), size: if local_engine == "kokoro" { "360 MB".to_string() } else { "75 MB".to_string() }, label: "{local_engine}", on_get: { let e = local_engine.clone(); move |_| get(info, &e, problem) } }
                    }
                } else {
                    div { class: ROW,
                        span { class: "flex min-w-0 flex-1 flex-col",
                            span { class: "text-[14px] text-ink", "Service" }
                            span { class: "text-[12px] text-ink-secondary", "OpenAI: about 1.5¢ a minute of speech. ElevenLabs: the most human, about 5–10¢ per 1,000 letters." }
                        }
                        Choice { value: online_engine.clone(), label: "Online voice", options: vec![("openai".into(), "OpenAI".into()), ("elevenlabs".into(), "ElevenLabs".into())], on_pick: move |p: String| change(info, json!({ "reply": { "online": p } }), problem) }
                    }
                }
                div { class: ROW,
                    span { class: "min-w-0 flex-1 text-[14px] text-ink", "Default voice" }
                    if voices.is_empty() {
                        span { class: "text-[13px] text-ink-secondary", if engine == "elevenlabs" { "Add an ElevenLabs key below" } else { "—" } }
                    } else {
                        select { class: SELECT, "aria-label": "Default voice", "data-voice-default": true,
                            onchange: move |e| change(info, json!({ "reply": { "voices": { engine_for_voice.clone(): e.value() } } }), problem),
                            for (id, name) in voices.iter() {
                                option { value: "{id}", selected: *id == chosen, "{name}" }
                            }
                        }
                    }
                    button { r#type: "button", "data-voice-try": true, class: SMALL_BUTTON, disabled: trying(),
                        onclick: move |_| {
                            trying.set(true);
                            spawn(async move {
                                if let Err(err) = say("Hello! This is how replies will sound.", None).await {
                                    let mut problem = problem;
                                    problem.set(Some(err));
                                }
                                trying.set(false);
                            });
                        },
                        if trying() { "Playing…" } else { "Try it" }
                    }
                }
            }
            if at_home {
                h3 { class: SECTION, "Online keys" }
                div { class: CARD,
                    KeyRow { service: "openai", name: "OpenAI", hint: openai_key, line: "Listening and speaking online. The same key the OpenAI AI uses in Settings › AI.", page: "https://platform.openai.com/api-keys" }
                    KeyRow { service: "elevenlabs", name: "ElevenLabs", hint: eleven_key, line: "The most human-sounding reply voices.", page: "https://elevenlabs.io/app/settings/api-keys" }
                }
                p { class: "px-1 pt-2 text-[12px] text-ink-secondary", "Keys stay on this computer. Catbot shows only their last four characters and never gives them to bots. Online voice sends your recording, or the reply's words, to that company." }
            }
        }
    }
}

/// One service's key: its last four and Remove, or a box to paste one.
#[component]
fn KeyRow(service: &'static str, name: &'static str, hint: Option<String>, line: &'static str, page: &'static str) -> Element {
    let info = use_context::<VoiceInfo>();
    let mut typed = use_signal(String::new);
    let mut problem = use_signal(|| None::<String>);
    let mut shown = info.0;
    let save = move |_| {
        let key = typed.peek().trim().to_string();
        if key.is_empty() {
            return;
        }
        spawn(async move {
            match api::call("PUT", &format!("/api/voice/keys/{service}"), Some(&json!({ "key": key }))).await {
                Ok(found) => {
                    shown.set(found);
                    typed.set(String::new());
                    problem.set(None);
                }
                Err(err) => problem.set(Some(err)),
            }
        });
    };
    rsx! {
        div { class: ROW, "data-voice-key": "{service}",
            span { class: "flex min-w-0 flex-1 flex-col",
                span { class: "text-[14px] text-ink", "{name}" }
                span { class: "text-[12px] text-ink-secondary", "{line} " a { href: "{page}", target: "_blank", rel: "noreferrer", class: "underline", "Get a key" } }
                if let Some(err) = problem() {
                    span { role: "alert", class: "text-[12px] text-danger", "{err}" }
                }
            }
            match hint {
                Some(hint) => rsx! {
                    span { class: "font-mono text-[13px] text-ink-secondary", "{hint}" }
                    button { r#type: "button", class: SMALL_BUTTON, "data-voice-key-remove": "{service}",
                        onclick: move |_| {
                            spawn(async move {
                                match api::call("DELETE", &format!("/api/voice/keys/{service}"), None).await {
                                    Ok(found) => shown.set(found),
                                    Err(err) => problem.set(Some(err)),
                                }
                            });
                        },
                        "Remove"
                    }
                },
                None => rsx! {
                    input { r#type: "password", autocomplete: "off", placeholder: "Paste the key", "aria-label": "{name} key", "data-voice-key-input": "{service}",
                        class: "w-48 rounded-lg bg-raised px-2 py-1 text-[13px] text-ink ring-1 ring-ink/10 focus:outline-none",
                        value: "{typed}", oninput: move |e| typed.set(e.value()),
                    }
                    button { r#type: "button", class: SMALL_BUTTON, "data-voice-key-save": "{service}", disabled: typed().trim().is_empty(), onclick: save, "Save" }
                },
            }
        }
    }
}

/// The bot panel's Voice row: this bot's own voice for the engine replies use now.
#[component]
pub fn BotVoice(bot_id: String) -> Element {
    let info = use_context::<VoiceInfo>();
    let problem = use_signal(|| None::<String>);
    let mut eleven_voices = use_signal(Vec::<Value>::new);
    let v = info.0.read().clone();
    if v.is_null() || !(info.reply_on() || info.listen_on()) {
        return rsx! {};
    }
    let engine = info.reply_engine();
    let wants_eleven = engine == "elevenlabs";
    use_effect(use_reactive!(|wants_eleven| {
        if wants_eleven {
            spawn(async move {
                if let Ok(found) = api::get("/api/voice/elevenlabs-voices").await {
                    eleven_voices.set(found["voices"].as_array().cloned().unwrap_or_default());
                }
            });
        }
    }));
    let list: Vec<Value> = if wants_eleven { eleven_voices.read().clone() } else { v["voices"][&engine].as_array().cloned().unwrap_or_default() };
    let mine = v["settings"]["bots"][&bot_id][&engine].as_str().unwrap_or("").to_string();
    rsx! {
        section { "data-panel-card": "voice",
            div { class: "flex items-center gap-2 px-2 py-1.5 text-[13px]",
                span { class: "text-ink-secondary", Icon { name: "volume", size: 14 } }
                span { class: "min-w-0 flex-1 text-ink", "Voice" }
                select { class: SELECT, "aria-label": "This bot's voice", "data-bot-voice": true,
                    onchange: move |e| change(info, json!({ "bots": { bot_id.clone(): { engine.clone(): e.value() } } }), problem),
                    option { value: "", selected: mine.is_empty(), "Default" }
                    for x in list.iter() {
                        option { value: "{x[\"id\"].as_str().unwrap_or_default()}", selected: x["id"] == mine.as_str(), "{x[\"name\"].as_str().unwrap_or_default()}" }
                    }
                }
            }
            if let Some(err) = problem() {
                p { class: "px-2 text-[12px] text-danger", "{err}" }
            }
        }
    }
}

// ---- Playing and recording ------------------------------------------------------------------

/// The page's side of voice: recording (as 16 kHz WAV, with a simple ear for when someone starts
/// and stops talking), muting, and playing audio. Put in place once.
const VOICE_JS: &str = r#"
if (!window.catbotVoice) {
  const V = window.catbotVoice = { stream: null, ctx: null, node: null, src: null, chunks: [], pre: [], muted: false, paused: false,
    ear: false, talking: false, loud: 0, quiet: 0, floor: 0.01, heard: 0, emit: () => {}, audio: null };
  V.open = async (ear) => {
    V.close();
    V.stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 } });
    V.ctx = new (window.AudioContext || window.webkitAudioContext)();
    V.src = V.ctx.createMediaStreamSource(V.stream);
    V.node = V.ctx.createScriptProcessor(4096, 1, 1);
    Object.assign(V, { ear, chunks: [], pre: [], talking: false, loud: 0, quiet: 0, heard: 0, muted: false, paused: false });
    V.node.onaudioprocess = (e) => {
      const d = e.inputBuffer.getChannelData(0);
      if (!V.ctx || V.muted || V.paused) { V.loud = 0; return; }
      let sum = 0; for (let i = 0; i < d.length; i++) sum += d[i] * d[i];
      const rms = Math.sqrt(sum / d.length), copy = new Float32Array(d), secs = d.length / V.ctx.sampleRate;
      if (!V.ear) { V.chunks.push(copy); return; }
      const threshold = Math.max(0.02, V.floor * 3);
      if (!V.talking) {
        V.pre.push(copy); while (V.pre.length > 4) V.pre.shift();
        if (rms > threshold) { V.loud += secs; if (V.loud > 0.15) { V.talking = true; V.chunks = V.pre.slice(); V.pre = []; V.quiet = 0; V.heard = 0; V.emit({ kind: "talking" }); } }
        else { V.loud = 0; V.floor = V.floor * 0.95 + rms * 0.05; }
      } else {
        V.chunks.push(copy); V.heard += secs;
        if (rms < threshold * 0.7) V.quiet += secs; else V.quiet = 0;
        if (V.quiet > 1.2 || V.heard > 90) { V.talking = false; V.loud = 0; const wav = V.wav(); V.emit({ kind: "said", wav }); }
      }
    };
    V.src.connect(V.node); V.node.connect(V.ctx.destination);
  };
  V.wav = () => {
    const rate = V.ctx ? V.ctx.sampleRate : 48000, n = V.chunks.reduce((a, c) => a + c.length, 0), all = new Float32Array(n);
    let at = 0; for (const c of V.chunks) { all.set(c, at); at += c.length; }
    V.chunks = [];
    const step = rate / 16000, len = Math.floor(n / step), pcm = new DataView(new ArrayBuffer(44 + len * 2));
    const put = (o, s) => { for (let i = 0; i < s.length; i++) pcm.setUint8(o + i, s.charCodeAt(i)); };
    put(0, "RIFF"); pcm.setUint32(4, 36 + len * 2, true); put(8, "WAVEfmt "); pcm.setUint32(16, 16, true); pcm.setUint16(20, 1, true);
    pcm.setUint16(22, 1, true); pcm.setUint32(24, 16000, true); pcm.setUint32(28, 32000, true); pcm.setUint16(32, 2, true); pcm.setUint16(34, 16, true);
    put(36, "data"); pcm.setUint32(40, len * 2, true);
    for (let i = 0; i < len; i++) { const x = Math.max(-1, Math.min(1, all[Math.floor(i * step)] || 0)); pcm.setInt16(44 + i * 2, x < 0 ? x * 0x8000 : x * 0x7fff, true); }
    const bytes = new Uint8Array(pcm.buffer); let bin = "";
    for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(bin);
  };
  V.close = () => {
    try { V.node && V.node.disconnect(); V.src && V.src.disconnect(); } catch (e) {}
    if (V.stream) V.stream.getTracks().forEach((t) => t.stop());
    if (V.ctx) V.ctx.close().catch(() => {});
    Object.assign(V, { stream: null, ctx: null, node: null, src: null, talking: false });
  };
  V.mute = (on) => { V.muted = on; if (V.stream) V.stream.getAudioTracks().forEach((t) => (t.enabled = !on)); if (on) { V.talking = false; V.chunks = []; } };
  V.pause = (on) => { V.paused = on; if (on) { V.talking = false; V.chunks = []; V.pre = []; } };
  V.play = (b64, type) => new Promise((done) => {
    V.stopPlaying();
    const bin = atob(b64), bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const url = URL.createObjectURL(new Blob([bytes], { type }));
    const audio = V.audio = new Audio(url);
    const end = () => { URL.revokeObjectURL(url); if (V.audio === audio) V.audio = null; done(true); };
    audio.onended = end; audio.onerror = end; audio._end = end;
    audio.play().catch(end);
  });
  V.stopPlaying = () => { const a = V.audio; if (a) { a.pause(); a._end && a._end(); } };
}
"#;

/// The Linux desktop app's web view (WebKitGTK) has the microphone switched off and turns down
/// every request for it. This switches it on and lets Catbot's own page have the microphone
/// (only the microphone: a request that wants the camera too is turned down).
#[cfg(all(feature = "desktop", target_os = "linux"))]
pub fn let_the_window_listen() {
    use dioxus::desktop::wry::WebViewExtUnix;
    use webkit2gtk::glib::object::Cast;
    use webkit2gtk::{PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, UserMediaPermissionRequestExt, WebViewExt};
    let view = dioxus::desktop::window().webview.webview();
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_enable_media_stream(true);
    }
    view.connect_permission_request(|_, request| match request.downcast_ref::<UserMediaPermissionRequest>() {
        Some(media) if media.is_for_audio_device() && !media.is_for_video_device() => {
            request.allow();
            true
        }
        Some(_) => {
            request.deny();
            true
        }
        None => false,
    });
}

/// Makes sure the page's voice helper is there.
pub fn ready_js() {
    document::eval(VOICE_JS);
}

/// Says the words (in this bot's voice, if given) and waits until they've been played.
pub async fn say(text: &str, bot: Option<&str>) -> Result<(), String> {
    let (audio, kind) = api::post_for_bytes("/api/voice/speak", &json!({ "text": text, "botId": bot })).await?;
    play(&audio, &kind).await;
    Ok(())
}

/// Plays audio bytes and waits for them to end (or be stopped).
pub async fn play(audio: &[u8], kind: &str) {
    ready_js();
    let kind = if kind.is_empty() { "audio/mpeg" } else { kind };
    let mut ev = document::eval(&format!("await window.catbotVoice.play(\"{}\", {}); dioxus.send(true);", b64_encode(audio), serde_json::to_string(kind).unwrap_or_default()));
    let _ = ev.recv::<bool>().await;
}

pub fn stop_playing() {
    document::eval("window.catbotVoice && window.catbotVoice.stopPlaying();");
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut buf, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let Some(v) = B64.iter().position(|&b| b == c) else { continue };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}

// ---- In the chat ---------------------------------------------------------------------------------

/// What's being read aloud in this chat, and whether voice chat is open. One per chat.
#[derive(Clone, Copy)]
pub struct Speaker {
    /// Voice chat is open (instead of the message box).
    pub chat_open: Signal<bool>,
    /// The reply being read now.
    pub playing: Signal<Option<String>>,
    queue: Signal<VecDeque<(String, String)>>,
    pub problem: Signal<Option<String>>,
    bot: Signal<String>,
}

impl Speaker {
    pub fn new(bot: String) -> Self {
        Speaker { chat_open: Signal::new(false), playing: Signal::new(None), queue: Signal::new(VecDeque::new()), problem: Signal::new(None), bot: Signal::new(bot) }
    }

    /// Reads a reply after any before it.
    pub fn read(mut self, id: String, text: String) {
        self.queue.write().push_back((id, text));
        if self.playing.peek().is_none() {
            self.next();
        }
    }

    /// Reads this reply now, stopping whatever was being read.
    pub fn read_now(mut self, id: String, text: String) {
        self.queue.write().clear();
        stop_playing();
        self.queue.write().push_back((id, text));
        self.playing.set(None);
        self.next();
    }

    pub fn stop(mut self) {
        self.queue.write().clear();
        stop_playing();
        self.playing.set(None);
    }

    fn next(mut self) {
        let Some((id, text)) = self.queue.write().pop_front() else {
            self.playing.set(None);
            return;
        };
        self.playing.set(Some(id.clone()));
        let bot = self.bot.peek().clone();
        spawn(async move {
            if let Err(err) = say(&text, Some(&bot)).await {
                let mut problem = self.problem;
                problem.set(Some(err));
                self.queue.write().clear();
            }
            // Only if nothing else took over meanwhile.
            if self.playing.peek().as_deref() == Some(id.as_str()) {
                self.next();
            }
        });
    }
}

/// Reads new replies aloud as they arrive (with voice reply on, or in voice chat). Replies already
/// there when the chat opened are left alone.
#[component]
pub fn Reader(replies: Vec<(String, String)>, loaded: bool) -> Element {
    let info = use_context::<VoiceInfo>();
    let speaker = use_context::<Speaker>();
    let mut seen = use_signal(|| None::<HashSet<String>>);
    use_drop(move || speaker.stop());
    use_effect(use_reactive!(|replies, loaded| {
        if !loaded {
            return;
        }
        let first = seen.peek().is_none();
        if first {
            seen.set(Some(replies.iter().map(|(id, _)| id.clone()).collect()));
            return;
        }
        let new: Vec<(String, String)> = replies.iter().filter(|(id, _)| !seen.peek().as_ref().is_some_and(|s| s.contains(id))).cloned().collect();
        if new.is_empty() {
            return;
        }
        if let Some(s) = seen.write().as_mut() {
            s.extend(new.iter().map(|(id, _)| id.clone()));
        }
        if info.reply_on() || *speaker.chat_open.peek() {
            for (id, text) in new {
                speaker.read(id, text);
            }
        }
    }));
    rsx! {}
}

/// The speaker button beside a reply: reads it (again), or stops it.
#[component]
pub fn SpeakButton(id: String, text: String) -> Element {
    let info = use_context::<VoiceInfo>();
    let Some(speaker) = try_use_context::<Speaker>() else { return rsx! {} };
    if !(info.reply_on() || info.listen_on()) || id == "live" {
        return rsx! {};
    }
    let on = speaker.playing.read().as_deref() == Some(id.as_str());
    let label = if on { "Stop reading" } else { "Read aloud" };
    let class = if on {
        "flex size-6 shrink-0 items-center justify-center self-end rounded-md text-accent"
    } else {
        "flex size-6 shrink-0 items-center justify-center self-end rounded-md text-ink-tertiary opacity-0 transition-opacity hover:bg-raised hover:text-ink group-hover:opacity-100 focus:opacity-100"
    };
    rsx! {
        button { r#type: "button", "aria-label": "{label}", title: "{label}", "data-speak": "{id}", class,
            onclick: move |_| if on { speaker.stop() } else { speaker.read_now(id.clone(), text.clone()) },
            Icon { name: "volume", size: 14 }
        }
    }
}

/// Opens the microphone (no ear: it records until stopped).
async fn open_mic() -> Result<(), String> {
    ready_js();
    let mut ev = document::eval(
        r#"try { await window.catbotVoice.open(false); dioxus.send(""); }
           catch (e) { dioxus.send((e && e.name === "NotAllowedError") ? "denied" : String((e && e.message) || e)); }"#,
    );
    match ev.recv::<String>().await {
        Ok(said) if said.is_empty() => Ok(()),
        Ok(said) => Err(mic_problem(&said)),
        Err(_) => Err("The microphone didn't start.".into()),
    }
}

fn mic_problem(said: &str) -> String {
    if said == "denied" {
        "Catbot isn't allowed to use the microphone. Allow it for this app or page, then try again.".into()
    } else if said.contains("mediaDevices") || said.contains("undefined") {
        "This window can't reach a microphone. In a browser, Catbot needs to be opened on this computer (localhost) or over https.".into()
    } else {
        format!("The microphone didn't start: {said}")
    }
}

/// What was recorded, written out by Catbot (here or online, as set).
async fn words_in(wav_b64: &str) -> Result<String, String> {
    let bytes = b64_decode(wav_b64);
    let answer = api::upload("/api/voice/listen", "speech.wav", "audio/wav", bytes).await?;
    Ok(answer["text"].as_str().unwrap_or_default().trim().to_string())
}

/// The microphone beside Send: tap to talk, tap again to stop; the words then go to `on_words`.
#[component]
pub fn MicButton(disabled: bool, on_words: EventHandler<String>, on_problem: EventHandler<String>) -> Element {
    let mut recording = use_signal(|| false);
    let mut writing = use_signal(|| false);
    use_drop(move || {
        if *recording.peek() {
            document::eval("window.catbotVoice && window.catbotVoice.close();");
        }
    });
    let toggle = move |_| {
        if writing() {
            return;
        }
        if !recording() {
            spawn(async move {
                match open_mic().await {
                    Ok(()) => recording.set(true),
                    Err(err) => on_problem.call(err),
                }
            });
            return;
        }
        recording.set(false);
        writing.set(true);
        spawn(async move {
            let mut ev = document::eval("const V = window.catbotVoice; const w = V.wav(); V.close(); dioxus.send(w);");
            let wav = ev.recv::<String>().await.unwrap_or_default();
            match words_in(&wav).await {
                Ok(words) if words.is_empty() => on_problem.call("Catbot didn't catch any words.".into()),
                Ok(words) => on_words.call(words),
                Err(err) => on_problem.call(err),
            }
            writing.set(false);
        });
    };
    let (label, class) = if recording() {
        ("Stop and write out", "flex size-7 shrink-0 animate-pulse items-center justify-center rounded-full bg-danger text-white")
    } else if writing() {
        ("Writing out what you said", "flex size-7 shrink-0 items-center justify-center rounded-full bg-ink/[0.06] text-ink-tertiary")
    } else {
        ("Talk", "flex size-7 shrink-0 items-center justify-center rounded-full bg-ink/[0.06] text-ink-tertiary ring-[0.5px] ring-inset ring-ink/[0.08] hover:bg-ink/[0.09] hover:text-ink disabled:opacity-40")
    };
    rsx! {
        button { r#type: "button", "aria-label": "{label}", title: "{label}", "data-mic": if recording() { "recording" } else if writing() { "writing" } else { "idle" }, disabled: disabled || writing(), class, onclick: toggle,
            if writing() {
                span { class: "inline-block size-3.5 animate-spin rounded-full border-2 border-ink-tertiary border-t-transparent" }
            } else {
                Icon { name: "mic", size: 15 }
            }
        }
    }
}

/// Where voice chat is.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Starting,
    Listening,
    Hearing,
    Writing,
    Waiting,
    Problem(String),
}

/// Voice chat, in place of the message box: Catbot listens, sends what was said when the person
/// stops, reads the reply, and listens again. Mute stops it hearing the room; End closes it.
#[component]
pub fn VoiceChat(bot_id: String, thread: String, name: String, working: bool) -> Element {
    let mut state = use_context::<Signal<State>>();
    let speaker = use_context::<Speaker>();
    let mut phase = use_signal(|| Phase::Starting);
    let mut muted = use_signal(|| false);
    let mut heard = use_signal(String::new);
    let target = use_signal(|| (bot_id.clone(), thread.clone()));
    // Opens the microphone with its ear on; what it hears comes back here.
    use_hook(move || {
        ready_js();
        spawn(async move {
            let mut ev = document::eval(
                r#"const V = window.catbotVoice; V.emit = (m) => dioxus.send(m);
                   try { await V.open(true); dioxus.send({ kind: "open" }); }
                   catch (e) { dioxus.send({ kind: "problem", text: (e && e.name === "NotAllowedError") ? "denied" : String((e && e.message) || e) }); }
                   await new Promise(() => {});"#,
            );
            while let Ok(m) = ev.recv::<Value>().await {
                match m["kind"].as_str().unwrap_or_default() {
                    "open" => phase.set(Phase::Listening),
                    "talking" => phase.set(Phase::Hearing),
                    "problem" => phase.set(Phase::Problem(mic_problem(m["text"].as_str().unwrap_or_default()))),
                    "said" => {
                        phase.set(Phase::Writing);
                        document::eval("window.catbotVoice.pause(true);");
                        let wav = m["wav"].as_str().unwrap_or_default().to_string();
                        match words_in(&wav).await {
                            Ok(words) if words.is_empty() => {
                                phase.set(Phase::Listening);
                                document::eval("window.catbotVoice.pause(false);");
                            }
                            Ok(words) => {
                                heard.set(words.clone());
                                phase.set(Phase::Waiting);
                                let (bot, thread) = target.peek().clone();
                                let send = crate::live::send_id();
                                state.write().sending(&thread, crate::live::optimistic(&words, &send, crate::chat::js_now(), Vec::new()));
                                let body = json!({ "text": words, "threadId": thread, "sendId": send, "attachments": [] });
                                if let Err(err) = api::post(&format!("/api/bots/{bot}/messages"), &body).await {
                                    state.write().unsend(&thread, &send);
                                    phase.set(Phase::Problem(err));
                                }
                            }
                            Err(err) => phase.set(Phase::Problem(err)),
                        }
                    }
                    _ => {}
                }
            }
        });
    });
    let mut open = speaker.chat_open;
    use_drop(move || {
        document::eval("window.catbotVoice && window.catbotVoice.close();");
        speaker.stop();
        open.set(false);
    });
    // The bot done and nothing left to read: listen again. While a reply is read, the microphone
    // rests, so it doesn't hear the bot.
    let playing = speaker.playing.read().is_some();
    let now = phase();
    use_effect(use_reactive!(|working, playing, now| {
        if now == Phase::Waiting && !working && !playing {
            phase.set(Phase::Listening);
        }
        let rest = playing || matches!(now, Phase::Writing | Phase::Waiting);
        document::eval(&format!("window.catbotVoice && window.catbotVoice.pause({rest});"));
    }));
    let toggle_mute = move |_| {
        let on = !muted();
        muted.set(on);
        document::eval(&format!("window.catbotVoice && window.catbotVoice.mute({on});"));
        if !on && *phase.peek() == Phase::Hearing {
            phase.set(Phase::Listening);
        }
    };
    let (status, orb) = match (&now, muted(), playing) {
        (Phase::Problem(_), _, _) => ("Voice chat stopped".to_string(), "bg-danger/20"),
        (_, _, true) => (format!("{name} is speaking"), "animate-pulse bg-accent scale-110"),
        (_, true, _) => ("Muted: Catbot isn't listening".to_string(), "bg-control"),
        (Phase::Starting, _, _) => ("Opening the microphone…".to_string(), "bg-control animate-pulse"),
        (Phase::Listening, _, _) => ("Listening".to_string(), "bg-accent/40 animate-pulse"),
        (Phase::Hearing, _, _) => ("Hearing you…".to_string(), "bg-accent scale-110"),
        (Phase::Writing, _, _) => ("Writing out what you said…".to_string(), "bg-accent/60 animate-pulse"),
        (Phase::Waiting, _, _) => (format!("{name} is thinking…"), "bg-accent/60 animate-pulse"),
    };
    let problem = match &now {
        Phase::Problem(p) => Some(p.clone()),
        _ => speaker.problem.read().clone(),
    };
    let mute_label = if muted() { "Unmute" } else { "Mute" };
    let (orb_icon, mute_icon): (&'static str, &'static str) = if muted() { ("mic-off", "mic-off") } else { ("audio-lines", "mic") };
    let mute_class = if muted() {
        "flex h-11 items-center gap-2 rounded-full bg-danger px-4 text-[14px] font-medium text-white"
    } else {
        "flex h-11 items-center gap-2 rounded-full bg-raised px-4 text-[14px] font-medium text-ink ring-1 ring-ink/10 hover:bg-raised-hover"
    };
    rsx! {
        div { class: "pointer-events-none relative shrink-0 px-5 pb-3",
            div { class: "chat-column pointer-events-auto relative w-full",
                div { "data-voice-chat": true, "data-voice-phase": "{status}", class: "flex flex-col items-center gap-3 rounded-[22px] bg-composer px-4 py-4 ring-1 ring-ink/[0.08] shadow-sm",
                    div { class: "flex size-16 items-center justify-center rounded-full transition-transform duration-300 {orb}",
                        span { class: "text-ink", Icon { name: orb_icon, size: 26 } }
                    }
                    p { role: "status", class: "text-[14px] font-medium text-ink", "{status}" }
                    if !heard().is_empty() {
                        p { class: "max-w-full truncate text-[12px] text-ink-tertiary", "You said: {heard}" }
                    }
                    if let Some(p) = problem {
                        p { role: "alert", class: "text-center text-[12px] text-danger", "{p}" }
                    }
                    div { class: "flex items-center gap-2",
                        button { r#type: "button", "data-voice-mute": if muted() { "on" } else { "off" }, "aria-pressed": "{muted()}", class: mute_class, onclick: toggle_mute,
                            Icon { name: mute_icon, size: 16 }
                            "{mute_label}"
                        }
                        if playing {
                            button { r#type: "button", "data-voice-skip": true, class: "flex h-11 items-center gap-2 rounded-full bg-raised px-4 text-[14px] text-ink ring-1 ring-ink/10 hover:bg-raised-hover", onclick: move |_| speaker.stop(), "Stop speaking" }
                        }
                        button { r#type: "button", "data-voice-end": true, class: "flex h-11 items-center gap-2 rounded-full bg-raised px-4 text-[14px] text-ink ring-1 ring-ink/10 hover:bg-raised-hover", onclick: move |_| open.set(false),
                            Icon { name: "x", size: 16 }
                            "End"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_goes_both_ways() {
        for text in ["", "a", "ab", "abc", "RIFF\u{0}\u{1}\u{ff}voice"] {
            let bytes = text.as_bytes();
            assert_eq!(b64_decode(&b64_encode(bytes)), bytes);
        }
        assert_eq!(b64_encode(b"Catbot"), "Q2F0Ym90");
        assert_eq!(b64_encode(b"Cat"), "Q2F0");
        assert_eq!(b64_encode(b"Ca"), "Q2E=");
    }
}
