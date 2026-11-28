//! The AIs bots can run on, for the interface (`crate::engines`).
//!
//! - `GET /api/engines`: each engine, whether it's ready (a key saved or in the environment, or
//!   none needed), the last four characters of a saved key, its address and suggested models.
//!   Never a key itself.
//! - `GET /api/engines/:id/models`: the models the API itself lists, for the picker.
//! - `PUT /api/engines/:id/key` `{ key, url? }` and `DELETE /api/engines/:id/key`: save or forget
//!   a key. Only at home: the remote door offers the first two and not these.
//! - Codex (12.4.1): `GET /api/engines/codex/account` says who it's signed in as;
//!   `POST /api/engines/codex/sign-in` `{ how: "code" | "browser" }` signs in with ChatGPT (the
//!   person's own Codex too) and `DELETE` gives up on it. Signing in is at home only.
//! - Agents Catbot downloads (12.4.3: Antigravity): `POST /api/engines/:id/install` fetches it
//!   (the list shows how far it's got); `POST /api/engines/:id/sign-in` gives the link to its
//!   sign-in page, `POST /api/engines/:id/sign-in/callback { url }` passes on the address a
//!   browser on another device ended at, and `DELETE /api/engines/:id/sign-in` gives up. At home.
//! - Quick sign-on (stage 29.7): the same two routes for every program Catbot can install and
//!   sign in by itself (`quickInstall`, `quickSignIn` in the list). Install runs the maker's own
//!   install line (`{ thenSignIn: true }` goes straight on to the sign-in); the sign-in runs the
//!   program's login command, opens its page in the browser and shows the code it gives.
//! - OpenRouter's log-in (stage 29.7): `POST /api/engines/openrouter/log-in` opens OpenRouter's
//!   page (OAuth with PKCE); the browser comes back to `GET /api/engines/openrouter/logged-in`,
//!   where Catbot trades the code for a key and keeps it as if pasted. At home only.

use std::sync::Arc;

use axum::{extract::Request, http::{Method, StatusCode}};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail, json_body};
use crate::engines::{self, Engine, Kind};

pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], req: Request, at_home: bool) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, json!({ "engines": list(app) }))),
        // The ChatGPT plan (stage 29.10): who's signed in, Continue with ChatGPT, its models.
        (&Method::GET, ["chatgpt", "account"]) => Ok((StatusCode::OK, app.chatgpt.snapshot())),
        (&Method::POST, ["chatgpt", "sign-in"]) if at_home => {
            let told = app.clone();
            let url = app
                .chatgpt
                .start(move || {
                    if told.chatgpt.signed_in() {
                        super::health::works(&told, engines::CHATGPT);
                    }
                    told.hub.send(json!({ "kind": "engines" }));
                })
                .await
                .map_err(bad)?;
            crate::login::open_in_browser(&url);
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "url": url, "account": app.chatgpt.snapshot() })))
        }
        (&Method::DELETE, ["chatgpt", "sign-in"]) if at_home => {
            app.chatgpt.cancel();
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, app.chatgpt.snapshot()))
        }
        (&Method::POST, ["chatgpt", "sign-out"]) if at_home => {
            let confirmed = app.chatgpt.sign_out().await.map_err(bad)?;
            app.hub.send(json!({ "kind": "engines" }));
            let mut answer = app.chatgpt.snapshot();
            if !confirmed {
                answer["warning"] = json!("Signed out here, but OpenAI didn't confirm it. To end it there too, disconnect catbot in ChatGPT's settings (Sign in with ChatGPT).");
            }
            Ok((StatusCode::OK, answer))
        }
        (&Method::GET, ["chatgpt", "models"]) => match app.chatgpt.models().await {
            Ok(models) if !models.is_empty() => Ok((StatusCode::OK, json!({ "models": models.iter().map(|m| m["id"].clone()).collect::<Vec<_>>(), "labels": models, "live": true }))),
            Ok(_) => Ok((StatusCode::OK, json!({ "models": [], "live": false }))),
            Err(problem) => Ok((StatusCode::OK, json!({ "models": [], "live": false, "problem": problem }))),
        },
        (&Method::GET, ["codex", "account"]) => {
            let state = app.codex_sign_in.state.lock().expect("sign-in lock").clone();
            let Some(program) = engines::find(engines::CODEX).and_then(engines::agent_program) else {
                return Ok((StatusCode::OK, json!({ "installed": false, "account": { "signedIn": false }, "signIn": state })));
            };
            let account = crate::codex::account(&program).await.unwrap_or_else(|problem| json!({ "signedIn": crate::codex::signed_in(), "problem": problem }));
            Ok((StatusCode::OK, json!({ "installed": true, "account": account, "signIn": state })))
        }
        (&Method::POST, ["codex", "sign-in"]) if at_home => {
            let program = engines::find(engines::CODEX).and_then(engines::agent_program).ok_or_else(|| bad("Codex isn't installed on this computer: npm install -g @openai/codex"))?;
            let body: Value = json_body(req).await?;
            let how = if body["how"] == "browser" { "browser" } else { "code" };
            if let Some(old) = app.codex_sign_in.task.lock().expect("sign-in lock").take() {
                old.abort();
            }
            let done = app.clone();
            let (shown, task) = crate::codex::sign_in(&program, how, move |outcome| {
                let mut state = done.codex_sign_in.state.lock().expect("sign-in lock");
                *state = if outcome["success"] == true { Value::Null } else { json!({ "error": outcome["error"].as_str().unwrap_or("the sign-in didn't finish") }) };
                drop(state);
                eprintln!("catbot: Codex sign-in {}", if outcome["success"] == true { "finished" } else { "didn't finish" });
                if outcome["success"] == true {
                    super::health::works(&done, engines::CODEX);
                }
                done.hub.send(json!({ "kind": "engines" }));
            })
            .await
            .map_err(bad)?;
            *app.codex_sign_in.state.lock().expect("sign-in lock") = json!({ "pending": true, "how": shown["how"], "url": shown["url"], "code": shown["code"] });
            *app.codex_sign_in.task.lock().expect("sign-in lock") = Some(task);
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "signIn": app.codex_sign_in.state.lock().expect("sign-in lock").clone() })))
        }
        (&Method::DELETE, ["codex", "sign-in"]) if at_home => {
            if let Some(task) = app.codex_sign_in.task.lock().expect("sign-in lock").take() {
                task.abort();
            }
            *app.codex_sign_in.state.lock().expect("sign-in lock") = Value::Null;
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "signIn": null })))
        }
        (_, ["codex", "sign-in"]) => Err((StatusCode::NOT_FOUND, "Codex signs in at home".into())),
        (&Method::POST, [id, "terminal"]) if at_home => {
            let engine = find(id)?;
            let body: Value = json_body(req).await?;
            super::terminal::terminal(engine, &body)
        }
        (&Method::PUT, [id, "path"]) if at_home => {
            let engine = find(id)?;
            let body: Value = json_body(req).await?;
            super::terminal::set_path(app, engine, &body).await
        }
        (&Method::POST, [id, "install"]) if at_home => {
            let engine = find(id)?;
            if work(app, engine.id)["installing"] == true {
                return Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })));
            }
            let then_sign_in = json_body(req).await.ok().is_some_and(|b: Value| b["thenSignIn"] == true);
            if engine.download.is_none() || engines::asset(engine).is_none() {
                // The maker's own install line, run here (stage 29.7).
                let line = install_line(engine).ok_or_else(|| bad(format!("{} can't be installed from here", engine.name)))?;
                start_install(app, engine, &line, then_sign_in).map_err(bad)?;
                app.hub.send(json!({ "kind": "engines" }));
                return Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })));
            }
            set_work(app, engine.id, json!({ "installing": true, "got": 0 }), None);
            let app2 = app.clone();
            let task = tokio::spawn(async move {
                let progress = |got: u64, total: Option<u64>| {
                    let mut all = app2.agent_work.lock().expect("agent work lock");
                    if let Some((state, _)) = all.get_mut(engine.id) {
                        *state = json!({ "installing": true, "got": got, "total": total });
                    }
                    drop(all);
                    app2.hub.send(json!({ "kind": "engines" }));
                };
                let done = crate::agents::install(engine, progress).await;
                let state = match &done {
                    Ok(_) => Value::Null,
                    Err(problem) => json!({ "problem": problem }),
                };
                eprintln!("catbot: {} {}", engine.name, if done.is_ok() { "installed" } else { "didn't install" });
                app2.agent_work.lock().expect("agent work lock").insert(engine.id.to_string(), (state, None));
                app2.hub.send(json!({ "kind": "engines" }));
                if done.is_ok() && then_sign_in {
                    signed_in_after(&app2, engine).await;
                }
                let _ = then_sign_in;
            });
            set_work(app, engine.id, json!({ "installing": true, "got": 0 }), Some(task));
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (&Method::POST, [id, "sign-in"]) if at_home && engines::find(id).is_some_and(|e| engines::agent_sign_in(e).is_none()) => {
            // The program's own login command (stage 29.7).
            let engine = find(id)?;
            start_cli_sign_in(app, engine).await.map_err(bad)?;
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (&Method::POST, [id, "sign-in"]) if at_home => {
            let engine = find(id)?;
            start_acp_sign_in(app, engine).await.map_err(bad)?;
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (&Method::POST, [id, "sign-in", "callback"]) if at_home => {
            let engine = find(id)?;
            let body: Value = json_body(req).await?;
            let link = work(app, engine.id)["url"].as_str().map(str::to_string).ok_or_else(|| bad("No sign-in is under way."))?;
            crate::acp::deliver_callback(&link, body["url"].as_str().unwrap_or_default()).await.map_err(bad)?;
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (&Method::DELETE, [id, "sign-in"]) if at_home => {
            let engine = find(id)?;
            set_work(app, engine.id, Value::Null, None);
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (_, [_, "install" | "sign-in", ..]) => Err((StatusCode::NOT_FOUND, "that's done at home".into())),
        (&Method::GET, ["codex", "models"]) => {
            let fallback = json!([]);
            let Some(program) = engines::find(engines::CODEX).and_then(engines::agent_program) else {
                return Ok((StatusCode::OK, json!({ "models": fallback, "live": false, "problem": "Codex isn't installed" })));
            };
            let home = app.with_store(|store| Ok(store.home().to_path_buf())).ok_or_else(|| fail("no home"))?;
            match crate::codex::models(&program, &home).await {
                Ok(models) if !models.is_empty() => Ok((StatusCode::OK, json!({ "models": models.iter().map(|m| m["id"].clone()).collect::<Vec<_>>(), "labels": models, "live": true }))),
                Ok(_) => Ok((StatusCode::OK, json!({ "models": fallback, "live": false }))),
                Err(problem) => Ok((StatusCode::OK, json!({ "models": fallback, "live": false, "problem": problem }))),
            }
        }
        (&Method::GET, ["pi", "models"]) => {
            let Some(program) = engines::find(engines::PI).and_then(engines::agent_program) else {
                return Ok((StatusCode::OK, json!({ "models": [], "live": false, "problem": "Pi isn't installed" })));
            };
            match crate::pi::models(&program).await {
                Ok(models) if !models.is_empty() => {
                    let labels: Vec<Value> = models.iter().map(|(id, label, thinks)| json!({ "id": id, "label": label, "thinks": thinks })).collect();
                    Ok((StatusCode::OK, json!({ "models": models.iter().map(|(id, _, _)| id).collect::<Vec<_>>(), "labels": labels, "live": true })))
                }
                Ok(_) => Ok((StatusCode::OK, json!({ "models": [], "live": false, "problem": "Pi has no models with a sign-in yet: run `pi auth login <provider>` on this computer" }))),
                Err(problem) => Ok((StatusCode::OK, json!({ "models": [], "live": false, "problem": problem }))),
            }
        }
        (&Method::GET, [id, "efforts"]) => {
            let engine = find(id)?;
            let model = super::query_param(&req, "model").map(super::urlencoding_decode).unwrap_or_default();
            Ok((StatusCode::OK, efforts(app, engine, &model).await))
        }
        (&Method::GET, [id, "models"]) => {
            let engine = find(id)?;
            let fallback: Vec<&str> = engine.models.iter().map(|(id, _)| *id).filter(|id| !id.is_empty()).collect();
            // An agent program's own models, as it listed them the last time it ran.
            if engine.kind == Kind::Agent
                && let Some(known) = crate::acp::known_models(engine)
            {
                let labels: Vec<Value> = known.options.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect();
                return Ok((StatusCode::OK, json!({ "models": known.options.iter().map(|(id, _)| id).collect::<Vec<_>>(), "labels": labels, "live": true })));
            }
            if engine.kind != Kind::Api {
                return Ok((StatusCode::OK, json!({ "models": fallback, "live": false })));
            }
            let saved = app.with_store(|store| store.engine_key(engine.id)).flatten();
            let Some(access) = engines::access(engine, saved.as_ref()) else {
                return Ok((StatusCode::OK, json!({ "models": fallback, "live": false, "problem": "no key yet" })));
            };
            match crate::api::models(&access).await {
                Ok(models) if !models.is_empty() => Ok((StatusCode::OK, json!({ "models": models, "live": true }))),
                Ok(_) => Ok((StatusCode::OK, json!({ "models": fallback, "live": false }))),
                Err(problem) => Ok((StatusCode::OK, json!({ "models": fallback, "live": false, "problem": problem }))),
            }
        }
        (&Method::POST, ["openrouter", "log-in"]) if at_home => {
            let url = openrouter_log_in(app).map_err(bad)?;
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "url": url, "engine": describe(app, find("openrouter")?) })))
        }
        (&Method::DELETE, ["openrouter", "log-in"]) if at_home => {
            OPENROUTER_ASK.lock().expect("openrouter lock").take();
            set_work(app, "openrouter", Value::Null, None);
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "engine": describe(app, find("openrouter")?) })))
        }
        (&Method::PUT, [id, "key"]) if at_home => {
            let engine = find(id)?;
            if engine.kind != Kind::Api {
                return Err(bad(format!("{} doesn't take a key here", engine.name)));
            }
            let body: Value = json_body(req).await?;
            let key = match body["key"].as_str().map(str::trim).filter(|k| !k.is_empty()) {
                Some(key) => engines::check_key(key).map_err(bad)?,
                None if !engine.needs_key => String::new(),
                None => return Err(bad("Paste the key itself.")),
            };
            let url = match body["url"].as_str().map(str::trim).filter(|u| !u.is_empty()) {
                Some(url) => Some(engines::check_url(url).map_err(bad)?),
                None => None,
            };
            app.try_store(|store| store.set_engine_key(engine.id, &key, url.as_deref())).map_err(fail)?;
            // A new key: whatever was wrong with the old one is for the next message to find.
            super::health::works(app, engine.id);
            eprintln!("catbot: saved the {} settings", engine.name);
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (&Method::DELETE, [id, "key"]) if at_home => {
            let engine = find(id)?;
            app.try_store(|store| store.remove_engine_key(engine.id)).map_err(fail)?;
            app.hub.send(json!({ "kind": "engines" }));
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (_, [_, "key"]) => Err((StatusCode::NOT_FOUND, "keys are managed at home".into())),
        _ => Err((StatusCode::NOT_FOUND, "no such engine route".into())),
    }
}

