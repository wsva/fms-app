use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings::SettingsState;
use crate::workspace::WorkspaceState;

/// Production origin this build talks to by default: OAuth login is verified
/// against it, and so is card sync.
pub(crate) const DEFAULT_BASE_URL: &str = "https://lusworkshop.site";

/// The single hub/auth origin. Defaults to [`DEFAULT_BASE_URL`]; a developer can
/// point it at a local/staging box by exporting `FMS_BASE_URL` (e.g.
/// `https://100.124.177.123`). Because [`crate::sync::hub_kind`] classifies an
/// origin by comparing it to this value, overriding it makes the dev box count as
/// `HubKind::Website` (Bearer auth, upload, feature-detection) *and* redirects
/// login / card-sync there — the two things needed to exercise the website path
/// end-to-end against a dev server. No env var (the shipped default) is byte-for
/// byte the old compile-time constant. Trailing slashes are trimmed so origin
/// comparison and `{base}/path` formatting stay consistent.
pub(crate) fn base_url() -> String {
    let from_env = std::env::var("FMS_BASE_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());
    from_env.unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// Dev-only TLS escape hatch. When `FMS_DEV_IGNORE_TLS=1` is set, the returned
/// builder accepts invalid/self-signed certificates (and skips hostname
/// verification) so a developer can point `FMS_BASE_URL` + `pc_url` at an HTTPS
/// box with a self-signed cert. **Unset — the production and mobile default —
/// leaves verification fully strict.** rustls (`rustls-tls`) uses its own bundled
/// roots and never reads the OS trust store, so installing the CA in Windows has
/// no effect on these native reqwest calls; this flag is the supported dev route.
pub(crate) fn apply_dev_tls(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    if std::env::var("FMS_DEV_IGNORE_TLS").map(|v| v.trim() == "1").unwrap_or(false) {
        log::warn!("[dev] FMS_DEV_IGNORE_TLS=1 — TLS certificate + hostname verification DISABLED (dev only, never ship enabled)");
        builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
    } else {
        builder
    }
}

/// A `ClientBuilder` with the dev TLS hatch pre-applied ([`apply_dev_tls`]).
/// Callers still chain their own timeouts and `.build()`. Byte-for-byte the same
/// as `reqwest::Client::builder()` when `FMS_DEV_IGNORE_TLS` is unset, so the
/// production/mobile trust path is untouched.
pub(crate) fn hub_client_builder() -> reqwest::ClientBuilder {
    apply_dev_tls(reqwest::Client::builder())
}

/// Deep link scheme used for login callback.
const DEEP_LINK_SCHEME: &str = "fms-app";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
struct AuthTokens {
    access_token: String,
    refresh_token: String,
    user_id: String,
    username: String,
    #[serde(default)]
    email: String,
    /// Unix seconds when this access token was minted (login or last silent
    /// refresh). The renewal schedule normally reads the JWT's own `exp`; this is
    /// the fallback clock for a token whose `exp` cannot be read, and `0` marks a
    /// file written before the field existed — such a session is renewed on sight.
    #[serde(default)]
    obtained_at: u64,
}

/// Re-mint the access token this far ahead of its own expiry, so a request never
/// races the deadline. When the token's `exp` claim cannot be read (a non-JWT
/// token from some other hub), renewal falls back to this age instead.
const TOKEN_RENEW_MARGIN_SECS: u64 = 24 * 3600;
const TOKEN_MAX_AGE_SECS: u64 = 3 * 24 * 3600;

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The `exp` claim of a JWT access token, read without verifying the signature.
/// Only the payload segment is base64url-decoded. This is not an authorization
/// decision — the hub owns that — it is the app's own calendar for "renew me
/// before the server starts saying no", and it has to tolerate a non-JWT token
/// by answering `None`.
fn token_expiry(access_token: &str) -> Option<u64> {
    use base64::Engine;
    let payload = access_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    json["exp"].as_u64()
}

/// Whether the token is already past its own expiry (or, unreadable, past the
/// age we assume). Logged for diagnosis: a request that goes out on a dead token
/// is about to be refused, which is what a 401 renewal is for.
fn token_is_dead(tokens: &AuthTokens) -> bool {
    match token_expiry(&tokens.access_token) {
        Some(exp) => now_unix() >= exp,
        // No readable `exp`: assume the hub's 7-day lifetime from when we got it.
        None => tokens.obtained_at == 0 || now_unix().saturating_sub(tokens.obtained_at) >= 7 * 24 * 3600,
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AuthUser {
    pub name: String,
    pub email: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Auth file path: `<workspace>/auth.json` when a workspace is selected,
/// otherwise the legacy global path `{app-data}/fms-app/auth.json`.
fn auth_file_path(settings: &SettingsState) -> Result<PathBuf, String> {
    if let Some(ws_dir) = settings.workspace_dir.lock().unwrap().as_ref() {
        std::fs::create_dir_all(ws_dir)
            .map_err(|e| format!("Failed to create workspace dir: {}", e))?;
        return Ok(ws_dir.join("auth.json"));
    }
    let dir = crate::app_paths::data_root();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create auth dir: {}", e))?;
    Ok(dir.join("auth.json"))
}

/// Legacy global auth file path (used for one-time migration into a workspace).
fn legacy_auth_file_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("fms-app").join("auth.json"))
}

/// Migrate the legacy global auth.json into the workspace (once).
/// The global tokens belonged to the pre-workspace single user, so the first
/// workspace that reads them claims them; later workspaces start logged out.
fn migrate_legacy_auth(settings: &SettingsState) {
    let ws_auth = match auth_file_path(settings) {
        Ok(p) => p,
        Err(_) => return,
    };
    // Only migrate when a workspace is selected and its auth.json is absent.
    if settings.workspace_dir.lock().unwrap().is_none() || ws_auth.exists() {
        return;
    }
    let Some(legacy) = legacy_auth_file_path() else { return; };
    if !legacy.exists() {
        return;
    }
    match std::fs::rename(&legacy, &ws_auth) {
        Ok(_) => log::info!(
            "[Auth] Migrated legacy auth.json into workspace: {}",
            ws_auth.display()
        ),
        Err(e) => log::warn!("[Auth] Failed to migrate legacy auth.json: {}", e),
    }
}

fn read_tokens(settings: &SettingsState) -> Result<Option<AuthTokens>, String> {
    migrate_legacy_auth(settings);
    let path = auth_file_path(settings)?;
    if !path.exists() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read auth file: {}", e))?;
    // Tolerate a UTF-8 BOM (Windows editors/PowerShell often add one).
    let content = content.trim_start_matches('\u{FEFF}');
    let tokens: AuthTokens =
        serde_json::from_str(content).map_err(|e| format!("Failed to parse auth file: {}", e))?;
    Ok(Some(tokens))
}

fn write_tokens(settings: &SettingsState, tokens: &AuthTokens) -> Result<(), String> {
    let path = auth_file_path(settings)?;
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("Failed to serialize tokens: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write auth file: {}", e))?;
    log::info!("[Auth] Tokens written to {}", path.display());
    Ok(())
}

/// The identity the UI needs, built straight from the stored file — used
/// whenever the server is unreachable, so being offline never logs anyone out.
fn stored_user(tokens: &AuthTokens) -> AuthUser {
    AuthUser {
        name: if tokens.username.is_empty() {
            tokens.user_id.clone()
        } else {
            tokens.username.clone()
        },
        email: tokens.email.clone(),
    }
}

fn delete_tokens(settings: &SettingsState) -> Result<(), String> {
    let path = auth_file_path(settings)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete auth file: {}", e))?;
        log::info!("[Auth] Tokens deleted from {}", path.display());
    }
    Ok(())
}

/// Get the user_id from stored tokens without making a network call.
/// Returns None if not logged in.
pub fn get_stored_user_id(settings: &SettingsState) -> Option<String> {
    read_tokens(settings)
        .ok()
        .flatten()
        .map(|t| t.user_id)
}

/// The account email from stored tokens, without a network call. `None` when
/// logged out or when the login echo carried no email (older deep-link flows).
/// The market surface uses it for the `is_owner` partition and the whoami read.
pub(crate) fn get_stored_email(settings: &SettingsState) -> Option<String> {
    read_tokens(settings)
        .ok()
        .flatten()
        .map(|t| t.email)
        .filter(|s| !s.is_empty())
}

/// The OAuth2 access token for the website hub, read from the single stored
/// token file — there is deliberately no second token store (dev guide T1).
/// This is the *synchronous* read: no expiry check, no network. Use it for
/// "am I signed in?" probes; use [`access_token`] on the request path, where the
/// token gets renewed before it can die. `datasets::cards::sync` and
/// `sync::client`'s website path both take it from here, so a logout (token
/// file removed) ends every bearer-authorized flow at once.
pub(crate) fn get_access_token(settings: &SettingsState) -> Result<String, String> {
    read_tokens(settings)?
        .map(|t| t.access_token)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Not logged in. Please log in first.".to_string())
}

/// Why a silent refresh could not hand back a fresh access token.
enum RefreshFail {
    /// The server refused the refresh token (unknown, revoked, or the account
    /// is deactivated). The session really is over — the token file goes with it.
    Rejected(String),
    /// The server was unreachable or answered unreadably. That says nothing
    /// about the session, so the stored token survives and the user stays
    /// signed in — only an explicit logout may end it.
    Transient(String),
}

/// Outcome of an in-place renewal attempt, used by every caller that must not
/// log the user out on a network hiccup.
enum Renewal {
    /// A new access token is in `tokens` and on disk.
    Renewed,
    /// The renewal could not be completed (hub unreachable, or a hub that does
    /// not serve the route yet); the old token stands and may well still work.
    Kept,
    /// The hub refused this session's refresh token; the file has been cleared.
    Ended,
}

/// Exchange the stored refresh token for a fresh access token at the website's
/// `/api/oauth2/refresh` and persist the result over the old one. The refresh
/// token itself is not rotated — the app keeps a single copy, and rotating it
/// would need a store-and-swap dance for no gain at this trust level.
async fn refresh_tokens(settings: &SettingsState, old: &AuthTokens) -> Result<AuthTokens, RefreshFail> {
    if old.refresh_token.is_empty() {
        return Err(RefreshFail::Rejected("the stored session carries no refresh token".into()));
    }
    let client = hub_client_builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| RefreshFail::Transient(e.to_string()))?;
    let resp = client
        .post(format!("{}/api/oauth2/refresh", base_url()))
        .json(&serde_json::json!({ "refresh_token": old.refresh_token }))
        .send()
        .await
        .map_err(|e| RefreshFail::Transient(err_chain(&e)))?;

    let status = resp.status();
    // The route reports a refused token as 200 + success:false; a bare 401/403
    // from a gateway or an older server says the same thing, and only a refusal
    // of *this* session's token justifies ending it.
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(RefreshFail::Rejected(format!("HTTP {status}")));
    }
    // Anything else non-2xx — including a 404 from a hub that predates this
    // route — is an availability problem, not a verdict on the session.
    if !status.is_success() {
        return Err(RefreshFail::Transient(format!("HTTP {status}")));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| RefreshFail::Transient(format!("unparsable refresh response (status {status}): {e}")))?;
    if json["success"].as_bool() != Some(true) {
        let msg = json["errMsg"].as_str().unwrap_or("refresh token refused");
        return Err(RefreshFail::Rejected(msg.to_string()));
    }
    let list = json["data"]["list"]
        .as_array()
        .ok_or_else(|| RefreshFail::Transient("refresh response missing data.list".into()))?;
    let field = |i: usize| list.get(i).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let access_token = field(0);
    if access_token.is_empty() {
        return Err(RefreshFail::Transient("refresh response carried no access token".into()));
    }
    let keep = |fresh: String, stored: String| if fresh.is_empty() { stored } else { fresh };
    let renewed = AuthTokens {
        access_token,
        refresh_token: keep(field(3), old.refresh_token.clone()),
        user_id: keep(field(1), old.user_id.clone()),
        username: keep(field(2), old.username.clone()),
        email: old.email.clone(),
        obtained_at: now_unix(),
    };
    write_tokens(settings, &renewed).map_err(RefreshFail::Transient)?;
    log::info!("[Auth] Access token silently refreshed for user_id='{}'", renewed.user_id);
    Ok(renewed)
}

