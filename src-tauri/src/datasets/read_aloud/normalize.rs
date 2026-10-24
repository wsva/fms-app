//! The normalized scoring reference: one prompt, two ways to run it, and the
//! measurement that decides whether the result is worth keeping.
//!
//! The prompt lives here, in the backend, rather than beside the button that
//! shows it, because three callers must agree word for word: the in-app model
//! call, the "copy the prompt" route (the user pastes it into whatever provider
//! they like), and the MCP tool an agent drives. A candidate produced by any of
//! them has the same shape, so the numbers are comparable.
//!
//! `measure` is the other half of the design. Scoring prefers `normalized_content`
//! the moment it is set, so an unmeasured rewrite silently degrades every future
//! attempt — the gate ("only keep what beats the reference on my own recordings")
//! is what makes the feature safe to use, and it needs the dataset's attempts,
//! which is why it is not a frontend-only calculation.

use rusqlite::params;
use serde::Serialize;
use tauri::State;

use crate::ai::llm::{llm_chat, ChatMessage};
use crate::datasets::asr_metric::score_value;
use crate::settings::SettingsState;

use super::open_attempts;

/// Deterministic on purpose: the same passage should normalize the same way
/// every time, so a second click does not churn a reference that already works.
const DEFAULT_TEMPERATURE: f32 = 0.0;

/// The instruction half of the prompt. Deliberately narrow: this is a spelling
/// transform (surface → spoken form), not an editing pass. Everything the model
/// is tempted to "improve" — wording, order, grammar, punctuation — scores
/// worse, so it is forbidden explicitly rather than implied.
const INSTRUCTION: &str = r#"You rewrite a passage into the form it takes when it is read aloud, to be used as a speech-recognition scoring reference.

Rewrite the passage below so that a transcription of someone reading it aloud matches it word for word. Change only how things are spelled out, never what is said:

- Spell out numbers, cardinals and ordinals, in the passage's own language (20 → zwanzig / twenty, 3. → dritte / third).
- Spell out dates and times (12.05.2024 → zwölfte Mai zweitausendvierundzwanzig / the twelfth of May 2024; 15:30 → fünfzehn Uhr dreißig / three thirty in the afternoon).
- Spell out currency, percentages, units and math symbols (5 € → fünf Euro / five euros, 20 % → zwanzig Prozent / twenty percent, ± → plus minus).
- Expand abbreviations the way they are pronounced (vgl. → vergleiche, usw. → und so weiter, bzw. → beziehungsweise, Dr. → Doktor).
- Keep the passage in its own language: do not translate it.
- Leave every other word, all punctuation and all capitalisation exactly as written. Do not reword, reorder, shorten, correct or explain anything.

Reply with the rewritten passage only: no preamble, no commentary, no code fences.

Passage:
"#;

/// Build the prompt for one passage. The text is quoted verbatim: the model's
/// output is compared against it, so adding or losing anything here shows up as
/// a score change that has nothing to do with the normalization.
pub(crate) fn build_prompt(content: &str) -> String {
    format!("{}\n{}\n", INSTRUCTION, content.trim())
}

/// Strip what a model adds around the answer it was told not to decorate: a
/// fenced block, a stray heading, trailing whitespace. Inner line breaks are
/// kept — they are the passage's own paragraphs.
pub(crate) fn clean_candidate(raw: &str) -> String {
    let mut body = raw.trim();
    if let Some(rest) = body.strip_prefix("```") {
        // Drop the language tag after the opening fence, if there is one.
        body = rest.split_once('\n').map_or(rest, |(_, r)| r);
    }
    let body = body.trim_end();
    let body = body.strip_suffix("```").map_or(body, |b| b);
    body.trim().to_string()
}

