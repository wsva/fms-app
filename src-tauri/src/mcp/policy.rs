//! Policy tiers and the audit journal for the built-in MCP server.
//!
//! `zone_guard` (sync/server.rs) answers *who may talk to us* — and its answer
//! for anything on loopback is "trusted, no credentials". That is not the same
//! claim as *may destroy*: a third-party agent attached over `/mcp` inherits
//! the loopback zone's trust and would otherwise inherit the whole tool set
//! with it, including `dataset_delete_database`. This module supplies the
//! second half.
//!
//! Every tool call that reaches the router is classified into a [`Tier`],
//! journalled to the `mcp_audit` table in the app DB, and then either delegated
//! to the real handler or refused. It ships in **audit mode**: the default
//! `mcp_policy` is `"permissive"`, which classifies and journals everything and
//! denies nothing, so every already-attached agent keeps working while the
//! classification table is proven against real traffic. `"enforce"`
//! additionally refuses the two tiers that cannot be undone.
//!
//! # The seam
//!
//! Enforcement is a wrapper around each `ToolRoute.call`, installed once by
//! [`guard`] on the merged router. Not a `ServerHandler` wrapper: `#[tool_handler]`
//! already generates `call_tool` for `DatasetMcpServer`, and not a body-sniffing
//! axum middleware: the wrapper sits below the transport, so it still applies
//! when a stdio bridge is added later.
//!
//! # `agent_id`
//!
//! Always `"loopback"` today. A per-agent tier needs the connection record from
//! integration.md §2 (which does not exist: the app no longer dials an agent, so
//! an attached agent cannot be told apart from another local process), so the
//! column is here so the journal does not have to be migrated when it does.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rmcp::handler::server::tool::{DynCallToolHandler, ToolCallContext, ToolRoute, ToolRouter};
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock, JsonObject};
use rmcp::ErrorData;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use super::DatasetMcpServer;
use crate::settings::SettingsState;

/// rmcp's `MaybeBoxFuture` with the `local` feature off, which is the
/// configuration this crate builds with. Spelled out because the alias itself is
/// `pub(crate)` upstream.
type BoxToolFuture<'s> =
    Pin<Box<dyn Future<Output = Result<CallToolResponse, ErrorData>> + Send + 's>>;

/// `AppSettings.mcp_policy` values.
pub(crate) const PERMISSIVE: &str = "permissive";
pub(crate) const ENFORCE: &str = "enforce";

/// Placeholder until a caller carries an identity. See the module docs.
const AGENT_ID: &str = "loopback";

/// Rows kept in `mcp_audit`. Pruned in place so a long-running hub does not grow
/// the app DB without bound.
const AUDIT_RETENTION: i64 = 5_000;
/// Prune only every Nth insert — the DELETE is a scan, the insert is not.
const PRUNE_EVERY: i64 = 256;

const AUDIT_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS mcp_audit (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL DEFAULT (datetime('now')),
    tool        TEXT NOT NULL,
    tier        TEXT NOT NULL,
    decision    TEXT NOT NULL,
    agent_id    TEXT NOT NULL DEFAULT 'loopback',
    args_digest TEXT NOT NULL DEFAULT ''
);";

// ---------------------------------------------------------------------------
// Tiers
// ---------------------------------------------------------------------------

/// What a tool call is allowed to cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    /// Observes state and changes nothing.
    Read,
    /// Changes content, but through a path that can be undone — the app trash, a
    /// soft delete, a re-downloadable artifact, or a derived index.
    Write,
    /// Loses content permanently, or makes a structural change with no inverse.
    Destructive,
    /// Drives the user's screen rather than the data.
    UiControl,
    /// Deliberately not automatable: the tool exists so a *human* can make the
    /// call, and an agent asking for it is asking for the wrong thing.
    HumanHandoff,
}

impl Tier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Tier::Read => "read",
            Tier::Write => "write",
            Tier::Destructive => "destructive",
            Tier::UiControl => "ui-control",
            Tier::HumanHandoff => "human-handoff",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Tier> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Tier::Read),
            "write" => Some(Tier::Write),
            "destructive" => Some(Tier::Destructive),
            "ui-control" | "uicontrol" | "ui_control" => Some(Tier::UiControl),
            "human-handoff" | "humanhandoff" | "human_handoff" => Some(Tier::HumanHandoff),
            _ => None,
        }
    }
}

