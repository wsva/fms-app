use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::settings::SettingsState;

use super::super::standard_audio::{self, StandardAudioRow};
use super::super::{find_dataset_dir, find_dataset_dir_write, open_db};
use super::index::{
    align_dir, load_index, preview, read_json, transcript_path, write_json, Preview,
    SourceTranscript,
};
use super::{round_ms, MIN_SPAN_SECS, PEAK_SECONDS};

/// What one text amounts to.
///
/// A file written by the verdict era also carries `state` and `applied_uuid`;
/// serde ignores both, so an old `proposals.json` needs no migration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Proposal {
    pub(super) text_uuid: String,
    #[serde(default)]
    pub(super) source_uuid: Option<String>,
    #[serde(default)]
    pub(super) source_name: String,
    #[serde(default)]
    pub(super) start: f64,
    #[serde(default)]
    pub(super) end: f64,
    #[serde(default)]
    pub(super) confidence: f64,
    #[serde(default)]
    pub(super) matched_words: usize,
    #[serde(default)]
    pub(super) reference_words: usize,
    /// `"proposed"` when there is a span, `"refused"` when there is not.
    pub(super) status: String,
    /// Why it was refused. A placed span carries none: its match is already the
    /// percentage beside the title, and restating that as prose was noise on
    /// every loose match.
    #[serde(default)]
    pub(super) note: String,
}

impl Proposal {
    pub(super) fn refusal(text_uuid: &str, note: String) -> Self {
        Self {
            text_uuid: text_uuid.to_string(),
            source_uuid: None,
            source_name: String::new(),
            start: 0.0,
            end: 0.0,
            confidence: 0.0,
            matched_words: 0,
            reference_words: 0,
            status: "refused".to_string(),
            note,
        }
    }

