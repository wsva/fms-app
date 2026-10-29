use std::path::Path;

use serde_json::{json, Value};

use crate::datasets::asr_metric;
use crate::settings::SettingsState;

use super::super::standard_audio::{
    self, GUESS_MAX_GAP, GUESS_MIN_MATCH_RATIO, GUESS_MIN_WORDS, GUESS_PAD_AFTER, GUESS_PAD_BEFORE,
};
use super::super::{edges, find_dataset_dir_write, open_db};
use super::index::{
    decode_peaks, load_index, read_json, transcript_path, write_json, SourceIndex,
    SourceTranscript,
};
use super::proposals::{counts, load_proposals, proposals_path, Proposal, ProposalSet};
use super::{
    round_ms, ANCHOR_MIN_MATCH_RATIO, ANCHOR_MIN_WORDS, MAX_WINDOW_TOKENS, MIN_SPAN_SECS,
    PRIOR_DISTANCE_PENALTY, PRIOR_SLACK_TOKENS, PRIOR_WIDE_SLACK_TOKENS,
};

// ============================================================
// Align
// ============================================================

/// A stretch of the stream that matches a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Found {
    /// Absolute index of the run's first and last token in the stream.
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) matched: usize,
}

/// What the guess decided about one text: the span worth proposing, and the
/// best stretch of the stream pass one saw — kept even when nothing is worth
/// proposing, because "the model heard 3 of your 5 words" is the difference
/// between a text that is missing and a model that misheard.
#[derive(Clone, Copy, Debug)]
pub(super) struct Guess {
    pub(super) span: Option<Found>,
    pub(super) heard: Option<Found>,
}

/// What one search turned up: the run worth a span, and the best run seen
/// regardless, so a refusal can still say how much of the text was heard.
#[derive(Debug, Default)]
pub(super) struct Search {
    pub(super) hit: Option<Found>,
    pub(super) best: Option<Found>,
}

/// `(first, last, matched)` of every maximal run of aligned pairs, in order.
///
/// A run breaks where the *text* skipped more than [`GUESS_MAX_GAP`] words: that
/// is the text reading something else in between, and the stretches either
/// side of it are separate claims about where it was read.
///
/// A jump on the stream side alone is not a break. Both sides of it are still the
/// same reading; what jumped is the audio, or the transcript's account of it — an
/// overlapping STT chunk that repeated the last fifteen seconds, a music bridge,
/// a hallucination the next chunk did not share. Breaking there cut a text's
/// head off: the longer half won the run score, so a chapter's second part was
/// placed at its *middle*, forty seconds past the words it opens with.
pub(super) fn runs_of(pairs: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    let mut from = 0usize;
    for index in 1..=pairs.len() {
        let breaks =
            index == pairs.len() || pairs[index].0.saturating_sub(pairs[index - 1].0) > GUESS_MAX_GAP;
        if breaks {
            if index > from {
                runs.push((pairs[from].1, pairs[index - 1].1, index - from));
            }
            from = index;
        }
    }
    runs
}

/// Where reading order says a text starts, in stream tokens.
///
/// A text's text *is* what a stretch of the recording says, so the number of
/// reference words before it maps onto the stream arithmetically: a book whose
/// 784 mails hold 44 699 words against 45 828 heard tokens settles at 1:1, and
/// every mail's expected position follows from the words counted before it. This
/// is what keeps one bad span from dragging the rest with it — the prediction
/// does not depend on where the previous text was put, only on the order the
/// texts were written in.
pub(super) fn prior_position(before: usize, total_reference: usize, total_stream: usize) -> usize {
    if total_reference == 0 || total_stream == 0 {
        return 0;
    }
    let position = (before as u128 * total_stream as u128 / total_reference as u128) as usize;
    position.min(total_stream - 1)
}

/// The score two candidate runs are compared by: words matched, less a small
/// charge for every token of distance from the predicted position.
fn run_score(found: &Found, expected: usize) -> f64 {
    found.matched as f64 - PRIOR_DISTANCE_PENALTY * found.from.abs_diff(expected) as f64
}

