//! The review check (stage 32.14, after OpenMausBot 0.1.176): per bot, off by default, its
//! answers get another look before they're done.
//!
//! Moo's choices (2026-11-27): who reviews is "the asking bot"; and "if it asks me directly put
//! something in my inbox".
//! - **Asked by the person**: after the bot answers a message of theirs, a review card goes in
//!   the chat (and so in the Inbox): Looks good, or Needs work, which the bot hears with the
//!   person's next message.
//! - **Asked by a bot in a team chat** (`handoffs.rs`): the asking bot checks the answer there; if
//!   it objects, the one that answered gets one more go to fix it, then it goes back.
//! - **Asked by a bot in a direct chat**: that chat only closes when both say DONE, so the asking
//!   bot's DONE is its review.

use serde_json::json;

use super::{App, turns};
use crate::store::{Bot, Role, Store};

fn key(bot_id: &str) -> String {
    format!("review:{bot_id}")
}

pub fn is_on(store: &Store, bot_id: &str) -> bool {
    store.setting(&key(bot_id)).ok().flatten().and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn set(store: &Store, bot_id: &str, on: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    store.set_setting(&key(bot_id), &json!(on))
}

/// What the asking bot is told when it checks an answer, and the word that passes it.
pub const APPROVED: &str = "APPROVED";

pub fn check_cue(name: &str, answer: &str) -> String {
    format!(
        "[{name}]: {answer}\n\n(You asked {name} for this, and {name} has the review check on: look it over before it goes \
         back. If it's right, end your reply with a line saying just {APPROVED}. If not, say plainly what's wrong or missing, \
         so {name} can fix it. Don't hand off from here.)"
    )
}

/// Whether a reviewer passed the answer: a last line saying just APPROVED.
pub fn passed(review: &str) -> bool {
    review.trim_end().lines().last().is_some_and(|l| l.trim().trim_matches(|c: char| !c.is_alphanumeric()).eq_ignore_ascii_case(APPROVED))
}

/// After a turn the person started: the review card, if the bot has the check on and answered a
/// message of the person's.
pub fn ask_the_person(app: &App, bot: &Bot, thread: &str, request_id: i64) {
    if !app.with_store(|store| Ok(is_on(store, &bot.id))).unwrap_or(false) {
        return;
    }
    let asked_by_person = app.with_store(|store| store.message(request_id)).flatten().is_some_and(|m| m.role == Role::Person);
    let latest = app.with_store(|store| store.messages(thread, 1)).unwrap_or_default().into_iter().last();
    let Some(answer) = latest.filter(|m| m.role == Role::Bot && !m.body.trim().is_empty()) else { return };
    if !asked_by_person {
        return;
    }
    let first: String = answer.body.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = if first.chars().count() > 200 { format!("{}…", first.chars().take(199).collect::<String>()) } else { first };
    let detail = json!({
        "kind": "team-change",
        "askedBy": bot.name,
        "heading": format!("Review {}'s answer", bot.name),
        "title": first,
        "lines": [],
        "action": "review",
        "target": bot.id,
        "answer": answer.id,
        "body": {},
        "approveLabel": "Looks good",
        "declineLabel": "Needs work",
        "state": "asking",
    });
    turns::save_and_send(app, thread, Role::Step, &format!("Review {}'s answer", bot.name), Some(&detail));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_passes_on_a_line_of_its_own() {
        assert!(passed("Checked the numbers.\n\nAPPROVED"));
        assert!(passed("**Approved.**"));
        assert!(!passed("Not approved: the dates are wrong."));
        assert!(!passed("APPROVED\n\nbut one thing: the shed."));
    }
}
