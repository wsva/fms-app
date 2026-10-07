use std::net::SocketAddr;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Multipart, Path as AxPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncSeekExt, AsyncReadExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;

use crate::datasets::{self, DatasetSummary};
use crate::datasets::book::BookChapter;
use crate::edge_tts;
use crate::models::{self, ModelState};
use crate::settings::SettingsState;
use crate::sync;

/// Default port the web server binds to.
const DEFAULT_PORT: u16 = 35711;

/// Default UDP port for the LAN/Tailscale discovery beacon + probe.
const DISCOVERY_PORT: u16 = 35712;

/// Cap on a signed (non-multipart) request body we buffer to hash for v2
/// signature verification. Sync change-batches and pair requests are small
/// JSON payloads; large binary uploads go through multipart (empty body hash).
const MAX_SIGNED_BODY_BYTES: usize = 32 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Config / status types
// ---------------------------------------------------------------------------

/// Which services the web server should expose.
#[derive(Clone, Serialize, Deserialize)]
pub struct WebServiceConfig {
    pub port: u16,
    pub stt: bool,
    pub dataset: bool,
    pub tts: bool,
    /// DEPRECATED: the legacy shared token is no longer enforced. Auth is now
    /// the trust-zone + device-signature model in `zone_guard` / `pairing.rs`.
    /// The field stays (serde default) so old persisted configs still parse.
    #[serde(default)]
    pub api_token: String,
}

impl Default for WebServiceConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            stt: true,
            dataset: true,
            tts: true,
            api_token: String::new(),
        }
    }
}

/// Runtime status returned to the frontend.
#[derive(Clone, Serialize)]
pub struct WebServiceStatus {
    pub running: bool,
    pub port: u16,
    pub stt: bool,
    pub dataset: bool,
    pub tts: bool,
    pub local_url: Option<String>,
    pub lan_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Managed state
// ---------------------------------------------------------------------------

pub struct WebServiceState {
    running: Arc<AtomicBool>,
    config: Mutex<WebServiceConfig>,
    bound_port: Mutex<Option<u16>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl WebServiceState {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            config: Mutex::new(WebServiceConfig::default()),
            bound_port: Mutex::new(None),
            shutdown: Mutex::new(None),
        }
    }

    fn stop_inner(&self) {
        if let Some(tx) = self.shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
        self.running.store(false, Ordering::SeqCst);
        *self.bound_port.lock().unwrap() = None;
    }

    /// Start the web service. Called from both the Tauri command and auto-start in setup.
    pub async fn start_inner(
        &self,
        app: AppHandle,
        config: WebServiceConfig,
    ) -> Result<WebServiceStatus, String> {
        self.stop_inner();

        let port = if config.port == 0 { DEFAULT_PORT } else { config.port };
        log::info!("Starting web service on port {}", port);
        let addr = SocketAddr::from(([0, 0, 0, 0], port));

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Failed to bind port {}: {}", port, e))?;
        let bound = listener.local_addr().map_err(|e| e.to_string())?.port();

        let router = build_router(app.clone(), config.clone());
        let (tx, rx) = oneshot::channel::<()>();

        tauri::async_runtime::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
        });

        {
            let mut cfg = self.config.lock().unwrap();
            *cfg = WebServiceConfig {
                port: bound,
                stt: config.stt,
                dataset: config.dataset,
                tts: config.tts,
                api_token: config.api_token.clone(),
            };
        }
        *self.bound_port.lock().unwrap() = Some(bound);
        *self.shutdown.lock().unwrap() = Some(tx);
        self.running.store(true, Ordering::SeqCst);

        spawn_discovery(app.clone(), bound);

        log::info!("Web service started on port {}", bound);
        Ok(self.build_status())
    }

    pub fn build_status(&self) -> WebServiceStatus {
        let running = self.running.load(Ordering::SeqCst);
        let config = self.config.lock().unwrap().clone();
        let port = self.bound_port.lock().unwrap().unwrap_or(config.port);
        let (local_url, lan_url) = if running {
            (
                Some(format!("http://127.0.0.1:{}", port)),
                local_ip().map(|ip| format!("http://{}:{}", ip, port)),
            )
        } else {
            (None, None)
        };
        WebServiceStatus {
            running,
            port,
            stt: config.stt,
            dataset: config.dataset,
            tts: config.tts,
            local_url,
            lan_url,
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Auto-start the web service. Called from app setup.
/// This is a standalone function to avoid borrow checker issues with State + AppHandle.
pub async fn auto_start(app: AppHandle) {
    let config = WebServiceConfig::default();
    let port = DEFAULT_PORT;
    log::info!("Auto-starting web service on port {}", port);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            log::warn!("Failed to auto-start web service: {}", e);
            return;
        }
    };
    let bound = listener.local_addr().unwrap().port();

    let router = build_router(app.clone(), config.clone());
    let (tx, rx) = oneshot::channel::<()>();

    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = rx.await;
        })
        .await;
    });

    // Update the managed state
    let state = app.state::<WebServiceState>();
    {
        let mut cfg = state.config.lock().unwrap();
        *cfg = WebServiceConfig {
            port: bound,
            stt: config.stt,
            dataset: config.dataset,
            tts: config.tts,
            api_token: config.api_token.clone(),
        };
    }
    *state.bound_port.lock().unwrap() = Some(bound);
    *state.shutdown.lock().unwrap() = Some(tx);
    state.running.store(true, Ordering::SeqCst);

    spawn_discovery(app.clone(), bound);

    log::info!("Web service auto-started on port {}", bound);
}

#[tauri::command]
pub async fn web_service_get_status(
    state: tauri::State<'_, WebServiceState>,
) -> Result<WebServiceStatus, String> {
    Ok(state.build_status())
}

#[tauri::command]
pub async fn web_service_start(
    app: AppHandle,
    state: tauri::State<'_, WebServiceState>,
    config: WebServiceConfig,
) -> Result<WebServiceStatus, String> {
    state.start_inner(app, config).await
}

#[tauri::command]
pub async fn web_service_stop(
    state: tauri::State<'_, WebServiceState>,
) -> Result<WebServiceStatus, String> {
    log::info!("Stopping web service");
    state.stop_inner();
    Ok(state.build_status())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub app: AppHandle,
    pub config: Arc<WebServiceConfig>,
}

