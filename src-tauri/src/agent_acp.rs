//! Agent Client Protocol (ACP) client for Goose (desktop only).
//!
//! fms-app connects to an already-running `goose serve` over WebSocket and acts
//! as the ACP *client*; goose is the *agent*. The user's typed/spoken prompts are
//! forwarded as `session/prompt`; goose streams `session/update` notifications
//! back (agent text, thoughts, tool calls, plans) which we re-emit as Tauri
//! events for the Agent page. Goose in turn drives fms-app through the built-in
//! MCP server (see `mcp.rs`) at `http://127.0.0.1:<port>/mcp`, including the
//! `app_*` control tools that emit `agent-action` events consumed by the shell.
//!
//! # Phase 0 — verified ACP wire protocol (JSON-RPC 2.0 over one text frame/msg)
//! - `initialize` → params `{ protocolVersion: 1, clientCapabilities: {} }`;
//!   result carries `agentCapabilities`. HTTP MCP registration per-session is
//!   only allowed when `agentCapabilities.mcpCapabilities.http == true`.
//! - `session/new` → params `{ cwd, mcpServers: [...] }`; result `{ sessionId }`.
//!   An HTTP MCP entry is `{ type: "http", name, url, headers: [{name,value}] }`.
//! - `session/prompt` → params `{ sessionId, prompt: [{ type:"text", text }] }`;
//!   result `{ stopReason }` (end_turn | max_tokens | max_turn_requests |
//!   refusal | cancelled). This is a long-running request: the result only
//!   arrives after the whole turn, so we do not block the command on it.
//! - `session/cancel` → *notification* (no id) `{ sessionId }`.
//! - `session/update` → *notification* `{ sessionId, update: { sessionUpdate, .. } }`
//!   with `sessionUpdate` ∈ agent_message_chunk | agent_thought_chunk | tool_call
//!   | tool_call_update | plan.
//! - `session/request_permission` → agent→client *request* (has id) with
//!   `{ sessionId, toolCall, options: [{ optionId, name, kind }] }`; the client
//!   replies `{ outcome: { outcome: "selected", optionId } }` or
//!   `{ outcome: { outcome: "cancelled" } }`.
//! - Transport auth for `goose serve` is the `X-Secret-Key` HTTP header; the ACP
//!   `authenticate` method is not needed for the shared-secret transport.
//!
//! The client lives in the Rust backend (not the webview) so the secret stays
//! server-side and WebView CORS/origin allow-listing is avoided.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::settings::SettingsState;
use crate::web_service::WebServiceState;

/// How long to wait for a normal JSON-RPC response (initialize / session/new).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a single prompt turn may run before we give up on its result.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(1800);
/// How long a permission dialog may stay unanswered before we auto-cancel.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct AcpInner {
    next_id: AtomicU64,
    connected: AtomicBool,
    /// Sender half of the outbound frame queue; `None` when disconnected.
    writer: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Pending client→agent requests awaiting a response, keyed by JSON-RPC id.
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    /// Pending agent→client permission requests, keyed by their JSON-RPC id.
    permission_pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    session_id: Mutex<Option<String>>,
    /// Last UI-state snapshot reported by the frontend, read by `app_get_ui_state`.
    ui_state: Mutex<Value>,
}

#[derive(Clone)]
pub struct AcpClientState {
    inner: Arc<AcpInner>,
}

