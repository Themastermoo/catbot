//! A bot's turn on a chat API (OpenAI's chat-completions protocol, which OpenAI, xAI, Groq,
//! Mistral, Cerebras, DeepSeek, OpenRouter, MiniMax, Ollama and LM Studio all speak).
//!
//! Catbot keeps the conversation (the API keeps nothing), sends it with the bot's instructions
//! and its tools (`tools.rs`), streams the reply, and when the model calls tools, runs them
//! (asking first where the bot's approval level says so) and sends the results back, until the
//! model answers without calling any. The turn comes out as the same events a Claude turn
//! does, so the rest of Catbot treats both alike.
//!
//! Adapted in outline from OpenMausBot's server/drivers/openai-chat.ts (Apache-2.0).

use std::collections::BTreeMap;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::approval::{Answerer, Decision};
use crate::claude::{Event, Outcome, Running, Tokens};
use crate::engines::{Access, Engine};
use crate::tools::{self, Workspace};

/// How many rounds of tool calls a turn may take before Catbot stops it.
const MOST_ROUNDS: usize = 40;

/// A turn on a chat API.
pub struct ApiTurn {
    pub engine: &'static Engine,
    pub access: Access,
    pub model: String,
    /// The bot's standing instructions.
    pub system: String,
    /// The conversation so far, oldest first, ending with the person's new message:
    /// (true for the person, false for the bot, text).
    pub history: Vec<(bool, String)>,
    /// Pictures attached to the person's newest message: (type, bytes), shown to the model.
    pub pictures: Vec<(String, Vec<u8>)>,
    pub workspace: Workspace,
    /// `reasoning_effort`, only where the model takes it (`engines::effort_levels`).
    pub effort: Option<String>,
}

/// What a bot on a chat API is told about working, after its own instructions.
pub fn tool_guide(folder: &std::path::Path) -> String {
    format!(
        "\n\nYou work through Catbot's tools: read_file, list_files and search_files to look, write_file and edit_file to \
         change files, and run_command for shell commands (run in {folder}). Use them rather than guessing about files. \
         Changes and commands may need the person's approval; if one is refused, don't try another way to do the same \
         thing: say what you wanted and why. Keep replies to the person plain and short.",
        folder = folder.display()
    )
}

/// Starts the turn; its events arrive on the returned `Running`.
pub fn start(turn: ApiTurn) -> Running {
    let (tx, events) = mpsc::channel(256);
    let answers = Answerer::local();
    let task = tokio::spawn(run(turn, tx, answers.clone()));
    Running::in_process(events, task, answers)
}

async fn run(turn: ApiTurn, tx: mpsc::Sender<Event>, answers: Answerer) {
    let started = std::time::Instant::now();
    let _ = tx.send(Event::Started { session_id: String::new(), model: Some(turn.model.clone()) }).await;
    let mut used = Used::default();
    let (ok, result) = match converse(&turn, &tx, &answers, &mut used).await {
        Ok(reply) => (true, Some(reply)),
        Err(message) => {
            let _ = tx.send(Event::Error { message: message.clone() }).await;
            (false, Some(message))
        }
    };
    let outcome = Outcome {
        ok,
        result,
        duration_ms: Some(started.elapsed().as_millis() as u64),
        cost_usd: used.cost,
        tokens: (used.tokens != Tokens::default()).then_some(used.tokens),
        ..Outcome::default()
    };
    let _ = tx.send(Event::Finished(outcome)).await;
}

/// What a turn used, round by round: what the API reported, or Catbot's guess where it didn't.
#[derive(Debug, Default)]
struct Used {
    tokens: Tokens,
    /// Money, where the API says (OpenRouter does).
    cost: Option<f64>,
    /// The API turned `stream_options` away: don't ask again this turn.
    no_usage_option: bool,
}