/// Scan one window: semi-global alignment of the whole text, then the runs of
/// the result, keeping the run that scores best against `expected`.
fn scan_window(
    reference: &[String],
    stream: &[String],
    lo: usize,
    hi: usize,
    expected: usize,
    need: usize,
) -> Search {
    let mut search = Search::default();
    if lo >= hi || reference.is_empty() {
        return search;
    }
    let pairs = asr_metric::aligned_pairs_glocal(reference, &stream[lo..hi]);
    for (first, last, matched) in runs_of(&pairs) {
        let found = Found { from: first + lo, to: last + lo, matched };
        if matched >= need && search.hit.map_or(true, |best| run_score(&found, expected) > run_score(&best, expected)) {
            search.hit = Some(found);
        }
        if search.best.map_or(true, |best| found.matched > best.matched) {
            search.best = Some(found);
        }
    }
    search
}

/// Where one text is read, judged against the position reading order predicts
/// for it.
///
/// Tight window first, wide second. The tight one is where nine texts in ten
/// are, and it is also what stops an eight-word one-liner from being matched to
/// the same words twenty minutes later — a match that would otherwise score
/// just as well.
pub(super) fn find_predicted(reference: &[String], stream: &[String], expected: usize, need: usize) -> Search {
    let mut best: Option<Found> = None;
    for slack in [PRIOR_SLACK_TOKENS, PRIOR_WIDE_SLACK_TOKENS] {
        let lo = expected.saturating_sub(slack);
        let width = slack
            .saturating_mul(2)
            .saturating_add(reference.len().saturating_mul(3))
            .saturating_add(300)
            .min(MAX_WINDOW_TOKENS);
        let hi = lo.saturating_add(width).min(stream.len());
        let found = scan_window(reference, stream, lo, hi, expected, need);
        if found.best.map_or(false, |f| best.map_or(true, |b| f.matched > b.matched)) {
            best = found.best;
        }
        if found.hit.is_some() {
            return Search { hit: found.hit, best: found.hit.or(best) };
        }
    }
    Search { hit: None, best }
}

/// Search forward from `cursor` for the text's reading.
///
/// The last resort, for the case the prediction cannot serve: the texts are not
/// the recordings read in order — a sample chapter added in front, half the
/// book's files untranscribed — so there is nothing to count words against and
/// the windows [`find_predicted`] builds point at unrelated audio. Walking a
/// cursor is worse than predicting (that is what [`place_all`] is for), but it
/// does find what is there in the order it was written, which beats placing
/// nothing at all.
#[cfg(feature = "stt")]
fn walk_from_cursor(stream: &[String], texts: &[Vec<String>]) -> Vec<Guess> {
    let mut slots: Vec<Guess> = vec![Guess { span: None, heard: None }; texts.len()];
    let mut cursor = 0usize;
    for (index, reference) in texts.iter().enumerate() {
        if reference.is_empty() {
            continue;
        }
        let search = find_predicted(reference, stream, cursor, need_for(reference));
        if let Some(found) = search.hit {
            slots[index] = Guess { span: Some(found), heard: Some(found) };
            cursor = found.to + 1;
        }
    }
    slots
}

/// The reading both guesses share: the recorded material on one side, the
/// reference texts on the other.
#[cfg(feature = "stt")]
struct Prepared {
    index: SourceIndex,
    transcripts: Vec<Option<SourceTranscript>>,
    /// Every heard token, in playing order, across every transcribed file.
    stream: Vec<String>,
    /// For each stream token: the recording it came from, and its word's
    /// `[start, end)` in that recording.
    at: Vec<(usize, f64, f64)>,
    /// `(text uuid, reference text)` in `order_num` — reading order.
    texts: Vec<(String, String)>,
}

#[cfg(feature = "stt")]
impl Prepared {
    fn source_position(&self, uuid: &str) -> Option<usize> {
        self.index.sources.iter().position(|source| source.uuid == uuid)
    }