impl Default for AcpClientState {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpClientState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AcpInner {
                next_id: AtomicU64::new(0),
                connected: AtomicBool::new(false),
                writer: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                permission_pending: Mutex::new(HashMap::new()),
                session_id: Mutex::new(None),
                ui_state: Mutex::new(json!({})),
            }),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    pub fn session_id(&self) -> Option<String> {
        self.inner.session_id.lock().unwrap().clone()
    }

    /// Cached UI-state snapshot for the `app_get_ui_state` MCP tool.
    pub fn ui_state(&self) -> Value {
        self.inner.ui_state.lock().unwrap().clone()
    }

    fn status_json(&self) -> Value {
        json!({
            "connected": self.is_connected(),
            "session_id": self.session_id(),
        })
    }

    /// Send a JSON-RPC request and return its id + a receiver for the response.
    fn send_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(u64, oneshot::Receiver<Value>), String> {
        let writer = self
            .inner
            .writer
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "ACP client is not connected".to_string())?;
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writer.send(msg.to_string()).map_err(|e| {
            self.inner.pending.lock().unwrap().remove(&id);
            format!("failed to send {}: {}", method, e)
        })?;
        Ok((id, rx))
    }

    /// Send a request and await its `result` (or surface its `error`).
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let (_, rx) = self.send_request(method, params)?;
        let msg = tokio::time::timeout(REQUEST_TIMEOUT, rx)
            .await
            .map_err(|_| format!("{} timed out", method))?
            .map_err(|_| format!("{}: connection closed", method))?;
        if let Some(err) = msg.get("error") {
            return Err(format!("{} error: {}", method, err));
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Drop the writer handle (ends the writer task, closing the socket) and
    /// reset connection state. The reader task then observes EOF and cleans up.
    fn close(&self) {
        self.inner.connected.store(false, Ordering::SeqCst);
        self.inner.writer.lock().unwrap().take();
        *self.inner.session_id.lock().unwrap() = None;
        self.inner.pending.lock().unwrap().clear();
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Connect to `goose serve`, run `initialize` + `session/new`, and start the
/// reader/writer tasks. Idempotent: returns the current status when connected.
#[tauri::command]
pub async fn agent_connect(
    app: AppHandle,
    state: State<'_, AcpClientState>,
    settings: State<'_, SettingsState>,
) -> Result<Value, String> {
    let st = state.inner().clone();
    if st.is_connected() {
        return Ok(st.status_json());
    }

    let (url, secret) = {
        let s = settings.settings.lock().unwrap();
        (s.goose_acp_url.clone(), s.goose_acp_secret.clone())
    };
    if url.trim().is_empty() {
        return Err("goose_acp_url is not configured (set it in Settings → Agent)".to_string());
    }

    // Build the WS handshake request so we can attach the X-Secret-Key header.
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("invalid ACP URL '{}': {}", url, e))?;
    if !secret.trim().is_empty() {
        let hv = HeaderValue::from_str(secret.trim())
            .map_err(|e| format!("invalid goose_acp_secret: {}", e))?;
        request.headers_mut().insert("X-Secret-Key", hv);
    }

    let (ws, _resp) = connect_async(request)
        .await
        .map_err(|e| format!("failed to connect to goose serve at {}: {}", url, e))?;
    let (mut sink, mut stream) = ws.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    *st.inner.writer.lock().unwrap() = Some(tx);
    st.inner.connected.store(true, Ordering::SeqCst);

    // Writer task: drain the outbound queue into the socket.
    tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Reader task: route frames to responses / notifications / agent requests.
    let inner = st.inner.clone();
    let app_reader = app.clone();
    tokio::spawn(async move {
        while let Some(frame) = stream.next().await {
            match frame {
                Ok(Message::Text(text)) => handle_message(&app_reader, &inner, text.as_str()),
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    log::warn!("[ACP] reader error: {}", e);
                    break;
                }
            }
        }
        inner.connected.store(false, Ordering::SeqCst);
        *inner.writer.lock().unwrap() = None;
        *inner.session_id.lock().unwrap() = None;
        inner.pending.lock().unwrap().clear();
        let _ = app_reader.emit("agent-status", json!({ "connected": false }));
        log::info!("[ACP] connection closed");
    });

    // initialize
    let init = st
        .request("initialize", json!({ "protocolVersion": 1, "clientCapabilities": {} }))
        .await
        .map_err(|e| {
            st.close();
            format!("initialize failed: {}", e)
        })?;
    let http_mcp = init
        .pointer("/agentCapabilities/mcpCapabilities/http")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // session/new — register the fms-app MCP server per-session when supported.
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    let mcp_port = app.state::<WebServiceState>().build_status().port;
    let mcp_url = format!("http://127.0.0.1:{}/mcp", mcp_port);
    let mcp_servers = if http_mcp {
        json!([{ "type": "http", "name": "fms-app", "url": mcp_url, "headers": [] }])
    } else {
        json!([])
    };
    let new_res = st
        .request("session/new", json!({ "cwd": cwd, "mcpServers": mcp_servers }))
        .await
        .map_err(|e| {
            st.close();
            format!("session/new failed: {}", e)
        })?;
    let session_id = new_res
        .get("sessionId")
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        st.close();
        return Err(format!("session/new returned no sessionId: {}", new_res));
    }
    *st.inner.session_id.lock().unwrap() = Some(session_id.clone());

    let status = json!({
        "connected": true,
        "session_id": session_id,
        "mcp_registered": http_mcp,
        "mcp_url": mcp_url,
    });
    let _ = app.emit("agent-status", &status);
    log::info!("[ACP] connected, session={} mcp_registered={}", session_id, http_mcp);
    Ok(status)
}

