//! Headless core of the AI Harness.
//!
//! Everything that is not drawing lives here: PTY-backed terminals and their
//! block model, git, files, search, workflows, the ACP client that drives the
//! dsh agent engine, the agent session model, and token accounting. The UI
//! crate consumes [`event::CoreEvent`] streams and query APIs; it never talks
//! to the agent engine directly.

pub mod acp;
pub mod agent;
pub mod config;
pub mod event;
pub mod fsops;
pub mod git;
pub mod term;
pub mod util;
pub mod workflow;

/// Crate version, surfaced in the app's about panel.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
