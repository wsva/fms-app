//! The reference timeline: what the learner was *supposed* to say, as phones.
//!
//! ## Why this is derived from TTS instead of from a dictionary
//!
//! GOP needs the expected phone sequence pinned to time. Three ways to get it:
//!
//! 1. A G2P engine (espeak-ng, CMUdict). Rejected: espeak-ng is a C library
//!    needing Android NDK work, and no G2P crate covers German.
//! 2. Teacher-forcing the STT model. Rejected: `models/` exposes no logits,
//!    logprobs or decoder access for any backend (see the capability table in
//!    `models::transcribe_file_word_level`), so it cannot be done at all.
//! 3. Synthesise the text canonically, then let the *same* phoneme model decode
//!    that rendering. Adopted.
//!
//! Option 3 is what makes the rest cheap: the reference phone inventory is the
//! model's own inventory, so there is no lexicon, no language table and no
//! hand-written mapping to keep in sync. It also measures the learner rather
//! than the model's guess, because the canon is produced by a different voice
//! reading the text correctly.
//!
//! ## The cost, named honestly
//!
//! One network fetch per text. So it is cached forever after, keyed by
//! `sha256(text, voice)` — the same text at the same voice always yields the
//! same canonical rendering, and scoring then works fully offline. A TTS
//! mispronunciation becomes a target mispronunciation, which is the one error
//! mode this design cannot detect; it is listed under known limitations in
//! `docs/design/read-aloud-score.md`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::audio;
use crate::edge_tts::{self, TtsWordBoundary};

use super::gop::PhoneSpan;

/// Directory name (beside `read_aloud_audio/`) holding the reference cache.
/// Deliberately **not** inside the dataset: practising must never write into a
/// dataset directory (INV-9).
pub(crate) const CACHE_DIR: &str = "read_aloud_tts";

/// A canonical rendering of a reference text.
pub(crate) struct Reference {
    /// 16 kHz mono PCM, decoded once when the entry is loaded.
    pub samples: Vec<f32>,
    pub boundaries: Vec<TtsWordBoundary>,
}

impl Reference {
    pub(crate) fn seconds(&self) -> f64 {
        self.samples.len() as f64 / audio::DECODED_SAMPLE_RATE as f64
    }
}

/// Why a reference timeline could not be produced. Both variants become a
/// `PronReason` — never a fabricated score.
#[derive(Debug)]
pub(crate) enum RefError {
    /// No connection / the request never completed.
    NoNetwork,
    /// The service answered but the audio could not be written or decoded.
    Failed(String),
}

/// Classify an `edge_tts` error string. The transport layer reports errors as
/// strings, so "could not reach the service" has to be recognised textually;
/// when in doubt it is reported as a hard failure rather than "no network", so
/// a real bug is not disguised as an offline device.
fn classify(message: &str) -> RefError {
    let lower = message.to_ascii_lowercase();
    const NETWORK: [&str; 9] = [
        "connect",
        "dns",
        "network",
        "timed out",
        "timeout",
        "unreachable",
        "os error",
        "failed to lookup",
        "certificate",
    ];
    if NETWORK.iter().any(|needle| lower.contains(needle)) {
        RefError::NoNetwork
    } else {
        RefError::Failed(message.to_string())
    }
}

/// Cache file stem: `{readable voice}-{8 hex}`. The readable part exists so the
/// directory can be inspected by a human; the hash is what makes it unique.
pub(crate) fn cache_key(text: &str, voice: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher.update([0x1f]);
    hasher.update(voice.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest[..4].iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}-{}", sanitize(voice), hex)
}

fn sanitize(voice: &str) -> String {
    let cleaned: String = voice
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "voice".to_string()
    } else {
        cleaned
    }
}

fn mp3_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{}.mp3", key))
}

fn json_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{}.json", key))
}

/// Load a cached reference. `None` means "not cached, or no longer usable" —
/// both are handled the same way (fetch it again).
pub(crate) fn load(dir: &Path, key: &str) -> Option<Reference> {
    let json = std::fs::read_to_string(json_path(dir, key)).ok()?;
    let boundaries: Vec<TtsWordBoundary> = serde_json::from_str(&json).ok()?;
    // An entry without word boundaries cannot be grouped per word, so it is
    // treated as absent rather than half-used.
    if boundaries.is_empty() {
        return None;
    }
    let samples = audio::decode_to_pcm(&mp3_path(dir, key)).ok()?;
    if samples.is_empty() {
        return None;
    }
    Some(Reference { samples, boundaries })
}

