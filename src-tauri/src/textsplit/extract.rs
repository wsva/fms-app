//! Core SaT extraction logic.
//!
//! Ported from wtsplit-rs `extract.rs` (SaT path only — WtP dropped). Handles:
//! - tokenizing + chunking with overlapping windows (CLS/SEP wrapped)
//! - batched inference through the ONNX model
//! - weighted aggregation of overlapping predictions
//! - mapping token-level logits back to character-level

use ndarray::{s, Array2};
use tokenizers::Tokenizer;

use crate::textsplit::consts::{NEWLINE_INDEX, CLS_TOKEN_ID, SEP_TOKEN_ID};
use crate::textsplit::model::OnnxModel;
use crate::textsplit::utils::{byte_to_char_offsets, sigmoid, token_to_char_probs};
use crate::textsplit::Result;

/// Weighting scheme for aggregating overlapping chunk predictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Hat is available for callers; Uniform is the default path.
pub enum Weighting {
    /// All positions weighted equally.
    Uniform,
    /// Triangular (hat) weighting — higher weight in the center.
    Hat,
}

impl Default for Weighting {
    fn default() -> Self {
        Weighting::Uniform
    }
}

/// Configuration for extraction.
#[derive(Debug, Clone)]
pub struct ExtractConfig {
    pub block_size: usize,
    pub stride: usize,
    pub batch_size: usize,
    #[allow(dead_code)] // The SaT path never pads the last batch; kept for parity.
    pub pad_last_batch: bool,
    pub weighting: Weighting,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            block_size: 512,
            stride: 64,
            batch_size: 32,
            pad_last_batch: false,
            weighting: Weighting::Uniform,
        }
    }
}

/// Chunk location tracking.
#[derive(Debug, Clone, Copy)]
struct ChunkLoc {
    text_idx: usize,
    start: usize,
    end: usize,
}

/// Result of extraction: character-level logits for each text.
pub struct ExtractionResult {
    /// Logits per text (character-level), shape per text: [num_chars][num_labels].
    pub logits: Vec<Vec<Vec<f32>>>,
}

