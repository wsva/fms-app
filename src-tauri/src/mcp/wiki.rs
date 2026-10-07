//! Wiki documents: multi-directory listing, read/write/delete, search and index.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::Manager;

use crate::settings::SettingsState;

use super::DatasetMcpServer;

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiPathParam {
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiWriteFileParam {
    path: String,
    content: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiSearchParam {
    keyword: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct WikiAddDirParam {
    name: String,
    path: String,
}

#[tool_router(router = wiki_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "wiki_list_dirs", description = "List top-level wiki directories (default wiki directory + linked directories). Returns entries with name, path, is_dir, is_linked, and modified timestamp.")]
    async fn wiki_list_dirs(&self) -> Result<String, String> {
        log::info!("[MCP] wiki_list_dirs");
        let state = self.app.state::<SettingsState>();
        let entries = crate::wiki::wiki_list_dirs(state.into()).await?;
        Ok(serde_json::to_string_pretty(&entries).unwrap_or_default())
    }

    #[tool(name = "wiki_list_dir", description = "List contents of a specific wiki directory. Returns entries with name, path, is_dir, is_linked, and modified timestamp.")]
    async fn wiki_list_dir(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_list_dir: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        let entries = crate::wiki::wiki_list_dir(state.into(), param.path).await?;
        Ok(serde_json::to_string_pretty(&entries).unwrap_or_default())
    }

    #[tool(name = "wiki_read_file", description = "Read the content of a wiki markdown file. Returns the full file content as a string.")]
    async fn wiki_read_file(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_read_file: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        let content = crate::wiki::wiki_read_file(state.into(), param.path).await?;
        Ok(serde_json::json!({"status": "ok", "content": content}).to_string())
    }

    #[tool(name = "wiki_write_file", description = "Write or create a wiki markdown file at the specified path. Creates parent directories if they don't exist. Use this to create new wiki pages or overwrite existing ones.")]
    async fn wiki_write_file(&self, Parameters(param): Parameters<WikiWriteFileParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_write_file: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_write_file(state.into(), param.path, param.content).await?;
        Ok(serde_json::json!({"status": "ok", "message": "File written successfully"}).to_string())
    }

    #[tool(name = "wiki_delete_file", description = "Delete a wiki markdown file. The file is moved to trash (not permanently deleted) for safety. Returns the trash location.")]
    async fn wiki_delete_file(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_delete_file: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_delete_file(state.into(), param.path).await?;
        Ok(serde_json::json!({"status": "ok", "message": "File moved to trash"}).to_string())
    }

    #[tool(name = "wiki_search", description = "Search wiki markdown files using full-text search. Returns matching files with snippets, ranked by relevance. Use wiki_index first if search returns no results.")]
    async fn wiki_search(&self, Parameters(param): Parameters<WikiSearchParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_search: keyword={}", param.keyword);
        let state = self.app.state::<SettingsState>();
        let results = crate::wiki::wiki_search(state.into(), param.keyword).await?;
        Ok(serde_json::to_string_pretty(&results).unwrap_or_default())
    }

    #[tool(name = "wiki_index", description = "Index all markdown files in the wiki directory and linked directories for search. Returns the number of files indexed. Run this after adding new files to make them searchable.")]
    async fn wiki_index(&self) -> Result<String, String> {
        log::info!("[MCP] wiki_index");
        let state = self.app.state::<SettingsState>();
        let count = crate::wiki::wiki_index(state.into()).await?;
        Ok(serde_json::json!({"status": "ok", "indexed_count": count}).to_string())
    }

    #[tool(name = "wiki_add_dir", description = "Link an external directory to the wiki. The directory will appear in the wiki sidebar with the specified name. Files in linked directories are included in search and indexing.")]
    async fn wiki_add_dir(&self, Parameters(param): Parameters<WikiAddDirParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_add_dir: name={}, path={}", param.name, param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_add_dir(state.into(), param.name, param.path).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Directory linked successfully"}).to_string())
    }

    #[tool(name = "wiki_remove_dir", description = "Unlink an external directory from the wiki. The directory itself is not deleted, only the link is removed.")]
    async fn wiki_remove_dir(&self, Parameters(param): Parameters<WikiPathParam>) -> Result<String, String> {
        log::info!("[MCP] wiki_remove_dir: path={}", param.path);
        let state = self.app.state::<SettingsState>();
        crate::wiki::wiki_remove_dir(state.into(), param.path).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Directory unlinked successfully"}).to_string())
    }
}
