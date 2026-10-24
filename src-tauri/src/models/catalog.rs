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
    /// Optional HuggingFace repo to source `tokenizer.json` from when the model
    /// repo ships weights only (typical for ONNX exports, e.g. SaT models live
    /// without a tokenizer). `None` means the tokenizer is part of `files`.
    pub tokenizer_repo: Option<&'static str>,
    /// Optional HuggingFace repo to source `vocab.json` from. Needed by models
    /// whose own repo ships weights and a *language-pruned* vocabulary while the
    /// consumer needs the full inventory (the phoneme model reads one
    /// `token -> id` map for every phone the model can emit). `None` means the
    /// model does not need a vocabulary file.
    pub vocab_repo: Option<&'static str>,
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

/// The pronunciation model the Models page offers first and that the MCP
/// download tool installs when no id is given. Named here so the catalog entry,
/// the read-aloud status payload and the agent tool cannot drift apart.
pub const DEFAULT_PHONEME_MODEL_ID: &str = "Hochien/wav2vec2-alignment";

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
        tokenizer_repo: None,
        vocab_repo: None,
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
        tokenizer_repo: None,
        vocab_repo: None,
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
        tokenizer_repo: None,
        vocab_repo: None,
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
        tokenizer_repo: None,
        vocab_repo: None,
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
        tokenizer_repo: None,
        vocab_repo: None,
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
        tokenizer_repo: None,
        vocab_repo: None,
        size_mb: 880,
        languages: &["en"],
    },

    // ========================================================================
    // Text segmentation — SaT (segment-any-text)
    // ========================================================================
    // Subword sentence segmentation (XLM-RoBERTa backbone) run via the vendored
    // `crate::textsplit` SaT port on the statically-linked `ort`. Consumed by the
    // desktop "Split Text" page and by the dataset "Split Book" stage (which runs
    // the `-sm` default). The ONNX repos ship weights only, so the
    // tokenizer is fetched from the XLM-RoBERTa backbone (see `tokenizer_repo`).
    HfModelDef {
        id: "segment-any-text/sat-3l-sm",
        name: "SaT 3L Small (Sentence Splitter)",
        description: "Subword sentence segmentation for 85+ languages; the Split Book stage's model",
        model_type: "textsplit",
        folder: "textsplit/segment-any-text/sat-3l-sm",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model_optimized.onnx", size_bytes: 427_756_189 },
        ],
        tokenizer_repo: Some("FacebookAI/xlm-roberta-base"),
        vocab_repo: None,
        size_mb: 430,
        languages: &[],
    },
    HfModelDef {
        id: "segment-any-text/sat-3l",
        name: "SaT 3L (Sentence Splitter)",
        description: "Higher-accuracy subword sentence segmentation for 85+ languages",
        model_type: "textsplit",
        folder: "textsplit/segment-any-text/sat-3l",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model_optimized.onnx", size_bytes: 427_946_487 },
        ],
        tokenizer_repo: Some("FacebookAI/xlm-roberta-base"),
        vocab_repo: None,
        size_mb: 430,
        languages: &[],
    },
    HfModelDef {
        id: "segment-any-text/sat-12l",
        name: "SaT 12L (Sentence Splitter)",
        description: "Most accurate 12-layer sentence segmentation. Slower, larger download",
        model_type: "textsplit",
        folder: "textsplit/segment-any-text/sat-12l",
        files: &[
            HfModelFile { path: "config.json", size_bytes: 0 },
            HfModelFile { path: "model_optimized.onnx", size_bytes: 556_478_056 },
        ],
        tokenizer_repo: Some("FacebookAI/xlm-roberta-base"),
        vocab_repo: None,
        size_mb: 560,
        languages: &[],
    },

    // ========================================================================
    // Pronunciation — wav2vec2 CTC phoneme model
    // ========================================================================
    // The acoustic half of read-aloud pronunciation scoring (GOP): audio in,
    // per-frame log-posteriors over IPA phones out. Multilingual by
    // construction — one espeak inventory covers every language the learner
    // reads in, which is why there is no per-language variant here.
    //
    // Two things about this repo are deliberate and load-bearing:
    //   * `folder` starts with `phoneme/`, so the files land in
    //     `{model_root}/phoneme/…` (a peer of `candle/`), which is where
    //     `read_aloud::phoneme::find_model_dir` looks.
    //   * the per-language `vocab.json` files in that repo are pruned to a single
    //     language's phones, so the full 392-entry map is fetched from the
    //     upstream PyTorch repo via `vocab_repo` instead.
    HfModelDef {
        id: DEFAULT_PHONEME_MODEL_ID,
        name: "Wav2Vec2 Phoneme (IPA)",
        description: "Phone-level model for read-aloud pronunciation scoring. 392 IPA phones, 30+ languages",
        model_type: "onnx-phoneme",
        folder: "phoneme/Hochien/wav2vec2-alignment",
        files: &[HfModelFile { path: "wav2vec2-lv60-espeak-fp16.onnx", size_bytes: 632_239_986 }],
        tokenizer_repo: None,
        vocab_repo: Some("facebook/wav2vec2-lv-60-espeak-cv-ft"),
        size_mb: 603,
        languages: &[],
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
