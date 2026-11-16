//! The AIs a bot can run on.
//!
//! - **Claude**, through Claude Code (`claude.rs`): the default, and the only one that needs no key.
//! - **Chat APIs** that speak OpenAI's chat-completions protocol (`api.rs`): OpenAI, xAI's Grok,
//!   Groq, Mistral, Cerebras, DeepSeek, OpenRouter, MiniMax, and models on this computer through
//!   Ollama or LM Studio. They need a key (saved in Settings, kept in Catbot's private store, or
//!   given in the environment), except the local ones. Catbot gives them its own tools.
//!
//! The list follows OpenMausBot's built-in drivers (server/drivers/, Apache-2.0); later stages add
//! its agent programs (Gemini CLI, Copilot, Cursor…) and Codex.

use serde::Serialize;

/// The engine every bot started on.
pub const CLAUDE: &str = "claude";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Claude Code, run as a program.
    Claude,
    /// A chat API over HTTP, OpenAI-style.
    Api,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Engine {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    /// The API's address (with `/v1` or its like); `/chat/completions` and `/models` go after it.
    pub url: &'static str,
    /// The environment variable that can hold its key instead of Settings.
    pub key_env: &'static str,
    /// Whether it needs a key at all (models on this computer don't).
    pub needs_key: bool,
    /// Where to get a key.
    pub key_page: &'static str,
    /// Models offered before Catbot has asked the API for its own list.
    pub models: &'static [(&'static str, &'static str)],
    pub default_model: &'static str,
}

const fn api(
    id: &'static str,
    name: &'static str,
    url: &'static str,
    key_env: &'static str,
    key_page: &'static str,
    models: &'static [(&'static str, &'static str)],
) -> Engine {
    Engine { id, name, kind: Kind::Api, url, key_env, needs_key: true, key_page, models, default_model: models[0].0 }
}

const fn local(id: &'static str, name: &'static str, url: &'static str, key_page: &'static str) -> Engine {
    Engine { id, name, kind: Kind::Api, url, key_env: "", needs_key: false, key_page, models: &[], default_model: "" }
}

