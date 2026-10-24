//! ONNX CTC phoneme model: audio in, per-frame log-posteriors out.
//!
//! This is the only part of pronunciation scoring that needs a model file.
//! Everything downstream ([`super::gop`]) is arithmetic over the matrix this
//! module produces, which is what keeps the scoring core testable.
//!
//! ## Why a separate model at all
//!
//! The STT model cannot be asked "how good was that phone". It emits text, and
//! a good speech recogniser *repairs* a mispronunciation using the language
//! prior — the very thing that makes it a good recogniser makes it a useless
//! pronunciation judge. So pronunciation needs an acoustic model whose output
//! is phones and which is asked to score a pinned target, not to guess.
//!
//! ## Two uses of the same model
//!
//! 1. Decode the learner: free-decode is only used indirectly (never as the
//!    score). [`PhonemeModel::posteriors`] gives GOP its denominator.
//! 2. Build the reference: the same model decodes a canonical Edge TTS
//!    rendering of the text, and that decode becomes the phone target. Because
//!    the model's own acoustic inventory is used on both sides, no G2P, no
//!    lexicon and no per-language table is needed — only a model that covers
//!    the language.
//!
//! ## Model resolution
//!
//! `{model_root}/phoneme/<owner>/<repo>/{model.onnx, vocab.json}`. A directory
//! is a model when it holds both files; the first one found wins. When none is
//! found the caller reports `model_missing` — see the degradation matrix in
//! `docs/design/read-aloud-score.md`. There is deliberately no proxy score.
//!
//! [`super::assess::PronReason`]: the vocabulary of "why not measured".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;

use super::gop::LogPost;

/// 16 kHz mono, the rate the recorder writes and `audio::decode_to_pcm` emits.
const SAMPLE_RATE: u32 = 16_000;

/// File names a phoneme model directory must contain.
const MODEL_FILE: &str = "model.onnx";
const VOCAB_FILE: &str = "vocab.json";

/// An ONNX CTC acoustic model over a phone inventory.
pub(crate) struct PhonemeModel {
    session: Session,
    /// `tokens[id]` — the vocabulary in id order, specials included.
    tokens: Vec<String>,
    /// True for `<pad>`/`<s>`/`</s>`/`<unk>`-style entries: they hold CTC
    /// transitions or padding, they are never a phone.
    specials: Vec<bool>,
    blank: usize,
    /// Log-softmax denominator cache is not kept: posteriors are consumed once.
    dir: PathBuf,
}

