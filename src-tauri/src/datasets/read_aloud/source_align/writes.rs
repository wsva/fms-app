use serde_json::{json, Value};

use crate::settings::SettingsState;

use super::super::find_dataset_dir_write;
use super::super::standard_audio::{self, text_reference};
use super::index::load_index;
use super::proposals::{counts, load_proposals};

// ============================================================
// Apply
// ============================================================

/// Store the spans whose text has no clip yet as their standard audio.
///
/// Nothing is decoded here: a span is three numbers and a recording id, the row
/// is written, and the clip is cut the first time it is played, scored or packed
/// (`standard_audio::materialize`). Applying a book is therefore a few hundred
/// row writes instead of an hour of decoding — and a text nobody practises costs
/// no disk at all. `limit` bounds the call (`generate_all`'s pattern): an agent
/// repeats until `remaining` is 0 rather than holding a request open for an hour.
///
/// A text that **already has** standard audio is skipped, never overwritten. A
/// second row would leave two clips claiming one text (the first one still
/// holds the seconds its wav was cut from), so changing a span means deleting
/// that clip first and saving again. `text_uuid` narrows the call to one text —
/// that is a row's Save button; omitting it is Save all.
pub(crate) fn apply(
    settings: &SettingsState,
    dataset_uuid: &str,
    limit: Option<i64>,
    text_uuid: Option<&str>,
) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let index = load_index(&dir);
    let set = load_proposals(&dir);
    let limit = limit.unwrap_or(10).clamp(1, 200) as usize;
    let only = text_uuid.map(str::trim).filter(|s| !s.is_empty());

    // Which texts already own a clip decides what may be stored at all, so it is
    // read once for the whole set rather than asked row by row.
    let clips = standard_audio::rows_by_text(&dir)?;

    let mut planned: Vec<usize> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for (i, proposal) in set.proposals.iter().enumerate() {
        if !proposal.is_placed() {
            continue;
        }
        if only.is_some_and(|uuid| proposal.text_uuid != uuid) {
            continue;
        }
        if let Some(existing) = clips.get(&proposal.text_uuid).and_then(|rows| rows.first()) {
            skipped.push(json!({
                "text_uuid": proposal.text_uuid,
                "uuid": existing.uuid,
                "kind": existing.kind(),
                "reason": "That text already has standard audio — delete the clip to save this span instead.",
            }));
            continue;
        }
        planned.push(i);
        if planned.len() == limit {
            break;
        }
    }

    let mut applied = 0usize;
    let mut failed: Vec<Value> = Vec::new();
    for i in planned {
        let proposal = set.proposals[i].clone();
        let Some(source_uuid) = proposal.source_uuid.clone() else {
            failed.push(json!({
                "text_uuid": proposal.text_uuid,
                "error": "The proposal names no recording; re-run the guess.",
            }));
            continue;
        };
        if !index.sources.iter().any(|s| s.uuid == source_uuid) {
            failed.push(json!({
                "text_uuid": proposal.text_uuid,
                "error": format!("Recording {} is no longer registered; re-register the folder.", source_uuid),
            }));
            continue;
        }
        let reference = match text_reference(&dir, &proposal.text_uuid) {
            Ok(reference) => reference,
            Err(error) => {
                failed.push(json!({ "text_uuid": proposal.text_uuid, "error": error }));
                continue;
            }
        };
        match standard_audio::store_span(
            settings,
            dataset_uuid,
            &dir,
            &proposal.text_uuid,
            &reference,
            &source_uuid,
            &proposal.source_name,
            proposal.start,
            proposal.end,
        ) {
            Ok(_) => {
                applied += 1;
            }
            Err(error) => failed.push(json!({
                "text_uuid": proposal.text_uuid,
                "error": error,
            })),
        }
    }

    // "Still to save" excludes what a clip blocks: a caller looping until 0
    // would otherwise spin on rows this call can never store. The map is read
    // again because the rows written above are clips too, and the stale one
    // would count them as still pending.
    let owned = standard_audio::rows_by_text(&dir)?;
    let remaining = set
        .proposals
        .iter()
        .filter(|p| p.is_placed() && !owned.contains_key(&p.text_uuid))
        .count();
    let failed_count = failed.len();
    let skipped_count = skipped.len();
    Ok(json!({
        "status": "ok",
        "applied": applied,
        "failed": failed,
        "failed_count": failed_count,
        "skipped": skipped,
        "skipped_count": skipped_count,
        "remaining": remaining,
        "counts": counts(&set, &owned),
    }))
}
