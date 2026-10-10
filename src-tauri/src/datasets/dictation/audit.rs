//! Read-only dataset audit — the deep version of the Workflow page's "Scan status".
//!
//! `dataset_get` can say "a subtitle directory exists". That is too coarse to act
//! on: a folder where 3 of 40 media files have neither a VTT nor any cue rows, or
//! where somebody edited a VTT after it was imported, looks identical to a
//! finished one. This module walks the media inventory from three sides at once:
//! the files under `media/`, their siblings under `subtitle/` + `waveform/` +
//! `transcript/`, and the rows in `data.sqlite3`. Every disagreement is reported
//! as a named check carrying the offending files and the workflow step that
//! repairs it.
//!
//! It never writes. Fixing is the pipeline's job (`sync_media`,
//! `generate_subtitles`, `generate_waveforms`, `adjust_cue_times`), which keeps a
//! status probe cheap, safe to re-run, and honest about what the run state does
//! and does not prove.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::State;

use crate::datasets::{
    find_dataset_dir, list_media_files, parse_stamp, parse_vtt, read_info, rel_source_string,
    sibling_path, DatasetInfo,
};
use crate::settings::SettingsState;

/// How many offending files one check lists before truncating. The count stays
/// exact, so a 300-file dataset still reports the truth.
const ITEM_CAP: usize = 100;

/// Marker both adjustment modes write into `listen_subtitle.note`: `in_place`
/// appends "(in-place)", `mode=new` writes it on the copy it inserts. It is the
/// only record an adjustment leaves behind.
const ADJUSTED_MARKER: &str = "adjusted using waveform";

// ---------------------------------------------------------------------------
// Report shape
// ---------------------------------------------------------------------------

/// One offending file: what the human should click, plus why it is flagged.
#[derive(Serialize)]
pub struct AuditFinding {
    /// Media-relative source path (`a/b.mp3`) — the name every layer agrees on.
    pub source: String,
    pub detail: String,
}

/// One check and its verdict. `level` is what the UI colours by, `count` what it
/// prints, `advice` + `fix_step` what an agent does next.
#[derive(Serialize)]
pub struct AuditCheck {
    pub id: &'static str,
    pub label: &'static str,
    /// `problem` blocks the pipeline, `warn` is drift worth a look, `info` is
    /// context, `clean` means the check found nothing.
    pub level: &'static str,
    pub count: usize,
    pub items: Vec<AuditFinding>,
    pub truncated: bool,
    pub advice: &'static str,
    /// Step id in the built-in dictation template that repairs this, if any.
    pub fix_step: Option<&'static str>,
    pub fix_label: Option<&'static str>,
}

/// The six booleans the template's `when:` guards evaluate, derived from the same
/// walk as everything else so the graph and the report cannot disagree.
#[derive(Serialize)]
pub struct AuditFacts {
    pub has_media: bool,
    pub has_subtitles: bool,
    pub has_waveforms: bool,
    pub has_database: bool,
    pub has_book: bool,
    pub has_transcript: bool,
}

#[derive(Serialize)]
pub struct DatasetAudit {
    pub dataset_uuid: String,
    pub path: String,
    pub info: DatasetInfo,
    pub facts: AuditFacts,
    pub media_on_disk: usize,
    pub media_in_db: usize,
    pub subtitles_in_db: usize,
    pub cues_in_db: usize,
    pub checks: Vec<AuditCheck>,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// A `listen_subtitle` row plus how many current cues it owns.
struct SubRow {
    uuid: String,
    name: String,
    is_active: bool,
    note: Option<String>,
    cues: usize,
    updated_at: Option<DateTime<Utc>>,
}

/// The VTT file's verdict. "Unreadable" stays distinct from "Missing" so a
/// malformed file is reported as itself instead of masquerading as no subtitle.
enum Vtt {
    Missing,
    Unreadable(String),
    Cues(usize),
}

/// One cue of a current version, in playback order.
struct CueRow {
    order_num: i64,
    start_ms: i64,
    end_ms: i64,
    content: String,
}

/// Everything known about one media file, merged from disk and from the DB.
/// A `None` on either side is itself the finding: media the DB never registered,
/// or rows whose file disappeared.
struct MediaState {
    source: String,
    /// `listen_media.uuid`; `None` when the file is not registered.
    db_uuid: Option<String>,
    duration_ms: Option<i64>,
    waveform: PathBuf,
    vtt_state: Vtt,
    media_mtime: Option<DateTime<Utc>>,
    vtt_mtime: Option<DateTime<Utc>>,
    waveform_mtime: Option<DateTime<Utc>>,
    has_transcript: bool,
    subs: Vec<SubRow>,
    /// Current cues of this media's active subtitles, tagged with the subtitle
    /// name so a finding can say which version it came from.
    cues: Vec<(String, Vec<CueRow>)>,
}

impl MediaState {
    /// Path-without-extension form of the source, i.e. how the artefacts are named.
    fn stem(&self) -> String {
        Path::new(&self.source)
            .with_extension("")
            .to_string_lossy()
            .into_owned()
    }

