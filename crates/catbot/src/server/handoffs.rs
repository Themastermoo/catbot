//! Hand-offs (stage 15.2): a bot that ends its reply with `@Name` brings that bot in next, in a
//! channel or in its own chat.
//!
//! Moo's choice (2026-11-16): bots hand work to each other freely. So two bots can't bounce a
//! message back and forth for ever, a chain stops at a depth (3) and a number of bot replies to
//! one message (6), both changeable in Settings, which can also make hand-offs unlimited. Each
//! hand-off shows as a line in the chat ("Scout handed this to Kiwi.").
//!
//! In a channel the bots handed to are its members, working as they do there. In a bot's own chat
//! any other bot can be brought in: it works in its own folder, shown the chat so far, and its
//! reply goes in that chat under its own name. Handed back, the chat's bot carries on in its own
//! session, told what was said.

use std::collections::VecDeque;
use std::sync::Arc;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, channels, turns};
use crate::store::{Bot, Role};

/// The limits on a chain of hand-offs, kept in the settings as `handoffs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rules {
    /// No limits at all: only the person (Stop) or a spend limit ends a chain.
    pub unlimited: bool,
    /// How many hand-offs deep a chain may go: one bot to another is 1.
    pub depth: u32,
    /// How many bot replies one message may bring (the ones the person asked for always answer).
    pub replies: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { unlimited: false, depth: 3, replies: 6 }
    }
}

pub fn rules(app: &App) -> Rules {
    app.with_store(|store| store.setting("handoffs")).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// `GET /api/handoffs` and `PUT /api/handoffs` (`{ unlimited, depth, replies }`).
pub fn route(app: &App, body: Option<Value>) -> Answer {
    let Some(body) = body else {
        return Ok((StatusCode::OK, json!({ "handoffs": rules(app) })));
    };
    let mut wanted: Rules = serde_json::from_value(body).map_err(|e| bad(format!("hand-off limits: {e}")))?;
    wanted.depth = wanted.depth.clamp(1, 50);
    wanted.replies = wanted.replies.clamp(1, 200);
    app.try_store(|store| store.set_setting("handoffs", &json!(wanted))).map_err(super::fail)?;
    app.hub.send(json!({ "kind": "handoffs", "handoffs": wanted }));
    Ok((StatusCode::OK, json!({ "handoffs": wanted })))
}

/// The bots a reply hands to: those it @names in its last paragraph, in the order named, never
/// the bot that wrote it.
pub fn named(reply: &str, candidates: &[Bot], writer: &str) -> Vec<Bot> {
    let last = reply.trim_end().rsplit("\n\n").next().unwrap_or_default();
    let others: Vec<Bot> = candidates.iter().filter(|b| b.id != writer).cloned().collect();
    channels::named(last, &others)
}

/// One bot's turn in a chain.
pub struct Turn {
    pub bot: Bot,
    /// How many hand-offs led here (0: the person asked for it).
    pub depth: u32,
    /// Who handed it on, and what they said.
    pub from: Option<(String, String)>,
}

/// Where a chain runs.
pub enum Place {
    Channel(String),
    /// A bot's own chat (its id).
    Chat(String),
}

/// What a bot in its own chat is told about the others it can bring in.
pub fn roster_note(app: &App, bot: &Bot) -> String {
    // The Chief is told about the whole team in its own note (`chief.rs`).
    if app.with_store(|store| Ok(super::chief::is_chief_in(store, &bot.id))).unwrap_or(false) {
        return String::new();
    }
    let others: Vec<String> = app
        .with_store(|store| store.bots())
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.id != bot.id)
        .map(|b| {
            let about: String = b.instructions.split_whitespace().take(14).collect::<Vec<_>>().join(" ");
            if about.is_empty() { b.name } else { format!("{} ({about})", b.name) }
        })
        .collect();
    if others.is_empty() {
        return String::new();
    }
    format!(
        "\n\nOther bots work alongside you: {}. To hand part of the work to one, end your reply with @Name and what \
         you need from them; they'll answer in this chat, and you'll be told what they said. Only hand off \
         when another bot is better placed to help.",
        others.join("; ")
    )
}