fn build_router(app: AppHandle, config: WebServiceConfig) -> Router {
    let enable_stt = config.stt;
    let enable_dataset = config.dataset;
    let enable_tts = config.tts;
    let state = AppState {
        app: app.clone(),
        config: Arc::new(config),
    };

    // PC-side REST API for the Android thin client (snapshot + writeback).
    // Carries its own state, so it is nested after the main router's state is
    // applied below.
    let rest_router = sync::rest::router(app.clone());

    // MCP server (always enabled when web service is running)
    let mcp_service = crate::mcp::create_mcp_service(app);

    let mut r = Router::new()
        .route("/", get(index_handler))
        .nest_service("/mcp", mcp_service);

    if enable_stt {
        r = r.route("/stt", get(stt_form).post(stt_handler));
    }
    if enable_dataset {
        r = r
            .route("/datasets", get(datasets_list))
            .route("/datasets/{uuid}", get(dataset_detail))
            .route("/datasets/{uuid}/list", get(dataset_file_list))
            .route("/datasets/{uuid}/file/{*path}", get(dataset_file));
    }
    if enable_tts {
        r = r
            .route("/tts", get(tts_get).post(tts_post))
            .route("/tts/voices", get(tts_voices));
    }

    // Reading library (books) — read-only browsing, always available.
    r = r
        .route("/books", get(books_list))
        .route("/books/{uuid}", get(book_chapters_page))
        .route("/books/{uuid}/{chapter}", get(book_chapter_content));

    // Apply the main router state, then nest the (self-contained) REST router,
    // then wrap *everything* in the trust-zone guard (auth over `/api/v1`,
    // `/mcp`, `/stt`, `/datasets`, `/tts`, `/books` uniformly).
    r.with_state(state.clone())
        .nest("/api/v1", rest_router)
        // Outermost guard: applied last so it wraps every route, including the
        // nested REST router. Trust zones are decided from the TCP peer addr.
        .layer(middleware::from_fn_with_state(state, zone_guard))
}

// ---------------------------------------------------------------------------
// Trust-zone auth (Bluetooth-style pairing enforcement)
// ---------------------------------------------------------------------------

/// Classify the peer: loopback (with DNS-rebinding `Host` guard, checked by
/// the caller) and the Tailscale CGNAT range `100.64.0.0/10` are Trusted —
/// local agent calls need no credentials. Everything else is Untrusted.
fn zone_trusted(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            // 100.64.0.0/10: top two bits of the second octet are `01`.
            let o = v4.octets();
            o[0] == 100 && (o[1] & 0xC0) == 0x40
        }
        std::net::IpAddr::V6(_) => false, // loopback handled via Host check
    }
}

