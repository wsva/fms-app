//! Post-processing for text a *vision model* transcribed (`ollama`).
//!
//! Tesseract answers in the page's own words; a VLM answers in **markdown**, and
//! GLM-OCR renders footnote and reference marks as LaTeX superscripts because
//! that is how the renderer it was trained on spelled them:
//!
//! ```text
//! … eines überragend tugendhaften Herrschers, $ ^{179} $ konkret sei die
//! Staatsform aber … zu wählen. $ ^{180} $ Monarchie
//! ```
//!
//! The number is a reference marker attached to the preceding word, not part of
//! the sentence — a dictation card carrying `$ ^{179} $` mid-sentence is
//! unusable. Tesseract never emitted that markup, so this pass belongs to the
//! model path only and runs there unconditionally.
//!
//! Every rule is deterministic: no model is consulted, the same page always
//! yields the same text, and the counts let the UI say what it removed instead
//! of silently rewriting the answer.

use regex::Regex;

/// What one cleanup pass did.
#[derive(serde::Serialize)]
pub struct Cleaned {
    /// The text with every rule applied.
    pub text: String,
    /// Reference/footnote marks removed (`$ ^{179} $`, `<sup>12</sup>`, …).
    pub references: usize,
    /// Other markup unwrapped or dropped (fences, headings, emphasis, `\text{}`).
    pub markup: usize,
}

/// One pattern → replacement rule, counted into either column of [`Cleaned`].
struct Rule {
    pattern: Regex,
    replacement: &'static str,
    /// True when the rule drops a reference mark rather than markup.
    reference: bool,
}

