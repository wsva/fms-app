//! Utility functions for the vendored SaT port.
//!
//! Ported from wtsplit-rs `utils.rs`, keeping the WtP-only helpers (hash
//! encoding) out. Char-index semantics match the Python reference exactly.

/// Sigmoid activation function.
#[inline]
pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Convert split indices to sentences.
///
/// Given text and the character indices where sentences *end*, reconstruct the
/// sentences. When `indices` is empty the whole text is returned as one
/// sentence.
pub fn indices_to_sentences(text: &str, indices: &[usize], strip_whitespace: bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut offset = 0;

    for &idx in indices {
        let mut end_idx = idx + 1;

        // Skip trailing whitespace for the next sentence's start.
        while end_idx < chars.len() && chars[end_idx].is_whitespace() {
            end_idx += 1;
        }

        let sentence: String = chars[offset..end_idx.min(chars.len())].iter().collect();
        let sentence = if strip_whitespace {
            sentence.trim().to_string()
        } else {
            sentence
        };

        if !sentence.is_empty() {
            sentences.push(sentence);
        }

        offset = end_idx;
    }

    // The final sentence (after the last split point).
    if offset < chars.len() {
        let last_sentence: String = chars[offset..].iter().collect();
        let last_sentence = if strip_whitespace {
            last_sentence.trim().to_string()
        } else {
            last_sentence
        };

        if !last_sentence.is_empty() {
            sentences.push(last_sentence);
        }
    }

    // No split points at all → return the whole text as a single sentence.
    if indices.is_empty() && !text.is_empty() {
        let sentence = if strip_whitespace {
            text.trim().to_string()
        } else {
            text.to_string()
        };
        if !sentence.is_empty() {
            return vec![sentence];
        }
    }

    sentences
}

/// Map token-level logits to character-level logits.
///
/// For subword models each token's logit is assigned to the *last* character of
/// that token (per the Python wtpsplit convention). Unmapped characters keep a
/// very negative logit so their boundary probability is ~0.
///
/// `offsets` must already be **char** offsets (see [`byte_to_char_offsets`]);
/// the `tokenizers` crate yields byte offsets, which would misplace every
/// boundary on non-ASCII text.
pub fn token_to_char_probs(
    text_len: usize,
    token_logits: &[Vec<f32>],
    offsets: &[(usize, usize)],
    num_labels: usize,
) -> Vec<Vec<f32>> {
    let mut char_probs = vec![vec![f32::NEG_INFINITY; num_labels]; text_len];

    for (token_idx, &(start, end)) in offsets.iter().enumerate() {
        if start < end && end > 0 && end <= text_len && token_idx < token_logits.len() {
            let char_idx = end - 1;
            char_probs[char_idx] = token_logits[token_idx].clone();
        }
    }

    char_probs
}

/// Remove spaces from text, tracking the original positions of the spaces.
///
/// Used by the `remove_whitespace_before_inference` option.
pub fn remove_spaces(text: &str) -> (String, Vec<usize>) {
    let mut result = String::new();
    let mut space_positions = Vec::new();

    for c in text.chars() {
        if c == ' ' {
            space_positions.push(result.len() + space_positions.len());
        } else {
            result.push(c);
        }
    }

    (result, space_positions)
}

/// Reinsert zero-probability slots at the recorded space positions.
pub fn reinsert_space_probs(probs: &[f32], space_positions: &[usize]) -> Vec<f32> {
    let mut result = probs.to_vec();
    for &pos in space_positions {
        if pos <= result.len() {
            result.insert(pos, 0.0);
        }
    }
    result
}

/// Convert `tokenizers` **byte** offsets into **char** offsets for `text`.
///
/// The Rust `tokenizers` crate reports offsets as byte positions into the
/// original string, while the Python reference (and every downstream step here
/// — `char_probs`, `indices_to_sentences`) indexes by `char`. On single-byte
/// (ASCII) text the two coincide; with multi-byte characters (e.g. German
/// `ö/ä/ü/ß`) byte offsets drift ahead of char offsets in proportion to how
/// many multi-byte chars precede them, which shows up as sentence boundaries
/// landing mid-word. Tokenizer byte offsets are guaranteed to fall on UTF-8
/// char boundaries, so slicing `text[..byte]` is safe.
pub fn byte_to_char_offsets(text: &str, byte_offsets: &[(usize, usize)]) -> Vec<(usize, usize)> {
    byte_offsets
        .iter()
        .map(|&(bstart, bend)| {
            (
                text[..bstart].chars().count(),
                text[..bend].chars().count(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_to_char_offsets_matches_ascii() {
        // "Möglichkeit": 'ö' is 2 UTF-8 bytes, so byte and char offsets differ
        // after it. Char indices must ignore the extra bytes.
        let text = "Möglichkeit ist nah";
        // Byte offset of the space after "Möglichkeit" (11 chars, 12 bytes).
        let b_space = "Möglichkeit".len(); // 12 bytes (11 chars + 1 extra for ö)
        assert_eq!(b_space, 12);
        let got = byte_to_char_offsets(text, &[(0, b_space)]);
        assert_eq!(got, vec![(0, 11)]); // 11 chars in "Möglichkeit"
    }

    #[test]
    fn byte_to_char_offsets_pure_ascii_is_identity() {
        let text = "hello world";
        let got = byte_to_char_offsets(text, &[(0, 5), (6, 11)]);
        assert_eq!(got, vec![(0, 5), (6, 11)]);
    }
}
