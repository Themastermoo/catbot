//! Pictures (stage 13.7, Moo's decisions of 2026-11-16).
//!
//! - A picture starts with `/image <what>` typed to a bot, or with the bot asking for one in its
//!   reply (`<make-image>what</make-image>`, which the bot is told about).
//! - Either way a card asks which AI should draw it, offering those that are set up: OpenAI's
//!   image model and xAI's (with their saved keys), and Claude working a drawing program: SVG
//!   (rendered here, nothing to install) or Blender (when it's installed). A Blender script is
//!   shown before it runs.
//! - The picture is saved in the bot's folder like an attachment and shown in the chat.
//!
//! Routes: `POST /api/bots/:id/images` `{ prompt, threadId }`, and
//! `POST /api/threads/:thread/images/:message` `{ choice }` / `{ run: true }` / `{ cancel: true }`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine as _;
use serde_json::{Value, json};

use super::{Answer, App, bad, fail, turns, wire};
use crate::claude::{self, Event, PermissionMode, Session, Turn};
use crate::engines;
use crate::store::{Bot, Role};

/// How long a picture may take.
const PATIENCE: Duration = Duration::from_secs(300);

/// The image models used unless the settings name others (`imageModels`: `{ "openai": …, "xai": … }`).
pub const OPENAI_MODEL: &str = "gpt-image-2";
pub const XAI_MODEL: &str = "grok-imagine-image-2.0";

/// What bots are told about asking for a picture.
pub const BOT_NOTE: &str = "\n\nYou can make pictures. When a picture would help, or the person asks for one, put \
    <make-image>a full description of the picture</make-image> in your reply (one per picture). Catbot asks the \
    person which AI should draw it, then shows the picture in the chat and saves it in your folder under attachments/.";

/// The AIs that can draw now: (id, label).
pub fn options(app: &App) -> Vec<(String, String)> {
    let models = app.with_store(|store| store.setting("imageModels")).flatten().unwrap_or(Value::Null);
    let mut found = Vec::new();
    for (id, default) in [("openai", OPENAI_MODEL), ("xai", XAI_MODEL)] {
        if super::backups::ready(app, id) {
            let model = models[id].as_str().unwrap_or(default);
            let name = engines::find(id).map(|e| e.name).unwrap_or(id);
            found.push((id.to_string(), format!("{name} · {model}")));
        }
    }
    found.push(("claude-svg".into(), "Claude, drawing in SVG".into()));
    if blender().is_some() {
        found.push(("claude-blender".into(), "Claude, building it in Blender".into()));
    }
    found
}

/// Blender, if it's installed (`CATBOT_BLENDER` names another).
fn blender() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CATBOT_BLENDER") {
        return Some(PathBuf::from(path)).filter(|p| p.is_file());
    }
    let names: &[&str] = if cfg!(windows) { &["blender.exe"] } else { &["blender"] };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(windows) {
        dirs.push(PathBuf::from(r"C:\Program Files\Blender Foundation"));
    } else {
        dirs.push(PathBuf::from("/snap/bin"));
        dirs.push(PathBuf::from("/var/lib/flatpak/exports/bin"));
    }
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// `POST /api/bots/:id/images`: the person's `/image …` message, and the card asking which AI.
pub fn ask(app: &Arc<App>, bot: &Bot, body: &Value) -> Answer {
    let prompt = body["prompt"].as_str().map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| bad("what should the picture show?"))?;
    let thread = body["threadId"].as_str().ok_or_else(|| bad("which chat?"))?.to_string();
    let conversation = app.try_store(|store| store.conversation(&thread)).map_err(fail)?.filter(|c| c.bot_id == bot.id).ok_or_else(|| bad("no such chat"))?;
    let detail = body["sendId"].as_str().map(|id| json!({ "sendId": id }));
    let message = turns::save_and_send(app, &conversation.id, Role::Person, &format!("/image {prompt}"), detail.as_ref())
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "couldn't save the message".to_string()))?;
    let card = card(app, &conversation.id, prompt, "you");
    Ok((StatusCode::ACCEPTED, json!({ "message": message, "card": card })))
}