async fn converse(turn: &ApiTurn, tx: &mpsc::Sender<Event>, answers: &Answerer, used: &mut Used) -> Result<String, String> {
    if turn.model.trim().is_empty() {
        return Err(format!("Pick a model for this bot: {} has no default one.", turn.engine.name));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let mut messages = vec![json!({ "role": "system", "content": turn.system })];
    for (person, text) in &turn.history {
        messages.push(json!({ "role": if *person { "user" } else { "assistant" }, "content": text }));
    }
    if !turn.pictures.is_empty()
        && let Some(last) = messages.iter_mut().rev().find(|m| m["role"] == "user")
    {
        use base64::Engine as _;
        let mut parts = vec![json!({ "type": "text", "text": last["content"] })];
        for (mime, bytes) in &turn.pictures {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            parts.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data}") } }));
        }
        last["content"] = json!(parts);
    }
    let mut use_tools = true;
    let mut reply = String::new();
    for _ in 0..MOST_ROUNDS {
        let mut attempt = ask(&client, turn, &messages, use_tools, used, tx).await;
        if let Err(Failure::NoUsageOption) = attempt {
            used.no_usage_option = true;
            attempt = ask(&client, turn, &messages, use_tools, used, tx).await;
        }
        let round = match attempt {
            Err(Failure::NoTools) if use_tools => {
                // Some models (often small local ones) can't take tools: carry on without them.
                use_tools = false;
                ask(&client, turn, &messages, false, used, tx).await
            }
            other => other,
        }
        .map_err(|f| f.to_string())?;
        match round.usage {
            Some((tokens, cost)) => {
                used.tokens.add(tokens);
                if let Some(cost) = cost {
                    *used.cost.get_or_insert(0.0) += cost;
                }
            }
            None => {
                let sent: usize = messages.iter().map(|m| m.to_string().len()).sum();
                let written = round.text.len() + round.calls.iter().map(|c| c.arguments.len() + c.name.len()).sum::<usize>();
                used.tokens.add(Tokens::guess(sent, written));
            }
        }
        if !round.text.trim().is_empty() {
            let _ = tx.send(Event::Text { text: round.text.clone() }).await;
            if !reply.is_empty() {
                reply.push_str("\n\n");
            }
            reply.push_str(round.text.trim_end());
        }
        let mut said = json!({ "role": "assistant", "content": round.text });
        if round.calls.is_empty() {
            return Ok(reply);
        }
        said["tool_calls"] = Value::Array(
            round.calls.iter().map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } })).collect(),
        );
        messages.push(said);
        for call in &round.calls {
            let output = use_tool(turn, call, tx, answers).await;
            messages.push(json!({ "role": "tool", "tool_call_id": call.id, "content": output }));
        }
    }
    Err(format!("The bot took more than {MOST_ROUNDS} rounds of tool calls, so Catbot stopped it here."))
}

/// Runs one tool call, with its step and any approval shown, and returns what the model reads.
async fn use_tool(turn: &ApiTurn, call: &Call, tx: &mpsc::Sender<Event>, answers: &Answerer) -> String {
    let args: Value = serde_json::from_str(if call.arguments.trim().is_empty() { "{}" } else { &call.arguments }).unwrap_or(Value::Null);
    let planned = match tools::plan(&turn.workspace, &call.name, &args) {
        Ok(planned) => planned,
        Err(problem) => {
            let _ = tx.send(Event::ToolStarted { id: call.id.clone(), name: call.name.clone(), input: args.clone() }).await;
            let _ = tx.send(Event::ToolFinished { id: call.id.clone(), ok: false, output: problem.clone() }).await;
            return format!("Error: {problem}");
        }
    };
    let _ = tx.send(Event::ToolStarted { id: call.id.clone(), name: planned.shown.to_string(), input: planned.input.clone() }).await;
    if planned.ask {
        let question = format!("ask-{}", uuid::Uuid::new_v4());
        let waiting = answers.expect(&question);
        let _ = tx.send(Event::ApprovalRequested { id: question, tool: planned.shown.to_string(), input: planned.input.clone() }).await;
        if let Decision::Deny(why) = waiting.await {
            let _ = tx.send(Event::ToolFinished { id: call.id.clone(), ok: false, output: "Refused".into() }).await;
            return format!("Refused: {why}");
        }
    }
    match tools::run(&planned, &turn.workspace).await {
        Ok(output) => {
            let _ = tx.send(Event::ToolFinished { id: call.id.clone(), ok: true, output: output.clone() }).await;
            output
        }
        Err(problem) => {
            let _ = tx.send(Event::ToolFinished { id: call.id.clone(), ok: false, output: problem.clone() }).await;
            format!("Error: {problem}")
        }
    }
}

