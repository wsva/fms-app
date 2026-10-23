//! Tesseract TSV → reading-order paragraphs.
//!
//! Tesseract's `tsv` config writes one row per layout element (page, block,
//! paragraph, line, word) with pixel geometry and per-word confidence. The
//! plain-text mode throws that structure away and leaves line re-joining to an
//! LLM; here the TSV is used directly: words go back into lines and lines into
//! paragraphs.
//!
//! Words the typesetter split across a line break are *recorded*, not decided:
//! the rebuild returns a [`Candidate`] per line-end hyphen (fragment, hyphen
//! character, first word of the next line) and leaves the text in its "kept"
//! form (`Sprach- verarbeitung`). [`super::hyphen`] resolves the candidates
//! against a Hunspell dictionary and [`super::hyphen::apply`] folds the
//! decisions back into the text. Soft hyphens (U+00AD) are the one exception —
//! they exist only to mark a break, so they always join.
//!
//! Column positions are read from the header row rather than assumed, so a
//! Tesseract that adds a column (or reorders them) still parses.
//!
//! Tesseract TSV columns:
//! level, page_num, block_num, par_num, line_num, word_num,
//! left, top, width, height, conf, text

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// Column layout used when the dump has no header row (Tesseract's default).
const DEFAULT_COLUMNS: [usize; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// A soft hyphen never survives a line break: it is invisible on the page and
/// exists only to say "this word continues".
const SOFT_HYPHEN: char = '\u{00AD}';

/// Hyphens a typesetter uses to break a word at the margin: hyphen-minus,
/// U+2010 HYPHEN and U+2011 NON-BREAKING HYPHEN. Tesseract emits all three, so
/// matching only `-` let whole classes of breaks slip through as plain words.
const BREAK_HYPHENS: [char; 3] = ['-', '\u{2010}', '\u{2011}'];

/// Dashes that stand alone in running text: em dash, en dash, horizontal bar.
/// A dash glued to its neighbours is a layout accident of the OCR, not part of
/// the word, so each one is padded on both sides.
const SPACED_DASHES: [char; 3] = ['\u{2014}', '\u{2013}', '\u{2015}'];

/// Header names in the order the parser wants them.
const COLUMN_NAMES: [&str; 12] = [
    "level", "page_num", "block_num", "par_num", "line_num", "word_num", "left", "top", "width",
    "height", "conf", "text",
];

/// One line-end hyphen, plus what the two sides of the break were.
///
/// `join` and `reason` are filled in by [`super::hyphen::resolve`]; until then
/// the text carries the hyphen and a space (`Sprach- verarbeitung`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Text before the hyphen, hyphen already stripped (`Sprach`).
    pub left: String,
    /// The hyphen character that ended the line, kept verbatim so the repair
    /// restores exactly what was printed.
    pub hyphen: char,
    /// First word of the following line (`verarbeitung`).
    pub right: String,
    /// `left` + `right`, i.e. the word "Join" produces (`Sprachverarbeitung`).
    pub joined: String,
    /// Whether to fold the two fragments into one word.
    pub join: bool,
    /// Short machine tag for the evidence: `dictionary`, `unknown-word`,
    /// `lowercase`, `uppercase`, `digit`, `policy`, or `pending`.
    pub reason: &'static str,
}

/// One page: the paragraphs as rebuilt, plus every line-break hyphen in them.
#[derive(Debug, Clone)]
pub struct Reconstruction {
    /// Paragraphs joined with the hyphens still in place, i.e. the layout as
    /// printed. [`super::hyphen::apply`] turns the decisions into the text.
    pub text: String,
    /// Line-break hyphens in reading order.
    pub candidates: Vec<Candidate>,
}

/// One OCR'd text line: its layout box plus its words in reading order.
#[derive(Debug, Clone)]
struct OcrLine {
    block: i32,
    paragraph: i32,
    top: i32,
    bottom: i32,
    left: i32,
    height: i32,
    words: Vec<(i32, String)>,
}

