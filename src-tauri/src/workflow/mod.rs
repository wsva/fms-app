//! Persistent workflow state machine — a generic, file-based framework for
//! describing and running multi-step processes that an AI agent can drive and
//! resume after any interruption without relying on conversation history.
//!
//! See `docs/design/workflow.md` for the full specification.
//!
//! # Layout
//!
//! The domain-free framework lives in [`core`]: it knows about *steps*,
//! *dependencies* and *statuses*, never what a step actually does. Actions are
//! symbolic names; in this agent-in-the-loop build every step is handed back to
//! the agent (via the `workflow_*` MCP tools) rather than bound to an executor.
//!
//! This group root intentionally leaves room for future *job-specific* code
//! alongside `core/` — e.g. a `jobs/` submodule or a `dataset.rs` that binds
//! concrete actions (download, transcribe, …) to the generic engine. None of
//! that exists yet; today the group is just the framework plus its MCP surface.
//!
//! Desktop-only: the engine is reached through the desktop-gated MCP server
//! (`mcp/workflow.rs`) and, for the Workflow page, its `workflow_*` Tauri
//! command twins ([`commands`]) — both call the same [`core`] fns.

pub(crate) mod core;
/// Tauri command twins of the `workflow_*` MCP tools, so the Workflow page can
/// drive the same engine in-app. Desktop-only (registered only in the desktop
/// handler list), matching the engine + MCP surface.
pub(crate) mod commands;

// Re-export the public, run-scoped API so callers reference `crate::workflow::*`
// and never have to name the `core` child module (which would otherwise shadow
// Rust's `core` crate in path resolution). `self::` disambiguates the child.
pub(crate) use self::core::{
    advance, create_run, definition_text, intervene, list_runs, next_steps, record, status,
};
