//! Catbot.
//!
//! Today this is one tool with two jobs:
//!
//! * `catbot record` sits between the interface and the reference server, passes every
//!   request through untouched, and writes what it saw to a JSON Lines file.
//! * `catbot routes` reads those files and prints the list of routes the interface used:
//!   the to-do list for the Rust server.
//! * `catbot ask` sends one message to Claude through Catbot's own driver and prints the reply.
//!
//! The recorder is the seed of the real server: routes move into Rust one at a time,
//! and whatever Rust does not answer yet keeps falling through to the reference server.

use std::{net::SocketAddr, path::PathBuf};

use catbot::{approval, ask, claude::PermissionMode, proxy, routes};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "catbot", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pass every request through to the reference server and record it.
    Record {
        /// Address to listen on. Start the interface with OMB_PORT set to this port.
        #[arg(long, default_value = "127.0.0.1:8800")]
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
    /// Send one message to Claude through Catbot's driver and print the reply.
    Ask {
        /// What to say.
        prompt: String,
        /// Continue an earlier conversation (the session id printed at the end of a reply).
        #[arg(long)]
        resume: Option<String>,
        /// Model, e.g. sonnet, opus, haiku. Defaults to Claude Code's own default.
        #[arg(long)]
        model: Option<String>,
        /// What the agent may do without asking. Anything needing a yes is refused for now.
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
    /// The approval helper Claude Code starts for a turn (not for running by hand).
    #[command(hide = true)]
    ApprovalMcp { socket: PathBuf },
    /// Summarize recordings into the list of routes the interface used.
    Routes {
        /// Recording files or folders. Defaults to every recording in the traffic folder.
        paths: Vec<PathBuf>,
    },
}

/// Recordings live next to the repo, never inside it: ~/Projects/catbot-traffic.
fn default_traffic_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Projects").join("catbot-traffic")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match Cli::parse().command {
        Command::Record { listen, upstream, out, body_cap } => {
            proxy::run(listen, &upstream, &out, body_cap).await
        }
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
        Command::ApprovalMcp { socket } => Ok(approval::serve_mcp(&socket).await?),
        Command::Routes { paths } => {
            let paths = if paths.is_empty() { vec![default_traffic_dir()] } else { paths };
            routes::run(&paths)
        }
    }
}