/// What other bots said in a bot's chat since its own last reply, for a session that didn't see it.
pub fn untold_note(app: &App, thread: &str, owner: &str) -> String {
    let messages = app.with_store(|store| store.messages(thread, 60)).unwrap_or_default();
    let names: std::collections::HashMap<String, String> = app.with_store(|store| store.bots()).unwrap_or_default().into_iter().map(|b| (b.id, b.name)).collect();
    let mut said = Vec::new();
    for m in messages.iter().rev() {
        if m.role != Role::Bot || m.body.trim().is_empty() {
            continue;
        }
        let speaker = m.detail.as_ref().and_then(|d| d["botId"].as_str()).unwrap_or(owner);
        if speaker == owner {
            break;
        }
        let name = names.get(speaker).cloned().unwrap_or_else(|| "Another bot".into());
        said.push(format!("[{name}]: {}", m.body.trim()));
    }
    if said.is_empty() {
        return String::new();
    }
    said.reverse();
    format!("\n\n(Said in this chat since your last reply:\n{})", said.join("\n\n"))
}

/// A bot brought into another bot's chat: in its own folder, told whose chat it's in.
fn as_guest(mut bot: Bot, owner: &str, from: &str) -> Bot {
    bot.instructions.push_str(&format!(
        "\n\n{from} has brought you into the person's chat with {owner} to help. Everything said there so far is \
         shown to you, others' words marked with their names. Answer as yourself, briefly, without repeating \
         what's been said. To hand back or on, end your reply with @Name and what you need."
    ));
    bot
}

/// The bots a reply in a bot's own chat hands on to.
pub fn next_after(app: &App, owner: &Bot, reply: &str) -> Vec<Bot> {
    let everyone = app.with_store(|store| store.bots()).unwrap_or_default();
    named(reply, &everyone, &owner.id)
}

/// A reply in a bot's own chat that hands on (to `next`, marked busy already): the chain that
/// follows it.
pub async fn after_reply(app: &Arc<App>, owner: &Bot, thread: &str, request_id: i64, reply: &str, next: Vec<Bot>) {
    let from = (owner.name.clone(), reply.to_string());
    let mut queue = VecDeque::new();
    let rules = rules(app);
    for bot in next {
        hand(app, thread, &rules, &mut queue, 1, 1, Turn { bot, depth: 1, from: Some(from.clone()) });
    }
    if !queue.is_empty() {
        follow(app, thread, thread, request_id, queue, Place::Chat(owner.id.clone()), 1, 1).await;
    }
    app.set_channel_busy(thread, false);
    app.announce_bot(&owner.id);
}

/// Queues a hand-off if the limits allow, noting it in the chat either way.
fn hand(app: &App, thread: &str, rules: &Rules, queue: &mut VecDeque<Turn>, done: u32, allowed: u32, turn: Turn) {
    if queue.iter().any(|t| t.bot.id == turn.bot.id) {
        return; // Already going to answer.
    }
    let from = turn.from.as_ref().map(|(name, _)| name.clone()).unwrap_or_default();
    if !rules.unlimited && turn.depth > rules.depth {
        let note = format!("{from} asked {} to help, but hand-offs go only {} deep (Settings › Hand-offs).", turn.bot.name, rules.depth);
        turns::save_and_send(app, thread, Role::Note, &note, None);
        return;
    }
    if !rules.unlimited && done + queue.len() as u32 >= allowed.max(rules.replies) {
        let note = format!(
            "{from} asked {} to help, but one message brings at most {} bot replies (Settings › Hand-offs).",
            turn.bot.name, rules.replies
        );
        turns::save_and_send(app, thread, Role::Note, &note, None);
        return;
    }
    turns::save_and_send(app, thread, Role::Note, &format!("{from} handed this to {}.", turn.bot.name), None);
    // The Team map draws who hands work to whom.
    if let Some(giver) = app.with_store(|store| store.bot(&from)).flatten() {
        super::departments::handed(app, &giver.id, &turn.bot.id, thread);
    }
    queue.push_back(turn);
}

