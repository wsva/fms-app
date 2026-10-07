//! Rust port of dataset_studio's transcript alignment.
//!
//! Ports `lib/text.py` (normalize + regex sentence splitting), `lib/alignment.py`
//! (multi-pass anchor-based alignment) and `scripts/align_cues_transcript.py`
//! (per-subtitle transcript alignment writing to `listen_subtitle_cue.reference`).
//!
//! Similarity uses a Ratcliff-Obershelp ratio to reproduce Python's
//! `difflib.SequenceMatcher.ratio()`. Sentence splitting uses dataset_studio's
//! regex fallback (no NLTK).
//!
//! Two intentional refinements over the reference `lib/alignment.py`:
//! 1. `similarity_score` containment is length-aware — a short fragment inside a
//!    long multi-sentence group no longer scores a false 90 anchor.
//! 2. `find_anchors` picks the best-scoring reference window, and when a cue
//!    spans several reference sentences `multi_pass_align` joins them into the
//!    cue's `reference` (1↔N) instead of orphaning the surplus sentences.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, State};

use crate::datasets::{find_dataset_dir, DatasetInfo, DatasetProgress};
use crate::settings::SettingsState;
use crate::datasets::textsim::normalize;

// The scoring helpers live in the always-compiled `textsim` module so they are
// available on mobile too. Re-exported here so existing `crate::datasets::dictation::align::
// similarity_score` callers (e.g. adjust.rs) keep working unchanged.
pub(crate) use crate::datasets::textsim::similarity_score;

// ---------------------------------------------------------------------------
// Configuration (mirrors lib/alignment.py)
// ---------------------------------------------------------------------------

const GAP_PENALTY: f64 = 30.0;
const MIN_MATCH_SCORE: f64 = 60.0;
const MIN_ANCHOR_LENGTH: usize = 20;
const MAX_WINDOW_SIZE: usize = 4;
const ANCHOR_THRESHOLDS: [f64; 5] = [100.0, 90.0, 80.0, 70.0, 60.0];

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Cue {
    uuid: String,
    order_num: i64,
    content: String,
}

#[derive(Clone)]
struct Ref {
    uuid: String,
    order_num: i64,
    content: String,
}

#[derive(Clone, Copy, PartialEq)]
enum MatchType {
    Anchor,
    Inferred,
}

#[derive(Clone)]
struct Match {
    cue: Cue,
    /// Last consumed reference (used for order_num gap boundaries).
    ref_: Ref,
    /// Reference text to persist; joined when a cue spans multiple ref sentences.
    ref_text: String,
    #[allow(dead_code)]
    score: f64,
    match_type: MatchType,
}

struct AnchorGroup {
    cues: Vec<Cue>,
    refs: Vec<Ref>,
    score: f64,
}

// ---------------------------------------------------------------------------
// Text helpers (port of lib/text.py)
// ---------------------------------------------------------------------------

fn is_sentence_end_char(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{ff01}' | '\u{ff1f}' | '\u{2026}')
}

/// Port of `_regex_split_sentences`: split on `(?<=[.!?。！？…])\s+`.
fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        current.push(c);
        if is_sentence_end_char(c) {
            let mut j = i + 1;
            if j < chars.len() && chars[j].is_whitespace() {
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    sentences.push(trimmed);
                }
                current.clear();
                i = j;
                continue;
            }
        }
        i += 1;
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        sentences.push(trimmed);
    }
    sentences
}

// ---------------------------------------------------------------------------
// Alignment (port of lib/alignment.py)
// ---------------------------------------------------------------------------

