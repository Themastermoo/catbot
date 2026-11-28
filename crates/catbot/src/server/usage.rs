//! Usage: what each turn used, the breakdown of it, and the plan limits of the AIs that have
//! them (Claude's five-hour and weekly windows, and Codex's on a ChatGPT plan, 12.4.2).
//!
//! - `GET /api/usage?period=this-month|last-month|30d&by=bot|model|engine|day`: the breakdown.
//! - `GET /api/usage/plans`: each plan's windows, as Claude's own usage page shows them. Read the
//!   way MausBot's `server/plan-usage.ts` does: Claude Code's sign-in on this computer
//!   (`~/.claude/.credentials.json`), sent only to Anthropic's usage address.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};
use crate::claude::Tokens;
use crate::engines;
use crate::store::{Bot, UsageRow};

/// Records what a turn used.
pub fn record(app: &App, bot: &Bot, thread: &str, engine: &str, model: &str, tokens: Option<Tokens>, cost: Option<f64>) {
    let Some(tokens) = tokens else { return };
    let row = UsageRow {
        bot_id: bot.id.clone(),
        conversation_id: thread.to_string(),
        engine: engine.to_string(),
        model: model.to_string(),
        billing: billing(engine).to_string(),
        at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        input: tokens.input,
        output: tokens.output,
        cache_read: tokens.cache_read,
        cache_write: tokens.cache_write,
        estimated: tokens.estimated,
        // Claude on a plan reports what the turn would have cost on the API: not money spent.
        cost_usd: cost.filter(|_| billing(engine) == "key"),
    };
    // An API that doesn't say what a turn cost: the price Moo entered for it, if any.
    let mut row = row;
    if row.cost_usd.is_none() && row.billing == "key" {
        row.cost_usd = super::limits::priced(&super::limits::load(app), engine, &row);
    }
    let _ = app.with_store(|store| store.add_usage(&row));
}

/// Who pays for an AI: "plan" (a subscription), "key" (an API key, by use) or "free" (local).
pub fn billing(engine: &str) -> &'static str {
    match engines::find(engine) {
        Some(e) if e.id == engines::CLAUDE => {
            if std::env::var_os("ANTHROPIC_API_KEY").is_some() { "key" } else { "plan" }
        }
        Some(e) if e.kind == engines::Kind::Api && e.needs_key => "key",
        Some(e) if e.kind == engines::Kind::Api => "free",
        // Codex on a ChatGPT plan, and agent programs signed in to their own accounts (plans, mostly).
        _ => "plan",
    }
}

/// `GET /api/usage…`.
pub async fn route(app: &Arc<App>, rest: &[&str], query: impl Fn(&str) -> Option<String>) -> Answer {
    match rest {
        [] => breakdown(app, query("period").as_deref().unwrap_or("this-month"), query("by").as_deref().unwrap_or("bot")),
        ["plans"] => {
            // The Refresh button asks for a fresh reading.
            if query("fresh").is_some() {
                forget(app);
            }
            Ok((StatusCode::OK, plans(app).await))
        }
        ["csv", "save"] => save_csv(app, query("period").as_deref().unwrap_or("this-month")),
        _ => Err((StatusCode::NOT_FOUND, "no such usage".into())),
    }
}

/// The period before a period, as long as it (stage 19.7.3): for this month so far, the same
/// days of last month; for last month, the month before; for 30 days, the 30 before those.
pub fn previous(name: &str, now: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (from, to) = period(name, now)?;
    match name {
        "this-month" => {
            let (last, _) = period("last-month", now)?;
            Some((last, last + (to - from)))
        }
        "last-month" => {
            let local = from.with_timezone(&chrono::Local);
            let (year, month) = if local.month() == 1 { (local.year() - 1, 12) } else { (local.year(), local.month() - 1) };
            Some((chrono::Local.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()?.with_timezone(&Utc), from))
        }
        _ => Some((from - (to - from), from)),
    }
}