/// One answer from the model: its text, and the tools it wants called.
#[derive(Debug, Default, PartialEq)]
struct Round {
    text: String,
    calls: Vec<Call>,
    /// The tokens (and any cost) the API reported for this round.
    usage: Option<(Tokens, Option<f64>)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Call {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug)]
enum Failure {
    /// The model or API doesn't take tools.
    NoTools,
    /// The API doesn't take `stream_options` (asking for the usage at the end).
    NoUsageOption,
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::NoTools => f.write_str("This model can't use tools."),
            Failure::NoUsageOption => f.write_str("This API doesn't report usage."),
            Failure::Other(text) => f.write_str(text),
        }
    }
}

/// Sends the conversation and streams the answer, passing its text on as it comes.
async fn ask(client: &reqwest::Client, turn: &ApiTurn, messages: &[Value], with_tools: bool, used: &Used, tx: &mpsc::Sender<Event>) -> Result<Round, Failure> {
    let mut body = json!({ "model": turn.model, "messages": messages, "stream": true });
    if let Some(effort) = turn.effort.as_deref().and_then(crate::engines::effort_sent) {
        body["reasoning_effort"] = json!(effort);
    }
    if !used.no_usage_option {
        // The usage comes in a last chunk of the stream.
        body["stream_options"] = json!({ "include_usage": true });
    }
    if with_tools {
        body["tools"] = tools::definitions(&turn.workspace);
    }
    let mut request = client.post(format!("{}/chat/completions", turn.access.url)).json(&body);
    if let Some(key) = &turn.access.key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|e| Failure::Other(reach_error(turn.engine, &turn.access.url, &e)))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let said = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).or(v["message"].as_str()).map(str::to_string))
            .unwrap_or_else(|| text.chars().take(300).collect());
        if status.as_u16() == 400 && !used.no_usage_option && said.to_lowercase().contains("stream_options") {
            return Err(Failure::NoUsageOption);
        }
        if with_tools && status.as_u16() == 400 && said.to_lowercase().contains("tool") {
            return Err(Failure::NoTools);
        }
        return Err(Failure::Other(match status.as_u16() {
            401 | 403 => format!("{} turned the key away ({status}): {said} Check the key in the bot's settings.", turn.engine.name),
            404 => format!("{} doesn't know the model {} ({said}). Pick another in the bot's settings.", turn.engine.name, turn.model),
            429 => format!("{} says to slow down or that the account is out of credit: {said}", turn.engine.name),
            _ => format!("{} answered {status}: {said}", turn.engine.name),
        }));
    }
    let mut round = Round::default();
    let mut calls: BTreeMap<u64, Call> = BTreeMap::new();
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Failure::Other(format!("{} stopped answering partway: {e}", turn.engine.name)))?;
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if apply_line(line.trim(), &mut round, &mut calls, tx).await {
                round.calls = calls.into_values().collect();
                return Ok(round);
            }
        }
    }
    round.calls = calls.into_values().collect();
    Ok(round)
}

