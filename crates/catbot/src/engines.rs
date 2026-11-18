//! The AIs a bot can run on.
//!
//! - **Claude**, through Claude Code (`claude.rs`): the default, and the only one that needs no key.
//! - **Chat APIs** that speak OpenAI's chat-completions protocol (`api.rs`): OpenAI, xAI's Grok,
//!   Groq, Mistral, Cerebras, DeepSeek, OpenRouter, MiniMax, and models on this computer through
//!   Ollama or LM Studio. They need a key (saved in Settings, kept in Catbot's private store, or
//!   given in the environment), except the local ones. Catbot gives them its own tools.
//! - **Codex** with a ChatGPT plan (`codex.rs`), over its app-server, signed in once for Catbot and
//!   the person's own Codex together.
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
/// Codex on a ChatGPT plan.
pub const CODEX: &str = "codex";
pub const PI: &str = "pi";
pub const MUSE: &str = "muse-code";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Claude Code, run as a program.
    Claude,
    /// A chat API over HTTP, OpenAI-style.
    Api,
    /// An agent program on the computer, over the Agent Client Protocol.
    Agent,
    /// Codex, over its own app-server protocol, on a ChatGPT plan.
    Codex,
    /// Pi, over its own RPC mode, with Catbot's extension asking for approvals.
    Pi,
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
    /// A program Catbot downloads itself (from the ACP registry's entry), when there's no
    /// installer to run.
    #[serde(skip)]
    pub download: Option<&'static Download>,
}

/// A program Catbot downloads and keeps in its own folder (`<catbot home>/agents/<id>/<version>`).
#[derive(Debug)]
pub struct Download {
    pub version: &'static str,
    /// Roughly the room it takes, for the Install button.
    pub size: &'static str,
    pub assets: &'static [Asset],
}

/// One platform's file: where from, its SHA-256, and the program in it.
#[derive(Debug)]
pub struct Asset {
    /// As the ACP registry names them: `linux-x86_64`, `windows-aarch64`, `darwin-aarch64`…
    pub platform: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// The program, inside the archive (or the file itself when it isn't one).
    pub cmd: &'static str,
    pub zip: bool,
}

