//! Burst mode (stages 32.15 and 32.16, after OpenMausBot 0.1.153's bursts): a bot splits a
//! request into independent parts, runs each as a fresh chat of its own, a few at once, and then
//! writes one merged result in the chat it was asked in.
//!
//! Moo's choices (2026-11-27): the parts are copies of the same bot, each in a new chat of its
//! own; five parts or fewer start by themselves, bigger bursts show a preview with Run, Edit and
//! Cancel; three run at once.
//!
//! - The bot proposes one with `<burst>{"task","instructions","items":[…]}</burst>` in its reply
//!   (the person's `/burst <task>` asks it to); Catbot takes the tag out and shows a card.
//! - The card is the burst's record: a step message whose `detail` holds the task, the parts and
//!   where each stands (`queued`, `running`, `done`, `failed`, `cancelled`).
//! - `POST /api/threads/:thread/burst/:message` `{ action: "run" | "cancel" | "retry", items? }`:
//!   Run (with the person's edited list, if any), Cancel, or Retry the parts that failed (also
//!   how a burst cut off by a restart goes on).
//! - Each part runs in a new chat of the bot's, titled after the burst; once merged they're
//!   archived (still there to open from the card).

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde_json::{Value, json};
use tokio::task::JoinSet;

use super::{Answer, App, bad, turns, wire};
use crate::store::{Bot, Role};

const OPEN: &str = "<burst>";
const CLOSE: &str = "</burst>";
/// Parts at once (Moo's choice).
pub const PARALLEL: usize = 3;
/// Up to this many parts start by themselves; more wait for Run (Moo's choice).
pub const STARTS_ALONE: usize = 5;
/// The most parts a burst may have.
pub const MAX_PARTS: usize = 25;
const ITEM_MAX: usize = 2_000;
const INSTRUCTIONS_MAX: usize = 12_000;
/// How much of each part the merge turn reads.
const MERGE_BUDGET: usize = 150_000;
const RESULT_MAX: usize = 20_000;

/// What a bot is told about bursts (in its own chats).
pub const NOTE: &str = "\n\nBursts: when the person asks you to burst something, or a request is clearly many independent \
items of work (one per file, page, topic or person, say), you can run it as a burst. Put in your reply \
<burst>{\"task\":\"a short name for the whole job\",\"instructions\":\"what every part needs to know\",\"items\":[\"one part\",\"another part\"]}</burst>. \
Each item runs as a separate chat of yours, three at a time, and when they're done you write one merged result here. \
Up to 5 items start at once; more wait for the person's Run. Do none of the parts' work in that reply; say briefly what you split it into.";

/// Read-modify-write of a burst's card, one at a time.
static EDITING: Mutex<()> = Mutex::new(());

/// `/burst <task>`: the turn's words become a request to propose a burst (the chat keeps what was typed).
pub fn command(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("/burst")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let request = rest.trim();
    Some(if request.is_empty() {
        "The person typed /burst without a request. Ask them, in one short line, what they want run in parallel.".to_string()
    } else {
        format!(
            "The person used /burst for this request: {request}\n\nPropose it as a burst now (the <burst> tag): their list as the items when they \
             gave one, otherwise split it into one item per independent part yourself. Do none of the work here."
        )
    })
}

