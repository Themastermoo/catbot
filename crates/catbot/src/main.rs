//! Catbot: a team of AI bots you chat with, run from a home Linux box.
//!
//! * `catbot bot …` makes and manages bots: name, instructions, model, approvals, Codex access.
//! * `catbot chat` talks to a bot; conversations continue where they left off and are saved.
//! * `catbot history` shows a bot's current conversation.
//! * `catbot ask` sends one unsaved message to Claude, for trying things out.
//! * `catbot serve` is the server the interface talks to.
//! * `catbot record` and `catbot routes` record the reference interface's traffic and list
//!   its routes: the to-do list for the coming Rust server.

use std::{net::SocketAddr, path::PathBuf};

use catbot::{
    approval, ask, bots, chat,
    claude::PermissionMode,
    proxy, routes, server,
    store::{self, NewBot, Store},
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "catbot", version, about = "A team of AI bots you chat with")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Make and manage bots.
    Bot {
        #[command(subcommand)]
        action: BotAction,
    },
    /// Talk to a bot. Continues its last conversation and saves what's said.
    Chat {
        /// Which bot.
        bot: String,
        /// What to say.
        message: String,
        /// Start a fresh conversation.
        #[arg(long)]
        new: bool,
        /// Print every event as a JSON line.
        #[arg(long)]
        json: bool,
        /// Refuse anything that needs approval instead of asking.
        #[arg(long)]
        no_approvals: bool,
        /// The Claude Code program to run.
        #[arg(long, default_value = "claude")]
        cli: String,
    },
    /// Show a bot's current conversation.
    History {
        bot: String,
        /// How many messages, counting back from the latest.
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Send one message to Claude, nothing saved. For trying things out.
    Ask {
        /// What to say.
        prompt: String,
        /// Continue an earlier conversation (the session id printed at the end of a reply).
        #[arg(long)]
        resume: Option<String>,
        /// Model, e.g. sonnet, opus, haiku. Defaults to Claude Code's own default.
        #[arg(long)]
        model: Option<String>,
        /// What the agent may do without asking.
        #[arg(long, value_enum, default_value_t = PermissionMode::Default)]
        mode: PermissionMode,
        /// Folder the agent works in. Defaults to the current folder.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Extra instructions: the bot's personality.
        #[arg(long)]
        system: Option<String>,
        /// The Claude Code program to run.
        #[arg(long, default_value = "claude")]
        cli: String,
        /// Let the bot use this machine's own Claude settings, CLAUDE.md and MCP servers.
        #[arg(long)]
        inherit_user_config: bool,
        /// Print every event as a JSON line instead of the readable reply.
        #[arg(long)]
        json: bool,
        /// Refuse anything that needs approval instead of asking.
        #[arg(long)]
        no_approvals: bool,
    },
    /// Run the server the interface talks to. Catbot's bots are answered here; everything
    /// else goes on to the reference server.
    Serve {
        /// Address to listen on. Start the interface with OMB_PORT set to this port.
        #[arg(long, default_value = "127.0.0.1:8788")]
        listen: SocketAddr,
        /// The reference server.
        #[arg(long, default_value = "http://127.0.0.1:8799")]
        upstream: String,
        /// The Claude Code program to run.
        #[arg(long, default_value = "claude")]
        cli: String,
        /// Also record what goes to the reference server, into ~/Projects/catbot-traffic.
        #[arg(long)]
        record: bool,
        /// Catbot's interface: the `public` folder of its dx build, served at `/`.
        #[arg(long, default_value = "target/dx/catbot-ui/release/web/public")]
        ui: PathBuf,
    },
    /// Pass every request through to the reference server and record it.
    Record {
        /// Address to listen on. Start the interface with OMB_PORT set to this port.
        #[arg(long, default_value = "127.0.0.1:8788")]
        listen: SocketAddr,
        /// The reference server.
        #[arg(long, default_value = "http://127.0.0.1:8799")]
        upstream: String,
        /// Folder for recordings (one file per run).
        #[arg(long, default_value_os_t = default_traffic_dir())]
        out: PathBuf,
        /// Most bytes of any one body to keep in the recording.
        #[arg(long, default_value_t = 16 * 1024)]
        body_cap: usize,
    },
    /// Summarize recordings into the list of routes the interface used.
    Routes {
        /// Recording files or folders. Defaults to every recording in the traffic folder.
        paths: Vec<PathBuf>,
    },
    /// The approval helper Claude Code starts for a turn (not for running by hand).
    #[command(hide = true)]
    ApprovalMcp { socket: PathBuf },
}

