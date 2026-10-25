//! Turn a source file into passages: the step every "build a read-aloud
//! dataset from a book" session has to redo by hand.
//!
//! The log's agent hand-rolled this four times: it extracted the epub, split the
//! text, guessed titles, labelled body text with index entries, imported the
//! table of contents as passage #1, then rebuilt a sizing preview twice. What it
//! never needed — and what is deliberately *not* here — is epub or PDF parsing:
//! extraction stays the caller's job. This module owns segmentation, titling and
//! insertion, so "here is the text" is one call.
//!
//! **A deterministic splitter, not `textsplit`.** That module is desktop-gated,
//! async and model-dependent, and cue-grade boundaries are not what chunking
//! needs: a passage should be about [`ChunkOptions::target_chars`] long and
//! should never break mid-word. A pure function that always answers the same
//! way for the same input is what makes a dry run comparable to the real write.
//!
//! Writes go through the same batch path as `read_aloud_save_texts`, so the
//! result carries the same `saved`/`failed` partition and there is one place to
//! fix a save.

use std::path::Path;

use serde_json::{json, Value};

use crate::settings::SettingsState;

use super::{
    find_dataset_dir_write, next_order_num, normalize_language, read_info, save_text_rows,
    write_info, DatasetInfo, ReadText, TextDraft,
};

// ============================================================
// Types
// ============================================================

/// How a source is read. `auto` decides from the extension and the first
/// non-space character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceFormat {
    Auto,
    Text,
    Markdown,
    Json,
}

impl SourceFormat {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "text" | "txt" | "plain" => Ok(Self::Text),
            "markdown" | "md" => Ok(Self::Markdown),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "Unknown format `{}` (expected auto, text, markdown or json)",
                other
            )),
        }
    }
}

/// How the plain-text splitter sizes a passage. Only the plain-text and
/// markdown paths use it: a json payload is already chunked by its author.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChunkOptions {
    /// Accumulate paragraphs until a passage reaches roughly this many
    /// characters — the log's own measure was a median of ~1,026.
    pub target_chars: usize,
    /// A passage below this is too short to practise: merge it backwards.
    pub min_chars: usize,
    /// A single paragraph above this is broken on sentence boundaries.
    pub max_chars: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            target_chars: 1100,
            min_chars: 450,
            max_chars: 1900,
        }
    }
}

impl ChunkOptions {
    /// Clamp a caller's numbers into a workable shape: `min <= target <= max`,
    /// and nothing so small that every word becomes a passage.
    pub(crate) fn sanitised(self) -> Self {
        let min_chars = self.min_chars.clamp(80, 20_000);
        let max_chars = self.max_chars.clamp(min_chars, 40_000);
        let target_chars = self.target_chars.clamp(min_chars, max_chars);
        Self {
            target_chars,
            min_chars,
            max_chars,
        }
    }
}

/// One passage the plan produced, before it is written.
struct Passage {
    title: String,
    note: String,
    content: String,
}

impl Passage {
    fn chars(&self) -> usize {
        self.content.chars().count()
    }
}

/// A source resolved to text plus the two facts the splitter needs.
pub(crate) struct Source {
    text: String,
    /// File stem, used as the default title prefix.
    stem: String,
    /// File name, used in the default note.
    file_name: String,
    format: SourceFormat,
}

impl Source {
    /// Read whichever of `source_path` / `source_text` the caller supplied.
    pub(crate) fn resolve(
        source_path: Option<&str>,
        source_text: Option<&str>,
        format: SourceFormat,
    ) -> Result<Self, String> {
        if let Some(text) = source_text.filter(|t| !t.trim().is_empty()) {
            return Ok(Self {
                text: text.to_string(),
                stem: String::new(),
                file_name: String::new(),
                format: detect(&text, format, None),
            });
        }
        let path = source_path
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                "No source given: pass `source_path` (a file) or `source_text` (the text itself)"
                    .to_string()
            })?;
        let path = Path::new(path);
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read `{}`: {}", path.display(), e))?;
        if text.trim().is_empty() {
            return Err(format!("`{}` is empty", path.display()));
        }
        Ok(Self {
            stem: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            file_name: path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            format: detect(&text, format, path.extension().and_then(|e| e.to_str())),
            text,
        })
    }
}