/// Every tier that `"enforce"` refuses. Both are the same shape of mistake: an
/// action whose consequences outlive the call.
fn is_denied(tier: Tier) -> bool {
    matches!(tier, Tier::Destructive | Tier::HumanHandoff)
}

/// Segments of a tool name that mean "this loses something". Matched on
/// `_`-separated segment *equality*, not substring: the seven words are the
/// vocabulary the app asks the user about when a call is classified destructive,
/// extended with the ones that turned up in the real tool set (`evict`,
/// `flatten`, `detach`, `rollback`, `remint`, `purge`, `forget`).
const DESTRUCTIVE_WORDS: [&str; 14] = [
    "delete", "remove", "clear", "drop", "revoke", "reset", "rebuild", "evict", "purge", "forget",
    "flatten", "detach", "rollback", "remint",
];

/// Tools that carry a destructive word but do not lose anything the user cannot
/// get back. Each entry is a deliberate decision, not an oversight — the reason
/// is the tool's own contract.
const RECOVERABLE: [&str; 18] = [
    // Whole-directory deletes land in the app trash and return `trashed_to`.
    "book_delete",
    "card_dataset_delete",
    "dataset_delete",
    "read_aloud_delete",
    "wiki_dataset_delete",
    "wiki_dataset_delete_file",
    // Derived artifacts are regenerable (`book_generate_artifact`) and every
    // delete is journalled, so the row is reconstructible; a user note is the
    // one non-derived kind and it is still recoverable from the journal.
    "book_delete_artifact",
    // A glossary term is re-extractable (`book_extract_terms`) and the delete
    // is journalled, so the definition is reconstructible.
    "book_delete_term",
    // Soft deletes: the row is marked, not dropped, precisely so sync can carry
    // the tombstone.
    "card_delete",
    "card_tag_delete",
    // Soft-deletes the row; the bytes stay in the dataset dir so an undo of a
    // card delete still finds its media, and they ride the file sync plane.
    "card_media_delete",
    // Edits a list, not content — `card_dataset_add_subscriber` is the inverse.
    "card_dataset_remove_subscriber",
    // Unlinks a location; the files on disk are untouched and `dataset_add_dir`
    // is the inverse.
    "dataset_remove_dir",
    // Moves the overlay to the trash and reverts to a pristine base.
    "dataset_layer_detach",
    // Regenerates a derived index from the cards that are still there.
    "card_fts_rebuild",
    // The in-memory ring only; `log_read_file_history` still sees the file.
    "log_clear",
    // A re-downloadable cache artifact; `ocr_hyphen_dictionary_download`
    // restores it byte-identically.
    "ocr_hyphen_dictionary_delete",
    // Creates a new version with the restored cues "without deleting history".
    "subtitle_rollback_to_version",
];

/// Drives the user's screen. `ui_invoke_action` is listed ahead of the UI action
/// registry landing so the tier is already right when it does.
const UI_CONTROL: [&str; 7] = [
    "app_navigate",
    "app_notify",
    "app_open_review",
    // Nothing on disk changes, but it raises an OS explorer window in front of
    // the user — the screen, not the app's data.
    "app_open_workspace_dir",
    "app_start_dictation",
    "capture_screenshot",
    "ui_invoke_action",
];

/// Irreversible *and* explicitly a human's call — neither name contains a word
/// from [`DESTRUCTIVE_WORDS`], so they are named here. `todo_add`/`todo_complete`
/// join this list when the TODO domain lands: a todo is the same shape of thing,
/// work the agent hands to the user instead of doing.
const HUMAN_HANDOFF: [&str; 2] = [
    // Raises the hub's authority epoch, which refuses every follower still
    // presenting the old one. There is no "un-raise".
    "sync_handover_epoch",
    // Hands journal authority to the cloud. Its own contract says it: "NEVER
    // automatic (INV-13 forbids failover)".
    "sync_handover_to_cloud",
];

