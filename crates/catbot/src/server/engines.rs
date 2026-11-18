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

fn find(id: &str) -> Result<&'static Engine, (StatusCode, String)> {
    engines::find(id).ok_or_else(|| (StatusCode::NOT_FOUND, format!("no engine called {id}")))
}

fn list(app: &App) -> Vec<Value> {
    engines::ALL.iter().map(|engine| describe(app, engine)).collect()
}

/// An engine for the interface: everything but the key.
fn describe(app: &App, engine: &Engine) -> Value {
    let saved = app.with_store(|store| store.engine_key(engine.id)).flatten();
    let access = engines::access(engine, saved.as_ref());
    let installed = matches!(engine.kind, Kind::Agent | Kind::Codex).then(|| engines::agent_program(engine).is_some());
    let signed_in = (engine.kind == Kind::Codex).then(crate::codex::signed_in);
    let ready = match engine.kind {
        Kind::Claude => true,
        Kind::Api => access.is_some(),
        Kind::Agent => installed == Some(true),
        Kind::Codex => installed == Some(true) && signed_in == Some(true),
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
        "install": engine.install,
        "signIn": engine.sign_in,
    })
}
