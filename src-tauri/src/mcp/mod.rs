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
//! `cards`, `books`, `models`, `ai`, `wiki`, `sync`, `system`, `workflow`,
//! `ui`, `todo`).
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

use std::future::Future;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    GetPromptRequestParams, GetPromptResponse, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
};
use rmcp::service::{MaybeSendFuture, RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use rmcp::{ErrorData as McpError, ServerHandler, tool_handler};
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
mod ui;
mod todo;
// Not a tool domain: the policy tier + audit journal every route is wrapped in
// by [`DatasetMcpServer::tool_router`].
mod policy;
// Not a tool domain either: the generated agent manual behind `/agent.md`,
// `/agent.json`, `app_bootstrap` and `get_info().instructions`.
mod manual;
// Not a tool domain either: the MCP *resources* (`fms://status`, `fms://docs/...`,
// `fms://workflows/templates`, `fms://datasets/{uuid}/layout`).
mod resources;
// Not a tool domain either: the MCP *prompts* — the goose recipes embedded from
// `docs/design/agent/goose_recipe/*.yaml`.
mod prompts;
// Not a tool domain either: the stdio ↔ HTTP proxy the executable enters with
// `--mcp-bridge`, which is how a stdio-only agent (and an ACP agent without
// `mcpCapabilities.http`) reaches this server's tools at all.
pub(crate) mod bridge;

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
    /// Merge every domain's router into the one router this server serves, then
    /// wrap each route in the policy guard so every call is classified and
    /// journalled before it reaches the tool (see `policy.rs`).
    pub(crate) fn tool_router() -> ToolRouter<Self> {
        policy::guard(
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
                + Self::ui_router()
                + Self::todo_router(),
        )
    }
}

#[tool_handler(router = Self::tool_router())]
impl ServerHandler for DatasetMcpServer {
    /// `InitializeResult` is built from this, so overriding it is how every
    /// conforming client receives the condensed manual without having to ask
    /// (ideas.md §5.0: the highest leverage per byte in the whole design). It
    /// also feeds `discover`. Hand-written rather than macro-generated —
    /// `#[tool_handler]` only emits `get_info` when the impl does not have one,
    /// so the default `Implementation::from_build_env()` (crate name + Cargo
    /// version) is replaced by the app's own identity.
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new("FmS", manual::build_id()))
        .with_instructions(manual::instructions(&self.app))
    }

    /// The documents an agent can read instead of calling tools. Static uris
    /// only — the one parameterised resource is a template, below.
    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(resources::list()))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(resources::templates()))
    }

    /// Resolve one `resources/read`. Read-only by construction: every body is
    /// derived at read time from the same values the tools serve.
    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResponse, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(
            resources::read(&self.app, &request.uri).map(ReadResourceResponse::from),
        )
    }

    /// The goose recipes, served as MCP prompts so a client gets the same idioms
    /// without a goose envelope.
    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(prompts::list()))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResponse, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(prompts::get(&request.name, request.arguments.as_ref()))
    }
}

// ---------------------------------------------------------------------------
// The generated manual, re-exported for the HTTP routes and the Connect card
// ---------------------------------------------------------------------------

/// The agent manual as JSON. `/agent.json`, the `app_bootstrap` tool and the
/// Settings → Connect card all render this one body.
pub fn manual_json(app: &AppHandle, zone: &str) -> serde_json::Value {
    manual::render_json(app, zone)
}

/// The same body as markdown. `/agent.md` serves this.
pub fn manual_markdown(app: &AppHandle, zone: &str) -> String {
    manual::render_markdown(app, zone)
}

/// Tauri twin of the `app_bootstrap` MCP tool: the Settings → Connect card reads
/// the same generated body the HTTP routes serve, so the port and the tool list
/// shown on the card cannot drift from the manual.
#[tauri::command]
pub async fn agent_manual_get(app: AppHandle) -> Result<serde_json::Value, String> {
    // The card runs inside the app's own webview, which is by definition the
    // loopback zone.
    Ok(manual::render_json(&app, "loopback"))
}

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