/// What's under way for an agent (a download, a sign-in), or null.
/// `GET /api/engines/:id/efforts?model=`: the thinking levels this engine takes with that model
/// (empty: the model picks its own), `[{ id, label }]` from least to most, its default if
/// known, and where they come from.
async fn efforts(app: &Arc<App>, engine: &'static Engine, model: &str) -> Value {
    let words = |levels: Vec<String>| levels.into_iter().map(|id| json!({ "label": effort_label(&id), "id": id })).collect::<Vec<_>>();
    match engine.kind {
        Kind::Claude | Kind::Api => {
            let levels = engines::effort_levels(engine, model).unwrap_or_default();
            json!({ "levels": words(levels.into_iter().map(str::to_string).collect()), "source": "catbot" })
        }
        Kind::Codex => {
            let Some(program) = engines::agent_program(engine) else { return json!({ "levels": [], "problem": "Codex isn't installed" }) };
            let Some(home) = app.with_store(|store| Ok(store.home().to_path_buf())) else { return json!({ "levels": [] }) };
            match crate::codex::models(&program, &home).await {
                Ok(models) => {
                    let chosen = models.iter().find(|m| !model.is_empty() && m["id"] == model).or_else(|| models.iter().find(|m| m["isDefault"] == true));
                    let levels: Vec<String> = chosen.and_then(|m| m["efforts"].as_array()).into_iter().flatten().filter_map(|e| e.as_str().map(str::to_string)).collect();
                    json!({ "levels": words(levels), "default": chosen.map(|m| m["defaultEffort"].clone()), "source": "codex" })
                }
                Err(problem) => json!({ "levels": [], "problem": problem }),
            }
        }
        // The plan through Codex: the Responses API's levels.
        Kind::Plan => json!({ "levels": words(["low", "medium", "high"].map(str::to_string).to_vec()), "source": "catbot" }),
        Kind::Pi => {
            let thinks = match engines::agent_program(engine) {
                Some(program) if !model.is_empty() => crate::pi::models(&program).await.ok().and_then(|all| all.into_iter().find(|(id, _, _)| id == model)).map(|(_, _, thinks)| thinks),
                _ => None,
            };
            // Pi fits the level to what the model can do, so its default model gets them all.
            let levels = if thinks == Some(false) { Vec::new() } else { ["off", "minimal", "low", "medium", "high", "xhigh"].map(str::to_string).to_vec() };
            json!({ "levels": words(levels), "source": "pi" })
        }
        Kind::Agent => match crate::acp::known_levels(engine) {
            Some(known) => json!({
                "levels": known.options.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
                "default": known.current,
                "source": "agent",
            }),
            None => json!({ "levels": [], "note": format!("{} hasn't said yet whether it has thinking levels: it says when a bot first uses it.", engine.name) }),
        },
    }
}