    fn active_subs(&self) -> impl Iterator<Item = &SubRow> {
        self.subs.iter().filter(|s| s.is_active)
    }

    fn is_adjusted(&self) -> bool {
        self.subs
            .iter()
            .any(|s| s.note.as_deref().unwrap_or("").contains(ADJUSTED_MARKER))
    }

    /// Subtitles that actually exist for this media: an active version with cues.
    /// A dataset may ship its cues straight inside `data.sqlite3` and carry no VTT
    /// file at all, so the file is never the only source of truth.
    fn has_db_cues(&self) -> bool {
        self.active_subs().any(|s| s.cues > 0)
    }

    /// The cue count of the longest active version — a re-import preserves the
    /// count, so the adjustment copy does not change this number.
    fn db_cue_count(&self) -> usize {
        self.active_subs().map(|s| s.cues).max().unwrap_or(0)
    }

    fn finding(&self, detail: String) -> AuditFinding {
        AuditFinding { source: self.source.clone(), detail }
    }
}

/// Filesystem mtime as the same instant type the DB stamps parse into, so every
/// staleness comparison is one `>` away.
fn mtime(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from)
}

fn fmt_ms(ms: i64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// Rough "how long was this left behind" phrasing for a staleness line.
fn human_gap(older: DateTime<Utc>, newer: DateTime<Utc>) -> String {
    let secs = (newer - older).num_seconds().max(0);
    if secs < 120 {
        format!("{secs}s")
    } else if secs < 7200 {
        format!("{} min", secs / 60)
    } else if secs < 172_800 {
        format!("{} h", secs / 3600)
    } else {
        format!("{} days", secs / 86_400)
    }
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Probe a dictation dataset on disk and in its SQLite DB and report every
/// inconsistency. Read-only, so it is safe to call as often as the UI likes.
#[tauri::command]
pub async fn dataset_audit(
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<DatasetAudit, String> {
    log::debug!("[Workflow] Auditing dataset {}", uuid);
    audit(&settings, &uuid)
}

pub(crate) fn audit(settings: &SettingsState, uuid: &str) -> Result<DatasetAudit, String> {
    let dir = find_dataset_dir(settings, uuid)?;
    let info = read_info(&dir)?;
    let media_dir = dir.join("media");
    let subtitle_dir = dir.join("subtitle");
    let waveform_dir = dir.join("waveform");
    let transcript_dir = dir.join("transcript");
    let db_path = dir.join("data.sqlite3");
    let has_book = dir.join("book.txt").exists();

    // Disk inventory first. Every artifact path is derived from its media file by
    // the same mirroring the pipeline writes with: media/a/b.mp3 -> a/b.vtt,
    // waveform/a/b.json, transcript/a/b.txt.
    let mut media: Vec<MediaState> = list_media_files(&media_dir)
        .iter()
        .map(|mf| {
            let abs = PathBuf::from(&mf.path);
            let vtt = sibling_path(&media_dir, &subtitle_dir, &abs, "vtt");
            let waveform = sibling_path(&media_dir, &waveform_dir, &abs, "json");
            let has_transcript = mf.has_transcript
                || sibling_path(&media_dir, &transcript_dir, &abs, "txt").exists();
            let vtt_state = if !vtt.exists() {
                Vtt::Missing
            } else {
                match parse_vtt(&vtt) {
                    Ok(cues) => Vtt::Cues(cues.len()),
                    Err(e) => Vtt::Unreadable(e),
                }
            };
            MediaState {
                source: rel_source_string(&media_dir, &abs),
                media_mtime: mtime(&abs),
                vtt_mtime: mtime(&vtt),
                waveform_mtime: mtime(&waveform),
                waveform,
                vtt_state,
                has_transcript,
                db_uuid: None,
                duration_ms: None,
                subs: Vec::new(),
                cues: Vec::new(),
            }
        })
        .collect();

    let mut facts = AuditFacts {
        has_media: !media.is_empty(),
        has_subtitles: media.iter().any(|m| !matches!(m.vtt_state, Vtt::Missing)),
        has_waveforms: media.iter().any(|m| m.waveform.exists()),
        has_database: db_path.exists(),
        has_book,
        has_transcript: media.iter().any(|m| m.has_transcript),
    };

    if !db_path.exists() {
        // No rows to compare against: report the single thing standing in the way.
        let mut checks = vec![check(
            "database_missing",
            "Cue database",
            "problem",
            vec![AuditFinding {
                source: "data.sqlite3".into(),
                detail: format!(
                    "the folder holds {} media file(s) but has no SQLite database",
                    media.len()
                ),
            }],
            "Initialize this folder as a dataset, then Sync media to register its \
             files — every later step writes into this database.",
            Some("init_dataset"),
            Some("Initialize dataset"),
        )];
        sort_checks(&mut checks);
        return Ok(DatasetAudit {
            dataset_uuid: uuid.to_string(),
            path: dir.to_string_lossy().into_owned(),
            info,
            facts,
            media_on_disk: media.len(),
            media_in_db: 0,
            subtitles_in_db: 0,
            cues_in_db: 0,
            checks,
        });
    }

    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    attach_db_state(&conn, &mut media)?;

    // Cues shipped in the database count as subtitles too — see `has_db_cues`.
    if !facts.has_subtitles {
        facts.has_subtitles = media.iter().any(MediaState::has_db_cues);
    }

    let mut checks = build_checks(&media, &conn, has_book);
    sort_checks(&mut checks);

    Ok(DatasetAudit {
        dataset_uuid: uuid.to_string(),
        path: dir.to_string_lossy().into_owned(),
        info,
        facts,
        media_on_disk: media.len(),
        media_in_db: count(&conn, "SELECT COUNT(*) FROM listen_media"),
        subtitles_in_db: count(&conn, "SELECT COUNT(*) FROM listen_subtitle"),
        cues_in_db: count(
            &conn,
            "SELECT COUNT(*) FROM listen_subtitle_cue WHERE version_superseded IS NULL",
        ),
        checks,
    })
}

// ---------------------------------------------------------------------------
// Database side
// ---------------------------------------------------------------------------

fn count(conn: &Connection, sql: &str) -> usize {
    conn.query_row(sql, [], |r| r.get::<_, usize>(0))
        .unwrap_or(0)
}

/// Read the DB inventory in three bulk queries and hang it off the disk walk.
/// Bulk rather than per-media on purpose: a 300-file dataset stays one pass over
/// each table instead of 300 small ones.
fn attach_db_state(conn: &Connection, media: &mut [MediaState]) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT uuid, source, duration_ms FROM listen_media")
        .map_err(|e| e.to_string())?;
    let mut by_source: HashMap<String, (String, Option<i64>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                (row.get::<_, String>(0)?, row.get::<_, Option<i64>>(2)?),
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Subtitles with their current-cue counts. `version_superseded IS NULL` is the
    // versioning scheme's rule for "what a player actually shows".
    let mut stmt = conn
        .prepare(
            "SELECT s.uuid, s.media_uuid, s.name, s.is_active, s.note, s.updated_at,
                    (SELECT COUNT(*) FROM listen_subtitle_cue c
                      WHERE c.subtitle_uuid = s.uuid AND c.version_superseded IS NULL)
             FROM listen_subtitle s",
        )
        .map_err(|e| e.to_string())?;
    let mut by_media: HashMap<String, Vec<SubRow>> = HashMap::new();
    let subs = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                SubRow {
                    uuid: row.get(0)?,
                    name: row.get(2)?,
                    is_active: row.get::<_, i64>(3)? != 0,
                    note: row.get(4)?,
                    updated_at: parse_stamp(&row.get::<_, String>(5)?),
                    cues: row.get(6)?,
                },
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    drop(stmt);
    for (media_uuid, sub) in subs {
        by_media.entry(media_uuid).or_default().push(sub);
    }

    // Cues of the active versions only, ordered per subtitle so the sequential
    // sanity checks (overlap, monotonic order_num) are a single pass.
    let mut stmt = conn
        .prepare(
            "SELECT c.subtitle_uuid, c.order_num, c.start_ms, c.end_ms, c.content
             FROM listen_subtitle_cue c
             JOIN listen_subtitle s ON s.uuid = c.subtitle_uuid
             WHERE c.version_superseded IS NULL AND s.is_active = 1
             ORDER BY c.subtitle_uuid, c.order_num",
        )
        .map_err(|e| e.to_string())?;
    let mut cues_by_sub: HashMap<String, Vec<CueRow>> = HashMap::new();
    let cues = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                CueRow {
                    order_num: row.get(1)?,
                    start_ms: row.get(2)?,
                    end_ms: row.get(3)?,
                    content: row.get(4)?,
                },
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    drop(stmt);
    for (sub_uuid, cue) in cues {
        cues_by_sub.entry(sub_uuid).or_default().push(cue);
    }

    for m in media.iter_mut() {
        let Some((media_uuid, duration_ms)) = by_source.remove(m.source.as_str()) else {
            continue;
        };
        m.db_uuid = Some(media_uuid.clone());
        m.duration_ms = duration_ms;
        m.subs = by_media.remove(&media_uuid).unwrap_or_default();
        m.cues = m
            .active_subs()
            .map(|s| {
                let rows = cues_by_sub.remove(&s.uuid).unwrap_or_default();
                (s.name.clone(), rows)
            })
            .collect();
    }
    Ok(())
}

/// A `listen_media` row summarized for the "row without a file" check.
struct DbMediaRow {
    source: String,
    subtitles: usize,
    cues: usize,
}

fn db_inventory(conn: &Connection) -> Vec<DbMediaRow> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT m.source,
                (SELECT COUNT(*) FROM listen_subtitle s WHERE s.media_uuid = m.uuid),
                (SELECT COUNT(*) FROM listen_subtitle_cue c
                  WHERE c.subtitle_uuid IN (SELECT uuid FROM listen_subtitle WHERE media_uuid = m.uuid)
                    AND c.version_superseded IS NULL)
         FROM listen_media m",
    ) else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([], |row| {
            Ok(DbMediaRow {
                source: row.get(0)?,
                subtitles: row.get(1)?,
                cues: row.get(2)?,
            })
        })
        .map_err(|_| ())
        .and_then(|it| it.collect::<Result<Vec<_>, _>>().map_err(|_| ()))
        .unwrap_or_default();
    rows
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

/// Assemble one check: the exact total is taken before the item list is capped.
fn check(
    id: &'static str,
    label: &'static str,
    level: &'static str,
    items: Vec<AuditFinding>,
    advice: &'static str,
    fix_step: Option<&'static str>,
    fix_label: Option<&'static str>,
) -> AuditCheck {
    let count = items.len();
    AuditCheck {
        id,
        label,
        level: if count == 0 { "clean" } else { level },
        count,
        truncated: count > ITEM_CAP,
        items: items.into_iter().take(ITEM_CAP).collect(),
        advice,
        fix_step,
        fix_label,
    }
}

/// Severity order, so the report leads with what blocks the pipeline.
fn sort_checks(checks: &mut Vec<AuditCheck>) {
    checks.sort_by_key(|c| {
        (
            match c.level {
                "problem" => 0,
                "warn" => 1,
                "info" => 2,
                _ => 3,
            },
            c.id,
        )
    });
}

fn build_checks(media: &[MediaState], conn: &Connection, has_book: bool) -> Vec<AuditCheck> {
    let mut new_media = Vec::new();
    let mut unreadable_vtt = Vec::new();
    let mut no_subtitle = Vec::new();
    let mut not_imported = Vec::new();
    let mut out_of_sync = Vec::new();
    let mut no_waveform = Vec::new();
    let mut stale_waveforms = Vec::new();
    let mut unadjusted = Vec::new();
    let mut no_waveform_for_adjust = Vec::new();
    let mut sanity = Vec::new();
    let mut versions = Vec::new();

    for m in media {
        let stem = m.stem();

        // (1) media folder vs the registered inventory.
        if m.db_uuid.is_none() {
            new_media.push(m.finding(
                "on disk with no listen_media row — nothing downstream can see it".into(),
            ));
        }

        // (2) Subtitles: look on disk *and* in the rows, never at the file alone.
        let vtt_cues = match &m.vtt_state {
            Vtt::Missing => None,
            Vtt::Unreadable(e) => {
                let mut detail = format!("subtitle/{}.vtt cannot be parsed: {e}", stem);
                if m.has_db_cues() {
                    // A broken file with usable rows: worth naming, but practice works.
                    detail.push_str(&format!(
                        " (the database still holds {} cue(s), so playback is unaffected)",
                        m.db_cue_count()
                    ));
                }
                unreadable_vtt.push(m.finding(detail));
                None
            }
            Vtt::Cues(n) => Some(*n),
        };

        // The normal case is a media file with no VTT sibling because its cues
        // already live in `data.sqlite3`. Only the absence of *both* is a gap.
        if vtt_cues.is_none() && !m.has_db_cues() {
            no_subtitle.push(m.finding(format!(
                "neither subtitle/{}.vtt nor cue rows exist — Generate subtitles transcribes it",
                stem
            )));
        }

        if let Some(vtt_cues) = vtt_cues {
            let actives: Vec<&SubRow> = m.active_subs().collect();

            // (3) imported into the database at all.
            if !m.has_db_cues() {
                not_imported.push(m.finding(format!(
                    "subtitle/{}.vtt holds {} cue(s), the database has {}",
                    stem,
                    vtt_cues,
                    if actives.is_empty() {
                        "no active subtitle"
                    } else {
                        "an active subtitle with 0 cues"
                    }
                )));
            } else {
                // (4) in sync? A re-import preserves the cue count, so a differing
                //     count means file and rows diverged. An adjusted copy counts
                //     the same, so any active row matching the file is enough.
                if !actives.iter().any(|s| s.cues == vtt_cues) {
                    let db = actives
                        .iter()
                        .map(|s| format!("“{}” → {} cue(s)", s.name, s.cues))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out_of_sync.push(m.finding(format!(
                        "subtitle/{}.vtt has {} cue(s) but the database has {}",
                        stem, vtt_cues, db
                    )));
                } else if let (Some(file_mtime), Some(imported)) =
                    (m.vtt_mtime, actives.iter().filter_map(|s| s.updated_at).max())
                {
                    if file_mtime > imported {
                        // The file was edited after its rows were written.
                        out_of_sync.push(m.finding(format!(
                            "subtitle/{}.vtt was changed {} after the database imported it",
                            stem,
                            human_gap(imported, file_mtime)
                        )));
                    } else if let Some(audio_mtime) = m.media_mtime {
                        // (5) or the audio was re-recorded after transcription.
                        if audio_mtime > file_mtime {
                            out_of_sync.push(m.finding(format!(
                                "the audio changed {} after subtitle/{}.vtt was written — re-transcribe",
                                human_gap(file_mtime, audio_mtime),
                                stem
                            )));
                        }
                    }
                }
            }
        }

        // (6) Adjustment is a property of the rows, not of the VTT file, so it is
        // checked for every media that has cues somewhere — including the
        // database-only ones this loop used to skip.
        if m.has_db_cues() && !m.is_adjusted() {
            if m.waveform.exists() {
                unadjusted.push(m.finding(format!(
                    "{} cue(s) still on raw STT timings; no subtitle note says “{}”",
                    m.db_cue_count(),
                    ADJUSTED_MARKER
                )));
            } else {
                no_waveform_for_adjust.push(m.finding(
                    "cannot be time-adjusted until its waveform is generated".into(),
                ));
            }
        }

        // Waveform coverage and freshness, independent of the subtitle state.
        if !m.waveform.exists() {
            no_waveform.push(m.finding(format!(
                "no waveform/{}.json — the player has no peaks and Adjust cue times no energy envelope",
                stem
            )));
        } else if let (Some(audio), Some(wf)) = (m.media_mtime, m.waveform_mtime) {
            if audio > wf {
                stale_waveforms.push(m.finding(format!(
                    "waveform/{}.json is {} older than the audio, so its peaks and silences no longer line up",
                    stem,
                    human_gap(wf, audio)
                )));
            }
        }

        // (7) cue-level sanity over what actually plays.
        for (sub_name, cues) in &m.cues {
            let mut prev_end: Option<i64> = None;
            let mut prev_order: Option<i64> = None;
            for cue in cues {
                let at = format!("“{}” cue #{}", sub_name, cue.order_num);
                if cue.content.trim().is_empty() {
                    sanity.push(m.finding(format!("{} ({}) has no text", at, fmt_ms(cue.start_ms))));
                }
                if cue.end_ms <= cue.start_ms {
                    sanity.push(m.finding(format!(
                        "{} ends at or before it starts ({} → {})",
                        at,
                        fmt_ms(cue.start_ms),
                        fmt_ms(cue.end_ms)
                    )));
                }
                if let Some(pe) = prev_end {
                    if cue.start_ms < pe {
                        sanity.push(m.finding(format!(
                            "{} starts inside its predecessor ({} < {})",
                            at,
                            fmt_ms(cue.start_ms),
                            fmt_ms(pe)
                        )));
                    }
                }
                if let Some(po) = prev_order {
                    if cue.order_num <= po {
                        sanity.push(m.finding(format!("{} does not follow #{}", at, po)));
                    }
                }
                if let Some(dur) = m.duration_ms {
                    if cue.end_ms > dur {
                        sanity.push(m.finding(format!(
                            "{} ends at {} but the media is only {}",
                            at,
                            fmt_ms(cue.end_ms),
                            fmt_ms(dur)
                        )));
                    }
                }
                prev_end = Some(cue.end_ms);
                prev_order = Some(cue.order_num);
            }
        }

        // (8) versions that make playback ambiguous. More than one active row is
        //     normal after `mode=new` adjustment — that is what the note marks —
        //     so only unmarked duplicates are worth reporting.
        let active_count = m.active_subs().count();
        if active_count > 1 && !m.is_adjusted() {
            versions.push(m.finding(format!(
                "{} subtitles are active at once, so which cues play is undefined",
                active_count
            )));
        }
        for s in m.subs.iter().filter(|s| !s.is_active && s.cues > 0) {
            versions.push(m.finding(format!(
                "inactive subtitle “{}” still holds {} cue(s)",
                s.name, s.cues
            )));
        }
    }

    // Rows whose file no longer exists. Their `source` is the only name left.
    let on_disk: HashSet<&str> = media.iter().map(|m| m.source.as_str()).collect();
    let gone: Vec<AuditFinding> = db_inventory(conn)
        .into_iter()
        .filter(|row| !on_disk.contains(row.source.as_str()))
        .map(|row| AuditFinding {
            source: row.source.clone(),
            detail: format!(
                "listen_media row with {} subtitle(s) and {} cue(s), but media/{} is not on disk",
                row.subtitles, row.cues, row.source
            ),
        })
        .collect();

    // Reference material decides whether the two alignment steps can run at all.
    let has_transcript = media.iter().any(|m| m.has_transcript);
    let reference_items: Vec<AuditFinding> = if has_book {
        Vec::new()
    } else {
        media
            .iter()
            .filter(|m| !m.has_transcript)
            .map(|m| m.finding("no transcript/.txt either".into()))
            .collect()
    };
    let mut reference = check(
        "reference_material",
        "Reference text for alignment",
        "info",
        reference_items,
        if has_book {
            "book.txt is present, so Align cues (book) can match the cue text against it."
        } else if has_transcript {
            "Per-media transcripts exist, so Align cues (transcript) can run."
        } else {
            "Neither book.txt nor transcript/ files exist, so both alignment steps are skipped and the cues keep their raw STT text."
        },
        Some("detect_reference"),
        Some("Detect reference"),
    );
    // Always informational: a dataset is legitimately practisable without any
    // reference, so an empty list here is not a failure.
    if has_book || has_transcript {
        reference.level = "info";
    }

    vec![
        check(
            "media_new",
            "New media not in the database",
            "problem",
            new_media,
            "Files under media/ with no listen_media row. Sync media registers them, and every later step keys off that table.",
            Some("sync_media"),
            Some("Sync media"),
        ),
        check(
            "media_gone",
            "Database rows without a media file",
            "warn",
            gone,
            "Registered media whose audio is gone. Sync media deletes the rows and cascades to their subtitles, cues, versions and transcripts.",
            Some("sync_media"),
            Some("Sync media"),
        ),
        check(
            "subtitles_missing",
            "Media with no subtitles anywhere",
            "problem",
            no_subtitle,
            "Neither a subtitle/<name>.vtt nor cue rows exist, so nothing is heard-and-typed yet. A missing VTT whose cues are already in the database is normal and is not reported here.",
            Some("generate_subtitles"),
            Some("Generate subtitles"),
        ),
        check(
            "subtitles_unreadable",
            "VTT file that cannot be parsed",
            "problem",
            unreadable_vtt,
            "The file exists but is not valid WebVTT, so its cues can never be re-imported. Delete the subtitle and regenerate.",
            Some("generate_subtitles"),
            Some("Generate subtitles"),
        ),
        check(
            "subtitles_not_imported",
            "Subtitles not in the database",
            "problem",
            not_imported,
            "A VTT file exists but its cues never reached data.sqlite3, so the practice UI has nothing to show.",
            Some("generate_subtitles"),
            Some("Generate subtitles"),
        ),
        check(
            "subtitles_out_of_sync",
            "VTT file and database disagree",
            "warn",
            out_of_sync,
            "Cue counts differ, or a file was touched after the import that wrote its rows. Re-import with Write subtitles to database after deleting the stale subtitle — Generate subtitles leaves media that already have an active subtitle alone.",
            Some("generate_subtitles"),
            Some("Generate subtitles"),
        ),
        check(
            "waveforms_missing",
            "Media without a waveform",
            "problem",
            no_waveform,
            "No waveform/<name>.json. Adjust cue times reads it for its energy envelope and the player draws it.",
            Some("generate_waveforms"),
            Some("Generate waveforms"),
        ),
        check(
            "waveforms_stale",
            "Waveform older than its audio",
            "warn",
            stale_waveforms,
            "The audio was replaced after the peaks were computed, so the waveform no longer describes the file being played.",
            Some("generate_waveforms"),
            Some("Generate waveforms"),
        ),
        check(
            "cues_unadjusted",
            "Cues not time-adjusted",
            "problem",
            unadjusted,
            "Cues are still on raw STT timings while a waveform is available, so Adjust cue times can snap them to silence.",
            Some("adjust_cue_times"),
            Some("Adjust cue times"),
        ),
        check(
            "cue_sanity",
            "Suspicious cues",
            "warn",
            sanity,
            "Empty, zero-length, overlapping, out-of-order or past-the-end cues. Fix these in the Dictation page’s cue editor — no workflow step rewrites them on its own.",
            None,
            None,
        ),
        check(
            "subtitle_versions",
            "Ambiguous subtitle versions",
            "warn",
            versions,
            "More than one active version without an adjustment note, or inactive versions still holding cues. Play the intended one (or delete the leftovers) so the choice is not arbitrary.",
            None,
            None,
        ),
        reference,
        check(
            "adjust_blocked_no_waveform",
            "Adjustment waiting on waveforms",
            "info",
            no_waveform_for_adjust,
            "These media have cues but no waveform yet, so Adjust cue times will skip them until Generate waveforms has run.",
            Some("generate_waveforms"),
            Some("Generate waveforms"),
        ),
    ]
}
