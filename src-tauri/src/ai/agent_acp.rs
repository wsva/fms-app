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
//!
//! # Transports
//! Everything above the frame layer is transport-agnostic: `initialize`,
//! `session/new`, prompting and permission routing all speak the same JSON-RPC.
//! [`AcpTransport`] therefore has exactly two arms — `Ws` (a `goose serve` the
//! user started, authenticated by `X-Secret-Key`) and `Stdio` (a program the
//! dock spawns and talks to over its pipes) — and [`open_transport`] normalises
//! both into an [`AcpLink`]: one outbound queue of `String`, one inbound stream
//! of `String`, and the child process backing it (if any).

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::{self, Stream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::settings::{AgentHost, AgentTransport, SettingsState};
use crate::sync::server::WebServiceState;

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
    /// The child process backing a `stdio` host; `None` for WS. Taken and
    /// killed by `close()` and by the reader task on EOF, so an adapter cannot
    /// outlive the session that spawned it.
    child: Mutex<Option<Child>>,
    /// Which host record this session came from, and what to call it in the UI.
    host_id: Mutex<Option<String>>,
    display_name: Mutex<Option<String>>,
    /// `"ws"` or `"stdio"` — the label the Connect card shows.
    transport: Mutex<Option<String>>,
    /// What we handed the agent for this session (`agent_host_status`).
    mcp_url: Mutex<Option<String>>,
    mcp_transport: Mutex<Option<String>>,
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
                child: Mutex::new(None),
                host_id: Mutex::new(None),
                display_name: Mutex::new(None),
                transport: Mutex::new(None),
                mcp_url: Mutex::new(None),
                mcp_transport: Mutex::new(None),
            }),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    pub fn session_id(&self) -> Option<String> {
        self.inner.session_id.lock().unwrap().clone()
    }

    fn status_json(&self) -> Value {
        json!({
            "connected": self.is_connected(),
            "session_id": self.session_id(),
            "host_id": self.inner.host_id.lock().unwrap().clone(),
            "display_name": self.inner.display_name.lock().unwrap().clone(),
            "transport": self.inner.transport.lock().unwrap().clone(),
            // A session exists only after `session/new` accepted the MCP
            // servers, so "connected" already implies "registered".
            "mcp_registered": self.is_connected(),
            "mcp_url": self.inner.mcp_url.lock().unwrap().clone(),
            "mcp_transport": self.inner.mcp_transport.lock().unwrap().clone(),
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

    /// Drop the writer handle (ends the writer task, closing the socket or the
    /// child's stdin) and reset connection state. The reader task then observes
    /// EOF and cleans up.
    fn close(&self) {
        self.inner.connected.store(false, Ordering::SeqCst);
        self.inner.writer.lock().unwrap().take();
        *self.inner.session_id.lock().unwrap() = None;
        self.inner.pending.lock().unwrap().clear();
        self.inner.permission_pending.lock().unwrap().clear();
        // Dropping the writer closes a stdio child's stdin, but an adapter that
        // ignores EOF would linger; kill it outright.
        if let Some(mut child) = self.inner.child.lock().unwrap().take() {
            let _ = child.start_kill();
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Connect to the active host, run `initialize` + `session/new`, and start the
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

    // `AppSettings` is the authority for dial records: the first enabled host,
    // or one synthesized from the legacy `goose_acp_*` fields (which is also
    // what the load-time migration persists — settings.rs).
    let (host, secret) = {
        let s = settings.settings.lock().unwrap();
        (s.active_agent_host(), s.goose_acp_secret.clone())
    };
    let transport = AcpTransport::from_host(&host, &secret);
    dial_gate(&app, &host, &transport)?;
    forget_approval(&host.id);
    connect_over(app, &st, transport, &host).await
}

// ---------------------------------------------------------------------------
// Transports
// ---------------------------------------------------------------------------

/// The two ways the dock reaches an agent. Built from an [`AgentHost`]; the WS
/// secret is injected from `goose_acp_secret` rather than stored in the record,
/// so a host record never carries a credential (settings.rs keeps that field in
/// `SECRET_FIELDS`, which is what redacts it on every read).
#[derive(Clone, Debug)]
pub(crate) enum AcpTransport {
    /// A server the user started (`goose serve`), authenticated by header.
    Ws { url: String, secret: String },
    /// A program the dock spawns and speaks ACP to over its pipes.
    Stdio {
        program: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
}

impl AcpTransport {
    pub(crate) fn from_host(host: &AgentHost, secret: &str) -> Self {
        match &host.transport {
            AgentTransport::Ws { url } => Self::Ws {
                url: url.clone(),
                secret: secret.to_string(),
            },
            AgentTransport::Stdio {
                program,
                args,
                env,
                cwd,
            } => Self::Stdio {
                program: program.clone(),
                args: args.clone(),
                env: env.clone(),
                cwd: cwd.clone(),
            },
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Ws { .. } => "ws",
            Self::Stdio { .. } => "stdio",
        }
    }

    /// What the confirmation copy names: `host:port` for a socket, the program
    /// for a child process.
    pub(crate) fn endpoint(&self) -> String {
        match self {
            Self::Ws { url, .. } => ws_authority(url),
            Self::Stdio { program, .. } => program.clone(),
        }
    }
}

/// Whether dialling this transport needs the user's say-so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trust {
    /// Loopback and already the user's own process: same zone `/mcp` trusts.
    Silent,
    /// Off this machine, or an arbitrary program: the user must approve it.
    Confirm,
}

pub(crate) fn trust_level(transport: &AcpTransport) -> Trust {
    match transport {
        AcpTransport::Ws { url, .. } if is_loopback(url) => Trust::Silent,
        AcpTransport::Ws { .. } => Trust::Confirm,
        // A stdio host is an arbitrary program the dock executes. Even a
        // well-known adapter can do anything the user can, so it always asks.
        AcpTransport::Stdio { .. } => Trust::Confirm,
    }
}

/// `host:port` of a ws URL, with any `user:pass@` prefix dropped so a label can
/// never echo a credential. Falls back to the whole string when unparseable.
fn ws_authority(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest
        .split(|c| c == '/' || c == '?')
        .next()
        .unwrap_or(rest);
    authority.rsplit('@').next().unwrap_or(authority).to_string()
}

fn is_loopback(url: &str) -> bool {
    let authority = ws_authority(url);
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// One outbound frame queue, one inbound frame stream, and the child backing
/// them. Above this, nothing knows which transport it is talking to.
pub(crate) struct AcpLink {
    pub tx: mpsc::UnboundedSender<String>,
    pub frames: Pin<Box<dyn Stream<Item = String> + Send>>,
    pub child: Option<Child>,
    pub label: &'static str,
}

async fn open_transport(app: Option<&AppHandle>, transport: &AcpTransport) -> Result<AcpLink, String> {
    match transport {
        AcpTransport::Ws { url, secret } => open_ws(url, secret).await,
        AcpTransport::Stdio {
            program,
            args,
            env,
            cwd,
        } => open_stdio(app, program, args, env, cwd.as_deref()).await,
    }
}

async fn open_ws(url: &str, secret: &str) -> Result<AcpLink, String> {
    // Build the handshake request so we can attach the X-Secret-Key header.
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("invalid ACP URL '{}': {}", url, e))?;
    if !secret.trim().is_empty() {
        let hv = HeaderValue::from_str(secret.trim())
            .map_err(|e| format!("invalid goose_acp_secret: {}", e))?;
        request.headers_mut().insert("X-Secret-Key", hv);
    }

    let (ws, _resp) = connect_async(request)
        .await
        .map_err(|e| format!("failed to connect to {}: {}", url, e))?;
    let (mut sink, stream) = ws.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    // Writer task: drain the outbound queue into the socket.
    tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Normalise every text frame to `String` so the demux never learns which
    // transport it is on. `Close` is dropped rather than forwarded: tungstenite
    // ends the stream right after it, which the reader task treats as EOF.
    let frames = stream.filter_map(|frame| async move {
        match frame {
            Ok(Message::Text(text)) => Some(text.as_str().to_string()),
            Ok(Message::Close(_)) => None,
            Ok(_) => None,
            Err(e) => {
                log::warn!("[ACP] reader error: {}", e);
                None
            }
        }
    });

    Ok(AcpLink {
        tx,
        frames: Box::pin(frames),
        child: None,
        label: "ws",
    })
}

async fn open_stdio(
    app: Option<&AppHandle>,
    program: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Result<AcpLink, String> {
    if program.trim().is_empty() {
        return Err("stdio agent host has an empty command".to_string());
    }
    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(env.iter())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A child that ignores stdin EOF must not outlive the dock.
        .kill_on_drop(true);
    if let Some(dir) = cwd.filter(|d| !d.trim().is_empty()) {
        cmd.current_dir(dir);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn '{}': {}", program, e))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("'{}' gave us no stdin", program))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("'{}' gave us no stdout", program))?;
    let stderr = child.stderr.take();

    // stderr is diagnostics, never protocol: pump it to the log and to the
    // dock's agent log, so a failing adapter is visible without a terminal.
    // `app` is None in tests, where there is no webview to emit to.
    if let Some(stderr) = stderr {
        let app_log = app.cloned();
        let name = program.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                log::warn!("[ACP stdio] {}", line);
                if let Some(app) = &app_log {
                    let _ = app.emit("agent-log", json!({ "source": name, "text": line }));
                }
            }
        });
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            // ACP stdio framing: one JSON-RPC message per line.
            if stdin.write_all(text.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                break;
            }
        }
        // Closing the pipe is the polite shutdown; `kill_on_drop` is the rest.
        drop(stdin);
    });

    let frames = stream::unfold(BufReader::new(stdout), |mut reader| async move {
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => return None,
                Ok(_) => {
                    let text = line.trim_end().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    return Some((text, reader));
                }
                Err(e) => {
                    log::warn!("[ACP stdio] read error: {}", e);
                    return None;
                }
            }
        }
    });

    Ok(AcpLink {
        tx,
        frames: Box::pin(frames),
        child: Some(child),
        label: "stdio",
    })
}