fn rows_between(app: &App, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<UsageRow>, (StatusCode, String)> {
    let since = from.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let until = to.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    Ok(app.try_store(|store| store.usage_since(&since)).map_err(fail)?.into_iter().filter(|r| r.at <= until).collect())
}

/// Each day of a period, oldest first, with its new tokens and cost: what the chart draws.
fn daily(rows: &[UsageRow], from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Value> {
    let day = |t: DateTime<Utc>| t.with_timezone(&chrono::Local).date_naive();
    let mut days: BTreeMap<chrono::NaiveDate, Sum> = BTreeMap::new();
    let mut d = day(from);
    while d <= day(to) {
        days.insert(d, Sum::default());
        d = d.succ_opt().unwrap_or(d);
        if days.len() > 400 {
            break;
        }
    }
    for row in rows {
        if let Ok(at) = DateTime::parse_from_rfc3339(&row.at) {
            days.entry(day(at.with_timezone(&Utc))).or_default().add(&Sum::of(row));
        }
    }
    days.into_iter()
        .map(|(d, sum)| {
            let mut wire = sum.wire();
            wire["day"] = json!(d.format("%Y-%m-%d").to_string());
            wire
        })
        .collect()
}

/// One line of the spreadsheet per turn.
pub fn csv(rows: &[UsageRow], names: &BTreeMap<String, String>) -> String {
    let field = |text: &str| if text.contains([',', '"', '\n']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() };
    let mut out = String::from("time,bot,ai,model,billing,input tokens,output tokens,cache read tokens,cache write tokens,estimated,cost usd\n");
    for r in rows {
        let bot = names.get(&r.bot_id).cloned().unwrap_or_else(|| "A deleted bot".into());
        let engine = engines::find(&r.engine).map(|e| e.name.to_string()).unwrap_or_else(|| r.engine.clone());
        let cost = r.cost_usd.map(|c| format!("{c:.6}")).unwrap_or_default();
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{}\n",
            field(&r.at), field(&bot), field(&engine), field(&r.model), field(&r.billing), r.input, r.output, r.cache_read, r.cache_write, r.estimated, cost
        ));
    }
    out
}

/// The period's turns as a spreadsheet in this computer's Downloads folder (the desktop app's
/// export; in a browser the file is downloaded from `/api/usage/csv` instead).
fn save_csv(app: &App, period_name: &str) -> Answer {
    let text = csv_text(app, period_name)?;
    let home = crate::store::user_home();
    let folder = [home.join("Downloads"), home.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(home);
    let path = folder.join(format!("catbot-usage-{period_name}-{}.csv", chrono::Local::now().format("%Y-%m-%d")));
    std::fs::write(&path, text).map_err(|e| fail(Box::new(e)))?;
    Ok((StatusCode::OK, json!({ "path": path })))
}

/// The CSV for a period.
pub fn csv_text(app: &App, period_name: &str) -> Result<String, (StatusCode, String)> {
    let (from, to) = period(period_name, Utc::now()).ok_or_else(|| bad("which period?"))?;
    let rows = rows_between(app, from, to)?;
    let names: BTreeMap<String, String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    Ok(csv(&rows, &names))
}

/// The start (and end) of a period, in UTC.
pub fn period(name: &str, now: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let local = now.with_timezone(&chrono::Local);
    let month_start = |year: i32, month: u32| chrono::Local.with_ymd_and_hms(year, month, 1, 0, 0, 0).single().map(|t| t.with_timezone(&Utc));
    match name {
        "this-month" => Some((month_start(local.year(), local.month())?, now)),
        "last-month" => {
            let (year, month) = if local.month() == 1 { (local.year() - 1, 12) } else { (local.year(), local.month() - 1) };
            Some((month_start(year, month)?, month_start(local.year(), local.month())?))
        }
        "30d" => Some((now - chrono::Duration::days(30), now)),
        _ => None,
    }
}

fn breakdown(app: &App, period_name: &str, by: &str) -> Answer {
    let (from, to) = period(period_name, Utc::now()).ok_or_else(|| bad("which period?"))?;
    if !["bot", "model", "engine", "day"].contains(&by) {
        return Err(bad("group by bot, model, engine or day"));
    }
    let since = from.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let until = to.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let rows: Vec<UsageRow> = rows_between(app, from, to)?;
    // The same length of time before, to compare with (stage 19.7.3).
    let before = match previous(period_name, Utc::now()) {
        Some((f, t)) => {
            let mut sum = Sum::default();
            for row in rows_between(app, f, t)? {
                sum.add(&Sum::of(&row));
            }
            json!({ "from": f.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), "to": t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), "total": sum.wire() })
        }
        None => Value::Null,
    };
    let names: BTreeMap<String, String> =
        app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    let label = |r: &UsageRow| -> String {
        match by {
            "bot" => names.get(&r.bot_id).cloned().unwrap_or_else(|| "A deleted bot".into()),
            "model" => {
                let engine = engines::find(&r.engine).map(|e| e.name).unwrap_or(r.engine.as_str());
                if r.model.is_empty() { format!("{engine} (default)") } else { format!("{engine} · {}", r.model) }
            }
            "engine" => engines::find(&r.engine).map(|e| e.name.to_string()).unwrap_or_else(|| r.engine.clone()),
            _ => DateTime::parse_from_rfc3339(&r.at).map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()).unwrap_or_default(),
        }
    };
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    let mut total = Sum::default();
    for row in &rows {
        let sum = Sum::of(row);
        total.add(&sum);
        let entry = groups.entry(label(row)).or_insert_with(|| Sum::default().wire());
        let mut current: Sum = Sum::from_wire(entry);
        current.add(&sum);
        *entry = current.wire();
    }
    let mut list: Vec<Value> = groups.into_iter().map(|(label, mut sum)| {
        sum["label"] = json!(label);
        sum
    }).collect();
    if by == "day" {
        list.reverse();
    } else {
        list.sort_by(|a, b| b["newTokens"].as_u64().cmp(&a["newTokens"].as_u64()));
    }
    // What online voice used (stage 22.4), beside the bots' turns.
    let voice = super::voice::spent(app, from, to);
    Ok((StatusCode::OK, json!({ "period": period_name, "by": by, "from": since, "to": until, "rows": list, "total": total.wire(), "daily": daily(&rows, from, to), "previous": before, "voice": voice })))
}