/// Takes `<burst>…</burst>` out of a reply: the reply without it, and the first proposal in it.
pub fn take(reply: &str) -> (String, Option<String>) {
    let Some(start) = reply.find(OPEN) else { return (reply.to_string(), None) };
    let after = &reply[start + OPEN.len()..];
    let (inside, rest) = match after.find(CLOSE) {
        Some(end) => (&after[..end], &after[end + CLOSE.len()..]),
        None => (after, ""),
    };
    let mut kept = format!("{}{}", &reply[..start], rest);
    // Any further tags go too (one burst per reply).
    while let Some(start) = kept.find(OPEN) {
        let end = kept[start..].find(CLOSE).map(|e| start + e + CLOSE.len()).unwrap_or(kept.len());
        kept.replace_range(start..end, "");
    }
    (kept.trim().to_string(), Some(inside.trim().to_string()))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max { format!("{}…", text.chars().take(max - 1).collect::<String>().trim_end()) } else { text.to_string() }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A part's short title, for the card and its chat.
fn title_of(item: &str) -> String {
    clip(&one_line(item), 60)
}

/// A proposal read: its task, instructions and items (cleaned), or why it can't be used.
pub fn read(raw: &str) -> Result<(String, String, Vec<String>), String> {
    let proposal: Value = serde_json::from_str(raw).map_err(|e| format!("it isn't JSON Catbot can read ({e})"))?;
    let task = proposal["task"].as_str().map(one_line).filter(|t| !t.is_empty()).ok_or("it has no task")?;
    let instructions = clip(proposal["instructions"].as_str().unwrap_or_default().trim(), INSTRUCTIONS_MAX);
    let items: Vec<String> = proposal["items"]
        .as_array()
        .ok_or("it has no items")?
        .iter()
        .filter_map(Value::as_str)
        .map(|i| clip(i.trim(), ITEM_MAX))
        .filter(|i| !i.is_empty())
        .collect();
    if items.len() < 2 {
        return Err("a burst needs at least two items".into());
    }
    if items.len() > MAX_PARTS {
        return Err(format!("a burst can have at most {MAX_PARTS} items (this one has {})", items.len()));
    }
    Ok((clip(&task, 200), instructions, items))
}

fn parts_of(items: &[String]) -> Vec<Value> {
    items.iter().enumerate().map(|(seq, item)| json!({ "seq": seq, "item": item, "title": title_of(item), "state": "queued" })).collect()
}

/// About how long it takes: three minutes a round of three.
fn estimate_minutes(parts: usize) -> usize {
    parts.div_ceil(PARALLEL).max(1) * 3
}

/// Shows the card for a bot's proposal (or a note when it can't be used), and starts a small one.
pub fn propose(app: &Arc<App>, thread: &str, bot: &Bot, raw: &str) {
    let (task, instructions, items) = match read(raw) {
        Ok(read) => read,
        Err(why) => {
            turns::save_and_send(app, thread, Role::Note, &format!("{} proposed a burst Catbot couldn't use: {why}.", bot.name), None);
            return;
        }
    };
    let starts = items.len() <= STARTS_ALONE;
    let model = bot.model.clone().filter(|m| !m.is_empty()).map(|m| format!(" ({m})")).unwrap_or_default();
    let detail = json!({
        "kind": "burst",
        "botId": bot.id,
        "botName": bot.name,
        "task": task,
        "instructions": instructions,
        "parts": parts_of(&items),
        "parallel": PARALLEL,
        "ai": format!("{}{model}", crate::engines::find(&bot.engine).map(|e| e.name).unwrap_or(bot.engine.as_str())),
        "estimateMinutes": estimate_minutes(items.len()),
        "state": if starts { "running" } else { "proposed" },
    });
    let Some(card) = turns::save_and_send(app, thread, Role::Step, &format!("Burst: {task}"), Some(&detail)) else { return };
    let Some(card) = card["id"].as_str().and_then(wire::parse_message_id) else { return };
    if starts {
        start(app.clone(), thread.to_string(), card);
    }
}

/// Changes a burst's card, under the lock, and redraws it. The change returns false to leave it.
fn edit(app: &App, thread: &str, card: i64, change: impl FnOnce(&mut Value) -> bool) -> Option<Value> {
    let _held = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut detail = app.with_store(|store| store.message_in(thread, card)).flatten()?.detail?;
    if detail["kind"] != "burst" || !change(&mut detail) {
        return Some(detail);
    }
    let wire = app.with_store(|store| {
        store.update_detail(card, &detail)?;
        let parent = store.previous_message_id(thread, card)?;
        Ok(wire::message(&store.message(card)?.ok_or("card vanished")?, parent))
    });
    if let Some(wire) = wire {
        app.hub.send(json!({ "kind": "message.patch", "threadId": thread, "message": wire }));
    }
    Some(detail)
}

fn load(app: &App, thread: &str, card: i64) -> Option<Value> {
    app.with_store(|store| store.message_in(thread, card)).flatten()?.detail.filter(|d| d["kind"] == "burst")
}

fn set_part(part: &mut Value, state: &str) {
    part["state"] = json!(state);
}

/// Runs a burst's parts, a few at once, then the merge. One driver per burst.
pub fn start(app: Arc<App>, thread: String, card: i64) {
    tokio::spawn(async move { drive(app, thread, card).await });
}

async fn drive(app: Arc<App>, thread: String, card: i64) {
    let Some(first) = load(&app, &thread, card) else { return };
    let Some(bot_id) = first["botId"].as_str().map(str::to_string) else { return };
    let mut running: JoinSet<(usize, Result<String, String>)> = JoinSet::new();
    loop {
        let Some(detail) = load(&app, &thread, card) else { return };
        let going = detail["state"] == "running";
        // Start parts while there's room.
        while going && running.len() < PARALLEL {
            let Some(bot) = app.with_store(|store| store.bot_by_id(&bot_id)).flatten() else { break };
            let mut picked = None;
            edit(&app, &thread, card, |d| {
                let total = d["parts"].as_array().map(Vec::len).unwrap_or(0);
                let Some(part) = d["parts"].as_array_mut().and_then(|p| p.iter_mut().find(|p| p["state"] == "queued")) else { return false };
                set_part(part, "running");
                picked = Some((part["seq"].as_u64().unwrap_or(0) as usize, part["item"].as_str().unwrap_or_default().to_string(), total));
                true
            });
            let Some((seq, item, total)) = picked else { break };
            let opening = opening(&detail, seq, total, &item);
            let Some(conversation) = app.with_store(|store| store.new_conversation(&bot.id)) else {
                edit(&app, &thread, card, |d| {
                    if let Some(p) = d["parts"].get_mut(seq) {
                        set_part(p, "failed");
                        p["error"] = json!("Catbot couldn't open a chat for it.");
                    }
                    true
                });
                continue;
            };
            let name = format!("Burst · {} · {}", clip(detail["task"].as_str().unwrap_or("parts"), 36), seq + 1);
            let _ = app.with_store(|store| store.set_title(&conversation.id, &name, false));
            edit(&app, &thread, card, |d| {
                if let Some(p) = d["parts"].get_mut(seq) {
                    p["thread"] = json!(conversation.id);
                }
                true
            });
            app.announce_bot(&bot.id);
            let app2 = app.clone();
            running.spawn(async move { (seq, part(&app2, bot, conversation, &item, &opening).await) });
        }
        let Some(joined) = running.join_next().await else { break };
        let Ok((seq, outcome)) = joined else { continue };
        edit(&app, &thread, card, |d| {
            let cancelled = d["state"] == "cancelled";
            if let Some(p) = d["parts"].get_mut(seq) {
                match &outcome {
                    _ if cancelled => set_part(p, "cancelled"),
                    Ok(text) => {
                        set_part(p, "done");
                        p["result"] = json!(clip(text, RESULT_MAX));
                    }
                    Err(why) => {
                        set_part(p, "failed");
                        p["error"] = json!(clip(&one_line(why), 200));
                    }
                }
            }
            true
        });
    }
    // All in: the merge, unless it was cancelled or nothing came of it.
    let Some(detail) = load(&app, &thread, card) else { return };
    if detail["state"] != "running" {
        return;
    }
    let parts = detail["parts"].as_array().cloned().unwrap_or_default();
    let done: Vec<&Value> = parts.iter().filter(|p| p["state"] == "done").collect();
    if parts.iter().any(|p| p["state"] == "queued") {
        return; // Restarted with parts still to run: Retry goes on.
    }
    if done.is_empty() {
        edit(&app, &thread, card, |d| {
            d["state"] = json!("finished");
            d["note"] = json!("No part finished, so there's nothing to merge. Retry runs the failed ones again.");
            true
        });
        return;
    }
    edit(&app, &thread, card, |d| {
        d["state"] = json!("merging");
        true
    });
    merge(&app, &thread, card, &detail, &bot_id).await;
    let failed = parts.iter().filter(|p| p["state"] == "failed").count();
    edit(&app, &thread, card, |d| {
        d["state"] = json!("finished");
        if failed > 0 {
            d["note"] = json!(format!("{failed} part{} failed: Retry runs {} again and merges anew.", if failed == 1 { "" } else { "s" }, if failed == 1 { "it" } else { "them" }));
        }
        true
    });
    // The parts' chats, out of the bot's list (the card still opens them).
    for p in parts.iter().filter(|p| p["state"] == "done") {
        if let Some(t) = p["thread"].as_str() {
            let _ = app.with_store(|store| store.set_archived(t, true));
        }
    }
    app.announce_bot(&bot_id);
}

/// What a part's chat is asked (theirs: `childOpening`).
fn opening(detail: &Value, seq: usize, total: usize, item: &str) -> String {
    format!(
        "[Part {} of {total} of a burst you run for this request: {}. These are your own words, not the person's. Work on this part only and end \
         with a clear result, since it will be merged with the others. Don't ask questions: if something is unclear, make a sensible choice and \
         say so. Don't hand work to other bots.]\n\n{}\n\nThis part: {item}",
        seq + 1,
        one_line(detail["task"].as_str().unwrap_or_default()),
        detail["instructions"].as_str().unwrap_or_default()
    )
}

/// One part: its chat gets the opening as a note, then the bot's turn; what it wrote is the result.
async fn part(app: &Arc<App>, bot: Bot, conversation: crate::store::Conversation, item: &str, opening: &str) -> Result<String, String> {
    let note = turns::save_and_send(app, &conversation.id, Role::Note, &format!("Part of a burst: {}", title_of(item)), Some(&json!({ "burstPart": true })))
        .ok_or("Catbot couldn't start it")?;
    let asked = note["id"].as_str().and_then(wire::parse_message_id).ok_or("Catbot couldn't start it")?;
    match turns::run_through(app, &bot, conversation, asked, opening, false).await {
        Ok(Some(text)) if !text.trim().is_empty() => Ok(text),
        Ok(_) => Err("it ended without a result".into()),
        Err(why) => Err(why.to_string()),
    }
}

/// The merge prompt (theirs: `mergePrompt`).
fn merge_prompt(detail: &Value) -> String {
    let parts = detail["parts"].as_array().cloned().unwrap_or_default();
    let done: Vec<&Value> = parts.iter().filter(|p| p["state"] == "done").collect();
    let failed: Vec<&Value> = parts.iter().filter(|p| p["state"] == "failed").collect();
    let cancelled = parts.iter().filter(|p| p["state"] == "cancelled").count();
    let each = (MERGE_BUDGET / done.len().max(1)).min(RESULT_MAX);
    let body: Vec<String> = done.iter().map(|p| format!("## {}\n{}", p["title"].as_str().unwrap_or_default(), clip(p["result"].as_str().unwrap_or_default(), each))).collect();
    let failures: Vec<String> = failed.iter().map(|p| format!("- {}: {}", p["title"].as_str().unwrap_or_default(), p["error"].as_str().unwrap_or("it didn't finish"))).collect();
    let left = if cancelled > 0 { format!(" {cancelled} more were cancelled.") } else { String::new() };
    let mut text = format!(
        "[The burst you ran for this request has finished: {}. {} parts are done and {} failed.{left} Write one merged result for the person here, \
         in your own words, from the parts below: don't repeat the work or hand it on. Then name every part that failed, and say why. Keep it as \
         short as the result allows.]\n\n{}",
        one_line(detail["task"].as_str().unwrap_or_default()),
        done.len(),
        failed.len(),
        body.join("\n\n")
    );
    if !failures.is_empty() {
        text.push_str(&format!("\n\nParts that failed:\n{}", failures.join("\n")));
    }
    text
}

/// The merged result, written by the bot in the chat the burst came from (once it's free).
async fn merge(app: &Arc<App>, thread: &str, card: i64, detail: &Value, bot_id: &str) {
    let Some(detail) = load(app, thread, card).or(Some(detail.clone())) else { return };
    for _ in 0..900 {
        if !app.turns.busy(thread) && !app.channel_busy(thread) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    let (Some(bot), Some(conversation)) = (app.with_store(|store| store.bot_by_id(bot_id)).flatten(), app.with_store(|store| store.conversation(thread)).flatten()) else {
        return;
    };
    if let Err(why) = turns::run_through(app, &bot, conversation, card, &merge_prompt(&detail), false).await {
        // Couldn't write it: the parts go in as they came.
        let parts: Vec<String> = detail["parts"].as_array().into_iter().flatten().filter(|p| p["state"] == "done").map(|p| format!("**{}**\n\n{}", p["title"].as_str().unwrap_or_default(), p["result"].as_str().unwrap_or_default())).collect();
        turns::save_and_send(app, thread, Role::Note, &format!("{} couldn't merge the burst ({why}); its parts follow as they came.", bot.name), None);
        turns::save_and_send(app, thread, Role::Bot, &parts.join("\n\n---\n\n"), Some(&json!({ "botId": bot.id })));
    }
}

/// `POST /api/threads/:thread/burst/:message`.
pub fn answer(app: &Arc<App>, thread: &str, message: &str, body: &Value) -> Answer {
    let card = wire::parse_message_id(message).ok_or_else(|| bad("which burst?"))?;
    let detail = load(app, thread, card).ok_or((StatusCode::NOT_FOUND, "no such burst in this chat".to_string()))?;
    let state = detail["state"].as_str().unwrap_or_default().to_string();
    match body["action"].as_str() {
        Some("run") if state == "proposed" => {
            // The person's edited list, if they changed it.
            let items: Option<Vec<String>> = body["items"].as_array().map(|l| l.iter().filter_map(Value::as_str).map(|i| clip(i.trim(), ITEM_MAX)).filter(|i| !i.is_empty()).collect());
            if let Some(items) = &items {
                if items.is_empty() {
                    return Err(bad("a burst needs at least one item"));
                }
                if items.len() > MAX_PARTS {
                    return Err(bad(format!("a burst can have at most {MAX_PARTS} items")));
                }
            }
            edit(app, thread, card, |d| {
                if let Some(items) = &items {
                    d["parts"] = json!(parts_of(items));
                    d["estimateMinutes"] = json!(estimate_minutes(items.len()));
                }
                d["state"] = json!("running");
                true
            });
            start(app.clone(), thread.to_string(), card);
        }
        Some("cancel") if matches!(state.as_str(), "proposed" | "running" | "interrupted") => {
            let mut stopping = Vec::new();
            edit(app, thread, card, |d| {
                d["state"] = json!("cancelled");
                for p in d["parts"].as_array_mut().into_iter().flatten() {
                    if p["state"] == "running"
                        && let Some(t) = p["thread"].as_str()
                    {
                        stopping.push(t.to_string());
                    }
                    if p["state"] == "queued" || p["state"] == "running" {
                        set_part(p, "cancelled");
                    }
                }
                true
            });
            for t in stopping {
                app.turns.interrupt(&t);
            }
        }
        Some("retry") if matches!(state.as_str(), "finished" | "interrupted") => {
            let mut any = false;
            edit(app, thread, card, |d| {
                for p in d["parts"].as_array_mut().into_iter().flatten() {
                    if p["state"] == "failed" || p["state"] == "queued" {
                        set_part(p, "queued");
                        if let Some(map) = p.as_object_mut() {
                            map.remove("error");
                        }
                        any = true;
                    }
                }
                if any {
                    d["state"] = json!("running");
                    if let Some(map) = d.as_object_mut() {
                        map.remove("note");
                    }
                }
                any
            });
            if !any {
                return Err((StatusCode::CONFLICT, "there's nothing to run again".into()));
            }
            start(app.clone(), thread.to_string(), card);
        }
        Some(other) => return Err((StatusCode::CONFLICT, format!("a burst that's {state} can't {other}"))),
        None => return Err(bad("run, cancel or retry?")),
    }
    let detail = load(app, thread, card).unwrap_or(Value::Null);
    Ok((StatusCode::OK, json!({ "ok": true, "state": detail["state"] })))
}

/// At start: bursts that were running when Catbot stopped. Their running parts were cut off; the
/// card says so, and Retry (Continue) runs those and the rest.
pub fn after_restart(app: &App) {
    let cards = app.with_store(|store| store.burst_cards()).unwrap_or_default();
    for (thread, id) in cards {
        edit(app, &thread, id, |d| {
            if !matches!(d["state"].as_str(), Some("running" | "merging")) {
                return false;
            }
            for p in d["parts"].as_array_mut().into_iter().flatten() {
                if p["state"] == "running" {
                    set_part(p, "failed");
                    p["error"] = json!("Catbot restarted while this part ran.");
                }
            }
            d["state"] = json!("interrupted");
            d["note"] = json!("Catbot restarted while this ran. Continue runs what's left.");
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_tag_comes_out_of_the_reply() {
        let (rest, found) = take("Splitting it.\n<burst>{\"task\":\"t\"}</burst>\nDone.");
        assert_eq!((rest.as_str(), found.as_deref()), ("Splitting it.\n\nDone.", Some("{\"task\":\"t\"}")));
        assert_eq!(take("no tag").1, None);
    }

    #[test]
    fn a_proposal_is_read_and_checked() {
        let (task, _, items) = read(r#"{"task":"Check  the\nbeds","instructions":"Be brief.","items":["North bed"," ","South bed"]}"#).unwrap();
        assert_eq!((task.as_str(), items.len()), ("Check the beds", 2));
        assert!(read(r#"{"task":"x","items":["one"]}"#).unwrap_err().contains("at least two"));
        let many: Vec<String> = (0..30).map(|n| format!("item {n}")).collect();
        assert!(read(&json!({ "task": "x", "items": many }).to_string()).unwrap_err().contains("at most 25"));
        assert_eq!(estimate_minutes(4), 6);
        assert!(command("/burst check every page").unwrap().contains("check every page"));
        assert!(command("/bursting").is_none());
    }
}