#[derive(Subcommand)]
enum BotAction {
    /// Make a bot.
    New {
        name: String,
        /// Who the bot is and how it should work.
        #[arg(long, default_value = "")]
        instructions: String,
        /// Read the instructions from a file instead.
        #[arg(long, conflicts_with = "instructions")]
        instructions_file: Option<PathBuf>,
        /// Model, e.g. sonnet, opus, haiku. Defaults to Claude Code's own default.
        #[arg(long)]
        model: Option<String>,
        /// What it may do without asking.
        #[arg(long, value_enum, default_value_t = PermissionMode::Default)]
        mode: PermissionMode,
        /// Keep this bot out of the Codex vault.
        #[arg(long)]
        no_codex: bool,
    },
    /// List your bots.
    List,
    /// Show one bot's settings.
    Show { name: String },
    /// Change a bot's settings.
    Edit {
        name: String,
        #[arg(long)]
        instructions: Option<String>,
        #[arg(long, conflicts_with = "instructions")]
        instructions_file: Option<PathBuf>,
        /// A model name, or "" for Claude Code's default.
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_enum)]
        mode: Option<PermissionMode>,
        /// Whether it may use the Codex vault.
        #[arg(long)]
        codex: Option<bool>,
    },
    /// Delete a bot and its conversations (its folder stays).
    Delete {
        name: String,
        /// Don't ask to confirm.
        #[arg(long)]
        yes: bool,
    },
}

/// Recordings live next to the repo, never inside it: ~/Projects/catbot-traffic.
fn default_traffic_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Projects").join("catbot-traffic")
}

fn read_instructions(text: Option<String>, file: Option<PathBuf>) -> Result<Option<String>, BoxError> {
    match file {
        Some(path) => Ok(Some(std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?)),
        None => Ok(text),
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("catbot: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), BoxError> {
    match Cli::parse().command {
        Command::Bot { action } => {
            let store = Store::open(&store::default_home())?;
            match action {
                BotAction::New { name, instructions, instructions_file, model, mode, no_codex } => {
                    let instructions = read_instructions(Some(instructions), instructions_file)?.unwrap_or_default();
                    bots::create(&store, NewBot { name, instructions, model, mode, codex: !no_codex })
                }
                BotAction::List => bots::list(&store),
                BotAction::Show { name } => bots::show(&store, &name),
                BotAction::Edit { name, instructions, instructions_file, model, mode, codex } => {
                    let instructions = read_instructions(instructions, instructions_file)?;
                    bots::edit(&store, &name, bots::Changes { instructions, model, mode, codex })
                }
                BotAction::Delete { name, yes } => bots::delete(&store, &name, yes),
            }
        }
        Command::Chat { bot, message, new, json, no_approvals, cli } => {
            let store = Store::open(&store::default_home())?;
            let options = chat::Options { bot, message, new, json, no_approvals, cli: vec![cli], codex: store::default_codex() };
            if !chat::chat(&store, options).await? {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::History { bot, limit, json } => chat::history(&Store::open(&store::default_home())?, &bot, limit, json),
        Command::Ask { prompt, resume, model, mode, cwd, system, cli, inherit_user_config, json, no_approvals } => {
            let cwd = match cwd {
                Some(cwd) => cwd,
                None => std::env::current_dir()?,
            };
            let options = ask::Options { prompt, resume, model, mode, cwd, system, cli, inherit_user_config, json, no_approvals };
            if !ask::run(options).await? {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::Serve { listen, upstream, cli, record, ui } => {
            server::serve(server::Options {
                listen,
                upstream,
                home: store::default_home(),
                codex: store::default_codex(),
                cli: vec![cli],
                record: record.then(default_traffic_dir),
                ui: Some(ui),
            })
            .await
        }
        Command::Record { listen, upstream, out, body_cap } => proxy::run(listen, &upstream, &out, body_cap).await,
        Command::Routes { paths } => {
            let paths = if paths.is_empty() { vec![default_traffic_dir()] } else { paths };
            routes::run(&paths)
        }
        Command::ApprovalMcp { socket } => Ok(approval::serve_mcp(&socket).await?),
    }
}