fn find_anchors(
    cues: &[Cue],
    refs: &[Ref],
    threshold: f64,
    max_window: usize,
    min_length: usize,
) -> Vec<AnchorGroup> {
    let mut anchors: Vec<AnchorGroup> = Vec::new();
    let mut used_cue: HashSet<usize> = HashSet::new();
    let mut used_ref: HashSet<usize> = HashSet::new();

    for window_size in 1..=max_window {
        if cues.len() < window_size {
            continue;
        }
        for i in 0..=(cues.len() - window_size) {
            if (0..window_size).any(|k| used_cue.contains(&(i + k))) {
                continue;
            }
            let cue_group = &cues[i..i + window_size];
            let combined: String = cue_group
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if normalize(&combined).chars().count() < min_length {
                continue;
            }

            // Pick the BEST-scoring ref window for this cue window (not merely the
            // first that clears the threshold), so a cue spanning several reference
            // sentences groups with ALL of them and none are orphaned.
            let mut best: Option<(f64, usize, usize)> = None; // (score, j, ref_window)
            for ref_window in 1..=max_window {
                if refs.len() < ref_window {
                    continue;
                }
                for j in 0..=(refs.len() - ref_window) {
                    if (0..ref_window).any(|k| used_ref.contains(&(j + k))) {
                        continue;
                    }
                    let ref_group = &refs[j..j + ref_window];
                    let ref_combined: String = ref_group
                        .iter()
                        .map(|r| r.content.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    let score = similarity_score(&combined, &ref_combined);
                    if score >= threshold && best.map_or(true, |(bs, _, _)| score > bs) {
                        best = Some((score, j, ref_window));
                    }
                }
            }
            if let Some((score, j, ref_window)) = best {
                anchors.push(AnchorGroup {
                    cues: cue_group.to_vec(),
                    refs: refs[j..j + ref_window].to_vec(),
                    score,
                });
                for k in 0..window_size {
                    used_cue.insert(i + k);
                }
                for k in 0..ref_window {
                    used_ref.insert(j + k);
                }
            }
        }
    }
    anchors
}

fn dp_align(cues: &[Cue], refs: &[Ref]) -> Vec<Match> {
    if cues.is_empty() || refs.is_empty() {
        return Vec::new();
    }
    let n = cues.len();
    let m = refs.len();
    let mut dp = vec![vec![0f64; m + 1]; n + 1];
    // direction: 0 = UP, 1 = LEFT, 2 = MATCH (with score)
    let mut back = vec![vec![(0u8, 0f64); m + 1]; n + 1];

    for i in 1..=n {
        dp[i][0] = dp[i - 1][0] - GAP_PENALTY;
        back[i][0] = (0, 0.0);
    }
    for j in 1..=m {
        dp[0][j] = dp[0][j - 1] - GAP_PENALTY;
        back[0][j] = (1, 0.0);
    }
    for i in 1..=n {
        for j in 1..=m {
            let match_score = similarity_score(&cues[i - 1].content, &refs[j - 1].content);
            let diag = dp[i - 1][j - 1] + match_score;
            let up = dp[i - 1][j] - GAP_PENALTY;
            let left = dp[i][j - 1] - GAP_PENALTY;
            let best = diag.max(up).max(left);
            dp[i][j] = best;
            if best == diag {
                back[i][j] = (2, match_score);
            } else if best == up {
                back[i][j] = (0, 0.0);
            } else {
                back[i][j] = (1, 0.0);
            }
        }
    }

    let mut matches: Vec<Match> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let step = back[i][j];
        match step.0 {
            2 => {
                let score = step.1;
                if score >= MIN_MATCH_SCORE {
                    matches.push(Match {
                        cue: cues[i - 1].clone(),
                        ref_: refs[j - 1].clone(),
                        ref_text: refs[j - 1].content.clone(),
                        score,
                        match_type: MatchType::Inferred,
                    });
                }
                i -= 1;
                j -= 1;
            }
            0 => i -= 1,
            _ => j -= 1,
        }
    }
    matches.reverse();
    matches
}