/// Runs a chain's turns one after another, queueing the hand-offs each reply makes. `key` is
/// what marks it running (a channel's id, or the chat's): Stop clears it. `asked` is how many the
/// person asked for, who always answer; `done` how many have answered already.
#[allow(clippy::too_many_arguments)]
pub async fn follow(app: &Arc<App>, key: &str, thread: &str, request_id: i64, mut queue: VecDeque<Turn>, place: Place, asked: u32, mut done: u32) {
    let rules = rules(app);
    let mut first = true;
    while let Some(turn) = queue.pop_front() {
        if !app.channel_busy(key) {
            break; // Stopped.
        }
        let Some(conversation) = app.with_store(|store| store.conversation(thread)).flatten() else { break };
        // The bot as it is now, not as it was when it was handed the work: a change made while
        // it waited (Full access, say) holds for its turn.
        let now = app.with_store(|store| store.bot_by_id(&turn.bot.id)).flatten().unwrap_or_else(|| turn.bot.clone());
        // A backup takes the turn if the bot's AI is at a limit; a bot that can't go is noted.
        let bot = match super::backups::choose(app, &now) {
            Ok(Some((stand_in, why))) => {
                turns::save_and_send(app, thread, Role::Note, &format!("{}: {why}", turn.bot.name), None);
                stand_in
            }
            Ok(None) => now,
            Err(why) => {
                turns::save_and_send(app, thread, Role::Note, &format!("{} can't answer: {why}", turn.bot.name), None);
                continue;
            }
        };
        let from = turn.from.as_ref().map(|(name, _)| name.as_str());
        let (runner, guest, cue, candidates) = match &place {
            Place::Channel(id) => {
                let Some(channel) = app.with_store(|store| store.channel(id)).flatten() else { break };
                let members: Vec<Bot> = channel.members.iter().filter_map(|m| app.with_store(|store| store.bot_by_id(m)).flatten()).collect();
                let cue = match from {
                    Some(from) => format!("{from} handed this to you. It's your turn now, as {}.", bot.name),
                    None if first => format!("It's your turn to answer the person's last message, as {}.", bot.name),
                    None => format!("It's your turn now, as {}.", bot.name),
                };
                let mut runner = channels::in_channel(bot, &channel, &members);
                if super::team_chat::is_team_chat(app, &channel.id) {
                    runner.instructions.push_str(&super::team_chat::note(&channel));
                }
                (runner, false, cue, members)
            }
            Place::Chat(owner_id) => {
                let everyone = app.with_store(|store| store.bots()).unwrap_or_default();
                let Some(owner) = everyone.iter().find(|b| &b.id == owner_id).cloned() else { break };
                let (from_name, from_said) = turn.from.clone().unwrap_or_default();
                if bot.id == owner.id {
                    // Handed back: the chat's own bot, in its own session, told what was said.
                    let mut bot = bot;
                    bot.instructions.push_str(&roster_note(app, &bot));
                    let cue = format!("[{from_name}]: {}\n\n({from_name} handed this back to you.)", from_said.trim());
                    (bot, false, cue, everyone)
                } else {
                    let cue = format!("{from_name} handed this to you. It's your turn now, as {}.", bot.name);
                    (as_guest(bot, &owner.name, &from_name), true, cue, everyone)
                }
            }
        };
        first = false;
        let reply = match turns::run_through(app, &runner, conversation, request_id, &cue, guest).await {
            Ok(reply) => reply,
            Err(err) => {
                eprintln!("catbot: {} couldn't answer: {err}", turn.bot.name);
                None
            }
        };
        done += 1;
        super::departments::hand_done(app, &turn.bot.id, thread);
        app.announce_bot(match &place {
            Place::Channel(id) => id,
            Place::Chat(owner) => owner,
        });
        let Some(reply) = reply else { continue };
        for next in named(&reply, &candidates, &turn.bot.id) {
            let handed = Turn { bot: next, depth: turn.depth + 1, from: Some((turn.bot.name.clone(), reply.clone())) };
            hand(app, thread, &rules, &mut queue, done, asked, handed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_paragraph_hands_on() {
        let bot = |id: &str, name: &str| Bot {
            id: id.into(),
            name: name.into(),
            instructions: String::new(),
            engine: "claude".into(),
            model: None,
            mode: crate::claude::PermissionMode::Default,
            codex: false,
            folder: std::path::PathBuf::new(),
            created_at: String::new(),
            mark: crate::store::Mark::CatHead,
            color: None,
            effort: None,
        };
        let all = [bot("a", "Scout"), bot("b", "Kiwi"), bot("c", "Moss")];
        assert!(named("As @Kiwi said, beans.\n\nThat's all.", &all, "a").is_empty());
        let next: Vec<String> = named("Beans.\n\n@Moss and @Kiwi, can you check the soil?", &all, "a").into_iter().map(|b| b.name).collect();
        assert_eq!(next, ["Moss", "Kiwi"]);
        assert!(named("Over to me, @Scout", &all, "a").is_empty(), "never hands to itself");
    }

    #[test]
    fn the_limits_have_defaults_and_can_be_lifted() {
        assert_eq!(Rules::default(), Rules { unlimited: false, depth: 3, replies: 6 });
        let lifted: Rules = serde_json::from_value(json!({ "unlimited": true })).unwrap();
        assert!(lifted.unlimited && lifted.depth == 3);
    }
}