lazy_static::lazy_static! {
    /// Applied in order; the whitespace rules come last so the deletions above
    /// do not each have to fix their own gaps.
    static ref RULES: Vec<Rule> = vec![
        // `$ ^{179} $`, `^{179}`, `$^{12}$`: a superscript whose content is a
        // reference number. The mark is replaced by one space rather than by
        // nothing: it always stood *between* words ("Herrschers, | konkret"),
        // and a deletion would glue them together. The whitespace rules below
        // then collapse the double space that leaves. The odd dashes cover
        // ranges like `^{12–14}`. Roman numerals are in the same rule because
        // front matter numbers its notes `i`, `ii`, `iv` rather than in digits.
        Rule {
            pattern: Regex::new(
                r"\$?[ \t]*\^[ \t]*\{[ \t]*(?:\d[\d \t,.;:–—-]*|[ivxlcdmIVXLCDM]{1,7})[ \t]*\}[ \t]*\$?",
            )
            .unwrap(),
            replacement: " ",
            reference: true,
        },
        // The HTML spelling of the same mark.
        Rule {
            pattern: Regex::new(r"[ \t]*<sup>[^<]{0,24}</sup>").unwrap(),
            replacement: " ",
            reference: true,
        },
        // The LaTeX spelling.
        Rule {
            pattern: Regex::new(r"[ \t]*\\textsuperscript[ \t]*\{[^{}]{0,24}\}").unwrap(),
            replacement: " ",
            reference: true,
        },
        // `## Heading` → `Heading`: the page's text, not the model's structure.
        Rule {
            pattern: Regex::new(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+").unwrap(),
            replacement: "",
            reference: false,
        },
        // `**bold**` / `__bold__` → `bold`. Both alternatives are kept in one
        // pattern so the count is one per emphasis span.
        Rule {
            pattern: Regex::new(r"\*\*([^*\n]+)\*\*|__([^_\n]+)__").unwrap(),
            replacement: "${1}${2}",
            reference: false,
        },
        // `\text{…}`, `\mathrm{…}`, `\textbf{…}` → the content alone. This is
        // what a rendered word arrives as when the page set it in small caps or
        // mathtext; the words are the page's, the wrapper is the model's.
        Rule {
            pattern: Regex::new(
                r"\\(?:text|textrm|textsf|texttt|textnormal|mathrm|mathit|textbf|textit|emph|operatorname)\s*\{([^{}]*)\}",
            )
            .unwrap(),
            replacement: "${1}",
            reference: false,
        },
        // Whitespace the removals (and the model) left behind.
        Rule {
            pattern: Regex::new(r"[ \t]{2,}").unwrap(),
            replacement: " ",
            reference: false,
        },
        // A space that ended up in front of a comma or a full stop (a mark that
        // sat before the punctuation). No language spaces those, unlike `:` and
        // `?`, which French does — so those are left alone.
        Rule {
            pattern: Regex::new(r"[ \t]+([,.])").unwrap(),
            replacement: "${1}",
            reference: false,
        },
        Rule {
            pattern: Regex::new(r"(?m)[ \t]+$").unwrap(),
            replacement: "",
            reference: false,
        },
        Rule {
            pattern: Regex::new(r"\n{3,}").unwrap(),
            replacement: "\n\n",
            reference: false,
        },
    ];
}

/// Clean one model answer.
pub fn clean(input: &str) -> Cleaned {
    let (mut text, fences) = unwrap_fence(input);
    let mut markup = fences;
    let mut references = 0;

    for rule in RULES.iter() {
        let hits = rule.pattern.find_iter(&text).count();
        if hits == 0 {
            continue;
        }
        if rule.reference {
            references += hits;
        } else {
            markup += hits;
        }
        text = rule.pattern.replace_all(&text, rule.replacement).into_owned();
    }

    Cleaned {
        text: text.trim().to_string(),
        references,
        markup,
    }
}

/// Strip one fence pair wrapped around the *whole* answer
/// (```` ```text\n … \n``` ````), which is how a chat model quotes a page. Only
/// a fence that opens the first line and closes the last one is removed —
/// anything else is text the page may itself contain.
fn unwrap_fence(text: &str) -> (String, usize) {
    let trimmed = text.trim();
    let Some(newline) = trimmed.find('\n') else {
        return (text.to_string(), 0);
    };
    if !trimmed.starts_with("```") || !trimmed.ends_with("```") || trimmed.len() < newline + 4 {
        return (text.to_string(), 0);
    }
    (trimmed[newline + 1..trimmed.len() - 3].trim().to_string(), 1)
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn reference_marks_leave_one_space() {
        let raw = "eines überragend tugendhaften Herrschers, $ ^{179} $ konkret sei die \
                   Staatsform aber je nach den historischen, sozialen, ökonomischen Umständen \
                   zu wählen. $ ^{180} $ Monarchie";
        let out = clean(raw);
        assert_eq!(
            out.text,
            "eines überragend tugendhaften Herrschers, konkret sei die Staatsform aber je nach \
             den historischen, sozialen, ökonomischen Umständen zu wählen. Monarchie"
        );
        assert_eq!(out.references, 2);
    }

    #[test]
    fn reference_glued_to_the_word_is_dropped() {
        let out = clean("Herrschers$^{12}$ konkret");
        assert_eq!(out.text, "Herrschers konkret");
        assert_eq!(out.references, 1);
    }

    #[test]
    fn roman_front_matter_marks_are_dropped_too() {
        let out = clean("Vorwort $ ^{iv} $ und Text");
        assert_eq!(out.text, "Vorwort und Text");
        assert_eq!(out.references, 1);
    }

    #[test]
    fn a_mark_before_the_full_stop_does_not_leave_a_space() {
        let out = clean("Ende$^{7}$.");
        assert_eq!(out.text, "Ende.");
        assert_eq!(out.references, 1);
    }

    #[test]
    fn fenced_answer_is_unwrapped_and_markdown_removed() {
        let out = clean("```text\n## Kapitel\n**fett** und __auch__\n```");
        assert_eq!(out.text, "Kapitel\nfett und auch");
        assert_eq!(out.references, 0);
        assert!(out.markup >= 3);
    }

    #[test]
    fn latex_text_wrapper_keeps_the_words() {
        let out = clean(r"Ein \text{kleiner} Rest");
        assert_eq!(out.text, "Ein kleiner Rest");
    }

    #[test]
    fn plain_text_passes_through() {
        let out = clean("Nur Text.\n\nZweiter Absatz.");
        assert_eq!(out.text, "Nur Text.\n\nZweiter Absatz.");
        assert_eq!(out.references, 0);
        assert_eq!(out.markup, 0);
    }
}
