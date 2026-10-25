//! The agent manual: one generator, three renderings (integration.md §5.0).
//!
//! `/agent.md`, `/agent.json`, `app_bootstrap` and the Agent page's "Attach an
//! agent" section are all the *same object* rendered differently. Nothing here is
//! `include_str!`'d and no port or tool name is written down twice: the body is
//! built from the live [`DatasetMcpServer::tool_router`] registry and from
//! [`WebServiceState::build_status`], so a tool rename or a port conflict shows
//! up in all three renderings at once. If the card says 35712 and the manual
//! says 35711, the design has already failed.
//!
//! The markdown is generated *from the JSON value*, not alongside it — one body,
//! two serializers, so they cannot disagree.
//!
//! Deliberately **not** in here: full tool input schemas. A conforming MCP client
//! already gets those from `tools/list`, and duplicating ~190 schemas into every
//! `app_bootstrap` response would cost thousands of tokens to say nothing new.
//! The index carries names, tiers and a one-line summary; `tools/list` carries
//! the rest.

use rmcp::model::Tool;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::policy;
use super::DatasetMcpServer;
use crate::sync::server::{WebServiceConfig, WebServiceState, WebServiceStatus};

/// Tool-name prefix → the domain submodule that owns it. Longest prefix wins, so
/// `read_aloud_model_status` lands in `read-aloud` and not `models`. Kept in step
/// with the `mod` list in `mcp/mod.rs`; [`tests::every_tool_has_a_domain`] fails
/// the build when a new prefix shows up unmapped.
///
/// The domain a tool belongs to and the *file* that implements it are separate
/// questions: `read_aloud_*` still lives in `mcp/books.rs`, but it is addressed
/// as the `read-aloud` domain because `fms://docs/domain/{domain}` is the slice
/// an agent reads before it starts, and "books" is not a word a read-aloud task
/// begins with.
const DOMAIN_PREFIXES: [(&str, &str); 27] = [
    ("dataset_", "datasets"),
    ("subtitle_", "datasets"),
    ("dictation_", "dictation"),
    ("card_", "cards"),
    ("book_", "books"),
    ("read_aloud_", "read-aloud"),
    ("model_", "models"),
    ("llm_", "ai"),
    ("chat_", "ai"),
    ("tts_", "ai"),
    ("wiki_", "wiki"),
    ("sync_", "sync"),
    ("hub_", "sync"),
    ("market_", "sync"),
    ("pairing_", "sync"),
    ("pc_", "sync"),
    ("web_service_", "sync"),
    ("settings_", "system"),
    ("auth_", "system"),
    ("ocr_", "system"),
    ("log_", "system"),
    ("capture_", "system"),
    ("text_", "system"),
    ("app_", "system"),
    ("workflow_", "workflow"),
    ("ui_", "ui"),
    ("todo_", "todo"),
];

/// The domain a name falls into when no prefix matches. A tool landing here is a
/// bug in the table above, not a new domain.
const UNMAPPED: &str = "misc";

/// The order domains are rendered in — the same order as the `mod` list, which
/// is roughly "content first, infrastructure last".
const DOMAIN_ORDER: [&str; 13] = [
    "datasets",
    "dictation",
    "cards",
    "books",
    "read-aloud",
    "models",
    "ai",
    "wiki",
    "sync",
    "system",
    "workflow",
    "ui",
    "todo",
];

/// The first five calls a cold agent should make, in order. Names only —
/// [`tests::first_calls_are_real_tools`] asserts each one exists.
const FIRST_CALLS: [(&str, &str); 5] = [
    (
        "app_get_ui_state",
        "Where the user is right now: active tab, selected dataset, open media and cue. `ui_mounted=false` means the webview has not reported yet — do not read that as 'nothing selected'.",
    ),
    (
        "auth_status",
        "Who is logged in. Several tool families (market, hub upload, sync handover) need an account and fail with an actionable message without one.",
    ),
    (
        "settings_get",
        "Directories, LLM config, sync role and the `mcp_policy` mode you are running under. Credentials come back as `\"***\"` by design.",
    ),
    (
        "dataset_list",
        "Every dataset with its uuid and kind. Datasets are uuid-keyed end to end — take the uuid from here, never from a name or a path.",
    ),
    (
        "dataset_full_report",
        "For one dataset: identity, info.json, the audit checks, workflow runs and model state in a single call. Its own description calls it START HERE.",
    ),
];

// ---------------------------------------------------------------------------
// Tool index
// ---------------------------------------------------------------------------

pub(crate) fn domain_for(name: &str) -> &'static str {
    let mut best: Option<&'static str> = None;
    let mut best_len = 0;
    for (prefix, domain) in DOMAIN_PREFIXES {
        if name.starts_with(prefix) && prefix.len() > best_len {
            best = Some(domain);
            best_len = prefix.len();
        }
    }
    best.unwrap_or(UNMAPPED)
}

/// The first sentence of a tool description. Long enough to say what the tool
/// does, short enough that 190 of them stay readable.
fn summary_of(tool: &Tool) -> String {
    let description = tool.description.as_deref().unwrap_or_default();
    let cut = description
        .find(". ")
        .map(|i| i + 1)
        .unwrap_or(description.len())
        .min(240);
    let mut out: String = description.chars().take(cut).collect();
    if out.chars().count() < description.chars().count() && !out.ends_with('.') {
        out.push('.');
    }
    out.trim().to_string()
}

