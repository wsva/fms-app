//! Source alignment: guess every text's span from a directory of recordings.
//!
//! ## Words used here
//!
//! | word | means |
//! |---|---|
//! | **recording** | one file outside the dataset (`source_uuid`). A *take* is the learner's own reading and is a different thing. |
//! | **text** | a text of the dataset (`read_text`). |
//! | **span** | the seconds one text occupies inside one recording, with the confidence that it was heard there. |
//! | **clip** | a text's standard audio: the `read_standard_audio` row pronunciation is measured against. An aligned clip remembers its span; its wav is cut later, on first need. |
//! | **save** | span → clip row, a few hundred row writes. **Cut** is the expensive half: clip row → wav, and only when something needs to hear it. |
//!
//! Two rules follow, and they are the whole page: a span is the truth and a
//! clip is a cache of it; and a save never overwrites a clip, so changing a
//! saved span means deleting that clip first.
//!
//! ## The problem this exists for
//!
//! An audiobook's recordings are **outside** the dataset and each holds many
//! texts. [`super::standard_audio::guess_span`] answers "where is *this* text
//! in *that* file" by decoding and transcribing the whole file, so guessing a
//! whole dataset that way costs O(texts × files) minutes and throws away the one
//! fact the bulk case has: the texts are in the same order as the recordings.
//!
//! This module pays for the transcription **once per file**, then places the
//! texts along one global token stream in `order_num`: first the texts long
//! enough to be found on their own, then everything in the gaps those leave. Each
//! text yields a span, the spans are written down as *proposals*, and `apply`
//! stores them — as rows, with the wav cut later, the first time something needs
//! to hear it. There is nothing to judge in between: a span the model placed is
//! the answer, and there is only a clip to save (and a clip to delete when the
//! span has to change, because a save never overwrites one).
//!
//! ## Why two passes and not one walk
//!
//! The first version walked the texts with a cursor that only moved forward
//! and aligned each one against a window growing from that cursor. On a real
//! audiobook (784 mails, 34 files, 45 000 heard words) it placed five of them.
//! Three things were wrong, and each one on its own is enough to lose a dataset:
//!
//! * the alignment was **global**, so a fifteen-word mail aligned against a
//!   four-minute window had to delete three quarters of the window, and paid for
//!   it with the words it should have matched — 7 matches where 11 were heard;
//! * a wider window made that *worse*, but the wider answer overwrote the
//!   narrower one, so refusing was the only way out of the loop;
//! * one text placed in the wrong place moved the cursor past the texts
//!   that came after it, and everything behind the cursor refused. 500 of the
//!   784 refusals in that run were one bad span in the first third of the book.
//!
//! So the guess no longer trusts a cursor. It trusts **reading order**: a text's
//! position is predicted from the words counted before it (`prior_position`),
//! a semi-global alignment asks "where in this window is this text" without
//! paying for the rest of the window, and every text's span is decided
//! against that prediction rather than against where its neighbour happened to
//! land. The texts too short to be evidence on their own are then aligned as
//! one block inside the gap their neighbours leave, which cannot put them out of
//! order even when the phrase they consist of occurs twenty times in the book.
//!
//! ## Storage and the rules
//!
//! The cache layout under `<dataset>/align/`, the four rules that follow from
//! it — delete first, no verdict, the derived index never syncs, one writer per
//! fact — and the eight commands have one written home:
//! `docs/design/read-aloud-align.md`. In short: `register` → `transcribe` (once
//! per file) → `run` → `apply`, each step idempotent and resumable and every one
//! of them a tool, so an agent drives the same pipeline a human does in the
//! Advanced panel.


mod index;
mod place;
mod proposals;
mod writes;

pub(crate) use index::{
    present_sources, register, source_file, source_identity, transcribe, transcript,
};
pub(crate) use place::{run, run_one};
pub(crate) use proposals::{list, move_span};
pub(crate) use writes::apply;

use serde_json::Value;
use tauri::State;

use crate::settings::SettingsState;


// ============================================================
// Constants
// ============================================================

/// Extensions the registration scan accepts. A list rather than "anything the
/// decoder opens", because a chapter directory usually also holds cover art and
/// a cue sheet, and a bogus entry costs a failed transcription to discover.
const AUDIO_EXTENSIONS: [&str; 12] = [
    "mp3", "m4a", "m4b", "aac", "wav", "flac", "ogg", "oga", "opus", "wma", "mp4", "mkv",
];