    /// Where the stream resumes after `end` of `source`.
    ///
    /// The bulk guess knows this exactly — it holds the token index it stopped
    /// on — but a one-text run starts from a stored span, so that span's end
    /// time is the only handle it has. Landing a word early costs nothing: an
    /// unmatched prefix is skipped by the alignment.
    fn cursor_after(&self, source: usize, end: f64) -> Option<usize> {
        self.at.iter().position(|(position, _, word_end)| {
            *position > source || (*position == source && *word_end >= end - 1e-6)
        })
    }
}

/// Read the recordings and the texts a guess walks.
///
/// Every refusal about *missing* input lives here, so a single text cannot
/// be answered differently from a bulk run: same registrations, same caches,
/// same reading order.
#[cfg(feature = "stt")]
fn prepare(dir: &Path) -> Result<Prepared, String> {
    let index = load_index(dir);
    if index.sources.is_empty() {
        return Err("No recordings are registered: pick the folder that holds them first.".to_string());
    }

    let transcripts: Vec<Option<SourceTranscript>> = index
        .sources
        .iter()
        .map(|source| read_json(&transcript_path(dir, &source.uuid)))
        .collect();
    if transcripts.iter().all(Option::is_none) {
        return Err(
            "None of the registered recordings has been transcribed yet. Transcribe at least one, then guess."
                .to_string(),
        );
    }

    // One token stream across every recording, in playing order.
    let mut stream: Vec<String> = Vec::new();
    let mut at: Vec<(usize, f64, f64)> = Vec::new();
    for (position, transcript) in transcripts.iter().enumerate() {
        let Some(transcript) = transcript else { continue };
        for word in &transcript.words {
            for token in &word.tokens {
                stream.push(token.clone());
                at.push((position, word.start, word.end));
            }
        }
    }
    if stream.is_empty() {
        return Err("The cached transcripts hold no words to align against.".to_string());
    }

    let texts: Vec<(String, String)> = {
        let conn = open_db(dir)?;
        let mut stmt = conn
            .prepare("SELECT uuid, content, normalized_content FROM read_text ORDER BY order_num, rowid")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.flatten()
            .map(|(uuid, content, normalized)| {
                (uuid, super::super::effective_reference(&content, normalized.as_deref()))
            })
            .collect()
    };
    if texts.is_empty() {
        return Err("This dataset has no texts to place.".to_string());
    }

    Ok(Prepared { index, transcripts, stream, at, texts })
}

/// How many of a text's tokens must be heard, in order, before the placement is
/// worth a span. A short text cannot be held to a percentage of itself.
#[cfg(feature = "stt")]
pub(super) fn need_for(tokens: &[String]) -> usize {
    (((tokens.len() as f64) * GUESS_MIN_MATCH_RATIO).ceil() as usize)
        .max(GUESS_MIN_WORDS.min(tokens.len()))
}

/// Place every text along the stream: anchors first, then the gaps.
///
/// Pass one asks every text long enough to be evidence in itself where it is,
/// against the position its order predicts. What comes out is an absolute span
/// with a confidence, and the spans are forced to be increasing, so a wrong one
/// costs itself and nothing else — it cannot drag the next text with it the
/// way a shared cursor could.
///
/// Pass two takes the gaps pass one left and aligns everything unplaced in each
/// as one block against the audio between the two surrounding anchors. That is
/// where the one-liners go: "40 Sekunden später" is eight words and occurs a
/// dozen times, so aligning it alone finds *an* occurrence — the block alignment
/// cannot put it out of order, and it can tell that there is no room here for
/// the texts that claim to be.
///
/// A text that survives both passes unplaced keeps the best run pass one saw,
/// so a refusal can still report it heard 5 of the text's 310 words — but that
/// run is *not* a placement: it is wherever the words happened to occur, which
/// is not where the text belongs.
#[cfg(feature = "stt")]
pub(super) fn place_all(stream: &[String], texts: &[Vec<String>]) -> Vec<Guess> {
    let total_reference: usize = texts.iter().map(Vec::len).sum();
    let mut slots: Vec<Option<Found>> = vec![None; texts.len()];
    let mut heard: Vec<Option<Found>> = vec![None; texts.len()];
    let mut anchors: Vec<(usize, usize, usize)> = Vec::new();
    let mut before = 0usize;
    let mut floor = 0usize;

    for (index, reference) in texts.iter().enumerate() {
        let start = before;
        before += reference.len();
        if reference.len() < ANCHOR_MIN_WORDS {
            continue;
        }
        let need = need_for(reference);
        let expected = prior_position(start, total_reference, stream.len());
        let search = find_predicted(reference, stream, expected, need);
        heard[index] = search.hit.or(search.best);
        let Some(found) = search.hit else { continue };
        if (found.matched as f64) < ANCHOR_MIN_MATCH_RATIO * reference.len() as f64 {
            continue;
        }
        if found.from <= floor {
            continue;
        }
        slots[index] = Some(found);
        anchors.push((index, found.from, found.to));
        floor = found.to;
    }

    let mut next_text = 0usize;
    let mut next_stream = 0usize;
    for &(index, from, to) in &anchors {
        fill_gap(stream, texts, &mut slots, next_text..index, next_stream..from);
        next_text = index + 1;
        next_stream = to + 1;
    }
    fill_gap(stream, texts, &mut slots, next_text..texts.len(), next_stream..stream.len());

    slots
        .into_iter()
        .zip(heard)
        .map(|(span, heard)| Guess { span, heard })
        .collect()
}

