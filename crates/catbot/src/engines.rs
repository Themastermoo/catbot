//! The AIs a bot can run on.
//!
//! - **Claude**, through Claude Code (`claude.rs`): the default, and the only one that needs no key.
//! - **Chat APIs** that speak OpenAI's chat-completions protocol (`api.rs`): OpenAI, xAI's Grok,
//!   Groq, Mistral, Cerebras, DeepSeek, OpenRouter, MiniMax, and models on this computer through
//!   Ollama or LM Studio. They need a key (saved in Settings, kept in Catbot's private store, or
//!   given in the environment), except the local ones. Catbot gives them its own tools.
//! - **Agent programs** that speak the Agent Client Protocol (`acp.rs`): Grok CLI, Gemini CLI,
//!   GitHub Copilot, Cursor, OpenCode, Qwen Code, Kimi, Droid, Cline, Kiro, Hermes, DeepSeek
//!   Harness, OMP and Command Code. Each must be installed and signed in on the computer, with its
//!   own login and plan; it uses its own tools and asks through Catbot.
//!
//! The list follows OpenMausBot's built-in drivers (server/drivers/, Apache-2.0); later stages add
//! its agent programs (Gemini CLI, Copilot, Cursor…) and Codex.

use serde::Serialize;

use crate::claude::PermissionMode;

/// The engine every bot started on.
pub const CLAUDE: &str = "claude";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Claude Code, run as a program.
    Claude,
    /// A chat API over HTTP, OpenAI-style.
    Api,
    /// An agent program on the computer, over the Agent Client Protocol.
    Agent,
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
    /// An agent's program, and its arguments for an approval level and model ("" for its own).
    #[serde(skip)]
    pub program: &'static str,
    #[serde(skip)]
    pub args: fn(PermissionMode, &str) -> Vec<String>,
    /// How to install the agent, and sign it in.
    pub install: &'static str,
    pub sign_in: &'static str,
    /// Sign-in methods to ask an agent for, in order, when it offers them.
    #[serde(skip)]
    pub auth: &'static [&'static str],
}

fn no_args(_: PermissionMode, _: &str) -> Vec<String> {
    Vec::new()
}

const BLANK: Engine = Engine {
    id: "",
    name: "",
    kind: Kind::Api,
    url: "",
    key_env: "",
    needs_key: false,
    key_page: "",
    models: &[],
    default_model: "",
    program: "",
    args: no_args,
    install: "",
    sign_in: "",
    auth: &[],
};

const fn api(
    id: &'static str,
    name: &'static str,
    url: &'static str,
    key_env: &'static str,
    key_page: &'static str,
    models: &'static [(&'static str, &'static str)],
) -> Engine {
    Engine { id, name, kind: Kind::Api, url, key_env, needs_key: true, key_page, models, default_model: models[0].0, ..BLANK }
}

#[allow(clippy::too_many_arguments)]
const fn agent(
    id: &'static str,
    name: &'static str,
    program: &'static str,
    args: fn(PermissionMode, &str) -> Vec<String>,
    install: &'static str,
    sign_in: &'static str,
    key_page: &'static str,
    auth: &'static [&'static str],
) -> Engine {
    Engine { id, name, kind: Kind::Agent, program, args, install, sign_in, key_page, auth, ..BLANK }
}

/// "-m model" when a model is picked.
fn model_flag(flag: &str, model: &str) -> Vec<String> {
    if model.is_empty() { Vec::new() } else { vec![flag.to_string(), model.to_string()] }
}