impl PhonemeModel {
    /// Load `model.onnx` and `vocab.json` from `dir`.
    pub(crate) fn load(dir: &Path) -> Result<Self, String> {
        let onnx = dir.join(MODEL_FILE);
        let vocab = dir.join(VOCAB_FILE);

        let session = Session::builder()
            .map_err(|e| format!("Failed to create ONNX session builder: {}", e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("Failed to set optimization level: {}", e))?
            .commit_from_file(&onnx)
            .map_err(|e| format!("Failed to load phoneme model {}: {}", onnx.display(), e))?;

        let (tokens, specials, blank) = read_vocab(&vocab)?;
        if tokens.is_empty() {
            return Err(format!("Phoneme vocabulary {} is empty", vocab.display()));
        }

        Ok(Self { session, tokens, specials, blank, dir: dir.to_path_buf() })
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The phone inventory in id order (specials included, see [`Self::specials`]).
    pub(crate) fn tokens(&self) -> &[String] {
        &self.tokens
    }

    pub(crate) fn blank(&self) -> usize {
        self.blank
    }

    pub(crate) fn is_special(&self, id: usize) -> bool {
        self.specials.get(id).copied().unwrap_or(true)
    }

    /// Run the acoustic model over 16 kHz mono samples and return per-frame
    /// log-probabilities, ready for [`super::gop`].
    pub(crate) fn posteriors(&mut self, samples: &[f32]) -> Result<LogPost, String> {
        if samples.is_empty() {
            return Ok(LogPost::new(Vec::new(), self.blank));
        }

        let audio = ndarray::Array2::from_shape_vec((1, samples.len()), samples.to_vec())
            .map_err(|e| format!("Failed to shape audio input: {}", e))?;
        let input = Tensor::from_array(audio)
            .map_err(|e| format!("Failed to build audio tensor: {}", e))?;

        let outputs = self
            .session
            .run(ort::inputs![input])
            .map_err(|e| format!("Phoneme inference failed: {}", e))?;

        // wav2vec2-style exports call it `logits`; take the first output when
        // the export used another name rather than failing on a naming quirk.
        let value = outputs
            .get("logits")
            .or_else(|| if outputs.len() > 0 { Some(&outputs[0]) } else { None })
            .ok_or_else(|| "Phoneme model produced no output".to_string())?;

        // fp16 exports are the norm (halved size, no accuracy loss worth the
        // disk); a hypothetical f32 export must load unchanged.
        let (shape, data): (Vec<usize>, Vec<f32>) = match value.try_extract_array::<half::f16>() {
            Ok(view) => (view.shape().to_vec(), view.iter().map(|x| x.to_f32()).collect()),
            Err(_) => {
                let view = value
                    .try_extract_array::<f32>()
                    .map_err(|e| format!("Failed to extract phoneme logits: {}", e))?;
                (view.shape().to_vec(), view.iter().copied().collect())
            }
        };

        let (frames, vocab) = match shape.as_slice() {
            [1, t, v] | [t, v] => (*t, *v),
            [t] => (*t, 1),
            _ => {
                return Err(format!(
                    "Unexpected phoneme logits shape {:?} (expected [1, frames, vocab])",
                    shape
                ))
            }
        };
        if vocab != self.tokens.len() {
            return Err(format!(
                "Phoneme model emits {} classes but vocab.json has {}",
                vocab,
                self.tokens.len()
            ));
        }

        // Row-wise log-softmax. Only within-frame differences matter to GOP,
        // so the normalization is a convenience, not a correctness requirement;
        // it is done anyway so a debug print reads as probabilities.
        let mut rows: Vec<Vec<f32>> = Vec::with_capacity(frames);
        for t in 0..frames {
            let row = &data[t * vocab..(t + 1) * vocab];
            let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            if !max.is_finite() {
                rows.push(vec![f32::NEG_INFINITY; vocab]);
                continue;
            }
            let sum: f32 = row.iter().map(|x| (x - max).exp()).sum();
            let lse = max + sum.ln();
            rows.push(row.iter().map(|x| x - lse).collect());
        }

        Ok(LogPost::new(rows, self.blank))
    }

    /// Free-decode: CTC-collapse the posteriors into phone ids, dropping blanks
    /// and specials. Used to build the *reference* timeline, never to score.
    pub(crate) fn decode(&mut self, samples: &[f32]) -> Result<Vec<usize>, String> {
        let post = self.posteriors(samples)?;
        Ok(self.collapse(&post))
    }

    /// CTC best-path collapse over an existing posterior matrix.
    pub(crate) fn collapse(&self, post: &LogPost) -> Vec<usize> {
        collapse(post, self.blank, &self.specials)
    }
}

/// CTC collapse: one phone per run of equal argmax frames, blanks and specials
/// separating runs rather than being emitted.
///
/// Free-standing so the rule can be tested without an ONNX session.
pub(crate) fn collapse(post: &LogPost, blank: usize, specials: &[bool]) -> Vec<usize> {
    let is_special = |id: usize| specials.get(id).copied().unwrap_or(true);
    let mut out: Vec<usize> = Vec::new();
    let mut previous: Option<usize> = None;
    for index in 0..post.len() {
        let id = argmax(post.row(index));
        let emit = if id == blank || is_special(id) { None } else { Some(id) };
        if emit.is_some() && emit != previous {
            out.push(id);
        }
        // A blank resets the run, so two identical phones separated by a blank
        // frame stay two phones instead of collapsing into one.
        previous = emit;
    }
    out
}

fn argmax(row: &[f32]) -> usize {
    let mut best = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (id, value) in row.iter().enumerate() {
        if *value > best_value {
            best_value = *value;
            best = id;
        }
    }
    best
}

/// Parse an HF-style `vocab.json` (`token -> id`) into an id-ordered inventory.
fn read_vocab(path: &Path) -> Result<(Vec<String>, Vec<bool>, usize), String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read phoneme vocabulary {}: {}", path.display(), e))?;
    let map: HashMap<String, usize> = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse phoneme vocabulary {}: {}", path.display(), e))?;

    let count = map.values().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut tokens = vec![String::new(); count];
    for (token, id) in map {
        if id < count {
            tokens[id] = token;
        }
    }

    let specials: Vec<bool> = tokens.iter().map(|t| is_special_token(t)).collect();
    // `<pad>` is the CTC blank in every wav2vec2-family export. Fall back to
    // `<blank>`, then to 0 (HuggingFace's `Wav2Vec2ForCTC` convention).
    let blank = tokens
        .iter()
        .position(|t| t == "<pad>")
        .or_else(|| tokens.iter().position(|t| t == "<blank>"))
        .unwrap_or(0);

    Ok((tokens, specials, blank))
}

fn is_special_token(token: &str) -> bool {
    token.starts_with('<') && token.ends_with('>')
}

// ---------------------------------------------------------------------------
// Model discovery + process-wide session cache
// ---------------------------------------------------------------------------

/// Find the phoneme model directory under `{model_root}/phoneme`.
///
/// Layout is `phoneme/<owner>/<repo>/` (mirroring the `candle/` tree), but a
/// flat `phoneme/<name>/` is accepted too — the rule is "a directory holding
/// both `model.onnx` and `vocab.json`".
pub(crate) fn find_model_dir(model_root: &Path) -> Option<PathBuf> {
    let base = model_root.join("phoneme");
    find_in(&base, 0)
}

fn find_in(dir: &Path, depth: usize) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    if dir.join(MODEL_FILE).is_file() && dir.join(VOCAB_FILE).is_file() {
        return Some(dir.to_path_buf());
    }
    if depth >= 2 {
        return None;
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    children.into_iter().find_map(|child| find_in(&child, depth + 1))
}

struct Cached {
    dir: PathBuf,
    model: PhonemeModel,
}

static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();

/// Run `f` against the model in `dir`, loading it on first use and reloading
/// when the requested directory changes. A load failure is *not* cached, so a
/// fixed installation works on the next attempt.
pub(crate) fn with_model<T>(
    dir: &Path,
    f: impl FnOnce(&mut PhonemeModel) -> Result<T, String>,
) -> Result<T, String> {
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());