/// Totals for a group of turns.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Sum {
    turns: u64,
    tokens: u64,
    new_tokens: u64,
    cached: u64,
    cost: f64,
    costed: bool,
    estimated: bool,
}

impl Sum {
    fn of(r: &UsageRow) -> Sum {
        let t = Tokens { input: r.input, output: r.output, cache_read: r.cache_read, cache_write: r.cache_write, estimated: r.estimated };
        Sum {
            turns: 1,
            tokens: t.new_tokens() + t.cache_read,
            new_tokens: t.new_tokens(),
            cached: t.cache_read,
            cost: r.cost_usd.unwrap_or(0.0),
            costed: r.cost_usd.is_some(),
            estimated: r.estimated,
        }
    }

    fn add(&mut self, o: &Sum) {
        self.turns += o.turns;
        self.tokens += o.tokens;
        self.new_tokens += o.new_tokens;
        self.cached += o.cached;
        self.cost += o.cost;
        self.costed |= o.costed;
        self.estimated |= o.estimated;
    }

    fn wire(&self) -> Value {
        json!({
            "turns": self.turns,
            "tokens": self.tokens,
            "newTokens": self.new_tokens,
            "cachedTokens": self.cached,
            "costUsd": self.costed.then_some((self.cost * 10_000.0).round() / 10_000.0),
            "estimated": self.estimated,
        })
    }

    fn from_wire(v: &Value) -> Sum {
        Sum {
            turns: v["turns"].as_u64().unwrap_or(0),
            tokens: v["tokens"].as_u64().unwrap_or(0),
            new_tokens: v["newTokens"].as_u64().unwrap_or(0),
            cached: v["cachedTokens"].as_u64().unwrap_or(0),
            cost: v["costUsd"].as_f64().unwrap_or(0.0),
            costed: v["costUsd"].is_number(),
            estimated: v["estimated"].as_bool().unwrap_or(false),
        }
    }
}

/// Reads the plans again in the background if the last reading is more than five minutes old.
pub fn refresh_soon(app: &Arc<App>) {
    let stale = app.plan_usage.lock().expect("plan usage lock").as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(300));
    if stale {
        let app = app.clone();
        tokio::spawn(async move {
            plans(&app).await;
        });
    }
}

/// Forgets the last reading, so the next one is fresh.
pub fn forget(app: &App) {
    *app.plan_usage.lock().expect("plan usage lock") = None;
}

/// The plan windows, as `{ plans: [...] }`: fetched at most once a minute.
pub async fn plans(app: &App) -> Value {
    {
        let cached = app.plan_usage.lock().expect("plan usage lock");
        if let Some((at, value)) = cached.as_ref()
            && at.elapsed() < Duration::from_secs(60)
        {
            return value.clone();
        }
    }
    let uses_claude = app.with_store(|store| store.bots()).unwrap_or_default().iter().any(|b| b.engine == engines::CLAUDE);
    let mut plans = Vec::new();
    if uses_claude && billing(engines::CLAUDE) == "plan" {
        plans.push(claude_plan().await);
    }
    let uses_codex = app.with_store(|store| store.bots()).unwrap_or_default().iter().any(|b| b.engine == engines::CODEX);
    if uses_codex && let Some(program) = engines::find(engines::CODEX).and_then(engines::agent_program) {
        plans.push(codex_plan(&program).await);
    }
    let value = json!({ "plans": plans, "fetchedAt": Utc::now().to_rfc3339() });
    *app.plan_usage.lock().expect("plan usage lock") = Some((std::time::Instant::now(), value.clone()));
    value
}