/// A level's name in the menu.
pub fn effort_label(id: &str) -> String {
    match id {
        "none" => "None".into(),
        "off" => "Off".into(),
        "on" => "On".into(),
        "minimal" => "Minimal".into(),
        "low" => "Low".into(),
        "medium" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" => "X-High".into(),
        "max" => "Max".into(),
        other => other.to_string(),
    }
}

/// The install line Catbot runs for an engine: `CATBOT_INSTALL_<ID>` when set (the person's own,
/// or a test's stand-in), else the maker's.
fn install_line(engine: &Engine) -> Option<String> {
    let variable = format!("CATBOT_INSTALL_{}", engine.id.to_uppercase().replace('-', "_"));
    std::env::var(variable).ok().filter(|l| !l.trim().is_empty()).or_else(|| engines::quick_install(engine))
}

/// Runs an install line, the list showing it under way and its last line; then, if asked, the sign-in.
fn start_install(app: &Arc<App>, engine: &'static Engine, line: &str, then_sign_in: bool) -> Result<(), String> {
    set_work(app, engine.id, json!({ "installing": true, "step": "Starting the installer" }), None);
    let (shown, ended) = (app.clone(), app.clone());
    let task = crate::login::install(
        line,
        move |said| {
            let said: String = said.trim().chars().take(120).collect();
            if said.is_empty() {
                return;
            }
            if let Some((state, _)) = shown.agent_work.lock().expect("agent work lock").get_mut(engine.id) {
                *state = json!({ "installing": true, "step": said });
            }
            shown.hub.send(json!({ "kind": "engines" }));
        },
        move |done| {
            eprintln!("catbot: {} {}", engine.name, if done.is_ok() { "installed" } else { "didn't install" });
            // The program it put somewhere new is looked for again.
            let found = done.is_ok() && program_of(&ended, engine).is_some();
            let state = match done {
                Ok(()) if found => Value::Null,
                Ok(()) => json!({ "problem": format!("The installer finished, but Catbot can't find {}. Point Catbot to its program below.", engine.name) }),
                Err(problem) => json!({ "problem": format!("{} didn't install: {problem}", engine.name) }),
            };
            ended.agent_work.lock().expect("agent work lock").insert(engine.id.to_string(), (state, None));
            ended.hub.send(json!({ "kind": "engines" }));
            if found && then_sign_in {
                let app = ended.clone();
                tokio::spawn(async move { signed_in_after(&app, engine).await });
            }
        },
    )?;
    if let Some((_, slot)) = app.agent_work.lock().expect("agent work lock").get_mut(engine.id) {
        *slot = Some(task);
    }
    Ok(())
}