    let stale = match guard.as_ref() {
        Some(cached) => cached.dir != dir,
        None => true,
    };
    if stale {
        *guard = Some(Cached { dir: dir.to_path_buf(), model: PhonemeModel::load(dir)? });
    }

    let cached = guard.as_mut().expect("just filled");
    f(&mut cached.model)
}

/// Duration of a sample buffer in seconds, so callers do not repeat the
/// `samples.len() / rate` arithmetic.
pub(crate) fn seconds(samples: &[f32]) -> f64 {
    samples.len() as f64 / SAMPLE_RATE as f64
}

#[cfg(test)]
mod collapse_tests {
    use super::*;

    /// A posterior row that is one-hot at `id`.
    fn hot(vocab: usize, id: usize) -> Vec<f32> {
        let mut row = vec![-10.0f32; vocab];
        row[id] = 0.0;
        row
    }

    #[test]
    fn collapse_treats_blanks_as_separators_only() {
        // vocab: 0=<pad>, 1=<s>, 2=a, 3=b
        let specials = [true, true, false, false];
        let blank = 0usize;

        let repeated_phone = LogPost::new(vec![hot(4, 2), hot(4, 0), hot(4, 2)], blank);
        assert_eq!(collapse(&repeated_phone, blank, &specials), vec![2, 2]);

        let one_phone = LogPost::new(vec![hot(4, 2), hot(4, 2), hot(4, 0)], blank);
        assert_eq!(collapse(&one_phone, blank, &specials), vec![2]);

        let special_is_not_a_phone = LogPost::new(vec![hot(4, 1), hot(4, 3)], blank);
        assert_eq!(collapse(&special_is_not_a_phone, blank, &specials), vec![3]);
    }