impl OcrLine {
    fn text(&self) -> String {
        self.words
            .iter()
            .map(|(_, word)| word.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Malformed TSV, or a file that is not a Tesseract TSV dump at all.
#[derive(Debug)]
pub struct OcrError(String);

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tesseract TSV error: {}", self.0)
    }
}

impl Error for OcrError {}

/// Rebuild paragraphs from one page of Tesseract TSV.
///
/// Pure and dictionary-free: every line-break hyphen is returned as a
/// [`Candidate`] so the caller decides it (see [`super::hyphen`]). Standalone
/// dashes (`—`, `–`, `―`) are padded on both sides here, and soft hyphens are
/// always removed at line boundaries.
pub fn reconstruct(tsv: &str) -> Result<Reconstruction, OcrError> {
    let mut lines = parse_lines(tsv)?;

    if lines.is_empty() {
        return Ok(Reconstruction {
            text: String::new(),
            candidates: Vec::new(),
        });
    }

    // Establish reading order for ordinary, single-column pages.
    // Multi-column pages should be segmented into columns first.
    lines.sort_by_key(|line| (line.top, line.left));

    let mut heights: Vec<i32> = lines.iter().map(|line| line.height).collect();
    let typical_height = median(&mut heights);

    // Estimate the normal left margin.
    let mut lefts: Vec<i32> = lines.iter().map(|line| line.left).collect();
    let typical_left = median(&mut lefts);

    let paragraphs = group_paragraphs(lines, typical_height, typical_left);

    let mut candidates = Vec::new();
    let text = paragraphs
        .iter()
        .map(|paragraph| join_paragraph(paragraph, &mut candidates))
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(Reconstruction { text, candidates })
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Map every wanted column name to its index in `header`.
fn header_indexes(header: &[&str]) -> Result<[usize; 12], OcrError> {
    let mut indexes = [0usize; 12];

    for (i, name) in COLUMN_NAMES.iter().enumerate() {
        indexes[i] = header
            .iter()
            .position(|field| field.trim() == *name)
            .ok_or_else(|| OcrError(format!("missing TSV column {name}")))?;
    }

    Ok(indexes)
}

fn parse_i32(value: &str, field: &str) -> Result<i32, OcrError> {
    value
        .trim()
        .parse::<i32>()
        .map_err(|_| OcrError(format!("invalid {field}: {value:?}")))
}

/// Collect the level-5 (word) rows into lines, keyed by Tesseract's own
/// block/paragraph/line ids.
///
/// The key deliberately stops at `line_num`: a per-word `top` in the key would
/// split one line into several whenever Tesseract reports a one-pixel jitter or
/// a superscript on a single word.
fn parse_lines(tsv: &str) -> Result<Vec<OcrLine>, OcrError> {
    let mut lines: BTreeMap<(i32, i32, i32), OcrLine> = BTreeMap::new();
    let mut columns: Option<[usize; 12]> = None;

    for (row_index, row) in tsv.lines().enumerate() {
        let row = row.trim_end_matches('\r');

        if row.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = row.split('\t').collect();

        if columns.is_none() && fields.first().map(|f| f.trim()) == Some("level") {
            columns = Some(header_indexes(&fields)?);
            continue;
        }

        let idx = columns.unwrap_or(DEFAULT_COLUMNS);

        if fields.len() <= *idx.iter().max().unwrap_or(&0) {
            return Err(OcrError(format!(
                "row {} has too few columns",
                row_index + 1
            )));
        }

        let level = parse_i32(fields[idx[0]], "level")?;

        // Level 5 represents individual words.
        if level != 5 {
            continue;
        }

        let block = parse_i32(fields[idx[2]], "block_num")?;
        let paragraph = parse_i32(fields[idx[3]], "par_num")?;
        let line_num = parse_i32(fields[idx[4]], "line_num")?;
        let word_num = parse_i32(fields[idx[5]], "word_num")?;
        let left = parse_i32(fields[idx[6]], "left")?;
        let top = parse_i32(fields[idx[7]], "top")?;
        // Width is only used to keep the column honest; all boxing needs `left`.
        let _width = parse_i32(fields[idx[8]], "width")?;
        let height = parse_i32(fields[idx[9]], "height")?;
        let confidence = fields[idx[10]].trim().parse::<f32>().unwrap_or(-1.0);

        let text = fields[idx[11]].trim();

        // Tesseract emits a `conf` of -1 for rows it recognised nothing in.
        if text.is_empty() || confidence < 0.0 {
            continue;
        }

        let entry = lines
            .entry((block, paragraph, line_num))
            .or_insert_with(|| OcrLine {
                block,
                paragraph,
                top,
                bottom: top + height,
                left,
                height,
                words: Vec::new(),
            });

        entry.left = entry.left.min(left);
        entry.top = entry.top.min(top);
        entry.bottom = entry.bottom.max(top + height);
        entry.height = entry.height.max(height);
        entry.words.push((word_num, text.to_string()));
    }

    let mut lines: Vec<OcrLine> = lines.into_values().collect();

    for line in &mut lines {
        line.words.sort_by_key(|(word_num, _)| *word_num);
    }

    Ok(lines)
}

fn median(values: &mut [i32]) -> i32 {
    if values.is_empty() {
        return 1;
    }

    values.sort_unstable();
    values[values.len() / 2].max(1)
}

// ---------------------------------------------------------------------------
// Paragraph grouping
// ---------------------------------------------------------------------------

/// Split reading-ordered lines into paragraphs.
///
/// Three independent signals, any of which starts a new paragraph: a vertical
/// gap wider than most of a line, a change of Tesseract block/paragraph id, or
/// an indented line following one that sits on the left margin.
fn group_paragraphs(
    lines: Vec<OcrLine>,
    typical_height: i32,
    typical_left: i32,
) -> Vec<Vec<OcrLine>> {
    let mut paragraphs: Vec<Vec<OcrLine>> = Vec::new();

    for line in lines {
        let should_split = match paragraphs.last().and_then(|p| p.last()) {
            Some(previous) => {
                let vertical_gap = line.top - previous.bottom;

                // A large vertical gap is a strong paragraph signal.
                let gap_split = vertical_gap > typical_height * 3 / 4;

                // Tesseract's paragraph and block IDs are useful hints.
                let id_split =
                    line.block != previous.block || line.paragraph != previous.paragraph;

                // Detect a likely indented first line.
                let current_is_indented = line.left > typical_left + typical_height;
                let previous_is_at_margin = previous.left <= typical_left + typical_height / 2;
                let indentation_split =
                    current_is_indented && previous_is_at_margin && vertical_gap >= 0;

                gap_split || id_split || indentation_split
            }
            None => true,
        };

        if should_split {
            paragraphs.push(vec![line]);
        } else if let Some(last) = paragraphs.last_mut() {
            last.push(line);
        }
    }

    paragraphs
}

// ---------------------------------------------------------------------------
// Line joining and hyphen detection
// ---------------------------------------------------------------------------

fn join_paragraph(paragraph: &[OcrLine], candidates: &mut Vec<Candidate>) -> String {
    let mut text = String::new();

    for line in paragraph {
        let line_text = line.text();

        if text.is_empty() {
            text = line_text;
            continue;
        }

        let next_first = line_text.split_whitespace().next().unwrap_or("");
        let previous_last = text
            .split_whitespace()
            .last()
            .unwrap_or("")
            .to_string();

        // A soft hyphen is unambiguous evidence of a broken word, so it joins
        // here without asking anyone.
        if glue_soft_hyphen(&mut text, &previous_last, &line_text, next_first) {
            continue;
        }

        // A real hyphen is ambiguous: it may be a break (`Sprach-` +
        // `verarbeitung`) or a genuine compound (`E-` + `Mail`). Record it and
        // join with a plain space for now; the resolver decides.
        record_candidate(&previous_last, next_first, candidates);

        text.push(' ');
        text.push_str(&line_text);
    }

    space_dashes(&text)
}

/// Note the line-end hyphen with the words on both sides of it, if any.
fn record_candidate(previous_last: &str, next_first: &str, candidates: &mut Vec<Candidate>) {
    let Some(hyphen) = previous_last.chars().last().filter(|c| BREAK_HYPHENS.contains(c)) else {
        return;
    };

    let fragment = previous_last.trim_end_matches(BREAK_HYPHENS);

    // Nothing to glue the next word onto (e.g. a lone dash as its own word).
    if fragment.is_empty() || next_first.is_empty() {
        return;
    }

    candidates.push(Candidate {
        left: fragment.to_string(),
        hyphen,
        right: next_first.to_string(),
        joined: format!("{fragment}{next_first}"),
        join: false,
        reason: "pending",
    });
}

/// Glue a line whose last word ends in a soft hyphen. Returns whether the text
/// was repaired.
fn glue_soft_hyphen(
    text: &mut String,
    previous_last: &str,
    line_text: &str,
    next_first: &str,
) -> bool {
    if next_first.is_empty() || !previous_last.ends_with(SOFT_HYPHEN) {
        return false;
    }

    let fragment = previous_last.trim_end_matches(SOFT_HYPHEN);

    if fragment.is_empty() {
        return false;
    }

    let Some(position) = text.rfind(previous_last) else {
        return false;
    };

    // Drop the soft hyphen, keep the rest of the next line, then insert its
    // first word straight after the fragment (no space).
    text.replace_range(position..position + previous_last.len(), fragment);

    let rest = match line_text.find(next_first) {
        Some(start) => &line_text[start + next_first.len()..],
        None => "",
    };
    text.push_str(rest);
    text.insert_str(position + fragment.len(), next_first);

    true
}

/// Put a single space on both sides of every standalone dash.
///
/// Tesseract frequently drops the spaces around a dash (`Wort—Wort`), which
/// reads as one glued token. Existing spaces are collapsed rather than doubled,
/// and an en dash between two digits (`2020–2021`) is left tight: there it is a
/// range sign, not a phrase break.
fn space_dashes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut index = 0;

    while index < chars.len() {
        let ch = chars[index];

        if !SPACED_DASHES.contains(&ch) || is_numeric_range(&chars, index) {
            out.push(ch);
            index += 1;
            continue;
        }

        // Drop spaces already on the left, and skip those on the right.
        while out.ends_with([' ', '\t']) {
            out.pop();
        }

        let mut next = index + 1;
        while matches!(chars.get(next), Some(' ') | Some('\t')) {
            next += 1;
        }

        if !out.is_empty() {
            out.push(' ');
        }
        out.push(ch);
        // No trailing space at the end of a paragraph.
        if next < chars.len() {
            out.push(' ');
        }

        index = next;
    }

    out
}

/// `true` when the dash at `index` sits directly between two digits.
fn is_numeric_range(chars: &[char], index: usize) -> bool {
    let before = index.checked_sub(1).and_then(|i| chars.get(i));
    let after = chars.get(index + 1);

    matches!(before, Some(ch) if ch.is_ascii_digit())
        && matches!(after, Some(ch) if ch.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext";

    #[test]
    fn records_a_line_break_hyphen_without_deciding_it() {
        // A hyphenated word is split across two Tesseract *lines* (line_num 1/2).
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tSprach-\n\
             5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t95.0\tverarbeitung\n\
             5\t1\t1\t1\t3\t1\t10\t70\t40\t20\t90.0\tist\n\
             5\t1\t1\t1\t3\t2\t60\t70\t40\t20\t92.0\tschwer\n"
        );

        let page = reconstruct(&tsv).unwrap();

        assert_eq!(page.text, "Sprach- verarbeitung ist schwer");
        assert_eq!(page.candidates.len(), 1);

        let candidate = &page.candidates[0];
        assert_eq!(candidate.left, "Sprach");
        assert_eq!(candidate.hyphen, '-');
        assert_eq!(candidate.right, "verarbeitung");
        assert_eq!(candidate.joined, "Sprachverarbeitung");
        // Undecided until the resolver runs.
        assert!(!candidate.join);
        assert_eq!(candidate.reason, "pending");
    }

    #[test]
    fn records_the_unicode_hyphens_tesseract_emits() {
        // U+2010 HYPHEN and U+2011 NON-BREAKING HYPHEN are what a scanned
        // German book usually shows; matching only `-` used to miss them.
        for hyphen in ['\u{2010}', '\u{2011}'] {
            let tsv = format!(
                "{HEADER}\n\
                 5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tSprach{hyphen}\n\
                 5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t95.0\tverarbeitung\n"
            );

            let page = reconstruct(&tsv).unwrap();

            assert_eq!(page.candidates.len(), 1);
            assert_eq!(page.candidates[0].hyphen, hyphen);
            assert_eq!(page.candidates[0].left, "Sprach");
            assert_eq!(page.candidates[0].joined, "Sprachverarbeitung");
        }
    }

    #[test]
    fn always_removes_soft_hyphen_at_line_end() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tSprach\u{00AD}\n\
             5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t95.0\tverarbeitung\n"
        );

