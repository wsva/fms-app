//! Tesseract TSV → reading-order paragraphs.
//!
//! Tesseract's `tsv` config writes one row per layout element (page, block,
//! paragraph, line, word) with pixel geometry and per-word confidence. The
//! plain-text mode throws that structure away and leaves line re-joining to an
//! LLM; here the TSV is used directly: words go back into lines, lines into
//! paragraphs, and words the typesetter split across a line break are repaired
//! against a dictionary.
//!
//! Column positions are read from the header row rather than assumed, so a
//! Tesseract that adds a column (or reorders them) still parses.
//!
//! Tesseract TSV columns:
//! level, page_num, block_num, par_num, line_num, word_num,
//! left, top, width, height, conf, text

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;

/// Column layout used when the dump has no header row (Tesseract's default).
const DEFAULT_COLUMNS: [usize; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// Dashes that stand alone in running text: em dash, en dash, horizontal bar.
/// A dash glued to its neighbours is a layout accident of the OCR, not part of
/// the word, so each one is padded on both sides.
const SPACED_DASHES: [char; 3] = ['\u{2014}', '\u{2013}', '\u{2015}'];

/// Header names in the order the parser wants them.
const COLUMN_NAMES: [&str; 12] = [
    "level", "page_num", "block_num", "par_num", "line_num", "word_num", "left", "top", "width",
    "height", "conf", "text",
];

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

/// Reconstruct paragraphs from one page of Tesseract TSV.
///
/// `known_words` holds lowercase, unhyphenated dictionary entries of the
/// document's language (e.g. "sprachverarbeitung"). It is only consulted for
/// ordinary line-break hyphens: `Sprach-\nverarbeitung` is joined when the
/// joined word is known, and left as `Sprach- verarbeitung` otherwise, since a
/// real compound written with a hyphen is indistinguishable without a
/// dictionary. Soft hyphens are always removed at line boundaries, and
/// standalone dashes (`—`, `–`, `―`) are padded on both sides.
pub fn reconstruct_paragraphs(
    tsv: &str,
    known_words: &HashSet<String>,
) -> Result<String, OcrError> {
    let mut lines = parse_lines(tsv)?;

    if lines.is_empty() {
        return Ok(String::new());
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

    Ok(paragraphs
        .iter()
        .map(|paragraph| join_paragraph(paragraph, known_words))
        .collect::<Vec<_>>()
        .join("\n\n"))
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
// Line joining and hyphen repair
// ---------------------------------------------------------------------------

fn join_paragraph(paragraph: &[OcrLine], known_words: &HashSet<String>) -> String {
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

        if join_hyphenated(
            &mut text,
            &previous_last,
            &line_text,
            next_first,
            known_words,
        ) {
            continue;
        }

        text.push(' ');
        text.push_str(&line_text);
    }

    space_dashes(&text)
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

/// Fold `previous_last` and `next_first` into one word when the previous line
/// ended in a break hyphen. Returns whether the text was repaired.
fn join_hyphenated(
    text: &mut String,
    previous_last: &str,
    line_text: &str,
    next_first: &str,
    known_words: &HashSet<String>,
) -> bool {
    if next_first.is_empty() || previous_last.is_empty() {
        return false;
    }

    let has_soft_hyphen = previous_last.ends_with('\u{00AD}');
    let has_ascii_hyphen = previous_last.ends_with('-');

    if !has_soft_hyphen && !has_ascii_hyphen {
        return false;
    }

    let fragment = previous_last.trim_end_matches(['\u{00AD}', '-']);

    // Nothing to glue the next word onto (e.g. a lone dash as its own word).
    if fragment.is_empty() {
        return false;
    }

    // Only repair normal hyphens when the dictionary confirms the
    // reconstructed German word.
    if !has_soft_hyphen {
        let candidate = format!("{}{}", fragment, next_first).to_lowercase();
        if !known_words.contains(&candidate) {
            return false;
        }
    }

    let Some(position) = text.rfind(previous_last) else {
        return false;
    };

    // Drop the hyphen, keep the rest of the next line, then insert its first
    // word straight after the fragment (no space).
    text.replace_range(position..position + previous_last.len(), fragment);

    let rest = match line_text.find(next_first) {
        Some(start) => &line_text[start + next_first.len()..],
        None => "",
    };
    text.push_str(rest);
    text.insert_str(position + fragment.len(), next_first);

    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext";

    fn words(dict: &[&str]) -> HashSet<String> {
        dict.iter().map(|w| w.to_lowercase()).collect()
    }

    #[test]
    fn joins_lines_and_repairs_known_hyphenation() {
        // A hyphenated word is split across two Tesseract *lines* (line_num 1/2).
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tSprach-\n\
             5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t95.0\tverarbeitung\n\
             5\t1\t1\t1\t3\t1\t10\t70\t40\t20\t90.0\tist\n\
             5\t1\t1\t1\t3\t2\t60\t70\t40\t20\t92.0\tschwer\n"
        );

        let text = reconstruct_paragraphs(&tsv, &words(&["sprachverarbeitung"])).unwrap();
        assert_eq!(text, "Sprachverarbeitung ist schwer");
    }

    #[test]
    fn keeps_ascii_hyphen_without_dictionary_confirmation() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tSprach-\n\
             5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t95.0\tverarbeitung\n"
        );

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
        assert_eq!(text, "Sprach- verarbeitung");
    }

    #[test]
    fn always_removes_soft_hyphen_at_line_end() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tSprach\u{00AD}\n\
             5\t1\t1\t1\t2\t1\t10\t40\t60\t20\t95.0\tverarbeitung\n"
        );

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
        assert_eq!(text, "Sprachverarbeitung");
    }

    #[test]
    fn pads_dashes_glued_to_words() {
        // Tesseract lost the spaces around the dash: one glued token.
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tWort\u{2014}Wort\n"
        );

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
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

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
        assert_eq!(text, "Wort — Wort");
    }

    #[test]
    fn pads_a_dash_at_the_end_of_a_line() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tEnde\u{2014}\n\
             5\t1\t1\t1\t2\t1\t10\t40\t50\t20\t96.5\tWeiter\n"
        );

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
        assert_eq!(text, "Ende — Weiter");
    }

    #[test]
    fn keeps_numeric_ranges_tight() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\t2020\u{2013}2021\n"
        );

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
        assert_eq!(text, "2020–2021");
    }

    #[test]
    fn splits_paragraphs_on_a_vertical_gap() {
        let tsv = format!(
            "{HEADER}\n\
             5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tErster\n\
             5\t1\t1\t1\t1\t2\t70\t10\t50\t20\t96.5\tAbsatz\n\
             5\t1\t2\t1\t1\t1\t10\t120\t50\t20\t96.5\tZweiter\n\
             5\t1\t2\t1\t1\t2\t70\t120\t50\t20\t96.5\tAbsatz\n"
        );

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
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

        let text = reconstruct_paragraphs(&tsv, &words(&[])).unwrap();
        assert_eq!(text, "Erster\n\nZweiter");
    }

    #[test]
    fn reads_columns_from_the_header_order() {
        // Complete header, but `left` and `top` swapped relative to the default.
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\ttop\tleft\twidth\theight\tconf\ttext\n\
                   5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96.5\tHallo\n\
                   5\t1\t1\t1\t1\t2\t10\t70\t50\t20\t95.0\tWelt\n";

        let text = reconstruct_paragraphs(tsv, &words(&[])).unwrap();
        assert_eq!(text, "Hallo Welt");
    }

    #[test]
    fn rejects_rows_with_missing_columns() {
        let tsv = format!("{HEADER}\n5\t1\t1\t1\t1\n");
        assert!(reconstruct_paragraphs(&tsv, &words(&[])).is_err());
    }

    #[test]
    fn empty_input_yields_empty_text() {
        assert_eq!(reconstruct_paragraphs("", &words(&[])).unwrap(), "");
    }
}
