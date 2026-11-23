//! Desktop pets: which bots come out onto the desktop as walking cats when the Linux or Windows
//! app's window closes ([[Catbot Desktop Pet]] in the Codex). Off until a bot is ticked as a pet.
//!
//! Kept in Catbot's settings: `pets` (the pets' bot ids, in the order they were ticked) and
//! `petTutorialSeen` (the little tutorial window has been shown).
//!
//! `GET /api/pets` gives the pets (for the app's pet windows) and whether the tutorial was seen;
//! `PUT /api/pets {tutorialSeen}` marks it. A bot is made a pet by `PATCH /api/bots/:id {pet}`.

use std::sync::Arc;

use hyper::StatusCode;
use serde_json::{Value, json};

use super::{Answer, App, fail};
use crate::store::Store;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const KEY: &str = "pets";
const SEEN: &str = "petTutorialSeen";

/// The pets' bot ids, without any bot since deleted.
pub fn ids(store: &Store) -> Vec<String> {
    let kept: Vec<String> = store.setting(KEY).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let bots: Vec<String> = store.bots().unwrap_or_default().into_iter().map(|b| b.id).collect();
    kept.into_iter().filter(|id| bots.contains(id)).collect()
}

pub fn is_pet(store: &Store, bot_id: &str) -> bool {
    ids(store).iter().any(|id| id == bot_id)
}

/// Makes a bot a pet, or not.
pub fn set(store: &Store, bot_id: &str, on: bool) -> Result<(), BoxError> {
    let mut list = ids(store);
    list.retain(|id| id != bot_id);
    if on {
        list.push(bot_id.to_string());
    }
    store.set_setting(KEY, &json!(list))
}

pub fn route(app: &Arc<App>, body: Option<Value>) -> Answer {
    if let Some(seen) = body.as_ref().and_then(|b| b["tutorialSeen"].as_bool()) {
        app.try_store(|store| store.set_setting(SEEN, &json!(seen))).map_err(fail)?;
    }
    let shown = app.try_store(|store| {
        let pets: Vec<Value> = ids(store)
            .into_iter()
            .filter_map(|id| store.bot_by_id(&id).ok().flatten())
            .map(|bot| json!({ "botId": bot.id, "name": bot.name, "color": super::wire::color_of(&bot), "mark": bot.mark.as_str() }))
            .collect();
        let seen = store.setting(SEEN)?.and_then(|v| v.as_bool()).unwrap_or(false);
        Ok(json!({ "pets": pets, "tutorialSeen": seen }))
    });
    Ok((StatusCode::OK, shown.map_err(fail)?))
}
