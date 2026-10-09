//! Card datasets: CRUD, tags, FTS5 search, SM-2 review and online sync.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::Manager;

use crate::datasets;
use crate::settings::SettingsState;

use super::{DatasetMcpServer, JsonValue, UuidParam};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardDatasetCreateParam {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    location: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardDatasetSubscriberParam {
    uuid: String,
    email: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardListParam {
    dataset_uuid: String,
    #[serde(default)]
    keyword: String,
    #[serde(default)]
    filter: String,
    #[serde(default)]
    tag_uuid: String,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardGetParam {
    dataset_uuid: String,
    card_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardSaveParam {
    dataset_uuid: String,
    card: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardForkParam {
    source_dataset_uuid: String,
    card_uuid: String,
    target_dataset_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardTestSubmitParam {
    dataset_uuid: String,
    card_uuid: String,
    quality: i32,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardTagSaveParam {
    dataset_uuid: String,
    tag: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardSetTagsParam {
    dataset_uuid: String,
    card_uuid: String,
    tag_uuids: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardSearchParam {
    query: String,
    #[serde(default)]
    mode: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardFtsRebuildParam {
    #[serde(default)]
    location: String,
}

#[tool_router(router = cards_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "card_dataset_list", description = "List all card datasets from all locations (default + linked directories). Returns UUID, name, description, sharing (visibility, owner_id), card count, and location for each dataset. Metadata edits go through dataset_info_update, which works on every type.")]
    async fn card_dataset_list(&self) -> Result<String, String> {
        log::info!("[MCP] card_dataset_list");
        let settings = self.app.state::<SettingsState>();
        let datasets = datasets::cards::card_dataset_list(settings).await?;
        let items: Vec<serde_json::Value> = datasets.iter().map(|d| {
            serde_json::json!({
                "uuid": d.info.uuid,
                "name": d.info.name,
                "description": d.info.description,
                "language": d.info.language,
                "visibility": d.info.sharing.visibility,
                "owner_id": d.info.sharing.owner_id,
                "card_count": d.card_count,
                "path": d.path,
                "location": d.location,
            })
        }).collect();
        Ok(serde_json::json!({"datasets": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_dataset_create", description = "Create a new card dataset with a name and optional description. Returns the dataset summary with UUID. Use card_save to add cards afterwards.")]
    async fn card_dataset_create(&self, Parameters(param): Parameters<CardDatasetCreateParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_create: name={}", param.name);
        let settings = self.app.state::<SettingsState>();
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        let loc = if param.location.is_empty() { None } else { Some(param.location) };
        let summary = datasets::cards::card_dataset_create(settings, param.name, desc, loc).await?;
        Ok(serde_json::json!({"status": "ok", "dataset": {
            "uuid": summary.info.uuid,
            "name": summary.info.name,
            "card_count": summary.card_count,
            "path": summary.path,
        }}).to_string())
    }

    #[tool(name = "card_dataset_delete", description = "Delete a card dataset and all its files (database, info.json). This is irreversible.")]
    async fn card_dataset_delete(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] card_dataset_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::cards::card_dataset_delete(settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset deleted"}).to_string())
    }

    #[tool(name = "card_dataset_add_subscriber", description = "Add a subscriber (by email) to a card dataset. Only the dataset owner can do this.")]
    async fn card_dataset_add_subscriber(&self, Parameters(param): Parameters<CardDatasetSubscriberParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_add_subscriber: uuid={}, email={}", param.uuid, param.email);
        let settings = self.app.state::<SettingsState>();
        datasets::cards::card_dataset_add_subscriber(settings, param.uuid, param.email).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Subscriber added"}).to_string())
    }

    #[tool(name = "card_dataset_remove_subscriber", description = "Remove a subscriber (by email) from a card dataset.")]
    async fn card_dataset_remove_subscriber(&self, Parameters(param): Parameters<CardDatasetSubscriberParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_remove_subscriber: uuid={}, email={}", param.uuid, param.email);
        let settings = self.app.state::<SettingsState>();
        datasets::cards::card_dataset_remove_subscriber(settings, param.uuid, param.email).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Subscriber removed"}).to_string())
    }

    #[tool(name = "card_list", description = "List cards in a dataset with optional keyword search, filter ('all', 'normal', 'easy', 'incomplete'), tag filter, and pagination. Returns cards and total count.")]
    async fn card_list(&self, Parameters(param): Parameters<CardListParam>) -> Result<String, String> {
        log::info!("[MCP] card_list: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let filter = if param.filter.is_empty() { None } else { Some(serde_json::from_value(serde_json::json!(param.filter)).unwrap_or(datasets::cards::CardFilter::All)) };
        let keyword = if param.keyword.is_empty() { None } else { Some(param.keyword) };
        let tag_uuid = if param.tag_uuid.is_empty() { None } else { Some(param.tag_uuid) };
        let (card_list, total) = datasets::cards::card_list(settings, param.dataset_uuid, filter, tag_uuid, keyword, param.limit, param.offset).await?;
        let items: Vec<serde_json::Value> = card_list.iter().map(|c| {
            serde_json::json!({
                "uuid": c.uuid,
                "question": c.question,
                "answer": c.answer,
                "note": c.note,
                "familiarity": c.familiarity,
                "created_at": c.created_at,
                "updated_at": c.updated_at,
            })
        }).collect();
        Ok(serde_json::json!({"cards": items, "total": total}).to_string())
    }

    #[tool(name = "card_get", description = "Get full details of a single card by dataset UUID and card UUID.")]
    async fn card_get(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_get: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        let card = datasets::cards::card_get(settings, param.dataset_uuid, param.card_uuid).await?;
        Ok(serde_json::to_string_pretty(&card).unwrap_or_default())
    }

    #[tool(name = "card_save", description = "Create or update a card in a dataset. Pass card as JSON with fields: uuid (empty for new), question, suggestion, answer, note. Returns the saved card.")]
    async fn card_save(&self, Parameters(param): Parameters<CardSaveParam>) -> Result<String, String> {
        log::info!("[MCP] card_save: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let card: datasets::cards::Card = serde_json::from_value(param.card.into())
            .map_err(|e| format!("Invalid card JSON: {}", e))?;
        let result = datasets::cards::card_save(settings, param.dataset_uuid, card).await?;
        Ok(serde_json::json!({"status": "ok", "card": {
            "uuid": result.uuid,
            "question": result.question,
            "answer": result.answer,
            "familiarity": result.familiarity,
        }}).to_string())
    }

    #[tool(name = "card_delete", description = "Soft-delete a card from a dataset. The card is marked as deleted but not permanently removed (for sync purposes).")]
    async fn card_delete(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_delete: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::cards::card_delete(settings, param.dataset_uuid, param.card_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Card deleted"}).to_string())
    }

    #[tool(name = "card_fork", description = "Fork a card from one dataset to another. Creates a copy with lineage tracking (source_card_uuid, source_dataset_uuid). Use for customizing shared cards.")]
    async fn card_fork(&self, Parameters(param): Parameters<CardForkParam>) -> Result<String, String> {
        log::info!("[MCP] card_fork: from={}, card={}, to={}", param.source_dataset_uuid, param.card_uuid, param.target_dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::cards::card_fork(settings, param.source_dataset_uuid, param.card_uuid, param.target_dataset_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "forked_card": {
            "uuid": result.uuid,
            "question": result.question,
            "answer": result.answer,
            "source_card_uuid": result.source_card_uuid,
            "source_dataset_uuid": result.source_dataset_uuid,
        }}).to_string())
    }

    #[tool(name = "card_search", description = "Search cards across all datasets using FTS5 full-text search. Returns matching cards with dataset info. Supports FTS5 query syntax (e.g., 'word1 word2' for AND, 'word1 OR word2' for OR). Mode: 'question' searches only questions, 'fulltext' (default) searches all fields.")]
    async fn card_search(&self, Parameters(param): Parameters<CardSearchParam>) -> Result<String, String> {
        log::info!("[MCP] card_search: query='{}', mode='{}'", param.query, param.mode);
        let settings = self.app.state::<SettingsState>();
        let mode = if param.mode.is_empty() { None } else { Some(param.mode) };
        let results = datasets::cards::card_search(settings, param.query, mode).await?;
        let items: Vec<serde_json::Value> = results.iter().map(|r| {
            serde_json::json!({
                "dataset_uuid": r.dataset_uuid,
                "dataset_name": r.dataset_name,
                "card_uuid": r.card_uuid,
                "question": r.question,
                "answer": r.answer,
                "note": r.note,
                "location": r.location,
            })
        }).collect();
        Ok(serde_json::json!({"results": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_fts_rebuild", description = "Rebuild the FTS5 full-text search index for card datasets. Optionally specify a location path to rebuild only that location's index. Returns the number of cards indexed.")]
    async fn card_fts_rebuild(&self, Parameters(param): Parameters<CardFtsRebuildParam>) -> Result<String, String> {
        log::info!("[MCP] card_fts_rebuild: location='{}'", param.location);
        let settings = self.app.state::<SettingsState>();
        let location = if param.location.is_empty() { None } else { Some(param.location) };
        let count = datasets::cards::card_fts_rebuild(settings, location).await?;
        Ok(serde_json::json!({"status": "ok", "cards_indexed": count}).to_string())
    }

    #[tool(name = "card_test_next", description = "Get the next card due for review using SM-2 spaced repetition. Returns the card and its review state, or null if no cards are due.")]
    async fn card_test_next(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_test_next: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        match datasets::cards::card_test_get(settings, param.uuid).await? {
            Some((card, review)) => Ok(serde_json::json!({
                "card": {
                    "uuid": card.uuid,
                    "question": card.question,
                    "answer": card.answer,
                    "note": card.note,
                    "familiarity": card.familiarity,
                },
                "review": review.map(|r| serde_json::json!({
                    "familiarity": r.familiarity,
                    "interval_days": r.interval_days,
                    "ease_factor": r.ease_factor,
                    "repetitions": r.repetitions,
                    "next_review_at": r.next_review_at,
                }))
            }).to_string()),
            None => Ok(serde_json::json!({"card": null, "message": "No cards due for review"}).to_string()),
        }
    }

    #[tool(name = "card_test_submit", description = "Submit a review result for a card. Quality rating: 0-5 (0=blackout, 2=hard, 3=okay, 4=good, 5=perfect). Returns updated review state with next review date.")]
    async fn card_test_submit(&self, Parameters(param): Parameters<CardTestSubmitParam>) -> Result<String, String> {
        log::info!("[MCP] card_test_submit: dataset={}, card={}, quality={}", param.dataset_uuid, param.card_uuid, param.quality);
        let settings = self.app.state::<SettingsState>();
        let review = datasets::cards::card_test_submit(settings, param.dataset_uuid, param.card_uuid, param.quality).await?;
        Ok(serde_json::json!({"status": "ok", "review": {
            "familiarity": review.familiarity,
            "interval_days": review.interval_days,
            "ease_factor": review.ease_factor,
            "repetitions": review.repetitions,
            "next_review_at": review.next_review_at,
        }}).to_string())
    }

    #[tool(name = "card_test_stats", description = "Count what the review queue holds for a dataset: due (overdue, served first), fresh (never reviewed, served only once due is empty), mature (familiarity 6, out of rotation), incomplete (missing question or answer, skipped), total, and serving = which pool the next draw comes from.")]
    async fn card_test_stats(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_test_stats: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let stats = datasets::cards::card_test_stats(settings, param.uuid).await?;
        Ok(serde_json::to_string_pretty(&stats).unwrap_or_default())
    }

    #[tool(name = "card_tag_list", description = "List all tags in a card dataset. Returns UUID, name, and color for each tag.")]
    async fn card_tag_list(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_tag_list: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let tags = datasets::cards::card_tag_list(settings, param.uuid).await?;
        let items: Vec<serde_json::Value> = tags.iter().map(|t| {
            serde_json::json!({
                "uuid": t.uuid,
                "name": t.name,
                "color": t.color,
            })
        }).collect();
        Ok(serde_json::json!({"tags": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_tag_save", description = "Create or update a tag in a card dataset. Pass tag as JSON with fields: uuid (empty for new), name, color (optional hex string).")]
    async fn card_tag_save(&self, Parameters(param): Parameters<CardTagSaveParam>) -> Result<String, String> {
        log::info!("[MCP] card_tag_save: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let tag: datasets::cards::Tag = serde_json::from_value(param.tag.into())
            .map_err(|e| format!("Invalid tag JSON: {}", e))?;
        let result = datasets::cards::card_tag_save(settings, param.dataset_uuid, tag).await?;
        Ok(serde_json::json!({"status": "ok", "tag": {
            "uuid": result.uuid,
            "name": result.name,
            "color": result.color,
        }}).to_string())
    }

    #[tool(name = "card_tag_delete", description = "Soft-delete a tag from a card dataset.")]
    async fn card_tag_delete(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_tag_delete: dataset={}, tag={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::cards::card_tag_delete(settings, param.dataset_uuid, param.card_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Tag deleted"}).to_string())
    }

    #[tool(name = "card_set_tags", description = "Set the tags for a card. Pass an array of tag UUIDs to assign to the card. Existing tag assignments are replaced.")]
    async fn card_set_tags(&self, Parameters(param): Parameters<CardSetTagsParam>) -> Result<String, String> {
        log::info!("[MCP] card_set_tags: dataset={}, card={}, tags={}", param.dataset_uuid, param.card_uuid, param.tag_uuids.len());
        let settings = self.app.state::<SettingsState>();
        datasets::cards::card_set_tags(settings, param.dataset_uuid, param.card_uuid, param.tag_uuids).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Tags set"}).to_string())
    }

    #[tool(name = "card_get_tags", description = "Get all tags assigned to a card. Returns tag UUIDs, names, and colors.")]
    async fn card_get_tags(&self, Parameters(param): Parameters<CardGetParam>) -> Result<String, String> {
        log::info!("[MCP] card_get_tags: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        let tags = datasets::cards::card_get_tags(settings, param.dataset_uuid, param.card_uuid).await?;
        let items: Vec<serde_json::Value> = tags.iter().map(|t| {
            serde_json::json!({
                "uuid": t.uuid,
                "name": t.name,
                "color": t.color,
            })
        }).collect();
        Ok(serde_json::json!({"tags": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_sync", description = "Perform a full bidirectional sync for a card dataset with the online server. Pulls remote changes and pushes local changes. Only works for datasets owned by the current user.")]
    async fn card_sync(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_sync: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::cards::sync::card_sync_full(settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "result": {
            "pulled": result.pulled,
            "pushed": result.pushed,
            "conflicts": result.conflicts,
            "server_time": result.server_time,
        }}).to_string())
    }

    #[tool(name = "card_sync_all", description = "Sync all owned card datasets with the online server. Returns results for each dataset synced.")]
    async fn card_sync_all(&self) -> Result<String, String> {
        log::info!("[MCP] card_sync_all");
        let settings = self.app.state::<SettingsState>();
        let results = datasets::cards::sync::card_sync_all(settings).await?;
        let items: Vec<serde_json::Value> = results.iter().map(|(uuid, result)| {
            match result {
                Ok(r) => serde_json::json!({
                    "dataset_uuid": uuid,
                    "status": "ok",
                    "pulled": r.pulled,
                    "pushed": r.pushed,
                    "conflicts": r.conflicts,
                }),
                Err(e) => serde_json::json!({
                    "dataset_uuid": uuid,
                    "status": "error",
                    "error": e,
                }),
            }
        }).collect();
        Ok(serde_json::json!({"results": items, "count": items.len()}).to_string())
    }

    #[tool(name = "card_sync_status", description = "Get the sync status for a card dataset: last synced timestamp, clock offset, and pending changes count.")]
    async fn card_sync_status(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_sync_status: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let dataset_uuid = param.uuid.clone();
        match datasets::cards::card_sync_status(settings, param.uuid).await? {
            Some(status) => Ok(serde_json::json!({
                "dataset_uuid": status.dataset_uuid,
                "last_synced_at": status.last_synced_at,
                "clock_offset_ms": status.clock_offset_ms,
                "pending_push": status.pending_push,
            }).to_string()),
            None => Ok(serde_json::json!({"dataset_uuid": dataset_uuid, "last_synced_at": null, "message": "Never synced"}).to_string()),
        }
    }
}
