//! Model configuration for the vendored SaT port.
//!
//! Reduced from wtsplit-rs `config.rs` to only the fields the subword SaT path
//! reads off the XLM-RoBERTa `config.json`. Unknown extra fields in the JSON are
//! ignored by serde, so the stock HF config loads cleanly.

use serde::{Deserialize, Serialize};

/// Subset of an HF `config.json` that SaT needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model type identifier (e.g. "xlm-roberta").
    #[serde(default)]
    pub model_type: String,

    /// Number of token-classification labels (1 for sentence boundary).
    #[serde(default = "default_num_labels")]
    pub num_labels: usize,

    /// Downsampling rate (1 for subword models; only >1 for LACanine/WtP).
    #[serde(default = "default_downsampling_rate")]
    pub downsampling_rate: usize,

    /// Base model name, used to confirm this is an XLM-RoBERTa subword model.
    #[serde(default)]
    pub base_model: String,
}

fn default_num_labels() -> usize {
    1
}

fn default_downsampling_rate() -> usize {
    1
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_type: String::new(),
            num_labels: default_num_labels(),
            downsampling_rate: default_downsampling_rate(),
            base_model: String::new(),
        }
    }
}

impl ModelConfig {
    /// Load configuration from a `config.json` file.
    pub fn from_file(path: &std::path::Path) -> crate::textsplit::Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read config.json: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config.json: {}", e))
    }
}
