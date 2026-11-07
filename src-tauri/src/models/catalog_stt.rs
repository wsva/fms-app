//! The STT model registry — generated data, parsed once.
//!
//! The rows are **not hand-written here**. `scripts/gen_catalog.py` produces
//! `catalog_stt.json` and commits it; this module only deserializes it and owns
//! the vocabulary ([`EngineType`], [`ModelDef`], [`ModelCapabilities`]) the rest
//! of the crate names. Run the script, never edit the JSON, and never retype a
//! URL, a hash, a size or a score by hand:
//!
//! ```text
//! python scripts/gen_catalog.py            # repo root; needs no dependencies
//! ```
//!
//! Each row describes one **artifact** — a file (or tar.gz of files) fetched
//! from one URL at one pinned revision — not an abstract model. So the same
//! weights in two containers are two rows with two ids and two runtimes:
//! `parakeet-v3` is the ONNX directory transcribe-rs loads on every platform,
//! `parakeet-v3-gguf` is the GGUF transcribe-cpp loads on desktop. They are
//! never merged (different bytes, different decoder), and an id is never
//! re-pointed at another artifact, because `model_set_default` persists the id:
//! silently changing what `parakeet-v3` means would swap the runtime under
//! existing user preferences. [`ModelDef::base_model`] is the identity the two
//! rows share, so the UI can present one model with two runtimes.
//!
//! On-disk layout is `<model_root>/<id>/…` for every row, where a single-file
//! artifact is stored under [`ModelDef::artifact_name`] (its real filename when
//! the source names one, otherwise its id — which is what every row downloaded
//! before this file existed uses).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Engine type determines how the model is loaded and used for transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineType {
    /// GGML/GGUF model via transcribe-cpp (Whisper, Parakeet, Breeze, …).
    /// Desktop-only: transcribe-cpp does not cross-compile for Android yet.
    TranscribeCpp,
    /// NVIDIA Parakeet ONNX models
    Parakeet,
    /// Moonshine base ONNX models
    Moonshine,
    /// Moonshine streaming ONNX models
    MoonshineStreaming,
    /// SenseVoice ONNX models
    SenseVoice,
    /// GigaAM ONNX models
    GigaAM,
    /// Canary ONNX models
    Canary,
    /// Cohere ONNX models
    Cohere,
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Named after the runtime, not after Whisper: this engine now loads
            // Parakeet and Canary GGUFs too, and a card that said "Whisper GGUF"
            // under a Parakeet row would be a lie the UI cannot undo.
            EngineType::TranscribeCpp => write!(f, "transcribe-cpp (desktop)"),
            EngineType::Parakeet => write!(f, "Parakeet"),
            EngineType::Moonshine => write!(f, "Moonshine"),
            EngineType::MoonshineStreaming => write!(f, "Moonshine Streaming"),
            EngineType::SenseVoice => write!(f, "SenseVoice"),
            EngineType::GigaAM => write!(f, "GigaAM"),
            EngineType::Canary => write!(f, "Canary"),
            EngineType::Cohere => write!(f, "Cohere"),
        }
    }
}

impl EngineType {
    /// Whether this build can load the engine. transcribe-cpp is a native C++
    /// stack compiled only for desktop targets, so a row of this engine is not
    /// merely unpreferred on a phone — it cannot load at all.
    ///
    /// `clippy::match_like_matches_macro` fires here because `cfg!` expands to a
    /// bool literal, and it proposes `!matches!(self, EngineType::TranscribeCpp)`
    /// — which is only equivalent in a non-desktop build. `desktop` implies `stt`,
    /// so in a desktop build that form would report the transcribe-cpp rows as
    /// unavailable here, hiding the very rows desktop exists to run.
    #[allow(clippy::match_like_matches_macro)]
    pub fn available_in_build(self) -> bool {
        match self {
            EngineType::TranscribeCpp => cfg!(feature = "desktop"),
            // Everything else speaks ONNX, which ships wherever local STT does.
            _ => cfg!(feature = "stt"),
        }
    }
}

