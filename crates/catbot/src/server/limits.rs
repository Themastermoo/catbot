//! Limits: the spend limits Moo sets, and Catbot's own token limits for AIs whose plans or keys
//! don't report any (everything but Claude on a plan, and local AIs, which cost nothing).
//!
//! - A **spend limit** for the month, overall and per bot, in dollars. It counts what the turns
//!   on API keys cost: what the API reported, or the price per million tokens Moo entered for
//!   that AI (cache re-reads at a tenth of the input price, as the big providers roughly charge).
//! - **Token limits** per AI, in new tokens: a session of five hours from its first turn (like
//!   Claude's) and the week from Monday. Catbot sets them at first; they can be changed or
//!   turned off.
//!
//! A bot whose AI is over a limit doesn't take the turn (or, with a backup AI, hands it on).
//! `GET /api/limits` (the limits and where each stands) and `PUT /api/limits` (home only).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::engines;
use crate::store::{Bot, UsageRow};

/// Catbot's own session length, as Claude's.
const SESSION_HOURS: i64 = 5;
/// The token limits Catbot starts an AI on.
pub const DEFAULT_SESSION_TOKENS: u64 = 1_000_000;
pub const DEFAULT_WEEK_TOKENS: u64 = 10_000_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Limits {
    /// The most the API keys may cost in a calendar month, all bots together.
    pub monthly_usd: Option<f64>,
    /// The same, per bot.
    pub per_bot_usd: BTreeMap<String, f64>,
    /// Per AI: token limits and prices. An AI not listed has Catbot's starting limits.
    pub engines: BTreeMap<String, EngineLimit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EngineLimit {
    /// New tokens in a five-hour session; None for no limit.
    pub session: Option<u64>,
    /// New tokens in a week (from Monday); None for no limit.
    pub week: Option<u64>,
    /// Dollars per million input and output tokens, for an API that doesn't report its cost.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
}

impl Default for EngineLimit {
    fn default() -> EngineLimit {
        EngineLimit { session: Some(DEFAULT_SESSION_TOKENS), week: Some(DEFAULT_WEEK_TOKENS), price_in: None, price_out: None }
    }
}

impl Limits {
    pub fn engine(&self, id: &str) -> EngineLimit {
        self.engines.get(id).cloned().unwrap_or_default()
    }
}

pub fn load(app: &App) -> Limits {
    app.with_store(|store| store.setting("limits"))
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Whether Catbot keeps token limits for an AI: those that don't report a plan's own.
pub fn limited(engine: &str) -> bool {
    let billing = super::usage::billing(engine);
    !(billing == "free" || (engine == engines::CLAUDE && billing == "plan"))
}

/// What a turn cost, if the API didn't say: from the price Moo entered for its AI.
pub fn priced(limits: &Limits, engine: &str, row: &UsageRow) -> Option<f64> {
    let limit = limits.engine(engine);
    let (price_in, price_out) = (limit.price_in?, limit.price_out.unwrap_or(0.0));
    let input = (row.input + row.cache_write) as f64 + row.cache_read as f64 / 10.0;
    Some((input * price_in + row.output as f64 * price_out) / 1_000_000.0)
}

/// A window of a limit: how much of it is used, and when it resets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub used: u64,
    pub limit: Option<u64>,
    pub resets: Option<DateTime<Utc>>,
}

impl Window {
    pub fn full(&self) -> bool {
        self.limit.is_some_and(|limit| self.used >= limit)
    }

    fn wire(&self) -> Value {
        json!({
            "used": self.used,
            "limit": self.limit,
            "usedPercent": self.limit.map(|l| if l == 0 { 100.0 } else { (self.used as f64 / l as f64 * 100.0).min(100.0) }),
            "resetsAt": self.resets.map(|r| r.to_rfc3339()),
        })
    }
}

fn new_tokens(r: &UsageRow) -> u64 {
    r.input + r.output + r.cache_write
}

fn at(r: &UsageRow) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&r.at).ok().map(|t| t.with_timezone(&Utc))
}

/// An AI's session: five hours from the first turn after the last session ended.
pub fn session(rows: &[UsageRow], engine: &str, limit: Option<u64>, now: DateTime<Utc>) -> Window {
    let mut start: Option<DateTime<Utc>> = None;
    for row in rows.iter().filter(|r| r.engine == engine) {
        let Some(t) = at(row) else { continue };
        if start.is_none_or(|s| t >= s + Duration::hours(SESSION_HOURS)) {
            start = Some(t);
        }
    }
    match start.filter(|s| now < *s + Duration::hours(SESSION_HOURS)) {
        Some(s) => {
            let used = rows.iter().filter(|r| r.engine == engine && at(r).is_some_and(|t| t >= s)).map(new_tokens).sum();
            Window { used, limit, resets: Some(s + Duration::hours(SESSION_HOURS)) }
        }
        None => Window { used: 0, limit, resets: None },
    }
}