/// Send a user prompt for the active session. Returns immediately; the turn's
/// completion is emitted later as `agent-done` (or `agent-error`).
#[tauri::command]
pub async fn agent_send_prompt(
    app: AppHandle,
    state: State<'_, AcpClientState>,
    text: String,
) -> Result<Value, String> {
    let st = state.inner().clone();
    let session_id = st
        .session_id()
        .ok_or_else(|| "no active ACP session; connect first".to_string())?;
    let params = json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": text }] });
    let (id, rx) = st.send_request("session/prompt", params)?;

    let app_done = app.clone();
    tokio::spawn(async move {
        match tokio::time::timeout(PROMPT_TIMEOUT, rx).await {
            Ok(Ok(msg)) => {
                if let Some(err) = msg.get("error") {
                    let _ = app_done.emit(
                        "agent-error",
                        json!({ "request_id": id, "message": err.to_string() }),
                    );
                } else {
                    let stop = msg.pointer("/result/stopReason").cloned().unwrap_or(Value::Null);
                    let _ = app_done.emit("agent-done", json!({ "request_id": id, "stop_reason": stop }));
                }
            }
            _ => {
                let _ = app_done.emit(
                    "agent-error",
                    json!({ "request_id": id, "message": "prompt turn timed out or connection closed" }),
                );
            }
        }
    });

    Ok(json!({ "ok": true, "request_id": id }))
}

/// Cancel the in-flight prompt turn for the active session (notification).
#[tauri::command]
pub async fn agent_cancel(state: State<'_, AcpClientState>) -> Result<(), String> {
    let st = state.inner().clone();
    let session_id = st
        .session_id()
        .ok_or_else(|| "no active ACP session".to_string())?;
    let writer = st
        .inner
        .writer
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "ACP client is not connected".to_string())?;
    let msg = json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session_id } });
    writer.send(msg.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Respond to an `agent-permission-request`. `option_id` selects an offered
/// option; `None`/empty cancels the tool call.
#[tauri::command]
pub async fn agent_respond_permission(
    state: State<'_, AcpClientState>,
    request_id: u64,
    option_id: Option<String>,
) -> Result<(), String> {
    let st = state.inner().clone();
    let tx = st
        .inner
        .permission_pending
        .lock()
        .unwrap()
        .remove(&request_id)
        .ok_or_else(|| format!("no pending permission request with id {}", request_id))?;
    let outcome = match option_id {
        Some(id) if !id.is_empty() => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        _ => json!({ "outcome": { "outcome": "cancelled" } }),
    };
    let _ = tx.send(outcome);
    Ok(())
}

/// Close the ACP connection and clear the session.
#[tauri::command]
pub async fn agent_disconnect(app: AppHandle, state: State<'_, AcpClientState>) -> Result<(), String> {
    let st = state.inner().clone();
    st.close();
    let _ = app.emit("agent-status", json!({ "connected": false }));
    Ok(())
}

/// Current connection status: `{ connected, session_id }`.
#[tauri::command]
pub async fn agent_status(state: State<'_, AcpClientState>) -> Result<Value, String> {
    Ok(state.inner().status_json())
}

/// Cache the frontend's current UI state so `app_get_ui_state` can answer the
/// agent synchronously without a round-trip to the webview.
#[tauri::command]
pub async fn agent_report_ui_state(
    state: State<'_, AcpClientState>,
    active_tab: Option<String>,
    active_dataset_uuid: Option<String>,
) -> Result<(), String> {
    let st = state.inner().clone();
    let mut ui = st.inner.ui_state.lock().unwrap();
    if let Some(tab) = active_tab {
        ui["active_tab"] = json!(tab);
    }
    if let Some(uuid) = active_dataset_uuid {
        ui["active_dataset_uuid"] = json!(uuid);
    }
    ui["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
    Ok(())
}

// ---------------------------------------------------------------------------
// Reader-side message routing
// ---------------------------------------------------------------------------

/// Route one inbound JSON-RPC frame: responses → pending map; notifications →
/// events; agent requests (permission) → auto-approve policy or an event.
fn handle_message(app: &AppHandle, inner: &Arc<AcpInner>, raw: &str) {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[ACP] ignoring non-JSON frame: {}", e);
            return;
        }
    };

    let method = v.get("method").and_then(|m| m.as_str());
    let id = v.get("id").and_then(|i| i.as_u64());

    match (method, id) {
        // Response to one of our requests.
        (None, Some(id)) => {
            if let Some(tx) = inner.pending.lock().unwrap().remove(&id) {
                let _ = tx.send(v);
            }
        }
        (Some("session/update"), _) => handle_session_update(app, &v),
        (Some("session/request_permission"), Some(id)) => {
            handle_permission_request(app, inner, &v, id);
        }
        // Any other agent→client request: refuse so the agent does not hang.
        (Some(m), Some(id)) => {
            log::info!("[ACP] unsupported agent request '{}', refusing", m);
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not supported: {}", m) },
            });
            send_raw(inner, resp);
        }
        _ => {}
    }
}

