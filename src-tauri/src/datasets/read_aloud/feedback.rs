//! Feedback on one attempt: turn the measurements into advice.
//!
//! Read Aloud measures three axes and stops there. The numbers answer *did you
//! say the right words*; they never answer *what should I do differently* —
//! this module is that second half. And a report about a reading needs the
//! reading: the app's text-only LLM seam cannot carry audio, so the feedback
//! route is the agent mailbox — the panel's "Ask agent" posts the evidence, the
//! prompt and the recording to an attached agent that can hear the take.
//!
//! One prompt, two callers:
//!
//! * [`request_feedback`] — the panel's "Ask agent" button: it builds the
//!   context and posts it to the mailbox. The agent claims the row, calls
//!   [`context`] (which carries `audio.path` and, on request, the take's bytes
//!   as base64), and answers through [`save_report`], which writes into the
//!   same UI card and closes the request.
//!
//! The report is derived output: `feedback_json` on the attempt row, device
//! local, deliberately outside the sync journal. It costs one click to
//! regenerate, and every device would produce a different one from the same
//! evidence — so there is nothing worth shipping.
//!
//! Prompt-in-the-backend follows `normalize.rs`: the in-app call, the
//! "copy the prompt" route and the MCP tool must send identical words.

use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::datasets::asr_metric::{normalize_words, score_asr, AsrOp, AsrScoreSummary};
use crate::datasets::{now_row_stamp, read_info_opt};
use crate::mail::{self, NewRequest, ORIGIN_USER};
use crate::settings::SettingsState;

use super::assess::{Assessment, PronReason};
use super::fluency::{PauseKind, RateBand};
use super::open_attempts;

/// Bumped when the stored shape changes incompatibly. Readers default every
/// field, so an older report still renders.
pub(crate) const SCHEMA_VERSION: i64 = 1;

/// Language the model is asked to answer in when the caller names none.
const DEFAULT_LANGUAGE: &str = "en";

/// Caps, so one stored report stays a report and not a transcript of the
/// model's reasoning.
const MAX_ISSUES: usize = 5;
const MAX_FOCUS_WORDS: usize = 8;
const MAX_PHONE_WORDS: usize = 6;
const MAX_PHONES_PER_WORD: usize = 2;
const MAX_ALIGNMENT_RUNS: usize = 12;
const MAX_EVIDENCE_PAUSES: usize = 10;
const MAX_PASSAGE_CHARS: usize = 4000;
const MAX_SUMMARY_CHARS: usize = 4000;
const MAX_DIAGNOSIS_CHARS: usize = 600;
const MAX_DRILL_CHARS: usize = 2000;
const MAX_NEXT_TRY_CHARS: usize = 500;
/// A stored report bigger than this is not a report.
const MAX_REPORT_BYTES: usize = 32 * 1024;

/// Below this a word's pronunciation is worth naming in the evidence.
const WEAK_WORD_SCORE: f64 = 0.4;
/// Below this a word is worth naming in the phone detail (same threshold the
/// UI highlights: it is early enough to explain, late enough not to be noise).
const WEAK_PHONE_WORD_SCORE: f64 = 0.6;

const LANGUAGE_PLACEHOLDER: &str = "{{language}}";

// ============================================================
// The stored report
// ============================================================

/// One report, stored verbatim as `feedback_json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedbackReport {
    #[serde(default = "default_schema_version")]
    pub schema_version: i64,
    /// Three or four sentences: what the numbers say about this take.
    pub summary: String,
    /// The single biggest cause of the lost points, when the model can name one.
    #[serde(default)]
    pub diagnosis: Option<String>,
    #[serde(default)]
    pub issues: Vec<FeedbackIssue>,
    #[serde(default)]
    pub focus_words: Vec<FeedbackFocusWord>,
    /// A two-to-five minute practice plan (markdown allowed).
    #[serde(default)]
    pub drill: Option<String>,
    #[serde(default)]
    pub next_try: String,
    /// `"llm"` for the in-app route, `"agent"` for a report written back
    /// through `read_aloud_save_feedback`.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub created_at: String,
    /// `"report"` for the structured answer, `"text"` when the model replied in
    /// prose and the answer was kept rather than thrown away.
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_schema_version() -> i64 {
    SCHEMA_VERSION
}

fn default_format() -> String {
    "report".to_string()
}

/// One thing to fix, with the evidence it came from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FeedbackIssue {
    /// `accuracy` | `fluency` | `pronunciation`.
    #[serde(default)]
    pub area: String,
    /// The measurement being explained, quoted from the evidence.
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub fix: String,
    #[serde(default)]
    pub drill: Option<String>,
}

/// One word to work on, with the sound that went wrong when it is known.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FeedbackFocusWord {
    #[serde(default)]
    pub word: String,
    /// Target IPA for the word, when the pronunciation pass measured it.
    #[serde(default)]
    pub ipa: Option<String>,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub score: Option<f64>,
}

impl FeedbackReport {
    fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            summary: String::new(),
            diagnosis: None,
            issues: Vec::new(),
            focus_words: Vec::new(),
            drill: None,
            next_try: String::new(),
            source: String::new(),
            model: None,
            created_at: String::new(),
            format: default_format(),
        }
    }
}

// ============================================================
// Evidence: what the scorer knows, rendered as facts
// ============================================================

/// The recording behind an attempt, as facts a report can quote. The take
/// itself is handed to the model through the agent mailbox — this app's LLM
/// seam carries text only.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioFacts {
    /// Absolute path on this device, when the wav is here.
    pub path: Option<String>,
    /// The wav exists and can be handed to a model that can hear.
    pub available: bool,
    pub bytes: Option<u64>,
    pub duration: Option<f64>,
    pub format: String,
}

/// One weak phone inside a weak word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoneNote {
    /// The phone the reference asked for.
    pub ipa: String,
    /// Its GOP score in `(0, 1]` — higher is clearer.
    pub score: f64,
    /// What the acoustic model heard instead, when it heard a single phone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heard: Option<String>,
}

