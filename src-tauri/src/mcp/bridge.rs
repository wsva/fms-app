//! stdio ↔ HTTP MCP bridge — `fms-app --mcp-bridge`.
//!
//! Two classes of client cannot use [`super::create_mcp_service`] directly:
//!
//! 1. products whose MCP config only accepts a *command* (their client only ever
//!    speaks the stdio transport), and
//! 2. ACP agents that do not advertise `mcpCapabilities.http` — `session/new`
//!    then has to hand them a stdio entry, because an `http` entry would be
//!    ignored and they would get `mcpServers: []`, i.e. zero tools.
//!
//! Both are solved by one artifact: this module, which the app binary enters
//! when its first argument is `--mcp-bridge`. It is a *transport* proxy, never a
//! second tool server: every tool implementation needs the running app's
//! `AppHandle` (settings, dataset paths, the web service state), so a standalone
//! process could only re-implement them, and a re-implementation drifts within a
//! week. Here a line of stdin is posted to the live `/mcp` endpoint and the
//! response is written back as a line of stdout.
//!
//! # Why a flag instead of a Tauri sidecar
//!
//! `bundle.externalBin` needs a per-target-triple artifact in `src-tauri/binaries/`
//! and a new CI step per platform, while the ACP stdio entry needs nothing more
//! than *the path of this executable* — which is exactly what
//! `std::env::current_exe()` already gives. `windows_subsystem = "windows"` does
//! not interfere: a GUI-subsystem process still reads stdin and writes stdout
//! normally when its parent supplies piped handles.
//!
//! # Wire behaviour
//!
//! - One JSON-RPC message per stdin line (MCP stdio framing); one per stdout line.
//! - `initialize`'s `Mcp-Session-Id` response header is captured and replayed on
//!   every later request. A `404` that means "that session is gone" (the app was
//!   restarted) triggers one transparent re-`initialize` and a replay, so the
//!   client never sees the restart.
//! - `text/event-stream` bodies are split into `data:` payloads; only payloads
//!   that parse as a JSON object carrying `jsonrpc` are forwarded, so `id:`,
//!   `retry:`, comments and the priming event are dropped. Buffering the whole
//!   body is correct here because each POST's stream is finite.
//! - Strictly sequential: no request pipelining.
//! - Diagnostics go to **stderr only** — a stray byte on stdout would corrupt
//!   the protocol channel.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

/// Where the bridge posts when `FMS_MCP_URL` is unset.
fn default_url() -> String {
    format!(
        "http://127.0.0.1:{}/mcp",
        crate::sync::server::DEFAULT_PORT
    )
}

/// Entry point for `--mcp-bridge`. Returns the process exit code: `0` when stdin
/// reached EOF (the normal end of a stdio MCP session), `1` when the bridge could
/// not start at all.
pub(crate) fn main_blocking() -> i32 {
    let url = std::env::var("FMS_MCP_URL")
        .ok()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(default_url);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[mcp-bridge] cannot start a runtime: {}", e);
            return 1;
        }
    };
    runtime.block_on(run(url))
}

async fn run(url: String) -> i32 {
    eprintln!("[mcp-bridge] forwarding stdio to {}", url);
    let client = reqwest::Client::new();

    // stdin is read on its own thread: a blocking read inside the async loop
    // would stall the HTTP side, and the loop must keep draining responses while
    // the client is quiet.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(l) if !l.trim().is_empty() => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("[mcp-bridge] stdin read error: {}", e);
                    break;
                }
            }
        }
        // Dropping `tx` ends the loop.
    });

    let mut session: Option<String> = None;
    let mut init_params: Option<Value> = None;

    while let Some(line) = rx.recv().await {
        let parsed: Option<Value> = serde_json::from_str(&line).ok();
        let method = parsed
            .as_ref()
            .and_then(|v| v.get("method"))
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        let id = parsed.as_ref().and_then(|v| v.get("id")).cloned();

        if method == "initialize" {
            init_params = parsed.as_ref().and_then(|v| v.get("params")).cloned();
        }

        let first = post(&client, &url, session.as_deref(), &line).await;
        match first {
            Ok((status, new_session, body)) => {
                if let Some(s) = new_session {
                    session = Some(s);
                }
                if status == reqwest::StatusCode::NOT_FOUND && method != "initialize" {
                    // The app restarted and forgot the session. Re-initialize and
                    // replay once, so the client does not see the hiccup.
                    eprintln!("[mcp-bridge] session expired upstream; re-initializing");
                    session = None;
                    if let Some(params) = init_params.clone() {
                        let reinit = json!({
                            "jsonrpc": "2.0",
                            "id": "fms-bridge-reinit",
                            "method": "initialize",
                            "params": params,
                        });
                        match post(&client, &url, None, &reinit.to_string()).await {
                            Ok((_s, s, _b)) => session = s,
                            Err(e) => eprintln!("[mcp-bridge] re-initialize failed: {}", e),
                        }
                    }
                    match post(&client, &url, session.as_deref(), &line).await {
                        Ok((_s, s, body)) => {
                            if let Some(s) = s {
                                session = Some(s);
                            }
                            forward(&body);
                        }
                        Err(e) => emit_error(
                            id.as_ref(),
                            &format!("bridge: upstream error on replay: {}", e),
                        ),
                    }
                } else if status.is_success() {
                    forward(&body);
                } else {
                    // Surface an upstream refusal as a JSON-RPC error rather than
                    // staying silent: a client waiting forever is worse than a
                    // message that names the status and the body.
                    let message = format!(
                        "bridge: upstream HTTP {}: {}",
                        status,
                        snippet(&body, 400)
                    );
                    if id.is_some() {
                        emit_error(id.as_ref(), &message);
                    } else {
                        eprintln!("[mcp-bridge] notification refused: {}", message);
                    }
                }
            }
            Err(e) => {
                let message = format!(
                    "bridge: cannot reach FmS at {} ({}). Start the app, or point FMS_MCP_URL at the port it bound.",
                    url, e
                );
                if id.is_some() {
                    emit_error(id.as_ref(), &message);
                } else {
                    eprintln!("[mcp-bridge] {}", message);
                }
            }
        }
    }

    eprintln!("[mcp-bridge] stdin closed, exiting");
    0
}

