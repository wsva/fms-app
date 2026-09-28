//! Rust port of dataset_studio's cue-time adjustment.
//!
//! Ports `lib/adjust_session.py` (waveform loading, energy envelope, silence
//! detection, boundary snapping/expansion) and the batch driver from
//! `scripts/adjust_cue_time.py`. Reads the audiowaveform JSON only (no raw
//! audio decoding) and rewrites `listen_subtitle_cue` boundaries.

use std::fs;
use std::path::Path;

use chrono::Utc;
use rusqlite::Connection;
use tauri::{Emitter, State};
use uuid::Uuid;

use crate::dataset::{find_dataset_dir, parse_vtt, DatasetProgress};
use crate::model::ModelState;
use crate::settings::SettingsState;
use crate::align::similarity_score;

// ---------------------------------------------------------------------------
// Configuration (mirrors lib/adjust_session.py)
// ---------------------------------------------------------------------------

const SILENCE_THRESHOLD_FACTOR: f64 = 4.0;
const MIN_SILENCE_MS: f64 = 300.0;
const MAX_SNAP_MS: i64 = 500;
const MAX_SNAP_GAP_MS: i64 = 2000;
const EXPAND_THRESHOLD_MS: i64 = 500;
const EXPAND_MS: i64 = 500;
const SPEECH_ENERGY_FRACTION: f64 = 0.10;
const EXPAND_SEARCH_MS: i64 = 500;

const TMP_SUBTITLE_NAME: &str = "tmp_for_adjust_time";

// Word-level alignment configuration
const WL_SKIP_STT: f64 = -15.0;       // Penalty for skipping an STT word (hallucination)
const WL_SKIP_CUE: f64 = -40.0;       // Penalty for skipping a cue word (STT missed it)
const WL_MATCH_THRESHOLD: f64 = 50.0; // Min match % to accept new timestamps

/// Format milliseconds as HH:MM:SS.mmm
fn ms_to_timestamp(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.unsigned_abs();
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1_000;
    let milli = ms % 1_000;
    format!("{}{:02}:{:02}:{:02}.{:03}", sign, h, m, s, milli)
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Cue {
    #[allow(dead_code)]
    uuid: String,
    order_num: i64,
    start_ms: i64,
    end_ms: i64,
    content: String,
    reference: Option<String>,
}

// ---------------------------------------------------------------------------
// Waveform loading
// ---------------------------------------------------------------------------

/// Load waveform JSON for a media source. Returns (data, ms_per_pixel).
fn load_waveform(waveform_dir: &Path, media_source: &str) -> Option<(Vec<i64>, f64)> {
    // `media_source` is the media-relative path; the waveform mirrors it under waveform/.
    let wf_path = waveform_dir.join(Path::new(media_source).with_extension("json"));
    if !wf_path.exists() {
        return None;
    }
    let text = fs::read_to_string(&wf_path).ok()?;
    let obj: serde_json::Value = serde_json::from_str(&text).ok()?;

    let data: Vec<i64> = obj
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_i64())
        .collect();
    let sample_rate = obj.get("sample_rate").and_then(|v| v.as_i64()).unwrap_or(16000);
    let samples_per_pixel = obj
        .get("samples_per_pixel")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    if samples_per_pixel <= 0 || sample_rate <= 0 {
        return None;
    }
    let ms_per_pixel = (samples_per_pixel as f64 / sample_rate as f64) * 1000.0;
    Some((data, ms_per_pixel))
}

// ---------------------------------------------------------------------------
// Energy envelope & silence detection
// ---------------------------------------------------------------------------

/// Peak-to-peak amplitude per pixel from alternating (min, max) envelope pairs.
fn compute_energy(envelope: &[i64]) -> Vec<f64> {
    let n_pairs = envelope.len() / 2;
    let mut energy = Vec::with_capacity(n_pairs);
    for i in 0..n_pairs {
        let lo = envelope[2 * i];
        let hi = envelope[2 * i + 1];
        let amp = if hi >= lo { hi - lo } else { lo - hi };
        energy.push(amp as f64);
    }
    energy
}

/// Background noise level estimated from the quietest 10% of frames.
fn estimate_noise_floor(energy: &[f64]) -> f64 {
    if energy.is_empty() {
        return 1.0;
    }
    let mut sorted = energy.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = (sorted.len() / 10).max(1);
    let avg = sorted[..n].iter().sum::<f64>() / n as f64;
    avg.max(1.0)
}