/// The container an artifact ships in, which decides what can be probed before
/// download: a GGUF carries the metadata transcribe-cpp reads, a GGML `.bin`
/// and an ONNX directory carry nothing we can read here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Container {
    OnnxDirectory,
    GgmlBin,
    Gguf,
}

/// Finest timestamp granularity the row is known to produce. `None` in this
/// field means *unknown*, not *none* — that distinction is the whole point of
/// probing instead of guessing, and it is why `word_timings` treats `None` as
/// "ask the engine" rather than "no".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimestampGranularity {
    None,
    Segment,
    Word,
    Token,
}

impl TimestampGranularity {
    /// Whether this granularity places *words*, as opposed to phrases or nothing.
    ///
    /// The one place the word-or-not test is written, because three layers ask it
    /// (the catalog, the loaded engine, a scored take) and a threshold invented
    /// twice is a threshold that will eventually disagree with itself. A
    /// `Segment` answer is not a failure — it places the take too, just at a
    /// phrase's resolution.
    pub fn yields_words(self) -> bool {
        matches!(self, Self::Word | Self::Token)
    }
}

/// What a row advertises it can do. Populated from the model card of the same
/// weights (GGUF rows), from the architecture's behaviour in transcribe-cpp
/// (`.bin` rows), or from the curated ONNX table — and overwritten at load time
/// by the engine's own answer (see `transcribe::EngineCapabilities`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub timestamps: Option<TimestampGranularity>,
    pub streaming: bool,
    pub translate: bool,
    pub lang_detect: bool,
    /// The longest audio the engine will take in one call, in seconds, when the
    /// catalog knows it. `None` = unknown: ask the loaded engine.
    pub max_chunk_secs: Option<f64>,
}

impl ModelCapabilities {
    /// Whether word-level rows can be expected from this artifact.
    ///
    /// Note this is the *advertised* answer; only a loaded engine is authority.
    pub fn word_timings(&self) -> bool {
        self.timestamps.map(TimestampGranularity::yields_words).unwrap_or_default()
    }
}

/// Model definition with everything needed for download, loading, and UI display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDef {
    /// Unique identifier for the artifact, also its directory name.
    pub id: String,
    /// Display name
    pub name: String,
    /// Short description
    pub description: String,
    /// Engine type determines loading and transcription logic
    pub engine: EngineType,
    /// The weights this artifact holds, shared by every container of them.
    pub base_model: String,
    /// transcribe-cpp architecture name, when the row describes a GGML/GGUF
    /// artifact. Validated against `known_architectures` by a test below.
    pub architecture: Option<String>,
    pub container: Container,
    /// HuggingFace repo the artifact was generated from, when there is one.
    pub repo_id: Option<String>,
    /// Repo commit the download URL and the hash were taken at — the pin that
    /// makes a mirror's bytes provably match this row.
    pub revision: Option<String>,
    /// File name inside the artifact directory; absent means "named after the id".
    pub filename: Option<String>,
    /// Quantization label, for GGUF rows that list several.
    pub quant: Option<String>,
    /// Full download URL (CDN or HuggingFace resolve path through a mirror).
    pub blob_url: String,
    /// HuggingFace repository URL for manual download fallback
    pub hf_repo_url: String,
    /// SHA256 hash for integrity verification (if available)
    pub sha256: Option<String>,
    /// Whether the model is a directory (tar.gz archive) or single file
    pub is_directory: bool,
    /// Exact byte count, and where it came from: `head` (measured from the URL),
    /// `handy_catalog`, or `table` (the carried-over estimate).
    pub size_bytes: u64,
    pub size_source: String,
    /// Supported language codes
    pub languages: Vec<String>,
    pub capabilities: ModelCapabilities,
    /// Derived from measured WER/RTF upstream; `None` when nothing measured it.
    pub accuracy_score: Option<f32>,
    pub speed_score: Option<f32>,
    /// Which catalog entry the scores came from, so a stale number is traceable.
    pub scores_from: Option<String>,
    /// Part of the small curated onboarding set.
    pub recommended: bool,
    /// Editorial sort position; lower first. `None` sorts last.
    pub recommended_rank: Option<u32>,
}

