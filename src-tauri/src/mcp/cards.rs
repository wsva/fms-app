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
    /// Family key to join (the `<type>_<key>` id grammar; see the
    /// `dataset_family` tool) — the key of the book or dictation set this deck
    /// was built from. Empty starts a standalone deck.
    #[serde(default)]
    family: String,
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

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardMediaListParam {
    dataset_uuid: String,
    card_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardMediaAddParam {
    dataset_uuid: String,
    /// Card the attachment is added to. Must be an existing card (open the
    /// editor with a minted uuid before attaching, so the media dir can be
    /// named after it).
    card_uuid: String,
    /// Absolute path to a file on this machine. A card accepts only the
    /// portable set: images `png jpg jpeg gif webp`, audio `mp3 m4a wav ogg`,
    /// video `mp4 webm` — an Android WebView must be able to play what a PC
    /// author attached.
    source_path: String,
    /// Which field the `{{handle}}` anchor will live in: `question`, `answer`,
    /// `note`, or `suggestion`.
    field: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardMediaUpdateParam {
    dataset_uuid: String,
    /// Full `CardMedia` JSON of the row to update; only `title`, `sort`,
    /// `autoplay`, `play_on`, `field`, and `handle` are read back. A `handle`
    /// change rewrites `{{old}}` → `{{new}}` in the owning field of the card
    /// in the same transaction.
    media: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct CardMediaDeleteParam {
    dataset_uuid: String,
    media_uuid: String,
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

    #[tool(name = "card_dataset_create", description = "Create a new card dataset with a name and optional description. Returns the dataset summary with UUID. Use card_save to add cards afterwards. Pass `family` (see dataset_family) to make this deck the card member of a book's or dictation set's family.")]
    async fn card_dataset_create(&self, Parameters(param): Parameters<CardDatasetCreateParam>) -> Result<String, String> {
        log::info!("[MCP] card_dataset_create: name={}", param.name);
        let settings = self.app.state::<SettingsState>();
        let desc = if param.description.is_empty() { None } else { Some(param.description) };
        let loc = if param.location.is_empty() { None } else { Some(param.location) };
        let family = if param.family.is_empty() { None } else { Some(param.family) };
        let summary = datasets::cards::card_dataset_create(settings, param.name, desc, loc, family).await?;
        Ok(serde_json::json!({"status": "ok", "dataset": {
            "uuid": summary.info.uuid,
            "name": summary.info.name,
            "card_count": summary.card_count,
            "path": summary.path,
        }}).to_string())
    }

    #[tool(name = "card_dataset_delete", description = "Delete a card dataset by moving its whole directory (database, info.json) into the app trash folder — not permanently removed, so this is recoverable. Returns `trashed_to`. Its cards leave the location's search index.")]
    async fn card_dataset_delete(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::warn!("[MCP] card_dataset_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let trashed_to = datasets::cards::card_dataset_delete(settings, param.uuid).await?;
        Ok(serde_json::json!({
            "status": "ok",
            "message": "Dataset moved to trash",
            "trashed_to": trashed_to
        })
        .to_string())
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

    #[tool(name = "card_test_next", description = "Get the next card due for review using SM-2 spaced repetition. Returns the card and its review state, or null if no cards are due. Reviews are per user: this is the *current* workspace's schedule, not the dataset's stored one.")]
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

    #[tool(name = "card_test_stats", description = "Count what the review queue holds for a dataset *for the current user*: due (overdue, served first), fresh (never reviewed by this user, served only once due is empty), mature (familiarity 6, out of rotation), incomplete (missing question or answer, skipped), total, and serving = which pool the next draw comes from.")]
    async fn card_test_stats(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_test_stats: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let stats = datasets::cards::card_test_stats(settings, param.uuid).await?;
        Ok(serde_json::to_string_pretty(&stats).unwrap_or_default())
    }

    #[tool(name = "card_test_next_all", description = "Get the next card due for review across EVERY card dataset (a cross-dataset session, no single dataset open). Same due-first priority as card_test_next but global: an overdue card in any dataset outranks a fresh one elsewhere. Returns the card, its review state, and the dataset_uuid + dataset_name it came from (submit the rating with card_test_submit using that dataset_uuid), or null if nothing is due anywhere.")]
    async fn card_test_next_all(&self) -> Result<String, String> {
        log::info!("[MCP] card_test_next_all");
        let settings = self.app.state::<SettingsState>();
        match datasets::cards::card_test_get_all(settings).await? {
            Some(x) => Ok(serde_json::json!({
                "dataset_uuid": x.dataset_uuid,
                "dataset_name": x.dataset_name,
                "card": {
                    "uuid": x.card.uuid,
                    "question": x.card.question,
                    "answer": x.card.answer,
                    "note": x.card.note,
                    "familiarity": x.card.familiarity,
                },
                "review": x.review.map(|r| serde_json::json!({
                    "familiarity": r.familiarity,
                    "interval_days": r.interval_days,
                    "ease_factor": r.ease_factor,
                    "repetitions": r.repetitions,
                    "next_review_at": r.next_review_at,
                }))
            }).to_string()),
            None => Ok(serde_json::json!({"card": null, "message": "No cards due for review in any dataset"}).to_string()),
        }
    }

    #[tool(name = "card_test_stats_all", description = "Count the review queue across EVERY card dataset *for the current user*, summed: due, fresh, mature, incomplete, total, and serving = which pool the next cross-dataset draw comes from. The aggregate counterpart of card_test_stats.")]
    async fn card_test_stats_all(&self) -> Result<String, String> {
        log::info!("[MCP] card_test_stats_all");
        let settings = self.app.state::<SettingsState>();
        let stats = datasets::cards::card_test_stats_all(settings).await?;
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

    #[tool(name = "card_sync", description = "Sync one card dataset through the dataset-scope row journal: pushes this dataset's pending edits and applies the hub's pulled rows (card rows are journal kinds since sync-v2 phase 7b — no bespoke card endpoint). Only for datasets upstreamed by the cloud hub and owned by the current user; never a read-only shared copy under the download root. A dataset that is not journal-synced here returns zeros.")]
    async fn card_sync(&self, Parameters(param): Parameters<UuidParam>) -> Result<String, String> {
        log::info!("[MCP] card_sync: dataset={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::cards::sync::card_sync_full(self.app.clone(), settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "result": {
            "pulled": result.pulled,
            "pushed": result.pushed,
            "conflicts": result.conflicts,
            "server_time": result.server_time,
        }}).to_string())
    }

    #[tool(name = "card_sync_all", description = "Sync every owned, cloud-hub-upstreamed card dataset through the dataset-scope journal. Returns per-dataset pulled/pushed/conflict counts.")]
    async fn card_sync_all(&self) -> Result<String, String> {
        log::info!("[MCP] card_sync_all");
        let settings = self.app.state::<SettingsState>();
        let results = datasets::cards::sync::card_sync_all(self.app.clone(), settings).await?;
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

    // ---------------------------------------------------------------------
    // Card media (attachments: image/audio/video under `media/<card-uuid>/`)
    // ---------------------------------------------------------------------
    //
    // Bytes live as files under the dataset dir; the row is a thin index over
    // that directory (see docs/design/card-media.md). A tool that returns
    // `present: false` means the row exists but the bytes are on another
    // device — call `card_media_register` after the sync planes land the file.

    #[tool(name = "card_media_list", description = "List every live attachment of one card: uuid, handle (the `{{anchor}}` slug), rel_path, kind (image|audio|video), title, sort, autoplay, play_on, size, plus `present` (does the file exist on this device?). Ordered by (field, sort, created_at). Never lists a file the table does not know — that is card_media_register's job.")]
    async fn card_media_list(&self, Parameters(param): Parameters<CardMediaListParam>) -> Result<String, String> {
        log::info!("[MCP] card_media_list: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        let rows = datasets::cards::media::card_media_list(
            settings,
            param.dataset_uuid,
            param.card_uuid,
        )
        .await?;
        let items: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
            "uuid": r.uuid,
            "card_uuid": r.card_uuid,
            "field": r.field,
            "handle": r.handle,
            "rel_path": r.rel_path,
            "kind": r.kind,
            "title": r.title,
            "sort": r.sort,
            "autoplay": r.autoplay,
            "play_on": r.play_on,
            "duration_ms": r.duration_ms,
            "width": r.width,
            "height": r.height,
            "size": r.size,
            "present": r.present,
            "path": r.path,
            "created_at": r.created_at,
            "updated_at": r.updated_at,
        })).collect();
        Ok(serde_json::json!({ "media": items, "count": items.len() }).to_string())
    }

    #[tool(name = "card_media_add", description = "Copy `source_path` into `<dataset>/media/<card_uuid>/` and attach it to the card's field. Returns the row including `handle`, so the caller can insert `{{handle}}` at the caret in the field text. Refuses an extension outside the card-portable set (image: png/jpg/jpeg/gif/webp, audio: mp3/m4a/wav/ogg, video: mp4/webm) so Android WebView can always play what was authored on a PC. A collision on the file name adds `-2`, `-3`, …")]
    async fn card_media_add(&self, Parameters(param): Parameters<CardMediaAddParam>) -> Result<String, String> {
        log::info!(
            "[MCP] card_media_add: dataset={}, card={}, field={}, source={}",
            param.dataset_uuid, param.card_uuid, param.field, param.source_path
        );
        let settings = self.app.state::<SettingsState>();
        let row = datasets::cards::media::card_media_add(
            settings,
            param.dataset_uuid,
            param.card_uuid,
            param.source_path,
            param.field,
        )
        .await?;
        Ok(serde_json::json!({
            "status": "ok",
            "media": {
                "uuid": row.uuid,
                "card_uuid": row.card_uuid,
                "field": row.field,
                "handle": row.handle,
                "rel_path": row.rel_path,
                "kind": row.kind,
                "title": row.title,
                "sort": row.sort,
                "autoplay": row.autoplay,
                "play_on": row.play_on,
                "duration_ms": row.duration_ms,
                "width": row.width,
                "height": row.height,
                "size": row.size,
                "present": row.present,
                "path": row.path,
            },
            "anchor_token": format!("{{{{{}}}}}", row.handle),
        }).to_string())
    }

    #[tool(name = "card_media_update", description = "Update one attachment's mutable fields: title, sort, autoplay, play_on (tap|reveal), and handle. A handle change rewrites `{{old}}` → `{{new}}` in the owning field of the card, so the token and the row cannot disagree after a partial write. Identity fields (uuid, card_uuid, rel_path, kind) are read from the stored row and cannot be moved here — use card_media_delete + card_media_add to switch.")]
    async fn card_media_update(&self, Parameters(param): Parameters<CardMediaUpdateParam>) -> Result<String, String> {
        log::info!("[MCP] card_media_update: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let media: datasets::cards::media::CardMedia = serde_json::from_value(param.media.into())
            .map_err(|e| format!("Invalid media JSON: {}", e))?;
        let row = datasets::cards::media::card_media_update(settings, param.dataset_uuid, media).await?;
        Ok(serde_json::json!({
            "status": "ok",
            "media": {
                "uuid": row.uuid,
                "handle": row.handle,
                "field": row.field,
                "title": row.title,
                "sort": row.sort,
                "autoplay": row.autoplay,
                "play_on": row.play_on,
                "rel_path": row.rel_path,
                "kind": row.kind,
                "present": row.present,
                "updated_at": row.updated_at,
            },
        }).to_string())
    }

    #[tool(name = "card_media_delete", description = "Soft-delete an attachment row. The bytes stay put so an undo of a card delete still finds its media — `trashed_to` is null on this response, and bytes are removed only by an explicit sweep or with the dataset itself.")]
    async fn card_media_delete(&self, Parameters(param): Parameters<CardMediaDeleteParam>) -> Result<String, String> {
        log::info!("[MCP] card_media_delete: dataset={}, media={}", param.dataset_uuid, param.media_uuid);
        let settings = self.app.state::<SettingsState>();
        let out = datasets::cards::media::card_media_delete(settings, param.dataset_uuid, param.media_uuid).await?;
        Ok(out.to_string())
    }

    #[tool(name = "card_media_register", description = "Reconcile `<dataset>/media/<card_uuid>/` with the `card_media` table for one card. Reports `imported` (a file with no row became an unanchored attachment), `missing` (a row whose bytes are gone on this device — leave alone until the sync planes deliver them), and `orphaned` (a file the card cannot hold — an unsupported extension, kept on disk, not deleted). Nothing is deleted by this call, matching the read-aloud register() rule.")]
    async fn card_media_register(&self, Parameters(param): Parameters<CardMediaListParam>) -> Result<String, String> {
        log::info!("[MCP] card_media_register: dataset={}, card={}", param.dataset_uuid, param.card_uuid);
        let settings = self.app.state::<SettingsState>();
        let out = datasets::cards::media::card_media_register(settings, param.dataset_uuid, param.card_uuid).await?;
        Ok(out.to_string())
    }
}
