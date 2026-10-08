//! Built-in MCP (Model Context Protocol) server for Goose integration.
//!
//! Uses the `rmcp` crate (v3.4) for protocol handling with Streamable HTTP transport.
//! Runs on the same axum HTTP server as the web service, nested at `/mcp`.
//!
//! Connect Goose Desktop at: http://localhost:35711/mcp
//!
//! # Layout
//!
//! The tools live one submodule per domain (`datasets`, `dictation`,
//! `cards`, `books`, `models`, `ai`, `wiki`, `sync`, `system`, `workflow`).
//! Each submodule holds the parameter structs for its own tools plus one
//! `#[tool_router(router = <domain>_router, vis = "pub(crate)")]` impl block on
//! [`DatasetMcpServer`]; [`DatasetMcpServer::tool_router`] merges those routers
//! with `+` into the single `ServerHandler` implemented at the bottom of this
//! file. Only `#[tool_router(server_handler)]` would generate that impl, and it
//! can only be applied once per type, hence the explicit composition.
//!
//! To add a tool, put it in the submodule for its domain (or add a submodule
//! and a `+ Self::<domain>_router()` line below). Tool names must stay unique
//! across domains: `ToolRouter::merge` silently overwrites a duplicate.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use rmcp::{ServerHandler, tool_handler};
use serde::Deserialize;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

// One submodule per tool domain. They stay private because nothing outside
// `mcp` needs them - only `create_mcp_service` and the server type are public.
mod datasets;
mod dictation;
mod cards;
mod books;
mod models;
mod ai;
mod wiki;
mod sync;
mod system;
mod workflow;

// ---------------------------------------------------------------------------
// MCP server state — shared across all connections
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DatasetMcpServer {
    pub app: AppHandle,
}

// ---------------------------------------------------------------------------
// Parameter types shared by more than one domain submodule
// ---------------------------------------------------------------------------

/// Wrapper around `serde_json::Value` that generates a proper JSON Schema
/// (`{"type": "object"}`) instead of the boolean `true` that `serde_json::Value`
/// produces. Ollama's tool parser cannot handle boolean schemas in `properties`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(transparent)]
pub(crate) struct JsonValue(serde_json::Value);

impl schemars::JsonSchema for JsonValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "JsonValue".into()
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object"
        })
    }

    fn inline_schema() -> bool {
        true
    }
}

impl From<JsonValue> for serde_json::Value {
    fn from(v: JsonValue) -> Self {
        v.0
    }
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub(crate) struct UuidParam {
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub(crate) struct ReadFileParam {
    uuid: String,
    path: String,
}

// ---------------------------------------------------------------------------
// Router composition
// ---------------------------------------------------------------------------

impl DatasetMcpServer {
    /// Merge every domain's router into the one router this server serves.
    pub(crate) fn tool_router() -> ToolRouter<Self> {
        Self::datasets_router()
            + Self::dictation_router()
            + Self::cards_router()
            + Self::books_router()
            + Self::models_router()
            + Self::ai_router()
            + Self::wiki_router()
            + Self::sync_router()
            + Self::system_router()
            + Self::workflow_router()
    }
}

#[tool_handler(router = Self::tool_router())]
impl ServerHandler for DatasetMcpServer {}

// ---------------------------------------------------------------------------
// Create the MCP service for nesting in axum router (called from sync/server.rs)
// ---------------------------------------------------------------------------

/// Create a StreamableHttpService for the MCP server.
/// Nest this in the axum router at `/mcp`.
pub fn create_mcp_service(app: AppHandle) -> StreamableHttpService<DatasetMcpServer, LocalSessionManager> {
    log::info!("[MCP] Creating MCP service");
    let server = DatasetMcpServer { app };
    let ct = CancellationToken::new();
    
    StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(ct),
    )
}
