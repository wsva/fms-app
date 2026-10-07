//! Text similarity helpers (Ratcliff-Obershelp), shared across the app.
//!
//! Extracted from `align.rs` so the scoring is available on every build target
//! (the alignment module itself is desktop-only). `similarity_score` reproduces
//! Python's `difflib.SequenceMatcher.ratio()` scaled to 0-100.

/// Lowercase, collapse whitespace runs to a single space, strip.
pub(crate) fn normalize(text: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Longest common substring; returns (start_in_a, start_in_b, length).
/// Ties resolve to the earliest block in `a` (matching difflib's preference).
fn longest_common_substring(a: &[char], b: &[char]) -> (usize, usize, usize) {
    let n = a.len();
    let m = b.len();
    let mut best_len = 0usize;
    let mut best_i = 0usize;
    let mut best_j = 0usize;
    let mut prev = vec![0usize; m + 1];
    for i in 1..=n {
        let mut cur = vec![0usize; m + 1];
        for j in 1..=m {
            if a[i - 1] == b[j - 1] {
                cur[j] = prev[j - 1] + 1;
                if cur[j] > best_len {
                    best_len = cur[j];
                    best_i = i - best_len;
                    best_j = j - best_len;
                }
            }
        }
        prev = cur;
    }
    (best_i, best_j, best_len)
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (ai, bj, len) = longest_common_substring(a, b);
    if len == 0 {
        return 0;
    }
    let mut total = len;
    total += matching_chars(&a[..ai], &b[..bj]);
    total += matching_chars(&a[ai + len..], &b[bj + len..]);
    total
}

fn ratcliff_obershelp(a: &[char], b: &[char]) -> f64 {
    let total = a.len() + b.len();
    if total == 0 {
        return 0.0;
    }
    2.0 * matching_chars(a, b) as f64 / total as f64
}

/// Similarity score 0-100 between two strings (port of similarity_score).
pub(crate) fn similarity_score(a: &str, b: &str) -> f64 {
    let na = normalize(a);
    let nb = normalize(b);
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    let ca: Vec<char> = na.chars().collect();
    let cb: Vec<char> = nb.chars().collect();
    if ca.len() > 20 && na == nb {
        return 100.0;
    }
    if na.contains(&nb) || nb.contains(&na) {
        // Length-aware containment: only treat containment as a near-match when the
        // shorter side is a substantial fraction of the longer. Otherwise a tiny
        // fragment (e.g. a short "Ja, tatsächlich.") inside a long multi-sentence
        // group scores 90 and hijacks the alignment.
        let lo = ca.len().min(cb.len()) as f64;
        let hi = ca.len().max(cb.len()) as f64;
        if hi > 0.0 && lo / hi >= 0.6 {
            return 90.0;
        }
    }
    ratcliff_obershelp(&ca, &cb) * 80.0
}