/// Align one gap's texts as a single block.
///
/// The concatenation of their texts is aligned once against the audio between
/// the two anchors that bound the gap, which makes the answer monotone by
/// construction: no text in a block can be placed before another one, and
/// none can claim audio that belongs to its neighbour. Where the block's own
/// alignment heard too little of a text — a gap that does not hold it at all —
/// that text stays unplaced instead of being stretched across the gap.
#[cfg(feature = "stt")]
fn fill_gap(
    stream: &[String],
    texts: &[Vec<String>],
    slots: &mut [Option<Found>],
    range: std::ops::Range<usize>,
    audio: std::ops::Range<usize>,
) {
    let pending: Vec<usize> = range
        .filter(|&index| !texts[index].is_empty() && slots[index].is_none())
        .collect();
    if pending.is_empty() || audio.start >= audio.end {
        return;
    }
    let start = audio.start.min(stream.len());
    let end = audio.end.min(stream.len());
    if start >= end {
        return;
    }

    let mut block: Vec<String> = Vec::new();
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    for &index in &pending {
        spans.push((index, block.len(), block.len() + texts[index].len()));
        block.extend(texts[index].iter().cloned());
    }

    let pairs = asr_metric::aligned_pairs(&block, &stream[start..end]);
    let mut cursor = 0usize;
    let mut previous_end: Option<usize> = None;
    for (index, from, to) in spans {
        while cursor < pairs.len() && pairs[cursor].0 < from {
            cursor += 1;
        }
        let first = cursor;
        while cursor < pairs.len() && pairs[cursor].0 < to {
            cursor += 1;
        }
        let matched = cursor - first;
        if matched < need_for(&texts[index]) {
            continue;
        }
        let lower = previous_end.map_or(start, |end| end + 1);
        let span = Found {
            from: (pairs[first].1 + start).max(lower),
            to: (pairs[cursor - 1].1 + start).max(lower),
            matched,
        };
        slots[index] = Some(span);
        previous_end = Some(span.to);
    }
}

/// A placed (or refused) text, and the floor the *next* text in reading
/// order must not start before.
#[cfg(feature = "stt")]
struct Placed {
    proposal: Proposal,
    floor: Option<(usize, f64)>,
}

