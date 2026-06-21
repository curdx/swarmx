//! `swarmx-mcp`: MCP stdio bridge between claude / codex and the
//! swarmx-server REST surface.
//!
//! Architecture:
//!
//! ```text
//!  claude/codex  ←─ stdio (newline-delimited JSON-RPC) ─→  swarmx-mcp
//!                                                              │
//!                                                              │ reqwest (HTTP)
//!                                                              ▼
//!                                                       swarmx-server :7777
//! ```
//!
//! The crate exposes its internals through `pub mod` so we can unit-test
//! the dispatcher without spinning up a real subprocess.

pub mod handlers;
pub mod protocol;
pub mod tools;
pub mod wake_check;
