//! Wiki datasets: syncable markdown-tree datasets (list, read/write, delete,
//! search, index), the translate road (ask an agent, close the ask), plus
//! read-only browse of the paired hub's datasets.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::service::RoleServer;
use rmcp::{tool, tool_router, Peer};
use serde::Deserialize;
use serde_json::json;
use tauri::Manager;

use crate::settings::SettingsState;

use super::mail::client_label;
use super::DatasetMcpServer;

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetCreateParam {
    name: String,
    /// Family key to join (the `<type>_<key>` id grammar; see the
    /// `dataset_family` tool) — the book these notes are about. Empty starts a
    /// standalone wiki.
    #[serde(default)]
    family: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetImportParam {
    name: String,
    path: String,
    /// Family key to join, as `wiki_dataset_create`'s.
    #[serde(default)]
    family: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetUuidParam {
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetRelParam {
    uuid: String,
    #[serde(default)]
    rel: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetWriteParam {
    uuid: String,
    rel: String,
    content: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetSearchParam {
    uuid: String,
    keyword: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiTranslateRequestParam {
    uuid: String,
    /// The document to translate, dataset-relative (`notes/idea.md`).
    source_rel: String,
    /// A language tag, lowercase — `de`, or a subtagged `zh-cn`. It becomes part
    /// of the file name, so it is validated rather than trusted.
    lang_code: String,
    /// Display form the learner reads, in the prompt's header line.
    lang_name: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiTranslateDoneParam {
    /// The wiki translate request this work answers.
    mail_uuid: String,
    /// Your claim_id, if you claimed the row. Offer it and the row closes as a
    /// claim-grounded delivery; omit it (or arrive after your lease lapsed) and
    /// it closes as a report.
    #[serde(default)]
    claim_id: Option<String>,
}

#[tool_router(router = wiki_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    // ---------------------------------------------------------------------
    // Wiki datasets (syncable dataset-type wikis). Paths are dataset-relative
    // ('' = root, 'notes/idea.md'); writes journal changes for sync.
    // ---------------------------------------------------------------------

    #[tool(name = "wiki_dataset_list", description = "List all wiki datasets (syncable markdown-tree datasets). Returns entries with uuid, name, description, updated, file_count, path, and location ('local' when stored under the datasets dir, 'linked' when an external folder was converted in place). Rename or re-describe one with dataset_info_update — it takes any dataset type.")]
    async fn wiki_dataset_list(&self) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_list");
        let state = self.app.state::<SettingsState>();
        let datasets = crate::datasets::wiki::wiki_dataset_list(state.into()).await?;
        Ok(serde_json::to_string_pretty(&datasets).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_create", description = "Create a new empty wiki dataset with the given name under the datasets directory. Pass `family` (see dataset_family) to attach it to the book or dictation set it documents. Returns the new dataset summary including its uuid.")]
    async fn wiki_dataset_create(&self, Parameters(param): Parameters<WikiDatasetCreateParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_create: name={}", param.name);
        let state = self.app.state::<SettingsState>();
        let family = if param.family.is_empty() { None } else { Some(param.family) };
        let summary = crate::datasets::wiki::wiki_dataset_create(state.into(), param.name, family).await?;
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_import_dir", description = "Convert an existing local markdown folder into a wiki dataset in place: writes an info.json into it and links the folder — no files are moved or copied. Pass `family` (see dataset_family) to attach it to the book or dictation set it documents.")]
    async fn wiki_dataset_import_dir(&self, Parameters(param): Parameters<WikiDatasetImportParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_import_dir: name={}, path={}", param.name, param.path);
        let state = self.app.state::<SettingsState>();
        let family = if param.family.is_empty() { None } else { Some(param.family) };
        let summary = crate::datasets::wiki::wiki_dataset_import_dir(state.into(), param.name, param.path, family).await?;
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_delete", description = "Delete a wiki dataset by moving its whole directory into the app trash folder — not permanently removed, so this is recoverable. Returns `trashed_to`. Also drops the directory's `meta.json` link when it lives in a linked location. Use `wiki_dataset_list` first to get the UUID.")]
    async fn wiki_dataset_delete(&self, Parameters(param): Parameters<WikiDatasetUuidParam>) -> Result<String, String> {
        log::warn!("[MCP] wiki_dataset_delete: uuid={}", param.uuid);
        let state = self.app.state::<SettingsState>();
        let trashed_to = crate::datasets::wiki::wiki_dataset_delete(state.into(), param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset moved to trash", "trashed_to": trashed_to}).to_string())
    }

    #[tool(name = "wiki_dataset_list_dir", description = "List one directory inside a wiki dataset. 'rel' is dataset-relative ('' or omitted = root). Returns entries with name, rel_path, is_dir, modified.")]
    async fn wiki_dataset_list_dir(&self, Parameters(param): Parameters<WikiDatasetRelParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_list_dir: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        let entries = crate::datasets::wiki::wiki_dataset_list_dir(state.into(), param.uuid, param.rel).await?;
        Ok(serde_json::to_string_pretty(&entries).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_read_file", description = "Read a wiki file, auto-classified for preview. 'kind' is 'markdown', 'text', 'image', 'media' or 'binary' (extension first, then a UTF-8 sniff). 'content' carries decoded text (max 2 MB; 'truncated' flags the cut) only for markdown/text; image/media/binary return content=null — use the returned absolute 'path' to open them externally. 'rel' is the dataset-relative file path (e.g. 'notes/idea.md').")]
    async fn wiki_dataset_read_file(&self, Parameters(param): Parameters<WikiDatasetRelParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_read_file: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        let file = crate::datasets::wiki::wiki_dataset_read_file(state.into(), param.uuid, param.rel).await?;
        Ok(serde_json::json!({
            "status": "ok",
            "kind": file.kind,
            "content": file.content,
            "size": file.size,
            "truncated": file.truncated,
            "path": file.path,
        })
        .to_string())
    }

    #[tool(name = "wiki_dataset_write_file", description = "Write or overwrite a markdown file in a wiki dataset, creating parent directories as needed. The change is journaled for sync (last-write-wins per path). Fails with an actionable error if the dataset is not downloaded on this device.")]
    async fn wiki_dataset_write_file(&self, Parameters(param): Parameters<WikiDatasetWriteParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_write_file: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        crate::datasets::wiki::wiki_dataset_write_file(state.into(), param.uuid, param.rel, param.content).await?;
        Ok(serde_json::json!({"status": "ok", "message": "File written successfully"}).to_string())
    }

    #[tool(name = "wiki_dataset_create_file", description = "Create a new empty markdown file in a wiki dataset (parent directories created automatically). Journals wiki_file_save so the empty file syncs to the hub.")]
    async fn wiki_dataset_create_file(&self, Parameters(param): Parameters<WikiDatasetRelParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_create_file: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        crate::datasets::wiki::wiki_dataset_create_file(state.into(), param.uuid, param.rel).await?;
        Ok(serde_json::json!({"status": "ok", "message": "File created"}).to_string())
    }

    #[tool(name = "wiki_dataset_create_dir", description = "Create a directory inside a local wiki dataset. Empty directories are not journaled (git-style) — they reach other devices implicitly once a file inside them is written.")]
    async fn wiki_dataset_create_dir(&self, Parameters(param): Parameters<WikiDatasetRelParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_create_dir: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        crate::datasets::wiki::wiki_dataset_create_dir(state.into(), param.uuid, param.rel).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Directory created"}).to_string())
    }

    #[tool(name = "wiki_dataset_delete_file", description = "Delete a markdown file from a wiki dataset. The file is moved to trash (not permanently deleted) and the deletion is journaled for sync.")]
    async fn wiki_dataset_delete_file(&self, Parameters(param): Parameters<WikiDatasetRelParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_delete_file: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        crate::datasets::wiki::wiki_dataset_delete_file(state.into(), param.uuid, param.rel).await?;
        Ok(serde_json::json!({"status": "ok", "message": "File moved to trash"}).to_string())
    }

    #[tool(name = "wiki_dataset_index", description = "(Re)build the FTS5 search index for one wiki dataset's markdown files. Returns the number of files indexed. Search auto-indexes on demand, so this is only needed to force a refresh.")]
    async fn wiki_dataset_index(&self, Parameters(param): Parameters<WikiDatasetUuidParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_index: uuid={}", param.uuid);
        let state = self.app.state::<SettingsState>();
        let count = crate::datasets::wiki::wiki_dataset_index(state.into(), param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "indexed_count": count}).to_string())
    }

    #[tool(name = "wiki_dataset_search", description = "Full-text search inside one wiki dataset (ranked snippets with dataset-relative paths). Indexes on demand if the dataset has no index yet.")]
    async fn wiki_dataset_search(&self, Parameters(param): Parameters<WikiDatasetSearchParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_search: uuid={}, keyword={}", param.uuid, param.keyword);
        let state = self.app.state::<SettingsState>();
        let results = crate::datasets::wiki::wiki_dataset_search(state.into(), param.uuid, param.keyword).await?;
        Ok(serde_json::to_string_pretty(&results).unwrap_or_default())
    }

    // ---------------------------------------------------------------------
    // The translate road: the app asks, an agent works, the file is the answer.
    // ---------------------------------------------------------------------

    #[tool(name = "wiki_translate_request", description = "File a request to translate one wiki markdown file into another language, and get back the canonical prompt for it. The app does not translate: an agent (often the one reading this) reads the source, writes a bilingual `<stem>_<lang_code>.md` beside it, and closes the item with `wiki_translate_done`. Returns {mail, prompt, replaced} — `replaced` names the earlier ask this one superseded, and a row another agent is holding is refused with `claim_required`. Pass a bare `lang_code` like `de`; `lang_name` is the form the learner reads.")]
    async fn wiki_translate_request(&self, Parameters(param): Parameters<WikiTranslateRequestParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_translate_request: uuid={}, rel={}, lang={}", param.uuid, param.source_rel, param.lang_code);
        let state = self.app.state::<SettingsState>();
        let ask = crate::datasets::wiki_translate::request_translation(
            state.inner(),
            &param.uuid,
            &param.source_rel,
            &param.lang_code,
            &param.lang_name,
        )?;
        Ok(serde_json::to_string_pretty(&ask).unwrap_or_default())
    }

    #[tool(name = "wiki_translate_done", description = "Close a wiki translate request once the translated file is written. Claiming was optional on this road, so this accepts a `claim_id` and closes through the claim (a delivery like any other) or, without one — or after the lease lapsed mid-work — closes the row as a **report**: it lands `delivered` with `reason: reported_done`, which means the worker vouches for it and the app did not read it. Verify your own write first (`wiki_dataset_list_dir`, then read the file back): nothing here will. Refuses `wrong_subject` for a row that is not a wiki ask — a book artifact delivers with `book_save_artifact`, never here.")]
    async fn wiki_translate_done(&self, peer: Peer<RoleServer>, Parameters(param): Parameters<WikiTranslateDoneParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_translate_done: mail_uuid={}", param.mail_uuid);
        let state = self.app.state::<SettingsState>();
        let who = client_label(&peer);
        match crate::datasets::wiki_translate::close_translation(
            state.inner(),
            &param.mail_uuid,
            param.claim_id.as_deref(),
            &who,
        ) {
            Ok(closed) => Ok(serde_json::to_string_pretty(&json!({
                "ok": true,
                "outcome": closed.outcome,
                "mail": closed.mail,
            }))
            .unwrap_or_default()),
            // A ledger refusal carries a code to branch on and a sentence to act
            // on, so it travels in the mail domain's shape rather than this
            // domain's plain strings.
            Err(e) => Err(serde_json::to_string(&json!({ "ok": false, "error": e })).unwrap_or(e)),
        }
    }

    // ---------------------------------------------------------------------
    // Hub wiki browse (read-only) — relay to the paired hub PC's wiki datasets.
    // ---------------------------------------------------------------------

    #[tool(name = "wiki_hub_list", description = "List wiki datasets available on the paired hub PC (read-only remote browse; no local storage). Returns {datasets: [...]}.")]
    async fn wiki_hub_list(&self) -> Result<String, String> {
        log::info!("[MCP] wiki_hub_list");
        let state = self.app.state::<SettingsState>();
        let v = crate::sync::client::wiki_hub_list(state.into()).await?;
        Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
    }

    #[tool(name = "wiki_hub_search", description = "Search one wiki dataset on the paired hub PC (read-only, ranked server-side; 'rel' in results is dataset-relative).")]
    async fn wiki_hub_search(&self, Parameters(param): Parameters<WikiDatasetSearchParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_hub_search: uuid={}, keyword={}", param.uuid, param.keyword);
        let state = self.app.state::<SettingsState>();
        let results = crate::sync::client::wiki_hub_search(state.into(), param.uuid, param.keyword).await?;
        Ok(serde_json::to_string_pretty(&results).unwrap_or_default())
    }
}
