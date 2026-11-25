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

use std::sync::Arc;

use axum::{extract::Request, http::{Method, StatusCode}};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail, json_body};
use crate::engines::{self, Engine, Kind};

pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], req: Request, at_home: bool) -> Answer {
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, json!({ "engines": list(app) }))),
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
            if engine.download.is_none() || engines::asset(engine).is_none() {
                return Err(bad(format!("{} can't be installed from here", engine.name)));
            }
            if work(app, engine.id)["installing"] == true {
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
            });
            set_work(app, engine.id, json!({ "installing": true, "got": 0 }), Some(task));
            Ok((StatusCode::OK, json!({ "engine": describe(app, engine) })))
        }
        (&Method::POST, [id, "sign-in"]) if at_home => {
            let engine = find(id)?;
            let method = engines::agent_sign_in(engine).ok_or_else(|| bad(format!("{} signs in on its own: {}", engine.name, engine.sign_in)))?;
            let program = engines::agent_program(engine).ok_or_else(|| bad(format!("Install {} first.", engine.name)))?;
            let env = engines::agent_env(engine, &program);
            let folder = engines::agents_dir().map(|d| d.join(engine.id)).ok_or_else(|| fail("no folder"))?;
            let done = app.clone();
            let (shown, task) = crate::acp::sign_in(engine, program, env, folder, method, move |outcome| {
                let state = match outcome {
                    Ok(()) => {
                        engines::note_signed_in(engine);
                        Value::Null
                    }
                    Err(problem) => json!({ "problem": format!("The sign-in didn't finish: {problem}") }),
                };
                done.agent_work.lock().expect("agent work lock").insert(engine.id.to_string(), (state, None));
                done.hub.send(json!({ "kind": "engines" }));
            })
            .await
            .map_err(bad)?;
            match shown {
                Some((url, code)) => set_work(app, engine.id, json!({ "signingIn": true, "url": url, "code": code }), Some(task)),
                None => {
                    engines::note_signed_in(engine);
                    set_work(app, engine.id, Value::Null, None)
                }
            }
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
    let installed = matches!(engine.kind, Kind::Agent | Kind::Codex | Kind::Pi).then(|| engines::agent_program(engine).is_some());
    let signed_in = match engine.kind {
        Kind::Codex => Some(crate::codex::signed_in()),
        Kind::Pi => crate::pi::signed_in(),
        _ => engines::agent_signed_in(engine),
    };
    let ready = match engine.kind {
        Kind::Claude => true,
        // One on this computer once the person added it (stage 24), or a bot uses it.
        Kind::Api if !engine.needs_key => super::local_ai::is_added(app, engine.id),
        Kind::Api => access.is_some(),
        Kind::Agent => installed == Some(true) && signed_in != Some(false),
        Kind::Codex => installed == Some(true) && signed_in == Some(true),
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
        "signsInHere": engines::agent_sign_in(engine).is_some(),
        "signInWith": engines::sign_in_with(engine),
        "work": work(app, engine.id),
        "install": engine.install,
        "signIn": engine.sign_in,
        // Where its program is (stage 19.7.2): in use, found by name, and chosen by the person.
        "program": matches!(engine.kind, Kind::Agent | Kind::Codex | Kind::Pi).then(|| engines::agent_program(engine)).flatten(),
        "foundProgram": matches!(engine.kind, Kind::Agent | Kind::Codex | Kind::Pi).then(|| engines::found_program(engine)).flatten(),
        "chosenProgram": engines::chosen_path(engine.id),
        "terminalInstall": engines::terminal_command(engine.install).filter(|c| engines::runs_here(c)),
        "terminalSignIn": engines::terminal_command(engine.sign_in).filter(|c| engines::runs_here(c)),
        // What went wrong when it last failed, and its fix (stage 27).
        "trouble": super::health::of(app, engine.id),
    })
}