fn handle_session_update(app: &AppHandle, v: &Value) {
    let params = &v["params"];
    let session_id = params["sessionId"].as_str().unwrap_or_default();
    let update = &params["update"];
    match update["sessionUpdate"].as_str().unwrap_or_default() {
        "agent_message_chunk" => {
            let text = extract_text(&update["content"]);
            if !text.is_empty() {
                let _ = app.emit("agent-message-chunk", json!({ "session_id": session_id, "text": text }));
            }
        }
        "agent_thought_chunk" => {
            let text = extract_text(&update["content"]);
            if !text.is_empty() {
                let _ = app.emit("agent-thought", json!({ "session_id": session_id, "text": text }));
            }
        }
        "tool_call" => {
            let _ = app.emit(
                "agent-tool-call",
                json!({
                    "session_id": session_id,
                    "id": update["toolCallId"],
                    "title": update["title"],
                    "kind": update["kind"],
                    "status": update["status"],
                    "raw_input": update["rawInput"],
                }),
            );
        }
        "tool_call_update" => {
            let _ = app.emit(
                "agent-tool-update",
                json!({
                    "session_id": session_id,
                    "id": update["toolCallId"],
                    "status": update["status"],
                    "raw_output": update["rawOutput"],
                }),
            );
        }
        "plan" => {
            let _ = app.emit("agent-plan", json!({ "session_id": session_id, "entries": update["entries"] }));
        }
        other => {
            let _ = app.emit(
                "agent-update",
                json!({ "session_id": session_id, "update_type": other, "update": update }),
            );
        }
    }
}

fn handle_permission_request(app: &AppHandle, inner: &Arc<AcpInner>, v: &Value, id: u64) {
    let params = &v["params"];
    let session_id = params["sessionId"].as_str().unwrap_or_default();
    let tool_call = params.get("toolCall").cloned().unwrap_or_else(|| json!({}));
    let options = params.get("options").cloned().unwrap_or_else(|| json!([]));

    // Auto-approve safe tools; only surface a dialog for destructive ones.
    if let Some(option_id) = auto_approve_option(&tool_call, &options) {
        log::info!("[ACP] auto-approving permission request {} ({})", id, option_id);
        send_raw(
            inner,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "outcome": { "outcome": "selected", "optionId": option_id } },
            }),
        );
        return;
    }

    let (tx, rx) = oneshot::channel();
    inner.permission_pending.lock().unwrap().insert(id, tx);
    let _ = app.emit(
        "agent-permission-request",
        json!({
            "request_id": id,
            "session_id": session_id,
            "tool_call": tool_call,
            "options": options,
        }),
    );

    let inner2 = inner.clone();
    tokio::spawn(async move {
        let outcome = match tokio::time::timeout(PERMISSION_TIMEOUT, rx).await {
            Ok(Ok(v)) => v,
            _ => json!({ "outcome": { "outcome": "cancelled" } }),
        };
        inner2.permission_pending.lock().unwrap().remove(&id);
        if let Some(w) = inner2.writer.lock().unwrap().clone() {
            let resp = json!({ "jsonrpc": "2.0", "id": id, "result": outcome });
            let _ = w.send(resp.to_string());
        }
    });
}

/// Decide whether a permission request can be auto-approved. Returns the option
/// id to select, or `None` to surface a dialog. Destructive tools always ask.
fn auto_approve_option(tool_call: &Value, options: &Value) -> Option<String> {
    let name = tool_call
        .get("name")
        .and_then(|n| n.as_str())
        .or_else(|| tool_call.get("title").and_then(|t| t.as_str()))
        .unwrap_or_default()
        .to_lowercase();
    // Unknown tool identity → be conservative and ask.
    if name.trim().is_empty() {
        return None;
    }
    const DESTRUCTIVE: [&str; 7] =
        ["delete", "remove", "clear", "drop", "revoke", "reset", "rebuild"];
    if DESTRUCTIVE.iter().any(|k| name.contains(k)) {
        return None;
    }
    let arr = options.as_array()?;
    for preferred in ["allow_once", "allow_always"] {
        for o in arr {
            if o.get("kind").and_then(|k| k.as_str()) == Some(preferred) {
                if let Some(id) = o.get("optionId").and_then(|i| i.as_str()) {
                    return Some(id.to_string());
                }
            }
        }
    }
    // No allow option offered → ask the user.
    None
}

/// Pull text out of an ACP content value: a bare string, a single
/// `{ type:"text", text }` block, or an array of such blocks.
fn extract_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if content.get("type").and_then(|t| t.as_str()) == Some("text") {
        return content.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    }
    if let Some(arr) = content.as_array() {
        return arr
            .iter()
            .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn send_raw(inner: &Arc<AcpInner>, v: Value) {
    if let Some(w) = inner.writer.lock().unwrap().clone() {
        let _ = w.send(v.to_string());
    }
}