/// Renew `tokens` in place (and on disk), classifying failure so that no caller
/// can mistake "server unreachable" for "logged out".
async fn renew(settings: &SettingsState, tokens: &mut AuthTokens) -> Renewal {
    match refresh_tokens(settings, tokens).await {
        Ok(fresh) => {
            *tokens = fresh;
            Renewal::Renewed
        }
        Err(RefreshFail::Rejected(e)) => {
            log::warn!("[Auth] refresh rejected ({}) — session ended, sign in again", e);
            let _ = delete_tokens(settings);
            Renewal::Ended
        }
        Err(RefreshFail::Transient(e)) => {
            // The hub could not be reached, or cannot serve a renewal right now
            // (a 404 from a hub that predates the route is this too). Nothing here
            // can end a session: the refresh token outlives the access token, so
            // even a long offline stretch is repaired on the next reachable call.
            log::warn!(
                "[Auth] refresh unavailable ({}) — keeping the session (token expired: {})",
                e,
                token_is_dead(tokens)
            );
            Renewal::Kept
        }
    }
}

fn token_is_stale(tokens: &AuthTokens) -> bool {
    // Prefer the token's own expiry: renew just before it, which also means a
    // token the hub handed us with a shorter or longer lifetime is renewed on the
    // hub's schedule rather than ours.
    match token_expiry(&tokens.access_token) {
        Some(exp) => now_unix().saturating_add(TOKEN_RENEW_MARGIN_SECS) >= exp,
        None => {
            tokens.obtained_at == 0
                || now_unix().saturating_sub(tokens.obtained_at) >= TOKEN_MAX_AGE_SECS
        }
    }
}