/// `auto` reads the extension, then the first non-space character — the same
/// rule a human would apply, spelled once.
fn detect(text: &str, format: SourceFormat, extension: Option<&str>) -> SourceFormat {
    if format != SourceFormat::Auto {
        return format;
    }
    match extension.map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("json") => return SourceFormat::Json,
        Some("md") | Some("markdown") => return SourceFormat::Markdown,
        Some("txt") => return SourceFormat::Text,
        _ => {}
    }
    if text.trim_start().starts_with(['[', '{']) {
        SourceFormat::Json
    } else {
        SourceFormat::Text
    }
}

// ============================================================
// The entry point
// ============================================================

/// Plan (and, unless `dry_run`, write) the passages a source produces.
///
/// `dry_run` returns the sizing preview the log's agent rebuilt by hand twice:
/// how many passages, their character distribution, an estimate of speaking
/// time, and the first few titles with an excerpt.
#[allow(clippy::too_many_arguments)]
pub(crate) fn import_texts(
    settings: &SettingsState,
    dataset_uuid: &str,
    source: Source,
    chunk: ChunkOptions,
    language: Option<&str>,
    title_prefix: Option<&str>,
    note: Option<&str>,
    start_order: Option<i64>,
    dry_run: bool,
) -> Result<Value, String> {
    let chunk = chunk.sanitised();
    let prefix = title_prefix
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| source.stem.clone());
    let default_note = note
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if source.file_name.is_empty() {
                "Imported".to_string()
            } else {
                format!("Imported from {}", source.file_name)
            }
        });

    let mut skipped: Vec<Value> = Vec::new();
    let passages = match source.format {
        SourceFormat::Json => passages_from_json(&source.text, &default_note, &mut skipped)?,
        _ => passages_from_prose(&source.text, &chunk, &prefix, &default_note),
    };
    if passages.is_empty() {
        return Err(
            "The source produced no passages. Check that it contains text, or pass `format: json` \
             with `[{title, content}]`."
                .to_string(),
        );
    }

    let plan = sizing(&passages);
    if dry_run {
        let preview: Vec<Value> = passages
            .iter()
            .take(5)
            .map(|p| {
                json!({
                    "title": p.title,
                    "chars": p.chars(),
                    "excerpt": p.content.chars().take(160).collect::<String>(),
                })
            })
            .collect();
        return Ok(json!({
            "status": "ok",
            "dry_run": true,
            "format": format_name(source.format),
            "texts": passages.len(),
            "chars": plan.0,
            "total_chars": plan.1,
            "estimate_speaking_seconds": plan.2,
            "preview": preview,
            "skipped": skipped,
        }));
    }

    // Write. `language` lands in info.json first so a generation right after the
    // import already resolves the right voice.
    if let Some(language) = language.map(str::trim).filter(|l| !l.is_empty()) {
        let dir = find_dataset_dir_write(settings, dataset_uuid)?;
        let mut info: DatasetInfo = read_info(&dir)?;
        let normalised = normalize_language(language);
        if info.language != normalised {
            info.language = normalised;
            info.updated_at = super::now_stamp();
            write_info(&dir, &info)?;
        }
    }

    let mut order = match start_order {
        Some(o) => o,
        None => next_order_num(settings, dataset_uuid)?,
    };
    let mut rows: Vec<ReadText> = Vec::with_capacity(passages.len());
    for passage in passages {
        let draft = TextDraft {
            title: passage.title,
            note: passage.note,
            content: passage.content,
            ..Default::default()
        };
        let row = draft.into_row(dataset_uuid, order);
        order = row.order_num + 1;
        rows.push(row);
    }
    let sent = rows.len();
    let orders: (i64, i64) = (
        rows.first().map(|r| r.order_num).unwrap_or(0),
        rows.last().map(|r| r.order_num).unwrap_or(0),
    );
    let batch = save_text_rows(settings, dataset_uuid, &rows)?;
    let errors: Vec<Value> = batch
        .errors
        .iter()
        .map(|(index, title, error)| {
            json!({ "index": index, "title": title, "error": error })
        })
        .collect();
    let texts_total = super::count_texts(settings, dataset_uuid);
    Ok(json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "dry_run": false,
        "format": format_name(source.format),
        "sent": sent,
        "saved": batch.saved,
        "failed": errors.len(),
        "errors": errors,
        "skipped": skipped,
        "texts_total": texts_total,
        "order_range": [orders.0, orders.1],
    }))
}