/// Detect silence regions as (start_ms, end_ms) pairs.
fn find_silence_regions(energy: &[f64], ms_per_pixel: f64) -> Vec<(i64, i64)> {
    if energy.is_empty() {
        return Vec::new();
    }
    let noise_floor = estimate_noise_floor(energy);
    let threshold = noise_floor * SILENCE_THRESHOLD_FACTOR;

    let mut regions: Vec<(usize, usize)> = Vec::new();
    let mut run_start: Option<usize> = None;
    for (i, e) in energy.iter().enumerate() {
        let silent = *e < threshold;
        if silent && run_start.is_none() {
            run_start = Some(i);
        } else if !silent && run_start.is_some() {
            let rs = run_start.unwrap();
            let duration_ms = ((i - rs) as f64) * ms_per_pixel;
            if duration_ms >= MIN_SILENCE_MS {
                regions.push((rs, i - 1));
            }
            run_start = None;
        }
    }
    if let Some(rs) = run_start {
        let duration_ms = ((energy.len() - rs) as f64) * ms_per_pixel;
        if duration_ms >= MIN_SILENCE_MS {
            regions.push((rs, energy.len() - 1));
        }
    }
    regions
        .into_iter()
        .map(|(s, e)| {
            (
                (s as f64 * ms_per_pixel) as i64,
                (e as f64 * ms_per_pixel) as i64,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Boundary snapping & expansion
// ---------------------------------------------------------------------------

fn snap_start(original_ms: i64, silences: &[(i64, i64)]) -> i64 {
    let mut silence: Option<(i64, i64)> = None;
    for &(s_start, s_end) in silences {
        if s_start <= original_ms {
            if silence.is_none() || s_start > silence.unwrap().0 {
                silence = Some((s_start, s_end));
            }
        }
    }
    let Some((s_start, s_end)) = silence else {
        return original_ms;
    };
    if original_ms - s_end > MAX_SNAP_GAP_MS {
        return original_ms;
    }
    ((s_start + s_end) / 2).max(s_end - 1000) + 1
}

fn snap_end(original_ms: i64, silences: &[(i64, i64)]) -> i64 {
    let mut silence: Option<(i64, i64)> = None;
    for &(s_start, s_end) in silences {
        if s_end > original_ms {
            if silence.is_none() || s_end < silence.unwrap().1 {
                silence = Some((s_start, s_end));
            }
        }
    }
    let Some((s_start, s_end)) = silence else {
        return original_ms;
    };
    if s_start - original_ms > MAX_SNAP_GAP_MS {
        return original_ms;
    }
    ((s_start + s_end) / 2).min(s_start + 1000)
}

fn expand_start(snapped_ms: i64, silences: &[(i64, i64)], energy: &[f64], ms_per_pixel: f64) -> i64 {
    let mut prev_silence: Option<(i64, i64)> = None;
    let mut best_gap = MAX_SNAP_MS;
    for &(s_start, s_end) in silences {
        if s_end <= snapped_ms {
            let gap = snapped_ms - s_end;
            if gap <= best_gap {
                prev_silence = Some((s_start, s_end));
                best_gap = gap;
            }
        }
    }
    let Some(prev) = prev_silence else {
        return snapped_ms;
    };

    let mut target: Option<(i64, i64)> = None;
    let mut best_gap2 = MAX_SNAP_MS;
    for &(s_start, s_end) in silences {
        if s_end <= prev.0 {
            let gap = prev.0 - s_end;
            if gap <= best_gap2 {
                target = Some((s_start, s_end));
                best_gap2 = gap;
            }
        }
    }
    let Some((s_start, s_end)) = target else {
        return snapped_ms;
    };
    if s_end - s_start < EXPAND_THRESHOLD_MS {
        return snapped_ms;
    }

    if ms_per_pixel > 0.0 && !energy.is_empty() {
        let peak = energy.iter().cloned().fold(1.0f64, f64::max);
        let speech_threshold = peak * SPEECH_ENERGY_FRACTION;
        let margin_start_ms = (prev.0 - EXPAND_SEARCH_MS).max(0);
        let mut px0 = (margin_start_ms as f64 / ms_per_pixel) as usize;
        let mut px1 = (prev.0 as f64 / ms_per_pixel) as usize;
        px0 = px0.min(energy.len());
        px1 = px1.min(energy.len());
        if px1 > px0 {
            let avg = energy[px0..px1].iter().sum::<f64>() / (px1 - px0) as f64;
            if avg < speech_threshold {
                return snapped_ms;
            }
        }
    }
    s_start.max(snapped_ms - EXPAND_MS)
}

fn expand_end(snapped_ms: i64, silences: &[(i64, i64)], energy: &[f64], ms_per_pixel: f64) -> i64 {
    let mut next_silence: Option<(i64, i64)> = None;
    let mut best_gap = MAX_SNAP_MS;
    for &(s_start, s_end) in silences {
        if s_start >= snapped_ms {
            let gap = s_start - snapped_ms;
            if gap <= best_gap {
                next_silence = Some((s_start, s_end));
                best_gap = gap;
            }
        }
    }
    let Some(next) = next_silence else {
        return snapped_ms;
    };

    let mut target: Option<(i64, i64)> = None;
    let mut best_gap2 = MAX_SNAP_MS;
    for &(s_start, s_end) in silences {
        if s_start >= next.1 {
            let gap = s_start - next.1;
            if gap <= best_gap2 {
                target = Some((s_start, s_end));
                best_gap2 = gap;
            }
        }
    }
    let Some((s_start, s_end)) = target else {
        return snapped_ms;
    };
    if s_end - s_start < EXPAND_THRESHOLD_MS {
        return snapped_ms;
    }

    if ms_per_pixel > 0.0 && !energy.is_empty() {
        let peak = energy.iter().cloned().fold(1.0f64, f64::max);
        let speech_threshold = peak * SPEECH_ENERGY_FRACTION;
        let margin_end_ms = next.1 + EXPAND_SEARCH_MS;
        let mut px0 = (next.1 as f64 / ms_per_pixel) as usize;
        let mut px1 = (margin_end_ms as f64 / ms_per_pixel) as usize;
        px0 = px0.min(energy.len());
        px1 = px1.min(energy.len());
        if px1 > px0 {
            let avg = energy[px0..px1].iter().sum::<f64>() / (px1 - px0) as f64;
            if avg < speech_threshold {
                return snapped_ms;
            }
        }
    }
    s_end.min(snapped_ms + EXPAND_MS)
}

/// Adjust all cue boundaries, preventing overlaps. Returns (cue, new_start, new_end).
fn adjust_cues(
    cues: &[Cue],
    silences: &[(i64, i64)],
    energy: &[f64],
    ms_per_pixel: f64,
) -> Vec<(Cue, i64, i64)> {
    let mut adjusted: Vec<(Cue, i64, i64)> = Vec::new();
    for cue in cues {
        let mut new_start = snap_start(cue.start_ms, silences);
        let mut new_end = snap_end(cue.end_ms, silences);
        new_start = expand_start(new_start, silences, energy, ms_per_pixel);
        new_end = expand_end(new_end, silences, energy, ms_per_pixel);

        if new_end - new_start < 50 {
            new_start = cue.start_ms;
            new_end = cue.end_ms;
        }
        if let Some((_, _, prev_end)) = adjusted.last() {
            if new_start < *prev_end {
                new_start = *prev_end;
            }
        }
        if new_start >= new_end {
            new_start = cue.start_ms;
            new_end = cue.end_ms;
        }
        adjusted.push((cue.clone(), new_start, new_end));
    }
    adjusted
}

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

/// Adjust every subtitle's cue boundaries using waveform silence detection.
/// `mode` is "new" (write an adjusted copy per subtitle) or "in_place"
/// (overwrite original cues and remove leftover tmp working subtitles).
#[tauri::command]
pub async fn dataset_adjust_cue_time(
    settings: State<'_, SettingsState>,
    uuid: String,
    mode: String,
) -> Result<String, String> {
    log::info!("dataset_adjust_cue_time: dataset={}, mode={}", uuid, mode);
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Please build the database first.".into());
    }
    let waveform_dir = dataset_dir.join("waveform");
    let in_place = mode == "in_place";

    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // All subtitles except the tmp working subtitle.
    let subtitles: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT uuid, media_uuid, name FROM listen_subtitle \
                 WHERE name != ?1 ORDER BY media_uuid, created_at",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(rusqlite::params![TMP_SUBTITLE_NAME], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };

    let mut processed = 0usize;
    let mut shifted_total = 0usize;
    let mut skipped = 0usize;
    let mut log_lines: Vec<String> = Vec::new();

    log::info!("Processing {} subtitles for cue time adjustment", subtitles.len());

    for (subtitle_uuid, media_uuid, name) in subtitles.iter() {
        // Resolve the media source path.
        let source: Option<String> = conn
            .query_row(
                "SELECT source FROM listen_media WHERE uuid = ?1",
                rusqlite::params![media_uuid],
                |row| row.get::<_, String>(0),
            )
            .ok();
        let Some(source) = source else {
            skipped += 1;
            continue;
        };

        let Some((data, ms_per_pixel)) = load_waveform(&waveform_dir, &source) else {
            skipped += 1;
            continue;
        };
        let energy = compute_energy(&data);
        let silences = find_silence_regions(&energy, ms_per_pixel);

        // Load cues for this subtitle.
        let cues: Vec<Cue> = {
            let mut stmt = conn
                .prepare(
                    "SELECT uuid, order_num, start_ms, end_ms, content, reference \
                     FROM listen_subtitle_cue WHERE subtitle_uuid = ?1 ORDER BY order_num",
                )
                .map_err(|e| e.to_string())?;
            let mapped = stmt
                .query_map(rusqlite::params![subtitle_uuid], |row| {
                    Ok(Cue {
                        uuid: row.get::<_, String>(0)?,
                        order_num: row.get::<_, i64>(1)?,
                        start_ms: row.get::<_, i64>(2)?,
                        end_ms: row.get::<_, i64>(3)?,
                        content: row.get::<_, String>(4)?,
                        reference: row.get::<_, Option<String>>(5)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            mapped.filter_map(|r| r.ok()).collect()
        };
        if cues.is_empty() {
            skipped += 1;
            continue;
        }

        let adjusted = adjust_cues(&cues, &silences, &energy, ms_per_pixel);
        let shifted_items: Vec<&(Cue, i64, i64)> = adjusted
            .iter()
            .filter(|(c, ns, ne)| *ns != c.start_ms || *ne != c.end_ms)
            .collect();
        let shifted = shifted_items.len();

        log_lines.push(format!(
            "Subtitle '{}' ({} cues, {} shifted, {} silence regions):",
            name, cues.len(), shifted, silences.len()
        ));
        for (cue, new_start, new_end) in &shifted_items {
            log_lines.push(format!(
                "  cue {:>3}  {} -> {}  {} -> {}  \"{}\"",
                cue.order_num,
                ms_to_timestamp(cue.start_ms),
                ms_to_timestamp(*new_start),
                ms_to_timestamp(cue.end_ms),
                ms_to_timestamp(*new_end),
                if cue.content.len() > 50 {
                    format!("{}...", &cue.content[..50])
                } else {
                    cue.content.clone()
                }
            ));
        }

        let now = Utc::now().to_rfc3339();
        let target_uuid = if in_place {
            conn.execute(
                "DELETE FROM listen_subtitle_cue WHERE subtitle_uuid = ?1",
                rusqlite::params![subtitle_uuid],
            )
            .map_err(|e| e.to_string())?;
            subtitle_uuid.clone()
        } else {
            let new_uuid = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO listen_subtitle (uuid, media_uuid, name, note, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    new_uuid,
                    media_uuid,
                    name,
                    "adjusted using waveform",
                    now,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
            new_uuid
        };

        for (cue, new_start, new_end) in adjusted.iter() {
            conn.execute(
                "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference, version_created) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    target_uuid,
                    cue.order_num,
                    new_start,
                    new_end,
                    cue.content,
                    cue.reference
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        if in_place {
            conn.execute(
                "UPDATE listen_subtitle SET updated_at = ?1 WHERE uuid = ?2",
                rusqlite::params![now, subtitle_uuid],
            )
            .map_err(|e| e.to_string())?;
        }

        processed += 1;
        shifted_total += shifted;
    }

    // In-place runs finalise the dataset: remove leftover tmp working subtitles.
    let mut tmp_deleted = 0usize;
    if in_place {
        tmp_deleted = conn
            .query_row(
                "SELECT COUNT(*) FROM listen_subtitle WHERE name = ?1",
                rusqlite::params![TMP_SUBTITLE_NAME],
                |row| row.get::<_, usize>(0),
            )
            .unwrap_or(0);
        if tmp_deleted > 0 {
            conn.execute(
                "DELETE FROM listen_subtitle_cue WHERE subtitle_uuid IN \
                 (SELECT uuid FROM listen_subtitle WHERE name = ?1)",
                rusqlite::params![TMP_SUBTITLE_NAME],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "DELETE FROM listen_subtitle WHERE name = ?1",
                rusqlite::params![TMP_SUBTITLE_NAME],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    drop(conn);

    log::info!("Adjust complete: {} subtitles processed, {} cues shifted", processed, shifted_total);
    let mut summary = String::new();
    // Append detailed log lines
    for line in &log_lines {
        summary.push_str(line);
        summary.push('\n');
    }
    summary.push_str(&format!(
        "\nAdjusted {} subtitle(s), {} cue(s) shifted ({} mode)",
        processed,
        shifted_total,
        if in_place { "in-place" } else { "new" }
    ));
    if skipped > 0 {
        summary.push_str(&format!("\nSkipped {} subtitle(s) without a waveform or cues", skipped));
    }
    if in_place && tmp_deleted > 0 {
        summary.push_str(&format!("\nRemoved {} leftover tmp working subtitle(s)", tmp_deleted));
    }
    // Touch info.json timestamp (best-effort).
    let info_path = dataset_dir.join("info.json");
    if let Ok(data) = fs::read_to_string(&info_path) {
        if let Ok(mut info) = serde_json::from_str::<crate::dataset::DatasetInfo>(&data) {
            info.updated = Utc::now().to_rfc3339();
            if let Ok(out) = serde_json::to_string_pretty(&info) {
                let _ = fs::write(&info_path, out);
            }
        }
    }
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Word-level cue time sync (Parakeet word timestamps → DP alignment)
// ---------------------------------------------------------------------------

/// A tokenized word with its character offset range in the original text.
struct TokenWord {
    word: String,
    #[allow(dead_code)]
    char_start: usize,
    #[allow(dead_code)]
    char_end: usize,
}

/// Tokenize text into lowercase words with character offsets.
fn tokenize_words(text: &str) -> Vec<TokenWord> {
    let mut words = Vec::new();
    let mut word_start: Option<usize> = None;

    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            if word_start.is_none() {
                word_start = Some(i);
            }
        } else if let Some(start) = word_start {
            let word: String = text[start..i]
                .chars()
                .flat_map(|ch| ch.to_lowercase())
                .collect();
            words.push(TokenWord {
                word,
                char_start: start,
                char_end: i,
            });
            word_start = None;
        }
    }
    // Handle word at end of string
    if let Some(start) = word_start {
        let end = text.len();
        let word: String = text[start..end]
            .chars()
            .flat_map(|ch| ch.to_lowercase())
            .collect();
        words.push(TokenWord {
            word,
            char_start: start,
            char_end: end,
        });
    }
    words
}

/// Word similarity score 0–100 using Ratcliff-Obershelp (via `similarity_score`).
fn word_match_score(a: &str, b: &str) -> f64 {
    similarity_score(a, b)
}

/// DP-align cue words against STT words to find the best alignment.
///
/// Transitions:
/// - **Match** (diagonal): cue_word[i] ↔ stt_word[j], score = word_match_score
/// - **Skip STT** (left): STT word is a hallucination, penalty = WL_SKIP_STT
/// - **Skip cue** (up): cue word was not recognized, penalty = WL_SKIP_CUE
///
/// Returns the list of (cue_word_index, stt_word_index) matches.
fn align_cue_words_dp(
    cue_words: &[TokenWord],
    stt_words: &[transcribe_rs::TranscriptionSegment],
) -> Vec<(usize, usize)> {
    let n = cue_words.len();
    let m = stt_words.len();
    if n == 0 || m == 0 {
        return Vec::new();
    }

    // Normalize STT words: strip non-alphanumeric chars and lowercase,
    // matching the same normalization that tokenize_words does for cue words.
    let stt_clean: Vec<String> = stt_words
        .iter()
        .map(|seg| {
            seg.text
                .chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect()
        })
        .collect();

    // Debug: show cleaned STT words and first few match scores.
    // println!("  [DP] stt_clean first 10:");
    // for (i, w) in stt_clean.iter().enumerate().take(10) {
    //     println!("    [{}] raw='{}' clean='{}'", i, stt_words[i].text, w);
    // }
    // println!("  [DP] cue_words first 10:");
    // for (i, w) in cue_words.iter().enumerate().take(10) {
    //     println!("    [{}] '{}'", i, w.word);
    // }
    // // Show match score matrix for first few pairs.
    // println!("  [DP] match scores (first 5x5):");
    // for i in 0..cue_words.len().min(5) {
    //     let mut row = format!("    cue[{}] '{}':", i, cue_words[i].word);
    //     for j in 0..stt_clean.len().min(10) {
    //         let score = word_match_score(&cue_words[i].word, &stt_clean[j]);
    //         row.push_str(&format!(" stt[{}]='{}'={:.0}", j, stt_clean[j], score));
    //     }
    //     println!("{}", row);
    // }

    // Flat DP table for performance (row-major: dp[i * (m+1) + j]).
    let stride = m + 1;
    let mut dp = vec![f64::NEG_INFINITY; (n + 1) * stride];
    // Direction: 0 = skip cue (came from above), 1 = skip STT (from left),
    //            2 = match (from diagonal).
    let mut dir = vec![0u8; (n + 1) * stride];

    dp[0] = 0.0;
    // First row: skip STT words before any cue word.
    for j in 1..=m {
        dp[j] = dp[j - 1] + WL_SKIP_STT;
        dir[j] = 1;
    }
    // First column: skip cue words before any STT word.
    for i in 1..=n {
        dp[i * stride] = dp[(i - 1) * stride] + WL_SKIP_CUE;
        dir[i * stride] = 0;
    }

    // Fill the DP table.
    for i in 1..=n {
        let ms = word_match_score(&cue_words[i - 1].word, &stt_clean[0]);
        let diag = dp[(i - 1) * stride] + ms;
        let up = dp[(i - 1) * stride] + WL_SKIP_CUE;
        let left = dp[i * stride - 1] + WL_SKIP_STT;
        let best = diag.max(up).max(left);
        dp[i * stride + 1] = best;
        dir[i * stride + 1] = if best == diag { 2 } else if best == up { 0 } else { 1 };

        for j in 2..=m {
            let ms = word_match_score(&cue_words[i - 1].word, &stt_clean[j - 1]);
            let diag = dp[(i - 1) * stride + j - 1] + ms;
            let up = dp[(i - 1) * stride + j] + WL_SKIP_CUE;
            let left = dp[i * stride + j - 1] + WL_SKIP_STT;
            let best = diag.max(up).max(left);
            dp[i * stride + j] = best;
            dir[i * stride + j] = if best == diag {
                2
            } else if best == up {
                0
            } else {
                1
            };
        }
    }

    // Backtrack to recover the alignment path.
    let mut matches = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        match dir[i * stride + j] {
            2 => {
                // Match: both indices consumed.
                matches.push((i - 1, j - 1));
                i -= 1;
                j -= 1;
            }
            0 => {
                // Skip cue word.
                i -= 1;
            }
            _ => {
                // Skip STT word.
                j -= 1;
            }
        }
    }
    matches.reverse();

    // Debug: show first/last matches.
    // if !matches.is_empty() {
    //     println!("  [DP] {} total matches. First 5:", matches.len());
    //     for idx in 0..matches.len().min(5) {
    //         let pair = matches[idx];
    //         let ci: usize = pair.0;
    //         let si: usize = pair.1;
    //         println!("    cue[{}]='{}' <-> stt[{}]='{}'", ci, cue_words[ci].word, si, stt_clean[si]);
    //     }
    //     if matches.len() > 5 {
    //         println!("    ... last 5:");
    //         let start = matches.len() - 5;
    //         for idx in start..matches.len() {
    //             let pair = matches[idx];
    //             let ci: usize = pair.0;
    //             let si: usize = pair.1;
    //             println!("    cue[{}]='{}' <-> stt[{}]='{}'", ci, cue_words[ci].word, si, stt_clean[si]);
    //         }
    //     }
    // } else {
    //     println!("  [DP] ZERO matches found!");
    // }

    matches
}

/// Sync cue timestamps using word-level STT transcription (Parakeet only).
///
/// For each media in the dataset:
/// 1. Transcribe with `TimestampGranularity::Word` to get per-word timestamps
/// 2. Tokenize each cue's text into words
/// 3. DP-align cue words against STT words (tolerates missing/extra words)
/// 4. Use matched word boundaries as new cue start/end timestamps
/// 5. Cues with < 50% match ratio are flagged as unmatched
#[tauri::command]
pub async fn dataset_sync_cue_times_word_level(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    model_state: State<'_, ModelState>,
    uuid: String,
    media_uuid: Option<String>,
) -> Result<String, String> {
    log::info!("dataset_sync_cue_times_word_level: dataset={}, media_uuid={:?}", uuid, media_uuid);
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Please build the database first.".into());
    }

    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Load subtitles (excluding tmp working subtitles).
    // When media_uuid is provided, only load subtitles for that specific media.
    let subtitles: Vec<(String, String, String)> = {
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(ref mid) = media_uuid {
            (
                "SELECT uuid, media_uuid, name FROM listen_subtitle \
                 WHERE name != ?1 AND media_uuid = ?2 ORDER BY media_uuid, created_at",
                vec![Box::new(TMP_SUBTITLE_NAME.to_string()), Box::new(mid.clone())],
            )
        } else {
            (
                "SELECT uuid, media_uuid, name FROM listen_subtitle \
                 WHERE name != ?1 ORDER BY media_uuid, created_at",
                vec![Box::new(TMP_SUBTITLE_NAME.to_string())],
            )
        };
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let mapped = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };

    let total_subs = subtitles.len();
    let mut total_matched = 0usize;
    let mut total_unmatched = 0usize;
    let mut total_shifted = 0usize;
    let mut log_lines: Vec<String> = Vec::new();

    for (sub_idx, (subtitle_uuid, media_uuid, subtitle_name)) in subtitles.iter().enumerate() {
        // Resolve media source path.
        let source: Option<String> = conn
            .query_row(
                "SELECT source FROM listen_media WHERE uuid = ?1",
                rusqlite::params![media_uuid],
                |row| row.get::<_, String>(0),
            )
            .ok();
        let Some(source) = source else {
            continue;
        };

        let media_path = dataset_dir.join("media").join(&source);
        if !media_path.exists() {
            log_lines.push(format!("Skipping '{}': media file not found", subtitle_name));
            continue;
        }

        // Emit progress.
        let _ = app.emit(
            "dataset-progress",
            DatasetProgress {
                uuid: uuid.clone(),
                current_file: subtitle_name.clone(),
                file_index: sub_idx + 1,
                total_files: total_subs,
                stage: "word-align".into(),
            },
        );

        // Transcribe with word-level timestamps.
        let word_result =
            crate::model::transcribe_file_word_level(&model_state, &media_path)?;
        let stt_words = match word_result.segments {
            Some(segs) if !segs.is_empty() => segs,
            _ => {
                log_lines.push(format!(
                    "'{}': no word-level segments returned",
                    subtitle_name
                ));
                continue;
            }
        };

        // DEBUG: Save STT words to temp file.
        // {
        //     let stt_dump_path = std::env::temp_dir().join("fms_wl_stt_words.txt");
        //     let mut stt_dump = String::new();
        //     stt_dump.push_str(&format!("# STT word-level transcription for media: {}\n", media_uuid));
        //     stt_dump.push_str(&format!("# Subtitle: {}\n", subtitle_name));
        //     stt_dump.push_str(&format!("# Total words: {}\n\n", stt_words.len()));
        //     for (i, w) in stt_words.iter().enumerate() {
        //         stt_dump.push_str(&format!("[{}] {}\t{:.3}-{:.3}\n", i, w.text, w.start, w.end));
        //     }
        //     let _ = std::fs::write(&stt_dump_path, &stt_dump);
        //     println!("[DEBUG] STT words saved to: {}", stt_dump_path.display());
        // }

        // Load existing DB cues.
        let db_cues: Vec<(String, i64, i64, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT uuid, start_ms, end_ms, content \
                     FROM listen_subtitle_cue WHERE subtitle_uuid = ?1 ORDER BY order_num",
                )
                .map_err(|e| e.to_string())?;
            let mapped = stmt
                .query_map(rusqlite::params![subtitle_uuid], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            mapped.filter_map(|r| r.ok()).collect()
        };
        if db_cues.is_empty() {
            continue;
        }

        // DEBUG: Save DB cues to temp file.
        // {
        //     let cue_dump_path = std::env::temp_dir().join("fms_wl_db_cues.txt");
        //     let mut cue_dump = String::new();
        //     cue_dump.push_str(&format!("# DB cues for subtitle: {}\n", subtitle_name));
        //     cue_dump.push_str(&format!("# Total cues: {}\n\n", db_cues.len()));
        //     for (i, (uuid, start, end, content)) in db_cues.iter().enumerate() {
        //         cue_dump.push_str(&format!("[{}] {}\t{}-{}\t\"{}\"\n", i, uuid, start, end, content));
        //     }
        //     let _ = std::fs::write(&cue_dump_path, &cue_dump);
        //     println!("[DEBUG] DB cues saved to: {}", cue_dump_path.display());
        // }

        // Tokenize all cue words, tracking which cue each word belongs to.
        let mut all_cue_words: Vec<TokenWord> = Vec::new();
        let mut word_to_cue: Vec<usize> = Vec::new(); // cue index per word
        for (cue_idx, (_, _, _, content)) in db_cues.iter().enumerate() {
            let words = tokenize_words(content);
            for w in words {
                word_to_cue.push(cue_idx);
                all_cue_words.push(w);
            }
        }

        if all_cue_words.is_empty() {
            continue;
        }

        // Dump ALL STT words one per line (no truncation).
        log_lines.push(format!(
            "'{}': {} DB cues ({} words), {} STT words",
            subtitle_name,
            db_cues.len(),
            all_cue_words.len(),
            stt_words.len()
        ));
        // log_lines.push(format!("  STT words ({}):", stt_words.len()));
        // println!("'{}': {} DB cues ({} words), {} STT words", subtitle_name, db_cues.len(), all_cue_words.len(), stt_words.len());
        // println!("  STT words ({}):", stt_words.len());
        // for (i, w) in stt_words.iter().enumerate() {
        //     let line = format!("    [{}] {}  {:.2}-{:.2}", i, w.text, w.start, w.end);
        //     log_lines.push(line.clone());
        //     println!("{}", line);
        // }
        // // Dump ALL cue words one per line.
        // log_lines.push(format!("  Cue words ({}):", all_cue_words.len()));
        // println!("  Cue words ({}):", all_cue_words.len());
        // for (i, w) in all_cue_words.iter().enumerate() {
        //     let line = format!("    [{}] {} (c{})", i, w.word, word_to_cue[i]);
        //     log_lines.push(line.clone());
        //     println!("{}", line);
        // }

        // DP-align all cue words against all STT words.
        let dp_matches = align_cue_words_dp(&all_cue_words, &stt_words);
        // Dump every DP match pair.
        // let dp_detail: Vec<String> = dp_matches
        //     .iter()
        //     .map(|&(ci, si)| format!(
        //         "{}(cue[{}])<->{}(stt[{}])",
        //         all_cue_words[ci].word, ci, stt_words[si].text, si
        //     ))
        //     .collect();
        // log_lines.push(format!("  DP alignment: {} word matches: {}", dp_matches.len(), dp_detail.join(", ")));
        // println!("  DP alignment: {} word matches: {}", dp_matches.len(), dp_detail.join(", "));

        // Group matches by cue: find first/last STT index per cue.
        let mut cue_first_stt: Vec<Option<usize>> = vec![None; db_cues.len()];
        let mut cue_last_stt: Vec<Option<usize>> = vec![None; db_cues.len()];
        let mut cue_matched_words: Vec<usize> = vec![0; db_cues.len()];

        for &(cw_idx, stt_idx) in &dp_matches {
            let ci = word_to_cue[cw_idx];
            cue_matched_words[ci] += 1;
            cue_first_stt[ci] = Some(
                cue_first_stt[ci]
                    .map_or(stt_idx, |prev: usize| prev.min(stt_idx)),
            );
            cue_last_stt[ci] = Some(
                cue_last_stt[ci]
                    .map_or(stt_idx, |prev: usize| prev.max(stt_idx)),
            );
        }

        // Count total cue words per cue for match-ratio calculation.
        let mut cue_word_counts: Vec<usize> = vec![0; db_cues.len()];
        for &ci in &word_to_cue {
            cue_word_counts[ci] += 1;
        }

        let mut matched_count = 0usize;
        let mut unmatched_count = 0usize;
        let mut shifted_count = 0usize;

        for (cue_idx, (cue_uuid, old_start, old_end, content)) in
            db_cues.iter().enumerate()
        {
            let total_words = cue_word_counts[cue_idx];
            let matched_words = cue_matched_words[cue_idx];
            let match_ratio = if total_words > 0 {
                (matched_words as f64 / total_words as f64) * 100.0
            } else {
                0.0
            };

            if match_ratio >= WL_MATCH_THRESHOLD {
                if let (Some(first), Some(last)) = (cue_first_stt[cue_idx], cue_last_stt[cue_idx])
                {
                    let new_start = (stt_words[first].start * 1000.0) as i64;
                    let new_end = (stt_words[last].end * 1000.0) as i64;

                    // Ensure minimum duration and non-inverted range.
                    let (new_start, new_end) = if new_end <= new_start {
                        (*old_start, *old_end)
                    } else {
                        (new_start, new_end.max(new_start + 50))
                    };

                    conn.execute(
                        "UPDATE listen_subtitle_cue SET start_ms = ?1, end_ms = ?2 WHERE uuid = ?3",
                        rusqlite::params![new_start, new_end, cue_uuid],
                    )
                    .map_err(|e| e.to_string())?;

                    matched_count += 1;
                    if new_start != *old_start || new_end != *old_end {
                        shifted_count += 1;
                    }
                    // Show which STT words were matched (full, no truncation).
                    let matched_stt_words: Vec<String> = stt_words[first..=last]
                        .iter()
                        .map(|w| format!("{}:{:.2}-{:.2}", w.text, w.start, w.end))
                        .collect();
                    let stt_text = matched_stt_words.join(" ");
                    // Show which cue words matched which STT words for this cue.
                    let cue_match_detail: Vec<String> = dp_matches
                        .iter()
                        .filter(|&&(ci, _)| word_to_cue[ci] == cue_idx)
                        .map(|&(ci, si)| format!("{}<->{}", all_cue_words[ci].word, stt_words[si].text))
                        .collect();
                    let shift_marker = if new_start != *old_start || new_end != *old_end {
                        ""
                    } else {
                        " (no change)"
                    };
                    let cue_log = format!(
                        "  CUE[{}] {} -> {}  {} -> {}  ({}/{} words){} \"{}\"  STT: [{}]  matches: [{}]",
                        cue_idx,
                        ms_to_timestamp(*old_start),
                        ms_to_timestamp(new_start),
                        ms_to_timestamp(*old_end),
                        ms_to_timestamp(new_end),
                        matched_words,
                        total_words,
                        shift_marker,
                        content,
                        stt_text,
                        cue_match_detail.join(", ")
                    );
                    log_lines.push(cue_log.clone());
                    println!("{}", cue_log);
                }
            } else {
                unmatched_count += 1;
                // Show full cue content and which words (if any) were matched.
                let cue_words_detail: Vec<String> = {
                    let start = word_to_cue.iter().position(|&c| c == cue_idx).unwrap_or(0);
                    let end = word_to_cue.iter().rposition(|&c| c == cue_idx).map_or(0, |p| p + 1);
                    all_cue_words[start..end].iter().map(|w| w.word.clone()).collect()
                };
                let matched_stt_indices: Vec<usize> = dp_matches
                    .iter()
                    .filter(|&&(ci, _)| word_to_cue[ci] == cue_idx)
                    .map(|&(_, si)| si)
                    .collect();
                let matched_stt_detail: Vec<String> = matched_stt_indices
                    .iter()
                    .map(|&si| format!("{}[{}]", stt_words[si].text, si))
                    .collect();
                let log_line = format!(
                    "  UNMATCHED ({}/{} words, {:.0}%) cue_words=[{}] matched_stt=[{}] \"{}\"",
                    matched_words, total_words, match_ratio,
                    cue_words_detail.join(", "),
                    matched_stt_detail.join(", "),
                    content
                );
                log_lines.push(log_line.clone());
                println!("{}", log_line);
            }
        }

        // Update subtitle timestamp.
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE listen_subtitle SET updated_at = ?1 WHERE uuid = ?2",
            rusqlite::params![now, subtitle_uuid],
        )
        .map_err(|e| e.to_string())?;

        total_matched += matched_count;
        total_unmatched += unmatched_count;
        total_shifted += shifted_count;

        // Per-media summary line.
        log_lines.push(format!(
            "  Summary: {} matched ({} shifted), {} unmatched out of {} cues",
            matched_count, shifted_count, unmatched_count, db_cues.len()
        ));
        println!("  Summary: {} matched ({} shifted), {} unmatched out of {} cues", matched_count, shifted_count, unmatched_count, db_cues.len());
    }

    drop(conn);

    log::info!(
        "Word-level sync complete: {} matched, {} unmatched, {} shifted",
        total_matched,
        total_unmatched,
        total_shifted
    );
    println!("\nWord-level sync complete: {} matched, {} unmatched, {} shifted", total_matched, total_unmatched, total_shifted);

    let mut summary = String::new();
    for line in &log_lines {
        summary.push_str(line);
        summary.push('\n');
    }
    summary.push_str(&format!(
        "\nWord-level sync: {} cue(s) matched ({} shifted), {} unmatched (threshold={:.0}%)",
        total_matched, total_shifted, total_unmatched, WL_MATCH_THRESHOLD
    ));

    // Touch info.json timestamp.
    let info_path = dataset_dir.join("info.json");
    if let Ok(data) = fs::read_to_string(&info_path) {
        if let Ok(mut info) = serde_json::from_str::<crate::dataset::DatasetInfo>(&data) {
            info.updated = Utc::now().to_rfc3339();
            if let Ok(out) = serde_json::to_string_pretty(&info) {
                let _ = fs::write(&info_path, out);
            }
        }
    }

    Ok(summary)
}

// ---------------------------------------------------------------------------
// Sync cue times from fresh subtitles
// ---------------------------------------------------------------------------

/// Sync cue timestamps from fresh VTT subtitles to existing DB cues.
///
/// For each media in the dataset, this function:
/// 1. Reads the fresh VTT file from the subtitle/ directory
/// 2. Loads existing cues from the database
/// 3. Matches cues by text similarity (Ratcliff-Obershelp)
/// 4. Updates start_ms/end_ms in the DB for matched cues
///
/// The cue text is NOT changed — only timestamps are updated.
#[tauri::command]
pub async fn dataset_sync_cue_times(
    settings: State<'_, SettingsState>,
    uuid: String,
    threshold: Option<f64>,
) -> Result<String, String> {
    let threshold = threshold.unwrap_or(70.0);
    log::info!(
        "dataset_sync_cue_times: dataset={}, threshold={}",
        uuid,
        threshold
    );
    let dataset_dir = find_dataset_dir(&settings, &uuid)?;
    let db_path = dataset_dir.join("data.sqlite3");
    if !db_path.exists() {
        return Err("Database file not found. Please build the database first.".into());
    }
    let subtitle_dir = dataset_dir.join("subtitle");
    if !subtitle_dir.exists() {
        return Err("Subtitle directory not found. Please generate subtitles first.".into());
    }

    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

    // Load all subtitles (excluding tmp working subtitles).
    let subtitles: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT uuid, media_uuid, name FROM listen_subtitle \
                 WHERE name != ?1 ORDER BY media_uuid, created_at",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(rusqlite::params![TMP_SUBTITLE_NAME], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };

    let mut total_matched = 0usize;
    let mut total_unmatched = 0usize;
    let mut total_shifted = 0usize;
    let mut log_lines: Vec<String> = Vec::new();

    for (subtitle_uuid, media_uuid, subtitle_name) in &subtitles {
        // Resolve media source path.
        let source: Option<String> = conn
            .query_row(
                "SELECT source FROM listen_media WHERE uuid = ?1",
                rusqlite::params![media_uuid],
                |row| row.get::<_, String>(0),
            )
            .ok();
        let Some(source) = source else {
            continue;
        };

        // Find the fresh VTT file (mirrors media sub-directory).
        let vtt_path = subtitle_dir.join(&source).with_extension("vtt");
        if !vtt_path.exists() {
            log_lines.push(format!(
                "Skipping '{}': no fresh VTT at {}",
                subtitle_name,
                vtt_path.display()
            ));
            continue;
        }

        // Parse fresh VTT cues.
        let fresh_cues = match parse_vtt(&vtt_path) {
            Ok(cues) => cues,
            Err(e) => {
                log_lines.push(format!("Failed to parse {}: {}", vtt_path.display(), e));
                continue;
            }
        };
        if fresh_cues.is_empty() {
            continue;
        }

        // Load existing DB cues.
        let db_cues: Vec<(String, i64, i64, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT uuid, start_ms, end_ms, content \
                     FROM listen_subtitle_cue WHERE subtitle_uuid = ?1 ORDER BY order_num",
                )
                .map_err(|e| e.to_string())?;
            let mapped = stmt
                .query_map(rusqlite::params![subtitle_uuid], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            mapped.filter_map(|r| r.ok()).collect()
        };
        if db_cues.is_empty() {
            continue;
        }

        log_lines.push(format!(
            "'{}': {} DB cues, {} fresh cues (threshold={})",
            subtitle_name,
            db_cues.len(),
            fresh_cues.len(),
            threshold
        ));

        let mut matched_count = 0usize;
        let mut unmatched_count = 0usize;
        let mut shifted_count = 0usize;

        // Match each DB cue to the best fresh cue by text similarity.
        for (cue_uuid, old_start, old_end, db_content) in &db_cues {
            let mut best_score: f64 = 0.0;
            let mut best_fresh: Option<&crate::dataset::VttCue> = None;

            for fc in &fresh_cues {
                let score = similarity_score(db_content, &fc.content);
                if score > best_score {
                    best_score = score;
                    best_fresh = Some(fc);
                }
            }

            if best_score >= threshold {
                if let Some(fc) = best_fresh {
                    let new_start = fc.start_ms;
                    let new_end = fc.end_ms;
                    conn.execute(
                        "UPDATE listen_subtitle_cue SET start_ms = ?1, end_ms = ?2 WHERE uuid = ?3",
                        rusqlite::params![new_start, new_end, cue_uuid],
                    )
                    .map_err(|e| e.to_string())?;

                    matched_count += 1;
                    if new_start != *old_start || new_end != *old_end {
                        shifted_count += 1;
                        let trunc = if db_content.len() > 50 {
                            format!("{}...", &db_content[..50])
                        } else {
                            db_content.clone()
                        };
                        log_lines.push(format!(
                            "  {} -> {}  {} -> {}  (score={:.0}) \"{}\"",
                            ms_to_timestamp(*old_start),
                            ms_to_timestamp(new_start),
                            ms_to_timestamp(*old_end),
                            ms_to_timestamp(new_end),
                            best_score,
                            trunc
                        ));
                    }
                }
            } else {
                unmatched_count += 1;
            }
        }

        // Update subtitle timestamp.
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE listen_subtitle SET updated_at = ?1 WHERE uuid = ?2",
            rusqlite::params![now, subtitle_uuid],
        )
        .map_err(|e| e.to_string())?;

        total_matched += matched_count;
        total_unmatched += unmatched_count;
        total_shifted += shifted_count;
    }

    drop(conn);

    log::info!(
        "Sync complete: {} matched, {} unmatched, {} shifted",
        total_matched,
        total_unmatched,
        total_shifted
    );

    let mut summary = String::new();
    for line in &log_lines {
        summary.push_str(line);
        summary.push('\n');
    }
    summary.push_str(&format!(
        "\nSynced {} cue(s) ({} shifted), {} unmatched (threshold={})",
        total_matched, total_shifted, total_unmatched, threshold
    ));

    // Touch info.json timestamp.
    let info_path = dataset_dir.join("info.json");
    if let Ok(data) = fs::read_to_string(&info_path) {
        if let Ok(mut info) = serde_json::from_str::<crate::dataset::DatasetInfo>(&data) {
            info.updated = Utc::now().to_rfc3339();
            if let Ok(out) = serde_json::to_string_pretty(&info) {
                let _ = fs::write(&info_path, out);
            }
        }
    }

    Ok(summary)
}