lazy_static::lazy_static! {
    /// Serializes silent renewals. A burst of callers that all notice a stale
    /// token — the keeper task, the startup probe, several hub requests in one
    /// sync round — would otherwise each make a refresh round trip and each
    /// rewrite auth.json. One renewal at a time, with staleness re-checked after
    /// the lock is taken, collapses that to a single trip.
    static ref REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::new(());
}

/// Bring the stored access token back from the edge of expiry. Silent and
/// infallible by design: request paths call this before they talk to the hub, so
/// the token they carry is always young enough to be accepted.
pub(crate) async fn ensure_fresh(settings: &SettingsState) {
    // Fast path: not signed in, or the token still has days of slack — no lock.
    let tokens = match read_tokens(settings) {
        Ok(Some(t)) => t,
        Ok(None) => return,
        Err(e) => {
            log::warn!("[Auth] ensure_fresh: {}", e);
            return;
        }
    };
    if !token_is_stale(&tokens) {
        return;
    }
    let _guard = REFRESH_LOCK.lock().await;
    // Re-read under the lock: a caller that got here first has already renewed.
    let mut tokens = match read_tokens(settings) {
        Ok(Some(t)) => t,
        Ok(None) => return,
        Err(e) => {
            log::warn!("[Auth] ensure_fresh: {}", e);
            return;
        }
    };
    if !token_is_stale(&tokens) {
        return;
    }
    renew(settings, &mut tokens).await;
}