/// Puts up the card asking which AI should draw `prompt`; `asked_by` is "you" or the bot.
pub fn card(app: &App, thread: &str, prompt: &str, asked_by: &str) -> Option<Value> {
    let options: Vec<Value> = options(app).into_iter().map(|(id, label)| json!({ "id": id, "label": label })).collect();
    let detail = json!({ "kind": "image", "prompt": prompt, "askedBy": asked_by, "state": "choosing", "options": options });
    turns::save_and_send(app, thread, Role::Step, &format!("Picture: {prompt}"), Some(&detail))
}

/// Takes `<make-image>…</make-image>` out of a bot's reply: the reply without them, and their prompts.
pub fn take_requests(reply: &str) -> (String, Vec<String>) {
    let mut rest = String::new();
    let mut prompts = Vec::new();
    let mut text = reply;
    while let Some(start) = text.find("<make-image>") {
        rest.push_str(&text[..start]);
        let after = &text[start + "<make-image>".len()..];
        match after.find("</make-image>") {
            Some(end) => {
                let prompt = after[..end].trim();
                if !prompt.is_empty() {
                    prompts.push(prompt.to_string());
                }
                text = &after[end + "</make-image>".len()..];
            }
            None => {
                text = "";
            }
        }
    }
    rest.push_str(text);
    let rest = rest.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string();
    (rest, prompts)
}

/// `POST /api/threads/:thread/images/:message`: a choice, a go-ahead for a script, or no.
pub async fn answer(app: &Arc<App>, thread: &str, message: &str, body: &Value) -> Answer {
    let id = wire::parse_message_id(message).ok_or_else(|| bad("which picture?"))?;
    let row = app
        .try_store(|store| store.message_in(thread, id))
        .map_err(fail)?
        .filter(|m| m.detail.as_ref().is_some_and(|d| d["kind"] == "image"))
        .ok_or((StatusCode::NOT_FOUND, "no such picture card in this chat".to_string()))?;
    let mut detail = row.detail.clone().unwrap_or(Value::Null);
    let conversation = app.try_store(|store| store.conversation(thread)).map_err(fail)?.ok_or_else(|| bad("no such chat"))?;
    let bot = app.try_store(|store| store.bot_by_id(&conversation.bot_id)).map_err(fail)?.ok_or_else(|| bad("no such bot"))?;
    let state = detail["state"].as_str().unwrap_or_default().to_string();
    if body["cancel"] == true {
        if !["choosing", "confirming"].contains(&state.as_str()) {
            return Err((StatusCode::CONFLICT, "it's too late to cancel".into()));
        }
        detail["state"] = json!("cancelled");
        update(app, thread, id, &detail);
        return Ok((StatusCode::OK, json!({ "ok": true })));
    }
    if body["run"] == true {
        if state != "confirming" {
            return Err((StatusCode::CONFLICT, "there's no script waiting".into()));
        }
        detail["state"] = json!("drawing");
        update(app, thread, id, &detail);
        spawn_drawing(app.clone(), bot, thread.to_string(), id, detail);
        return Ok((StatusCode::ACCEPTED, json!({ "ok": true })));
    }
    let choice = body["choice"].as_str().ok_or_else(|| bad("which AI?"))?.to_string();
    // A failed picture can be tried again with another AI.
    if state != "choosing" && state != "failed" {
        return Err((StatusCode::CONFLICT, "an AI was already chosen".into()));
    }
    if let Some(map) = detail.as_object_mut() {
        map.remove("error");
        map.remove("script");
    }
    let label = options(app).into_iter().find(|(o, _)| *o == choice).map(|(_, l)| l).ok_or_else(|| bad("that AI can't draw right now"))?;
    detail["choice"] = json!(choice);
    detail["choiceLabel"] = json!(label);
    detail["state"] = json!(if choice == "claude-blender" { "writing" } else { "drawing" });
    update(app, thread, id, &detail);
    spawn_drawing(app.clone(), bot, thread.to_string(), id, detail);
    Ok((StatusCode::ACCEPTED, json!({ "ok": true })))
}