/// Turn a found run into a proposal: the padded span, both edges placed in the
/// pause they belong to, and clamped against the text before it. Nothing is
/// carried over from a previous run — the same stream gives the same span, so a
/// re-run only changes what a changed transcript changes.
#[cfg(feature = "stt")]
fn span_proposal(
    prepared: &Prepared,
    text_uuid: &str,
    tokens: &[String],
    found: &Found,
    floor: Option<(usize, f64)>,
) -> Placed {
    let (first_source, first_start, _) = prepared.at[found.from];
    let (last_source, _, last_end) = prepared.at[found.to];
    if first_source != last_source {
        let mut refusal = Proposal::refusal(
            text_uuid,
            format!(
                "This text is read across the boundary between {} and {}. Crop it by hand from the two files.",
                prepared.index.sources[first_source].file_name,
                prepared.index.sources[last_source].file_name
            ),
        );
        refusal.matched_words = found.matched;
        refusal.reference_words = tokens.len();
        return Placed {
            proposal: refusal,
            floor: Some((last_source, last_end)),
        };
    }

    let Some(transcript) = prepared.transcripts[first_source].as_ref() else {
        return Placed {
            proposal: Proposal::refusal(
                text_uuid,
                "The recording this text matched has no cached transcript.".to_string(),
            ),
            floor: None,
        };
    };
    let clamped = floor
        .filter(|(source, _)| *source == first_source)
        .map(|(_, end)| end);
    // The pads are the fallback; where a real pause belongs to the edge, the cut
    // goes into the middle of it instead, so two neighbouring clips divide the
    // silence rather than leaving half of it to neither. The pause can be most of
    // a second from the word: the model glues a signature to the number after it
    // and runs a word's end past its audio. The bounds are what keeps a clip off
    // its neighbour and off the minimum length.
    let peaks = decode_peaks(&transcript.peaks);
    let loudness = edges::loudness_from_envelope(&peaks);
    let start = round_ms(edges::place_edge(
        &loudness,
        transcript.peak_seconds,
        first_start,
        GUESS_PAD_BEFORE,
        edges::Side::Head,
        clamped.unwrap_or(0.0),
        transcript.duration,
    ));
    let mut end = round_ms(edges::place_edge(
        &loudness,
        transcript.peak_seconds,
        last_end,
        GUESS_PAD_AFTER,
        edges::Side::Tail,
        start + MIN_SPAN_SECS,
        transcript.duration,
    ));
    if end - start < MIN_SPAN_SECS {
        end = round_ms((start + MIN_SPAN_SECS).min(transcript.duration));
    }
    let confidence = (found.matched as f64 / tokens.len() as f64 * 100.0).round() / 100.0;
    let source_uuid = prepared.index.sources[first_source].uuid.clone();

    Placed {
        proposal: Proposal {
            text_uuid: text_uuid.to_string(),
            source_uuid: Some(source_uuid),
            source_name: transcript.file_name.clone(),
            start,
            end,
            confidence,
            matched_words: found.matched,
            reference_words: tokens.len(),
            status: "proposed".to_string(),
            // `confidence` already says how much of the text was heard, so a
            // placed span has nothing to explain; `note` is a refusal's reason.
            note: String::new(),
        },
        floor: Some((first_source, end)),
    }
}