/// Claude's windows, from Anthropic's usage address with Claude Code's sign-in.
async fn claude_plan() -> Value {
    let base = json!({ "engine": engines::CLAUDE, "name": "Claude" });
    let with = |extra: Value| {
        let mut row = base.clone();
        if let (Value::Object(row), Value::Object(extra)) = (&mut row, extra) {
            row.extend(extra);
        }
        row
    };
    let Some(token) = claude_token() else {
        return with(json!({ "ok": false, "error": "Claude Code isn't signed in on this computer (run `claude` and sign in), so its plan usage can't be read." }));
    };
    let url = std::env::var("CATBOT_CLAUDE_USAGE_URL").unwrap_or_else(|_| "https://api.anthropic.com/api/oauth/usage".into());
    let response = crate::http::client()
        .get(url)
        .timeout(Duration::from_secs(12))
        .bearer_auth(token)
        .header("accept", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await;
    let response = match response {
        Ok(r) => r,
        Err(e) => return with(json!({ "ok": false, "error": format!("Couldn't reach Anthropic for Claude's usage ({e}).") })),
    };
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return with(json!({ "ok": false, "error": "Claude Code's sign-in has expired: run `claude` once to refresh it." }));
    }
    let Ok(body) = response.json::<Value>().await else {
        return with(json!({ "ok": false, "error": format!("Anthropic answered {status} without usage.") }));
    };
    with(parse_claude(&body))
}

/// Codex's windows on the ChatGPT plan, asked of Codex itself (`account/rateLimits/read`).
async fn codex_plan(program: &std::path::Path) -> Value {
    let row = |extra: Value| {
        let mut row = json!({ "engine": engines::CODEX, "name": "Codex (ChatGPT)" });
        if let (Value::Object(row), Value::Object(extra)) = (&mut row, extra) {
            row.extend(extra);
        }
        row
    };
    if !crate::codex::signed_in() {
        return row(json!({ "ok": false, "error": "Codex isn't signed in: sign in with ChatGPT in a Codex bot's settings." }));
    }
    let read = async {
        let talk = crate::codex::Talk::open(program, &crate::codex::person_home()).await?;
        talk.call("account/rateLimits/read", json!({})).await
    };
    match tokio::time::timeout(Duration::from_secs(30), read).await {
        Ok(Ok(body)) => row(parse_codex(&body, Utc::now())),
        Ok(Err(problem)) => row(json!({ "ok": false, "error": format!("Codex didn't say its usage ({problem}).") })),
        Err(_) => row(json!({ "ok": false, "error": "Codex took too long to say its usage." })),
    }
}

/// Codex's answer as Catbot's plan row: a window of five hours or so is the session, one of a
/// week the week; any other is shown by its length.
pub fn parse_codex(body: &Value, now: DateTime<Utc>) -> Value {
    let snapshot = &body["rateLimits"];
    let mut windows = Vec::new();
    let mut headline = None;
    for w in [&snapshot["primary"], &snapshot["secondary"]] {
        let Some(used) = w["usedPercent"].as_f64() else { continue };
        let minutes = w["windowDurationMins"].as_i64().unwrap_or(0);
        // Seconds, though a value in milliseconds is understood too.
        let resets = w["resetsAt"].as_i64().map(|t| if t > 100_000_000_000 { t / 1000 } else { t }).and_then(|t| DateTime::from_timestamp(t, 0)).map(|t| t.to_rfc3339());
        let mut line = json!({ "usedPercent": used.clamp(0.0, 100.0), "resetsAt": resets });
        let (label, kind) = match minutes {
            1..=480 => ("Current session".to_string(), "session"),
            m if m >= 6 * 24 * 60 => ("This week".to_string(), "week"),
            0 => ("Usage".to_string(), "other"),
            m if m % (24 * 60) == 0 => (format!("Every {} days", m / (24 * 60)), "other"),
            m => (format!("Every {} hours", m / 60), "other"),
        };
        line["label"] = json!(label);
        line["kind"] = json!(kind);
        if kind == "week" || headline.is_none() {
            headline = Some(forecast(&line, (minutes.max(60) as f64) / 60.0, now));
        }
        windows.push(line);
    }
    windows.sort_by_key(|w| if w["kind"] == "session" { 0 } else { 1 });
    json!({ "ok": !windows.is_empty(), "plan": snapshot["planType"], "windows": windows, "extra": [], "headline": headline })
}