/// Saves a card's new state and sends it to every open interface.
fn update(app: &App, thread: &str, id: i64, detail: &Value) {
    let wire = app.with_store(|store| {
        store.update_detail(id, detail)?;
        let parent = store.previous_message_id(thread, id)?;
        Ok(wire::message(&store.message(id)?.ok_or("message vanished")?, parent))
    });
    if let Some(wire) = wire {
        app.hub.send(json!({ "kind": "message.patch", "threadId": thread, "message": wire }));
    }
    super::inbox::changed(app);
}

fn spawn_drawing(app: Arc<App>, bot: Bot, thread: String, id: i64, mut detail: Value) {
    tokio::spawn(async move {
        let prompt = detail["prompt"].as_str().unwrap_or_default().to_string();
        let choice = detail["choice"].as_str().unwrap_or_default().to_string();
        let result = tokio::time::timeout(PATIENCE, draw(&app, &bot, &choice, &prompt, &mut detail)).await.unwrap_or_else(|_| Err("it took too long".into()));
        match result {
            Ok(Some(png)) => match keep(&app, &bot, &thread, &prompt, &detail, png) {
                Ok(attachment) => {
                    detail["state"] = json!("done");
                    detail["attachment"] = attachment;
                }
                Err(err) => {
                    detail["state"] = json!("failed");
                    detail["error"] = json!(err);
                }
            },
            // A Blender script waiting to be looked at.
            Ok(None) => detail["state"] = json!("confirming"),
            Err(err) => {
                detail["state"] = json!("failed");
                detail["error"] = json!(err);
            }
        }
        update(&app, &thread, id, &detail);
    });
}

/// Draws a picture straight away, with OpenAI's, xAI's or Claude's SVG (a bot's own picture,
/// `profiles.rs`): its PNG.
pub async fn draw_now(app: &App, bot: &Bot, choice: &str, prompt: &str) -> Result<Vec<u8>, String> {
    if choice == "claude-blender" {
        return Err("Blender's script has to be looked at first, so it doesn't draw pictures here".into());
    }
    let mut detail = json!({});
    draw(app, bot, choice, prompt, &mut detail).await?.ok_or_else(|| "nothing was drawn".to_string())
}

/// Draws the picture: its PNG, or None when a script now waits for the person's go-ahead.
async fn draw(app: &App, bot: &Bot, choice: &str, prompt: &str, detail: &mut Value) -> Result<Option<Vec<u8>>, String> {
    match choice {
        "openai" | "xai" => from_api(app, choice, prompt).await.map(Some),
        "claude-svg" => {
            let reply = ask_claude(
                app,
                bot,
                "You draw pictures as SVG. Answer with one complete, self-contained <svg> element (with a viewBox, \
                 about 1024 units wide) and nothing else: no explanation, no Markdown fences, no external images or fonts.",
                &format!("Draw this picture: {prompt}"),
            )
            .await?;
            let svg = svg_in(&reply).ok_or("Claude's answer had no SVG in it")?;
            detail["source"] = json!(svg);
            render_svg(&svg).map(Some)
        }
        "claude-blender" => {
            if let Some(script) = detail["script"].as_str().filter(|_| detail["state"] == "drawing") {
                return run_blender(script).await.map(Some);
            }
            let reply = ask_claude(
                app,
                bot,
                "You write Blender Python (bpy) scripts that build a scene and render it. Start from an empty scene \
                 (delete the default objects), add the objects, materials, lights and a camera the picture needs, set \
                 the render to 1024 by 1024 PNG with EEVEE (or Cycles at 64 samples if EEVEE is missing), and render \
                 a still to the path after '--' on the command line (sys.argv). Touch no files but that one; don't \
                 use the network or run other programs. Answer with the script alone in one ```python block.",
                &format!("Write the script for this picture: {prompt}"),
            )
            .await?;
            let script = code_in(&reply).ok_or("Claude's answer had no script in it")?;
            detail["script"] = json!(script);
            Ok(None)
        }
        other => Err(format!("Catbot can't draw with {other}")),
    }
}