/// Renew regardless of age — the hub just said the token it was given is not
/// acceptable, which is exactly what a refresh fixes.
async fn force_renew(settings: &SettingsState, tokens: &mut AuthTokens) -> Renewal {
    let _guard = REFRESH_LOCK.lock().await;
    renew(settings, tokens).await
}

/// The request-path accessor: [`get_access_token`] with the staleness renewal
/// in front of it. Errors only when nobody is signed in, or when the server has
/// actively refused the session.
pub(crate) async fn access_token(settings: &SettingsState) -> Result<String, String> {
    ensure_fresh(settings).await;
    get_access_token(settings)
}

/// Handle a 401 from the website. The token is renewed rather than thrown away:
/// a 401 with a stored refresh token means "our copy aged out", which the
/// refresh route fixes. Only a refresh token the server also refuses ends the
/// session. Returns the fresh access token when the caller could retry.
pub(crate) async fn token_rejected(settings: &SettingsState) -> Option<String> {
    let mut tokens = read_tokens(settings).ok().flatten()?;
    match force_renew(settings, &mut tokens).await {
        Renewal::Renewed => Some(tokens.access_token),
        Renewal::Kept | Renewal::Ended => None,
    }
}

/// Keep the session alive for as long as the app runs: renew the stored token
/// whenever it approaches the server's expiry, on a poll tighter than the
/// renewal margin so a token is never found dead at request time. Started once
/// from `setup()` on every platform. Without this, an app left open for a week
/// would walk into the expiry mid-session.
pub(crate) async fn session_keeper(app: AppHandle) {
    loop {
        {
            let settings = app.state::<SettingsState>();
            ensure_fresh(&settings).await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(TOKEN_RENEW_MARGIN_SECS / 3)).await;
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Open the browser to the login page. After login, the server redirects to
/// `fms-app://login?access_token=...&user_id=...&username=...&refresh_token=...`
/// which is handled by the deep link handler in lib.rs.
#[tauri::command]
pub async fn auth_open_login(app: AppHandle) -> Result<String, String> {
    log::info!("auth_open_login: opening browser");
    let callback = format!("{}://login", DEEP_LINK_SCHEME);
    let login_url = format!(
        "{}/oauth2/login?desktop_callback={}",
        base_url(),
        urlencoding::encode(&callback)
    );
    log::info!("auth_open_login: url={}", login_url);

    // Open the URL in the default browser.
    open_url(&app, &login_url)?;
    Ok(login_url)
}

/// Format a reqwest/IO error with its full source chain. reqwest's top-level
/// Display is just "error sending request for url (...)"; the actual cause
/// (TLS handshake, unknown issuer, DNS, timeout, no crypto provider) lives in
/// the source chain, which is what we need to diagnose Android network errors.
fn err_chain<E: std::error::Error>(err: &E) -> String {
    let mut msg = err.to_string();
    let mut src: Option<&dyn std::error::Error> = err.source();
    while let Some(s) = src {
        msg.push_str(" | caused by: ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

/// Verify credentials directly against the website's first-party signin API and
/// store the returned tokens. This is the in-app login path (primary on
/// Android): the user never leaves the app for a browser OAuth2 + `fms-app://`
/// deep-link round trip, which Chrome on Android silently blocks because the
/// server-issued custom-scheme redirect has no transient user activation.
///
/// Response contract of `POST /api/oauth2/signin` (no `desktop_callback`):
///   success -> { success: true, data: { list: [accessToken, user_id, username, refreshToken] } }
///   failure -> { success: false, errMsg: "..." }
#[tauri::command]
pub async fn auth_login_password(
    app: AppHandle,
    email: String,
    password: String,
) -> Result<AuthUser, String> {
    log::info!("[Auth] login: verifying credentials for '{}'", email);
    let base = base_url();
    let client = apply_dev_tls(reqwest::Client::builder())
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
    let body = serde_json::json!({
        "data": { "Nickname": email, "Email": email, "Password": password }
    });
    let resp = client
        .post(format!("{}/api/oauth2/signin", base))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            // Log the full source chain so the underlying cause (TLS handshake,
            // unknown issuer, DNS, timeout, no crypto provider) is visible in
            // the in-app Logs page, not just reqwest's generic top-level text.
            let detail = err_chain(&e);
            log::error!("[Auth] login: signin request to {} failed: {}", base, detail);
            format!("Signin request failed: {}", detail)
        })?;

    let status = resp.status();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| {
            log::error!("[Auth] login: failed to parse signin response (status {}): {}", status, e);
            format!("Failed to parse signin response (status {}): {}", status, e)
        })?;

    if json["success"].as_bool() != Some(true) {
        let msg = json["errMsg"].as_str().unwrap_or("Invalid email or password");
        log::warn!("[Auth] login: signin rejected for '{}': {}", email, msg);
        return Err(msg.to_string());
    }

    let list = json["data"]["list"]
        .as_array()
        .ok_or_else(|| "Malformed signin response (missing data.list)".to_string())?;
    let field = |i: usize| list.get(i).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let access_token = field(0);
    let user_id = field(1);
    let username = field(2);
    let refresh_token = field(3);

    if access_token.is_empty() {
        return Err("Signin response contained no access token".to_string());
    }

    log::info!(
        "[Auth] login: signin succeeded for user_id='{}', username='{}'",
        user_id,
        username
    );
    // Reuse the shared post-login pipeline: persist tokens, fetch user info,
    // claim the current workspace, fold logged-out XP, emit `auth-login-success`.
    auth_process_token(app, access_token, refresh_token, user_id, username).await
}

/// Process tokens received from the deep link callback.
/// Called by the deep link handler when `fms-app://login?access_token=...` is received.
pub async fn auth_process_token(
    app: tauri::AppHandle,
    access_token: String,
    refresh_token: String,
    user_id: String,
    username: String,
) -> Result<AuthUser, String> {
    log::info!("auth_process_token: user={}, id={}", username, user_id);

    // Tokens are stored in the currently selected workspace's auth.json
    // (workspace must be selected before login; otherwise legacy global path).
    let settings = app.state::<SettingsState>();

    let tokens = AuthTokens {
        access_token,
        refresh_token,
        user_id: user_id.clone(),
        username,
        email: String::new(),
        obtained_at: now_unix(),
    };
    write_tokens(&settings, &tokens)?;

    // Fetch user info (includes email).
    let user = match fetch_user_info(&tokens.access_token).await {
        Userinfo::User(u) => u,
        Userinfo::Rejected(e) => {
            // The tokens we just stored were refused outright — a real login
            // failure, not a hiccup, so report it and leave the file for retry.
            return Err(format!("Login not confirmed by the server: {}", e));
        }
        Userinfo::Transient(e) => {
            return Err(format!("Failed to fetch user info: {}", e));
        }
    };
    log::info!("auth_process_token success: user={}, email={}", user.name, user.email);

    // Persist email in tokens.
    let tokens = AuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        user_id: tokens.user_id,
        username: tokens.username,
        email: user.email.clone(),
        obtained_at: tokens.obtained_at,
    };
    let _ = write_tokens(&settings, &tokens);

    // Try to claim the current workspace with the user's email.
    let ws_state = app.state::<WorkspaceState>();
    let ws_current = ws_state.current.lock().unwrap().clone();
    if let Some(ws) = ws_current {
        if ws.user_id.is_empty() || ws.user_id == "local" {
            let email_to_claim = if !user.email.is_empty() {
                user.email.clone()
            } else {
                user_id.clone()
            };
            log::info!("[Auth] Claiming workspace '{}' for user '{}'", ws.uuid, email_to_claim);
            
            // Update registry
            {
                let mut registry = ws_state.registry.lock().unwrap();
                if let Some(ws_mut) = registry.workspaces.iter_mut().find(|w| w.uuid == ws.uuid) {
                    ws_mut.user_id = email_to_claim.clone();
                }
                let _ = crate::workspace::WorkspaceState::save_registry(&registry);
            }
            
            // Update workspace.json
            {
                let registry = ws_state.registry.lock().unwrap();
                if let Some(ws_to_save) = registry.workspaces.iter().find(|w| w.uuid == ws.uuid) {
                    let _ = crate::workspace::save_workspace_json(ws_to_save);
                }
            }
            
            // Fold any progress/XP recorded while logged out ("local"/"") into the owner.
            crate::xp::migrate_identity_to_owner(
                &crate::workspace::workspace_dir(&ws.uuid),
                &email_to_claim,
            );

            // Update current workspace state
            {
                let mut current = ws_state.current.lock().unwrap();
                if let Some(ref mut ws) = *current {
                    ws.user_id = email_to_claim;
                }
            }
        } else if ws.user_id != user.email && ws.user_id != user_id {
            log::warn!(
                "[Auth] Workspace '{}' belongs to '{}', but user '{}' logged in",
                ws.uuid, ws.user_id, user.email
            );
        }
    }

    // Notify frontend that login succeeded.
    let _ = app.emit("auth-login-success", &user);

    Ok(user)
}

/// Get the currently logged-in user, or None if not logged in.
///
/// This is the startup / workspace-switch probe the UI runs, so it is written to
/// preserve the session: a token near expiry is renewed first, a renewal that
/// cannot reach the server falls back to the stored identity, and only a token
/// the server actively refuses (with the refresh token refusing too) reports
/// `None`. Logging out is the user's decision, never a side effect of a lookup.
#[tauri::command]
pub async fn auth_get_user(
    settings: State<'_, SettingsState>,
) -> Result<Option<AuthUser>, String> {
    if read_tokens(&settings)?.is_none() {
        return Ok(None);
    }
    // 1. Renew proactively while the old token is still accepted. An ended
    //    session here means the refresh token was refused — file already gone.
    ensure_fresh(&settings).await;
    let Some(mut tokens) = read_tokens(&settings)? else {
        return Ok(None);
    };

    // 2. Confirm against the server. A rejection gets exactly one renew + retry;
    //    anything unreadable keeps the session, since the token is young.
    for attempt in 0..2 {
        match fetch_user_info(&tokens.access_token).await {
            Userinfo::User(user) => return Ok(Some(user)),
            Userinfo::Transient(e) => {
                log::warn!("auth_get_user: userinfo unreachable ({}), reporting stored user", e);
                return Ok(Some(stored_user(&tokens)));
            }
            Userinfo::Rejected(e) if attempt == 0 => {
                log::warn!("auth_get_user: token refused ({}), refreshing once", e);
                match force_renew(&settings, &mut tokens).await {
                    Renewal::Renewed => continue,
                    Renewal::Kept => return Ok(Some(stored_user(&tokens))),
                    Renewal::Ended => return Ok(None),
                }
            }
            Userinfo::Rejected(e) => {
                log::warn!("auth_get_user: token refused twice ({}), ending session", e);
                let _ = delete_tokens(&settings);
                return Ok(None);
            }
        }
    }
    Ok(None)
}

/// Log out: clear tokens and notify the server.
#[tauri::command]
pub async fn auth_logout(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<(), String> {
    log::info!("auth_logout");
    let tokens = read_tokens(&settings)?;

    if let Some(ref t) = tokens {
        if let Ok(client) = apply_dev_tls(reqwest::Client::builder()).build() {
            let _ = client
                .post(format!(
                    "{}/api/oauth2/logout?user_id={}",
                    base_url(),
                    t.user_id
                ))
                .header("Authorization", format!("Bearer {}", t.access_token))
                .send()
                .await;
        }
    }

    delete_tokens(&settings)?;
    let _ = app.emit("auth-logout", ());
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

/// What a userinfo probe says about the token it was given. Distinguishing "the
/// server refused this token" from "we could not ask" is what lets the session
/// survive the latter.
enum Userinfo {
    User(AuthUser),
    Rejected(String),
    Transient(String),
}

async fn fetch_user_info(access_token: &str) -> Userinfo {
    let client = match apply_dev_tls(reqwest::Client::builder()).build() {
        Ok(c) => c,
        Err(e) => return Userinfo::Transient(format!("Failed to build HTTP client: {}", e)),
    };
    let resp = match client
        .get(format!("{}/api/oauth2/userinfo", base_url()))
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Userinfo::Transient(err_chain(&e)),
    };

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Userinfo::Rejected(format!("HTTP {status}"));
    }
    if !status.is_success() {
        return Userinfo::Transient(format!("HTTP {status}"));
    }

    match resp.json::<serde_json::Value>().await {
        Ok(json) => Userinfo::User(AuthUser {
            name: json["name"].as_str().unwrap_or("").to_string(),
            email: json["email"].as_str().unwrap_or("").to_string(),
        }),
        Err(e) => Userinfo::Transient(format!("Failed to parse userinfo response: {}", e)),
    }
}

/// Get the current user's identifier (email preferred, fallback to user_id).
/// Used by other modules (e.g., XP, dictation) to identify the user.
///
/// Resolution order:
/// 1. The current workspace's owner (`workspace.json` -> `user_id`), so that
///    per-workspace progress data is always attributed to the workspace owner
///    even if a different user is logged in globally.
/// 2. The workspace's `auth.json` tokens (unclaimed workspace / no workspace).
pub(crate) fn get_current_user_email(settings: &SettingsState) -> String {
    // Prefer the workspace owner from workspace.json.
    if let Some(ws_dir) = settings.workspace_dir.lock().unwrap().as_ref() {
        if let Some(ws) = crate::workspace::load_workspace_json(ws_dir) {
            if !ws.user_id.is_empty() && ws.user_id != "local" {
                return ws.user_id;
            }
        }
    }
    // Fallback: auth tokens of the current workspace (or legacy global).
    let tokens = match read_tokens(settings).ok().flatten() {
        Some(t) => t,
        None => return String::new(),
    };
    // Prefer email, fall back to user_id if email is empty.
    if !tokens.email.is_empty() {
        tokens.email
    } else if !tokens.user_id.is_empty() {
        tokens.user_id
    } else {
        tokens.username
    }
}

/// Stable identity used to key per-workspace progress and XP data.
///
/// Unlike [`get_current_user_email`], this NEVER returns an empty string, so
/// logged-out practice in an unclaimed workspace is still recorded (and earns
/// XP) under a stable per-workspace sentinel instead of being silently dropped.
///
/// Resolution:
/// 1. Claimed workspace (`workspace.json` -> `user_id`) → the owner email. This
///    is stable across login/logout, so progress/XP never move out from under it.
/// 2. Selected but unclaimed workspace → the sentinel `"local"`. On claim,
///    [`crate::xp::migrate_identity_to_owner`] folds these rows into the owner.
/// 3. No workspace selected (legacy global DB) → the auth identity, else `"local"`.
pub(crate) fn workspace_identity(settings: &SettingsState) -> String {
    // Selected workspace: owner email when claimed, else a stable sentinel.
    if let Some(ws_dir) = settings.workspace_dir.lock().unwrap().as_ref() {
        if let Some(ws) = crate::workspace::load_workspace_json(ws_dir) {
            if !ws.user_id.is_empty() && ws.user_id != "local" {
                return ws.user_id;
            }
        }
        return "local".to_string();
    }
    // No workspace selected (legacy global DB): fall back to the auth identity.
    let id = get_current_user_email(settings);
    if id.is_empty() {
        "local".to_string()
    } else {
        id
    }
}

/// Open a URL in the default browser via the opener plugin (cross-platform,
/// works on desktop and Android/iOS).
fn open_url(app: &AppHandle, url: &str) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Failed to open browser: {}", e))
}

