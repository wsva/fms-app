//! Wiki datasets: syncable markdown-tree datasets (list, read/write, delete,
//! search, index) plus read-only browse of the paired hub's datasets.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::Manager;

use crate::settings::SettingsState;

use super::DatasetMcpServer;

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetNameParam {
    name: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiDatasetImportParam {
    name: String,
    path: String,
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

#[tool_router(router = wiki_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    // ---------------------------------------------------------------------
    // Wiki datasets (syncable dataset-type wikis). Paths are dataset-relative
    // ('' = root, 'notes/idea.md'); writes journal changes for sync.
    // ---------------------------------------------------------------------

    #[tool(name = "wiki_dataset_list", description = "List all wiki datasets (syncable markdown-tree datasets). Returns entries with uuid, name, updated, file_count, path, and location ('local' when stored under the datasets dir, 'linked' when an external folder was converted in place).")]
    async fn wiki_dataset_list(&self) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_list");
        let state = self.app.state::<SettingsState>();
        let datasets = crate::datasets::wiki::wiki_dataset_list(state.into()).await?;
        Ok(serde_json::to_string_pretty(&datasets).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_create", description = "Create a new empty wiki dataset with the given name under the datasets directory. Returns the new dataset summary including its uuid.")]
    async fn wiki_dataset_create(&self, Parameters(param): Parameters<WikiDatasetNameParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_create: name={}", param.name);
        let state = self.app.state::<SettingsState>();
        let summary = crate::datasets::wiki::wiki_dataset_create(state.into(), param.name).await?;
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_import_dir", description = "Convert an existing local markdown folder into a wiki dataset in place: writes an info.json into it and links the folder — no files are moved or copied.")]
    async fn wiki_dataset_import_dir(&self, Parameters(param): Parameters<WikiDatasetImportParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_import_dir: name={}, path={}", param.name, param.path);
        let state = self.app.state::<SettingsState>();
        let summary = crate::datasets::wiki::wiki_dataset_import_dir(state.into(), param.name, param.path).await?;
        Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_delete", description = "Delete a wiki dataset. The dataset folder is moved to trash (not permanently deleted) for safety.")]
    async fn wiki_dataset_delete(&self, Parameters(param): Parameters<WikiDatasetUuidParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_delete: uuid={}", param.uuid);
        let state = self.app.state::<SettingsState>();
        crate::datasets::wiki::wiki_dataset_delete(state.into(), param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset moved to trash"}).to_string())
    }

    #[tool(name = "wiki_dataset_list_dir", description = "List one directory inside a wiki dataset. 'rel' is dataset-relative ('' or omitted = root). Returns entries with name, rel_path, is_dir, modified.")]
    async fn wiki_dataset_list_dir(&self, Parameters(param): Parameters<WikiDatasetRelParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_list_dir: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        let entries = crate::datasets::wiki::wiki_dataset_list_dir(state.into(), param.uuid, param.rel).await?;
        Ok(serde_json::to_string_pretty(&entries).unwrap_or_default())
    }

    #[tool(name = "wiki_dataset_read_file", description = "Read a markdown file from a wiki dataset. 'rel' is the dataset-relative file path (e.g. 'notes/idea.md'). Returns the full file content.")]
    async fn wiki_dataset_read_file(&self, Parameters(param): Parameters<WikiDatasetRelParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_dataset_read_file: uuid={}, rel={}", param.uuid, param.rel);
        let state = self.app.state::<SettingsState>();
        let content = crate::datasets::wiki::wiki_dataset_read_file(state.into(), param.uuid, param.rel).await?;
        Ok(serde_json::json!({"status": "ok", "content": content}).to_string())
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