/// Signs an agent in through ACP's `authenticate` (Antigravity, Gemini CLI, Muse Code): the page
/// it gives, or none when it was signed in already.
async fn start_acp_sign_in(app: &Arc<App>, engine: &'static Engine) -> Result<(), String> {
    let method = engines::agent_sign_in(engine).ok_or_else(|| format!("{} signs in on its own: {}", engine.name, engine.sign_in))?;
    let program = engines::agent_program(engine).ok_or_else(|| format!("Install {} first.", engine.name))?;
    let env = engines::agent_env(engine, &program);
    let folder = engines::agents_dir().map(|d| d.join(engine.id)).ok_or("no folder")?;
    let done = app.clone();
    let (shown, task) = crate::acp::sign_in(engine, program, env, folder, method, move |outcome| {
        let state = match outcome {
            Ok(()) => {
                engines::note_signed_in(engine);
                // Signed in: what it stopped goes again (stage 32.6).
                super::health::works(&done, engine.id);
                Value::Null
            }
            Err(problem) => json!({ "problem": format!("The sign-in didn't finish: {problem}") }),
        };
        done.agent_work.lock().expect("agent work lock").insert(engine.id.to_string(), (state, None));
        done.hub.send(json!({ "kind": "engines" }));
    })
    .await?;
    match shown {
        Some((url, code)) => {
            // Opened for the person, as the program's own login does (stage 29.7).
            crate::login::open_in_browser(&url);
            set_work(app, engine.id, json!({ "signingIn": true, "url": url, "code": code }), Some(task))
        }
        None => {
            engines::note_signed_in(engine);
            super::health::works(app, engine.id);
            set_work(app, engine.id, Value::Null, None)
        }
    }
    Ok(())
}

