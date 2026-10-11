//! Constants for the vendored SaT (Segment any Text) port.
//!
//! Ported from wtsplit-rs `constants.rs`, reduced to the values the subword SaT
//! path actually reads (all WtP / language-adapter constants are dropped).

/// Index of the newline / sentence-boundary logit in the model output.
pub const NEWLINE_INDEX: usize = 0;

/// Default sentence-boundary probability threshold (matches Python wtpsplit).
pub const DEFAULT_THRESHOLD: f32 = 0.01;

/// XLM-RoBERTa special token ids used to wrap each chunk.
pub const CLS_TOKEN_ID: u32 = 0; // <s>
pub const SEP_TOKEN_ID: u32 = 2; // </s>
/// Pad id — kept for parity with the reference tokenizer; the SaT chunker
/// preallocates zeros and only writes real spans, so it is not read here.
#[allow(dead_code)]
pub const PAD_TOKEN_ID: u32 = 1; // <pad>