/// Reads, named explicitly. A prefix heuristic was tried first and produced
/// false positives in both directions (`wiki_dataset_index` mutates,
/// `read_aloud_*` starts with "read", `sync_check_availability` persists the
/// availability it checks), so this is a list. Anything absent falls through to
/// [`Tier::Write`] — a new tool is never silently a read.
///
/// Also left at the default [`Tier::Write`] on purpose, and recorded here because
/// both reach *another machine's* disk: `dataset_push_to_hub` and
/// `dataset_push_all` replace the hub's copy of a dataset, but every file they
/// overwrite or drop is moved into the hub's app trash first (the ingest commit
/// reports it as `trashed_to`), so the act is undoable where it happened.
/// Neither name carries a word from [`DESTRUCTIVE_WORDS`].
const READ_TOOLS: [&str; 96] = [
    "app_audit_get",
    "app_bootstrap",
    "app_events",
    "app_get_ui_state",
    "app_trash_list",
    "app_update_check",
    "app_version",
    "auth_status",
    "book_list",
    "book_list_artifact_jobs",
    "book_list_artifacts",
    "book_list_chapters",
    "book_list_sentences",
    "book_list_terms",
    "book_list_words",
    "card_dataset_list",
    "card_get",
    "card_get_tags",
    "card_list",
    "card_media_list",
    "card_media_register",
    "card_search",
    "card_sync_status",
    "card_tag_list",
    "card_test_next",
    "card_test_next_all",
    "card_test_stats",
    "card_test_stats_all",
    "chat_messages",
    "dataset_audit",
    "dataset_family",
    "dataset_full_report",
    "dataset_get",
    "dataset_get_summary",
    "dataset_info_get",
    "dataset_layer_status",
    "dataset_list",
    "dataset_list_cues",
    "dataset_list_dirs",
    "dataset_list_favorite_cues",
    "dataset_list_files",
    "dataset_list_media",
    "dataset_list_subtitles",
    "dataset_push_preview",
    "dataset_read_file",
    "hub_kind",
    "hub_local_datasets",
    "hub_upload_state",
    "llm_check_connection",
    "llm_list_models",
    "log_get_file_path",
    "log_get_history",
    "log_read_file_history",
    "mail_head",
    "mail_list",
    "mail_wait",
    "market_list",
    "model_status",
    "ocr_hyphen_dictionary_status",
    "ocr_list_languages",
    "pairing_list_devices",
    "pc_sync_status",
    "read_aloud_align_proposals",
    "read_aloud_align_transcript",
    "read_aloud_audit",
    "read_aloud_feedback_context",
    "read_aloud_get_text",
    "read_aloud_list",
    "read_aloud_list_attempts",
    "read_aloud_list_texts",
    "read_aloud_model_status",
    "read_aloud_standard_audio_assess",
    "read_aloud_standard_audio_list",
    "read_aloud_standard_audio_probe",
    "read_aloud_voice_resolve",
    "settings_get",
    "subtitle_get_cues_at_version",
    "subtitle_get_versions",
    "sync_changes_since",
    "sync_diagnose_queue",
    "sync_status_detail",
    "todo_list",
    "tts_list_voices",
    "ui_list_actions",
    "web_service_get_status",
    "wiki_dataset_list",
    "wiki_dataset_list_dir",
    "wiki_dataset_read_file",
    "wiki_dataset_search",
    "wiki_hub_list",
    "wiki_hub_search",
    "workflow_builtin_templates",
    "workflow_get_definition",
    "workflow_list",
    "workflow_list_actions",
    "workflow_status",
];

/// Classify one tool name. Order matters: the explicit sets win over the word
/// heuristic, and the word heuristic wins over the read list, so a name that
/// somehow lands in both is treated as the more dangerous of the two.
pub(crate) fn classify(tool: &str) -> Tier {
    if HUMAN_HANDOFF.contains(&tool) {
        return Tier::HumanHandoff;
    }
    if UI_CONTROL.contains(&tool) {
        return Tier::UiControl;
    }
    let destructive_word = tool.split('_').any(|seg| DESTRUCTIVE_WORDS.contains(&seg));
    if destructive_word && !RECOVERABLE.contains(&tool) {
        return Tier::Destructive;
    }
    if READ_TOOLS.contains(&tool) {
        return Tier::Read;
    }
    Tier::Write
}

// ---------------------------------------------------------------------------
// Mode
// ---------------------------------------------------------------------------

static WARNED_UNKNOWN_MODE: AtomicBool = AtomicBool::new(false);