/// Every engine, in the order the picker shows them.
pub const ALL: &[Engine] = &[
    Engine {
        id: CLAUDE,
        name: "Claude",
        kind: Kind::Claude,
        url: "",
        key_env: "",
        needs_key: false,
        key_page: "https://claude.com/product/claude-code",
        models: &[("", "Default"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")],
        default_model: "",
    },
    api(
        "openai",
        "OpenAI",
        "https://api.openai.com/v1",
        "OPENAI_API_KEY",
        "https://platform.openai.com/api-keys",
        &[("gpt-5", "GPT-5"), ("gpt-5-mini", "GPT-5 mini")],
    ),
    api(
        "xai",
        "Grok (xAI)",
        "https://api.x.ai/v1",
        "XAI_API_KEY",
        "https://console.x.ai",
        &[("grok-4.7", "Grok 4.7"), ("grok-4", "Grok 4"), ("grok-4-fast", "Grok 4 Fast"), ("grok-3-mini", "Grok 3 Mini")],
    ),
    api(
        "groq",
        "Groq",
        "https://api.groq.com/openai/v1",
        "GROQ_API_KEY",
        "https://console.groq.com/keys",
        &[("openai/gpt-oss-120b", "GPT OSS 120B"), ("openai/gpt-oss-20b", "GPT OSS 20B")],
    ),
    api(
        "mistral",
        "Mistral",
        "https://api.mistral.ai/v1",
        "MISTRAL_API_KEY",
        "https://console.mistral.ai/api-keys",
        &[("mistral-large-latest", "Mistral Large"), ("mistral-small-latest", "Mistral Small")],
    ),
    api(
        "cerebras",
        "Cerebras",
        "https://api.cerebras.ai/v1",
        "CEREBRAS_API_KEY",
        "https://cloud.cerebras.ai",
        &[("gpt-oss-120b", "GPT OSS 120B")],
    ),
    api(
        "deepseek",
        "DeepSeek",
        "https://api.deepseek.com/v1",
        "DEEPSEEK_API_KEY",
        "https://platform.deepseek.com/api_keys",
        &[("deepseek-chat", "DeepSeek Chat"), ("deepseek-reasoner", "DeepSeek Reasoner")],
    ),
    api(
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        "OPENROUTER_API_KEY",
        "https://openrouter.ai/keys",
        &[("meta-llama/llama-3.3-70b-instruct", "Llama 3.3 70B")],
    ),
    api(
        "minimax",
        "MiniMax",
        "https://api.minimax.io/v1",
        "MINIMAX_API_KEY",
        "https://platform.minimax.io",
        &[("MiniMax-M3", "MiniMax M3"), ("MiniMax-M2.7", "MiniMax M2.7")],
    ),
    local("ollama", "Ollama (this computer)", "http://127.0.0.1:11434/v1", "https://ollama.com/download"),
    local("lmstudio", "LM Studio (this computer)", "http://127.0.0.1:1234/v1", "https://lmstudio.ai"),
];

pub fn find(id: &str) -> Option<&'static Engine> {
    ALL.iter().find(|engine| engine.id == id)
}

/// How an engine is reached: its address and key, from Settings or the environment. None when
/// it needs a key and has none.
#[derive(Debug, Clone)]
pub struct Access {
    pub url: String,
    pub key: Option<String>,
    /// Where the key came from, for the settings screen: "settings" or "environment".
    pub key_from: Option<&'static str>,
}

/// The address and key for an engine, given what Settings holds for it.
pub fn access(engine: &Engine, saved: Option<&crate::store::EngineKey>) -> Option<Access> {
    let url = saved.and_then(|s| s.url.clone()).filter(|u| !u.trim().is_empty()).unwrap_or_else(|| engine.url.to_string());
    let url = url.trim().trim_end_matches('/').to_string();
    let from_env = (!engine.key_env.is_empty()).then(|| std::env::var(engine.key_env).ok()).flatten().filter(|k| !k.trim().is_empty());
    let (key, key_from) = match (saved.map(|s| s.key.clone()).filter(|k| !k.is_empty()), from_env) {
        (Some(key), _) => (Some(key), Some("settings")),
        (None, Some(key)) => (Some(key), Some("environment")),
        (None, None) => (None, None),
    };
    if engine.needs_key && key.is_none() {
        return None;
    }
    Some(Access { url, key, key_from })
}

/// The last four characters of a key, for showing which one is saved.
pub fn key_hint(key: &str) -> String {
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

/// What's wrong with a key someone pasted, if anything obvious.
pub fn check_key(key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Paste the key itself.".into());
    }
    if key.starts_with("http://") || key.starts_with("https://") {
        return Err("That looks like a web address. Paste the secret key itself.".into());
    }
    if key.chars().any(char::is_whitespace) || key.len() > 4096 {
        return Err("That doesn't look like a key: it has spaces in it, or it's far too long.".into());
    }
    Ok(key.to_string())
}

/// An address someone typed for an engine: http(s), no name, password or query in it.
pub fn check_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or("Use an http or https address.")?;
    if rest.is_empty() || rest.contains(['@', '?', '#', ' ']) {
        return Err("Use a plain address, without a name, password or query in it.".into());
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_a_unique_id_and_claude_comes_first() {
        let mut ids: Vec<&str> = ALL.iter().map(|e| e.id).collect();
        assert_eq!(ids[0], CLAUDE);
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
        assert!(find("xai").is_some_and(|e| e.kind == Kind::Api && e.needs_key));
        assert!(find("ollama").is_some_and(|e| !e.needs_key));
    }

    #[test]
    fn a_saved_key_wins_and_a_missing_one_means_no_access() {
        let openai = find("openai").unwrap();
        let saved = crate::store::EngineKey { key: "sk-saved".into(), url: None, updated_at: String::new() };
        let got = access(openai, Some(&saved)).unwrap();
        assert_eq!((got.key.as_deref(), got.key_from, got.url.as_str()), (Some("sk-saved"), Some("settings"), "https://api.openai.com/v1"));
        let lm = find("lmstudio").unwrap();
        assert!(access(lm, None).is_some_and(|a| a.key.is_none()));
        let moved = crate::store::EngineKey { key: String::new(), url: Some("http://10.0.0.5:1234/v1/".into()), updated_at: String::new() };
        assert_eq!(access(lm, Some(&moved)).unwrap().url, "http://10.0.0.5:1234/v1");
    }

    #[test]
    fn keys_and_addresses_are_checked() {
        assert_eq!(key_hint("sk-abcdef1234"), "…1234");
        assert!(check_key("https://x").is_err());
        assert!(check_key("two words").is_err());
        assert_eq!(check_key("  xai-123  ").unwrap(), "xai-123");
        assert_eq!(check_url("http://127.0.0.1:11434/v1/").unwrap(), "http://127.0.0.1:11434/v1");
        assert!(check_url("ftp://x").is_err());
        assert!(check_url("https://me:pw@x").is_err());
    }
}