/// What a generation call returns: always the prompt (so a caller can copy it
/// instead of running it), plus the candidate when a model was named.
#[derive(Serialize)]
pub struct NormalizeResult {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

/// Generate a normalized reference for `content`.
///
/// `model` empty/absent is the "copy the prompt" route: nothing is called and
/// only the prompt comes back. Naming a model runs it through the app's chat
/// seam (the configured provider, Ollama by default). Nothing is written:
/// the candidate is a draft the user saves.
#[tauri::command]
pub async fn read_aloud_normalize(
    settings: State<'_, SettingsState>,
    content: String,
    model: Option<String>,
    temperature: Option<f32>,
) -> Result<NormalizeResult, String> {
    generate(settings, content, model, temperature).await
}

pub(crate) async fn generate(
    settings: State<'_, SettingsState>,
    content: String,
    model: Option<String>,
    temperature: Option<f32>,
) -> Result<NormalizeResult, String> {
    if content.trim().is_empty() {
        return Err(
            "The text to read is empty, so there is nothing to normalize yet. Write the passage first."
                .into(),
        );
    }
    let prompt = build_prompt(&content);
    let model = model.unwrap_or_default();
    if model.trim().is_empty() {
        return Ok(NormalizeResult { prompt, candidate: None });
    }
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt.clone(),
    }];
    let response = llm_chat(
        settings,
        model.trim().to_string(),
        messages,
        Some(temperature.unwrap_or(DEFAULT_TEMPERATURE)),
    )
    .await?;
    let candidate = clean_candidate(&response.content);
    if candidate.is_empty() {
        return Err(
            "The model returned nothing. Try again, pick another model, or copy the prompt and use an external provider."
                .into(),
        );
    }
    Ok(NormalizeResult { prompt, candidate: Some(candidate) })
}

/// One candidate measured against the user's own recordings.
#[derive(Serialize)]
pub struct NormalizeMeasure {
    /// How many transcripts the comparison had to work with. Zero means the
    /// numbers below prove nothing — the text has never been recorded.
    pub attempts: i64,
    /// Best score the reference in use today achieves.
    pub baseline: Option<f64>,
    /// Best score the candidate achieves against the same transcripts.
    pub candidate: Option<f64>,
}

/// Measure a candidate against the recordings of one text: the accept/reject
/// gate from the design, as one call.
///
/// `reference` is what scoring reads today (the stored normalized reference, or
/// the passage itself), passed in rather than read here so a caller can measure
/// a draft it is still editing. Nothing is written.
#[tauri::command]
pub async fn read_aloud_normalize_measure(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    text_uuid: String,
    reference: String,
    candidate: String,
) -> Result<NormalizeMeasure, String> {
    measure(&settings, &dataset_uuid, &text_uuid, &reference, &candidate)
}

pub(crate) fn measure(
    settings: &SettingsState,
    dataset_uuid: &str,
    text_uuid: &str,
    reference: &str,
    candidate: &str,
) -> Result<NormalizeMeasure, String> {
    if candidate.trim().is_empty() {
        return Err("Nothing to measure: the candidate is empty.".into());
    }
    let ctx = open_attempts(settings, dataset_uuid)?;
    let mut stmt = ctx
        .conn
        .prepare(
            "SELECT recognized FROM app.read_attempt_event \
             WHERE text_uuid = ?1 AND user_id = ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![text_uuid, &ctx.me], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;

    let mut attempts = 0i64;
    let mut baseline: Option<f64> = None;
    let mut best: Option<f64> = None;
    for recognized in rows.flatten() {
        if recognized.trim().is_empty() {
            continue;
        }
        attempts += 1;
        let b = score_value(reference, &recognized);
        let c = score_value(candidate, &recognized);
        baseline = Some(baseline.map_or(b, |v| v.max(b)));
        best = Some(best.map_or(c, |v| v.max(c)));
    }
    Ok(NormalizeMeasure { attempts, baseline, candidate: best })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_quotes_the_passage_verbatim() {
        let prompt = build_prompt("  Am 3. Mai kostet es 20 %.  ");
        assert!(prompt.ends_with("\nAm 3. Mai kostet es 20 %.\n"));
        assert!(prompt.contains("Do not reword"));
    }

    #[test]
    fn a_fenced_answer_is_unwrapped() {
        assert_eq!(clean_candidate("```text\nAm dritten Mai.\n```"), "Am dritten Mai.");
        assert_eq!(clean_candidate("```\nAm dritten Mai.\n```  "), "Am dritten Mai.");
    }

    #[test]
    fn a_plain_answer_keeps_its_paragraphs() {
        let raw = "\nErster Absatz.\n\nZweiter Absatz.\n\n";
        assert_eq!(clean_candidate(raw), "Erster Absatz.\n\nZweiter Absatz.");
    }

    #[test]
    fn an_empty_answer_stays_empty() {
        assert_eq!(clean_candidate("   \n\n"), "");
    }
}