/// One POST to the upstream MCP endpoint: `(status, session id, raw body)`.
async fn post(
    client: &reqwest::Client,
    url: &str,
    session: Option<&str>,
    body: &str,
) -> Result<(reqwest::StatusCode, Option<String>, String), String> {
    let mut req = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .body(body.to_string());
    if let Some(s) = session {
        req = req.header("mcp-session-id", s);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let new_session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());
    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok((status, new_session, text))
}

/// Write every forwardable message in `body` to stdout, one per line.
fn forward(body: &str) {
    let lines = forwardable(body);
    if lines.is_empty() {
        return;
    }
    let mut out = std::io::stdout().lock();
    for line in lines {
        if writeln!(out, "{}", line).is_err() {
            break;
        }
    }
    let _ = out.flush();
}

/// JSON-RPC messages contained in one upstream response body.
///
/// Handles both shapes a Streamable HTTP server may answer with: a bare JSON
/// object, or an event stream where only some `data:` payloads carry a message.
fn forwardable(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        if let Some(msg) = parse_message(trimmed) {
            out.push(msg);
        }
        return out;
    }
    for line in body.lines() {
        let Some(rest) = line.trim_start().strip_prefix("data:") else {
            continue;
        };
        let payload = rest.trim();
        if payload.is_empty() {
            continue;
        }
        if let Some(msg) = parse_message(payload) {
            out.push(msg);
        }
    }
    out
}

/// A payload is forwarded only if it is a JSON-RPC message — that single check
/// is what keeps `id: 0`, `retry: 3000`, comments and the priming event out of
/// stdout.
fn parse_message(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("jsonrpc").is_none() {
        return None;
    }
    Some(value.to_string())
}

/// Emit a JSON-RPC error for `id` on stdout (notifications, which have no id, are
/// only logged).
fn emit_error(id: Option<&Value>, message: &str) {
    let id = match id {
        Some(id) => id.clone(),
        None => {
            eprintln!("[mcp-bridge] {}", message);
            return;
        }
    };
    let msg = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32603, "message": message },
    });
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", msg);
    let _ = out.flush();
    eprintln!("[mcp-bridge] error for {}: {}", id, message);
}

fn snippet(body: &str, max: usize) -> String {
    let flat = body.replace(['\r', '\n'], " ");
    let cut = flat.chars().take(max).collect::<String>();
    if flat.chars().count() > max {
        format!("{}…", cut)
    } else {
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The priming event, `id:`/`retry:` lines and a non-JSON payload must all
    /// be dropped, or the client sees garbage on its protocol channel.
    #[test]
    fn event_stream_forwards_only_json_messages() {
        let body = "data:\r\nid: 0\r\nretry: 3000\r\n\r\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\r\n\r\n\
                    data: not json\r\n\r\n\
                    : a comment\r\n\
                    event: message\r\n\
                    data:{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\r\n";
        let lines = forwardable(body);
        assert_eq!(lines.len(), 2, "got {:?}", lines);
        assert!(lines[0].contains("\"result\""));
        assert!(lines[1].contains("notifications/progress"));
    }

    #[test]
    fn bare_json_body_is_forwarded_once() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}";
        let lines = forwardable(body);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"id\":7"));
    }

    #[test]
    fn non_json_body_forwards_nothing() {
        assert!(forwardable("<!doctype html><h1>404</h1>").is_empty());
        assert!(forwardable("").is_empty());
    }

    #[test]
    fn snippet_flattens_and_truncates() {
        assert_eq!(snippet("a\nb", 10), "a b");
        assert_eq!(snippet("abcdef", 3), "abc…");
    }
}
