//! The follower's side of the hub MCP call — compute offload, Variant B.
//!
//! One generic command, [`hub_mcp_call`], posts a `tools/call` to the hub's
//! `/mcp` surface with the recording inline and returns the tool's JSON reply.
//! The call is *not* fire-and-forget plumbing: the hub tool persists nothing,
//! the phone saves the result itself, so authorship (journal, XP, audio plane)
//! stays on the follower. See `docs/design/hub-mcp-compute-offload.md`.
//!
//! Framing follows `src-tauri/src/mcp/bridge.rs` — the stdio ↔ HTTP proxy that
//! documents every Streamable HTTP quirk of the rmcp server: the session id
//! arrives as the `Mcp-Session-Id` response header of `initialize`, a lost
//! session answers `404` (the hub restarted) and one transparent re-`initialize`
//! covers it, and a reply may come back as a bare JSON object or as an event
//! stream whose `data:` payloads carry the JSON-RPC messages.
//!
//! Auth is the same device-HMAC signing every sync round uses
//! ([`super::auth::hub_request`]), with the signed path exactly `/mcp`, so an
//! off-LAN (tailnet) follower passes `zone_guard` like any other API call. A
//! website hub is refused up front: it serves no `/mcp` compute, and
//! `hub_request` would silently degrade to anonymous on that origin.

use serde_json::{json, Value};
use tauri::{Emitter, State};

use crate::settings::SettingsState;

use super::auth::hub_request;
use super::pc_base;
use crate::sync::{hub_kind, HubKind};

/// The protocol version the bridge negotiates with the same server; kept in
/// step deliberately, because a `tools/call` before `initialize` is answered
/// `503` by rmcp.
const PROTOCOL_VERSION: &str = "2025-03-26";

/// Call one MCP tool on the configured hub and return its JSON reply.
///
/// `tool` is the hub's tool name (e.g. `read_aloud_assess_take`); `args` its
/// parameters verbatim. A tool whose reply is JSON in a text content part (all
/// of FmS's own tools) comes back as that parsed `Value`; anything else comes
/// back as the raw text. Timeouts: the hub may sit behind a cold model load, so
/// a call runs up to 300 s — the UI names the escape long before that.
/// The overlay is fed milestones rather than left on one spinner:
/// `hub-mcp-phase` events ("waiting" when the tool call is dispatched) let
/// the follower separate "sending to the hub" from "waiting for the hub
/// response".
#[tauri::command]
pub async fn hub_mcp_call(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    tool: String,
    args: Value,
) -> Result<Value, String> {
    hub_mcp_call_inner(&app, &settings, &tool, args).await
}

async fn hub_mcp_call_inner(app: &tauri::AppHandle, settings: &SettingsState, tool: &str, args: Value) -> Result<Value, String> {
    if tool.trim().is_empty() {
        return Err("No tool named for the hub call.".to_string());
    }
    let base = pc_base(settings);
    if base.is_empty() {
        return Err(
            "no hub configured — set the PC address on Devices & Hub, then connect".to_string(),
        );
    }
    if hub_kind(&base) == HubKind::Website {
        return Err(
            "the configured hub is the website; it serves no /mcp compute — the hub must be a LAN PC"
                .to_string(),
        );
    }

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    // Distinguish "the surface is switched off" from "the address is wrong"
    // before speaking any MCP on it (the same reason /status carries the field).
    let status = super::auth::hub_get_json(settings, "/api/v1/status", "").await?;
    if status.get("mcp_enabled").and_then(|v| v.as_bool()) == Some(false) {
        return Err("the hub's MCP surface is switched off — enable it on the Agent page".to_string());
    }

    let url = format!("{base}/mcp");

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "fms-follower", "version": "1.0" },
        },
    });
    let (status_code, new_session, body) =
        mcp_post(&client, settings, &url, None, &init).await?;
    if status_code == reqwest::StatusCode::NOT_FOUND || !status_code.is_success() {
        return Err(format!(
            "hub refused the MCP initialize (HTTP {status_code}): {}",
            snippet(&body, 300)
        ));
    }
    let mut session = new_session;

    // rmcp wants the handshake completed before it routes any tool call. The
    // notification has no id and its empty 2xx answer is not worth checking; a
    // transport error is, because the session is then not usable either way.
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let _ = mcp_post(&client, settings, &url, session.as_deref(), &initialized).await?;

    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    });
    // Everything the client can still see is over: from here the bytes are
    // uploading and the hub is computing, however long each of those takes.
    let _ = app.emit("hub-mcp-phase", "waiting");
    let (mut status_code, mut body) = {
        let (s, _, b) = mcp_post(&client, settings, &url, session.as_deref(), &call).await?;
        (s, b)
    };
    if status_code == reqwest::StatusCode::NOT_FOUND {
        // The hub restarted since initialize. Re-handshake and replay once,
        // so the caller never sees the hiccup (the bridge's behaviour).
        let _ = mcp_post(&client, settings, &url, None, &init).await?;
        session = None;
        let _ =
            mcp_post(&client, settings, &url, session.as_deref(), &initialized).await?;
        let r = mcp_post(&client, settings, &url, session.as_deref(), &call).await?;
        status_code = r.0;
        body = r.2;
    }
    if !status_code.is_success() {
        return Err(format!(
            "hub MCP call '{}' failed (HTTP {}): {}",
            tool,
            status_code,
            snippet(&body, 300)
        ));
    }

    let msg = rpc_message(&body, 2).ok_or_else(|| format!(
        "hub MCP reply carried no JSON-RPC message for the tool call: {}",
        snippet(&body, 300)
    ))?;
    if let Some(err) = msg.get("error") {
        let text = err
            .get("message")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| err.to_string());
        return Err(format!("hub MCP error: {text}"));
    }
    let result = msg.get("result").cloned().unwrap_or(Value::Null);
    if result.get("isError").and_then(|v| v.as_bool()) == Some(true) {
        // A tool refusal rides back as an error text content, not a JSON-RPC
        // error — surface its own words, which are the actionable ones.
        let text = first_text_content(&result).unwrap_or_else(|| result.to_string());
        return Err(text);
    }
    match result.get("structuredContent") {
        Some(v) if !v.is_null() => Ok(v.clone()),
        _ => match first_text_content(&result) {
            Some(text) => Ok(serde_json::from_str(&text)
                .unwrap_or_else(|_| Value::String(text))),
            None => Ok(result),
        },
    }
}