        let page = reconstruct(&tsv).unwrap();

        assert_eq!(page.text, "Sprachverarbeitung");
        // A soft hyphen is evidence, not a question.
        assert!(page.candidates.is_empty());
    }

    #[test]
    fn ignores_a_line_ending_in_a_lone_dash() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\t-\n\
             5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t95.0\tverarbeitung\n"
        );

        let page = reconstruct(&tsv).unwrap();

        assert!(page.candidates.is_empty());
        assert_eq!(page.text, "- verarbeitung");
    }

    #[test]
    fn pads_dashes_glued_to_words() {
        // Tesseract lost the spaces around the dash: one glued token.
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tWort\u{2014}Wort\n"
        );

        let text = reconstruct(&tsv).unwrap().text;
        assert_eq!(text, "Wort — Wort");
    }

    #[test]
    fn keeps_single_spaces_around_a_dash() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tWort\n\
             5\t1\t1\t1\t1\t2\t60\t10\t20\t20\t96.5\t\u{2014}\n\
             5\t1\t1\t1\t1\t3\t80\t10\t50\t20\t96.5\tWort\n"
        );

        let text = reconstruct(&tsv).unwrap().text;
        assert_eq!(text, "Wort — Wort");
    }

    #[test]
    fn pads_a_dash_at_the_end_of_a_line() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tEnde\u{2014}\n\
             5\t1\t1\t1\t2\t1\t10\t40\t50\t20\t96.5\tWeiter\n"
        );

        let text = reconstruct(&tsv).unwrap().text;
        assert_eq!(text, "Ende — Weiter");
    }

    #[test]
    fn keeps_numeric_ranges_tight() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\t2020\u{2013}2021\n"
        );

        let text = reconstruct(&tsv).unwrap().text;
        assert_eq!(text, "2020–2021");
    }

    #[test]
    fn splits_paragraphs_on_a_vertical_gap() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tErster\n\
             5\t1\t1\t1\t1\t2\t70\t10\t50\t20\t96.5\tAbsatz\n\
             5\t1\t2\t1\t1\t1\t10\t120\t50\t20\t96.5\tZweiter\n\
             5\t1\t2\t1\t2\t2\t70\t120\t50\t20\t96.5\tAbsatz\n"
        );

        let text = reconstruct(&tsv).unwrap().text;
        assert_eq!(text, "Erster Absatz\n\nZweiter Absatz");
    }

    #[test]
    fn splits_paragraphs_on_tesseract_ids() {
        // Same baseline geometry, different Tesseract paragraph id.
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tErster\n\
             5\t1\t1\t2\t1\t1\t10\t10\t50\t20\t96.5\tZweiter\n"
        );

        let text = reconstruct(&tsv).unwrap().text;
        assert_eq!(text, "Erster\n\nZweiter");
    }

    #[test]
    fn reads_columns_from_the_header_order() {
        // Complete header, but `left` and `top` swapped relative to the default.
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\ttop\tleft\twidth\theight\tconf\ttext\n\
                   5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tHallo\n\
                   5\t1\t1\t1\t1\t2\t10\t70\t50\t20\t95.0\tWelt\n";

        let text = reconstruct(tsv).unwrap().text;
        assert_eq!(text, "Hallo Welt");
    }

    #[test]
    fn rejects_rows_with_missing_columns() {
        let tsv = format!("{HEADER}\n5\t1\t1\t1\t1\n");
        assert!(reconstruct(&tsv).is_err());
    }

    #[test]
    fn empty_input_yields_empty_text() {
        let page = reconstruct("").unwrap();
        assert_eq!(page.text, "");
        assert!(page.candidates.is_empty());
    }
}