/// Normalise `AppSettings.mcp_policy`. An unrecognised value degrades to
/// `"permissive"` — the whole point of audit mode is that a typo in a settings
/// file must not brick the tool set — but it is logged, once, so it does not go
/// unnoticed either.
pub(crate) fn normalize_mode(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        ENFORCE => ENFORCE,
        PERMISSIVE => PERMISSIVE,
        other => {
            if !WARNED_UNKNOWN_MODE.swap(true, Ordering::Relaxed) {
                log::warn!(
                    "[MCP/policy] unknown mcp_policy {:?}; using {:?}. Set it with settings_set.",
                    other,
                    PERMISSIVE
                );
            }
            PERMISSIVE
        }
    }
}

fn policy_mode(app: &AppHandle) -> &'static str {
    let Some(settings) = app.try_state::<SettingsState>() else {
        return PERMISSIVE;
    };
    // Bound to a local: returning the `match` directly would keep the
    // `MutexGuard` alive until after `settings` is dropped.
    let mode = match settings.settings.lock() {
        Ok(s) => normalize_mode(&s.mcp_policy),
        Err(_) => PERMISSIVE,
    };
    mode
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

/// First 16 hex chars of SHA-256 over the serialized arguments. A digest, not
/// the arguments: they routinely carry text the user typed, and the journal's
/// job is to correlate calls, not to store content.
fn args_digest(arguments: Option<&JsonObject>) -> String {
    let Some(arguments) = arguments else {
        return String::new();
    };
    let Ok(serialized) = serde_json::to_string(arguments) else {
        return String::new();
    };
    let hash = Sha256::digest(serialized.as_bytes());
    hash.iter().take(8).map(|b| format!("{:02x}", b)).collect()
}

/// Append one row. Never fatal: a locked or missing app DB must not turn a
/// working tool call into a failed one, so the failure is logged and dropped.
fn journal(app: &AppHandle, tool: &str, tier: Tier, decision: &str, digest: &str) {
    let Some(settings) = app.try_state::<SettingsState>() else {
        return;
    };
    if let Err(e) = append_audit(&settings, tool, tier, decision, digest) {
        log::warn!("[MCP/policy] could not journal `{}`: {}", tool, e);
    }
}

fn append_audit(
    settings: &SettingsState,
    tool: &str,
    tier: Tier,
    decision: &str,
    digest: &str,
) -> Result<(), String> {
    let conn = crate::datasets::dictation::open_app_db(settings)?;
    ensure_schema(&conn)?;
    conn.execute(
        "INSERT INTO mcp_audit (ts, tool, tier, decision, agent_id, args_digest)
         VALUES (datetime('now'), ?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![tool, tier.as_str(), decision, AGENT_ID, digest],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    if id % PRUNE_EVERY == 0 {
        conn.execute(
            "DELETE FROM mcp_audit WHERE id <= ?1",
            rusqlite::params![id.saturating_sub(AUDIT_RETENTION)],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Run on every open rather than once per process: the app DB path follows the
/// active workspace, so a process-global flag would skip the second workspace's
/// file. `IF NOT EXISTS` makes the repeat a no-op, which is exactly what
/// `open_app_db` itself does for `listen_dictation`.
fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(AUDIT_SCHEMA).map_err(|e| e.to_string())
}

/// Newest first. Backs the `app_audit_get` tool.
pub(crate) fn audit_query(
    settings: &SettingsState,
    limit: usize,
    tier: Option<Tier>,
) -> Result<Vec<serde_json::Value>, String> {
    let conn = crate::datasets::dictation::open_app_db(settings)?;
    ensure_schema(&conn)?;

    let limit = limit.clamp(1, AUDIT_RETENTION as usize) as i64;
    let mut sql = String::from(
        "SELECT id, ts, tool, tier, decision, agent_id, args_digest FROM mcp_audit",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(tier) = tier {
        sql.push_str(" WHERE tier = ?");
        binds.push(Box::new(tier.as_str().to_string()));
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");
    binds.push(Box::new(limit));

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("cannot read mcp_audit: {}", e))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "ts": row.get::<_, String>(1)?,
                "tool": row.get::<_, String>(2)?,
                "tier": row.get::<_, String>(3)?,
                "decision": row.get::<_, String>(4)?,
                "agent_id": row.get::<_, String>(5)?,
                "args_digest": row.get::<_, String>(6)?,
            }))
        })
        .map_err(|e| format!("cannot read mcp_audit: {}", e))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Fields of `settings_set` that would change the policy governing the caller.
/// An agent that can rewrite its own leash has no leash, so under `"enforce"`
/// these are refused whatever tier the tool itself sits in — the user makes this
/// change on the Agent page (Tools → Agent → Surface), which an attached agent
/// has no write path to.
///
/// `mcp_enabled` is the same escalation one level up: turning the server off is
/// how an agent would silence the journal and the rest of the tool set, and
/// turning it on again is a decision about what this machine exposes.
const SELF_GOVERNING_SETTINGS: [&str; 2] = ["mcp_policy", "mcp_enabled"];

fn escalation_reason(field: &str) -> &'static str {
    match field {
        "mcp_enabled" => "whether this server's MCP surface exists at all",
        _ => "the setting that decides what this server refuses",
    }
}