/// Goes on to the sign-in after an install, when the program has one Catbot can run.
async fn signed_in_after(app: &Arc<App>, engine: &'static Engine) {
    let started = if engines::cli_sign_in(engine).is_some() {
        start_cli_sign_in(app, engine).await
    } else if engines::agent_sign_in(engine).is_some() {
        start_acp_sign_in(app, engine).await
    } else {
        Ok(())
    };
    if let Err(problem) = started {
        set_work(app, engine.id, json!({ "problem": problem }), None);
    }
    app.hub.send(json!({ "kind": "engines" }));
}

/// The program to run an engine's login with: Claude Code's command, or the agent's program.
fn program_of(app: &App, engine: &Engine) -> Option<(std::path::PathBuf, Vec<String>)> {
    if engine.kind == Kind::Claude {
        let (first, rest) = app.cli.split_first()?;
        let program = engines::find_program(first)?;
        return Some((program, rest.to_vec()));
    }
    engines::agent_program(engine).map(|p| (p, Vec::new()))
}

/// Starts the program's own login: its page is opened in the browser, and the list shows the
/// page and any code until it says it's done.
async fn start_cli_sign_in(app: &Arc<App>, engine: &'static Engine) -> Result<(), String> {
    let login = engines::cli_sign_in(engine).ok_or_else(|| format!("{} signs in on its own: {}", engine.name, engine.sign_in))?;
    let (program, mut args) = program_of(app, engine).ok_or_else(|| format!("Install {} first.", engine.name))?;
    args.extend(login.iter().map(|a| a.to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let env = if engine.kind == Kind::Agent { engines::agent_env(engine, &program) } else { Vec::new() };
    let done = app.clone();
    // Set, under the work lock, once it has ended: "under way" is never shown after that.
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ending = ended.clone();
    let (shown, task) = crate::login::sign_in(&program, &args, env, !engines::opens_its_own_page(engine), move |outcome| {
        let state = match outcome {
            Ok(()) => {
                engines::note_signed_in(engine);
                // Signed in: what it stopped goes again (stage 32.6).
                super::health::works(&done, engine.id);
                Value::Null
            }
            Err(problem) => json!({ "problem": format!("The sign-in didn't finish: {problem}. Try “Open sign-in in Terminal”.") }),
        };
        let mut all = done.agent_work.lock().expect("agent work lock");
        ending.store(true, std::sync::atomic::Ordering::SeqCst);
        all.insert(engine.id.to_string(), (state, None));
        drop(all);
        done.hub.send(json!({ "kind": "engines" }));
    })
    .await?;
    let mut all = app.agent_work.lock().expect("agent work lock");
    if !ended.load(std::sync::atomic::Ordering::SeqCst) {
        let state = json!({ "signingIn": true, "url": shown.url, "code": shown.code });
        if let Some((_, Some(old))) = all.insert(engine.id.to_string(), (state, Some(task))) {
            old.abort();
        }
    }
    Ok(())
}

/// OpenRouter's log-in under way: what it was asked with, to check what comes back.
struct OpenRouterAsk {
    state: String,
    verifier: String,
}

static OPENROUTER_ASK: std::sync::Mutex<Option<OpenRouterAsk>> = std::sync::Mutex::new(None);

/// OpenRouter's address (`CATBOT_OPENROUTER_AUTH` for a test's stand-in).
fn openrouter_base() -> String {
    std::env::var("CATBOT_OPENROUTER_AUTH").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| "https://openrouter.ai".into())
}

/// Starts OpenRouter's log-in: its page, opened in the browser, asking for a key for Catbot.
fn openrouter_log_in(app: &Arc<App>) -> Result<String, String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let port = app.local.get().map(|a| a.port()).ok_or("Catbot isn't listening yet")?;
    let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let state = uuid::Uuid::new_v4().simple().to_string();
    let back = format!("http://localhost:{port}/api/engines/openrouter/logged-in");
    let mut page = reqwest::Url::parse(&format!("{}/auth", openrouter_base())).map_err(|e| e.to_string())?;
    page.query_pairs_mut()
        .append_pair("callback_url", &back)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("key_label", "Catbot");
    *OPENROUTER_ASK.lock().expect("openrouter lock") = Some(OpenRouterAsk { state, verifier });
    let page = page.to_string();
    crate::login::open_in_browser(&page);
    set_work(app, "openrouter", json!({ "signingIn": true, "url": page }), None);
    Ok(page)
}