impl ModelDef {
    /// The name the artifact file carries inside its directory. Rows downloaded
    /// before this file existed all have `filename: None` semantics on disk, and
    /// they keep resolving to `<dir>/<id>` exactly as before.
    pub fn artifact_name(&self) -> &str {
        self.filename.as_deref().unwrap_or(&self.id)
    }

    /// Rough MB for the UI and the MCP `size_mb` field, from measured bytes.
    pub fn size_mb(&self) -> u64 {
        self.size_bytes / 1_000_000
    }

    /// Whether this build can offer the row at all (see
    /// [`EngineType::available_in_build`]). Platform support is a property of
    /// the engine, never a field on the row, so a GGUF row cannot accidentally
    /// become selectable on a phone by editing the JSON.
    pub fn loadable_here(&self) -> bool {
        self.engine.available_in_build()
    }
}

#[derive(Deserialize)]
struct Catalog {
    #[allow(dead_code)]
    catalog_version: u32,
    #[allow(dead_code)]
    generated_at: String,
    /// Download mirrors tried when HuggingFace is unreachable. Read only by the
    /// test that asserts every mirror-backed row is hash-verified, so nothing in
    /// the shipping code names it.
    #[allow(dead_code)]
    mirrors: Vec<String>,
    /// Architectures transcribe-cpp can load, copied from its registry. Same:
    /// consulted only by the architecture invariant test.
    #[allow(dead_code)]
    known_architectures: Vec<String>,
    models: Vec<ModelDef>,
}

const CATALOG_JSON: &str = include_str!("catalog_stt.json");

static CATALOG: OnceLock<Catalog> = OnceLock::new();

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| match serde_json::from_str(CATALOG_JSON) {
        Ok(c) => c,
        Err(e) => panic!("bundled catalog_stt.json does not parse: {e}"),
    })
}

/// Every row in the catalog, in generation order. Callers that show these to a
/// human filter on [`ModelDef::loadable_here`] and sort by
/// `recommended_rank` — see `state::ModelState::available_models`.
pub fn all() -> &'static [ModelDef] {
    &catalog().models
}

/// Find a model definition by ID.
pub fn find_model(id: &str) -> Option<&'static ModelDef> {
    all().iter().find(|m| m.id == id)
}