/// The start of this week (Monday, midnight, here) and of the next.
pub fn week_bounds(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let local = now.with_timezone(&chrono::Local);
    let monday = local.date_naive() - Duration::days(local.weekday().num_days_from_monday() as i64);
    let start = chrono::Local
        .from_local_datetime(&monday.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(now);
    (start, start + Duration::days(7))
}

pub fn week(rows: &[UsageRow], engine: &str, limit: Option<u64>, now: DateTime<Utc>) -> Window {
    let (start, end) = week_bounds(now);
    let used = rows.iter().filter(|r| r.engine == engine && at(r).is_some_and(|t| t >= start)).map(new_tokens).sum();
    Window { used, limit, resets: Some(end) }
}

/// The start of this calendar month, here.
fn month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    super::usage::period("this-month", now).map(|(from, _)| from).unwrap_or(now)
}

/// The usage that counts towards any limit now: since the earlier of the month's and week's
/// starts, and a day back (for sessions).
fn recent(app: &App, now: DateTime<Utc>) -> Vec<UsageRow> {
    let since = [month_start(now), week_bounds(now).0, now - Duration::days(1)].into_iter().min().unwrap_or(now);
    app.with_store(|store| store.usage_since(&since.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))).unwrap_or_default()
}

/// Why a bot can't use an AI now (with when that changes), or None if it can.
pub fn blocked(app: &App, bot: &Bot, engine: &str) -> Option<(String, Option<DateTime<Utc>>)> {
    let now = Utc::now();
    let limits = load(app);
    let rows = recent(app, now);
    let name = engines::find(engine).map(|e| e.name).unwrap_or(engine);
    if super::usage::billing(engine) == "key" {
        let since = month_start(now);
        let month: Vec<&UsageRow> = rows.iter().filter(|r| at(r).is_some_and(|t| t >= since)).collect();
        // The first of next month: the start of the month 32 days after this one began.
        let next_month = super::usage::period("this-month", since + Duration::days(32)).map(|(f, _)| f).unwrap_or(now);
        if let Some(cap) = limits.monthly_usd {
            let spent: f64 = month.iter().filter_map(|r| r.cost_usd).sum();
            if spent >= cap {
                return Some((format!("This month's spend limit (${cap:.2}) is used up, so AIs paid by use are paused"), Some(next_month)));
            }
        }
        if let Some(cap) = limits.per_bot_usd.get(&bot.id) {
            let spent: f64 = month.iter().filter(|r| r.bot_id == bot.id).filter_map(|r| r.cost_usd).sum();
            if spent >= *cap {
                return Some((format!("{}'s spend limit for the month (${cap:.2}) is used up", bot.name), Some(next_month)));
            }
        }
    }
    if limited(engine) {
        let limit = limits.engine(engine);
        let s = session(&rows, engine, limit.session, now);
        if s.full() {
            return Some((format!("{name} is at Catbot's session limit"), s.resets));
        }
        let w = week(&rows, engine, limit.week, now);
        if w.full() {
            return Some((format!("{name} is at Catbot's weekly limit"), w.resets));
        }
    }
    if let Some(until) = app.limited_until.lock().expect("limited lock").get(engine).copied()
        && until > now
    {
        return Some((format!("{name} turned the last turn away at its usage limit"), Some(until)));
    }
    // A plan that says it's full (Claude's, or Codex's on ChatGPT).
    if (engine == engines::CLAUDE || engine == engines::CODEX)
        && let Some(plan) = app.plan_usage.lock().expect("plan usage lock").as_ref().map(|(_, v)| v.clone())
        && let Some(row) = plan["plans"].as_array().into_iter().flatten().find(|p| p["engine"] == engine)
    {
        let who = if engine == engines::CLAUDE { "Claude" } else { "Codex" };
        for window in row["windows"].as_array().into_iter().flatten() {
            if window["usedPercent"].as_f64().is_some_and(|u| u >= 100.0) {
                let resets = window["resetsAt"].as_str().and_then(|r| DateTime::parse_from_rfc3339(r).ok()).map(|t| t.with_timezone(&Utc));
                if resets.is_none_or(|r| r > now) {
                    return Some((format!("{who}'s plan is at its limit ({})", window["label"].as_str().unwrap_or("usage").to_lowercase()), resets));
                }
            }
        }
    }
    None
}