fn plain(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn grok_args(mode: PermissionMode, model: &str) -> Vec<String> {
    let level = match mode {
        PermissionMode::BypassPermissions => "bypassPermissions",
        PermissionMode::Auto => "auto",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Default | PermissionMode::Plan => "default",
    };
    [plain(&["--permission-mode", level, "agent"]), model_flag("-m", model), plain(&["stdio"])].concat()
}

fn gemini_args(mode: PermissionMode, model: &str) -> Vec<String> {
    let approval = match mode {
        PermissionMode::BypassPermissions => plain(&["--yolo"]),
        PermissionMode::AcceptEdits => plain(&["--approval-mode", "auto_edit"]),
        _ => Vec::new(),
    };
    [plain(&["--acp"]), approval, model_flag("-m", model)].concat()
}

fn copilot_args(_: PermissionMode, _: &str) -> Vec<String> {
    plain(&["--acp", "--stdio"])
}

fn cursor_args(mode: PermissionMode, model: &str) -> Vec<String> {
    let approval = match mode {
        PermissionMode::BypassPermissions => plain(&["--force"]),
        PermissionMode::Auto => plain(&["--auto-review"]),
        _ => Vec::new(),
    };
    [approval, model_flag("--model", model), plain(&["acp"])].concat()
}

fn acp_word(_: PermissionMode, _: &str) -> Vec<String> {
    plain(&["acp"])
}

fn qwen_args(mode: PermissionMode, _: &str) -> Vec<String> {
    let approval = match mode {
        PermissionMode::BypassPermissions => plain(&["--yolo"]),
        PermissionMode::Auto => plain(&["--approval-mode", "auto"]),
        PermissionMode::AcceptEdits => plain(&["--approval-mode", "auto-edit"]),
        _ => Vec::new(),
    };
    [plain(&["--acp"]), approval].concat()
}

fn kimi_args(_: PermissionMode, model: &str) -> Vec<String> {
    [model_flag("-m", model), plain(&["acp"])].concat()
}

fn droid_args(_: PermissionMode, _: &str) -> Vec<String> {
    plain(&["exec", "-o", "acp"])
}

fn cline_args(_: PermissionMode, _: &str) -> Vec<String> {
    plain(&["--acp", "--auto-approve", "false"])
}

fn kiro_args(_: PermissionMode, _: &str) -> Vec<String> {
    plain(&["acp", "--agent-engine=v3", "--auth-method=cli"])
}

fn deepseek_args(_: PermissionMode, _: &str) -> Vec<String> {
    plain(&["--profile", "acp"])
}

const fn local(id: &'static str, name: &'static str, url: &'static str, key_page: &'static str) -> Engine {
    Engine { id, name, kind: Kind::Api, url, needs_key: false, key_page, ..BLANK }
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
        ..BLANK
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
    // Agent programs (after OpenMausBot's server/drivers/acp/, Apache-2.0).
    agent("grok-cli", "Grok CLI", "grok", grok_args, "curl -fsSL https://x.ai/cli/install.sh | bash", "grok login", "https://x.ai/cli", &["cached_token", "grok.com"]),
    agent("gemini", "Gemini CLI", "gemini", gemini_args, "npm install -g @google/gemini-cli", "gemini (then follow its sign-in)", "https://github.com/google-gemini/gemini-cli", &["gemini-api-key", "oauth-personal", "vertex-ai"]),
    agent("copilot", "GitHub Copilot", "copilot", copilot_args, "npm install -g @github/copilot", "copilot login", "https://docs.github.com/en/copilot/reference/acp-server", &[]),
    agent("cursor", "Cursor", "cursor-agent", cursor_args, "curl https://cursor.com/install -fsS | bash", "cursor-agent login", "https://cursor.com/docs/cli/installation", &[]),
    agent("opencode", "OpenCode", "opencode", acp_word, "npm install -g opencode-ai", "opencode auth login", "https://opencode.ai/docs/", &[]),
    agent("qwen", "Qwen Code", "qwen", qwen_args, "npm install -g @qwen-code/qwen-code", "qwen (then follow its sign-in)", "https://qwenlm.github.io/qwen-code-docs/", &[]),
    agent("kimi", "Kimi Code", "kimi", kimi_args, "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash", "kimi login", "https://moonshotai.github.io/kimi-code/", &[]),
    agent("droid", "Droid (Factory)", "droid", droid_args, "curl -fsSL https://app.factory.ai/cli | sh", "droid", "https://docs.factory.ai/droid-cli/quickstart", &[]),
    agent("cline", "Cline", "cline", cline_args, "npm install -g cline", "cline auth", "https://docs.cline.bot/usage/acp", &[]),
    agent("kiro", "Kiro", "kiro-cli", kiro_args, "curl -fsSL https://cli.kiro.dev/install | bash", "kiro-cli login", "https://kiro.dev/docs/getting-started/installation/", &["aws-builder-id", "aws-iam-identity-center", "kiro-login"]),
    agent("hermes", "Hermes", "hermes", acp_word, "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash", "hermes setup", "https://hermes-agent.nousresearch.com/docs/", &[]),
    agent("deepseek-harness", "DeepSeek Harness", "dsh", deepseek_args, "npm install -g @deepseek-ai/dsh", "dsh", "https://github.com/deepseek-ai/deepseek-harness", &[]),
    agent("omp", "OMP", "omp", acp_word, "npm install -g @oh-my-pi/pi-coding-agent", "omp login", "https://omp.sh", &["agent"]),
    agent("command-code", "Command Code", "command-code", acp_word, "npm install -g command-code", "cmd login", "https://commandcode.ai/docs/acp", &[]),
];

/// An agent's program: `CATBOT_AGENT_<ID>` if set (`CATBOT_AGENT_GROK_CLI=/opt/grok/bin/grok`),
/// else found by name.
pub fn agent_program(engine: &Engine) -> Option<std::path::PathBuf> {
    let variable = format!("CATBOT_AGENT_{}", engine.id.to_uppercase().replace('-', "_"));
    if let Some(path) = std::env::var_os(&variable).filter(|p| !p.is_empty()) {
        let path = std::path::PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    find_program(engine.program)
}

/// Where an agent's program is: on the PATH, or in the folders installers usually use.
pub fn find_program(program: &str) -> Option<std::path::PathBuf> {
    if program.is_empty() {
        return None;
    }
    let given = std::path::Path::new(program);
    if given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    let home = crate::store::user_home();
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend([home.join(".local/bin"), home.join(".npm-global/bin"), home.join("bin"), home.join(".bun/bin"), home.join(".cargo/bin")]);
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(std::path::PathBuf::from(appdata).join("npm"));
    }
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"].iter().map(|ext| format!("{program}.{ext}")).collect()
    } else {
        vec![program.to_string()]
    };
    dirs.iter().flat_map(|dir| names.iter().map(move |name| dir.join(name))).find(|path| path.is_file())
}

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
        assert!(find("grok-cli").is_some_and(|e| e.kind == Kind::Agent && e.program == "grok"));
    }

    #[test]
    fn agents_get_their_flags() {
        let grok = find("grok-cli").unwrap();
        assert_eq!((grok.args)(PermissionMode::AcceptEdits, "grok-4"), ["--permission-mode", "acceptEdits", "agent", "-m", "grok-4", "stdio"]);
        assert_eq!((grok.args)(PermissionMode::Default, ""), ["--permission-mode", "default", "agent", "stdio"]);
        assert_eq!((find("gemini").unwrap().args)(PermissionMode::BypassPermissions, ""), ["--acp", "--yolo"]);
        assert_eq!((find("cursor").unwrap().args)(PermissionMode::Default, "gpt-5"), ["--model", "gpt-5", "acp"]);
        assert_eq!((find("copilot").unwrap().args)(PermissionMode::Default, ""), ["--acp", "--stdio"]);
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
