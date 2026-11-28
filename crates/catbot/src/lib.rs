//! Catbot: a self-hosted team of AI bots in a chat app, served by a Rust server on a home box.

pub mod acp;
pub mod agents;
pub mod api;
pub mod approval;
pub mod ask;
pub mod bots;
pub mod catbot_tools;
pub mod chat;
pub mod chatgpt;
pub mod checkpoints;
pub mod claude;
pub mod codex;
pub mod console;
pub mod engines;
pub mod http;
pub mod login;
pub mod memory;
pub mod pi;
pub mod proxy;
pub mod record;
pub mod routes;
pub mod server;
pub mod store;
mod stream_lines;
pub mod titles;
pub mod tools;