/// Google Antigravity's ACP server, as the ACP registry lists it
/// (agentclientprotocol/registry, antigravity-acp/agent.json, read 2026-11-18).
pub const ANTIGRAVITY_DOWNLOAD: Download = Download {
    version: "1.5.0",
    size: "about 1 GB",
    assets: &[
        Asset { platform: "linux-x86_64", url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.5.0-linux-x86_64.zip", sha256: "36f373e8948edf1727b39c7d62b5722d6a4474bdfd72b04ad3d6def571808c9f", cmd: "agy_acp_server.par", zip: true },
        Asset { platform: "linux-aarch64", url: "https://dl.google.com/agy-extensions/releases/linux/agy-acp-server-1.5.0-linux-arm64.zip", sha256: "1e184f439477b54138e94440056b26a78c0a8075a8fd2ffda637ec31267c6459", cmd: "agy_acp_server.par", zip: true },
        Asset { platform: "windows-x86_64", url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.5.0-windows-x86_64.zip", sha256: "6163f6b59b381bae419652dbae167bc8cda0ef29d665c0ba8af3d8c7fddd8867", cmd: "agy_acp_server.exe", zip: true },
        Asset { platform: "windows-aarch64", url: "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.5.0-windows-arm64.zip", sha256: "340092b8af6dafc71eb15f4a8ec54b4d1f0bce16d1fa4d163b562c50b905f8f4", cmd: "agy_acp_server.exe", zip: true },
        Asset { platform: "darwin-aarch64", url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.5.0-darwin-arm64.zip", sha256: "c82ecb4ef476bfc35cbe5b2745cdddef873b99c9f5bd7fd11e5ca1fc1667a3fc", cmd: "agy_acp_server.par", zip: true },
        Asset { platform: "darwin-x86_64", url: "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.5.0-darwin-x86_64.zip", sha256: "86a94e33b25d4a38ff30526ad844546860aa9ed98698498d9e87d30eb794426e", cmd: "agy_acp_server.par", zip: true },
    ],
};

/// Meta's Muse Code, as the ACP registry lists it (muse-code/agent.json, read 2026-11-18): one
/// program per platform, not an archive.
pub const MUSE_DOWNLOAD: Download = Download {
    version: "1.4.14",
    size: "about 420 MB",
    assets: &[
        Asset { platform: "linux-x86_64", url: "https://lookaside.facebook.com/lookaside/muse/download/?channel=muse&version=1.4.14-R7309.1&file=muse-x86-linux", sha256: "0809aac7cb58d18f5022b6d05e02aa1974683c0ec430e5f04d25b60ed17de2da", cmd: "muse", zip: false },
        Asset { platform: "linux-aarch64", url: "https://lookaside.facebook.com/lookaside/muse/download/?channel=muse&version=1.4.14-R7309.1&file=muse-aarch64-linux", sha256: "3dc39b8fa26f9963cc2038f2966c0932ed5a39f0f9efb39cc0266a261cbbcf76", cmd: "muse", zip: false },
        Asset { platform: "windows-x86_64", url: "https://lookaside.facebook.com/lookaside/muse/download/?channel=muse&version=1.4.14-R7309.1&file=muse-x86-windows.exe", sha256: "20d18c426cc26e9e4b3cc07008ec07ec97e12e466a2be8cd2797b2e0e2018382", cmd: "muse.exe", zip: false },
        Asset { platform: "windows-aarch64", url: "https://lookaside.facebook.com/lookaside/muse/download/?channel=muse&version=1.4.14-R7309.1&file=muse-aarch64-windows.exe", sha256: "64cef76cb210de993d31478ee582ef4639bbb212df8912b74f45d40431e66e82", cmd: "muse.exe", zip: false },
        Asset { platform: "darwin-aarch64", url: "https://lookaside.facebook.com/lookaside/muse/download/?channel=muse&version=1.4.14-R7309.1&file=muse-aarch64-macos", sha256: "1c96114408acbeade9a8fb79e3b4425e5b64d50425938568aef01bdc04ce2e03", cmd: "muse", zip: false },
        Asset { platform: "darwin-x86_64", url: "https://lookaside.facebook.com/lookaside/muse/download/?channel=muse&version=1.4.14-R7309.1&file=muse-x86-macos", sha256: "6da5a12d3298afba45afec929fc385b516084ed28e9504ae29354dbcdfc4089d", cmd: "muse", zip: false },
    ],
};

/// The engines that sign in through Catbot, with the method to ask for: Antigravity through
/// Google's page, Muse Code with a code typed at Meta's.
pub fn agent_sign_in(engine: &Engine) -> Option<&'static str> {
    match engine.id {
        "antigravity" => Some("oauth-personal"),
        MUSE => Some("muse-login"),
        _ => None,
    }
}

/// Whose account an engine that signs in through Catbot signs in with, for its button.
pub fn sign_in_with(engine: &Engine) -> Option<&'static str> {
    match engine.id {
        "antigravity" => Some("Google"),
        MUSE => Some("Meta"),
        _ => None,
    }
}

/// Notes that Catbot signed an engine in (Muse may keep its sign-in in the system's keychain,
/// where Catbot can't look).
pub fn note_signed_in(engine: &Engine) {
    if engine.id == MUSE
        && let Some(dir) = agents_dir().map(|d| d.join(MUSE))
    {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("signed-in"), "Catbot signed Muse Code in here.\n");
    }
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
    download: None,
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

fn antigravity_args(_: PermissionMode, _: &str) -> Vec<String> {
    if cfg!(target_os = "linux") { plain(&["--uid="]) } else { Vec::new() }
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
    Engine {
        id: CODEX,
        name: "Codex (ChatGPT)",
        kind: Kind::Codex,
        key_page: "https://chatgpt.com/codex",
        models: &[("", "Default")],
        program: "codex",
        install: "npm install -g @openai/codex",
        sign_in: "codex login",
        ..BLANK
    },
    Engine {
        id: PI,
        name: "Pi",
        kind: Kind::Pi,
        key_page: "https://github.com/earendil-works/pi",
        models: &[("", "Default")],
        program: "pi",
        install: "npm install -g @earendil-works/pi-coding-agent",
        sign_in: "pi auth login <provider>",
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
    // Google Antigravity: Catbot downloads its ACP server and signs it in with Google itself.
    // Before release 1.5.1 it offers its own Gemini models to every client and others only to
    // editors it knows, so Catbot (which doesn't pretend to be one) gets the Gemini ones.
    Engine {
        id: "antigravity",
        name: "Antigravity (Google)",
        kind: Kind::Agent,
        args: antigravity_args,
        install: "Install it from this bot's settings",
        sign_in: "Sign in with Google in this bot's settings",
        key_page: "https://antigravity.google/docs/ide/extensions",
        download: Some(&ANTIGRAVITY_DOWNLOAD),
        ..BLANK
    },
    // Meta's Muse Code: Catbot downloads it and signs it in with a code at Meta's page. The
    // sign-in is the person's own Muse's too (`muse login`), so it's shared both ways.
    Engine {
        id: MUSE,
        name: "Muse Code (Meta)",
        kind: Kind::Agent,
        program: "muse",
        args: acp_word,
        install: "Install it from this bot's settings",
        sign_in: "Sign in with Meta in this bot's settings (or run `muse login` on this computer)",
        key_page: "https://dev.meta.ai/docs/muse-code",
        download: Some(&MUSE_DOWNLOAD),
        ..BLANK
    },
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
    find_program(engine.program).or_else(|| downloaded(engine))
}

static AGENTS_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Where Catbot keeps the programs it downloads: `<catbot home>/agents`.
pub fn set_agents_dir(dir: std::path::PathBuf) {
    *AGENTS_DIR.lock().expect("agents dir lock") = Some(dir);
}

/// `CATBOT_AGENTS_DIR` if set, else `<catbot home>/agents`.
pub fn agents_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("CATBOT_AGENTS_DIR").filter(|d| !d.is_empty()) {
        return Some(dir.into());
    }
    AGENTS_DIR.lock().expect("agents dir lock").clone()
}

/// This computer, as the ACP registry names platforms.
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// This computer's file for a downloadable engine.
pub fn asset(engine: &Engine) -> Option<&'static Asset> {
    let platform = platform();
    engine.download?.assets.iter().find(|a| a.platform == platform)
}