// ---------------------------------------------------------------------------
// Tests — the renewal calendar: the pure decisions behind "stay logged in until
// the user logs out". When a token is due for renewal, and when it is already
// past its life. No network, no workspace.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an unsigned JWT-shaped token carrying `exp` in its payload — the
    /// same shape the hub hands out (the app only reads the claim).
    fn jwt_with_exp(exp: u64) -> String {
        use base64::Engine;
        let seg = |json: String| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        format!(
            "{}.{}.sig",
            seg(r#"{"alg":"RS256","typ":"JWT"}"#.to_string()),
            seg(format!(r#"{{"sub":"user-1","exp":{exp}}}"#))
        )
    }

    fn tokens(access: &str, obtained_at: u64) -> AuthTokens {
        AuthTokens {
            access_token: access.to_string(),
            refresh_token: "r".repeat(32),
            user_id: "user-1".to_string(),
            username: "Tester".to_string(),
            email: "t@example.com".to_string(),
            obtained_at,
        }
    }

    #[test]
    fn expiry_is_read_from_the_payload() {
        assert_eq!(token_expiry(&jwt_with_exp(1_700_000_000)), Some(1_700_000_000));
        // Not a JWT: the caller falls back to the age-based schedule.
        assert_eq!(token_expiry("opaque-token"), None);
        assert_eq!(token_expiry(""), None);
    }

    #[test]
    fn a_token_with_days_left_is_not_renewed_yet() {
        let t = tokens(&jwt_with_exp(now_unix() + 6 * 24 * 3600), now_unix());
        assert!(!token_is_stale(&t), "a fresh token must not cost a round trip");
        assert!(!token_is_dead(&t));
    }

    #[test]
    fn a_token_inside_the_margin_is_renewed_before_it_dies() {
        let t = tokens(&jwt_with_exp(now_unix() + TOKEN_RENEW_MARGIN_SECS / 2), now_unix());
        assert!(token_is_stale(&t), "renewal must start while the hub still accepts us");
        assert!(!token_is_dead(&t), "stale is not dead — the old token still works");
    }

    #[test]
    fn a_past_expiry_token_is_stale_and_dead() {
        let t = tokens(&jwt_with_exp(now_unix() - 60), now_unix() - 8 * 24 * 3600);
        assert!(token_is_stale(&t));
        assert!(token_is_dead(&t));
    }

    #[test]
    fn opaque_tokens_fall_back_to_the_age_schedule() {
        // A file written before `obtained_at` existed (0) is renewed on sight.
        assert!(token_is_stale(&tokens("opaque", 0)));
        let fresh = tokens("opaque", now_unix() - TOKEN_MAX_AGE_SECS + 3600);
        assert!(!token_is_stale(&fresh));
        assert!(!token_is_dead(&fresh));
        let aged = tokens("opaque", now_unix() - TOKEN_MAX_AGE_SECS - 3600);
        assert!(token_is_stale(&aged));
    }

    #[test]
    fn legacy_auth_json_without_obtained_at_still_parses() {
        // Sessions stored before this field existed must load, not error out.
        let legacy = r#"{"access_token":"a","refresh_token":"b","user_id":"u","username":"n"}"#;
        let parsed: AuthTokens = serde_json::from_str(legacy).expect("legacy auth.json must parse");
        assert_eq!(parsed.obtained_at, 0);
        assert_eq!(parsed.email, "");
    }

    #[test]
    fn stored_user_prefers_the_username_and_falls_back_to_the_id() {
        let u = stored_user(&tokens("opaque", now_unix()));
        assert_eq!(u.name, "Tester");
        assert_eq!(u.email, "t@example.com");
        // A deep-link login that carried no username reports the user_id.
        let mut anon = tokens("opaque", now_unix());
        anon.username = String::new();
        assert_eq!(stored_user(&anon).name, "user-1");
    }
}
