//! `ovis` — the OVIS command line and terminal UI.
//!
//! The CLI is an **API client**. It speaks the OVIS HTTP API and holds no
//! database or OpenSearch credentials; `ovis-core`'s
//! `db`/`search` modules are the backend's data plane and are deliberately
//! unreachable from here. Only `ovis server start`, which embeds the backend in
//! this same binary, and `ovis server setup-onyx-key`, which is server-side
//! setup, touch anything else.

pub mod api;
pub mod cli;
pub mod commands;
pub mod config;
pub mod ctx;
pub mod error;
pub mod handles;
pub mod output;
pub mod picker;
pub mod prompt;
pub mod render;
pub mod resolve;
pub mod sse;
pub mod tui;

pub use cli::Cli;
pub use error::{exit, CliError, CliResult};