/// Get all model IDs.
#[allow(dead_code)]
pub fn model_ids() -> impl Iterator<Item = &'static str> {
    all().iter().map(|m| m.id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog is the trust anchor for every download, so a row that loses
    /// its hash silently disables verification.
    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = all().iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "catalog model ids must be unique");
    }

    #[test]
    fn every_row_has_a_download_url_and_a_size() {
        for m in all() {
            assert!(m.blob_url.starts_with("https://"), "{}: bad url {}", m.id, m.blob_url);
            assert!(m.size_bytes > 0, "{}: size must be known", m.id);
            assert!(
                matches!(m.size_source.as_str(), "head" | "handy_catalog" | "table"),
                "{}: unknown size source {}",
                m.id,
                m.size_source
            );
        }
    }

    /// A mirror path is an untrusted bit-pipe: without a hash we cannot prove
    /// what it gave us, so the generator refuses to emit one. The legacy flat
    /// CDN filenames are first-party uploads that predate that rule and carry no
    /// hash — the distance here is deliberate, so that a row added *through* a
    /// mirror cannot quietly skip verification by looking legacy.
    #[test]
    fn mirror_backed_rows_are_hash_verified() {
        let mirror = catalog()
            .mirrors
            .first()
            .expect("catalog declares at least one mirror")
            .trim_end_matches('/')
            .to_string();
        for m in all().iter().filter(|m| m.repo_id.is_some()) {
            assert!(m.revision.is_some(), "{}: repo-backed row without a pinned revision", m.id);
            assert!(
                m.blob_url.starts_with(&mirror),
                "{}: repo-backed row should be served from the mirror, got {}",
                m.id,
                m.blob_url
            );
            let sha = m
                .sha256
                .as_deref()
                .unwrap_or_else(|| panic!("{}: served from a mirror without a sha256", m.id));
            assert_eq!(sha.len(), 64, "{}: sha256 must be 64 hex chars", m.id);
            assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{}: bad sha", m.id);
        }
    }

    /// An id is the directory on disk, the key in `index.json` and the value
    /// persisted by `model_set_default`. Renaming one orphans a downloaded model
    /// and silently drops the preference that pointed at it, so the ids shipped
    /// before the catalog was generated are frozen here.
    #[test]
    fn legacy_ids_are_still_served() {
        for id in [
            "whisper-small",
            "whisper-medium",
            "whisper-turbo",
            "whisper-large",
            "breeze-asr",
            "parakeet-v2",
            "parakeet-v3",
            "moonshine-base",
            "moonshine-tiny",
            "moonshine-small",
            "moonshine-medium",
            "sense-voice",
            "gigaam-v3",
            "canary-flash",
            "canary-1b",
            "cohere",
        ] {
            assert!(find_model(id).is_some(), "catalog lost the pre-generated id {id}");
        }
    }

    /// Scores are derived from measurements upstream; a number outside the bar
    /// range means the rescale in the generator drifted.
    #[test]
    fn scores_stay_in_range() {
        for m in all() {
            for (label, score) in [("accuracy", m.accuracy_score), ("speed", m.speed_score)] {
                if let Some(s) = score {
                    assert!((0.0..=1.0).contains(&s), "{}: {} score {} out of 0..=1", m.id, label, s);
                }
            }
        }
    }

    /// An architecture transcribe-cpp has no code for loads into an error at
    /// runtime with no hint that the catalog was wrong. Fail the build instead.
    #[test]
    fn architectures_are_known_to_the_engine() {
        for m in all() {
            if let Some(arch) = &m.architecture {
                assert!(
                    catalog().known_architectures.iter().any(|k| k == arch),
                    "{}: architecture {arch:?} is not in the engine's registry",
                    m.id
                );
            }
        }
    }

    /// Sortformer and the VAD/language-id models produce no transcript text;
    /// they belong to another domain, not to a list of STT models.
    #[test]
    fn models_producing_no_text_are_not_listed() {
        for m in all() {
            assert!(
                !matches!(m.architecture.as_deref(), Some("sortformer") | Some("silero_vad") | Some("ecapa_tdnn")),
                "{}: produces no transcription text and must not be offered as an STT model",
                m.id
            );
        }
    }

    /// Two formats of one model are two rows sharing `base_model` — never one
    /// row whose meaning changed underneath a persisted default.
    #[test]
    fn same_weights_appear_as_separate_rows() {
        let parakeet: Vec<&str> = all()
            .iter()
            .filter(|m| m.base_model == "nvidia/parakeet-tdt-0.6b-v3")
            .map(|m| m.id.as_str())
            .collect();
        assert!(
            parakeet.contains(&"parakeet-v3") && parakeet.contains(&"parakeet-v3-gguf"),
            "expected the ONNX and GGUF rows of Parakeet v3 to coexist, got {parakeet:?}"
        );
        let onnx = find_model("parakeet-v3").unwrap();
        let gguf = find_model("parakeet-v3-gguf").unwrap();
        assert_ne!(onnx.blob_url, gguf.blob_url, "separate files");
        assert_eq!(onnx.engine, EngineType::Parakeet);
        assert_eq!(gguf.engine, EngineType::TranscribeCpp);
        assert!(gguf.capabilities.word_timings(), "the GGUF row advertises token rows");
    }

    /// The layout rule the loader depends on: id is the directory, and a single
    /// file keeps its own name only when the source supplied one.
    #[test]
    fn artifact_names_match_the_on_disk_convention() {
        for m in all() {
            if m.is_directory {
                assert!(m.filename.is_none(), "{}: an archive unpacks, it has no file name", m.id);
            }
            assert!(!m.artifact_name().is_empty());
            assert!(
                !m.artifact_name().contains('/'),
                "{}: artifact name must be one path component",
                m.id
            );
        }
        // Legacy rows name their file after the id, which is what is already on
        // disk; only the generated mirror rows carry a real filename.
        assert_eq!(find_model("whisper-small").unwrap().artifact_name(), "whisper-small");
        assert_ne!(
            find_model("parakeet-v3-gguf").unwrap().artifact_name(),
            "parakeet-v3-gguf",
            "the GGUF row must keep the name its repo gives the file"
        );
    }
}