fn store(dir: &Path, key: &str, new: &Reference, mp3: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create reference cache {}: {}", dir.display(), e))?;
    std::fs::write(mp3_path(dir, key), mp3)
        .map_err(|e| format!("Failed to write reference audio: {}", e))?;
    let json = serde_json::to_string(&new.boundaries)
        .map_err(|e| format!("Failed to serialise reference boundaries: {}", e))?;
    std::fs::write(json_path(dir, key), json)
        .map_err(|e| format!("Failed to write reference boundaries: {}", e))?;
    Ok(())
}

/// Produce the reference timeline for `text`, from the cache when possible.
pub(crate) async fn resolve(
    dir: &Path,
    text: &str,
    voice: &str,
) -> Result<Reference, RefError> {
    let key = cache_key(text, voice);
    if let Some(cached) = load(dir, &key) {
        log::debug!("read_aloud reference: cache hit for {}", key);
        return Ok(cached);
    }

    log::info!("read_aloud reference: synthesising {} chars with {}", text.len(), voice);
    let audio = edge_tts::synthesize_with_boundaries(text, voice, None, None, None)
        .await
        .map_err(|e| classify(&e))?;

    if audio.boundaries.is_empty() {
        return Err(RefError::Failed(
            "Edge TTS returned no word boundaries for this text".to_string(),
        ));
    }

    // Decode through the same path a cached entry uses, so the first run and
    // every later run see byte-identical PCM.
    std::fs::create_dir_all(dir)
        .map_err(|e| RefError::Failed(format!("Failed to create reference cache: {}", e)))?;
    let key = cache_key(text, voice);
    std::fs::write(mp3_path(dir, &key), &audio.audio)
        .map_err(|e| RefError::Failed(format!("Failed to write reference audio: {}", e)))?;
    let samples = audio::decode_to_pcm(&mp3_path(dir, &key))
        .map_err(|e| RefError::Failed(format!("Failed to decode reference audio: {}", e)))?;
    if samples.is_empty() {
        return Err(RefError::Failed("Reference audio decoded to nothing".to_string()));
    }

    let reference = Reference { samples, boundaries: audio.boundaries };
    store(dir, &key, &reference, &audio.audio)
        .map_err(|e| RefError::Failed(e))?;
    Ok(reference)
}

/// Seconds per frame of a posterior matrix, from its frame count and the audio
/// it was computed over. The model's own stride, not an assumed one.
pub(crate) fn frame_seconds(frames: usize, audio_secs: f64) -> f64 {
    if frames == 0 || audio_secs <= 0.0 {
        return 0.0;
    }
    audio_secs / frames as f64
}

/// Attribute the phones of the canonical rendering to reference words.
///
/// `spans` are the phone spans on the **TTS** posteriors, `word_count` the
/// number of compared reference words. When the boundary count and the word
/// count agree — the normal case — each phone joins the word interval its
/// midpoint falls in (nearest interval when it falls in a gap). When they
/// disagree (a punctuation-only event, a number split into two words) the
/// phones are distributed by time proportional to the word count, which keeps
/// their order — the only thing forced alignment actually needs.
pub(crate) fn group_phones(
    spans: &[PhoneSpan],
    frame_secs: f64,
    boundaries: &[TtsWordBoundary],
    word_count: usize,
) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); word_count];
    if spans.is_empty() || word_count == 0 {
        return groups;
    }

    let midpoint = |span: &PhoneSpan| (span.start as f64 + span.end as f64) / 2.0 * frame_secs;

    if boundaries.len() == word_count {
        for span in spans {
            let at = midpoint(span);
            let mut best = 0usize;
            let mut best_distance = f64::INFINITY;
            for (index, boundary) in boundaries.iter().enumerate() {
                let start = boundary.start;
                let end = boundary.start + boundary.duration;
                let distance = if at < start {
                    start - at
                } else if at > end {
                    at - end
                } else {
                    0.0
                };
                if distance < best_distance {
                    best_distance = distance;
                    best = index;
                }
            }
            groups[best].push(span.phone);
        }
        return groups;
    }

    log::debug!(
        "read_aloud reference: {} boundaries vs {} words, distributing by time",
        boundaries.len(),
        word_count
    );
    let total = boundaries
        .iter()
        .map(|b| b.start + b.duration)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    for span in spans {
        let fraction = (midpoint(span) / total).clamp(0.0, 0.999_999);
        let index = ((fraction * word_count as f64) as usize).min(word_count - 1);
        groups[index].push(span.phone);
    }
    groups
}

/// The phone target of the whole take, in reading order.
pub(crate) fn flatten(groups: &[Vec<usize>]) -> Vec<usize> {
    groups.iter().flatten().copied().collect()
}