/// Extract character-level logits from a batch of texts using a SaT model.
pub fn extract_sat(
    texts: &[&str],
    model: &mut OnnxModel,
    tokenizer: &Tokenizer,
    config: &ExtractConfig,
) -> Result<ExtractionResult> {
    if texts.is_empty() {
        return Ok(ExtractionResult { logits: vec![] });
    }

    // Tokenize all texts (no add_special_tokens; we wrap with CLS/SEP manually).
    let encodings: Vec<_> = texts
        .iter()
        .map(|text| {
            tokenizer
                .encode(*text, false)
                .map_err(|e| format!("Tokenization failed: {}", e))
        })
        .collect::<Result<_>>()?;

    let token_ids: Vec<Vec<u32>> = encodings.iter().map(|e| e.get_ids().to_vec()).collect();
    // `tokenizers` returns byte offsets; every downstream index is a char
    // offset. Convert so boundaries land on the right character (otherwise
    // multi-byte text such as German umlauts splits mid-word).
    let offset_mappings: Vec<Vec<(usize, usize)>> = texts
        .iter()
        .zip(encodings.iter())
        .map(|(text, e)| byte_to_char_offsets(*text, e.get_offsets()))
        .collect();

    let text_lengths: Vec<usize> = token_ids.iter().map(|t| t.len()).collect();

    // Effective block size (accounting for CLS/SEP and the 512 model window).
    let max_len = text_lengths.iter().max().copied().unwrap_or(0);
    let mut block_size = config.block_size.min(max_len);
    if block_size > 510 {
        block_size = 510;
    }

    // Align to the downsampling rate (1 for subword models).
    let downsampling_rate = model.config.downsampling_rate.max(1);
    block_size = ((block_size + downsampling_rate - 1) / downsampling_rate) * downsampling_rate;

    // Total number of chunks across all texts.
    let num_chunks: usize = text_lengths
        .iter()
        .map(|&len| {
            if len <= block_size {
                1
            } else {
                ((len - block_size + config.stride - 1) / config.stride) + 1
            }
        })
        .sum();

    // Preallocate inputs with room for CLS + SEP.
    let seq_len = block_size + 2;
    let mut input_ids = Array2::<i64>::zeros((num_chunks, seq_len));
    let mut attention_mask = Array2::<f32>::zeros((num_chunks, seq_len));
    let mut locs = Vec::with_capacity(num_chunks);

    let mut current_chunk = 0;
    for (text_idx, tokens) in token_ids.iter().enumerate() {
        let text_len = tokens.len();
        let mut j = 0;

        loop {
            let mut start = j;
            let mut end = j + block_size;
            let done = end >= text_len;

            if done {
                end = text_len;
                start = end.saturating_sub(block_size);
            }

            let chunk_tokens = &tokens[start..end];
            let chunk_len = chunk_tokens.len();

            input_ids[[current_chunk, 0]] = CLS_TOKEN_ID as i64;
            for (i, &tok) in chunk_tokens.iter().enumerate() {
                input_ids[[current_chunk, i + 1]] = tok as i64;
            }
            input_ids[[current_chunk, chunk_len + 1]] = SEP_TOKEN_ID as i64;

            for i in 0..(chunk_len + 2) {
                attention_mask[[current_chunk, i]] = 1.0;
            }

            locs.push(ChunkLoc {
                text_idx,
                start,
                end,
            });
            current_chunk += 1;

            if done {
                break;
            }
            j += config.stride;
        }
    }

    let num_chunks = current_chunk;
    let input_ids = input_ids.slice(s![..num_chunks, ..]).to_owned();
    let attention_mask = attention_mask.slice(s![..num_chunks, ..]).to_owned();
    let locs = &locs[..num_chunks];

    // Aggregation weights.
    let weights: Vec<f32> = match config.weighting {
        Weighting::Uniform => vec![1.0; block_size],
        Weighting::Hat => {
            let denom = (block_size.saturating_sub(1)).max(1) as f32;
            (0..block_size)
                .map(|i| 1.0 - ((2.0 * i as f32 / denom) - 1.0).abs())
                .collect()
        }
    };

    let num_labels = model.config.num_labels.max(1);
    let mut all_logits: Vec<Vec<Vec<f32>>> = text_lengths
        .iter()
        .map(|&len| vec![vec![0.0; num_labels]; len])
        .collect();
    let mut all_counts: Vec<Vec<f32>> = text_lengths.iter().map(|&len| vec![0.0; len]).collect();

    let n_batches = (num_chunks + config.batch_size - 1) / config.batch_size.max(1);

    for batch_idx in 0..n_batches {
        let start = batch_idx * config.batch_size;
        let end = (start + config.batch_size).min(num_chunks);

        let batch_input_ids = input_ids.slice(s![start..end, ..]);
        let batch_attention_mask = attention_mask.slice(s![start..end, ..]);

        let batch_logits = model.forward_sat(batch_input_ids, batch_attention_mask)?;

        // Drop CLS and SEP predictions (first and last positions).
        let batch_logits = batch_logits.slice(s![.., 1..-1, ..]);

        for (i, chunk_idx) in (start..end).enumerate() {
            let loc = &locs[chunk_idx];
            let n = loc.end - loc.start;

            for j in 0..n {
                let char_idx = loc.start + j;
                let weight = weights.get(j).copied().unwrap_or(1.0);

                for label in 0..num_labels {
                    all_logits[loc.text_idx][char_idx][label] += weight * batch_logits[[i, j, label]];
                }
                all_counts[loc.text_idx][char_idx] += weight;
            }
        }
    }

    // Average the aggregated logits.
    for (text_logits, text_counts) in all_logits.iter_mut().zip(all_counts.iter()) {
        for (char_logits, &count) in text_logits.iter_mut().zip(text_counts.iter()) {
            if count > 0.0 {
                for logit in char_logits.iter_mut() {
                    *logit /= count;
                }
            }
        }
    }

    // Map token-level logits back to character positions.
    let char_level_logits: Vec<Vec<Vec<f32>>> = texts
        .iter()
        .zip(all_logits.iter())
        .zip(offset_mappings.iter())
        .map(|((text, token_logits), offsets)| {
            token_to_char_probs(text.chars().count(), token_logits, offsets, num_labels)
        })
        .collect();

    Ok(ExtractionResult {
        logits: char_level_logits,
    })
}

/// Convert per-character logits into sentence-boundary probabilities.
pub fn logits_to_probs(logits: &[Vec<f32>]) -> Vec<f32> {
    logits
        .iter()
        .map(|char_logits| {
            if char_logits.len() > NEWLINE_INDEX {
                sigmoid(char_logits[NEWLINE_INDEX])
            } else {
                0.0
            }
        })
        .collect()
}