/// Outer guard implementing the three trust zones:
/// * loopback peers are Trusted only when the `Host` header names the local
///   machine (DNS-rebinding guard) — a browser tricked into pointing a public
///   hostname at 127.0.0.1 gets 403;
/// * trusted-zone peers pass with no credentials;
/// * untrusted peers reach only the handshake paths (`/api/v1/status`,
///   `/api/v1/pair/*`); everything else needs valid device-signature headers
///   (`x-fms-device` / `x-fms-ts` / `x-fms-sig`, verified in `pairing.rs`).
async fn zone_guard(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());
    let headers = req.headers().clone();
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let Some(peer) = peer else {
        // No peer info (shouldn't happen over TCP): fail closed.
        return json_error(StatusCode::FORBIDDEN, "unknown source address");
    };

    // Loopback: trusted only behind a local-looking Host header.
    if peer.is_loopback() {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let host_name = if let Some(rest) = host.strip_prefix('[') {
            // IPv6 literal: `[::1]` or `[::1]:port`.
            rest.split_once(']').map(|(h, _)| h).unwrap_or(rest)
        } else {
            host.split_once(':').map(|(h, _)| h).unwrap_or(host)
        };
        let local_host =
            matches!(host_name, "localhost" | "127.0.0.1" | "::1") || host_name.starts_with("127.");
        if !local_host {
            return json_error(
                StatusCode::FORBIDDEN,
                "loopback access requires a local Host header (DNS-rebinding guard)",
            );
        }
        // Trusted zone: no device binding. Handlers fall back to the PC's own
        // workspace identity for writeback attribution.
        req.extensions_mut().insert(sync::pairing::AuthContext::trusted());
        return next.run(req).await;
    }

    if zone_trusted(peer) {
        req.extensions_mut().insert(sync::pairing::AuthContext::trusted());
        return next.run(req).await;
    }

    // Untrusted zone: pairing handshake routes stay open, everything else
    // requires a valid per-device Ed25519 signature.
    if path == "/api/v1/status" || path.starts_with("/api/v1/pair/") {
        req.extensions_mut().insert(sync::pairing::AuthContext::trusted());
        return next.run(req).await;
    }
    let get_header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let device_id = get_header("x-fms-device");
    let ts = get_header("x-fms-ts");
    let sig = get_header("x-fms-sig");
    if device_id.is_empty() || ts.is_empty() || sig.is_empty() {
        return json_error(
            StatusCode::UNAUTHORIZED,
            sync::pairing::AuthError::Unpaired.message(),
        );
    }
    // v2 signatures cover `sha256(query\nbody)`, so read the declared protocol
    // and the exact query string before verification. Peer protocol gates the
    // preimage format (§3.3 hardening); a not-yet-upgraded device (missing /
    // zero header) keeps the legacy path.
    let peer_protocol = get_header("x-fms-protocol")
        .parse::<u32>()
        .unwrap_or(0);
    let query = req.uri().query().unwrap_or("").to_string();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    // Buffer the body only when we must hash it. Streaming multipart uploads
    // (`/chat/message`) can carry large media and sign an *empty* body
    // component on both ends, so leave those untouched for the handler.
    let body_bytes: Vec<u8> = if peer_protocol >= 2 && content_type.starts_with("multipart/") {
        Vec::new()
    } else if peer_protocol >= 2 {
        let (parts, body) = req.into_parts();
        match axum::body::to_bytes(body, MAX_SIGNED_BODY_BYTES).await {
            Ok(bytes) => {
                let v = bytes.to_vec();
                req = Request::from_parts(parts, Body::from(v.clone()));
                v
            }
            Err(_) => return json_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"),
        }
    } else {
        Vec::new()
    };
    // Verification touches SQLite (cached) + crypto: keep it off the async
    // runtime; settings are reached through the app handle in router state.
    let app = st.app.clone();
    let method = method.to_string();
    let authed_device_id = device_id.clone();
    let verify_query = query.clone();
    let verify_body = body_bytes.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let settings = app.state::<SettingsState>();
        sync::pairing::verify_request(
            &settings,
            &device_id,
            &method,
            &path,
            &verify_query,
            &verify_body,
            peer_protocol,
            &ts,
            &sig,
        )
    })
    .await
    .unwrap_or_else(|_| Err(sync::pairing::AuthError::BadSignature));
    match result {
        Ok(bound_user_id) => {
            // Paired device: expose its identity binding to the handler so
            // writeback can enforce that changes land under the bound user.
            // Also capture the declared role/cluster/protocol headers for the
            // status surface (§3.1). A cluster mismatch is logged, not yet
            // hard-rejected — the client enforces its own refusal + TOFU
            // adoption (docs/my_sync_design.md §7).
            let peer_role = get_header("x-fms-role");
            let peer_cluster = get_header("x-fms-cluster");
            let settings = st.app.state::<SettingsState>();
            let our_cluster = settings.cluster_id();
            if !peer_cluster.is_empty() && !our_cluster.is_empty() && peer_cluster != our_cluster {
                log::warn!(
                    "[zone_guard] device {} declared cluster '{}' but this hub is '{}' (mis-pairing?)",
                    authed_device_id, peer_cluster, our_cluster
                );
            }
            req.extensions_mut().insert(sync::pairing::AuthContext {
                device_id: Some(authed_device_id),
                bound_user_id,
                role: if peer_role.is_empty() { None } else { Some(peer_role) },
                cluster_id: if peer_cluster.is_empty() { None } else { Some(peer_cluster) },
                protocol_version: peer_protocol,
            });
            next.run(req).await
        }
        Err(e) => json_error(
            StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::FORBIDDEN),
            e.message(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Index — rich dashboard
// ---------------------------------------------------------------------------

/// Shared stylesheet for the embedded HTML pages, reusing the app's Light
/// theme tokens and the typography patterns of markdown.css.
const PAGE_STYLE: &str = r#"
:root{--bg-body:#fbfbfb;--bg-card:#ffffff;--bg-muted:#f0f0f0;--text-primary:#0f0f0f;--text-secondary:#666666;--border-default:#e0e0e0;--border-light:#cccccc;--accent:#0EA89A;--accent-hover:#0c9488;--success-bg:#c6f0d0;--success-text:#1e7e34}
*{box-sizing:border-box}
body{font-family:system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:var(--bg-body);color:var(--text-primary);line-height:1.7}
.container{max-width:960px;margin:0 auto;padding:32px 24px}
header{margin-bottom:24px}
h1{font-size:2em;font-weight:800;color:var(--accent);margin:0 0 8px;line-height:1.2;border-bottom:2px solid var(--accent);padding-bottom:.25em;display:flex;align-items:center;gap:10px}
h1 .badge{font-size:11px;padding:2px 8px;border-radius:10px;background:var(--success-bg);color:var(--success-text);font-weight:600}
.subtitle{color:var(--text-secondary);font-size:14px;margin:8px 0 0;border:none}
h3{font-size:1.25em;font-weight:600;color:var(--accent);margin:1.2em 0 .4em 0;line-height:1.3}
.card{background:var(--bg-card);border:1px solid var(--border-default);border-radius:10px;padding:20px;margin-bottom:20px}
details{background:var(--bg-card);border:1px solid var(--border-default);border-radius:10px;margin-bottom:16px}
summary{display:flex;align-items:center;gap:8px;padding:14px 20px;cursor:pointer;font-size:1.5em;font-weight:700;color:var(--accent);line-height:1.3;list-style:none;user-select:none}
summary::-webkit-details-marker{display:none}
summary::before{content:"\25B8";font-size:.6em;transition:transform .15s}
details[open]>summary::before{transform:rotate(90deg)}
details[open]>summary{border-bottom:1px solid rgba(14,168,154,.35);margin-bottom:12px}
.sec-body{padding:0 20px 16px}
table{border-collapse:collapse;width:100%;font-size:.95em;margin:.75em 0}
th,td{text-align:left;padding:.5em .75em;border:1px solid var(--border-default);word-break:break-word;vertical-align:top}
th{font-weight:600;color:var(--accent);background:rgba(14,168,154,.1);font-size:12px;text-transform:uppercase;letter-spacing:.5px;text-align:left}
tbody tr:nth-child(even){background:#f8f8f8}
tbody tr:hover{background:rgba(14,168,154,.06)}
code{font-family:'JetBrains Mono','Fira Code','Cascadia Code',ui-monospace,monospace;font-size:.9em;background:var(--bg-muted);padding:.15em .4em;border-radius:4px;border:1px solid var(--border-light)}
pre{background:var(--bg-muted);border:1px solid var(--border-default);border-radius:6px;padding:.75em 1em;margin:.75em 0;white-space:pre-wrap;overflow-wrap:break-word;font-size:13px;max-height:300px;overflow-y:auto;line-height:1.5}
a{color:var(--accent);text-decoration:underline;text-decoration-color:rgba(14,168,154,.4);text-underline-offset:2px}
a:hover{text-decoration-color:var(--accent)}
.muted{color:var(--text-secondary)}
.method{display:inline-block;font-family:monospace;font-size:11px;font-weight:600;padding:2px 6px;border-radius:4px;background:var(--bg-muted);border:1px solid var(--border-default);color:var(--text-secondary)}
.method.get{color:var(--success-text);border-color:rgba(30,126,52,.4)}
.method.post{color:#b45309;border-color:rgba(180,83,9,.4)}
.tag{display:inline-block;font-size:11px;padding:2px 8px;border-radius:10px;border:1px solid var(--border-default);color:var(--text-secondary);margin-right:4px;background:var(--bg-card)}
.tag.on{border-color:var(--accent);color:var(--accent)}
form label{display:block;font-size:13px;color:var(--text-secondary);margin:10px 0 4px}
form input[type=file],form input[type=text],form textarea,form select{width:100%;padding:8px 10px;border-radius:6px;border:1px solid var(--border-light);background:var(--bg-card);color:var(--text-primary);font-size:13px}
form button{margin-top:12px;padding:8px 20px;border-radius:6px;border:0;background:var(--accent);color:#fff;font-size:13px;font-weight:600;cursor:pointer}
form button:hover{background:var(--accent-hover)}
.tool-list{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:8px;margin-top:8px}
.tool-item{padding:8px 10px;border-radius:6px;border:1px solid var(--border-default);background:var(--bg-muted);font-size:12px}
.tool-item .name{font-weight:600;color:var(--accent);margin-bottom:2px}
.tool-item .desc{color:var(--text-secondary);font-size:11px}
.status-bar{display:flex;gap:6px;flex-wrap:wrap;margin-bottom:20px}
.footer{text-align:center;color:var(--text-secondary);font-size:12px;margin-top:40px;padding-top:20px;border-top:1px solid var(--border-default)}
.crumb{margin:0 0 16px;font-size:14px}
.chap-tree a{display:block;padding:3px 0}
.chapter-content{font-size:1.05em;max-width:720px}
.chapter-content .para{margin:0 0 1.8em 0}
.chapter-content .sent{display:block;line-height:1.9}
"#;

async fn index_handler(State(s): State<AppState>) -> Response {
    let c = &s.config;
    let port = s.app.state::<WebServiceState>().bound_port.lock().unwrap().unwrap_or(DEFAULT_PORT);

    // Fetch datasets for the dashboard.
    let settings = s.app.state::<SettingsState>();
    let datasets = datasets::list_datasets(&settings);

    // --- Build sections dynamically based on enabled services ---
    let mut services_html = String::new();

    if c.stt {
        services_html.push_str(STT_SECTION_HTML);
    }
    if c.tts {
        services_html.push_str(TTS_SECTION_HTML);
    }
    if c.dataset {
        let mut ds_rows = String::new();
        for d in &datasets {
            ds_rows.push_str(&format!(
                "<tr><td><a href=\"/datasets/{}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                html_escape(&d.info.uuid),
                html_escape(&d.info.name),
                html_escape(&d.info.description),
                d.status,
                d.media_count,
            ));
        }
        if ds_rows.is_empty() {
            ds_rows = "<tr><td colspan=\"4\" class=\"muted\">No datasets found.</td></tr>".to_string();
        }
        services_html.push_str(&DATASET_SECTION_HTML.replace("{}", &ds_rows));
    }

    // Books section (always shown) — read-only reading library browser.
    let books = datasets::book::list_books(&settings);
    let mut book_rows = String::new();
    for b in &books {
        book_rows.push_str(&format!(
            "<tr><td><a href=\"/books/{}\">{}</a></td><td>{}</td></tr>",
            html_escape(&b.uuid),
            html_escape(&b.title),
            html_escape(&b.updated_at),
        ));
    }
    if book_rows.is_empty() {
        book_rows = "<tr><td colspan=\"2\" class=\"muted\">No books found.</td></tr>".to_string();
    }
    services_html.push_str(&BOOK_SECTION_HTML.replace("{}", &book_rows));

    // MCP section (always shown).
    services_html.push_str(MCP_SECTION_HTML);

    // --- Endpoint reference table ---
    let mut endpoint_rows = String::new();
    if c.stt {
        endpoint_rows.push_str(&ep_row("STT", "GET", "/stt", "Browser test form for uploading WAV audio"));
        endpoint_rows.push_str(&ep_row("STT", "POST", "/stt", "multipart <code>file</code> = 16kHz mono WAV &rarr; JSON <code>{ text }</code>"));
    }
    if c.dataset {
        endpoint_rows.push_str(&ep_row("Dataset", "GET", "/datasets", "JSON list of all datasets"));
        endpoint_rows.push_str(&ep_row("Dataset", "GET", "/datasets/{uuid}", "HTML file browser for a dataset"));
        endpoint_rows.push_str(&ep_row("Dataset", "GET", "/datasets/{uuid}/list", "JSON recursive file listing"));
        endpoint_rows.push_str(&ep_row("Dataset", "GET", "/datasets/{uuid}/file/{path}", "Download / stream a file"));
    }
    if c.tts {
        endpoint_rows.push_str(&ep_row("TTS", "GET", "/tts?text=...&voice=...", "Synthesize speech &rarr; audio/mpeg"));
        endpoint_rows.push_str(&ep_row("TTS", "POST", "/tts", "JSON <code>{ text, voice, rate?, volume?, pitch? }</code> &rarr; audio/mpeg"));
        endpoint_rows.push_str(&ep_row("TTS", "GET", "/tts/voices", "JSON list of all Edge TTS voices"));
    }
    endpoint_rows.push_str(&ep_row("Book", "GET", "/books", "HTML list of all books in the reading library"));
    endpoint_rows.push_str(&ep_row("Book", "GET", "/books/{uuid}", "HTML chapter list for a book"));
    endpoint_rows.push_str(&ep_row("Book", "GET", "/books/{uuid}/{chapter}", "HTML chapter content (one sentence per line)"));
    endpoint_rows.push_str(&ep_row("MCP", "POST", "/mcp", "Streamable HTTP MCP endpoint (rmcp v3.4)"));

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>fms-app</title>
<style>{PAGE_STYLE}</style>
</head>
<body>
<div class="container">
<header>
<h1>fms-app <span class="badge">v2</span></h1>
<p class="subtitle">Desktop language-learning app &mdash; HTTP API running on port <strong>{port}</strong></p>
</header>

<div class="status-bar">
<span class="tag {stt_tag}">STT</span>
<span class="tag {dataset_tag}">Datasets</span>
<span class="tag {tts_tag}">TTS</span>
<span class="tag on">Books</span>
<span class="tag on">MCP</span>
</div>

{services_html}

<details>
<summary>API Reference</summary>
<div class="sec-body">
<table>
<thead><tr><th>Service</th><th>Method</th><th>Path</th><th>Description</th></tr></thead>
<tbody>{endpoint_rows}</tbody>
</table>
</div>
</details>

<div class="footer">
<p>fms-app &mdash; built with Tauri, axum, and Rust</p>
</div>
</div>
</body>
</html>"#,
        port = port,
        stt_tag = if c.stt { "tag on" } else { "tag" },
        dataset_tag = if c.dataset { "tag on" } else { "tag" },
        tts_tag = if c.tts { "tag on" } else { "tag" },
        services_html = services_html,
        endpoint_rows = endpoint_rows,
    );
    Html(html).into_response()
}

fn ep_row(service: &str, method: &str, path: &str, desc: &str) -> String {
    let mclass = match method {
        "GET" => "method get",
        "POST" => "method post",
        _ => "method",
    };
    format!(
        "<tr><td><strong>{}</strong></td><td><span class=\"{}\">{}</span></td><td><code>{}</code></td><td>{}</td></tr>",
        service, mclass, method, path, desc
    )
}

// --- Static HTML fragments for interactive sections ---

const STT_SECTION_HTML: &str = r#"
<details>
<summary>&#x1f3a4; Speech-to-Text</summary>
<div class="sec-body">
<p class="muted" style="margin:0 0 12px;font-size:13px">Upload a 16kHz mono PCM WAV file. The downloaded STT model is loaded automatically.</p>
<form id="stt-form">
<label>Audio file (WAV)</label>
<input type="file" name="file" accept=".wav,audio/wav" required>
<button type="submit">Transcribe</button>
</form>
<pre id="stt-out" style="margin-top:12px">Result will appear here.</pre>
<script>
document.getElementById('stt-form').addEventListener('submit',async e=>{
  e.preventDefault();
  const out=document.getElementById('stt-out');
  const fd=new FormData(e.target);
  out.textContent='Transcribing...';
  try{const r=await fetch('/stt',{method:'POST',body:fd});const j=await r.json();out.textContent=j.text||j.error||JSON.stringify(j);}
  catch(err){out.textContent='Error: '+err;}
});
</script>
</div>
</details>"#;

const TTS_SECTION_HTML: &str = r#"
<details>
<summary>&#x1f50a; Text-to-Speech</summary>
<div class="sec-body">
<p class="muted" style="margin:0 0 12px;font-size:13px">Type text and pick a voice to synthesize speech via Edge TTS.</p>
<form id="tts-form">
<label>Text</label>
<textarea name="text" rows="3" placeholder="Hello, world!" required></textarea>
<label>Voice</label>
<select name="voice" id="tts-voice"><option value="">Loading voices...</option></select>
<button type="submit">Synthesize &amp; Play</button>
</form>
<pre id="tts-out" style="margin-top:12px;display:none"></pre>
<script>
(async()=>{
  try{const r=await fetch('/tts/voices');const v=await r.json();
    const sel=document.getElementById('tts-voice');sel.innerHTML='';
    const groups={};v.forEach(x=>{const loc=x.Locale||x.locale||'other';(groups[loc]=groups[loc]||[]).push(x);});
    Object.keys(groups).sort().forEach(loc=>{
      const og=document.createElement('optgroup');og.label=loc;
      groups[loc].forEach(x=>{const o=document.createElement('option');o.value=x.ShortName||x.short_name;o.textContent=(x.ShortName||x.short_name)+' ('+( x.Gender||x.gender||'')+ ')';og.appendChild(o);});
      sel.appendChild(og);
    });
  }catch(e){document.getElementById('tts-voice').innerHTML='<option>Failed to load</option>';}
})();
document.getElementById('tts-form').addEventListener('submit',async e=>{
  e.preventDefault();
  const out=document.getElementById('tts-out');
  const fd=new FormData(e.target);
  const text=fd.get('text'),voice=fd.get('voice');
  out.style.display='block';out.textContent='Synthesizing...';
  try{
    const r=await fetch('/tts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({text,voice})});
    if(!r.ok)throw new Error(await r.text());
    const blob=await r.blob();const url=URL.createObjectURL(blob);
    out.innerHTML='<audio controls src="'+url+'" autoplay></audio>';
  }catch(err){out.textContent='Error: '+err;}
});
</script>
</div>
</details>"#;

const DATASET_SECTION_HTML: &str = r#"
<details>
<summary>&#x1f4c2; Datasets</summary>
<div class="sec-body">
<p class="muted" style="margin:0 0 12px;font-size:13px">Browse dataset files. Click a dataset name to see its contents.</p>
<table>
<thead><tr><th>Name</th><th>Description</th><th>Status</th><th>Media</th></tr></thead>
<tbody>{}</tbody>
</table>
</div>
</details>"#;

const BOOK_SECTION_HTML: &str = r#"
<details>
<summary>&#x1f4d6; Books</summary>
<div class="sec-body">
<p class="muted" style="margin:0 0 12px;font-size:13px">Reading library. Click a book title to browse its chapters.</p>
<table>
<thead><tr><th>Title</th><th>Last updated</th></tr></thead>
<tbody>{}</tbody>
</table>
</div>
</details>"#;

const MCP_SECTION_HTML: &str = r#"
<details>
<summary>&#x1f916; MCP (Model Context Protocol)</summary>
<div class="sec-body">
<p class="muted" style="margin:0 0 12px;font-size:13px">
Connect AI agents via Streamable HTTP at <code>/mcp</code>. Built with <strong>rmcp v3.4</strong>.
</p>
<div class="tool-list">
<div class="tool-item"><div class="name">dataset_list</div><div class="desc">List all datasets with UUID, name, status</div></div>
<div class="tool-item"><div class="name">dataset_get</div><div class="desc">Get detailed dataset info and artifacts</div></div>
<div class="tool-item"><div class="name">dataset_read_file</div><div class="desc">Read a file from a dataset directory</div></div>
<div class="tool-item"><div class="name">dataset_list_files</div><div class="desc">Recursive file listing with sizes</div></div>
<div class="tool-item"><div class="name">dataset_list_subtitles</div><div class="desc">List subtitles with cue counts</div></div>
<div class="tool-item"><div class="name">dataset_list_cues</div><div class="desc">List cues for a subtitle</div></div>
<div class="tool-item"><div class="name">dataset_get_summary</div><div class="desc">Per-media breakdown summary</div></div>
<div class="tool-item"><div class="name">dataset_generate_subtitles</div><div class="desc">Generate subtitles via STT model</div></div>
<div class="tool-item"><div class="name">dataset_generate_waveforms</div><div class="desc">Generate waveform JSON files</div></div>
<div class="tool-item"><div class="name">dataset_write_subtitles_to_db</div><div class="desc">Re-import VTT into SQLite</div></div>
<div class="tool-item"><div class="name">dataset_delete_subtitles</div><div class="desc">Delete subtitles (VTT + DB)</div></div>
<div class="tool-item"><div class="name">dataset_delete_waveforms</div><div class="desc">Delete waveform files</div></div>
<div class="tool-item"><div class="name">dataset_delete_database</div><div class="desc">Delete dataset SQLite database</div></div>
<div class="tool-item"><div class="name">tts_list_voices</div><div class="desc">List all Edge TTS voices</div></div>
<div class="tool-item"><div class="name">tts_synthesize</div><div class="desc">Synthesize text to audio file</div></div>
<div class="tool-item"><div class="name">ocr_recognize</div><div class="desc">Tesseract OCR on base64 image</div></div>
<div class="tool-item"><div class="name">ocr_list_languages</div><div class="desc">List installed Tesseract languages</div></div>
<div class="tool-item"><div class="name">capture_screenshot</div><div class="desc">Capture screen via native snipping tool</div></div>
<div class="tool-item"><div class="name">web_service_get_status</div><div class="desc">Get web service status and URLs</div></div>
<div class="tool-item"><div class="name">web_service_start</div><div class="desc">Start web service with config</div></div>
<div class="tool-item"><div class="name">web_service_stop</div><div class="desc">Stop the web service</div></div>
<div class="tool-item"><div class="name">log_get_history</div><div class="desc">Get recent log entries</div></div>
<div class="tool-item"><div class="name">log_clear</div><div class="desc">Clear the log buffer</div></div>
<div class="tool-item"><div class="name">log_get_file_path</div><div class="desc">Path of the persisted log file</div></div>
<div class="tool-item"><div class="name">log_read_file_history</div><div class="desc">Read archived logs from file</div></div>
</div>
</div>
</details>"#;

// ---------------------------------------------------------------------------
// STT
// ---------------------------------------------------------------------------

async fn stt_form(State(_s): State<AppState>) -> Response {
    let html = r##"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>STT test</title><style>"##.to_string()
        + PAGE_STYLE
        + r##"</style></head><body>
<div class="container">
<h1>Speech-to-Text test</h1>
<p class="muted" style="font-size:13px">Upload a 16kHz mono PCM WAV file. The selected STT model is loaded automatically.</p>
<form id="f"><input type="file" name="file" accept=".wav,audio/wav" required><br><button type="submit">Transcribe</button></form>
<pre id="out">Result will appear here.</pre>
<script>
document.getElementById('f').addEventListener('submit', async (e) => {
  e.preventDefault();
  const out = document.getElementById('out');
  const fd = new FormData(e.target);
  out.textContent = 'Transcribing...';
  try {
    const r = await fetch('/stt', { method: 'POST', body: fd });
    const j = await r.json();
    out.textContent = j.text || j.error || JSON.stringify(j);
  } catch (err) { out.textContent = 'Error: ' + err; }
});
</script>
</div>
</body></html>"##;
    Html(html).into_response()
}

async fn stt_handler(State(s): State<AppState>, mut multipart: Multipart) -> Response {
    log::info!("[web_service] STT request received");
    let mut wav: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        let is_file = field.file_name().is_some()
            || matches!(field.name(), Some("file") | Some("audio"));
        if is_file {
            if let Ok(bytes) = field.bytes().await {
                wav = Some(bytes.to_vec());
                break;
            }
        }
    }

    let Some(wav) = wav else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "No audio file uploaded. Send a multipart/form-data field named 'file' containing 16kHz mono PCM WAV.",
        );
    };

    let app = s.app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let model_state = app.state::<ModelState>();
        models::transcribe_wav_bytes(&model_state, &wav)
    })
    .await;

    match joined {
        Ok(Ok(text)) => {
            log::info!("[web_service] STT success: {} chars", text.len());
            (StatusCode::OK, Json(serde_json::json!({ "text": text }))).into_response()
        }
        Ok(Err(e)) => {
            log::error!("[web_service] STT failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, &e)
        }
        Err(e) => {
            log::error!("[web_service] STT task panicked: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Datasets
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct DatasetEntry {
    uuid: String,
    name: String,
    description: String,
    status: String,
    media_count: usize,
}

#[derive(Serialize)]
struct FileEntry {
    path: String,
    size: u64,
}

async fn datasets_list(State(s): State<AppState>) -> Response {
    log::debug!("[web_service] GET /datasets");
    let settings = s.app.state::<SettingsState>();
    let datasets: Vec<DatasetSummary> = datasets::list_datasets(&settings);
    let entries: Vec<DatasetEntry> = datasets
        .into_iter()
        .map(|d| DatasetEntry {
            uuid: d.info.uuid.clone(),
            name: d.info.name.clone(),
            description: d.info.description.clone(),
            status: d.status.clone(),
            media_count: d.media_count,
        })
        .collect();
    (StatusCode::OK, Json(entries)).into_response()
}

async fn dataset_detail(State(s): State<AppState>, AxPath(uuid): AxPath<String>) -> Response {
    log::debug!("[web_service] GET /datasets/{}", uuid);
    let settings = s.app.state::<SettingsState>();
    let dir = match datasets::find_dataset_dir(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };

    let mut files = Vec::new();
    collect_files(&dir, &dir, &mut files);

    let mut rows = String::new();
    for f in &files {
        let href = format!("/datasets/{}/file/{}", uuid, url_encode_path(&f.path));
        rows.push_str(&format!(
            "<tr><td><a href=\"{}\">{}</a></td><td>{}</td></tr>",
            href,
            html_escape(&f.path),
            human_size(f.size)
        ));
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"2\" class=\"muted\">No files.</td></tr>");
    }

    let html = format!(
        r##"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Dataset {uuid}</title><style>{PAGE_STYLE}</style></head><body>
<div class="container">
<p><a href="/datasets">&larr; all datasets</a></p>
<h1>Dataset <code>{uuid}</code></h1>
<details>
<summary>Files</summary>
<div class="sec-body">
<table><thead><tr><th>File</th><th>Size</th></tr></thead><tbody>{rows}</tbody></table>
</div>
</details>
</div>
</body></html>"##
    );
    Html(html).into_response()
}

async fn dataset_file_list(State(s): State<AppState>, AxPath(uuid): AxPath<String>) -> Response {
    let settings = s.app.state::<SettingsState>();
    let dir = match datasets::find_dataset_dir(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    let mut files = Vec::new();
    collect_files(&dir, &dir, &mut files);
    (StatusCode::OK, Json(files)).into_response()
}

async fn dataset_file(
    State(s): State<AppState>,
    headers: HeaderMap,
    AxPath((uuid, rel)): AxPath<(String, String)>,
) -> Response {
    let settings = s.app.state::<SettingsState>();
    let dir = match datasets::find_dataset_dir(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };

    let rel_clean = rel.trim_start_matches('/');
    let target = dir.join(rel_clean);

    // Prevent path traversal: canonicalized target must stay inside the dataset dir.
    let base_c = match dir.canonicalize() {
        Ok(p) => p,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let target_c = match target.canonicalize() {
        Ok(p) => p,
        Err(_) => return json_error(StatusCode::NOT_FOUND, "File not found"),
    };
    if !target_c.starts_with(&base_c) {
        return json_error(StatusCode::FORBIDDEN, "Access denied");
    }
    if !target_c.is_file() {
        return json_error(StatusCode::NOT_FOUND, "Not a file");
    }

    stream_file(&target_c, &headers).await
}

/// Parse a single `Range: bytes=start-end` header against a known length.
/// Open-ended ranges (`bytes=5-`, `bytes=-100`) and suffix lengths are handled.
fn parse_range(headers: &HeaderMap, len: u64) -> Option<(u64, u64)> {
    let raw = headers.get(header::RANGE)?.to_str().ok()?;
    let spec = raw.trim().strip_prefix("bytes=")?;
    let part = spec.split(',').next()?.trim();
    let (start_s, end_s) = part.split_once('-')?;
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let (start, end) = if start_s.is_empty() {
        // suffix: last N bytes
        let n: u64 = end_s.parse().ok()?;
        if n == 0 {
            return None;
        }
        (last.saturating_sub(n - 1), last)
    } else {
        let start: u64 = start_s.parse().ok()?;
        let end: u64 = if end_s.is_empty() { last } else { end_s.parse().ok()? };
        (start, end.min(last))
    };
    if start > end || start > last {
        return None;
    }
    Some((start, end))
}

pub(crate) async fn stream_file(path: &Path, req_headers: &HeaderMap) -> Response {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let ct = mime_for(path);

    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let len = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    // Range request -> 206 Partial Content (lets a browser media element seek).
    if let Some((start, end)) = parse_range(req_headers, len) {
        if let Err(e) = file.seek(SeekFrom::Start(start)).await {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
        let take = end - start + 1;
        let stream = ReaderStream::new(file.take(take));
        let body = Body::from_stream(stream);

        let mut headers = HeaderMap::new();
        if let Ok(v) = ct.parse() {
            headers.insert(header::CONTENT_TYPE, v);
        }
        let _ = format!("inline; filename=\"{}\"", name.replace('"', "")).parse::<axum::http::HeaderValue>().map(|v| headers.insert(header::CONTENT_DISPOSITION, v));
        headers.insert(header::ACCEPT_RANGES, header::HeaderValue::from_static("bytes"));
        if let Ok(v) = format!("bytes {}-{}/{}", start, end, len).parse() {
            headers.insert(header::CONTENT_RANGE, v);
        }
        if let Ok(v) = take.to_string().parse() {
            headers.insert(header::CONTENT_LENGTH, v);
        }
        return (StatusCode::PARTIAL_CONTENT, headers, body).into_response();
    }

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    if let Ok(v) = ct.parse() {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = format!("inline; filename=\"{}\"", name.replace('"', "")).parse() {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    headers.insert(header::ACCEPT_RANGES, header::HeaderValue::from_static("bytes"));
    if let Ok(v) = len.to_string().parse() {
        headers.insert(header::CONTENT_LENGTH, v);
    }

    (StatusCode::OK, headers, body).into_response()
}

// ---------------------------------------------------------------------------
// UDP discovery responder (desktop): beacon + probe reply for the Android client
// ---------------------------------------------------------------------------

/// Bind UDP `0.0.0.0:35712`, reply to `fms-probe` datagrams, and broadcast a
/// JSON beacon every 30 s. Degrades to probe-only where broadcast is blocked.
pub fn spawn_discovery(app: AppHandle, http_port: u16) {
    tauri::async_runtime::spawn(async move {
        let socket = match tokio::net::UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)).await {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[discovery] UDP bind on {} failed: {} (Tailscale /24 probe still works)",
                    DISCOVERY_PORT,
                    e
                );
                return;
            }
        };
        let _ = socket.set_broadcast(true);

        let name = app.package_info().name.clone();
        // Beacon is rebuilt each tick from live settings so a role/cluster
        // change (designating this workspace a hub) is reflected without a
        // restart. Every desktop runs the web service, so the beacon must say
        // whether *this* one is a hub or a follower (§3.1) — the phone lists
        // only hubs as connect targets.
        let make_beacon = |app: &AppHandle| -> String {
            let settings = app.state::<SettingsState>();
            serde_json::json!({
                "app": "fms-app",
                "name": name,
                "http_port": http_port,
                "ips": [local_ip().unwrap_or_default()],
                "role": settings.role(),
                "cluster_id": settings.cluster_id(),
                "protocol_version": crate::sync::PROTOCOL_VERSION,
            })
            .to_string()
        };
        let mut beacon = make_beacon(&app);
        let bcast = format!("255.255.255.255:{}", DISCOVERY_PORT);
        let mcast = format!("224.0.1.87:{}", DISCOVERY_PORT);
        let mut buf = [0u8; 2048];
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    beacon = make_beacon(&app);
                    let _ = socket.send_to(beacon.as_bytes(), bcast.as_str()).await;
                    let _ = socket.send_to(beacon.as_bytes(), mcast.as_str()).await;
                }
                r = socket.recv_from(&mut buf) => {
                    if let Ok((n, peer)) = r {
                        let msg = String::from_utf8_lossy(&buf[..n]);
                        if msg.trim() == "fms-probe" {
                            let _ = socket.send_to(beacon.as_bytes(), peer).await;
                        }
                    }
                }
            }
        }
    });
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<FileEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            collect_files(base, &p, out);
        } else {
            let rel = p
                .strip_prefix(base)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(FileEntry { path: rel, size });
        }
    }
}

// ---------------------------------------------------------------------------
// TTS
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TtsRequest {
    text: String,
    voice: String,
    #[serde(default)]
    rate: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    pitch: Option<String>,
}

async fn tts_voices(State(_s): State<AppState>) -> Response {
    match edge_tts::edge_tts_list_voices().await {
        Ok(voices) => (StatusCode::OK, Json(voices)).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn tts_post(State(_s): State<AppState>, Json(req): Json<TtsRequest>) -> Response {
    synthesize_response(req).await
}

async fn tts_get(State(_s): State<AppState>, Query(req): Query<TtsRequest>) -> Response {
    synthesize_response(req).await
}

async fn synthesize_response(req: TtsRequest) -> Response {
    if req.text.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Missing 'text'");
    }
    if req.voice.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Missing 'voice'");
    }
    log::info!("[web_service] TTS synthesize: voice={}, text_len={}", req.voice, req.text.len());
    match edge_tts::synthesize_to_bytes(
        &req.text,
        &req.voice,
        req.rate,
        req.volume,
        req.pitch,
    )
    .await
    {
        Ok(audio) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "audio/mpeg".to_string()),
                (header::CONTENT_LENGTH, audio.len().to_string()),
            ],
            audio,
        )
            .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

