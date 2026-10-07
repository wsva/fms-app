//! AI features: the Ollama LLM client, cross-device chat, and the Goose agent
//! integrations.
//!
//! * [`llm`] — Ollama connection check, model list/pull/delete, chat +
//!   streaming chat. Compiles everywhere.
//! * [`chat`] — one shared message thread persisted on the PC (the hub, matching
//!   the thin-client sync model); the mobile client relays to it.
//! * [`goose_llm`] / [`agent_acp`] — desktop only. `agent_acp` connects to an
//!   already-running `goose serve` over WebSocket as the ACP *client*.
//!
//! The MCP server that exposes all of this to external agents lives at
//! `crate::mcp` (mounted on the sync `server`'s axum router at `/mcp`).

#[cfg(feature = "desktop")]
pub(crate) mod agent_acp;
#[cfg(feature = "desktop")]
pub(crate) mod goose_llm;

pub(crate) mod chat;
pub(crate) mod llm;