pub async fn route(app: &Arc<App>, method: &axum::http::Method, body: Option<Value>, home: bool) -> Answer {
    if method == axum::http::Method::PUT {
        if !home {
            return Err((StatusCode::FORBIDDEN, "limits are set at home".into()));
        }
        let limits: Limits = serde_json::from_value(body.unwrap_or(Value::Null)).map_err(|e| bad(format!("those limits don't read: {e}")))?;
        if limits.monthly_usd.is_some_and(|c| c < 0.0) || limits.per_bot_usd.values().any(|c| *c < 0.0) {
            return Err(bad("a limit can't be below zero"));
        }
        app.try_store(|store| store.set_setting("limits", &serde_json::to_value(&limits)?)).map_err(fail)?;
        app.hub.send(json!({ "kind": "limits" }));
    }
    Ok((StatusCode::OK, status(app)))
}

/// The limits, and where each stands now.
pub fn status(app: &App) -> Value {
    let now = Utc::now();
    let limits = load(app);
    let rows = recent(app, now);
    let bots = app.with_store(|store| store.bots()).unwrap_or_default();
    let since = month_start(now);
    let month: Vec<&UsageRow> = rows.iter().filter(|r| at(r).is_some_and(|t| t >= since)).collect();
    let spent: f64 = month.iter().filter_map(|r| r.cost_usd).sum();
    // The AIs Catbot limits: those a bot runs on, and any used this week.
    let mut ids: Vec<String> = bots.iter().map(|b| b.engine.clone()).chain(rows.iter().map(|r| r.engine.clone())).collect();
    ids.sort();
    ids.dedup();
    let engines: Vec<Value> = ids
        .iter()
        .filter(|id| limited(id))
        .map(|id| {
            let limit = limits.engine(id);
            json!({
                "engine": id,
                "name": engines::find(id).map(|e| e.name).unwrap_or(id.as_str()),
                "billing": super::usage::billing(id),
                "session": session(&rows, id, limit.session, now).wire(),
                "week": week(&rows, id, limit.week, now).wire(),
                "priceIn": limit.price_in,
                "priceOut": limit.price_out,
            })
        })
        .collect();
    let per_bot: Vec<Value> = bots
        .iter()
        .map(|b| {
            let used: f64 = month.iter().filter(|r| r.bot_id == b.id).filter_map(|r| r.cost_usd).sum();
            json!({ "botId": b.id, "name": b.name, "usedUsd": round(used), "capUsd": limits.per_bot_usd.get(&b.id) })
        })
        .collect();
    json!({
        "limits": limits,
        "spend": { "usedUsd": round(spent), "capUsd": limits.monthly_usd, "resetsAt": super::usage::period("this-month", since + Duration::days(32)).map(|(f, _)| f.to_rfc3339()), "perBot": per_bot },
        "engines": engines,
        "defaults": { "session": DEFAULT_SESSION_TOKENS, "week": DEFAULT_WEEK_TOKENS },
    })
}

fn round(x: f64) -> f64 {
    // (+ 0.0 turns the -0.0 an empty sum gives into 0.)
    (x * 10_000.0).round() / 10_000.0 + 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(engine: &str, at: DateTime<Utc>, tokens: u64) -> UsageRow {
        UsageRow { engine: engine.into(), at: at.to_rfc3339(), input: tokens, ..UsageRow::default() }
    }

    #[test]
    fn sessions_start_at_the_first_turn_and_last_five_hours() {
        let now = Utc::now();
        let rows = vec![
            row("openai", now - Duration::hours(9), 500),
            row("openai", now - Duration::hours(3), 300),
            row("openai", now - Duration::hours(1), 200),
            row("xai", now - Duration::hours(1), 999),
        ];
        let s = session(&rows, "openai", Some(400), now);
        assert_eq!(s.used, 500);
        assert!(s.full());
        assert_eq!(s.resets, Some(at(&rows[1]).unwrap() + Duration::hours(5)));
        let none = session(&rows[..1], "openai", Some(400), now);
        assert_eq!((none.used, none.resets), (0, None));
        let (start, end) = week_bounds(now);
        assert!(start <= now && now < end && end - start == Duration::days(7));
    }

    #[test]
    fn a_price_gives_a_cost() {
        let mut limits = Limits::default();
        limits.engines.insert("openai".into(), EngineLimit { price_in: Some(2.0), price_out: Some(8.0), ..EngineLimit::default() });
        let r = UsageRow { input: 1_000_000, output: 500_000, cache_read: 1_000_000, ..UsageRow::default() };
        assert_eq!(priced(&limits, "openai", &r), Some(2.0 + 4.0 + 0.2));
        assert_eq!(priced(&limits, "xai", &r), None);
        assert!(limited("openai") && !limited("ollama"));
    }
}