/// `Some(field)` when this call would loosen the caller's own policy.
fn escalates_policy(tool: &str, arguments: Option<&JsonObject>) -> Option<&'static str> {
    if tool != "settings_set" {
        return None;
    }
    let fields = arguments?.get("settings")?.as_object()?;
    SELF_GOVERNING_SETTINGS
        .iter()
        .copied()
        .find(|field| fields.contains_key(*field))
}

fn deny_message(tool: &str, tier: Tier, escalation: Option<&str>) -> String {
    if let Some(field) = escalation {
        return format!(
            "`{}` was refused because it writes `{}`, {}. An agent that can rewrite its own \
             surface has none. Nothing was changed.\n\
             \n\
             Ask the user to make this change themselves on the Agent page \
             (Tools → Agent → Surface). Every other \
             settings field is still writable through `settings_set`.\n\
             \n\
             The attempt was recorded in the audit journal (`app_audit_get`).",
            tool,
            field,
            escalation_reason(field)
        );
    }
    let what = match tier {
        Tier::HumanHandoff => format!(
            "`{}` is a human decision, not an agent one — it is classified human-handoff \
             because its consequences outlive this session and cannot be rolled back from here.",
            tool
        ),
        _ => format!(
            "`{}` is classified destructive: it loses content permanently or makes a \
             structural change with no inverse.",
            tool
        ),
    };
    format!(
        "{} This server's MCP policy is \"{}\", so the call was refused and nothing was changed.\n\
         \n\
         What to do instead:\n\
         - Prefer the trash-backed equivalent. `dataset_delete`, `book_delete`, `card_dataset_delete`, \
         `wiki_dataset_delete` and `read_aloud_delete` move the whole directory into the app trash \
         and return `trashed_to`, so the user can undo it. Those are allowed under this policy.\n\
         - If there is no trash-backed equivalent, tell the user what you were about to do and let \
         them run it from the UI.\n\
         - If the user wants agents to be able to do this, the setting is `mcp_policy`: \
         `settings_set {{\"settings\": {{\"mcp_policy\": \"permissive\"}}}}`.\n\
         \n\
         The attempt was recorded in the audit journal (`app_audit_get`).",
        what,
        ENFORCE
    )
}

// ---------------------------------------------------------------------------
// The guard
// ---------------------------------------------------------------------------

/// Wrap every route in `router` so each call is classified and journalled before
/// it reaches the real handler. Called once, from
/// [`DatasetMcpServer::tool_router`].
pub(crate) fn guard(
    mut router: ToolRouter<DatasetMcpServer>,
) -> ToolRouter<DatasetMcpServer> {
    for route in router.map.values_mut() {
        let tool = route.attr.name.to_string();
        let inner = route.call.clone();
        let attr = route.attr.clone();
        *route = ToolRoute::new_dyn(attr, move |ctx| dispatch(inner.clone(), tool.clone(), ctx));
    }
    router
}