/// Place every text and write the proposals.
///
/// Every text is decided from its own position in reading order, so re-running
/// is idempotent — same stream, same spans — except where a transcript changed
/// under it, and one surprising span costs only itself.
#[cfg(feature = "stt")]
pub(crate) fn run(settings: &SettingsState, dataset_uuid: &str) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let prepared = prepare(&dir)?;
    let texts: Vec<Vec<String>> = prepared
        .texts
        .iter()
        .map(|(_, reference)| asr_metric::normalize_tokens(reference))
        .collect();
    // Reading order is the model the guess is built on, and when it does not
    // hold — half the files were never transcribed, the texts were reordered
    // by hand — the predicted windows point at unrelated audio and everything
    // refuses. A quarter placed is the line where that is the explanation
    // rather than a hard book, and a cursor walk that finds what is there in
    // the order it was written beats placing nothing.
    let mut found = place_all(&prepared.stream, &texts);
    let placed = found.iter().filter(|guess| guess.span.is_some()).count();
    if placed * 4 < texts.len() {
        found = walk_from_cursor(&prepared.stream, &texts);
    }

    let mut proposals: Vec<Proposal> = Vec::with_capacity(prepared.texts.len());
    let mut floor: Option<(usize, f64)> = None;
    let mut last_end = 0usize;

    for (index, (text_uuid, _)) in prepared.texts.iter().enumerate() {
        let tokens = &texts[index];
        if tokens.is_empty() {
            proposals.push(Proposal::refusal(text_uuid, "That text has no words to look for.".to_string()));
            continue;
        }
        let guess = found[index];
        // Nothing worth proposing. A refusal that can say how much of the text
        // the model did hear points at a mishearing (a weak model, a strange
        // name); one that cannot points at the text itself — a subject line, a
        // greeting, a date that the narrator never reads.
        let Some(span) = guess.span else {
            let mut refusal = Proposal::refusal(
                text_uuid,
                match guess.heard {
                    Some(heard) => format!(
                        "The model heard only {} of this text's {} words in the recordings, which is too little to place the reading. Try a stronger model, normalize the text, or set the edges by hand.",
                        heard.matched, tokens.len()
                    ),
                    None => "No stretch of the recordings holds this text, so it was not placed. It may be missing from the audio, or it may be all header — a subject line, a greeting, a date — that is never read aloud. Normalize the text, then guess again.".to_string(),
                },
            );
            if let Some(heard) = guess.heard {
                refusal.matched_words = heard.matched;
                refusal.reference_words = tokens.len();
            }
            proposals.push(refusal);
            continue;
        };

        let placed = span_proposal(&prepared, text_uuid, tokens, &span, floor);
        if let Some(next) = placed.floor {
            floor = Some(next);
        }
        last_end = last_end.max(span.to + 1);
        proposals.push(placed.proposal);
    }

    let set = ProposalSet {
        generated_at: crate::datasets::now_row_stamp(),
        proposals,
    };
    write_json(&proposals_path(&dir), &set)?;

    let proposed = set.proposals.iter().filter(|p| p.status == "proposed").count();
    let refused = set.proposals.len() - proposed;
    let mean_confidence = if proposed == 0 {
        0.0
    } else {
        let total: f64 = set
            .proposals
            .iter()
            .filter(|p| p.status == "proposed")
            .map(|p| p.confidence)
            .sum();
        (total / proposed as f64 * 100.0).round() / 100.0
    };
    Ok(json!({
        "status": "ok",
        "texts": set.proposals.len(),
        "proposed": proposed,
        "refused": refused,
        "mean_confidence": mean_confidence,
        "unused_words": prepared.stream.len().saturating_sub(last_end),
        "sources": prepared.index.sources.len(),
        "generated_at": set.generated_at,
    }))
}

#[cfg(not(feature = "stt"))]
pub(crate) fn run(_settings: &SettingsState, _dataset_uuid: &str) -> Result<Value, String> {
    Err("This build has no speech recognition, so the spans cannot be guessed.".to_string())
}

