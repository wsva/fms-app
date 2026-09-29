//! Static catalog of HuggingFace models available for download.
//!
//! Covers non-STT models (OCR, TTS, etc.) that users can download via the app
//! and use externally. STT models live in [`model_list_stt`].
//!
//! Each entry defines the repo, required files, download sub-folder, and
//! metadata needed by the model index scanner and download infrastructure.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Definition of a HuggingFace model available for download.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HfModelDef {
    /// Unique identifier — the HuggingFace repo ID (e.g. "microsoft/trocr-base-printed").
    pub id: &'static str,
    /// Human-readable display name.
    pub name: &'static str,
    /// Short description of the model.
    pub description: &'static str,
    /// Model category: "candle-ocr", "candle-tts", etc.
    pub model_type: &'static str,
    /// Sub-folder under `{model_root}/candle/` where files are stored.
    /// Convention: "{type_short}/{owner}/{repo}" e.g. "ocr/microsoft/trocr-base-printed".
    pub folder: &'static str,
    /// Required files to download from the HF repo.
    pub files: &'static [HfModelFile],
    /// Total download size in MB (approximate).
    pub size_mb: u64,
    /// Supported language codes.
    pub languages: &'static [&'static str],
}

/// A single file to download from a HuggingFace repo.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HfModelFile {
    /// File path within the HF repo (e.g. "model.safetensors").
    pub path: &'static str,
    /// Expected file size in bytes (0 if unknown — used for progress tracking).
    pub size_bytes: u64,
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// All available HuggingFace models (non-STT).
pub const MODELS: &[HfModelDef] = &[
    // ========================================================================
    // OCR — TrOCR (microsoft)
    // ========================================================================
    HfModelDef {
        id: "microsoft/trocr-base-printed",
        name: "TrOCR Base Printed",
        description: "OCR for printed text. Good general-purpose model",
        model_type: "candle-ocr",
        folder: "ocr/microsoft/trocr-base-printed",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model.safetensors", size_bytes: 344_000_000 },
            HfModelFile { path: "tokenizer.json", size_bytes: 0 },
        ],
        size_mb: 330,
        languages: &["en"],
    },
    HfModelDef {
        id: "microsoft/trocr-large-printed",
        name: "TrOCR Large Printed",
        description: "Higher accuracy OCR for printed text. Slower",
        model_type: "candle-ocr",
        folder: "ocr/microsoft/trocr-large-printed",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model.safetensors", size_bytes: 862_000_000 },
            HfModelFile { path: "tokenizer.json", size_bytes: 0 },
        ],
        size_mb: 822,
        languages: &["en"],
    },
    HfModelDef {
        id: "microsoft/trocr-base-handwritten",
        name: "TrOCR Base Handwritten",
        description: "OCR optimized for handwritten text",
        model_type: "candle-ocr",
        folder: "ocr/microsoft/trocr-base-handwritten",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model.safetensors", size_bytes: 344_000_000 },
            HfModelFile { path: "tokenizer.json", size_bytes: 0 },
        ],
        size_mb: 330,
        languages: &["en"],
    },
    HfModelDef {
        id: "microsoft/trocr-large-handwritten",
        name: "TrOCR Large Handwritten",
        description: "Higher accuracy OCR for handwritten text. Slower",
        model_type: "candle-ocr",
        folder: "ocr/microsoft/trocr-large-handwritten",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model.safetensors", size_bytes: 862_000_000 },
            HfModelFile { path: "tokenizer.json", size_bytes: 0 },
        ],
        size_mb: 822,
        languages: &["en"],
    },

    // ========================================================================
    // TTS — Parler-TTS (parler-tts)
    // ========================================================================
    HfModelDef {
        id: "parler-tts/parler-tts-mini-v1",
        name: "Parler-TTS Mini V1",
        description: "Fast text-to-speech. English, single speaker",
        model_type: "candle-tts",
        folder: "tts/parler-tts/parler-tts-mini-v1",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model.safetensors", size_bytes: 0 },
            HfModelFile { path: "model.safetensors.index.json", size_bytes: 0 },
        ],
        size_mb: 880,
        languages: &["en"],
    },
    HfModelDef {
        id: "parler-tts/parler-tts-mini-v1.1",
        name: "Parler-TTS Mini V1.1",
        description: "Improved text-to-speech. English, single speaker",
        model_type: "candle-tts",
        folder: "tts/parler-tts/parler-tts-mini-v1.1",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model.safetensors", size_bytes: 0 },
            HfModelFile { path: "model.safetensors.index.json", size_bytes: 0 },
        ],
        size_mb: 880,
        languages: &["en"],
    },
];

// ---------------------------------------------------------------------------
// Lookup helpers
// ---------------------------------------------------------------------------

/// Find a model by its HuggingFace repo ID.
#[allow(dead_code)]
pub fn find_model(id: &str) -> Option<&'static HfModelDef> {
    MODELS.iter().find(|m| m.id == id)
}

/// Get all models of a given type (e.g. "candle-ocr", "candle-tts").
#[allow(dead_code)]
pub fn models_by_type(model_type: &str) -> Vec<&'static HfModelDef> {
    MODELS.iter().filter(|m| m.model_type == model_type).collect()
}

/// Get all model IDs.
#[allow(dead_code)]
pub fn model_ids() -> impl Iterator<Item = &'static str> {
    MODELS.iter().map(|m| m.id)
}

/// Get the download folder for a model, relative to `{model_root}/candle/`.
#[allow(dead_code)]
pub fn model_folder(def: &HfModelDef) -> String {
    def.folder.to_string()
}