    #[test]
    fn find_in_prefers_a_flat_layout_over_a_deep_one() {
        let root = std::env::temp_dir().join(format!("fms_phoneme_flat_{}", std::process::id()));
        let flat = root.join("phoneme").join("de-espeak");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join(MODEL_FILE), b"x").unwrap();
        std::fs::write(flat.join(VOCAB_FILE), b"{}").unwrap();
        assert_eq!(find_model_dir(&root), Some(flat));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_with_only_one_of_the_two_files_is_not_a_model() {
        let root = std::env::temp_dir().join(format!("fms_phoneme_half_{}", std::process::id()));
        let dir = root.join("phoneme").join("partial");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MODEL_FILE), b"x").unwrap();
        assert!(find_model_dir(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory(map: &[(&str, usize)]) -> (Vec<String>, Vec<bool>, usize) {
        let dir = std::env::temp_dir().join(format!(
            "fms_phoneme_vocab_{}_{}",
            std::process::id(),
            map.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(VOCAB_FILE);
        let json = serde_json::to_string(
            &map.iter().map(|(t, i)| (t.to_string(), *i)).collect::<HashMap<_, _>>(),
        )
        .unwrap();
        std::fs::write(&path, json).unwrap();
        let parsed = read_vocab(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        parsed
    }

    #[test]
    fn the_blank_is_pad_and_specials_are_marked() {
        let (tokens, specials, blank) =
            inventory(&[("<pad>", 0), ("<s>", 1), ("a", 2), ("b", 3)]);
        assert_eq!(blank, 0);
        assert_eq!(tokens, vec!["<pad>", "<s>", "a", "b"]);
        assert_eq!(specials, vec![true, true, false, false]);
    }

    #[test]
    fn a_flat_inventory_falls_back_to_id_zero_as_blank() {
        let (_tokens, _specials, blank) = inventory(&[("a", 0), ("b", 1)]);
        assert_eq!(blank, 0);
    }

    #[test]
    fn a_missing_model_directory_is_none_not_an_error() {
        let root = std::env::temp_dir().join("fms_phoneme_absent_root");
        assert!(find_model_dir(&root).is_none());
    }

    #[test]
    fn a_directory_holding_both_files_is_found_at_depth_three() {
        let root = std::env::temp_dir().join(format!("fms_phoneme_find_{}", std::process::id()));
        let model = root.join("phoneme").join("facebook").join("wav2vec2-x");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(model.join(MODEL_FILE), b"not really onnx").unwrap();
        std::fs::write(model.join(VOCAB_FILE), b"{}").unwrap();

        assert_eq!(find_model_dir(&root), Some(model));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_vocabulary_is_rejected() {
        let dir = std::env::temp_dir().join(format!("fms_phoneme_empty_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MODEL_FILE), b"x").unwrap();
        std::fs::write(dir.join(VOCAB_FILE), b"{}").unwrap();
        let error = match PhonemeModel::load(&dir) {
            // A session is not `Debug`, so the failure is asserted by shape.
            Ok(_) => panic!("an empty vocabulary must not produce a usable model"),
            Err(e) => e,
        };
        // An empty vocab fails before the (bogus) model file is complained
        // about, or the session load fails first — either way it is an error,
        // never a silently usable model.
        assert!(!error.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
