//! Dataset infrastructure shared by every dataset type, plus the dictation
//! dataset pipeline (import, STT subtitles, waveforms, cue DB generation).
//!
//! This module tree mirrors the on-disk layout: every type resolves through
//! [`dataset_roots`] to `<datasets_dir>/{dictation,card,book,read_aloud,wiki}/`
//! plus the linked directories listed in that folder's `meta.json`.
//!
//! The unified `info.json` descriptor every type shares lives in [`info`]. The
//! shared core is split by concern:
//!
//! - [`roots`] — `DatasetType`, where each type lives, `meta.json` links
//! - [`registry`] — discovery, the summary/detail DTOs, import/update/delete
//! - [`paths`] — uuid to directory (base vs overlay)
//! - [`media`] — media enumeration, the `media/`-mirroring paths, trash, sync
//! - [`subtitle`] — STT pass and VTT shaping
//! - [`waveform`] — symphonia peaks
//! - [`db`] — `data.sqlite3` schema and cue generation
//!
//! Everything those modules define is re-exported here, so the rest of the crate
//! keeps naming `datasets::find_dataset_dir` and friends through this root.

pub(crate) mod asr_metric;
pub(crate) mod book;
pub(crate) mod book_artifacts;
pub(crate) mod cards;
pub(crate) mod db;
pub(crate) mod dictation;
pub(crate) mod info;
pub(crate) mod media;
pub(crate) mod paths;
pub(crate) mod read_aloud;
pub(crate) mod registry;
#[cfg(feature = "desktop")]
pub(crate) mod report;
pub(crate) mod roots;
pub(crate) mod subtitle;
pub(crate) mod textsim;
#[cfg(feature = "desktop")]
pub(crate) mod tools;
pub(crate) mod waveform;
pub(crate) mod wiki;

// The only item left in this file is the desktop Stage-2 orchestrator, so its imports
// are gated with it.
#[cfg(feature = "desktop")]
use tauri::{AppHandle, State};

#[cfg(feature = "desktop")]
use crate::{models::ModelState, settings::SettingsState};

// The shared descriptor and its helpers are addressed through this module root by
// every dataset type and by the sync layer.
pub(crate) use info::{
    assert_uuid_free, canonical_stamp, is_derived_index, now_row_stamp, now_stamp, read_info,
    read_info_opt, row_stamp_at, touch_info, write_info, DatasetInfo,
    FAVORITES_DATASET_UUID, FORMAT_BOOK, FORMAT_CARD, FORMAT_DICTATION, FORMAT_READ_ALOUD,
    FORMAT_WIKI, INFO_FILE, LAYER_ROLE_FLAT, LAYER_ROLE_OVERLAY, LayerBlock, READ_ALOUD_ALIGN_DIR,
};
// The hub's edit-time normalizer is the only caller outside this group; the row
// stamps every type writes come through the ungated names above.
#[cfg(feature = "desktop")]
pub(crate) use info::parse_stamp;
// The cross-type enumerator's only caller is the desktop MCP listing; an ungated
// re-export would warn in the mobile build.
#[cfg(feature = "desktop")]
pub(crate) use info::scan_datasets;

// The stage modules re-export through this root, so the rest of the crate keeps naming
// `datasets::find_dataset_dir` and friends. Commands are not reached this way: lib.rs
// registers each one by the module that now owns it (`datasets::registry::dataset_list`).
pub(crate) use db::*;
pub(crate) use media::*;
pub(crate) use paths::*;
pub(crate) use registry::*;
pub(crate) use roots::*;
pub(crate) use subtitle::*;
// Everything in `waveform` is desktop-only; an ungated glob would warn on Android.
#[cfg(feature = "desktop")]
pub(crate) use waveform::*;

/// Advance a dataset to Stage 2: generate subtitles + waveforms.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn dataset_advance_to_stage2(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
    uuid: String,
) -> Result<(), String> {
    dataset_generate_subtitles(app.clone(), settings.clone(), model_state, uuid.clone()).await?;
    dataset_generate_waveform(app, settings, uuid).await?;
    Ok(())
}