/// One signed JSON-RPC POST to the hub's `/mcp`: `(status, session header, body)`.
/// The signature covers the exact bytes sent, over the exact path `/mcp` —
/// both are part of what `zone_guard` verifies.
async fn mcp_post(
    client: &reqwest::Client,
    settings: &SettingsState,
    url: &str,
    session: Option<&str>,
    msg: &Value,
) -> Result<(reqwest::StatusCode, Option<String>, String), String> {
    let body = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    let mut req = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        );
    if let Some(s) = session {
        req = req.header("mcp-session-id", s);
    }
    let req = hub_request(req, settings, "POST", "/mcp", "", &body)
        .await?
        .body(body);
    let resp = req.send().await.map_err(|e| {
        format!("cannot reach the hub's MCP endpoint at {url} — is the PC running and paired? ({e})")
    })?;
    let status = resp.status();
    let new_session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());
    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok((status, new_session, text))
}

/// The JSON-RPC message with `id` inside one Streamable HTTP reply body —
/// either a bare JSON object or an event stream whose `data:` payloads carry
/// messages (`id:`/`retry:` lines, comments and the priming event are dropped,
/// exactly as the bridge does).
fn rpc_message(body: &str, id: i64) -> Option<Value> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        let v: Value = serde_json::from_str(trimmed).ok()?;
        return (v.get("jsonrpc")?.as_i64() == Some(2)).then_some(v);
    }
    for line in body.lines() {
        // A `data:` line carries a JSON-RPC payload; `id:`/`retry:` fields and
        // `:` comment/priming lines are part of the SSE framing and must be
        // *skipped*, not treated as a fatal parse stop — the real reply for the
        // tool call is on a later `data:` line, so bailing here would discard a
        // hub response that arrived perfectly.
        let Some(rest) = line.trim_start().strip_prefix("data:") else {
            continue;
        };
        let payload = rest.trim();
        if payload.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else { continue };
        if v.get("jsonrpc").and_then(|j| j.as_i64()) == Some(2)
            && v.get("id").and_then(|i| i.as_i64()) == Some(id)
        {
            return Some(v);
        }
    }
    None
}

/// The first text content part of a `tools/call` result, if any.
fn first_text_content(result: &Value) -> Option<String> {
    result
        .get("content")?
        .as_array()?
        .iter()
        .find(|c| c.get("type").and_then(|t| t.as_str()) == Some("text"))?
        .get("text")?
        .as_str()
        .map(|s| s.to_string())
}

/// A one-line excerpt of a reply body for an error message.
fn snippet(body: &str, max: usize) -> String {
    let flat = body.replace(['\r', '\n'], " ");
    let cut: String = flat.chars().take(max).collect();
    if flat.chars().count() > max {
        format!("{cut}…")
    } else {
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_json_reply_is_found_by_id() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[]}}";
        let msg = rpc_message(body, 2).expect("message");
        assert!(msg["result"].is_object());
        assert!(rpc_message(body, 3).is_none());
    }

    /// The priming event, `id:`/`retry:` lines and a non-JSON payload must all
    /// be skipped, or the call reports garbage as its reply.
    #[test]
    fn event_stream_reply_is_scanned_for_the_matching_id() {
        let body = "data:\r\nid: 0\r\nretry: 3000\r\n\r\n\
                    data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\r\n\r\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"ok\\\":true}\"}]}}\r\n";
        let msg = rpc_message(body, 2).expect("message");
        let text = first_text_content(msg.get("result").unwrap()).expect("text");
        assert_eq!(text, "{\"ok\":true}");
    }

    #[test]
    fn non_json_body_carries_no_message() {
        assert!(rpc_message("<!doctype html><h1>404</h1>", 2).is_none());
        assert!(rpc_message("", 2).is_none());
    }

    #[test]
    fn snippet_flattens_and_truncates() {
        assert_eq!(snippet("a\nb", 10), "a b");
        assert_eq!(snippet("abcdef", 3), "abc…");
    }
}