/// classify -> journal -> (deny | delegate).
///
/// A separate generic-lifetime function rather than a closure body so the
/// `for<'s>` in `DynCallToolHandler` comes from `ToolRoute::new_dyn`'s bound
/// instead of having to be inferred through an `Arc::new` coercion.
fn dispatch<'s>(
    inner: Arc<DynCallToolHandler<DatasetMcpServer>>,
    tool: String,
    ctx: ToolCallContext<'s, DatasetMcpServer>,
) -> BoxToolFuture<'s> {
    Box::pin(async move {
        let app = ctx.service.app.clone();
        let digest = args_digest(ctx.arguments.as_ref());
        let tier = classify(&tool);
        let mode = policy_mode(&app);
        let enforcing = mode == ENFORCE;
        let escalation = if enforcing {
            escalates_policy(&tool, ctx.arguments.as_ref())
        } else {
            None
        };
        let denied = escalation.is_some() || (enforcing && is_denied(tier));
        journal(
            &app,
            &tool,
            tier,
            if denied { "deny" } else { "allow" },
            &digest,
        );
        if denied {
            log::warn!(
                "[MCP/policy] denied `{}` ({}) under \"enforce\"",
                tool,
                tier.as_str()
            );
            return Ok(
                CallToolResult::error(vec![ContentBlock::text(deny_message(
                    &tool, tier, escalation,
                ))])
                .into(),
            );
        }
        inner(ctx).await
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The classification table is a hand-maintained list of ~190 names; this is
    /// what stops it rotting. A new tool carrying a destructive word must be
    /// placed in one of the two lists deliberately, or the build fails.
    #[test]
    fn destructive_words_stay_destructive() {
        let router = DatasetMcpServer::tool_router();
        assert!(!router.map.is_empty(), "router is empty — feature gating?");
        for name in router.map.keys() {
            let carries = name.split('_').any(|seg| DESTRUCTIVE_WORDS.contains(&seg));
            if !carries {
                continue;
            }
            let tier = classify(name);
            assert!(
                tier == Tier::Destructive || RECOVERABLE.contains(&name.as_ref()),
                "`{}` carries a destructive word but classifies as {:?}. Add it to \
                 DESTRUCTIVE_WORDS' effect (leave it alone) or list it in RECOVERABLE \
                 with the reason it is undoable.",
                name,
                tier
            );
        }
    }

    /// A read list that names a tool which no longer exists silently stops
    /// protecting anything.
    #[test]
    fn every_listed_name_is_a_real_tool() {
        let router = DatasetMcpServer::tool_router();
        for list in [
            ("READ_TOOLS", &READ_TOOLS[..]),
            ("RECOVERABLE", &RECOVERABLE[..]),
            ("UI_CONTROL", &UI_CONTROL[..]),
            ("HUMAN_HANDOFF", &HUMAN_HANDOFF[..]),
        ] {
            for name in list.1 {
                assert!(
                    router.map.contains_key(*name),
                    "{} lists `{}`, which is not a tool",
                    list.0,
                    name
                );
            }
        }
    }

    #[test]
    fn unknown_names_default_to_write() {
        assert_eq!(classify("brand_new_tool"), Tier::Write);
        assert_eq!(classify(""), Tier::Write);
    }

    /// `settings_set` is an ordinary write, except for the one field that decides
    /// what an ordinary write is allowed to be.
    #[test]
    fn only_the_policy_field_escalates() {
        use serde_json::json;
        let args = |v: serde_json::Value| {
            v.as_object().cloned().unwrap()
        };
        assert_eq!(
            escalates_policy("settings_set", Some(&args(json!({ "settings": { "mcp_policy": "permissive" } })))),
            Some("mcp_policy")
        );
        assert_eq!(
            escalates_policy("settings_set", Some(&args(json!({ "settings": { "llm_model": "x" } })))),
            None
        );
        assert_eq!(escalates_policy("settings_set", None), None);
        assert_eq!(
            escalates_policy("settings_get", Some(&args(json!({ "settings": { "mcp_policy": "x" } })))),
            None
        );
    }

    #[test]
    fn modes_normalise_and_denies_match() {
        assert_eq!(normalize_mode("enforce"), ENFORCE);
        assert_eq!(normalize_mode("  ENFORCE "), ENFORCE);
        assert_eq!(normalize_mode("permissive"), PERMISSIVE);
        assert_eq!(normalize_mode("nonsense"), PERMISSIVE);
        assert!(is_denied(Tier::Destructive));
        assert!(is_denied(Tier::HumanHandoff));
        assert!(!is_denied(Tier::Write));
        assert!(!is_denied(Tier::UiControl));
        assert!(!is_denied(Tier::Read));
    }

    #[test]
    fn tier_strings_round_trip() {
        for tier in [
            Tier::Read,
            Tier::Write,
            Tier::Destructive,
            Tier::UiControl,
            Tier::HumanHandoff,
        ] {
            assert_eq!(Tier::parse(tier.as_str()), Some(tier));
        }
        assert_eq!(Tier::parse("nope"), None);
    }
}