    /// A placed span: the row a Save would store, unless its text already has a
    /// clip — `apply` checks that, because only it reads the standard audio.
    pub(super) fn is_placed(&self) -> bool {
        self.status == "proposed"
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct ProposalSet {
    #[serde(default)]
    pub(super) generated_at: String,
    #[serde(default)]
    pub(super) proposals: Vec<Proposal>,
}

pub(super) fn proposals_path(dir: &Path) -> PathBuf {
    align_dir(dir).join("proposals.json")
}

pub(super) fn load_proposals(dir: &Path) -> ProposalSet {
    read_json(&proposals_path(dir)).unwrap_or_default()
}

// ============================================================
// Review
// ============================================================

/// One proposal as the panel and an agent see it: the span, the text it is
/// for, and a thumbnail of the audio around it.
#[derive(Serialize)]
pub struct ProposalView {
    pub text_uuid: String,
    pub title: String,
    pub content: String,
    pub status: String,
    pub note: String,
    pub source_uuid: Option<String>,
    pub source_name: String,
    /// Absolute path of the recording, for playback.
    pub source_path: String,
    pub start: f64,
    pub end: f64,
    pub confidence: f64,
    pub matched_words: usize,
    pub reference_words: usize,
    /// The standard audio this text already has, if any — the reason its row is
    /// badged, and the reason `apply` will refuse to give it a second clip.
    pub clip: Option<ClipView>,
    /// `None` when the span cannot be previewed — a refusal, or a recording
    /// whose cache is gone on this device.
    pub preview: Option<Preview>,
}

/// The standard audio a text already owns, as the align page needs to see it:
/// enough to badge the row and to send the user to the right delete button, not
/// the whole row.
#[derive(Serialize)]
pub struct ClipView {
    pub uuid: String,
    /// `tts`, `upload` or `align`.
    pub kind: String,
    /// The clip the scorer reads.
    pub is_default: bool,
    /// The seconds it covers, when it was cut from a recording.
    pub start_secs: Option<f64>,
    pub end_secs: Option<f64>,
    /// Provenance as written: `Align · chapter1.mp3 · 12.30–18.90 s`, `Edge TTS · …`.
    pub note: String,
}

/// The clip a row is about: the one this page cut if there is one, else the one
/// the scorer reads, else whatever is there. `None` — the text has no standard
/// audio at all, and saving one is exactly what its row offers.
fn clip_of(rows: Option<&Vec<StandardAudioRow>>) -> Option<ClipView> {
    let rows = rows?;
    let row = rows
        .iter()
        .find(|row| row.origin == "align")
        .or_else(|| rows.iter().find(|row| row.is_default))
        .or_else(|| rows.first())?;
    Some(ClipView {
        uuid: row.uuid.clone(),
        kind: row.kind().to_string(),
        is_default: row.is_default,
        start_secs: row.start_secs,
        end_secs: row.end_secs,
        note: row.note.clone(),
    })
}

/// The proposals, joined with the texts they are for.
pub(crate) fn list(settings: &SettingsState, dataset_uuid: &str) -> Result<Value, String> {
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let set = load_proposals(&dir);
    let index = load_index(&dir);

    let (titles, texts) = {
        let conn = open_db(&dir)?;
        let mut stmt = conn
            .prepare("SELECT uuid, title, content FROM read_text")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let pairs: Vec<(String, String, String)> = rows.flatten().collect();
        let total = pairs.len();
        (
            pairs
                .into_iter()
                .map(|(uuid, title, content)| (uuid, (title, content)))
                .collect::<HashMap<_, _>>(),
            total,
        )
    };

    // Which texts already have standard audio: one query for the whole dataset,
    // because a row's badge and its Save verb both depend on it.
    let clips = standard_audio::rows_by_text(&dir)?;

    // One envelope decode per recording, not per text: a ten-hour book is a
    // few megabytes of base64 and forty texts would decode it forty times.
    let mut cache: HashMap<String, SourceTranscript> = HashMap::new();
    let mut views: Vec<ProposalView> = Vec::with_capacity(set.proposals.len());
    for proposal in &set.proposals {
        let (title, content) = titles.get(&proposal.text_uuid).cloned().unwrap_or_default();
        let source = index
            .sources
            .iter()
            .find(|s| Some(s.uuid.as_str()) == proposal.source_uuid.as_deref());
        let preview = if proposal.status == "proposed" {
            source.and_then(|source| {
                let transcript = cache.entry(source.uuid.clone()).or_insert_with(|| {
                    read_json(&transcript_path(&dir, &source.uuid)).unwrap_or(SourceTranscript {
                        uuid: source.uuid.clone(),
                        file_name: source.file_name.clone(),
                        duration: source.duration.unwrap_or(0.0),
                        peak_seconds: PEAK_SECONDS,
                        peaks: String::new(),
                        words: Vec::new(),
                    })
                });
                (transcript.duration > 0.0).then(|| preview(transcript, proposal.start, proposal.end))
            })
        } else {
            None
        };
        views.push(ProposalView {
            text_uuid: proposal.text_uuid.clone(),
            title,
            content,
            status: proposal.status.clone(),
            // Files written by an older build still carry the "heard N of M
            // words; check both edges" hint. Dropping it here clears it from
            // the page without waiting for a re-guess to rewrite the file.
            note: if proposal.is_placed() {
                String::new()
            } else {
                proposal.note.clone()
            },
            source_uuid: proposal.source_uuid.clone(),
            source_name: proposal.source_name.clone(),
            source_path: source.map(|s| s.path.clone()).unwrap_or_default(),
            start: proposal.start,
            end: proposal.end,
            confidence: proposal.confidence,
            matched_words: proposal.matched_words,
            reference_words: proposal.reference_words,
            clip: clip_of(clips.get(&proposal.text_uuid)),
            preview,
        });
    }

    Ok(json!({
        "status": "ok",
        "generated_at": set.generated_at,
        "proposals": views,
        "texts_total": texts,
        "counts": counts(&set, &clips),
    }))
}

/// The four numbers the queue's filter chips show, and nothing else.
///
/// `saved` counts every row whose text owns a clip — a refusal can own one too
/// (a text with TTS audio the guess could not place) — so the four overlap the
/// way the filters do; `unsaved` is exactly what a save would still store.
pub(super) fn counts(set: &ProposalSet, clips: &HashMap<String, Vec<StandardAudioRow>>) -> Value {
    let placed = set.proposals.iter().filter(|p| p.is_placed()).count();
    let unsaved = set
        .proposals
        .iter()
        .filter(|p| p.is_placed() && !clips.contains_key(&p.text_uuid))
        .count();
    json!({
        "placed": placed,
        "refused": set.proposals.len() - placed,
        "unsaved": unsaved,
        "saved": set
            .proposals
            .iter()
            .filter(|p| clips.contains_key(&p.text_uuid))
            .count(),
    })
}

/// Move one placed span by hand — the page's 10 ms nudges and its typed edges.
///
/// Moving is a judgement about the recording, not a write to it: `apply` is what
/// stores a clip. A clip already on disk keeps the seconds its wav was cut from,
/// so a text that owns one still has to have it deleted before the moved span can
/// be saved — that rule is the clip's existence, not a flag set here.
pub(crate) fn move_span(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    start: f64,
    end: f64,
) -> Result<Value, String> {
    let uuid = text_uuid.trim();
    if uuid.is_empty() {
        return Err("Moving a span needs a text_uuid.".to_string());
    }
    if !start.is_finite() || !end.is_finite() || start < 0.0 {
        return Err("The span must be two finite times after 0.".to_string());
    }
    let (start, end) = (round_ms(start), round_ms(end));
    if end - start < MIN_SPAN_SECS {
        return Err(format!(
            "The span is {:.3} s long; it must be at least {:.1} s.",
            end - start,
            MIN_SPAN_SECS
        ));
    }

    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let mut set = load_proposals(&dir);
    if set.proposals.is_empty() {
        return Err("There are no proposals yet — run the guess first.".to_string());
    }
    // Read before the row is borrowed: moving a span writes no clip, so this map
    // is still the truth when the answer goes out.
    let clips = standard_audio::rows_by_text(&dir)?;
    let Some(proposal) = set.proposals.iter_mut().find(|p| p.text_uuid == uuid) else {
        return Err(format!("No proposal for text {}", uuid));
    };
    if proposal.status != "proposed" {
        return Err(format!(
            "That text has no span to move: {}",
            if proposal.note.is_empty() {
                "the guess refused it"
            } else {
                proposal.note.as_str()
            }
        ));
    }

    // A nudge that lands on the same millisecond is not a write: the page sends a
    // burst of them while an arrow key is held.
    let moved = (proposal.start - start).abs() > 1e-9 || (proposal.end - end).abs() > 1e-9;
    if moved {
        proposal.start = start;
        proposal.end = end;
        write_json(&proposals_path(&dir), &set)?;
    }

    Ok(json!({
        "status": "ok",
        "changed": if moved { 1 } else { 0 },
        "counts": counts(&set, &clips),
    }))
}