/// The half of a session that does not depend on the transport: start the
/// reader task, then `initialize` → `session/new`. Shared by `agent_connect`
/// (the stored default) and `agent_host_connect` (an explicit record), so the
/// handshake exists exactly once.
pub(crate) async fn connect_over(
    app: AppHandle,
    st: &AcpClientState,
    transport: AcpTransport,
    host: &AgentHost,
) -> Result<Value, String> {
    if st.is_connected() {
        return Ok(st.status_json());
    }
    let label = transport.label();
    let link = open_transport(Some(&app), &transport).await?;
    // `AcpLink::label` is the transport that actually opened, which is what the
    // status card must show.
    debug_assert_eq!(label, link.label);
    let label = link.label;
    let AcpLink {
        tx,
        frames,
        child,
        ..
    } = link;

    *st.inner.writer.lock().unwrap() = Some(tx);
    *st.inner.child.lock().unwrap() = child;
    *st.inner.transport.lock().unwrap() = Some(label.to_string());
    *st.inner.host_id.lock().unwrap() = Some(host.id.clone());
    *st.inner.display_name.lock().unwrap() = Some(host.display_name.clone());
    st.inner.connected.store(true, Ordering::SeqCst);

    // Reader task: route frames to responses / notifications / agent requests.
    let inner = st.inner.clone();
    let app_reader = app.clone();
    let mut frames = frames;
    tokio::spawn(async move {
        while let Some(text) = frames.next().await {
            handle_message(&app_reader, &inner, &text);
        }
        // EOF: the agent went away, or we closed it. Both transports tear down
        // here, so there is one cleanup path rather than two.
        inner.connected.store(false, Ordering::SeqCst);
        *inner.writer.lock().unwrap() = None;
        *inner.session_id.lock().unwrap() = None;
        inner.pending.lock().unwrap().clear();
        inner.permission_pending.lock().unwrap().clear();
        if let Some(mut child) = inner.child.lock().unwrap().take() {
            let _ = child.start_kill();
        }
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

    // session/new — register the fms-app MCP server per-session. An `http`
    // entry is only honoured when the agent advertises `mcpCapabilities.http`;
    // otherwise the agent would get `mcpServers: []` and no tools at all, so the
    // fallback is a stdio entry pointing at this same executable in bridge mode
    // (see `mcp/bridge.rs`), which proxies to the URL below.
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    let mcp_port = app.state::<WebServiceState>().build_status().port;
    let mcp_url = format!("http://127.0.0.1:{}/mcp", mcp_port);
    let (mcp_servers, mcp_transport) = if http_mcp {
        (
            json!([{ "type": "http", "name": "fms-app", "url": mcp_url, "headers": [] }]),
            "http",
        )
    } else {
        log::info!("[ACP] agent has no http MCP support; handing it the stdio bridge");
        (json!([mcp_stdio_entry(&mcp_url)]), "stdio-bridge")
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
    *st.inner.mcp_url.lock().unwrap() = Some(mcp_url.clone());
    *st.inner.mcp_transport.lock().unwrap() = Some(mcp_transport.to_string());

    let status = json!({
        "connected": true,
        "session_id": session_id,
        "host_id": host.id,
        "display_name": host.display_name,
        "transport": label,
        "mcp_registered": true,
        "mcp_transport": mcp_transport,
        "mcp_url": mcp_url,
    });
    let _ = app.emit("agent-status", &status);
    log::info!(
        "[ACP] connected via {} to {}, session={} mcp_transport={}",
        label,
        host.display_name,
        session_id,
        mcp_transport
    );
    Ok(status)
}

/// The ACP stdio MCP entry that reaches this app's `/mcp` through the bridge.
/// The executable is the app itself: `--mcp-bridge` turns it into a proxy (see
/// `mcp/bridge.rs`), so there is no second artifact to install or sign.
fn mcp_stdio_entry(mcp_url: &str) -> Value {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "fms-app".to_string());
    json!({
        "type": "stdio",
        "name": "fms-app",
        "command": exe,
        "args": ["--mcp-bridge"],
        "env": [{ "name": "FMS_MCP_URL", "value": mcp_url }],
    })
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

/// Current connection status: `{ connected, session_id, host_id, transport, … }`.
#[tauri::command]
pub async fn agent_status(state: State<'_, AcpClientState>) -> Result<Value, String> {
    Ok(state.inner().status_json())
}

// ---------------------------------------------------------------------------
// Host records + trust confirmation (ideas.md §8 step 5)
// ---------------------------------------------------------------------------

/// Which hosts the user has already approved for this process lifetime, keyed
/// by host id. A confirmation is consumed by a successful connect, so an
/// approval is never a standing licence for a later different endpoint.
static APPROVED_HOSTS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn approved_hosts() -> HashMap<String, String> {
    APPROVED_HOSTS.lock().unwrap().clone().unwrap_or_default()
}

fn approve_host(host_id: &str, endpoint: &str) {
    let mut guard = APPROVED_HOSTS.lock().unwrap();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(host_id.to_string(), endpoint.to_string());
}

fn forget_approval(host_id: &str) {
    if let Some(map) = APPROVED_HOSTS.lock().unwrap().as_mut() {
        map.remove(host_id);
    }
}

/// Every host record, plus the rule that governs dialling it. The Settings card
/// renders this verbatim: trust is decided here, never re-derived in the UI, so
/// the dialog and the dialler cannot disagree.
pub(crate) fn hosts_json(s: &crate::settings::AppSettings) -> Value {
    let hosts: Vec<Value> = s
        .agent_hosts
        .iter()
        .map(|host| {
            // The secret is irrelevant to the trust rule, so do not pull it in.
            let transport = AcpTransport::from_host(host, "");
            json!({
                "id": host.id,
                "display_name": host.display_name,
                "enabled": host.enabled,
                "transport": host.transport,
                "kind": transport.label(),
                "endpoint": transport.endpoint(),
                "needs_confirmation": trust_level(&transport) == Trust::Confirm,
                "approved": approved_hosts().contains_key(&host.id),
                "last_session": host.last_session,
            })
        })
        .collect();
    json!({
        "hosts": hosts,
        "active_id": s.active_agent_host().id,
        "connected": false,
    })
}

/// Register (or replace) an agent host record. A record with an empty `id` gets
/// one derived from its display name, so the caller never has to invent ids.
#[tauri::command]
pub async fn agent_host_register(app: AppHandle, host: AgentHost) -> Result<Value, String> {
    register_host(&app, host)
}

/// Core of [`agent_host_register`] and of the MCP twin: validate, persist,
/// announce. Enabling a host disables the others — the dock hosts one at a time.
pub(crate) fn register_host(app: &AppHandle, host: AgentHost) -> Result<Value, String> {
    let settings = app.state::<SettingsState>();
    let mut host = host;
    host.display_name = host.display_name.trim().to_string();
    if host.display_name.is_empty() {
        return Err("an agent host needs a display name".to_string());
    }
    if host.id.trim().is_empty() {
        host.id = slugify(&host.display_name);
    }
    match &host.transport {
        AgentTransport::Ws { url } => {
            if url.trim().is_empty() {
                return Err("a ws agent host needs a url".to_string());
            }
        }
        AgentTransport::Stdio { program, .. } => {
            if program.trim().is_empty() {
                return Err("a stdio agent host needs a command".to_string());
            }
        }
    }

    let ws_dir = settings.workspace_dir.lock().unwrap().clone();
    let mut s = settings.settings.lock().unwrap().clone();
    s.upsert_agent_host(host.clone());
    SettingsState::save(&s, ws_dir.as_ref())?;
    *settings.settings.lock().unwrap() = s.clone();
    log::info!(
        "[ACP] registered agent host '{}' ({})",
        host.display_name,
        host.id
    );
    let _ = app.emit("settings-changed", ());
    Ok(json!({ "ok": true, "host": host, "hosts": hosts_json(&s)["hosts"].clone() }))
}

/// Drop a host record. If it is the one currently hosted, the session is closed
/// first — a removed record must not keep running behind the user's back.
#[tauri::command]
pub async fn agent_host_unregister(app: AppHandle, id: String) -> Result<Value, String> {
    unregister_host(&app, &id)
}

pub(crate) fn unregister_host(app: &AppHandle, id: &str) -> Result<Value, String> {
    let settings = app.state::<SettingsState>();
    let ws_dir = settings.workspace_dir.lock().unwrap().clone();
    let mut s = settings.settings.lock().unwrap().clone();
    let before = s.agent_hosts.len();
    s.agent_hosts.retain(|h| h.id != id);
    if s.agent_hosts.len() == before {
        return Err(format!(
            "unknown agent host '{}'. Known hosts: {}",
            id,
            host_id_list(&s)
        ));
    }
    SettingsState::save(&s, ws_dir.as_ref())?;
    *settings.settings.lock().unwrap() = s.clone();
    forget_approval(id);

    let st = app.state::<AcpClientState>().inner().clone();
    if st.inner.host_id.lock().unwrap().as_deref() == Some(id) {
        st.close();
        let _ = app.emit("agent-status", json!({ "connected": false }));
    }
    log::info!("[ACP] unregistered agent host '{}'", id);
    let _ = app.emit("settings-changed", ());
    Ok(json!({ "ok": true, "hosts": hosts_json(&s)["hosts"].clone() }))
}

/// List hosts and the trust rule for each (also the MCP twin's payload).
#[tauri::command]
pub async fn agent_host_list(app: AppHandle) -> Result<Value, String> {
    Ok(hosts_payload(&app))
}

pub(crate) fn hosts_payload(app: &AppHandle) -> Value {
    let mut out = {
        let settings = app.state::<SettingsState>();
        let s = settings.settings.lock().unwrap();
        hosts_json(&s)
    };
    // Fold in the live session, so one call answers "what would I dial" and
    // "what is dialled" at once.
    if let Some(dock) = app.try_state::<AcpClientState>() {
        let live = dock.inner().status_json();
        out["connected"] = live["connected"].clone();
        out["session"] = live;
    }
    out
}

/// Status of the hosted session and of one host record (`id` optional: defaults
/// to the active host).
#[tauri::command]
pub async fn agent_host_status(app: AppHandle, id: Option<String>) -> Result<Value, String> {
    Ok(host_status_payload(&app, id.as_deref()))
}

pub(crate) fn host_status_payload(app: &AppHandle, id: Option<&str>) -> Value {
    let live = app
        .try_state::<AcpClientState>()
        .map(|dock| dock.inner().status_json())
        .unwrap_or_else(|| json!({ "connected": false }));
    let settings = app.state::<SettingsState>();
    let s = settings.settings.lock().unwrap();
    let active_id = s.active_agent_host().id;
    let host = match id {
        Some(want) => s.agent_host(want).cloned(),
        None => Some(s.active_agent_host()),
    };
    let mut out = live;
    out["active_id"] = json!(active_id);
    out["requested_id"] = json!(id);
    out["host"] = match &host {
        Some(h) => {
            let transport = AcpTransport::from_host(h, "");
            json!({
                "id": h.id,
                "display_name": h.display_name,
                "enabled": h.enabled,
                "kind": transport.label(),
                "endpoint": transport.endpoint(),
                "needs_confirmation": trust_level(&transport) == Trust::Confirm,
                "approved": approved_hosts().contains_key(&h.id),
                "last_session": h.last_session,
            })
        }
        None => Value::Null,
    };
    out
}

/// The single dial rule, shared by the legacy `agent_connect` (the dock's
/// auto-connect on startup) and `agent_host_connect`: a host the trust table
/// marks `Confirm` is refused until the user has answered the dialog. It lives
/// here rather than in each caller so the auto-connect cannot bypass the
/// approval a manual connect would demand — a stdio host is a process, and
/// starting one without being asked is not something a background reconnect may
/// do.
///
/// The refusal *emits the dialog it names*: an agent calling `agent_host_connect`
/// over MCP has no way to click, so the ask has to appear in the app by itself,
/// and the error is then true when it says approval is waiting. The approval
/// itself is recorded by `agent_confirm_respond` — there is no MCP tool for it,
/// because it exists to be a human answer.
fn dial_gate(app: &AppHandle, host: &AgentHost, transport: &AcpTransport) -> Result<(), String> {
    if trust_level(transport) == Trust::Silent || approved_hosts().contains_key(&host.id) {
        return Ok(());
    }
    let payload = confirm_payload(host, transport);
    let _ = app.emit("agent-confirm-request", &payload);
    log::info!(
        "[ACP] refusing to dial '{}' ({}) until the user approves it",
        host.display_name,
        transport.endpoint()
    );
    Err(format!(
        "'{}' ({}) needs your approval first: FmS is asking the user now. Once they \
         approve it in the Connect card (Settings → Connect an agent), retry \
         agent_host_connect. Nothing was dialled.",
        host.display_name,
        transport.endpoint()
    ))
}

/// What the confirm dialog renders. One builder, so the event, the command and
/// the refusal cannot disagree about what the user is being asked.
fn confirm_payload(host: &AgentHost, transport: &AcpTransport) -> Value {
    json!({
        "host_id": host.id,
        "display_name": host.display_name,
        "kind": transport.label(),
        "endpoint": transport.endpoint(),
        "reason": match transport {
            AcpTransport::Stdio { .. } => "FmS will run this program on your machine",
            AcpTransport::Ws { .. } => "FmS will connect to another machine",
        },
    })
}

/// Connect to one host record. Refuses until the user has approved a host the
/// trust rule marks as `Confirm` — the refusal names the dialog and the retry,
/// because that is the whole recovery path.
#[tauri::command]
pub async fn agent_host_connect(app: AppHandle, id: String) -> Result<Value, String> {
    connect_host(&app, &id).await
}

/// Core of [`agent_host_connect`] and of the MCP twin.
pub(crate) async fn connect_host(app: &AppHandle, id: &str) -> Result<Value, String> {
    let st = app.state::<AcpClientState>().inner().clone();
    if st.is_connected() {
        return Ok(st.status_json());
    }
    let settings = app.state::<SettingsState>();
    let (host, secret) = {
        let s = settings.settings.lock().unwrap();
        let host = s.agent_host(id).cloned().ok_or_else(|| {
            format!(
                "unknown agent host '{}'. Known hosts: {}",
                id,
                host_id_list(&s)
            )
        })?;
        (host, s.goose_acp_secret.clone())
    };

    let transport = AcpTransport::from_host(&host, &secret);
    dial_gate(app, &host, &transport)?;
    forget_approval(&host.id);

    let status = connect_over(app.clone(), &st, transport, &host).await?;

    // Remember the session id on the record: it is what the status card shows
    // and what "last session" means. Advisory only.
    if let Some(session_id) = status.get("session_id").and_then(|v| v.as_str()) {
        let ws_dir = settings.workspace_dir.lock().unwrap().clone();
        let mut s = settings.settings.lock().unwrap().clone();
        if let Some(slot) = s.agent_hosts.iter_mut().find(|h| h.id == host.id) {
            slot.last_session = Some(session_id.to_string());
            SettingsState::save(&s, ws_dir.as_ref())?;
            *settings.settings.lock().unwrap() = s;
        }
    }
    let _ = app.emit("settings-changed", ());
    Ok(status)
}

/// Close the hosted session. The record stays: the user may reconnect with it.
#[tauri::command]
pub async fn agent_host_disconnect(
    app: AppHandle,
    state: State<'_, AcpClientState>,
) -> Result<Value, String> {
    let st = state.inner().clone();
    // A disconnect revokes the approval too: approving "once" must not survive
    // into a later session the user did not ask for.
    if let Some(host_id) = st.inner.host_id.lock().unwrap().clone() {
        forget_approval(&host_id);
    }
    st.close();
    let _ = app.emit("agent-status", json!({ "connected": false }));
    Ok(st.status_json())
}

/// Ask the user to approve dialling a host (the pairing flow's shape: emit an
/// event, let the caller poll `agent_confirm_status`). A loopback WS host needs
/// no approval, and says so, so the caller has one code path.
#[tauri::command]
pub async fn agent_confirm_request(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    id: String,
) -> Result<Value, String> {
    let (host, secret) = {
        let s = settings.settings.lock().unwrap();
        let host = s.agent_host(&id).cloned().ok_or_else(|| {
            format!(
                "unknown agent host '{}'. Known hosts: {}",
                id,
                host_id_list(&s)
            )
        })?;
        (host, s.goose_acp_secret.clone())
    };
    let transport = AcpTransport::from_host(&host, &secret);
    if trust_level(&transport) == Trust::Silent {
        return Ok(json!({
            "state": "approved",
            "host_id": host.id,
            "reason": "loopback ws hosts are trusted, like /mcp",
        }));
    }

    let endpoint = transport.endpoint();
    let payload = confirm_payload(&host, &transport);
    let _ = app.emit("agent-confirm-request", &payload);
    log::info!(
        "[ACP] asking the user to approve '{}' ({})",
        host.display_name,
        endpoint
    );
    Ok(json!({ "state": "pending", "host": payload }))
}

/// Answer `agent-confirm-request`. Approving is what unblocks
/// `agent_host_connect`; denying clears any earlier approval.
#[tauri::command]
pub async fn agent_confirm_respond(
    app: AppHandle,
    state: State<'_, AcpClientState>,
    settings: State<'_, SettingsState>,
    host_id: String,
    approve: bool,
) -> Result<Value, String> {
    let (host, secret) = {
        let s = settings.settings.lock().unwrap();
        let host = s.agent_host(&host_id).cloned().ok_or_else(|| {
            format!(
                "unknown agent host '{}'. Known hosts: {}",
                host_id,
                host_id_list(&s)
            )
        })?;
        (host, s.goose_acp_secret.clone())
    };
    let endpoint = AcpTransport::from_host(&host, &secret).endpoint();
    if approve {
        approve_host(&host.id, &endpoint);
        log::info!("[ACP] user approved '{}' ({})", host.display_name, endpoint);
    } else {
        forget_approval(&host.id);
        let st = state.inner().clone();
        if st.inner.host_id.lock().unwrap().as_deref() == Some(host.id.as_str()) {
            st.close();
            let _ = app.emit("agent-status", json!({ "connected": false }));
        }
        log::info!("[ACP] user denied '{}' ({})", host.display_name, endpoint);
    }
    let _ = app.emit(
        "agent-confirm-status",
        json!({
            "host_id": host.id,
            "state": if approve { "approved" } else { "denied" },
        }),
    );
    Ok(json!({
        "host_id": host.id,
        "state": if approve { "approved" } else { "denied" },
        "endpoint": endpoint,
    }))
}

/// Poll a host's approval state: `approved` | `denied` | `pending`.
#[tauri::command]
pub async fn agent_confirm_status(
    settings: State<'_, SettingsState>,
    host_id: String,
) -> Result<Value, String> {
    let s = settings.settings.lock().unwrap();
    let host = s.agent_host(&host_id).ok_or_else(|| {
        format!(
            "unknown agent host '{}'. Known hosts: {}",
            host_id,
            host_id_list(&s)
        )
    })?;
    let transport = AcpTransport::from_host(host, "");
    let state = if trust_level(&transport) == Trust::Silent
        || approved_hosts().contains_key(&host.id)
    {
        "approved"
    } else {
        "pending"
    };
    Ok(json!({ "host_id": host.id, "state": state }))
}

fn host_id_list(s: &crate::settings::AppSettings) -> String {
    if s.agent_hosts.is_empty() {
        return "(none registered)".to_string();
    }
    s.agent_hosts
        .iter()
        .map(|h| h.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Stable, readable id from a display name: lower-case ASCII words joined by
/// `-`. Falls back to `agent` for a name with no usable characters.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed
    }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(program: &str) -> AcpTransport {
        AcpTransport::Stdio {
            program: program.to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    /// The trust rule is the only thing standing between an agent request and a
    /// process spawn, so it is spelled out as a table rather than inferred.
    #[test]
    fn trust_level_table() {
        let ws = |url: &str| AcpTransport::Ws {
            url: url.to_string(),
            secret: String::new(),
        };
        assert_eq!(trust_level(&ws("ws://127.0.0.1:3284/acp")), Trust::Silent);
        assert_eq!(trust_level(&ws("ws://localhost:3284/acp")), Trust::Silent);
        assert_eq!(trust_level(&ws("wss://[::1]:3284/acp")), Trust::Silent);
        assert_eq!(trust_level(&ws("ws://127.0.0.1/acp")), Trust::Silent);
        // Anything off this machine needs the user, including the LAN and a
        // hostname that only *looks* local.
        assert_eq!(trust_level(&ws("ws://192.168.1.20:3284/acp")), Trust::Confirm);
        assert_eq!(trust_level(&ws("wss://agent.example.com/acp")), Trust::Confirm);
        assert_eq!(trust_level(&ws("ws://127.0.0.1.evil.com/acp")), Trust::Confirm);
        assert_eq!(trust_level(&ws("ws://0.0.0.0:3284/acp")), Trust::Confirm);
        // A stdio host is an arbitrary program: always ask, even for a name
        // this app ships with.
        assert_eq!(trust_level(&stdio("codex-acp")), Trust::Confirm);
        assert_eq!(trust_level(&stdio("/usr/bin/goose")), Trust::Confirm);
    }

    #[test]
    fn endpoint_labels_never_echo_credentials() {
        let t = AcpTransport::Ws {
            url: "ws://user:pw@example.com:3284/acp".to_string(),
            secret: "shh".to_string(),
        };
        assert_eq!(t.endpoint(), "example.com:3284");
        assert_eq!(t.label(), "ws");
        assert_eq!(stdio("goose").endpoint(), "goose");
        assert_eq!(stdio("goose").label(), "stdio");
    }

    /// A stdio transport must hand every produced line up and then end on the
    /// child's EOF — a stream that never ends would leave the dock "connected"
    /// to a process that is gone.
    #[tokio::test]
    async fn stdio_transport_ends_on_child_eof() {
        let (program, args) = if cfg!(windows) {
            (
                "cmd".to_string(),
                vec!["/c".to_string(), "echo {\"jsonrpc\":\"2.0\",\"id\":1}".to_string()],
            )
        } else {
            (
                "sh".to_string(),
                vec![
                    "-c".to_string(),
                    "printf '{\"jsonrpc\":\"2.0\",\"id\":1}\\n'".to_string(),
                ],
            )
        };
        let transport = AcpTransport::Stdio {
            program,
            args,
            env: BTreeMap::new(),
            cwd: None,
        };
        let mut link = open_transport(None, &transport).await.expect("spawn echo");
        assert!(link.child.is_some(), "a stdio link owns its child");
        let mut frames = link.frames;
        let first = frames.next().await.expect("one forwarded line");
        assert!(first.contains("jsonrpc"), "line was {:?}", first);
        assert!(!first.ends_with('\n'), "framing strips the newline");
        assert!(frames.next().await.is_none(), "stream ends on child EOF");
    }

    /// A command that does not exist is an error at open time, not a session
    /// that silently never answers.
    #[tokio::test]
    async fn stdio_transport_rejects_an_empty_command() {
        let err = open_transport(None, &stdio("  ")).await.err().expect("must fail");
        assert!(err.contains("empty command"), "got {:?}", err);
    }
}