/// Cut an aligned phone timeline back into the words it came from.
///
/// Returns one empty group per word when the alignment does not cover exactly
/// the phones the grouping asked for — a word with no alignment has no
/// pronunciation score, which is honest; inventing one is not.
pub(crate) fn split_by_word(spans: &[PhoneSpan], groups: &[Vec<usize>]) -> Vec<Vec<PhoneSpan>> {
    let total: usize = groups.iter().map(|g| g.len()).sum();
    if spans.len() != total {
        return vec![Vec::new(); groups.len()];
    }
    let mut out: Vec<Vec<PhoneSpan>> = Vec::with_capacity(groups.len());
    let mut offset = 0usize;
    for group in groups {
        out.push(spans[offset..offset + group.len()].to_vec());
        offset += group.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(text: &str, start: f64, duration: f64) -> TtsWordBoundary {
        TtsWordBoundary { text: text.to_string(), start, duration }
    }

    fn span(phone: usize, start: usize, end: usize) -> PhoneSpan {
        PhoneSpan { phone, start, end }
    }

    #[test]
    fn the_key_is_stable_and_depends_on_text_and_voice() {
        let a = cache_key("Guten Tag", "de-DE-KatjaNeural");
        assert_eq!(a, cache_key("Guten Tag", "de-DE-KatjaNeural"));
        assert_ne!(a, cache_key("Guten Tag!", "de-DE-KatjaNeural"));
        assert_ne!(a, cache_key("Guten Tag", "de-DE-ConradNeural"));
        // Readable prefix, 8 hex chars of digest.
        assert!(a.starts_with("de-DE-KatjaNeural-"));
        assert_eq!(a.rsplit('-').next().unwrap().len(), 8);
    }

    #[test]
    fn phones_are_grouped_by_the_word_they_fall_into() {
        // Two words: [0, 0.5) and [0.6, 1.1). Frames are 10 ms.
        let boundaries = vec![boundary("ab", 0.0, 0.5), boundary("cd", 0.6, 0.5)];
        let spans = vec![span(11, 0, 5), span(12, 5, 20), span(13, 60, 65), span(14, 65, 70)];

        let groups = group_phones(&spans, 0.01, &boundaries, 2);
        assert_eq!(groups, vec![vec![11, 12], vec![13, 14]]);
    }

    #[test]
    fn a_phone_in_a_gap_joins_the_nearest_word() {
        let boundaries = vec![boundary("ab", 0.0, 0.2), boundary("cd", 0.9, 0.1)];
        let spans = vec![span(11, 0, 2), span(12, 40, 42), span(13, 91, 95)];
        // 0.41 s sits closer to the end of word one (0.2) than to word two
        // (0.9), so it stays with word one.
        let groups = group_phones(&spans, 0.01, &boundaries, 2);
        assert_eq!(groups, vec![vec![11, 12], vec![13]]);
    }

    #[test]
    fn a_boundary_count_mismatch_falls_back_to_time_order() {
        let boundaries = vec![boundary("a", 0.0, 0.5), boundary("b", 0.5, 0.5), boundary("c", 0.9, 0.1)];
        let spans = vec![span(11, 0, 10), span(12, 50, 60), span(13, 95, 99)];
        let groups = group_phones(&spans, 0.01, &boundaries, 2);
        // Order is preserved even though the counts disagree.
        let flat = flatten(&groups);
        assert_eq!(flat, vec![11, 12, 13]);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn no_phones_means_empty_groups_not_a_panic() {
        let groups = group_phones(&[], 0.01, &[boundary("a", 0.0, 1.0)], 1);
        assert_eq!(groups, vec![Vec::<usize>::new()]);
        assert!(group_phones(&[], 0.01, &[], 0).is_empty());
        assert_eq!(frame_seconds(0, 1.0), 0.0);
    }

    #[test]
    fn alignment_spans_are_cut_back_into_words() {
        let groups = vec![vec![11, 12], vec![13]];
        let spans = vec![span(11, 0, 3), span(12, 3, 6), span(13, 6, 9)];
        let split = split_by_word(&spans, &groups);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].len(), 2);
        assert_eq!(split[1], vec![span(13, 6, 9)]);
    }

    #[test]
    fn a_short_alignment_leaves_words_unscored() {
        let groups = vec![vec![11, 12], vec![13]];
        let spans = vec![span(11, 0, 3)];
        let split = split_by_word(&spans, &groups);
        assert_eq!(split, vec![Vec::new(), Vec::new()]);
    }

    #[test]
    fn network_looking_errors_are_classified_as_offline() {
        assert!(matches!(classify("error sending request: connect error"), RefError::NoNetwork));
        assert!(matches!(classify("dns error"), RefError::NoNetwork));
        assert!(matches!(classify("websocket closed unexpectedly"), RefError::Failed(_)));
    }
}