/// Re-place one text, leaving every other row exactly as it is.
///
/// The bulk guess is the right tool for a set that has never been placed and
/// the wrong one for a single row: it rewrites the whole file, so a span
/// somebody nudged by hand would be replaced by the model's opinion of it. This
/// places only the requested text — at the position its order predicts, along the
/// same stream the bulk guess reads, and with the end of the nearest earlier
/// text as a fallback when that prediction finds nothing — and replaces that
/// one row. Everything else is untouched.
#[cfg(feature = "stt")]
pub(crate) fn run_one(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
) -> Result<Value, String> {
    let dir = find_dataset_dir_write(settings, dataset_uuid)?;
    let prepared = prepare(&dir)?;
    let mut set = load_proposals(&dir);

    let position = prepared
        .texts
        .iter()
        .position(|(uuid, _)| uuid == text_uuid)
        .ok_or_else(|| format!("No text with id {text_uuid} is in this dataset — reload the list."))?;
    if !set.proposals.iter().any(|p| p.text_uuid == text_uuid) {
        return Err(
            "This text has never been placed, so there is nothing to re-place: guess for all first."
                .to_string(),
        );
    }

    // Where to start looking: the end of the nearest earlier text that has a
    // span. Reading order is the only order the guess knows, and it is the one
    // the recordings follow.
    let mut cursor = 0usize;
    let mut floor: Option<(usize, f64)> = None;
    for (uuid, _) in prepared.texts[..position].iter().rev() {
        let Some(row) = set
            .proposals
            .iter()
            .find(|p| p.text_uuid == *uuid && p.status == "proposed")
        else {
            continue;
        };
        let Some(source) = row
            .source_uuid
            .as_deref()
            .and_then(|uuid| prepared.source_position(uuid))
        else {
            continue;
        };
        cursor = prepared.cursor_after(source, row.end).unwrap_or(prepared.stream.len());
        floor = Some((source, row.end));
        break;
    }

    let tokens = asr_metric::normalize_tokens(&prepared.texts[position].1);
    if tokens.is_empty() {
        return Err("That text has no words to look for, so it cannot be placed.".to_string());
    }
    let need = need_for(&tokens);

    // The prediction first, the neighbour's end second. The prediction is the
    // better answer — it is where the bulk guess would put this row — and the
    // cursor is only a fallback for a set whose order no longer matches the
    // recordings, e.g. after texts were added or re-placed by hand.
    let every_text: Vec<Vec<String>> = prepared
        .texts
        .iter()
        .map(|(_, reference)| asr_metric::normalize_tokens(reference))
        .collect();
    let total_reference: usize = every_text.iter().map(Vec::len).sum();
    let before: usize = every_text[..position].iter().map(Vec::len).sum();
    let expected = prior_position(before, total_reference, prepared.stream.len());
    let mut search = find_predicted(&tokens, &prepared.stream, expected, need);
    if search.hit.is_none() && cursor.abs_diff(expected) > PRIOR_SLACK_TOKENS {
        let retry = find_predicted(&tokens, &prepared.stream, cursor, need);
        if retry.hit.is_some() {
            search = retry;
        }
    }
    let found = search.hit.or(search.best);

    // The two ways to come up empty are written as refusals, not errors: the row
    // exists, and the answer to "why is it still not placed?" belongs on it.
    let placed = match found {
        None => Placed {
            proposal: Proposal::refusal(
                text_uuid,
                "No stretch of the recordings holds this text, so it was not placed. It may be missing from the audio, or it may be all header — a subject line, a greeting, a date — that is never read aloud. Normalize the text, then guess again.".to_string(),
            ),
            floor: None,
        },
        Some(found) if found.matched < need => {
            let mut refusal = Proposal::refusal(
                text_uuid,
                format!(
                    "The model heard only {} of this text's {} words in the recordings, which is too little to place the reading. Try a stronger model, or set the edges by hand.",
                    found.matched,
                    tokens.len()
                ),
            );
            refusal.matched_words = found.matched;
            refusal.reference_words = tokens.len();
            Placed { proposal: refusal, floor: None }
        }
        Some(found) => span_proposal(&prepared, text_uuid, &tokens, &found, floor),
    };

    // Replace the row in place: the set's order is the queue's order, and
    // nothing else about it changes.
    if let Some(slot) = set.proposals.iter_mut().find(|p| p.text_uuid == text_uuid) {
        *slot = placed.proposal.clone();
    }
    set.generated_at = crate::datasets::now_row_stamp();
    write_json(&proposals_path(&dir), &set)?;

    let proposal = &placed.proposal;
    // The counts describe the whole set, so they need the clip map as well.
    let clips = standard_audio::rows_by_text(&dir)?;
    Ok(json!({
        "status": "ok",
        "text_uuid": proposal.text_uuid,
        "placed": proposal.status == "proposed",
        "source_uuid": proposal.source_uuid,
        "source_name": proposal.source_name,
        "start": proposal.start,
        "end": proposal.end,
        "confidence": proposal.confidence,
        "matched_words": proposal.matched_words,
        "reference_words": proposal.reference_words,
        "note": proposal.note,
        "counts": counts(&set, &clips),
        "generated_at": set.generated_at,
    }))
}

#[cfg(not(feature = "stt"))]
pub(crate) fn run_one(
    _settings: &SettingsState,
    _dataset_uuid: &str,
    _text_uuid: &str,
) -> Result<Value, String> {
    Err("This build has no speech recognition, so the spans cannot be guessed.".to_string())
}