// ---------------------------------------------------------------------------
// Books (reading library)
// ---------------------------------------------------------------------------

/// Wrap a body fragment in a full HTML document using the shared stylesheet.
fn render_page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>{PAGE_STYLE}</style></head><body>{body}</body></html>",
        title = html_escape(title),
        body = body,
    )
}

/// Flatten a chapter tree into `(depth, chapter)` pairs ordered like the
/// reading view: each parent followed immediately by its nested children.
/// Chapters with an unresolvable parent are appended at the top level so no
/// content is ever hidden.
fn flatten_chapters(chapters: &[BookChapter]) -> Vec<(usize, &BookChapter)> {
    fn walk<'a>(
        chapters: &'a [BookChapter],
        parent: Option<&str>,
        depth: usize,
        out: &mut Vec<(usize, &'a BookChapter)>,
    ) {
        let mut children: Vec<&BookChapter> = chapters
            .iter()
            .filter(|c| c.parent_uuid.as_deref().filter(|s| !s.is_empty()) == parent)
            .collect();
        children.sort_by_key(|c| c.order_num);
        for c in children {
            out.push((depth, c));
            walk(chapters, Some(c.uuid.as_str()), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(chapters, None, 0, &mut out);
    let visited: std::collections::HashSet<&str> =
        out.iter().map(|(_, c)| c.uuid.as_str()).collect();
    for c in chapters.iter().filter(|c| !visited.contains(c.uuid.as_str())) {
        out.push((0, c));
    }
    out
}

async fn books_list(State(s): State<AppState>) -> Response {
    log::debug!("[web_service] GET /books");
    let settings = s.app.state::<SettingsState>();
    let books = datasets::book::list_books(&settings);

    let mut rows = String::new();
    for b in &books {
        rows.push_str(&format!(
            "<tr><td><a href=\"/books/{}\">{}</a></td><td>{}</td></tr>",
            url_encode_path(&b.uuid),
            html_escape(&b.title),
            html_escape(&b.updated_at),
        ));
    }
    if rows.is_empty() {
        rows = "<tr><td colspan=\"2\" class=\"muted\">No books found.</td></tr>".to_string();
    }

    let body = format!(
        "<div class=\"container\">\
         <p class=\"crumb\"><a href=\"/\">&larr; Home</a></p>\
         <h1>Books</h1>\
         <p class=\"subtitle\">Reading library &mdash; {} book(s). Select a book to view its chapters.</p>\
         <div class=\"card\">\
         <table><thead><tr><th>Title</th><th>Last updated</th></tr></thead><tbody>{}</tbody></table>\
         </div></div>",
        books.len(),
        rows,
    );
    Html(render_page("Books", &body)).into_response()
}

async fn book_chapters_page(State(s): State<AppState>, AxPath(uuid): AxPath<String>) -> Response {
    log::debug!("[web_service] GET /books/{}", uuid);
    let settings = s.app.state::<SettingsState>();
    let chapters = match datasets::book::list_chapters(&settings, &uuid) {
        Ok(c) => c,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    let book_title = datasets::book::list_books(&settings)
        .into_iter()
        .find(|b| b.uuid == uuid)
        .map(|b| b.title)
        .unwrap_or_else(|| uuid.clone());

    let mut links = String::new();
    for (depth, c) in flatten_chapters(&chapters) {
        links.push_str(&format!(
            "<a style=\"padding-left:{}px\" href=\"/books/{}/{}\">{}{}</a>",
            depth * 20,
            url_encode_path(&uuid),
            url_encode_path(&c.uuid),
            if depth > 0 { "&#9492; " } else { "" },
            html_escape(&c.title),
        ));
    }
    if links.is_empty() {
        links = "<p class=\"muted\">No chapters in this book.</p>".to_string();
    }

    let body = format!(
        "<div class=\"container\">\
         <p class=\"crumb\"><a href=\"/books\">&larr; All books</a></p>\
         <h1>{}</h1>\
         <p class=\"subtitle\">Select a chapter to read its content.</p>\
         <div class=\"card chap-tree\">{}</div></div>",
        html_escape(&book_title),
        links,
    );
    Html(render_page("Chapters", &body)).into_response()
}

async fn book_chapter_content(
    State(s): State<AppState>,
    AxPath((uuid, chapter)): AxPath<(String, String)>,
) -> Response {
    log::debug!("[web_service] GET /books/{}/{}", uuid, chapter);
    let settings = s.app.state::<SettingsState>();
    let sentences = match datasets::book::list_sentences(&settings, &uuid, &chapter) {
        Ok(x) => x,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    let chapter_title = datasets::book::list_chapters(&settings, &uuid)
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.uuid == chapter)
        .map(|c| c.title)
        .unwrap_or_else(|| "Chapter".to_string());
    let book_title = datasets::book::list_books(&settings)
        .into_iter()
        .find(|b| b.uuid == uuid)
        .map(|b| b.title)
        .unwrap_or_else(|| uuid.clone());

    // Group sentences into paragraphs split on `paragraph_break` rows; each
    // text sentence renders on its own line, with a larger gap between paragraphs.
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for sent in &sentences {
        if sent.sentence_type == "paragraph_break" {
            if !current.is_empty() {
                paragraphs.push(render_paragraph(&current));
                current.clear();
            }
        } else if !sent.content.trim().is_empty() {
            current.push(sent.content.clone());
        }
    }
    if !current.is_empty() {
        paragraphs.push(render_paragraph(&current));
    }

    let content = if paragraphs.is_empty() {
        "<p class=\"muted\">This chapter has no content yet.</p>".to_string()
    } else {
        format!("<div class=\"chapter-content\">{}</div>", paragraphs.join(""))
    };

    let body = format!(
        "<div class=\"container\">\
         <p class=\"crumb\"><a href=\"/books\">&larr; All books</a> &middot; \
         <a href=\"/books/{}\">{}</a></p>\
         <h1>{}</h1>\
         <div style=\"margin-top:16px\">{}</div></div>",
        url_encode_path(&uuid),
        html_escape(&book_title),
        html_escape(&chapter_title),
        content,
    );
    Html(render_page(&chapter_title, &body)).into_response()
}

/// Render one paragraph: each sentence becomes a block-level line.
fn render_paragraph(lines: &[String]) -> String {
    let inner: String = lines
        .iter()
        .map(|l| format!("<span class=\"sent\">{}</span>", html_escape(l)))
        .collect();
    format!("<p class=\"para\">{}</p>", inner)
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn json_error(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

/// Best-effort LAN IP detection via a UDP socket (no packets are actually sent).
fn local_ip() -> Option<String> {
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    Some(sock.local_addr().ok()?.ip().to_string())
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a") | Some("m4b") => "audio/mp4",
        Some("ogg") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("vtt") => "text/vtt; charset=utf-8",
        Some("json") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        _ => "application/octet-stream",
    }
}

fn url_encode_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}