/// OpenAI's or xAI's image API.
async fn from_api(app: &App, engine_id: &str, prompt: &str) -> Result<Vec<u8>, String> {
    let engine = engines::find(engine_id).ok_or("no such AI")?;
    let saved = app.with_store(|store| store.engine_key(engine.id)).flatten();
    let access = engines::access(engine, saved.as_ref()).ok_or_else(|| format!("{} has no key saved", engine.name))?;
    let models = app.with_store(|store| store.setting("imageModels")).flatten().unwrap_or(Value::Null);
    let model = models[engine_id].as_str().unwrap_or(if engine_id == "xai" { XAI_MODEL } else { OPENAI_MODEL }).to_string();
    let mut body = json!({ "model": model, "prompt": prompt, "n": 1 });
    if engine_id == "xai" {
        body["response_format"] = json!("b64_json");
    } else {
        body["size"] = json!("1024x1024");
    }
    let client = reqwest::Client::builder().timeout(PATIENCE).build().map_err(|e| e.to_string())?;
    let mut request = client.post(format!("{}/images/generations", access.url)).json(&body);
    if let Some(key) = &access.key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|e| format!("couldn't reach {} ({e})", engine.name))?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let said = answer["error"]["message"].as_str().or(answer["error"].as_str()).unwrap_or("no reason given");
        return Err(format!("{} said no ({status}): {said}", engine.name));
    }
    let picture = &answer["data"][0];
    if let Some(b64) = picture["b64_json"].as_str() {
        return base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string());
    }
    let url = picture["url"].as_str().ok_or_else(|| format!("{} sent no picture", engine.name))?;
    let bytes = client.get(url).send().await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
    Ok(bytes.to_vec())
}

/// Asks Claude once, with no tools, in a scratch folder of Catbot's own.
async fn ask_claude(app: &App, bot: &Bot, system: &str, prompt: &str) -> Result<String, String> {
    let cwd = app.try_store(|store| Ok(store.home().join("drawing"))).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;
    let turn = Turn {
        prompt: prompt.to_string(),
        session: Session::New(uuid::Uuid::new_v4().to_string()),
        model: if bot.engine == engines::CLAUDE { bot.model.clone() } else { None },
        permission_mode: PermissionMode::Default,
        cwd,
        system_prompt: Some(system.to_string()),
        cli: app.cli.clone(),
        isolated: true,
        approval_helper: None,
        extra_dirs: Vec::new(),
        catbot_tools: None,
        effort: None,
    };
    let mut running = claude::start(&turn).await.map_err(|e| format!("couldn't start Claude ({e})"))?;
    let mut text = String::new();
    while let Some(event) = running.events.recv().await {
        match event {
            Event::Text { text: block } => text = block,
            Event::Finished(outcome) => {
                if !outcome.ok {
                    let said = outcome.result.unwrap_or_default();
                    return Err(if said.is_empty() { "Claude couldn't do it".into() } else { format!("Claude couldn't do it: {said}") });
                }
                return Ok(outcome.result.filter(|r| !r.trim().is_empty()).unwrap_or(text));
            }
            _ => {}
        }
    }
    Err("Claude stopped without an answer".into())
}

/// The `<svg …>…</svg>` in an answer.
pub fn svg_in(reply: &str) -> Option<String> {
    let start = reply.find("<svg")?;
    let end = reply.rfind("</svg>")? + "</svg>".len();
    (end > start).then(|| reply[start..end].to_string())
}

/// The code in an answer's first fenced block (or the whole answer, if there's no fence).
pub fn code_in(reply: &str) -> Option<String> {
    let code = match reply.find("```") {
        Some(start) => {
            let after = &reply[start + 3..];
            let body = after.split_once('\n').map(|(_, b)| b).unwrap_or(after);
            body.split("```").next().unwrap_or(body)
        }
        None => reply,
    };
    let code = code.trim();
    (!code.is_empty()).then(|| code.to_string())
}