/// Width of one peak-envelope bucket. Short enough that a syllable boundary is
/// visible at the crop edges, long enough that ten hours is 1.8M pairs.
const PEAK_SECONDS: f64 = 0.02;

/// How far from its predicted position a text is looked for. The tight value
/// is the common case and also what keeps a five-word one-liner from being
/// matched to the same phrase twenty minutes later; the wide one is the retry
/// for a text the prediction put more than a page away.
const PRIOR_SLACK_TOKENS: usize = 400;
const PRIOR_WIDE_SLACK_TOKENS: usize = 1500;

/// Longest window one semi-global pass aligns. The matrix costs a byte per cell,
/// so a text of several hundred words must not be allowed to ask for a window
/// the size of the whole book.
const MAX_WINDOW_TOKENS: usize = 12_000;

/// A text with fewer words than this is not evidence of anything by itself:
/// "40 Sekunden später" is read a dozen times in a 20-hour book, so aligning it
/// alone finds *an* occurrence, not *the* one. Those are placed as a block
/// inside the gap their longer neighbours leave.
const ANCHOR_MIN_WORDS: usize = 12;

/// What a text long enough to divide a gap must still have been heard as. Below
/// this it is a hint at a span, not a fact about where the book is.
const ANCHOR_MIN_MATCH_RATIO: f64 = 0.7;

/// How many tokens of distance from the predicted position one matched word is
/// worth when two runs compete for the same text. At 0.005 a run 400 tokens
/// nearer the prediction beats a run that merely matched two more words, which
/// is the point: both are plausible alignments, and only one is in the right
/// part of the book.
const PRIOR_DISTANCE_PENALTY: f64 = 0.005;

/// Shortest span a proposal may describe. Below this the two edges are a
/// mis-drag, not a reading.
const MIN_SPAN_SECS: f64 = 0.3;

/// How many peak pairs one proposal's preview carries. Enough for a readable
/// thumbnail, small enough that forty of them are still one response.
const PREVIEW_PAIRS: usize = 400;

/// How much of the surrounding audio a preview keeps beyond the span itself:
/// thirty per cent of the span's own length on each side, so the words either
/// side of an edge are in the window and a longer text buys proportionally
/// more context. The fixed quarter second this replaced showed the edge and
/// nothing around it.
const PREVIEW_MARGIN_RATIO: f64 = 0.3;

/// The floor under that ratio: a two-word one-liner still gets a second of
/// context each side, or there is nothing to judge its edges against.
const PREVIEW_MARGIN_MIN_SECS: f64 = 1.0;

/// An edge is judged in milliseconds, so the three files that produce one round
/// it the same way — a nudge is not allowed to lose precision to its writer.
fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

// ============================================================
// Commands
// ============================================================

/// Register (or list) the recordings that hold this dataset's texts.
#[tauri::command]
pub async fn read_aloud_align_sources(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    directory: Option<String>,
) -> Result<Value, String> {
    register(&settings, &dataset_uuid, directory.as_deref())
}

/// Cache the word timings of one recording. Repeat until `remaining` is 0.
#[cfg(feature = "stt")]
#[tauri::command]
pub async fn read_aloud_align_transcribe(
    settings: State<'_, SettingsState>,
    model_state: State<'_, crate::models::ModelState>,
    dataset_uuid: String,
    source_uuid: Option<String>,
) -> Result<Value, String> {
    transcribe(
        model_state.inner(),
        &settings,
        &dataset_uuid,
        source_uuid.as_deref(),
    )
}

#[cfg(not(feature = "stt"))]
#[tauri::command]
pub async fn read_aloud_align_transcribe(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    source_uuid: Option<String>,
) -> Result<Value, String> {
    transcribe(&settings, &dataset_uuid, source_uuid.as_deref())
}