fn multi_pass_align(cues: &[Cue], refs: &[Ref]) -> Vec<Match> {
    let mut all_matches: Vec<Match> = Vec::new();
    let mut matched_cue_uuids: HashSet<String> = HashSet::new();
    let mut matched_ref_uuids: HashSet<String> = HashSet::new();

    // Pass 1 & 2: find anchors at each threshold level.
    for threshold in ANCHOR_THRESHOLDS.iter() {
        let unmatched_cues: Vec<Cue> = cues
            .iter()
            .filter(|c| !matched_cue_uuids.contains(&c.uuid))
            .cloned()
            .collect();
        let unmatched_refs: Vec<Ref> = refs
            .iter()
            .filter(|r| !matched_ref_uuids.contains(&r.uuid))
            .cloned()
            .collect();
        if unmatched_cues.is_empty() || unmatched_refs.is_empty() {
            break;
        }
        let new_anchors = find_anchors(
            &unmatched_cues,
            &unmatched_refs,
            *threshold,
            MAX_WINDOW_SIZE,
            MIN_ANCHOR_LENGTH,
        );
        for anchor in new_anchors {
            let fresh_cues: Vec<Cue> = anchor
                .cues
                .iter()
                .filter(|c| !matched_cue_uuids.contains(&c.uuid))
                .cloned()
                .collect();
            let fresh_refs: Vec<Ref> = anchor
                .refs
                .iter()
                .filter(|r| !matched_ref_uuids.contains(&r.uuid))
                .cloned()
                .collect();
            if fresh_cues.is_empty() || fresh_refs.is_empty() {
                continue;
            }
            let n_c = fresh_cues.len();
            let n_r = fresh_refs.len();
            // Positional pairing; when a cue window matched MORE refs than cues
            // (n_r > n_c) the surplus refs fold into the LAST cue's reference text,
            // so a multi-sentence cue keeps its full book text instead of orphaning
            // the extra reference sentences.
            for (idx, cue) in fresh_cues.iter().enumerate() {
                if idx >= n_r {
                    break; // more cues than refs: extras fall through to gap-fill/final DP
                }
                let is_last = idx == n_c - 1;
                let end = if is_last { n_r } else { idx + 1 };
                let ref_text: String = fresh_refs[idx..end]
                    .iter()
                    .map(|r| r.content.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let last_ref = fresh_refs[end - 1].clone();
                all_matches.push(Match {
                    cue: cue.clone(),
                    ref_: last_ref,
                    ref_text,
                    score: anchor.score,
                    match_type: MatchType::Anchor,
                });
                matched_cue_uuids.insert(cue.uuid.clone());
                for r in &fresh_refs[idx..end] {
                    matched_ref_uuids.insert(r.uuid.clone());
                }
            }
        }
    }

    // Pass 3: fill gaps between anchors.
    let mut anchor_matches: Vec<Match> = all_matches
        .iter()
        .filter(|m| m.match_type == MatchType::Anchor)
        .cloned()
        .collect();
    anchor_matches.sort_by_key(|m| m.cue.order_num);

    let mut prev_cue_idx: i64 = -1;
    let mut prev_ref_idx: i64 = -1;
    for anchor in anchor_matches.iter() {
        let gap_cues: Vec<Cue> = cues
            .iter()
            .filter(|c| {
                prev_cue_idx < c.order_num
                    && c.order_num < anchor.cue.order_num
                    && !matched_cue_uuids.contains(&c.uuid)
            })
            .cloned()
            .collect();
        let gap_refs: Vec<Ref> = refs
            .iter()
            .filter(|r| {
                prev_ref_idx < r.order_num
                    && r.order_num < anchor.ref_.order_num
                    && !matched_ref_uuids.contains(&r.uuid)
            })
            .cloned()
            .collect();
        if !gap_cues.is_empty() && !gap_refs.is_empty() {
            let gap_matches = dp_align(&gap_cues, &gap_refs);
            for m in gap_matches.iter() {
                matched_cue_uuids.insert(m.cue.uuid.clone());
                matched_ref_uuids.insert(m.ref_.uuid.clone());
            }
            all_matches.extend(gap_matches);
        }
        prev_cue_idx = anchor.cue.order_num;
        prev_ref_idx = anchor.ref_.order_num;
    }

    // Final gap: after last anchor to end.
    let remaining_cues: Vec<Cue> = cues
        .iter()
        .filter(|c| !matched_cue_uuids.contains(&c.uuid))
        .cloned()
        .collect();
    let remaining_refs: Vec<Ref> = refs
        .iter()
        .filter(|r| !matched_ref_uuids.contains(&r.uuid))
        .cloned()
        .collect();
    if !remaining_cues.is_empty() && !remaining_refs.is_empty() {
        let final_matches = dp_align(&remaining_cues, &remaining_refs);
        all_matches.extend(final_matches);
    }

    all_matches
}

// ---------------------------------------------------------------------------
// Transcript loading (port of align_cues_transcript.load_transcript_sentences)
// ---------------------------------------------------------------------------

/// Split a book/transcript text into sentences: paragraphs (`\n\n`) → lines
/// (`\n`) → sentence boundaries. Rust port of dataset_studio's `split_book.py`
/// sentence extraction, shared by the "split book" stage and transcript
/// alignment so both agree on where sentences start and end.
pub(crate) fn split_book_sentences(text: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    for para in text.split("\n\n") {
        let p = para.trim();
        if p.is_empty() {
            continue;
        }
        for line in p.split('\n') {
            let l = line.trim();
            if l.is_empty() {
                continue;
            }
            sentences.extend(split_sentences(l));
        }
    }
    sentences
}

/// Wrap already-split sentences into ordered `Ref`s (uuid = line index). Shared
/// by the book and transcript align commands.
fn refs_from_sentences(sentences: Vec<String>) -> Vec<Ref> {
    sentences
        .into_iter()
        .enumerate()
        .map(|(i, s)| Ref {
            uuid: i.to_string(),
            order_num: i as i64,
            content: s,
        })
        .collect()
}

fn load_transcript_sentences(text: &str) -> Vec<Ref> {
    refs_from_sentences(split_book_sentences(text))
}

/// Bump info.json's `updated` timestamp (best-effort).
fn touch_info(dataset_dir: &PathBuf) {
    let info_path = dataset_dir.join("info.json");
    if let Ok(data) = fs::read_to_string(&info_path) {
        if let Ok(mut info) = serde_json::from_str::<DatasetInfo>(&data) {
            info.updated = Utc::now().to_rfc3339();
            if let Ok(out) = serde_json::to_string_pretty(&info) {
                let _ = fs::write(&info_path, out);
            }
        }
    }
}

/// Emit a per-media progress event so the Studio UI shows live alignment
/// progress, matching the "subtitles"/"waveform"/"database" stages.
fn emit_align_progress(app: &AppHandle, uuid: &str, index: usize, total: usize, media: &str) {
    let _ = app.emit(
        "dataset-progress",
        DatasetProgress {
            uuid: uuid.to_string(),
            current_file: media.to_string(),
            file_index: index,
            total_files: total,
            stage: "align".into(),
        },
    );
}

// ---------------------------------------------------------------------------
// Parallel alignment helpers
// ---------------------------------------------------------------------------

/// One subtitle's alignment work for the transcript variant: its cues plus its
/// own reference sentences. The book variant shares one `refs` slice across all
/// jobs, so its [`BookJob`] carries no per-job references.
struct TranscriptJob {
    subtitle_uuid: String,
    label: String,
    cues: Vec<Cue>,
    refs: Vec<Ref>,
}

/// One subtitle's alignment work for the book variant (references are shared).
struct BookJob {
    subtitle_uuid: String,
    label: String,
    cues: Vec<Cue>,
}

/// Load every cue for a subtitle, in order.
fn load_cues(conn: &Connection, subtitle_uuid: &str) -> Result<Vec<Cue>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT uuid, order_num, content FROM listen_subtitle_cue \
             WHERE subtitle_uuid = ?1 ORDER BY order_num",
        )
        .map_err(|e| e.to_string())?;
    let mapped = stmt
        .query_map(rusqlite::params![subtitle_uuid], |row| {
            Ok(Cue {
                uuid: row.get::<_, String>(0)?,
                order_num: row.get::<_, i64>(1)?,
                content: row.get::<_, String>(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(mapped.filter_map(|r| r.ok()).collect())
}

/// Worker-thread count: the machine's CPU parallelism (leaving one core free for
/// the UI), capped by the job count so we never spawn idle threads.
fn worker_threads(jobs: usize) -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let budget = cores.saturating_sub(1).max(1);
    jobs.min(budget).max(1)
}

/// Run `f(i)` for every index `0..n` across a pool of scoped worker threads
/// (sized by [`worker_threads`]), emitting a "dataset-progress" event as each
/// item completes. Results come back sorted by index.
///
/// Only the CPU-bound alignment runs here: every SQLite access stays on the
/// calling thread because `rusqlite::Connection` is neither `Send` nor `Sync`.
fn parallel_map<T, F>(
    app: &AppHandle,
    uuid: &str,
    n: usize,
    labels: &[String],
    f: F,
) -> Vec<(usize, T)>
where
    F: Fn(usize) -> T + Sync,
    T: Send,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    if n == 0 {
        return Vec::new();
    }

    let results: Mutex<Vec<(usize, T)>> = Mutex::new(Vec::with_capacity(n));
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);

    std::thread::scope(|s| {
        for _ in 0..worker_threads(n) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let value = f(i);
                if let Ok(mut guard) = results.lock() {
                    guard.push((i, value));
                }
                let completed = done.fetch_add(1, Ordering::Relaxed) + 1;
                let label = labels.get(i).map(String::as_str).unwrap_or("");
                emit_align_progress(app, uuid, completed, n, label);
            });
        }
    });

    let mut out = results.into_inner().unwrap_or_default();
    out.sort_by_key(|(i, _)| *i);
    out
}

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