fn format_name(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::Json => "json",
        SourceFormat::Markdown => "markdown",
        _ => "text",
    }
}

/// `(chars: {min, median, max}, total_chars, estimate_speaking_seconds)`.
fn sizing(passages: &[Passage]) -> (Value, i64, i64) {
    let mut lengths: Vec<usize> = passages.iter().map(Passage::chars).collect();
    lengths.sort_unstable();
    let total: usize = lengths.iter().sum();
    let median = if lengths.is_empty() {
        0
    } else {
        lengths[lengths.len() / 2]
    };
    // Read-aloud pace: 150 words a minute at about five characters a word.
    let seconds = (total as f64 / 12.5).round() as i64;
    (
        json!({
            "min": lengths.first().copied().unwrap_or(0),
            "median": median,
            "max": lengths.last().copied().unwrap_or(0),
        }),
        total as i64,
        seconds,
    )
}

// ============================================================
// Prose: paragraphs, then sentences
// ============================================================

/// Split a prose or markdown source into passages.
///
/// Rules, in one place so they can be argued with:
/// - a blank line ends a paragraph; `#` heading lines are hard boundaries and
///   keep their text (dropping headings is what made the log's passage #1 the
///   table of contents);
/// - paragraphs accumulate to `target_chars`;
/// - a paragraph over `max_chars` breaks on sentence boundaries — after
///   `[.!?»"'…]` plus whitespace, never inside a word;
/// - a passage under `min_chars` at a hard boundary merges into the previous
///   one, because a 40-character leftover is not a reading exercise.
fn passages_from_prose(
    text: &str,
    chunk: &ChunkOptions,
    prefix: &str,
    note: &str,
) -> Vec<Passage> {
    let blocks = blocks(text, chunk.max_chars);
    let mut chunks: Vec<(Option<String>, String)> = Vec::new();
    let mut current = String::new();
    let mut current_heading: Option<String> = None;

    for (heading, body) in blocks {
        if let Some(heading) = heading {
            close(&mut chunks, &mut current, &mut current_heading);
            current_heading = Some(heading.clone());
            current.push_str(&heading);
            continue;
        }
        let projected = chars(&current) + chars(&body) + 1;
        if !current.is_empty() && projected > chunk.target_chars {
            close(&mut chunks, &mut current, &mut current_heading);
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(&body);
        if chars(&current) >= chunk.target_chars {
            close(&mut chunks, &mut current, &mut current_heading);
        }
    }
    close(&mut chunks, &mut current, &mut current_heading);

    // A leftover below `min_chars` joins its predecessor unless that would
    // overflow the ceiling by more than one minimum passage.
    let mut merged: Vec<(Option<String>, String)> = Vec::new();
    for (heading, body) in chunks {
        if let Some(previous) = merged.last_mut() {
            let short = chars(&body) < chunk.min_chars;
            let fits = chars(&previous.1) + chars(&body) + 2 <= chunk.max_chars + chunk.min_chars;
            if short && fits {
                previous.1.push_str("\n\n");
                previous.1.push_str(&body);
                continue;
            }
        }
        merged.push((heading, body));
    }

    merged
        .into_iter()
        .filter(|(_, body)| !body.trim().is_empty())
        .enumerate()
        .map(|(index, (heading, body))| {
            // A heading is the best title a book offers; the number is the
            // fallback for a source that has none (the log's importer produced
            // numbered passages that were unreadable in a list).
            let title = match heading.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
                Some(heading) if prefix.is_empty() => heading.to_string(),
                Some(heading) => format!("{} · {}", prefix, heading),
                None if prefix.is_empty() => format!("Passage {}", index + 1),
                None => format!("{} · {}", prefix, index + 1),
            };
            Passage {
                title,
                note: note.to_string(),
                content: body.trim().to_string(),
            }
        })
        .collect()
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn close(
    chunks: &mut Vec<(Option<String>, String)>,
    current: &mut String,
    heading: &mut Option<String>,
) {
    let body = std::mem::take(current);
    if !body.trim().is_empty() {
        chunks.push((heading.take(), body.trim().to_string()));
    }
    *heading = None;
}

/// Paragraphs and headings, in document order. `(heading, body)` — a heading
/// block carries its own text as `body` so it stays in the reading.
fn blocks(text: &str, max_chars: usize) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    let mut paragraph = String::new();
    let mut in_code_fence = false;

    let flush = |out: &mut Vec<(Option<String>, String)>, paragraph: &mut String| {
        let body = collapse(paragraph);
        paragraph.clear();
        if body.is_empty() {
            return;
        }
        for piece in split_oversized(&body, max_chars) {
            out.push((None, piece));
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();
        // A fenced code block is kept verbatim: a line of code inside it is not
        // a heading, and blank lines inside it are not paragraph breaks.
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            flush(&mut out, &mut paragraph);
            paragraph.push_str(trimmed);
            paragraph.push(' ');
            continue;
        }
        if in_code_fence {
            paragraph.push_str(trimmed);
            paragraph.push(' ');
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut out, &mut paragraph);
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            flush(&mut out, &mut paragraph);
            let heading = heading.trim_start_matches('#').trim();
            if !heading.is_empty() {
                out.push((Some(heading.to_string()), heading.to_string()));
            }
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(trimmed);
    }
    flush(&mut out, &mut paragraph);
    out
}