/// Place every text along the recordings, in order, and write the proposals.
#[tauri::command]
pub async fn read_aloud_align_guess(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Value, String> {
    run(&settings, &dataset_uuid)
}

/// Re-place one text, leaving every other row of the proposal set untouched.
#[tauri::command]
pub async fn read_aloud_align_guess_one(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
) -> Result<Value, String> {
    run_one(&settings, &dataset_uuid, &text_uuid)
}

/// The proposals with their texts and a thumbnail of the audio around each span.
#[tauri::command]
pub async fn read_aloud_align_proposals(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
) -> Result<Value, String> {
    list(&settings, &dataset_uuid)
}

/// One recording's cached transcript, for the result pane.
#[tauri::command]
pub async fn read_aloud_align_transcript(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    source_uuid: String,
) -> Result<Value, String> {
    transcript(&settings, &dataset_uuid, &source_uuid)
}

/// Move one placed text's span by hand: the page's nudges and typed edges.
#[tauri::command]
pub async fn read_aloud_align_move_span(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
    start: f64,
    end: f64,
) -> Result<Value, String> {
    move_span(&settings, &dataset_uuid, &text_uuid, start, end)
}

/// Cut the spans that have no clip yet into standard audio. `text_uuid` saves
/// one text; omitting it saves every text that still has none.
#[tauri::command]
pub async fn read_aloud_align_apply(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    limit: Option<i64>,
    text_uuid: Option<String>,
) -> Result<Value, String> {
    apply(&settings, &dataset_uuid, limit, text_uuid.as_deref())
}


#[cfg(test)]
mod tests {
    use super::super::edges;
    use super::super::standard_audio::GUESS_PAD_BEFORE;
    use super::index::{decode_peaks, encode_peaks, natural_cmp};
    use super::place::prior_position;
    #[cfg(feature = "stt")]
    use super::place::{find_predicted, need_for, place_all, runs_of};
    use super::*;

    /// Chapters ripped with and without zero padding have to agree: a person
    /// reads `ch2` before `ch10`, and so must the stream.
    #[test]
    fn file_names_sort_the_way_they_are_read() {
        let mut names = vec!["ch10.mp3", "ch2.mp3", "Chapter 1.mp3", "ch1.mp3", "ch02.mp3"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec!["ch1.mp3", "ch2.mp3", "ch02.mp3", "ch10.mp3", "Chapter 1.mp3"]
        );
    }

    /// The envelope is stored as base64 of raw `i8`; a round trip must not
    /// change a single bucket, -128 included.
    #[test]
    fn peaks_survive_the_round_trip() {
        let peaks = vec![-128i8, 127, 0, 0, -1, 1, 5, -5];
        assert_eq!(decode_peaks(&encode_peaks(&peaks)), peaks);
    }

    /// Padding, the minimum length, and the monotonic clamp — the three ways a
    /// span can be the wrong size. Where an edge lands inside the pause it
    /// meets is `edges`' question; the bounds this call site hands it are what
    /// keeps two clips apart.
    #[test]
    fn a_span_is_padded_and_never_overlaps_the_one_before() {
        let loud = vec![100i8; 20_000];
        let loudness = edges::loudness_from_envelope(&loud);
        let start = round_ms(edges::place_edge(
            &loudness,
            edges::EDGE_FRAME_SECS,
            10.0,
            GUESS_PAD_BEFORE,
            edges::Side::Head,
            0.0,
            100.0,
        ));
        assert!((start - 9.8).abs() < 1e-9, "start {start}");

        // The previous text ends at 10.5, so this one may not start before it.
        let start = round_ms(edges::place_edge(
            &loudness,
            edges::EDGE_FRAME_SECS,
            10.0,
            GUESS_PAD_BEFORE,
            edges::Side::Head,
            10.5,
            100.0,
        ));
        assert!((start - 10.5).abs() < 1e-9, "start {start} overlaps the previous text");
    }

    /// Milli precision, so a typed 12.345 s is stored as 12.345 s.
    #[test]
    fn spans_round_to_milliseconds() {
        assert!((round_ms(12.3454) - 12.345).abs() < 1e-9);
        assert!((round_ms(12.3456) - 12.346).abs() < 1e-9);
    }

    /// Reading order maps word counts onto stream positions, and the answer can
    /// never point past the last heard token.
    #[test]
    fn the_prior_follows_the_word_count() {
        assert_eq!(prior_position(0, 100, 1000), 0);
        assert_eq!(prior_position(50, 100, 1000), 500);
        assert_eq!(prior_position(100, 100, 1000), 999);
        assert_eq!(prior_position(5, 0, 1000), 0);
    }

    /// The predicted window decides *which* occurrence: a text that occurs
    /// once near the start and once far later belongs to the one reading order
    /// points at, even though the earlier one scores just as well.
    #[cfg(feature = "stt")]
    #[test]
    fn the_prediction_picks_the_occurrence_in_reading_order() {
        let reference: Vec<String> = (0..15).map(|i| format!("w{}", i)).collect();
        let mut stream: Vec<String> = (0..6000).map(|i| format!("x{}", i)).collect();
        for offset in [200usize, 5000] {
            for (index, token) in reference.iter().enumerate() {
                stream[offset + index] = token.clone();
            }
        }
        let need = need_for(&reference);
        let found = find_predicted(&reference, &stream, 5000, need).hit.expect("the text is there");
        assert_eq!(found.from, 5000, "placed at {} instead of the predicted 5000", found.from);
        assert_eq!(found.matched, 15);
    }

    /// A transcript that repeats itself — what an overlapping STT chunk did at
    /// every four-minute boundary — is a hole in the audio, not in the text. The
    /// text is still one reading either side of it, so the step over the repeat
    /// must not split the span in two: the longer half would win the run score and
    /// the text would be placed a minute past its own first word.
    #[cfg(feature = "stt")]
    #[test]
    fn a_repeated_transcript_does_not_split_the_span() {
        // Fifteen ref tokens in two stretches, the second of them heard eleven
        // tokens further on because the repeat sits between them.
        let pairs: Vec<(usize, usize)> = (0..9)
            .map(|i| (i, 10 + i))
            .chain((9..15).map(|i| (i, 20 + i)))
            .collect();
        assert_eq!(runs_of(&pairs), vec![(10, 34, 15)]);

        // A hole in the *text* is still a boundary: those words are read
        // somewhere else, and each stretch is its own claim about where.
        let pairs: Vec<(usize, usize)> = (0..9)
            .map(|i| (i, 10 + i))
            .chain((20..25).map(|i| (i, 40 + i)))
            .collect();
        assert_eq!(runs_of(&pairs), vec![(10, 18, 9), (60, 64, 5)]);
    }

    /// The gap between two anchors is where the one-liners go: a phrase that
    /// occurs a dozen times in the book still lands between its neighbours,
    /// because a block alignment cannot put it anywhere else.
    #[cfg(feature = "stt")]
    #[test]
    fn the_gap_fill_keeps_a_one_liner_between_its_neighbours() {
        let words = |prefix: &str| -> Vec<String> {
            (0..14).map(|i| format!("{prefix}{i}")).collect()
        };
        let texts = vec![words("a"), ["ja", "ja", "ja"].map(String::from).to_vec(), words("c")];

        let mut stream: Vec<String> = (0..100).map(|i| format!("x{}", i)).collect();
        stream.extend(texts[0].iter().cloned());
        stream.extend((0..40).map(|i| format!("y{}", i)));
        stream.extend(texts[1].iter().cloned());
        stream.extend((0..40).map(|i| format!("z{}", i)));
        stream.extend(texts[2].iter().cloned());
        stream.extend((0..100).map(|i| format!("q{}", i)));

        let slots = place_all(&stream, &texts);
        let first = slots[0].span.expect("the long first text is placed");
        let middle = slots[1].span.expect("the one-liner is placed in the gap");
        let last = slots[2].span.expect("the long last text is placed");
        assert_eq!((first.from, first.to), (100, 113));
        assert_eq!((middle.from, middle.to), (154, 156));
        assert_eq!((last.from, last.to), (197, 210));
    }

    /// A gap that does not hold the texts claiming to be in it leaves them
    /// unplaced rather than stretching them across the audio it does hold.
    #[cfg(feature = "stt")]
    #[test]
    fn a_gap_that_holds_nothing_refuses_instead_of_stretching() {
        let words = |prefix: &str| -> Vec<String> {
            (0..14).map(|i| format!("{prefix}{i}")).collect()
        };
        let texts = vec![words("a"), words("m"), words("c")];

        let mut stream: Vec<String> = (0..100).map(|i| format!("x{}", i)).collect();
        stream.extend(texts[0].iter().cloned());
        stream.extend((0..40).map(|i| format!("y{}", i)));
        stream.extend(texts[2].iter().cloned());
        stream.extend((0..100).map(|i| format!("q{}", i)));

        let slots = place_all(&stream, &texts);
        assert!(slots[0].span.is_some() && slots[2].span.is_some());
        assert!(slots[1].span.is_none(), "the missing text was placed at {:?}", slots[1].span);
    }
}
