//! `catbot ask`: one message to Claude, nothing saved. For trying things out; `catbot chat`
//! is the same with a bot's instructions, folder and memory.

use std::path::PathBuf;

use crate::{
    claude::{self, PermissionMode, Session, Turn},
    console::{self, Paint},
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct Options {
    pub prompt: String,
    pub resume: Option<String>,
    pub model: Option<String>,
    pub mode: PermissionMode,
    pub cwd: PathBuf,
    pub system: Option<String>,
    pub cli: String,
    pub inherit_user_config: bool,
    pub json: bool,
    pub no_approvals: bool,
}

/// Runs the turn. Returns whether it finished well.
pub async fn run(options: Options) -> Result<bool, BoxError> {
    let paint = Paint::stderr();
    if options.mode == PermissionMode::BypassPermissions {
        eprintln!("{}", paint.red("! bypass-permissions: the agent runs every command without asking."));
    }
    let approvals_on = !options.no_approvals && console::can_ask(options.json);
    let turn = Turn {
        prompt: options.prompt,
        session: match options.resume {
            Some(id) => Session::Resume(id),
            None => Session::New(uuid::Uuid::new_v4().to_string()),
        },
        model: options.model,
        permission_mode: options.mode,
        cwd: options.cwd,
        system_prompt: options.system,
        cli: vec![options.cli],
        isolated: !options.inherit_user_config,
        approval_helper: approvals_on.then(helper).transpose()?,
        extra_dirs: Vec::new(),
    };
    let mut running = claude::start(&turn).await?;
    let transcript = console::drive(&mut running, approvals_on, options.json).await?;
    if let (true, Some(id), false) = (transcript.ok(), &transcript.session_id, options.json) {
        eprintln!("{}", paint.dim(&format!("  continue: catbot ask --resume {id} \"…\"")));
    }
    Ok(transcript.ok())
}

/// This program, run as the approval helper.
pub fn helper() -> std::io::Result<Vec<String>> {
    Ok(vec![std::env::current_exe()?.to_string_lossy().into_owned()])
}