/// An SVG as a PNG, 1024 pixels on its longer side.
pub fn render_svg(svg: &str) -> Result<Vec<u8>, String> {
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|e| format!("the SVG didn't read ({e})"))?;
    let size = tree.size();
    let scale = 1024.0 / size.width().max(size.height()).max(1.0);
    let (w, h) = ((size.width() * scale).ceil().max(1.0) as u32, (size.height() * scale).ceil().max(1.0) as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("the picture is too big")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Runs a Blender script without a window, in a scratch folder; the PNG it renders.
async fn run_blender(script: &str) -> Result<Vec<u8>, String> {
    let blender = blender().ok_or("Blender isn't installed")?;
    let dir = std::env::temp_dir().join(format!("catbot-blender-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (script_path, out) = (dir.join("scene.py"), dir.join("picture.png"));
    std::fs::write(&script_path, script).map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(&blender);
    command
        .args(["--background", "--factory-startup", "--python-exit-code", "1", "--python"])
        .arg(&script_path)
        .arg("--")
        .arg(&out)
        .current_dir(&dir)
        .kill_on_drop(true);
    let run = command.output().await.map_err(|e| format!("couldn't start Blender ({e})"))?;
    let png = std::fs::read(&out);
    let _ = std::fs::remove_dir_all(&dir);
    match png {
        Ok(png) if !png.is_empty() => Ok(png),
        _ => {
            let said = String::from_utf8_lossy(&run.stderr);
            let tail: String = said.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            Err(format!("Blender rendered nothing{}", if tail.is_empty() { String::new() } else { format!(":\n{tail}") }))
        }
    }
}

/// Saves the picture in the bot's folder and shows it in the chat.
fn keep(app: &App, bot: &Bot, thread: &str, prompt: &str, detail: &Value, png: Vec<u8>) -> Result<Value, String> {
    let dir = bot.folder.join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let slug: String = prompt.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let slug: String = slug.split('-').filter(|s| !s.is_empty()).take(6).collect::<Vec<_>>().join("-");
    let name = format!("picture-{}.png", if slug.is_empty() { "untitled" } else { &slug });
    let path = free_path(&dir, &format!("{}-{name}", chrono::Local::now().format("%Y-%m-%d")));
    std::fs::write(&path, &png).map_err(|e| e.to_string())?;
    let attachment = app.try_store(|store| store.add_attachment(&bot.id, &name, &path, "image/png", png.len() as u64)).map_err(|e| e.to_string())?;
    let described = super::attachments::describe(&attachment);
    let label = detail["choiceLabel"].as_str().unwrap_or("an AI");
    let file = described["file"].as_str().unwrap_or_default();
    let text = format!("Here's the picture, drawn by {label}: “{prompt}”. It's saved in my folder as attachments/{file}.");
    turns::save_and_send(app, thread, Role::Bot, &text, Some(&json!({ "attachments": [described], "picture": true })));
    Ok(described)
}

fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name.trim_end_matches(".png");
    (2..).map(|n| dir.join(format!("{stem} ({n}).png"))).find(|p| !p.exists()).unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_come_out_of_replies() {
        let (rest, prompts) = take_requests("Here's my idea.\n<make-image>a red barn at dusk</make-image>\nAnd another: <make-image> a fox </make-image>");
        assert_eq!(prompts, vec!["a red barn at dusk", "a fox"]);
        assert_eq!(rest, "Here's my idea.\n\nAnd another:");
        assert_eq!(take_requests("nothing here").1.len(), 0);
    }

    #[test]
    fn svg_and_code_are_found_and_svg_renders() {
        let svg = svg_in("Sure!\n```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 50\"><rect width=\"100\" height=\"50\" fill=\"red\"/></svg>\n```").unwrap();
        let png = render_svg(&svg).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert!(render_svg("<svg").is_err());
        assert_eq!(code_in("```python\nimport bpy\nprint(1)\n```").as_deref(), Some("import bpy\nprint(1)"));
        assert_eq!(code_in("import bpy").as_deref(), Some("import bpy"));
    }
}