/// Whitespace collapsed to single spaces — what makes a length budget mean
/// characters of speech rather than characters of layout.
fn collapse(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Break one oversized paragraph on sentence boundaries. A sentence longer than
/// `max_chars` on its own is broken at the last space before the limit — never
/// inside a word (the log's first attempt cut words in half).
fn split_oversized(paragraph: &str, max_chars: usize) -> Vec<String> {
    if chars(paragraph) <= max_chars {
        return vec![paragraph.to_string()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for sentence in sentences(paragraph) {
        if chars(&sentence) > max_chars {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            out.extend(break_on_spaces(&sentence, max_chars));
            continue;
        }
        if !current.is_empty() && chars(&current) + chars(&sentence) + 1 > max_chars {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// Sentences: a run ending in terminal punctuation plus whitespace. The
/// punctuation set includes the German/Anglo typographic quotes the log's book
/// used, because a closing `«…“` is as good a place to break as a period.
fn sentences(paragraph: &str) -> Vec<String> {
    const TERMINALS: [char; 8] = ['.', '!', '?', '…', '»', '“', '”', '"'];
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = paragraph.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if TERMINALS.contains(&c) {
            // Swallow a run of terminals (`?!`, `...`).
            while let Some(&next) = chars.peek() {
                if TERMINALS.contains(&next) {
                    current.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if chars.peek().is_some_and(|n| n.is_whitespace()) {
                out.push(current.trim().to_string());
                current.clear();
            }
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Last resort for a sentence over the ceiling: cut at the last space that fits.
fn break_on_spaces(sentence: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in sentence.split_whitespace() {
        if !current.is_empty() && chars(&current) + chars(word) + 1 > max_chars {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

// ============================================================
// JSON: the shape an agent already produced
// ============================================================

/// Read the exact shape the log's agent wrote (`[{section, order, title, note,
/// content}]`), so `texts.json` imports verbatim. A payload that is already
/// chunked is not re-chunked; items with blank content are reported instead of
/// dropped silently, because "382 sent, 380 saved" must never be a surprise.
fn passages_from_json(
    text: &str,
    default_note: &str,
    skipped: &mut Vec<Value>,
) -> Result<Vec<Passage>, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| format!("The json source did not parse: {}", e))?;
    let items = match value {
        Value::Array(items) => items,
        Value::Object(mut map) => match map.remove("texts") {
            Some(Value::Array(items)) => items,
            _ => {
                return Err(
                    "A json source must be a list of passages, or an object with a `texts` list"
                        .to_string(),
                )
            }
        },
        _ => {
            return Err(
                "A json source must be a list of passages, or an object with a `texts` list"
                    .to_string(),
            )
        }
    };
    let mut passages = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let Some(object) = item.as_object() else {
            skipped.push(json!({ "index": index, "reason": "not an object" }));
            continue;
        };
        let field = |name: &str| {
            object
                .get(name)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let content = field("content");
        if content.is_empty() {
            skipped.push(json!({
                "index": index,
                "title": field("title"),
                "reason": "blank content",
            }));
            continue;
        }
        let title = {
            let given = field("title");
            if given.is_empty() {
                super::fallback_title(&content)
            } else {
                given
            }
        };
        let note = {
            let given = field("note");
            if given.is_empty() {
                default_note.to_string()
            } else {
                given
            }
        };
        passages.push(Passage {
            title,
            note,
            content,
        });
    }
    Ok(passages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(text: &str) -> Vec<String> {
        passages_from_prose(text, &ChunkOptions::default(), "Book", "note")
            .into_iter()
            .map(|p| p.content)
            .collect()
    }

    #[test]
    fn a_heading_is_a_hard_boundary_and_stays_in_the_reading() {
        let text = "Intro paragraph.\n\n# Chapter One\n\nBody of one.\n\n# Chapter Two\n\nBody of two.";
        let passages = prose(text);
        assert_eq!(passages.len(), 3, "two headings and an intro: {:?}", passages);
        assert!(passages[1].starts_with("Chapter One"));
        assert!(passages[1].contains("Body of one."));
        assert!(!passages[1].contains("Body of two."));
    }

    #[test]
    fn paragraphs_accumulate_to_the_target_and_never_mid_word() {
        let paragraph = "Ein Satz mit mehreren Wörtern. ".repeat(60);
        let text = format!("{}\n\n{}", paragraph, paragraph);
        let passages = prose(&text);
        assert!(passages.len() >= 2);
        for passage in &passages {
            assert!(passage.chars().count() <= ChunkOptions::default().max_chars);
            // A break inside a word would leave a fragment with no space around it.
            assert!(!passage.ends_with('-'), "broke inside a word: {:?}", passage);
        }
    }

    #[test]
    fn a_short_leftover_merges_backwards() {
        let long = "Ein langer Absatz. ".repeat(60);
        let text = format!("{}\n\n# Kurz\n\nEnde.", long);
        let passages = prose(&text);
        assert_eq!(passages.len(), 1, "the stub joined its predecessor");
    }

    #[test]
    fn whitespace_is_collapsed_so_lengths_mean_speech() {
        let passages = prose("Ein   Satz\n  mit\tviel   Leerraum.");
        assert_eq!(passages, vec!["Ein Satz mit viel Leerraum.".to_string()]);
    }

    #[test]
    fn json_items_import_verbatim_and_blank_ones_are_reported() {
        let source = r#"[{"section":"1","order":1,"title":"Erster","note":"n","content":"Hallo."},
                          {"title":"Leer","content":"   "}]"#;
        let mut skipped = Vec::new();
        let passages = passages_from_json(source, "note", &mut skipped).unwrap();
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].title, "Erster");
        assert_eq!(passages[0].content, "Hallo.");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0]["reason"], "blank content");
    }

    #[test]
    fn json_without_a_title_falls_back_to_the_first_line() {
        let source = r#"[{"content":"Die Känguru-Offenbarung\n\nEs beginnt."}]"#;
        let mut skipped = Vec::new();
        let passages = passages_from_json(source, "note", &mut skipped).unwrap();
        assert_eq!(passages[0].title, "Die Känguru-Offenbarung");
    }

    #[test]
    fn a_sentence_too_long_to_fit_breaks_on_a_space() {
        let sentence = "wort ".repeat(600);
        let pieces = split_oversized(sentence.trim(), 500);
        assert!(pieces.len() > 1);
        for piece in &pieces {
            assert!(piece.chars().count() <= 500);
            assert!(piece.starts_with("wort"));
        }
    }

    #[test]
    fn auto_detects_json_and_markdown() {
        assert_eq!(detect("[{\"a\":1}]", SourceFormat::Auto, None), SourceFormat::Json);
        assert_eq!(detect("# Titel", SourceFormat::Auto, Some("md")), SourceFormat::Markdown);
        assert_eq!(detect("Text", SourceFormat::Auto, Some("txt")), SourceFormat::Text);
    }

    #[test]
    fn chunk_options_are_clamped_into_a_sane_order() {
        let opts = ChunkOptions {
            target_chars: 10,
            min_chars: 5000,
            max_chars: 100,
        }
        .sanitised();
        assert!(opts.min_chars <= opts.target_chars);
        assert!(opts.target_chars <= opts.max_chars);
    }
}