/// The phone-level detail of one word, recomputed on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordPhones {
    pub word: String,
    /// The whole word's target IPA, phones joined by a space.
    pub ipa: String,
    /// The word's GOP score in `(0, 1]`.
    pub score: f64,
    /// Its weakest phones, worst first.
    pub weak: Vec<PhoneNote>,
}

/// Everything one attempt can tell a model, plus the prompt that asks for the
/// report. Shared by the command route and the agent mailbox, so both see
/// exactly the same evidence.
#[derive(Debug, Clone, Serialize)]
pub struct FeedbackContext {
    /// Identity of the attempt: uuid, text, score, transcript, recording path.
    pub attempt: Value,
    /// The reference the scorer compared against (normalized when one exists).
    pub passage: String,
    /// The dataset's language tag, empty when it has none.
    pub language: String,
    /// The language the report is asked to be written in.
    pub feedback_language: String,
    pub audio: AudioFacts,
    /// The stored assessment, verbatim, when the take has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assessment: Option<Value>,
    /// Weak words with their weakest phones, when they could be recomputed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_detail: Option<Vec<WordPhones>>,
    /// The rendered evidence block the prompt quotes.
    pub evidence: String,
    pub prompt: String,
}

/// One attempt as the feedback paths need it.
struct AttemptFacts {
    text_uuid: String,
    audio_path: Option<String>,
    recognized: String,
    score: f64,
    metrics: Option<Assessment>,
    created_at: String,
}

fn load_attempt(
    settings: &SettingsState,
    dataset_uuid: &str,
    attempt_uuid: &str,
    expect_text: Option<&str>,
) -> Result<AttemptFacts, String> {
    let facts = {
        let ctx = open_attempts(settings, dataset_uuid)?;
        ctx.conn
            .query_row(
                "SELECT a.text_uuid, a.audio_path, a.recognized, a.score, a.metrics_json, a.created_at \
                 FROM app.read_attempt_event a WHERE a.uuid = ?1 AND a.user_id = ?2",
                params![attempt_uuid, &ctx.me],
                |row| {
                    let raw: Option<String> = row.get(4)?;
                    Ok(AttemptFacts {
                        text_uuid: row.get(0)?,
                        audio_path: row.get(1)?,
                        recognized: row.get(2)?,
                        score: row.get(3)?,
                        metrics: raw.and_then(|s| serde_json::from_str::<Assessment>(&s).ok()),
                        created_at: row.get(5)?,
                    })
                },
            )
            .map_err(|_| {
                format!(
                    "Attempt {} not found for this user. List them with read_aloud_list_attempts \
                     and pass an attempt uuid you own.",
                    attempt_uuid
                )
            })?
    };
    if let Some(text_uuid) = expect_text.filter(|t| !t.is_empty()) {
        if text_uuid != facts.text_uuid {
            return Err(format!(
                "Attempt {} belongs to text {}, not {}. Drop text_uuid or pass the attempt's own.",
                attempt_uuid, facts.text_uuid, text_uuid
            ));
        }
    }
    Ok(facts)
}

/// The reference the scorer compared against: `normalized_content` when set,
/// the passage otherwise.
fn passage_for(settings: &SettingsState, dataset_uuid: &str, text_uuid: &str) -> Result<String, String> {
    let dir = super::find_dataset_dir(settings, dataset_uuid)?;
    super::text_reference(&dir, text_uuid)
}

fn dataset_language(settings: &SettingsState, dataset_uuid: &str) -> String {
    super::find_dataset_dir(settings, dataset_uuid)
        .ok()
        .and_then(|dir| read_info_opt(&dir))
        .map(|info| info.language)
        .unwrap_or_default()
}

fn audio_facts(
    settings: &SettingsState,
    dataset_uuid: &str,
    audio_path: &Option<String>,
    duration: Option<f64>,
) -> AudioFacts {
    let mut facts = AudioFacts { duration, format: "wav".to_string(), ..Default::default() };
    let Some(rel) = audio_path.as_ref().filter(|p| !p.is_empty()) else {
        return facts;
    };
    if let Some(ext) = Path::new(rel).extension() {
        facts.format = ext.to_string_lossy().to_lowercase();
    }
    let Ok(dir) = super::find_dataset_dir(settings, dataset_uuid) else {
        return facts;
    };
    let Some(path) = super::resolve_audio_url(&dir, &super::audio_dir(settings), audio_path) else {
        return facts;
    };
    facts.bytes = std::fs::metadata(&path).ok().map(|m| m.len());
    facts.available = true;
    facts.path = Some(path);
    facts
}

/// A block quoted verbatim, shortened so one huge passage cannot crowd out the
/// measurements the report has to explain.
fn quoted(text: &str, limit: usize) -> String {
    let flat = text.trim();
    if flat.is_empty() {
        return "(empty)".to_string();
    }
    let mut body: String = flat.chars().take(limit).collect();
    if flat.chars().count() > limit {
        body.push('…');
    }
    format!("\"\"\"\n{}\n\"\"\"", body)
}

fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let mut body: String = trimmed.chars().take(limit.saturating_sub(1)).collect();
    body.push('…');
    body
}

/// One line per alignment run of a given op, capped.
fn alignment_runs(summary: &AsrScoreSummary, op: AsrOp) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for chunk in &summary.chunks {
        if chunk.op != op {
            continue;
        }
        let line = match op {
            AsrOp::Substitute => format!("\"{}\" heard as \"{}\"", chunk.ref_word, chunk.hyp_word),
            AsrOp::Delete => format!("\"{}\"", chunk.ref_word),
            AsrOp::Insert => format!("\"{}\"", chunk.hyp_word),
            AsrOp::Equal => continue,
        };
        out.push(line);
        if out.len() >= MAX_ALIGNMENT_RUNS {
            break;
        }
    }
    out
}

fn band_word(band: RateBand) -> &'static str {
    match band {
        RateBand::Slow => "slow",
        RateBand::Target => "at a steady pace",
        RateBand::Fast => "fast",
    }
}