/// Align subtitle cues with reference text from transcript/{media title}.txt.
/// Per subtitle, resets `listen_subtitle_cue.reference` to NULL and rewrites the
/// matched cues in one transaction, so stale references from a previous run
/// never survive.
#[tauri::command]
pub async fn dataset_align_cues_transcript(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    uuid: String,
) -> Result<String, String> {
    log::info!("dataset_align_cues_transcript: dataset={}", uuid);
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    let transcript_dir = dataset_dir.join("transcript");

    if !db_path.exists() {
        return Err("Database file not found. Please build the database first.".into());
    }
    if !transcript_dir.exists() {
        return Err("No transcript directory found in dataset.".into());
    }

    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // All subtitles joined with their media (title + media-relative source path).
    let rows: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT s.uuid, m.title, m.source FROM listen_subtitle s \
                 JOIN listen_media m ON s.media_uuid = m.uuid ORDER BY m.source",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };

    if rows.is_empty() {
        return Err("No subtitles found in database. Generate subtitles first.".into());
    }
    log::info!("Aligning {} subtitles against transcripts", rows.len());
    // sentences and cues on the single connection. Media without a usable
    // transcript are skipped (and reported) and never cleared.
    let mut jobs: Vec<TranscriptJob> = Vec::new();
    let mut skipped = 0usize;
    let mut lines: Vec<String> = Vec::new();

    for (subtitle_uuid, media_title, media_source) in rows.iter() {
        let label = if media_title.trim().is_empty() {
            media_source.clone()
        } else {
            media_title.clone()
        };

        // Transcript mirrors the media sub-directory: source a/b.mp3 -> transcript/a/b.txt
        let transcript_path = transcript_dir.join(Path::new(media_source).with_extension("txt"));
        if !transcript_path.exists() {
            lines.push(format!("  {}: no transcript", label));
            skipped += 1;
            continue;
        }
        let text = fs::read_to_string(&transcript_path).map_err(|e| e.to_string())?;
        let refs = load_transcript_sentences(&text);
        if refs.is_empty() {
            lines.push(format!("  {}: empty transcript", label));
            skipped += 1;
            continue;
        }

        let cues = load_cues(&conn, subtitle_uuid)?;
        if cues.is_empty() {
            continue;
        }

        jobs.push(TranscriptJob {
            subtitle_uuid: subtitle_uuid.clone(),
            label,
            cues,
            refs,
        });
    }

    // Parallel compute phase: align every subtitle's cues against its own
    // transcript sentences across CPU-sized worker threads; workers emit progress.
    let labels: Vec<String> = jobs.iter().map(|j| j.label.clone()).collect();
    let matches_per_job: Vec<Vec<Match>> = {
        let jobs = &jobs;
        parallel_map(&app, &uuid, jobs.len(), &labels, move |i| {
            multi_pass_align(&jobs[i].cues, &jobs[i].refs)
        })
        .into_iter()
        .map(|(_, matches)| matches)
        .collect()
    };

    // Serial write phase: reset + rewrite each subtitle's references, one
    // transaction per subtitle so a failure can't wipe already-written media.
    let mut total_matches = 0usize;
    for (job, matches) in jobs.iter().zip(matches_per_job.into_iter()) {
        let anchors = matches
            .iter()
            .filter(|m| m.match_type == MatchType::Anchor)
            .count();
        let inferred = matches
            .iter()
            .filter(|m| m.match_type == MatchType::Inferred)
            .count();

        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE listen_subtitle_cue SET reference = NULL WHERE subtitle_uuid = ?1",
            rusqlite::params![job.subtitle_uuid],
        )
        .map_err(|e| e.to_string())?;
        for m in matches.iter() {
            tx.execute(
                "UPDATE listen_subtitle_cue SET reference = ?1 WHERE uuid = ?2",
                rusqlite::params![m.ref_text, m.cue.uuid],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;

        total_matches += matches.len();
        lines.push(format!(
            "  {}: {} cues, {} refs, {} matches ({} anchors, {} inferred)",
            job.label,
            job.cues.len(),
            job.refs.len(),
            matches.len(),
            anchors,
            inferred
        ));
    }

    drop(conn);
    touch_info(&dataset_dir);

    let aligned = jobs.len();
    log::info!("Transcript alignment complete: {} media aligned, {} skipped", aligned, skipped);
    let mut summary = String::new();
    summary.push_str(&lines.join("\n"));
    if !summary.is_empty() {
        summary.push('\n');
    }
    summary.push_str(&format!(
        "Done: {} total matches across {} media",
        total_matches, aligned
    ));
    if skipped > 0 {
        summary.push_str(&format!("\nSkipped {} media without matching transcripts", skipped));
    }
    Ok(summary)
}

/// Stage 4b (Book): align subtitle cues against the shared `book.txt` reference.
///
/// Reference sentences are loaded once and reused for every subtitle: the
/// `book_sentences.txt` cache (produced by "4a. Split Book", possibly via the
/// higher-quality NLTK engine) is used when present — one sentence per line —
/// otherwise `book.txt` is split with the built-in splitter. Uses the same
/// multi-pass anchor alignment as the transcript variant and, per subtitle,
/// resets `listen_subtitle_cue.reference` to NULL before rewriting the matched
/// cues in one transaction (so stale references from a previous book never
/// survive).
#[tauri::command]
pub async fn dataset_align_cues(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<String, String> {
    log::info!("dataset_align_cues (book): dataset={}", dataset_uuid);
    let dataset_dir = find_dataset_dir(&settings, &dataset_uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    let cache_path = dataset_dir.join("book_sentences.txt");
    let book_path = dataset_dir.join("book.txt");

    if !db_path.exists() {
        return Err("Database file not found. Please generate the database first.".into());
    }

    // Load the shared reference sentences: cache-first (already one per line),
    // else split book.txt with the built-in splitter.
    let sentences: Vec<String> = if cache_path.exists() {
        fs::read_to_string(&cache_path)
            .map_err(|e| format!("Failed to read book_sentences.txt: {}", e))?
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    } else if book_path.exists() {
        let text =
            fs::read_to_string(&book_path).map_err(|e| format!("Failed to read book.txt: {}", e))?;
        split_book_sentences(&text)
    } else {
        return Err("No book_sentences.txt or book.txt found in dataset.".into());
    };

    let refs = refs_from_sentences(sentences);
    if refs.is_empty() {
        return Err("No reference sentences found (book is empty).".into());
    }

    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Subtitles that have cues, joined with their media so the progress log can
    // name each media item.
    let rows: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT s.uuid, m.title FROM listen_subtitle s \
                 JOIN listen_media m ON s.media_uuid = m.uuid \
                 WHERE s.uuid IN (SELECT DISTINCT subtitle_uuid FROM listen_subtitle_cue) \
                 ORDER BY m.source",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };

    if rows.is_empty() {
        return Err("No subtitles found in database. Generate subtitles first.".into());
    }
    log::info!("Aligning {} subtitles against book reference", rows.len());
    // book's reference sentences are shared by every job (loaded once above).
    let mut jobs: Vec<BookJob> = Vec::new();
    for (subtitle_uuid, media_title) in rows.iter() {
        let cues = load_cues(&conn, subtitle_uuid)?;
        if cues.is_empty() {
            continue;
        }
        let label = if media_title.trim().is_empty() {
            subtitle_uuid[..8.min(subtitle_uuid.len())].to_string()
        } else {
            media_title.clone()
        };
        jobs.push(BookJob {
            subtitle_uuid: subtitle_uuid.clone(),
            label,
            cues,
        });
    }

    // Parallel compute phase: align every subtitle's cues against the shared book
    // references across CPU-sized worker threads; workers emit progress.
    let labels: Vec<String> = jobs.iter().map(|j| j.label.clone()).collect();
    let matches_per_job: Vec<Vec<Match>> = {
        let jobs = &jobs;
        let refs = &refs;
        parallel_map(&app, &dataset_uuid, jobs.len(), &labels, move |i| {
            multi_pass_align(&jobs[i].cues, refs)
        })
        .into_iter()
        .map(|(_, matches)| matches)
        .collect()
    };

    // Serial write phase: reset + rewrite each subtitle's references, one
    // transaction per subtitle so a failure can't wipe already-written media.
    let mut total_matches = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for (job, matches) in jobs.iter().zip(matches_per_job.into_iter()) {
        let anchors = matches
            .iter()
            .filter(|m| m.match_type == MatchType::Anchor)
            .count();
        let inferred = matches
            .iter()
            .filter(|m| m.match_type == MatchType::Inferred)
            .count();

        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE listen_subtitle_cue SET reference = NULL WHERE subtitle_uuid = ?1",
            rusqlite::params![job.subtitle_uuid],
        )
        .map_err(|e| e.to_string())?;
        for m in matches.iter() {
            tx.execute(
                "UPDATE listen_subtitle_cue SET reference = ?1 WHERE uuid = ?2",
                rusqlite::params![m.ref_text, m.cue.uuid],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;

        total_matches += matches.len();
        lines.push(format!(
            "  {}: {} cues, {} matches ({} anchors, {} inferred)",
            job.label,
            job.cues.len(),
            matches.len(),
            anchors,
            inferred
        ));
    }

    drop(conn);
    touch_info(&dataset_dir);

    let total = jobs.len();
    log::info!("Book alignment complete: {} media, {} total matches", total, total_matches);
    let mut summary = lines.join("\n");
    if !summary.is_empty() {
        summary.push('\n');
    }
    summary.push_str(&format!(
        "Done: {} total matches across {} media",
        total_matches,
        total
    ));
    Ok(summary)
}
