//! AI features: the Ollama LLM client, cross-device chat, and the Goose LLM
//! bridge.
//!
//! * [`llm`] — Ollama connection check, model list/pull/delete, and chat.
//!   Compiles everywhere.
//! * [`chat`] — one shared message thread persisted on the PC (the hub, matching
//!   the thin-client sync model); the mobile client relays to it.
//! * [`goose_llm`] — desktop only: the Goose SDK provider bridge inference goes
//!   through.
//!
//! The MCP server that exposes all of this to external agents lives at
//! `crate::mcp` (mounted on the sync `server`'s axum router at `/mcp`). Agents
//! reach the app that way; the app no longer dials an agent itself.

#[cfg(feature = "desktop")]
pub(crate) mod goose_llm;

pub(crate) mod chat;
pub(crate) mod llm;