fn pause_word(kind: PauseKind) -> &'static str {
    match kind {
        PauseKind::AtPunctuation => "at punctuation",
        PauseKind::WithinPhrase => "mid-phrase",
        PauseKind::Leading => "before the first word",
        PauseKind::Trailing => "after the last word",
    }
}

/// Why an axis is missing, in the words a report can hand back to a learner.
pub(crate) fn describe_reason(reason: PronReason) -> &'static str {
    match reason {
        PronReason::ModelMissing => {
            "no pronunciation model is installed, so the sounds were never compared with the reference"
        }
        PronReason::ModelError => "the pronunciation model failed to run on this take",
        PronReason::NoReference => {
            "this text has no standard audio, so there is nothing to compare the sounds against"
        }
        PronReason::StaleReference => {
            "the standard audio was made from an earlier version of the text, so it cannot be compared"
        }
        PronReason::NoNetwork => "the reference timeline was unavailable when this take was scored",
        PronReason::TtsFailed => "the reference audio could not be analysed when this take was scored",
        PronReason::TooShort => "the take is too short to align against the reference",
    }
}

/// The evidence block. Pure on purpose: every fact comes from the arguments, so
/// the rendering is unit-testable without a model, a recording or a network.
fn build_evidence(
    target_language: &str,
    feedback_language: &str,
    passage: &str,
    summary: &AsrScoreSummary,
    stored: Option<&Assessment>,
    phone: Option<&[WordPhones]>,
    audio: &AudioFacts,
) -> String {
    let mut out = String::new();
    out.push_str("READ-ALOUD ATTEMPT EVIDENCE\n");
    out.push_str(&format!(
        "Passage language: {}\n",
        if target_language.is_empty() { "unknown" } else { target_language }
    ));
    out.push_str(&format!("Answer language: {}\n\n", feedback_language));

    out.push_str(&format!(
        "PASSAGE (the reference the scorer compared against; {} words)\n{}\n\n",
        normalize_words(passage).len(),
        quoted(passage, MAX_PASSAGE_CHARS)
    ));
    // The take itself, not the recognizer's guess at it: the model that
    // answers this prompt hears the recording, so no transcript is dumped here.
    out.push_str("RECORDING\n");
    if audio.available {
        out.push_str(&format!(
            "- {}{} is passed to you with this evidence — judge the reading by listening to it, \
             not by reading a recognizer's transcript of it\n",
            audio
                .duration
                .map(|d| format!("{:.1} s ", d))
                .unwrap_or_default(),
            audio.format.to_uppercase()
        ));
    } else {
        out.push_str(
            "- the recording is not on this device, so there is nothing to listen to\n",
        );
    }
    out.push_str("\n");

    // --- accuracy ---------------------------------------------------------
    out.push_str("ACCURACY — were the right words said (0-100; below 60 does not pass)\n");
    let counts = stored.map(|a| a.accuracy.counts);
    match counts {
        Some(c) => {
            out.push_str(&format!(
                "- score {:.1} of 100 · lenient {:.1} (near-miss forms counted as correct)\n",
                stored.map(|a| a.accuracy.score).unwrap_or(summary.score),
                stored.map(|a| a.accuracy.lenient_score).unwrap_or(summary.lenient_score)
            ));
            out.push_str(&format!(
                "- {} reference words: {} correct, {} near-miss, {} misread, {} never said, \
                 {} extra, {} repeated · word error rate {:.2}\n",
                c.reference, c.correct, c.minor, c.substituted, c.omitted, c.inserted, c.repeated,
                stored.map(|a| a.accuracy.wer).unwrap_or(summary.wer)
            ));
        }
        None => {
            out.push_str(&format!(
                "- score {:.1} of 100 · lenient {:.1} (near-miss forms counted as correct)\n",
                summary.score, summary.lenient_score
            ));
            out.push_str(&format!(
                "- {} reference words: {} correct, {} near-miss, {} misread, {} never said, \
                 {} extra, {} repeated · word error rate {:.2}\n",
                summary.ref_len, summary.hits, summary.minor, summary.substitutions,
                summary.deletions, summary.insertions, summary.repeated, summary.wer
            ));
        }
    }
    let mut wrote_runs = false;
    for (op, label) in [
        (AsrOp::Substitute, "misread"),
        (AsrOp::Delete, "never said"),
        (AsrOp::Insert, "said but not in the passage"),
    ] {
        let runs = alignment_runs(summary, op);
        if runs.is_empty() {
            continue;
        }
        wrote_runs = true;
        out.push_str(&format!("- {}: {}\n", label, runs.join("; ")));
    }
    if !wrote_runs {
        out.push_str("- the transcript matched the passage word for word\n");
    }
    if let Some(stored_score) = stored.map(|a| a.accuracy.score) {
        if (stored_score - summary.score).abs() > 1.0 {
            out.push_str(&format!(
                "- note: the passage or the metric changed since this take was scored \
                 (stored {:.1}, recomputed {:.1}); the score above is the stored one and the \
                 word list was recomputed against the current reference\n",
                stored_score, summary.score
            ));
        }
    }
    out.push('\n');

    // --- fluency ----------------------------------------------------------
    out.push_str("FLUENCY — pace and pauses\n");
    match stored.and_then(|a| a.fluency.as_ref()) {
        Some(f) => {
            out.push_str(&format!(
                "- {:.0} correct words per minute ({}) · articulation rate {:.0} words per minute\n",
                f.wcpm,
                band_word(f.band),
                f.articulation_rate
            ));
            out.push_str(&format!(
                "- {} pauses ({} long, {:.0}% mid-phrase) · {:.1} words between pauses · \
                 {} single-word repeats, {} restarts\n",
                f.pause_count,
                f.long_pause_count,
                f.within_phrase_pause_ratio * 100.0,
                f.mean_length_of_run,
                f.repetition_count,
                f.restart_count
            ));
            out.push_str(&format!(
                "- reading span {:.1} s inside a {:.1} s recording\n",
                f.speaking_time, f.duration
            ));
        }
        None => out.push_str(
            "- not measured: the speech model gave no word timings for this take, so pace and \
             pauses cannot be judged. Say so; do not guess.\n",
        ),
    }
    out.push('\n');

    // --- pronunciation ----------------------------------------------------
    out.push_str("PRONUNCIATION — how clearly the sounds came out (0-100)\n");
    match stored.map(|a| &a.pronunciation) {
        Some(p) if p.score.is_some() => {
            let scored = stored.map(|a| a.words.iter().filter(|w| w.pron.is_some()).count()).unwrap_or(0);
            out.push_str(&format!(
                "- score {:.1} of 100 · {:.0}% of the {} measured words are weak (GOP below {})\n",
                p.score.unwrap_or(0.0) * 100.0,
                p.weak_ratio.unwrap_or(0.0) * 100.0,
                scored,
                WEAK_WORD_SCORE
            ));
            let mut weak: Vec<(String, f64)> = stored
                .map(|a| {
                    a.words
                        .iter()
                        .filter_map(|w| w.pron.map(|p| (w.ref_word.clone(), p)))
                        .collect()
                })
                .unwrap_or_default();
            weak.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            weak.truncate(MAX_FOCUS_WORDS);
            if !weak.is_empty() {
                let listed: Vec<String> = weak
                    .iter()
                    .map(|(word, score)| format!("\"{}\" {:.2}", word, score))
                    .collect();
                out.push_str(&format!("- weakest words (0-1, higher is clearer): {}\n", listed.join("; ")));
            }
        }
        Some(p) => out.push_str(&format!(
            "- not measured: {}.\n",
            p.unavailable_reason.map(describe_reason).unwrap_or("the reason was not recorded")
        )),
        None => out.push_str(
            "- not measured: this take has no stored pronunciation result. Say so; the learner can \
             re-assess it to measure the sounds.\n",
        ),
    }
    if let Some(words) = phone.filter(|w| !w.is_empty()) {
        out.push_str("- phone detail (\"heard\" is what the acoustic model heard instead):\n");
        for word in words {
            let mut line = format!("  - \"{}\" /{}/ {:.2}", word.word, word.ipa, word.score);
            let weak: Vec<String> = word
                .weak
                .iter()
                .map(|phone| match &phone.heard {
                    Some(heard) if !heard.is_empty() => {
                        format!("/{}/ {:.2} (heard /{}/)", phone.ipa, phone.score, heard)
                    }
                    _ => format!("/{}/ {:.2}", phone.ipa, phone.score),
                })
                .collect();
            if !weak.is_empty() {
                line.push_str(&format!(" — weak {}", weak.join(", ")));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.push('\n');

    // --- pauses -----------------------------------------------------------
    out.push_str("PAUSES — where the silences fell\n");
    match stored {
        Some(a) if !a.pauses.is_empty() => {
            let ref_words = normalize_words(passage);
            for pause in a.pauses.iter().take(MAX_EVIDENCE_PAUSES) {
                let after = pause
                    .after_ref
                    .and_then(|i| ref_words.get(i))
                    .map(|w| format!(" after \"{}\"", w.norm))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "- {:.2} s at {:.1} s ({}){}\n",
                    (pause.end - pause.start).max(0.0),
                    pause.start,
                    pause_word(pause.kind),
                    after
                ));
            }
            if a.pauses.len() > MAX_EVIDENCE_PAUSES {
                out.push_str(&format!("- and {} more\n", a.pauses.len() - MAX_EVIDENCE_PAUSES));
            }
        }
        Some(_) => out.push_str("- no pauses were detected\n"),
        None => out.push_str("- not measured (no word timings for this take)\n"),
    }
    out.push('\n');

    out
}

// ============================================================
// The prompt
// ============================================================

/// The instruction half of the prompt. Everything it forbids is a failure mode
/// seen from a small local model: invented numbers, advice on comprehension,
/// a lecture instead of a fix.
const INSTRUCTION: &str = r#"You are reviewing one read-aloud practice take in a language-learning app. A scorer has already measured the take; your job is to explain the numbers and say what to do differently.

Rules:
- The evidence below is measured fact. Never invent numbers, words, sounds or timings that are not in it, and quote the ones you use.
- When an axis says "not measured", say that plainly. Do not guess what the learner did.
- Judge pronunciation, rhythm and accuracy of the reading. Do not comment on comprehension, grammar, the passage's content, or what the learner should read next.
- Every issue must quote the evidence it comes from (a word, a count, a phone, a time).
- Every fix must be doable in under five minutes, with no app, tool or partner.
- Keep target-language words in the passage's language, in quotes.
- Write the whole answer in {{language}}.
- Reply with JSON only, no preamble, no code fences, in this shape:
{
  "summary": "3-4 sentences on what these numbers say about this take",
  "diagnosis": "one sentence: the single biggest cause of the lost points",
  "issues": [
    {"area": "accuracy|fluency|pronunciation", "evidence": "the measurement you are explaining", "detail": "what it means / what it probably sounded like", "fix": "what to do differently", "drill": "an optional 30-60 second exercise"}
  ],
  "focus_words": [{"word": "...", "ipa": "/.../", "hint": "how to say it", "score": 0.22}],
  "drill": "a 2-5 minute practice plan; markdown is allowed",
  "next_try": "one sentence to keep in mind for the next recording"
}

Limits: at most 5 issues, worst first; at most 8 focus words, taken from the evidence.
"#;

/// The prompt for one attempt: the instruction plus the evidence it talks
/// about. `language` is a BCP-47 tag from the UI (`en`, `de`).
pub(crate) fn build_prompt(evidence: &str, language: &str) -> String {
    let instruction = INSTRUCTION.replace(LANGUAGE_PLACEHOLDER, language);
    format!("{instruction}\n{evidence}")
}

// ============================================================
// Parsing — tolerant, because small local models decorate
// ============================================================

/// ` ```json … ``` ` around an answer that was told not to be fenced.
fn strip_fences(raw: &str) -> String {
    let body = raw.trim();
    let body = body
        .strip_prefix("```")
        .map(|rest| rest.split_once('\n').map_or(rest, |(_, r)| r))
        .unwrap_or(body);
    let body = body.trim_end();
    body.strip_suffix("```").unwrap_or(body).trim().to_string()
}

/// The first balanced `{…}` in `text`, ignoring braces inside strings. Prose
/// around the object (or a truncated tail after it) is dropped rather than
/// failing the parse.
fn first_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        let index = start + offset;
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return text.get(start..=index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Trim every field to its budget and drop what carries nothing.
fn normalize_report(mut report: FeedbackReport) -> FeedbackReport {
    report.summary = truncate(&report.summary, MAX_SUMMARY_CHARS);
    report.diagnosis = report
        .diagnosis
        .map(|d| truncate(&d, MAX_DIAGNOSIS_CHARS))
        .filter(|d| !d.is_empty());
    report.drill = report
        .drill
        .map(|d| truncate(&d, MAX_DRILL_CHARS))
        .filter(|d| !d.is_empty());
    report.next_try = truncate(&report.next_try, MAX_NEXT_TRY_CHARS);
    report.issues = report
        .issues
        .into_iter()
        .map(|mut issue| {
            issue.area = truncate(&issue.area, 40);
            issue.evidence = truncate(&issue.evidence, 300);
            issue.detail = truncate(&issue.detail, 600);
            issue.fix = truncate(&issue.fix, 600);
            issue.drill = issue.drill.map(|d| truncate(&d, 400)).filter(|d| !d.is_empty());
            issue
        })
        .filter(|issue| !issue.fix.is_empty() || !issue.detail.is_empty())
        .take(MAX_ISSUES)
        .collect();
    report.focus_words = report
        .focus_words
        .into_iter()
        .map(|mut word| {
            word.word = truncate(&word.word, 60);
            word.ipa = word.ipa.map(|i| truncate(&i, 60)).filter(|i| !i.is_empty());
            word.hint = truncate(&word.hint, 200);
            word
        })
        .filter(|word| !word.word.is_empty())
        .take(MAX_FOCUS_WORDS)
        .collect();
    report
}

/// The structured answer, or `None` when the reply was not a usable report —
/// which includes JSON that parsed but carried no `summary`.
fn parse_json_report(raw: &str) -> Option<FeedbackReport> {
    let cleaned = strip_fences(raw);
    let candidate = first_object(&cleaned).unwrap_or(cleaned.as_str());
    let mut report: FeedbackReport = serde_json::from_str(candidate).ok()?;
    if report.summary.trim().is_empty() {
        return None;
    }
    if report.format.trim().is_empty() {
        report.format = default_format();
    }
    Some(normalize_report(report))
}

/// The report, or the prose answer kept as one. A model that ignored the JSON
/// instruction still said something useful, and losing it is worse than storing
/// it unstructured.
pub(crate) fn parse_report(raw: &str) -> Result<FeedbackReport, String> {
    if let Some(report) = parse_json_report(raw) {
        return Ok(report);
    }
    let prose = strip_fences(raw);
    if prose.trim().is_empty() {
        return Err(
            "The model returned an empty answer. Try again, choose another model, or copy the \
             prompt and run it with a different provider."
                .to_string(),
        );
    }
    let mut report = FeedbackReport::empty();
    report.summary = truncate(&prose, MAX_SUMMARY_CHARS);
    report.format = "text".to_string();
    Ok(report)
}

/// BCP-47 tag for the answer; anything odd falls back to English rather than
/// sending a directive the model cannot interpret.
fn normalize_language(raw: Option<&str>) -> String {
    let tag = raw.unwrap_or("").trim().replace('_', "-");
    if tag.is_empty() || tag.chars().count() > 35 {
        return DEFAULT_LANGUAGE.to_string();
    }
    if !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return DEFAULT_LANGUAGE.to_string();
    }
    tag
}

// ============================================================
// Phone detail — recomputed, never stored
// ============================================================

/// The words worth naming a sound for: the worst six below 0.6, each with its
/// two weakest phones and what the model heard there.
///
/// Recomputed from the device-local recording rather than stored, which is what
/// keeps [`Assessment`] and its ~2 KB metrics budget untouched. Every missing
/// ingredient (recording, model, standard clip) simply yields no phone detail.
#[cfg(feature = "phoneme")]
async fn phone_detail(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    passage: &str,
    audio_path: &Option<String>,
) -> Option<Vec<WordPhones>> {
    let samples = super::stored_recording(settings, dataset_uuid, audio_path);
    if samples.is_empty() {
        return None;
    }
    let ref_words = normalize_words(passage);
    let clip = super::load_default_clip(settings, dataset_uuid, text_uuid, passage);
    let run = super::pronunciation_evidence(settings, ref_words.len(), &samples, &clip).await;
    if run.reason.is_some() || run.tokens.is_empty() {
        return None;
    }

    let ipa_of = |phones: &[usize]| -> String {
        phones
            .iter()
            .filter_map(|id| run.tokens.get(*id).cloned())
            .collect::<Vec<_>>()
            .join(" ")
    };

    let mut words: Vec<WordPhones> = Vec::new();
    for (index, spans) in run.spans.iter().enumerate() {
        let Some(word) = ref_words.get(index) else { continue };
        let Some(score) = super::gop::word_score(&run.post, spans, super::assess::GOP_TAU) else {
            continue;
        };
        if score >= WEAK_PHONE_WORD_SCORE {
            continue;
        }
        let mut weak: Vec<PhoneNote> = spans
            .iter()
            .filter_map(|span| {
                let ipa = run.tokens.get(span.phone)?.clone();
                let score = super::gop::pronunciation_score(
                    super::gop::gop(&run.post, span),
                    super::assess::GOP_TAU,
                );
                let heard = run
                    .post
                    .argmax_phone(span)
                    .and_then(|id| run.tokens.get(id).cloned());
                Some(PhoneNote { ipa, score, heard })
            })
            .collect();
        weak.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
        weak.truncate(MAX_PHONES_PER_WORD);
        words.push(WordPhones {
            word: word.norm.clone(),
            ipa: ipa_of(&spans.iter().map(|s| s.phone).collect::<Vec<_>>()),
            score,
            weak,
        });
    }
    if words.is_empty() {
        return None;
    }
    words.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
    words.truncate(MAX_PHONE_WORDS);
    Some(words)
}

/// Without the `phoneme` feature there is no acoustic model to ask.
#[cfg(not(feature = "phoneme"))]
async fn phone_detail(
    _settings: &SettingsState,
    _dataset_uuid: &str,
    _text_uuid: &str,
    _passage: &str,
    _audio_path: &Option<String>,
) -> Option<Vec<WordPhones>> {
    None
}

// ============================================================
// Context, mailbox request, storage
// ============================================================

/// Build the evidence + prompt for one attempt. Read-only.
///
/// `language` is whatever tag the caller had — a UI locale, an agent's choice —
/// and is normalized here, so every route (command, MCP tool, mailbox request)
/// sends the model the same directive for the same input.
pub(crate) async fn context(
    settings: &SettingsState,
    dataset_uuid: &str,
    attempt_uuid: &str,
    text_uuid: Option<&str>,
    language: Option<&str>,
) -> Result<FeedbackContext, String> {
    let feedback_language = normalize_language(language);
    let attempt = load_attempt(settings, dataset_uuid, attempt_uuid, text_uuid)?;
    let passage = passage_for(settings, dataset_uuid, &attempt.text_uuid)?;
    let language = dataset_language(settings, dataset_uuid);
    let summary = score_asr(&passage, &attempt.recognized);
    let audio = audio_facts(
        settings,
        dataset_uuid,
        &attempt.audio_path,
        attempt.metrics.as_ref().and_then(|m| m.fluency.as_ref()).map(|f| f.duration),
    );
    let phone = phone_detail(settings, dataset_uuid, &attempt.text_uuid, &passage, &attempt.audio_path).await;
    let evidence = build_evidence(
        &language,
        &feedback_language,
        &passage,
        &summary,
        attempt.metrics.as_ref(),
        phone.as_deref(),
        &audio,
    );
    let prompt = build_prompt(&evidence, &feedback_language);
    Ok(FeedbackContext {
        attempt: json!({
            "uuid": attempt_uuid,
            "text_uuid": attempt.text_uuid,
            "score": attempt.score,
            "recognized": attempt.recognized,
            "audio_path": attempt.audio_path,
            "created_at": attempt.created_at,
        }),
        passage,
        language,
        feedback_language,
        audio,
        assessment: attempt.metrics.as_ref().map(|m| serde_json::to_value(m).unwrap_or(Value::Null)),
        phone_detail: phone,
        evidence,
        prompt,
    })
}

/// Write one report onto the attempt row. Device-local on purpose: no journal,
/// no protocol version, no other device's business.
fn persist(
    settings: &SettingsState,
    attempt_uuid: &str,
    report: &FeedbackReport,
) -> Result<(), String> {
    let raw = serde_json::to_string(report).map_err(|e| e.to_string())?;
    if raw.len() > MAX_REPORT_BYTES {
        return Err(format!(
            "The feedback report is {} bytes, above the {} byte limit. Shorten the drill and the \
             issue list and save again.",
            raw.len(),
            MAX_REPORT_BYTES
        ));
    }
    let me = crate::auth::workspace_identity(settings);
    let conn = super::open_app_attempts(settings)?;
    let changed = conn
        .execute(
            "UPDATE read_attempt_event SET feedback_json = ?1 WHERE uuid = ?2 AND user_id = ?3",
            params![raw, attempt_uuid, &me],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(format!(
            "Attempt {} was not found for this user, so the report was not stored.",
            attempt_uuid
        ));
    }
    Ok(())
}

/// The mailbox's stable address for one attempt's feedback: the agent route
/// needs one open request per take, so "Ask agent" twice reuses the row
/// instead of queueing the same ask twice (§2.2).
fn feedback_subject_key(dataset_uuid: &str, attempt_uuid: &str) -> String {
    format!("read_aloud:{dataset_uuid}:{attempt_uuid}:feedback")
}

/// Post the feedback handoff to the agent mailbox (T1): the exact prompt, the
/// evidence and the recording's location, so the answering model hears the take
/// itself instead of reading a recognizer's transcript. The agent claims the
/// row with `mail_claim`, reads the same context via `read_aloud_feedback_context`
/// — passing `include_audio_base64` to get the bytes inline — and answers with
/// `read_aloud_save_feedback`, citing the request it is closing.
pub(crate) async fn request_feedback(
    settings: &SettingsState,
    dataset_uuid: &str,
    attempt_uuid: &str,
    text_uuid: Option<String>,
    model: Option<String>,
    language: Option<String>,
) -> Result<Value, String> {
    let feedback_language = normalize_language(language.as_deref());
    let ctx =
        context(settings, dataset_uuid, attempt_uuid, text_uuid.as_deref(), Some(&feedback_language))
            .await?;
    if !ctx.audio.available {
        return Err(
            "This attempt's recording is not on this device, so there is nothing to hand an \
             agent to listen to. Ask agent only works for takes this device recorded."
                .to_string(),
        );
    }
    // serde escapes the interpolated ids: a uuid-like string from the caller
    // must not be able to break the payload JSON.
    let payload = json!({
        "dataset_uuid": dataset_uuid,
        "attempt_uuid": attempt_uuid,
        "text_uuid": ctx.attempt.get("text_uuid").and_then(Value::as_str),
        "model": model.as_deref().map(str::trim).filter(|m| !m.is_empty()),
        "language": feedback_language,
        "audio_path": ctx.audio.path,
        "prompt": ctx.prompt,
    })
    .to_string();
    let body = format!(
        "Listen to this read-aloud take and answer the feedback prompt. The recording is at \n         {} — or call \n         read_aloud_feedback_context {{dataset_uuid, attempt_uuid}} with include_audio_base64: true \n         for the bytes inline. Answer the prompt in the request payload's `prompt` field (the same \n         one context() renders) and write the JSON report back with read_aloud_save_feedback, \n         citing mail_uuid and claim_id so this request closes.",
        ctx.audio.path.as_deref().unwrap_or("(see audio.path in the payload)")
    );
    let subject = feedback_subject_key(dataset_uuid, attempt_uuid);
    let channel = format!("read_aloud:{dataset_uuid}:{attempt_uuid}");
    let label = format!("feedback · {}", attempt_uuid.chars().take(8).collect::<String>());
    let request = NewRequest::new(&channel, &subject, &label)
        .body(&body)
        .payload(&payload)
        .resource(Some("llm"))
        .refer(Some("read_attempt"), Some(attempt_uuid))
        .origin(ORIGIN_USER);
    let mut conn = mail::open(settings)?;
    let receipt = mail::request(&mut conn, mail::now_ms(), &request)
        .map_err(|r| match r.successor {
            Some(successor) => format!("{}: {} (replaced by request {successor})", r.code, r.message),
            None => format!("{}: {}", r.code, r.message),
        })?;
    Ok(json!({
        "mail_uuid": receipt.mail.uuid,
        "status": receipt.mail.status,
        "subject": receipt.mail.subject,
        "recording": ctx.audio.path,
    }))
}

/// Store a report produced by the agent route — one that read the mailbox
/// request (and listened to the recording) and is writing its answer back.
///
/// When the delivery cites the request it answers (`mail_uuid` + the `claim_id`
/// from `mail_claim`), the mail row closes: begin_delivery → store →
/// finish_delivery. The report itself is cheap to regenerate, so a lost race on
/// the claim (another agent took the row) costs a journal line, not the answer —
/// it is stored either way rather than thrown away.
pub(crate) fn save_report(
    settings: &SettingsState,
    attempt_uuid: &str,
    report: Value,
    model: Option<String>,
    mail_uuid: Option<String>,
    claim_id: Option<String>,
) -> Result<FeedbackReport, String> {
    let mut parsed: FeedbackReport = serde_json::from_value(report)
        .map_err(|e| format!("The report does not match the feedback schema: {}", e))?;
    parsed.summary = truncate(&parsed.summary, MAX_SUMMARY_CHARS);
    if parsed.summary.is_empty() {
        return Err(
            "The report has no `summary`. Write 3-4 sentences on what the measurements say and \
             send it again."
                .to_string(),
        );
    }
    let mut parsed = normalize_report(parsed);
    parsed.schema_version = SCHEMA_VERSION;
    parsed.source = "agent".to_string();
    parsed.model = model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .or(parsed.model);
    parsed.created_at = now_row_stamp();
    if parsed.format.trim().is_empty() {
        parsed.format = default_format();
    }
    persist(settings, attempt_uuid, &parsed)?;
    if let (Some(mail_uuid), Some(claim_id)) = (
        mail_uuid.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        claim_id.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    ) {
        let conn = mail::open(settings)?;
        let now = mail::now_ms();
        if mail::begin_delivery(&conn, mail_uuid, claim_id, now).unwrap_or(0) == 1 {
            // result_ref names the row the answer landed on.
            let _ = mail::finish_delivery(&conn, mail_uuid, claim_id, attempt_uuid, now);
        } else {
            log::warn!(
                "read_aloud_save_feedback: request {} no longer names this claim; the report was stored but the request stays open for its holder",
                mail_uuid
            );
        }
    }
    Ok(parsed)
}

// ============================================================
// Commands
// ============================================================

/// "Ask agent": post the evidence, the exact prompt and the recording's
/// location to the agent mailbox, so an attached model that can hear answers
/// this take. The recording itself — not a recognizer's transcript — is the
/// handoff. Returns the mail receipt; the report lands later through
/// `read_aloud_save_feedback`.
#[tauri::command]
pub async fn read_aloud_request_feedback(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    attempt_uuid: String,
    text_uuid: Option<String>,
    model: Option<String>,
    language: Option<String>,
) -> Result<Value, String> {
    log::info!(
        "read_aloud_request_feedback: dataset={}, attempt={}, model={:?}",
        dataset_uuid,
        attempt_uuid,
        model
    );
    let receipt =
        request_feedback(&settings, &dataset_uuid, &attempt_uuid, text_uuid, model, language).await?;
    use tauri::Emitter;
    let _ = app.emit(
        "read-aloud-feedback-requested",
        serde_json::json!({
            "dataset_uuid": dataset_uuid,
            "attempt_uuid": attempt_uuid,
        }),
    );
    Ok(receipt)
}

/// The evidence, the exact prompt and the recording's path, for a caller that
/// wants to do the reasoning (and the listening) itself. Read-only.
#[tauri::command]
pub async fn read_aloud_feedback_context(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    attempt_uuid: String,
    text_uuid: Option<String>,
    language: Option<String>,
) -> Result<FeedbackContext, String> {
    context(&settings, &dataset_uuid, &attempt_uuid, text_uuid.as_deref(), language.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::super::assess::{assess, PronEvidence};
    use super::super::fluency::Fluency;
    use super::*;

    fn evidence_for(passage: &str, recognized: &str) -> (AsrScoreSummary, String) {
        let summary = score_asr(passage, recognized);
        let text = build_evidence(
            "de",
            "en",
            passage,
            &summary,
            None,
            None,
            &AudioFacts::default(),
        );
        (summary, text)
    }

    #[test]
    fn the_prompt_quotes_the_real_numbers() {
        let (summary, evidence) = evidence_for("der hund bellt laut", "der hund bellt");
        assert!(evidence.contains(&format!("{:.1} of 100", summary.score)));
        assert!(evidence.contains("1 never said"));
        assert!(evidence.contains("\"laut\""));
        let prompt = build_prompt(&evidence, "de");
        assert!(prompt.contains(&evidence), "the prompt must carry the evidence verbatim");
        assert!(prompt.contains("Write the whole answer in de."));
    }

    #[test]
    fn the_evidence_hands_over_the_recording_not_a_transcript() {
        let (_, evidence) = evidence_for("der hund bellt laut", "der hund bellt");
        assert!(
            !evidence.contains("WHAT THE RECOGNIZER HEARD"),
            "the recognizer's transcript is no longer dumped into the evidence"
        );
        // With the take present, the model is told to listen rather than read it.
        let summary = score_asr("der hund", "der hund");
        let heard = build_evidence(
            "de",
            "en",
            "der hund",
            &summary,
            None,
            None,
            &AudioFacts { available: true, format: "wav".into(), ..Default::default() },
        );
        assert!(heard.contains("judge the reading by listening to it"));
    }

    #[test]
    fn every_unmeasured_axis_says_so() {
        let (_, evidence) = evidence_for("hallo welt", "hallo welt");
        assert!(evidence.contains("not measured: the speech model gave no word timings"));
        assert!(evidence.contains("not measured: this take has no stored pronunciation result"));
        assert!(evidence.contains("nothing to listen to"));
    }

    #[test]
    fn misread_and_inserted_runs_are_named() {
        let (_, evidence) = evidence_for("guten tag wie geht es", "guten tak wie geht es ja");
        assert!(evidence.contains("misread: \"tag\" heard as \"tak\""));
        assert!(evidence.contains("said but not in the passage: \"ja\""));
    }

    #[test]
    fn a_measured_axis_is_rendered_from_the_stored_numbers() {
        let passage = "der hund bellt laut";
        let summary = score_asr(passage, passage);
        let mut assessment = assess(
            &summary,
            passage,
            passage,
            &[],
            None,
            PronEvidence::Unavailable(PronReason::ModelMissing),
        );
        assessment.pronunciation.score = Some(0.63);
        assessment.pronunciation.weak_ratio = Some(0.5);
        assessment.words[0].pron = Some(0.22);
        assessment.fluency = Some(Fluency {
            duration: 4.0,
            speaking_time: 3.2,
            wcpm: 84.0,
            speech_rate: 90.0,
            articulation_rate: 122.0,
            pause_count: 2,
            long_pause_count: 1,
            within_phrase_pause_ratio: 0.5,
            mean_length_of_run: 7.4,
            repetition_count: 1,
            restart_count: 0,
            band: RateBand::Slow,
        });
        let evidence = build_evidence(
            "de",
            "en",
            passage,
            &summary,
            Some(&assessment),
            None,
            &AudioFacts { duration: Some(4.0), available: true, format: "wav".into(), ..Default::default() },
        );
        assert!(evidence.contains("84 correct words per minute (slow)"));
        assert!(evidence.contains("score 63.0 of 100"));
        assert!(evidence.contains("weakest words"));
        assert!(evidence.contains("4.0 s WAV"));
    }

    #[test]
    fn phone_detail_lines_name_the_sound_and_what_was_heard() {
        let (summary, _) = evidence_for("der hund", "der hund");
        let evidence = build_evidence(
            "de",
            "en",
            "der hund",
            &summary,
            None,
            Some(&[WordPhones {
                word: "hund".into(),
                ipa: "h u n t".into(),
                score: 0.31,
                weak: vec![PhoneNote { ipa: "u".into(), score: 0.12, heard: Some("o".into()) }],
            }]),
            &AudioFacts::default(),
        );
        assert!(evidence.contains("\"hund\" /h u n t/ 0.31"));
        assert!(evidence.contains("/u/ 0.12 (heard /o/)"));
    }

    #[test]
    fn json_is_found_inside_fences_and_prose() {
        let raw = "Here you go:\n```json\n{\"summary\": \"Der Hund war zu schnell.\", \"issues\": [], \"focus_words\": []}\n```\nHope that helps.";
        let report = parse_report(raw).expect("parsed");
        assert_eq!(report.summary, "Der Hund war zu schnell.");
        assert_eq!(report.format, "report");
    }

    #[test]
    fn a_truncated_tail_after_the_object_is_ignored() {
        let raw = "{\"summary\": \"Sauber gelesen.\", \"next_try\": \"Tempo halten\"} und dann noch Text";
        let report = parse_report(raw).expect("parsed");
        assert_eq!(report.next_try, "Tempo halten");
    }

    #[test]
    fn prose_survives_as_one_summary() {
        let report = parse_report("  Du hast die Endungen verschluckt. Sprich langsamer.  ")
            .expect("kept");
        assert_eq!(report.format, "text");
        assert!(report.summary.starts_with("Du hast die Endungen"));
    }

    #[test]
    fn an_empty_answer_is_an_error() {
        assert!(parse_report("   \n\n").is_err());
    }

    #[test]
    fn a_report_without_a_summary_is_not_a_report() {
        let raw = r#"{"issues": [{"area": "fluency", "fix": "slow down"}], "next_try": "x"}"#;
        assert!(parse_json_report(raw).is_none());
        // …and the prose fallback keeps it rather than losing it.
        assert_eq!(parse_report(raw).map(|r| r.format).as_deref(), Ok("text"));
    }

    #[test]
    fn caps_hold_on_a_flooded_report() {
        let issues: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"area": "accuracy", "evidence": "e{i}", "detail": "d", "fix": "f"}}"#))
            .collect();
        let words: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"word": "w{i}", "hint": "h"}}"#))
            .collect();
        let raw = format!(
            r#"{{"summary": "s", "issues": [{}], "focus_words": [{}]}}"#,
            issues.join(","),
            words.join(",")
        );
        let report = parse_json_report(&raw).expect("parsed");
        assert_eq!(report.issues.len(), MAX_ISSUES);
        assert_eq!(report.focus_words.len(), MAX_FOCUS_WORDS);
    }

    #[test]
    fn the_language_defaults_and_is_echoed() {
        assert_eq!(normalize_language(None), "en");
        assert_eq!(normalize_language(Some("  ")), "en");
        assert_eq!(normalize_language(Some("de_DE")), "de-DE");
        assert_eq!(normalize_language(Some("de; rm -rf")), "en");
        let prompt = build_prompt("EVIDENCE", "de");
        assert!(prompt.contains("Write the whole answer in de."));
        assert!(prompt.contains("EVIDENCE"));
    }

    #[test]
    fn an_issue_without_any_content_is_dropped() {
        let raw = r#"{"summary": "s", "issues": [{"area": "accuracy"}, {"detail": "d", "fix": "f"}]}"#;
        let report = parse_json_report(raw).expect("parsed");
        assert_eq!(report.issues.len(), 1);
    }
}