/// The browser back from OpenRouter: the code is traded for a key, kept like a pasted one, and
/// the tab says so.
pub async fn openrouter_back(app: &Arc<App>, query: &std::collections::HashMap<String, String>) -> axum::response::Response {
    use axum::response::{Html, IntoResponse};
    let page = |status: StatusCode, said: &str| {
        let html = format!(
            "<!doctype html><meta charset=utf-8><title>Catbot</title><body style=\"font:16px system-ui;margin:4em auto;max-width:32em;text-align:center\"><h1>Catbot</h1><p>{said}</p></body>"
        );
        (status, Html(html)).into_response()
    };
    let Some(code) = query.get("code").filter(|c| !c.is_empty()) else {
        return page(StatusCode::BAD_REQUEST, "No OpenRouter log-in code was given. Start it again from Catbot.");
    };
    let asked = {
        let mut guard = OPENROUTER_ASK.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(pending) if query.get("state").map(String::as_str) == Some(pending.state.as_str()) => guard.take(),
            Some(_) => {
                set_work(app, "openrouter", json!({ "problem": "The log-in came back from somewhere else, so Catbot ignored it. Try again." }), None);
                app.hub.send(json!({ "kind": "engines" }));
                return page(StatusCode::BAD_REQUEST, "This doesn't match the log-in Catbot started. Try again from Catbot.");
            }
            None => None,
        }
    };
    let Some(asked) = asked else {
        return page(StatusCode::BAD_REQUEST, "No OpenRouter log-in is under way. Start it again from Catbot.");
    };
    let traded = async {
        let answer = crate::http::client()
            .post(format!("{}/api/v1/auth/keys", openrouter_base()))
            .json(&json!({ "code": code, "code_verifier": asked.verifier, "code_challenge_method": "S256" }))
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("couldn't reach OpenRouter: {e}"))?;
        let status = answer.status();
        let body: Value = answer.json().await.unwrap_or(Value::Null);
        match body["key"].as_str() {
            Some(key) if status.is_success() => engines::check_key(key),
            _ => Err(format!("OpenRouter said no ({status})")),
        }
    };
    match traded.await {
        Ok(key) => {
            let saved = app.with_store(|store| store.set_engine_key("openrouter", &key, None));
            if saved.is_none() {
                set_work(app, "openrouter", json!({ "problem": "Catbot couldn't keep the key." }), None);
                app.hub.send(json!({ "kind": "engines" }));
                return page(StatusCode::INTERNAL_SERVER_ERROR, "Catbot got a key but couldn't keep it.");
            }
            super::health::works(app, "openrouter");
            set_work(app, "openrouter", Value::Null, None);
            eprintln!("catbot: logged in to OpenRouter");
            app.hub.send(json!({ "kind": "engines" }));
            page(StatusCode::OK, "Catbot is connected to OpenRouter. You can close this tab.")
        }
        Err(problem) => {
            set_work(app, "openrouter", json!({ "problem": format!("The OpenRouter log-in didn't finish: {problem}") }), None);
            app.hub.send(json!({ "kind": "engines" }));
            page(StatusCode::BAD_GATEWAY, "The log-in didn't finish. Go back to Catbot and try again.")
        }
    }
}

fn work(app: &App, id: &str) -> Value {
    app.agent_work.lock().expect("agent work lock").get(id).map(|(state, _)| state.clone()).unwrap_or(Value::Null)
}