/// One entry per tool in the merged router, sorted by domain then name.
pub(crate) fn tool_index() -> Vec<Value> {
    let router = DatasetMcpServer::tool_router();
    let mut entries: Vec<(usize, String, Value)> = router
        .map
        .values()
        .map(|route| {
            let name = route.attr.name.to_string();
            let domain = domain_for(&name);
            let order = DOMAIN_ORDER
                .iter()
                .position(|d| *d == domain)
                .unwrap_or(DOMAIN_ORDER.len());
            let entry = json!({
                "name": name,
                "domain": domain,
                "tier": policy::classify(&name).as_str(),
                "summary": summary_of(&route.attr),
            });
            (order, name, entry)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    entries.into_iter().map(|(_, _, entry)| entry).collect()
}

fn grouped_domains(tools: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for domain in DOMAIN_ORDER.iter().chain(std::iter::once(&UNMAPPED)) {
        let names: Vec<&str> = tools
            .iter()
            .filter(|t| t.get("domain").and_then(|d| d.as_str()) == Some(domain))
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        if names.is_empty() {
            continue;
        }
        out.push(json!({ "domain": domain, "count": names.len(), "tools": names }));
    }
    out
}

// ---------------------------------------------------------------------------
// The shared body
// ---------------------------------------------------------------------------

/// The bound port and whether the service is actually up. When the manual is
/// being read over HTTP the service is by definition running; the flag exists so
/// `app_bootstrap` and the Connect card can say so honestly when it is not.
pub(crate) fn endpoints(app: &AppHandle) -> Value {
    let status = app
        .try_state::<WebServiceState>()
        .map(|s| s.build_status())
        .unwrap_or_else(|| WebServiceStatus {
            running: false,
            // The configured default, which is what the service will try to bind
            // when it starts. Not a claim that it is bound there now.
            port: WebServiceConfig::default().port,
            stt: false,
            dataset: false,
            tts: false,
            local_url: None,
            lan_url: None,
        });

    let port = status.port;
    let base = format!("http://127.0.0.1:{}", port);
    json!({
        "running": status.running,
        "port": port,
        "transport": "streamable-http",
        "mcp": format!("{}/mcp", base),
        "manual_markdown": format!("{}/agent.md", base),
        "manual_json": format!("{}/agent.json", base),
        "status": format!("{}/api/v1/status", base),
        "lan_mcp": status.lan_url.map(|u| format!("{}/mcp", u)),
        "note": if status.running {
            "This is the port the service actually bound, not the configured default — the default can be taken, in which case the service moves. Re-read it from web_service_get_status rather than caching it across restarts."
        } else {
            "The web service is not running, so these URLs will not answer yet. Start it with web_service_start (or from Settings → Sync); the port may differ once it binds."
        },
    })
}

/// Which feature-gated tool families exist in *this* build. An agent that assumes
/// `text_split` is there and gets "tool not found" learns nothing; this says so
/// up front.
fn capabilities() -> Value {
    json!({
        "mcp_server": cfg!(feature = "server"),
        "local_stt": cfg!(feature = "stt"),
        "phoneme_scoring": cfg!(feature = "phoneme"),
        "text_split": cfg!(feature = "textsplit"),
        "workflow_engine": cfg!(feature = "desktop"),
        "ocr": cfg!(feature = "desktop"),
        "screen_capture": cfg!(feature = "desktop"),
        // The readable resources, by uri — the uri template for a dataset layout
        // is included as a template, not a literal uri (see `resources.rs`).
        "mcp_resources": super::resources::uris(),
        // The prompts (the goose recipes), by name — bodies come from
        // `prompts/get` (see `prompts.rs`).
        "mcp_prompts": super::prompts::names(),
    })
}

/// The build string advertised as the MCP server's version: `git_describe` when
/// the build has one, otherwise the Cargo version. The same two fields
/// `app_version` returns, so a client reading either gets a consistent answer.
pub(crate) fn build_id() -> String {
    let describe = option_env!("FMS_GIT_DESCRIBE").unwrap_or_default();
    if describe.is_empty() {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        describe.to_string()
    }
}

pub(crate) fn identity() -> Value {
    json!({
        "app": "FmS",
        "git_describe": option_env!("FMS_GIT_DESCRIBE").unwrap_or_default(),
        "package_version": env!("CARGO_PKG_VERSION"),
        "build_id": build_id(),
        "protocol_version": crate::sync::PROTOCOL_VERSION,
    })
}

/// The trust zone the reader is in. `/agent.md` is whitelisted in `zone_guard`,
/// so an unpaired peer can reach it and needs to be told what that does and does
/// not buy it.
fn trust(app: &AppHandle) -> Value {
    let policy_mode = app
        .try_state::<crate::settings::SettingsState>()
        .and_then(|s| s.settings.lock().map(|v| v.mcp_policy.clone()).ok())
        .unwrap_or_else(|| "permissive".to_string());
    let mode = policy::normalize_mode(&policy_mode);
    json!({
        "zone": "loopback",
        "credentials": "none",
        "note": "Requests from 127.0.0.1 with a local Host header are in the trusted zone and need no credentials at all. That is a statement about *who may talk to us*, not about what they may do — the policy mode below is the second half.",
        "policy": {
            "mode": mode,
            "modes": {
                "permissive": "classify and journal every call, deny nothing (the default)",
                "enforce": "additionally refuse the destructive and human-handoff tiers",
            },
            "tiers": ["read", "write", "destructive", "ui-control", "human-handoff"],
            "journal": "Every call is appended to the mcp_audit table; read it back with app_audit_get.",
            "escalation": "Under \"enforce\", settings_set refuses a patch that writes mcp_policy or mcp_enabled — an agent that can loosen, or switch off, its own surface has none.",
        },
    })
}

/// The rules that make an agent correct here. Each one is a real trap, not
/// general advice.
pub(crate) fn house_rules() -> Value {
    json!([
        "Datasets are uuid-keyed. Take the uuid from dataset_list / dataset_full_report; never derive one from a name or a path.",
        "Prefer the trash over a destructive confirm. dataset_delete, book_delete, card_dataset_delete, wiki_dataset_delete and read_aloud_delete move the whole directory into the app trash and return `trashed_to`, so the user can undo them. The undo is itself a tool: app_trash_list shows what is in there (with the uuid each entry still carries) and app_restore moves one back out. The tools that lose data permanently have no trash behind them.",
        "Inside a workflow run, the per-step read path is workflow_verify, and workflow_adopt is what commits it. dataset_audit is a subject-level health check, not a per-step read.",
        "dataset_full_report is the single call for model state on one dataset — audit checks, workflow runs and info.json together. Prefer it over composing three reads.",
        "Structured JSON in, structured JSON out. Every tool returns an object with a status/ok field; parse it rather than pattern-matching prose.",
        "Some families only exist in some builds — see `capabilities`. A \"tool not found\" is usually a missing cargo feature, not a typo.",
        "The whole MCP server is desktop-only. The Android thin client has no /mcp at all; it consumes the desktop over the network.",
        "app_* tools drive the user's screen (navigate, notify, open a panel). They are fire-and-forget events to the webview — confirm the effect with app_get_ui_state rather than assuming it landed.",
        "To watch a job the app is running (a subtitle pass, a sync push, a workflow step), poll app_events with the `after` cursor instead of asking each domain in turn — it is the same log timeline the log_* tools read. `evicted_since_cursor` is non-zero when you fell behind the 1000-entry ring, and log_read_file_history is the archive.",
        "Before authoring a workflow definition, call workflow_list_actions: the step ids and commands it returns are the vocabulary the app can run, published by its Workflow page. Invented step ids fail workflow_validate, and the dictation templates under workflow_builtin_templates are a known-good starting point.",
    ])
}

/// integration.md §5.0 item 6: the way in, spelled as calls. There is one door —
/// attach over MCP and drive the app from your own window. FmS used to offer a
/// second one (it dialled an agent over ACP and hosted the session in its own
/// dock); that direction was removed, so nothing here advertises it.
fn paths(endpoints: &Value) -> Value {
    let mcp = endpoints.get("mcp").and_then(|v| v.as_str()).unwrap_or_default();
    json!({
        "attach": {
            "name": "Attach (your own window)",
            "available": true,
            "how": format!(
                "Register {} as a streamable-HTTP MCP server in your own client. No credentials on loopback. Every tool works, including the app_* ones that drive the FmS window. A client that speaks stdio MCP only points at this app's executable with `--mcp-bridge` instead — same tools, proxied.",
                mcp
            ),
            "trade_off": "FmS sees none of your reasoning: no streamed text, no thoughts, no tool-call cards, no plan, no permission dialog. The user watches your window — so say what you did with app_notify, and use todo_add when you need the user to act.",
        },
    })
}

/// integration.md §5.0 item 10: the generic pointer sentence the Agent page
/// copies, and the per-product registration lines. Both are generated from the
/// same bound port, and both are copy-only — FmS never writes into another
/// product's config files (§6).
///
/// The Agent page renders only `pointer_sentence` (it dropped the rest as copies
/// of one instruction — integration.md §4.1). The per-product lines stay here
/// because this document is also the install page a *human* reads when adding a
/// second client, and a client's registration syntax is the one thing an agent
/// reading the manual cannot derive: the transport flag, the config key, the
/// trust step.
///
/// Deliberately gone (2026-10-25): `inline_block` / `inline_when` / `never`.
/// The inline block served "an agent that can attach an MCP server but cannot
/// fetch a URL" — a class that by definition never opens this document, and
/// whose body was a digest of the sections above it. `never` stated a rule about
/// FmS's own behaviour to a reader who cannot act on it. Do not restore either
/// to `/agent.md`; the pointer sentence above is the whole hand-off.
///
/// The last entry is the stdio door: a client that cannot take a URL spawns this
/// executable with `--mcp-bridge`, which proxies to the same endpoint (see
/// `bridge.rs`).
fn connect(endpoints: &Value) -> Value {
    let mcp = endpoints.get("mcp").and_then(|v| v.as_str()).unwrap_or_default();
    let manual = endpoints
        .get("manual_markdown")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    json!({
        "pointer_sentence": format!("Read {} and connect to FmS as it instructs.", manual),
        "pointer_when": "Any agent that can fetch a URL. Preferred: one line, and the manual then teaches everything, including what this build cannot do.",
        "snippets": [
            {
                "product": "Claude Code",
                "copy_only": true,
                "text": format!("claude mcp add --transport http fms {}", mcp),
            },
            {
                "product": "Codex",
                "copy_only": true,
                // `codex mcp add --url` writes only `url = "…"` into config.toml.
                // The TOML block this used to emit also carried
                // `transport = "streamable-http"`, which Codex accepts but does
                // not need: a `url` implies streamable HTTP (verified against
                // codex-cli 0.164). The CLI form is also one line to paste.
                "text": format!("codex mcp add fms --url {}", mcp),
            },
            {
                "product": "OpenCode",
                "copy_only": true,
                "text": format!(
                    "{{\n  \"mcp\": {{\n    \"fms\": {{\n      \"type\": \"remote\",\n      \"url\": \"{}\",\n      \"enabled\": true\n    }}\n  }}\n}}",
                    mcp
                ),
            },
            workbuddy_snippet(mcp),
            stdio_snippet(),
        ],
    })
}

/// Tencent's WorkBuddy: `~/.workbuddy/mcp.json`, entries under `mcpServers`.
///
/// A plain `url` entry is first-class there. Its loader classifies each entry by
/// shape — `command` → stdio, `url` → HTTP — and folds `streamable-http`,
/// `streamablehttp`, `streamable_http` and `remote` onto `http` (read out of the
/// installed CLI by a WorkBuddy agent, after this line first shipped as a stdio
/// bridge on the theory that remote support was uncertain). So: no
/// `--mcp-bridge` child process proxying stdio to the endpoint WorkBuddy can call
/// itself, and no executable path baked into the user's config to break the next
/// time the app is rebuilt, moved, or installed elsewhere. `type` is spelled out
/// because it costs one field and is the value the loader normalises to.
fn workbuddy_snippet(mcp: &str) -> Value {
    json!({
        "product": "WorkBuddy",
        "copy_only": true,
        "text": format!(
            "{{\n  \"mcpServers\": {{\n    \"fms\": {{\n      \"type\": \"http\",\n      \"url\": \"{}\"\n    }}\n  }}\n}}",
            mcp
        ),
        // The step everyone misses: writing the file only registers the
        // connector, and a changed entry can keep its stale registration — so
        // trusting an earlier shape of this line is not enough either.
        "note": "Merge this into ~/.workbuddy/mcp.json — keep the other entries in mcpServers — or paste it through 连接器 → 自定义连接器 → 配置 MCP. Then click 信任 in 连接器管理: a connector stays inactive until you trust it, and one trusted in an earlier shape needs re-trusting, or a restart, before the loader picks up the change.",
    })
}

/// The stdio door, generated from the running executable: a client that speaks
/// stdio MCP only spawns FmS with `--mcp-bridge` and gets the same tools, each
/// message proxied to the live HTTP endpoint (`bridge.rs`).
///
/// TOML single-quoted literals hold the path verbatim, so a Windows path needs
/// no backslash escaping.
fn stdio_snippet() -> Value {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "<path to the FmS executable>".to_string());
    json!({
        "product": "Any stdio-only client",
        "copy_only": true,
        "text": format!(
            "[mcp_servers.fms]\ncommand = '{}'\nargs = [\"--mcp-bridge\"]",
            exe
        ),
    })
}

pub(crate) fn errors_convention() -> Value {
    json!({
        "shape": "Errors are returned as tool-level errors (isError), not JSON-RPC faults, so they arrive as text you can act on.",
        "contract": "Every error names the valid set and the next call. app_navigate is the model: `unknown tab 'x'. Valid tabs: dictation, read-book, ...`.",
        "therefore": "Read the message instead of retrying with a variation — it already tells you what to pass.",
    })
}

/// Build the whole manual as one JSON value. `zone` is echoed back to the caller
/// so it can see what it was treated as.
pub(crate) fn render_json(app: &AppHandle, zone: &str) -> Value {
    let endpoints = endpoints(app);
    let tools = tool_index();
    let mut trust = trust(app);
    if let Some(obj) = trust.as_object_mut() {
        obj.insert("zone".to_string(), json!(zone));
    }

    let templates = workflow_templates();
    let step_ops = workflow_actions(app);

    let count = tools.len();
    json!({
        "identity": identity(),
        "endpoints": endpoints,
        "trust": trust,
        "capabilities": capabilities(),
        "tools": {
            "count": count,
            "schema_note": "Names, tiers and one-line summaries only. Call tools/list for the full input schemas — they are not duplicated here.",
            "first_calls": FIRST_CALLS.iter().map(|(tool, why)| json!({ "tool": tool, "why": why })).collect::<Vec<_>>(),
            "domains": grouped_domains(&tools),
            "index": tools,
        },
        "paths": paths(&endpoints),
        "resources": resources_index(),
        "prompts": prompts_index(),
        "house_rules": house_rules(),
        "errors": errors_convention(),
        "workflow_templates": templates,
        "workflow_actions": step_ops,
        "connect": connect(&endpoints),
    })
}

/// The built-in workflow templates, minus their YAML. The full definitions are
/// kilobytes each and `workflow_builtin_templates` already serves them; the
/// manual only needs to say which pipelines exist.
fn workflow_templates() -> Value {
    match crate::workflow::builtin_templates() {
        Ok(value) => {
            let trimmed: Vec<Value> = value
                .get("templates")
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .map(|t| {
                            json!({
                                "category": t.get("category"),
                                "id": t.get("id"),
                                "name": t.get("name"),
                                "version": t.get("version"),
                                "step_count": t.get("step_count"),
                                "run_id": t.get("run_id"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "count": trimmed.len(),
                "templates": trimmed,
                "yaml_note": "Call workflow_builtin_templates for the parsed YAML of each; it is too large to inline here.",
            })
        }
        Err(e) => json!({ "count": 0, "templates": [], "error": e }),
    }
}

/// The readable MCP resources, straight off the server's own list so the manual
/// cannot advertise a uri the server would reject.
fn resources_index() -> Value {
    let list = super::resources::list().resources;
    json!({
        "count": list.len(),
        "note": "Read these with resources/read rather than rediscovering the same facts through tools. `fms://datasets/{uuid}/layout` is a uri template — substitute a uuid from dataset_list.",
        "list": serde_json::to_value(list).unwrap_or_else(|_| json!([])),
    })
}

/// The MCP prompts — the goose recipes, compiled in. Names only: `prompts/get`
/// renders one, and `prompts/list` carries the descriptions.
fn prompts_index() -> Value {
    let names = super::prompts::names();
    json!({
        "count": names.len(),
        "names": names,
        "note": "These are the same recipes the app ships for goose, served in MCP's prompt shape: get_prompt with one of these names returns the full instructions as a user message. They are the idioms for building a dataset, correcting timestamps and normalizing read-aloud texts.",
    })
}

/// The step vocabulary the running app published (from `lib/workflow/step-ops.ts`
/// via `agent_report_workflow_actions`), trimmed to what is worth inlining: which
/// step ids exist and the command each runs. Form fields stay behind
/// `workflow_list_actions` — they are not what a reader of the manual needs to
/// know exists, and Rust keeps no copy to summarize them from.
fn workflow_actions(app: &AppHandle) -> Value {
    let snapshot = app
        .try_state::<crate::ui_state::UiStateHandle>()
        .map(|s| s.workflow_actions())
        .unwrap_or_else(|| json!({}));
    let actions = snapshot
        .get("actions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if actions.is_empty() {
        return json!({
            "count": 0,
            "steps": [],
            "note": "Empty because the app's Workflow page has not published its step registry this session. Call app_navigate with tab=\"workflow\" once, then workflow_list_actions; until then copy step ids from workflow_builtin_templates.",
        });
    }

    let steps: Vec<Value> = actions
        .iter()
        .map(|a| {
            json!({
                "id": a.get("id"),
                "run_command": a.get("run_command"),
            })
        })
        .collect();

    json!({
        "count": steps.len(),
        "steps": steps,
        "commands": snapshot.get("commands").cloned().unwrap_or(Value::Null),
        "published_at": snapshot.get("updated_at").cloned().unwrap_or(Value::Null),
        "detail_note": "workflow_list_actions returns these same rows with each step's form fields (name, type, closed option set) and its Clear command.",
    })
}

// ---------------------------------------------------------------------------
// The markdown rendering of the same body
// ---------------------------------------------------------------------------

pub(crate) fn render_markdown(app: &AppHandle, zone: &str) -> String {
    render_markdown_from(&render_json(app, zone))
}

fn at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for key in path {
        cur = match cur.get(*key) {
            Some(next) => next,
            None => return "",
        };
    }
    cur.as_str().unwrap_or_default()
}

/// Render the markdown *from the JSON body*. A separate hand-written template
/// would be the fourth copy integration.md §5.0 forbids.
pub(crate) fn render_markdown_from(body: &Value) -> String {
    let mut out = String::new();
    let version = at(body, &["identity", "package_version"]);
    let describe = at(body, &["identity", "git_describe"]);
    let protocol = body
        .pointer("/identity/protocol_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    out.push_str("# FmS — agent manual\n\n");
    out.push_str(
        "_Generated from the live tool registry and the bound port by \
         `src-tauri/src/mcp/manual.rs`. The machine-readable twin is \
         `/agent.json` (also `app_bootstrap`); both come from one body, so they \
         cannot disagree._\n\n",
    );

    // 1. identity
    out.push_str("## What you are talking to\n\n");
    out.push_str(&format!(
        "**FmS** — a language-learning app (dictation, reading, flashcards, \
         workflow pipelines) that exposes itself over MCP.\n\n\
         | | |\n|---|---|\n\
         | build | `{}` |
| git describe | `{}` |
| sync protocol | `{}` |
| tools | {} |

",
        version,
        if describe.is_empty() { "unknown (shallow/tag-less build)" } else { describe },
        protocol,
        body.pointer("/tools/count").and_then(|v| v.as_u64()).unwrap_or(0),
    ));

    // 2. endpoints
    out.push_str("## Endpoints\n\n");
    let running = body.pointer("/endpoints/running").and_then(|v| v.as_bool()).unwrap_or(false);
    out.push_str(&format!(
        "Web service **{}**.\n\n| | |\n|---|---|\n| MCP | `{}` |\n| transport | `{}` |\n| this manual | `{}` |\n| JSON twin | `{}` |\n| status | `{}` |\n\n> {}\n\n",
        if running { "running" } else { "NOT running" },
        at(body, &["endpoints", "mcp"]),
        at(body, &["endpoints", "transport"]),
        at(body, &["endpoints", "manual_markdown"]),
        at(body, &["endpoints", "manual_json"]),
        at(body, &["endpoints", "status"]),
        at(body, &["endpoints", "note"]),
    ));

    // 3. trust + policy
    out.push_str("## Your trust zone\n\n");
    out.push_str(&format!(
        "You are in the **{}** zone; credentials required: **{}**.\n\n{}\n\n",
        at(body, &["trust", "zone"]),
        at(body, &["trust", "credentials"]),
        at(body, &["trust", "note"]),
    ));
    out.push_str(&format!(
        "Policy mode: **{}** — {}. Every call is classified into one of {} and \
         journalled to `mcp_audit` (`app_audit_get`). {}\n\n",
        at(body, &["trust", "policy", "mode"]),
        at(body, &["trust", "policy", "modes", "permissive"]),
        body.pointer("/trust/policy/tiers")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_default(),
        at(body, &["trust", "policy", "escalation"]),
    ));

    // 4. capabilities
    out.push_str("## Capabilities of this build\n\n");
    if let Some(obj) = body.get("capabilities").and_then(|v| v.as_object()) {
        out.push_str("| family | present |\n|---|---|\n");
        for (key, value) in obj {
            match value {
                Value::Bool(b) => out.push_str(&format!("| `{}` | {} |\n", key, if *b { "yes" } else { "no" })),
                Value::Array(a) => {
                    let detail = if a.is_empty() {
                        String::from("not implemented")
                    } else {
                        format!("{} entries", a.len())
                    };
                    out.push_str(&format!("| `{}` | {} |\n", key, detail));
                }
                _ => {}
            }
        }
        out.push_str("\nA \"tool not found\" is usually a missing feature here, not a typo.\n\n");
    }

    // 5. first calls
    out.push_str("## Start here\n\n");
    if let Some(calls) = body.pointer("/tools/first_calls").and_then(|v| v.as_array()) {
        for (i, call) in calls.iter().enumerate() {
            out.push_str(&format!(
                "{}. `{}` — {}\n",
                i + 1,
                call.get("tool").and_then(|v| v.as_str()).unwrap_or_default(),
                call.get("why").and_then(|v| v.as_str()).unwrap_or_default(),
            ));
        }
        out.push_str("\n");
    }

    // 6. the way in
    out.push_str("## Connecting\n\n");
    for key in ["attach"] {
        let path = match body.pointer("/paths").and_then(|p| p.get(key)) {
            Some(p) => p,
            None => continue,
        };
        let available = path.get("available").and_then(|v| v.as_bool()).unwrap_or(false);
        out.push_str(&format!(
            "### {} — {}\n\n{}\n\n*Trade-off:* {}\n\n",
            path.get("name").and_then(|v| v.as_str()).unwrap_or_default(),
            if available { "available" } else { "not available yet" },
            path.get("how").and_then(|v| v.as_str()).unwrap_or_default(),
            path.get("trade_off").and_then(|v| v.as_str()).unwrap_or_default(),
        ));
    }

    // 7. house rules
    out.push_str("## House rules\n\n");
    if let Some(rules) = body.get("house_rules").and_then(|v| v.as_array()) {
        for rule in rules {
            if let Some(text) = rule.as_str() {
                out.push_str(&format!("- {}\n", text));
            }
        }
        out.push('\n');
    }

    // 8. errors
    out.push_str("## Errors\n\n");
    out.push_str(&format!(
        "{}\n\n{}\n\n**{}**\n\n",
        at(body, &["errors", "shape"]),
        at(body, &["errors", "contract"]),
        at(body, &["errors", "therefore"]),
    ));

    // 9. tool index
    out.push_str("## Tool index\n\n");
    out.push_str(&format!("{}\n\n", at(body, &["tools", "schema_note"])));
    if let Some(domains) = body.pointer("/tools/domains").and_then(|v| v.as_array()) {
        for domain in domains {
            let name = domain.get("domain").and_then(|v| v.as_str()).unwrap_or_default();
            let count = domain.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push_str(&format!("### `{}` ({} tools)\n\n", name, count));
            if let Some(names) = domain.get("tools").and_then(|v| v.as_array()) {
                let index = body.pointer("/tools/index").and_then(|v| v.as_array());
                for tool in names.iter().filter_map(|n| n.as_str()) {
                    let entry = index.and_then(|list| {
                        list.iter()
                            .find(|e| e.get("name").and_then(|v| v.as_str()) == Some(tool))
                    });
                    let tier = entry
                        .and_then(|e| e.get("tier"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("write");
                    let summary = entry
                        .and_then(|e| e.get("summary"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    out.push_str(&format!("- `{}` _[{}]_ — {}\n", tool, tier, summary));
                }
            }
            out.push('\n');
        }
    }

    // 10. workflow templates
    if let Some(templates) = body.get("workflow_templates") {
        if let Some(list) = templates.get("templates").and_then(|v| v.as_array()) {
            out.push_str("## Workflow templates\n\n");
            out.push_str(
                "Authorable pipeline definitions; `workflow_builtin_templates` returns the same \
                 list with the parsed YAML. Create a run with `workflow_create`.\n\n",
            );
            for template in list {
                let id = template.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let category = template
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let title = template
                    .get("title")
                    .and_then(|v| v.as_str())
                    .or_else(|| template.get("name").and_then(|v| v.as_str()))
                    .unwrap_or_default();
                out.push_str(&format!("- `{}` ({}) — {}\n", id, category, title));
            }
            out.push('\n');
        }
    }

    // 10b. the step vocabulary an authored definition may name
    if let Some(vocab) = body.get("workflow_actions") {
        let steps = vocab
            .get("steps")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        out.push_str("## Workflow step vocabulary\n\n");
        out.push_str(
            "What a `workflow.yaml` step may name — the registry the running app's Workflow \
             page publishes on mount. Read it before authoring: `workflow_validate` rejects \
             unknown ids. `workflow_list_actions` returns these rows with each step's form \
             fields and Clear command.\n\n",
        );
        if steps.is_empty() {
            let note = at(body, &["workflow_actions", "note"]);
            out.push_str(&format!(
                "> {}\n\n",
                if note.is_empty() {
                    "The app has not published its step registry this session."
                } else {
                    note
                }
            ));
        } else {
            out.push_str("| step | runs |\n|---|---|\n");
            for step in &steps {
                let id = step.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let command = step
                    .get("run_command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("—");
                out.push_str(&format!("| `{}` | `{}` |\n", id, command));
            }
            out.push('\n');
        }
    }

    // 10c. readable resources
    if let Some(list) = body.pointer("/resources/list").and_then(|v| v.as_array()) {
        out.push_str("## Resources\n\n");
        out.push_str(
            "Read these with `resources/read` instead of rediscovering the same facts through \
             tools. `fms://docs/start` is this document; `fms://datasets/{uuid}/layout` is a \
             template — substitute a uuid from `dataset_list`.\n\n",
        );
        for resource in list {
            let uri = resource.get("uri").and_then(|v| v.as_str()).unwrap_or_default();
            let title = resource
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| resource.get("name").and_then(|v| v.as_str()))
                .unwrap_or_default();
            out.push_str(&format!("- `{}` — {}\n", uri, title));
        }
        out.push('\n');
    }

    // 10d. prompts (the goose recipes)
    if let Some(names) = body.pointer("/prompts/names").and_then(|v| v.as_array()) {
        if !names.is_empty() {
            out.push_str("## Prompts\n\n");
            out.push_str(
                "The app's own recipes, served in MCP's prompt shape. Call `get_prompt` with one \
                 of these names to receive the full instructions as a user message; \
                 `prompts/list` has the descriptions. They are the idioms for the pipeline \
                 work — build a dataset, correct timestamps, normalize read-aloud texts.\n\n",
            );
            for name in names {
                if let Some(name) = name.as_str() {
                    out.push_str(&format!("- `{}`\n", name));
                }
            }
            out.push('\n');
        }
    }

    // 11. per-client registration lines. Not addressed to the reader — it is
    // already attached — but to the human adding a second client, for whom this
    // document doubles as the install page.
    if let Some(snippets) = body.pointer("/connect/snippets").and_then(|v| v.as_array()) {
        if !snippets.is_empty() {
            out.push_str("## Registering FmS in another client\n\n");
            out.push_str(
                "You are attached already, so none of this is needed to use FmS. It is here for a \
                 *human* adding a second client: each product spells the same registration \
                 differently, and one of them needs a further step no config file can express. The \
                 lines are copy-only — FmS does not probe for installed products and never edits \
                 another product's config file, because merging someone else's schema risks \
                 corrupting it.\n\n",
            );
            for snippet in snippets {
                out.push_str(&format!(
                    "**{}**\n\n```\n{}\n```\n\n",
                    snippet.get("product").and_then(|v| v.as_str()).unwrap_or_default(),
                    snippet.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
                ));
                if let Some(note) = snippet.get("note").and_then(|v| v.as_str()) {
                    out.push_str(&format!("{}\n\n", note));
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// `initialize` instructions (integration.md §5.0 items 3–7, condensed)
// ---------------------------------------------------------------------------

/// The prose every conforming MCP client receives in `InitializeResult.instructions`
/// without asking. Deliberately short — it rides on every connection — and
/// deliberately generated: a hard-coded port here is exactly the drift §5.0
/// forbids.
pub(crate) fn instructions(app: &AppHandle) -> String {
    let endpoints = endpoints(app);
    let mcp = endpoints.get("mcp").and_then(|v| v.as_str()).unwrap_or_default();
    let count = tool_index().len();
    let trust = trust(app);
    let mode = at(&trust, &["policy", "mode"]);

    format!(
        "FmS is a language-learning app; you are attached to it as an MCP server at {mcp} \
         ({count} tools). You are in the loopback trust zone, so no credentials are needed.\n\
         \n\
         Start with app_get_ui_state — it tells you the tab, dataset, media item and cue the \
         user is looking at right now, which is what most \"do something with this\" requests \
         mean. Then auth_status, settings_get, dataset_list, and dataset_full_report on the \
         dataset in question.\n\
         \n\
         Rules that matter here: datasets are uuid-keyed, so take uuids from dataset_list and \
         never derive one from a name or path. Prefer the trash-backed deletes (dataset_delete, \
         book_delete, card_dataset_delete, wiki_dataset_delete, read_aloud_delete) — they return \
         `trashed_to` and the user can undo them; the tools that lose data permanently have no \
         trash behind them. Inside a workflow run, read a step's subject with workflow_verify \
         and commit it with workflow_adopt; dataset_audit is a subject-level health check, not a \
         per-step read. The app_* tools drive the user's window and are fire-and-forget, so \
         confirm the effect with app_get_ui_state instead of assuming it landed.\n\
         \n\
         Every error names the valid set and the next call — read it rather than retrying with a \
         variation. Policy mode is \"{mode}\": every call is classified and journalled \
         (app_audit_get), and in \"enforce\" mode the destructive and human-handoff tiers are \
         refused. Full manual: {mcp}/../agent.md, or app_bootstrap for the same body as JSON.",
        mcp = mcp,
        count = count,
        mode = mode,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The WorkBuddy payload is JSON pasted *into* a JSON file, and it has to
    /// point at the live endpoint rather than a bridge process: parse it, and
    /// check there is no `command` — one would make WorkBuddy spawn it.
    #[test]
    fn workbuddy_snippet_is_a_plain_url_entry() {
        let snippet = workbuddy_snippet("http://127.0.0.1:4321/mcp");
        let text = snippet.get("text").and_then(|v| v.as_str()).expect("text");
        let parsed: Value = serde_json::from_str(text).expect("the snippet must parse as JSON");
        let entry = &parsed["mcpServers"]["fms"];
        assert_eq!(entry["url"], "http://127.0.0.1:4321/mcp");
        assert_eq!(entry["type"], "http");
        assert!(entry.get("command").is_none());
        // The trust step is the one a config file cannot express.
        assert!(snippet.get("note").is_some());
    }

    /// A tool that lands in `misc` means the prefix table drifted from
    /// `mcp/mod.rs`. Silent bucketing would hide a whole domain from the manual.
    #[test]
    fn every_tool_has_a_domain() {
        let router = DatasetMcpServer::tool_router();
        let mut unmapped: Vec<String> = Vec::new();
        for name in router.map.keys() {
            if domain_for(name) == UNMAPPED {
                unmapped.push(name.to_string());
            }
        }
        assert!(
            unmapped.is_empty(),
            "no domain prefix matches: {:?}. Add it to DOMAIN_PREFIXES in mcp/manual.rs.",
            unmapped
        );
    }

    #[test]
    fn first_calls_are_real_tools() {
        let router = DatasetMcpServer::tool_router();
        for (tool, _) in FIRST_CALLS {
            assert!(
                router.map.contains_key(tool),
                "FIRST_CALLS recommends `{}`, which is not a tool",
                tool
            );
        }
    }

    #[test]
    fn longest_prefix_wins() {
        assert_eq!(domain_for("read_aloud_model_status"), "read-aloud");
        assert_eq!(domain_for("model_status"), "models");
        assert_eq!(domain_for("card_dataset_remove_subscriber"), "cards");
        assert_eq!(domain_for("web_service_get_status"), "sync");
        assert_eq!(domain_for("totally_unknown"), UNMAPPED);
    }

    /// The markdown must be a rendering of the body, not a second source of
    /// truth: every load-bearing string in it has to appear in the JSON.
    #[test]
    fn markdown_agrees_with_json() {
        let body = json!({
            "identity": { "package_version": "9.9.9", "git_describe": "v9.9.9", "protocol_version": 6 },
            "endpoints": {
                "running": true, "port": 4321, "transport": "streamable-http",
                "mcp": "http://127.0.0.1:4321/mcp",
                "manual_markdown": "http://127.0.0.1:4321/agent.md",
                "manual_json": "http://127.0.0.1:4321/agent.json",
                "status": "http://127.0.0.1:4321/api/v1/status",
                "note": "live port note",
            },
            "trust": { "zone": "loopback", "credentials": "none", "note": "zone note",
                "policy": { "mode": "enforce", "modes": { "permissive": "p" },
                    "tiers": ["read", "write"], "escalation": "esc" } },
            "capabilities": { "local_stt": true },
            "tools": { "count": 1, "schema_note": "schemas elsewhere",
                "first_calls": [{ "tool": "app_get_ui_state", "why": "because" }],
                "domains": [{ "domain": "system", "count": 1, "tools": ["app_get_ui_state"] }],
                "index": [{ "name": "app_get_ui_state", "domain": "system", "tier": "read", "summary": "S." }] },
            "paths": {},
            "house_rules": ["rule one"],
            "errors": { "shape": "shape", "contract": "contract", "therefore": "therefore" },
            "connect": { "pointer_sentence": "point", "pointer_when": "pw",
                "snippets": [{ "product": "Codex", "text": "codex mcp add" }] },
        });
        let md = render_markdown_from(&body);
        for needle in [
            "9.9.9",
            "http://127.0.0.1:4321/mcp",
            "http://127.0.0.1:4321/agent.md",
            "live port note",
            "loopback",
            "**enforce**",
            "app_get_ui_state",
            "rule one",
            "schemas elsewhere",
            "_[read]_",
            "Codex",
            "codex mcp add",
        ] {
            assert!(md.contains(needle), "markdown is missing {:?}", needle);
        }
    }
}