/// Anthropic's usage answer as Catbot's plan row: `five_hour` is the session, `seven_day` the
/// week, and other windows (`seven_day_opus`, …) are extra lines.
pub fn parse_claude(body: &Value) -> Value {
    let window = |w: &Value| -> Option<Value> {
        let used = w["utilization"].as_f64()?;
        Some(json!({ "usedPercent": used.clamp(0.0, 100.0), "resetsAt": w["resets_at"].as_str() }))
    };
    let mut extra = Vec::new();
    if let Some(map) = body.as_object() {
        for (key, value) in map {
            if key == "five_hour" || key == "seven_day" {
                continue;
            }
            let Some(mut line) = window(value) else { continue };
            let name = key.trim_start_matches("seven_day_").trim_start_matches("five_hour_").replace('_', " ");
            let span = if key.starts_with("five_hour") { "this session" } else { "this week" };
            let mut label: String = name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
            label.push_str(&name.chars().skip(1).collect::<String>());
            line["label"] = json!(format!("{label}, {span}"));
            extra.push(line);
        }
    }
    let session = window(&body["five_hour"]);
    let week = window(&body["seven_day"]);
    let headline = week.as_ref().map(|w| forecast(w, 7.0 * 24.0, Utc::now()));
    let ok = session.is_some() || week.is_some();
    let windows: Vec<Value> = [
        session.map(|mut w| {
            w["label"] = json!("Current session");
            w["kind"] = json!("session");
            w
        }),
        week.map(|mut w| {
            w["label"] = json!("This week");
            w["kind"] = json!("week");
            w
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    json!({
        "ok": ok,
        "plan": body["subscription_type"].as_str(),
        "windows": windows,
        "extra": extra,
        "headline": headline,
    })
}

/// Whether a window is on track to last until it resets, at the pace so far.
pub fn forecast(window: &Value, length_hours: f64, now: DateTime<Utc>) -> Value {
    let used = window["usedPercent"].as_f64().unwrap_or(0.0);
    let Some(resets) = window["resetsAt"].as_str().and_then(|r| DateTime::parse_from_rfc3339(r).ok()).map(|t| t.with_timezone(&Utc)) else {
        return json!({ "onTrack": used < 100.0, "text": if used < 100.0 { "On track." } else { "At the limit." } });
    };
    let left = (resets - now).num_seconds().max(0) as f64 / 3600.0;
    let gone = (length_hours - left).max(0.1);
    let projected = used / gone * length_hours;
    if used >= 100.0 {
        return json!({ "onTrack": false, "text": "At the limit until it resets." });
    }
    if projected <= 100.0 {
        json!({ "onTrack": true, "text": "On track. You should reach the reset with room to spare.", "resetsAt": resets.to_rfc3339() })
    } else {
        let hours_to_limit = (100.0 - used) / (used / gone);
        let at = now + chrono::Duration::seconds((hours_to_limit * 3600.0) as i64);
        json!({ "onTrack": false, "text": "At this pace you'll reach the limit before it resets.", "limitAt": at.to_rfc3339(), "resetsAt": resets.to_rfc3339() })
    }
}

/// Claude Code's sign-in token on this computer, if it's signed in and not expired.
fn claude_token() -> Option<String> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from).or_else(|| {
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|home| std::path::PathBuf::from(home).join(".claude"))
    })?;
    let text = std::fs::read_to_string(dir.join(".credentials.json")).ok()?;
    let saved: Value = serde_json::from_str(&text).ok()?;
    let oauth = &saved["claudeAiOauth"];
    if let Some(expires) = oauth["expiresAt"].as_i64()
        && expires <= Utc::now().timestamp_millis()
    {
        return None;
    }
    oauth["accessToken"].as_str().filter(|t| !t.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claudes_answer_becomes_windows_and_a_forecast() {
        let now = Utc::now();
        let reset = (now + chrono::Duration::hours(24)).to_rfc3339();
        let body = json!({
            "five_hour": { "utilization": 78.0, "resets_at": (now + chrono::Duration::hours(2)).to_rfc3339() },
            "seven_day": { "utilization": 61.0, "resets_at": reset },
            "seven_day_opus": { "utilization": 12.0, "resets_at": reset },
            "extra_usage": { "is_enabled": false },
        });
        let plan = parse_claude(&body);
        assert_eq!(plan["windows"][0]["label"], "Current session");
        assert_eq!(plan["windows"][1]["usedPercent"], 61.0);
        assert_eq!(plan["extra"][0]["label"], "Opus, this week");
        // 61% used with six of seven days gone: about 71% by the reset.
        assert_eq!(plan["headline"]["onTrack"], true);
        let fast = forecast(&json!({ "usedPercent": 61.0, "resetsAt": (now + chrono::Duration::days(5)).to_rfc3339() }), 168.0, now);
        assert_eq!(fast["onTrack"], false);
    }

    #[test]
    fn codexs_answer_becomes_windows() {
        let now = Utc::now();
        let body = json!({ "rateLimits": {
            "planType": "plus",
            "primary": { "usedPercent": 42, "windowDurationMins": 300, "resetsAt": now.timestamp() + 3600 },
            "secondary": { "usedPercent": 100, "windowDurationMins": 10080, "resetsAt": (now.timestamp() + 86400) * 1000 },
        } });
        let plan = parse_codex(&body, now);
        assert_eq!((plan["ok"].as_bool(), plan["plan"].as_str()), (Some(true), Some("plus")));
        assert_eq!((plan["windows"][0]["label"].as_str(), plan["windows"][0]["usedPercent"].as_f64()), (Some("Current session"), Some(42.0)));
        assert_eq!(plan["windows"][1]["label"], "This week");
        let resets = DateTime::parse_from_rfc3339(plan["windows"][1]["resetsAt"].as_str().unwrap()).unwrap();
        assert!((resets.timestamp() - (now.timestamp() + 86400)).abs() < 2, "milliseconds read as seconds");
        assert_eq!(plan["headline"]["onTrack"], false);
        assert_eq!(parse_codex(&json!({ "rateLimits": {} }), now)["ok"], false);
    }

    #[test]
    fn the_period_before_is_as_long() {
        let now = Utc.with_ymd_and_hms(2026, 11, 24, 18, 0, 0).unwrap();
        let (from, to) = period("30d", now).unwrap();
        let (pf, pt) = previous("30d", now).unwrap();
        assert_eq!((pt, pt - pf), (from, to - from));
        let (from, to) = period("this-month", now).unwrap();
        let (pf, pt) = previous("this-month", now).unwrap();
        assert_eq!(pt - pf, to - from, "the same days of last month");
        assert!(pf < from && pt < from);
        let (lf, _) = period("last-month", now).unwrap();
        let (_, before_end) = previous("last-month", now).unwrap();
        assert_eq!(before_end, lf);
    }

    #[test]
    fn turns_make_a_spreadsheet() {
        let row = UsageRow {
            bot_id: "b".into(),
            conversation_id: "c".into(),
            engine: "claude".into(),
            model: "sonnet".into(),
            billing: "plan".into(),
            at: "2026-11-24T10:00:00Z".into(),
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            estimated: false,
            cost_usd: Some(0.0125),
        };
        let mut names = BTreeMap::new();
        names.insert("b".to_string(), "Kiwi, the \"chief\"".to_string());
        let text = csv(&[row], &names);
        let mut lines = text.lines();
        assert!(lines.next().unwrap().starts_with("time,bot,ai,model"));
        assert_eq!(lines.next().unwrap(), "2026-11-24T10:00:00Z,\"Kiwi, the \"\"chief\"\"\",Claude,sonnet,plan,10,5,0,0,false,0.012500");
        let days = daily(&[], Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap(), Utc.with_ymd_and_hms(2026, 11, 3, 12, 0, 0).unwrap());
        assert_eq!(days.len(), 3, "every day, used or not");
    }

    #[test]
    fn periods_and_billing() {
        let now = Utc::now();
        let (from, to) = period("this-month", now).unwrap();
        assert!(from <= now && to == now);
        let (last_from, last_to) = period("last-month", now).unwrap();
        assert_eq!(last_to, from);
        assert!(last_from < last_to);
        assert!(period("ever", now).is_none());
        assert_eq!(billing("ollama"), "free");
        assert_eq!(billing("openai"), "key");
    }
}