/// Says what's under way, stopping whatever was before.
fn set_work(app: &App, id: &str, state: Value, task: Option<tokio::task::JoinHandle<()>>) {
    let mut all = app.agent_work.lock().expect("agent work lock");
    if let Some((_, Some(old))) = all.insert(id.to_string(), (state, task)) {
        old.abort();
    }
}

fn find(id: &str) -> Result<&'static Engine, (StatusCode, String)> {
    engines::find(id).ok_or_else(|| (StatusCode::NOT_FOUND, format!("no engine called {id}")))
}

fn list(app: &App) -> Vec<Value> {
    engines::ALL.iter().filter(|engine| engines::works_here(engine)).map(|engine| describe(app, engine)).collect()
}

/// An engine for the interface: everything but the key.
fn describe(app: &App, engine: &Engine) -> Value {
    let saved = app.with_store(|store| store.engine_key(engine.id)).flatten();
    let access = engines::access(engine, saved.as_ref());
    let installed = match engine.kind {
        Kind::Agent | Kind::Codex | Kind::Pi | Kind::Plan => Some(engines::agent_program(engine).is_some()),
        // Claude Code (stage 29.3): told apart, so a new computer's first run can say what's missing.
        Kind::Claude => Some(crate::claude::installed(&app.cli)),
        Kind::Api => None,
    };
    let signed_in = match engine.kind {
        Kind::Codex => Some(crate::codex::signed_in()),
        Kind::Plan => Some(app.chatgpt.signed_in()),
        Kind::Pi => crate::pi::signed_in(),
        Kind::Claude => engines::claude_signed_in(),
        _ => engines::agent_signed_in(engine),
    };
    let ready = match engine.kind {
        Kind::Claude => installed == Some(true) && signed_in != Some(false),
        // One on this computer once the person added it (stage 24), or a bot uses it.
        Kind::Api if !engine.needs_key => super::local_ai::is_added(app, engine.id),
        Kind::Api => access.is_some(),
        Kind::Agent => installed == Some(true) && signed_in != Some(false),
        Kind::Codex | Kind::Plan => installed == Some(true) && signed_in == Some(true),
        Kind::Pi => installed == Some(true),
    };
    json!({
        "id": engine.id,
        "name": engine.name,
        "kind": engine.kind,
        "needsKey": engine.needs_key,
        "ready": ready,
        "keyFrom": access.as_ref().and_then(|a| a.key_from),
        "keyHint": saved.as_ref().filter(|s| !s.key.is_empty()).map(|s| engines::key_hint(&s.key)),
        "url": access.as_ref().map(|a| a.url.clone()).unwrap_or_else(|| engine.url.to_string()),
        "keyPage": engine.key_page,
        "models": engine.models.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
        "defaultModel": engine.default_model,
        "installed": installed,
        "signedIn": signed_in,
        "download": engine.download.filter(|_| engines::asset(engine).is_some()).map(|d| json!({ "version": d.version, "size": d.size })),
        "signsInHere": engines::agent_sign_in(engine).is_some() || engines::cli_sign_in(engine).is_some(),
        // The ChatGPT plan's own sign-in, as the interface may see it (stage 29.10).
        "account": (engine.kind == Kind::Plan).then(|| app.chatgpt.snapshot()),
        // Catbot can install it (the maker's line) and sign it in itself (stage 29.7).
        "quickInstall": engine.download.is_none() && install_line(engine).is_some(),
        "quickSignIn": engines::agent_sign_in(engine).is_some() || engines::cli_sign_in(engine).is_some() || engine.id == "openrouter",
        "signInWith": engines::sign_in_with(engine),
        "work": work(app, engine.id),
        "install": engine.install,
        "signIn": engine.sign_in,
        // Where its program is (stage 19.7.2): in use, found by name, and chosen by the person.
        "program": matches!(engine.kind, Kind::Agent | Kind::Codex | Kind::Pi | Kind::Plan).then(|| engines::agent_program(engine)).flatten(),
        "foundProgram": matches!(engine.kind, Kind::Agent | Kind::Codex | Kind::Pi | Kind::Plan).then(|| engines::found_program(engine)).flatten(),
        "chosenProgram": engines::chosen_path(engine.id),
        "terminalInstall": engines::terminal_command(engine.install).filter(|c| engines::runs_here(c)),
        "terminalSignIn": engines::terminal_command(engine.sign_in).filter(|c| engines::runs_here(c)),
        // What went wrong when it last failed, and its fix (stage 27).
        "trouble": super::health::of(app, engine.id),
    })
}
