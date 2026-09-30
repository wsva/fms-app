use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Multipart, Path as AxPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;

use crate::dataset::{self, DatasetSummary};
use crate::edge_tts;
use crate::model::{self, ModelState};
use crate::settings::SettingsState;

/// Default port the web server binds to.
const DEFAULT_PORT: u16 = 8787;

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
}

impl Default for WebServiceConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            stt: true,
            dataset: true,
            tts: true,
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
            let _ = axum::serve(listener, router)
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
            };
        }
        *self.bound_port.lock().unwrap() = Some(bound);
        *self.shutdown.lock().unwrap() = Some(tx);
        self.running.store(true, Ordering::SeqCst);

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
        let _ = axum::serve(listener, router)
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
        };
    }
    *state.bound_port.lock().unwrap() = Some(bound);
    *state.shutdown.lock().unwrap() = Some(tx);
    state.running.store(true, Ordering::SeqCst);

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

    r.with_state(state)
}

// ---------------------------------------------------------------------------
// Index — rich dashboard
// ---------------------------------------------------------------------------

async fn index_handler(State(s): State<AppState>) -> Response {
    let c = &s.config;
    let port = s.app.state::<WebServiceState>().bound_port.lock().unwrap().unwrap_or(DEFAULT_PORT);

    // Fetch datasets for the dashboard.
    let settings = s.app.state::<SettingsState>();
    let datasets = dataset::list_datasets(&settings);

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
    endpoint_rows.push_str(&ep_row("MCP", "POST", "/mcp", "Streamable HTTP MCP endpoint (rmcp v3.4)"));

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>fms-app</title>
<style>
:root{{--bg:#0f1115;--card:#161920;--border:#23262d;--text:#e6e6e6;--muted:#9aa0a6;--accent:#6cb6ff;--green:#4ade80;--code-bg:#1b1e24}}
*{{box-sizing:border-box}}
body{{font-family:system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;margin:0;padding:0;background:var(--bg);color:var(--text);line-height:1.6}}
.container{{max-width:960px;margin:0 auto;padding:32px 24px}}
header{{margin-bottom:32px}}
h1{{font-size:24px;margin:0 0 4px;display:flex;align-items:center;gap:10px}}
h1 .badge{{font-size:11px;padding:2px 8px;border-radius:10px;background:var(--green);color:#000;font-weight:600}}
.subtitle{{color:var(--muted);font-size:14px;margin:4px 0 0}}
h2{{font-size:16px;margin:0 0 12px;color:var(--text)}}
.card{{background:var(--card);border:1px solid var(--border);border-radius:10px;padding:20px;margin-bottom:20px}}
.card h2{{display:flex;align-items:center;gap:8px}}
table{{border-collapse:collapse;width:100%;font-size:13px}}
th,td{{text-align:left;padding:8px 10px;border-bottom:1px solid var(--border);vertical-align:top}}
th{{color:var(--muted);font-weight:600;font-size:12px;text-transform:uppercase;letter-spacing:.5px}}
code{{background:var(--code-bg);padding:2px 5px;border-radius:4px;font-size:12px}}
a{{color:var(--accent);text-decoration:none}}a:hover{{text-decoration:underline}}
.muted{{color:var(--muted)}}
.method{{display:inline-block;font-family:monospace;font-size:11px;font-weight:600;padding:2px 6px;border-radius:4px;background:var(--code-bg);border:1px solid var(--border);color:var(--muted)}}
.method.get{{color:#4ade80;border-color:#4ade8040}}
.method.post{{color:#f59e0b;border-color:#f59e0b40}}
.tag{{display:inline-block;font-size:11px;padding:2px 8px;border-radius:10px;border:1px solid var(--border);color:var(--muted);margin-right:4px}}
.tag.on{{border-color:var(--green);color:var(--green)}}
form label{{display:block;font-size:13px;color:var(--muted);margin:10px 0 4px}}
form input[type=file],form input[type=text],form textarea,form select{{width:100%;padding:8px 10px;border-radius:6px;border:1px solid var(--border);background:var(--code-bg);color:var(--text);font-size:13px}}
form button{{margin-top:12px;padding:8px 20px;border-radius:6px;border:0;background:#2563eb;color:#fff;font-size:13px;font-weight:600;cursor:pointer}}
form button:hover{{opacity:.9}}
pre{{background:var(--code-bg);padding:12px;border-radius:6px;white-space:pre-wrap;font-size:13px;max-height:300px;overflow-y:auto;border:1px solid var(--border)}}
.tool-list{{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:8px;margin-top:8px}}
.tool-item{{padding:8px 10px;border-radius:6px;border:1px solid var(--border);font-size:12px}}
.tool-item .name{{font-weight:600;color:var(--accent);margin-bottom:2px}}
.tool-item .desc{{color:var(--muted);font-size:11px}}
.status-bar{{display:flex;gap:6px;flex-wrap:wrap;margin-bottom:20px}}
section+section{{margin-top:24px}}
.footer{{text-align:center;color:var(--muted);font-size:12px;margin-top:40px;padding-top:20px;border-top:1px solid var(--border)}}
</style>
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
<span class="tag on">MCP</span>
</div>

{services_html}

<div class="card">
<h2>API Reference</h2>
<table>
<thead><tr><th>Service</th><th>Method</th><th>Path</th><th>Description</th></tr></thead>
<tbody>{endpoint_rows}</tbody>
</table>
</div>

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
<div class="card">
<h2>&#x1f3a4; Speech-to-Text</h2>
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
</div>"#;

const TTS_SECTION_HTML: &str = r#"
<div class="card">
<h2>&#x1f50a; Text-to-Speech</h2>
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
</div>"#;

const DATASET_SECTION_HTML: &str = r#"
<div class="card">
<h2>&#x1f4c2; Datasets</h2>
<p class="muted" style="margin:0 0 12px;font-size:13px">Browse dataset files. Click a dataset name to see its contents.</p>
<table>
<thead><tr><th>Name</th><th>Description</th><th>Status</th><th>Media</th></tr></thead>
<tbody>{}</tbody>
</table>
</div>"#;

const MCP_SECTION_HTML: &str = r#"
<div class="card">
<h2>&#x1f916; MCP (Model Context Protocol)</h2>
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
</div>
</div>"#;

// ---------------------------------------------------------------------------
// STT
// ---------------------------------------------------------------------------

async fn stt_form(State(_s): State<AppState>) -> Response {
    let html = r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>STT test</title><style>
body{font-family:system-ui,sans-serif;margin:0;padding:32px;background:#0f1115;color:#e6e6e6}
h1{font-size:18px} input[type=file]{margin:12px 0} button{background:#2563eb;color:#fff;border:0;padding:8px 16px;border-radius:6px;cursor:pointer}
pre{background:#1b1e24;padding:12px;border-radius:6px;white-space:pre-wrap;max-width:720px}
</style></head><body>
<h1>Speech-to-Text test</h1>
<p style="color:#9aa0a6;font-size:13px">Upload a 16kHz mono PCM WAV file. The selected STT model is loaded automatically.</p>
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
</script></body></html>"#;
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
        model::transcribe_wav_bytes(&model_state, &wav)
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
    let datasets: Vec<DatasetSummary> = dataset::list_datasets(&settings);
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
    let dir = match dataset::find_dataset_dir(&settings, &uuid) {
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
        rows.push_str("<tr><td colspan=\"2\" style=\"color:#888\">No files.</td></tr>");
    }

    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Dataset {uuid}</title><style>
body{{font-family:system-ui,sans-serif;margin:0;padding:32px;background:#0f1115;color:#e6e6e6}}
h1{{font-size:18px}} a{{color:#6cb6ff;text-decoration:none}} a:hover{{text-decoration:underline}}
table{{border-collapse:collapse;width:100%;max-width:820px}} th,td{{text-align:left;padding:8px 10px;border-bottom:1px solid #23262d;font-size:13px}} th{{color:#9aa0a6}}
</style></head><body>
<p><a href="/datasets">&larr; all datasets</a></p>
<h1>Dataset <code>{uuid}</code></h1>
<table><thead><tr><th>File</th><th>Size</th></tr></thead><tbody>{rows}</tbody></table>
</body></html>"#
    );
    Html(html).into_response()
}

async fn dataset_file_list(State(s): State<AppState>, AxPath(uuid): AxPath<String>) -> Response {
    let settings = s.app.state::<SettingsState>();
    let dir = match dataset::find_dataset_dir(&settings, &uuid) {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::NOT_FOUND, &e),
    };
    let mut files = Vec::new();
    collect_files(&dir, &dir, &mut files);
    (StatusCode::OK, Json(files)).into_response()
}

async fn dataset_file(
    State(s): State<AppState>,
    AxPath((uuid, rel)): AxPath<(String, String)>,
) -> Response {
    let settings = s.app.state::<SettingsState>();
    let dir = match dataset::find_dataset_dir(&settings, &uuid) {
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

    stream_file(&target_c).await
}

async fn stream_file(path: &Path) -> Response {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let ct = mime_for(path);

    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let len = file.metadata().await.map(|m| m.len()).ok();
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    if let Ok(v) = ct.parse() {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = format!("inline; filename=\"{}\"", name.replace('"', "")).parse() {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Ok(v) = "bytes".parse() {
        headers.insert(header::ACCEPT_RANGES, v);
    }
    if let Some(l) = len {
        if let Ok(v) = l.to_string().parse() {
            headers.insert(header::CONTENT_LENGTH, v);
        }
    }

    (StatusCode::OK, headers, body).into_response()
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