/// Takes one line of the event stream into the round. True at `[DONE]`.
async fn apply_line(line: &str, round: &mut Round, calls: &mut BTreeMap<u64, Call>, tx: &mpsc::Sender<Event>) -> bool {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else { return false };
    if data == "[DONE]" {
        return true;
    }
    let Ok(chunk) = serde_json::from_str::<Value>(data) else { return false };
    if let Some(usage) = chunk["usage"].as_object().filter(|u| u.contains_key("prompt_tokens") || u.contains_key("completion_tokens")) {
        let cached = usage.get("prompt_tokens_details").and_then(|d| d["cached_tokens"].as_u64()).unwrap_or(0);
        let prompt = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
        let tokens = Tokens {
            input: prompt.saturating_sub(cached),
            output: usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_read: cached,
            ..Tokens::default()
        };
        round.usage = Some((tokens, usage.get("cost").and_then(Value::as_f64)));
    }
    let delta = &chunk["choices"][0]["delta"];
    if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
        round.text.push_str(text);
        let _ = tx.send(Event::TextDelta { text: text.to_string() }).await;
    }
    if let Some(text) = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()).filter(|t| !t.is_empty()) {
        let _ = tx.send(Event::ThinkingDelta { text: text.to_string() }).await;
    }
    if let Some(list) = delta["tool_calls"].as_array() {
        for (n, piece) in list.iter().enumerate() {
            let index = piece["index"].as_u64().unwrap_or(n as u64);
            let call = calls.entry(index).or_default();
            if let Some(id) = piece["id"].as_str().filter(|id| !id.is_empty()) {
                call.id = id.to_string();
            }
            if let Some(name) = piece["function"]["name"].as_str() {
                call.name.push_str(name);
            }
            if let Some(arguments) = piece["function"]["arguments"].as_str() {
                call.arguments.push_str(arguments);
            }
            if call.id.is_empty() {
                call.id = format!("call-{index}");
            }
        }
    }
    false
}

fn reach_error(engine: &Engine, url: &str, err: &reqwest::Error) -> String {
    if engine.needs_key {
        format!("Couldn't reach {} at {url}: {err}", engine.name)
    } else {
        format!("Couldn't reach {} at {url}. Is it running on this computer? ({err})", engine.name)
    }
}

/// The models an API offers (`GET /models`), for the picker. Chat models only, as best it can tell.
pub async fn models(access: &Access) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    let mut request = client.get(format!("{}/models", access.url));
    if let Some(key) = &access.key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(body["error"]["message"].as_str().unwrap_or("the API refused").to_string());
    }
    let not_chat = ["embed", "whisper", "tts", "dall-e", "image", "moderation", "audio", "transcribe", "realtime", "guard", "rerank"];
    let mut ids: Vec<String> = body["data"]
        .as_array()
        .map(|list| list.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    ids.retain(|id| !not_chat.iter().any(|word| id.to_lowercase().contains(word)));
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streamed_text_and_tool_calls_are_put_together() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut round = Round::default();
        let mut calls = BTreeMap::new();
        let lines = [
            r#"data: {"choices":[{"delta":{"content":"Let me "}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"look."}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_","arguments":"{\"pa"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"th\":\"a.md\"}"}}]}}]}"#,
            ": keep-alive",
            "",
        ];
        for line in lines {
            assert!(!apply_line(line, &mut round, &mut calls, &tx).await);
        }
        assert!(apply_line("data: [DONE]", &mut round, &mut calls, &tx).await);
        assert_eq!(round.text, "Let me look.");
        let call = calls.get(&0).unwrap();
        assert_eq!((call.id.as_str(), call.name.as_str(), call.arguments.as_str()), ("c1", "read_file", r#"{"path":"a.md"}"#));
        drop(tx);
        let mut deltas = String::new();
        while let Some(event) = rx.recv().await {
            if let Event::TextDelta { text } = event {
                deltas.push_str(&text);
            }
        }
        assert_eq!(deltas, "Let me look.");
    }
}