/// Where a downloaded engine's version lives.
pub fn download_folder(engine: &Engine) -> Option<std::path::PathBuf> {
    Some(agents_dir()?.join(engine.id).join(engine.download?.version))
}

/// A downloaded engine's program, if it's there.
pub fn downloaded(engine: &Engine) -> Option<std::path::PathBuf> {
    let program = download_folder(engine)?.join(asset(engine)?.cmd);
    program.is_file().then_some(program)
}

/// What an engine's program needs in its environment.
pub fn agent_env(engine: &Engine, program: &std::path::Path) -> Vec<(String, String)> {
    match engine.id {
        "antigravity" => {
            let Some(profile) = agents_dir().map(|d| d.join("antigravity").join("profile")) else { return Vec::new() };
            let acp = profile.join("antigravity-acp");
            let _ = std::fs::create_dir_all(&acp);
            let settings = acp.join("settings.json");
            if !settings.is_file() {
                let _ = std::fs::write(&settings, "{\"auth\":{\"type\":\"oauth-personal\"}}\n");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700));
            }
            let harness = if cfg!(windows) { "localharness_external.exe" } else { "localharness_external" };
            let mut env = vec![
                ("GEMINI_HOME".to_string(), profile.display().to_string()),
                ("AGY_ACP_FORCE_FILE_STORAGE".to_string(), "1".to_string()),
                ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
            ];
            if let Some(dir) = program.parent() {
                env.push(("ANTIGRAVITY_HARNESS_PATH".to_string(), dir.join(harness).display().to_string()));
            }
            // On Windows it unpacks itself into TEMP each start: one folder of its own.
            if cfg!(windows) {
                let temp = profile.parent().map(|p| p.join("tmp")).unwrap_or_else(std::env::temp_dir);
                let _ = std::fs::create_dir_all(&temp);
                env.push(("TEMP".to_string(), temp.display().to_string()));
                env.push(("TMP".to_string(), temp.display().to_string()));
            }
            env
        }
        // Its ACP mode is still marked experimental.
        MUSE => vec![("MUSE_EXPERIMENTAL_ACP".to_string(), "on".to_string())],
        _ => Vec::new(),
    }
}

/// Whether an engine that signs in through Catbot is signed in: None when Catbot can't tell.
pub fn agent_signed_in(engine: &Engine) -> Option<bool> {
    match engine.id {
        "antigravity" => Some(agents_dir().is_some_and(|d| d.join("antigravity/profile/antigravity-acp/acp_token.json").metadata().is_ok_and(|m| m.is_file() && m.len() > 0))),
        // A key, Muse's own file, or Catbot's note; else Catbot can't tell (the keychain).
        MUSE => {
            let config = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(|| crate::store::user_home().join(".config"));
            let keyed = std::env::var_os("META_API_KEY").is_some_and(|k| !k.is_empty());
            let noted = agents_dir().is_some_and(|d| d.join(MUSE).join("signed-in").is_file());
            (keyed || noted || config.join("muse/auth.json").is_file()).then_some(true)
        }
        _ => None,
    }
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
