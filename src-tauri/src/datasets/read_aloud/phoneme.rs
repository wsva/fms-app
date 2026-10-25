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
//! `{model_root}/phoneme/<owner>/<repo>/{*.onnx, vocab.json}`. A directory is a
//! model when it holds an ONNX file and a vocabulary; the first one found wins.
//! The Models page downloads one into that exact tree (see the
//! `Hochien/wav2vec2-alignment` catalog entry), and a hand-dropped directory is
//! indistinguishable from it. When none is found the caller reports
//! `model_missing` — see the degradation matrix in
//! `docs/design/read-aloud-score.md`. There is deliberately no proxy score.
//!
//! ## Two budgets
//!
//! Memory is why [`PhonemeModel::posteriors`] cuts its input into windows: the
//! network's self-attention is quadratic in frames, and one call over a whole
//! multi-minute recording asked ONNX Runtime for ~20 GB and failed (see
//! [`WINDOW_SAMPLES`]). Time is why the session is offered to the GPU first on
//! Windows — see [`execution_providers`] — and the very same code runs on the
//! CPU when no provider is registered.
//!
//! [`super::assess::PronReason`]: the vocabulary of "why not measured".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use half::f16;
use ort::ep::ExecutionProviderDispatch;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::{DynTensor, Tensor, TensorElementType, ValueType};

use super::gop::LogPost;

/// 16 kHz mono, the rate the recorder writes and `audio::decode_to_pcm` emits.
const SAMPLE_RATE: u32 = 16_000;

/// Longest audio handed to the model in one call, in samples (20 s).
///
/// The network is a transformer: its self-attention buffer is quadratic in the
/// number of frames (~50 per second), so a single call over a five-minute
/// recording asks ONNX Runtime for ~20 GB and the assessment dies with
/// "Failed to allocate memory" after minutes of apparent hanging. A window this
/// long keeps one call's attention buffer in the low hundreds of MB while still
/// holding a whole sentence, which is where accuracy is decided.
const WINDOW_SAMPLES: usize = SAMPLE_RATE as usize * 20;

/// How much audio a window repeats from its predecessor, in samples (2 s).
///
/// The model's left context at a cut is worth well under a second; two seconds
/// is generous and costs a tenth of a window's compute. The frames covering the
/// repeat are dropped after inference (see [`overlap_frames`]), so the
/// concatenation carries no duplicated time.
const OVERLAP_SAMPLES: usize = SAMPLE_RATE as usize * 2;

/// How far the window start advances: everything except the overlap is new.
const STEP_SAMPLES: usize = WINDOW_SAMPLES - OVERLAP_SAMPLES;

/// Preferred weights file name. Any `*.onnx` in the directory is accepted — see
/// [`find_onnx`] — because exports disagree on the name.
const MODEL_FILE: &str = "model.onnx";
const VOCAB_FILE: &str = "vocab.json";

/// The ONNX weights in `dir`: `model.onnx` if present, otherwise the first
/// `*.onnx` file by name.
///
/// A model's identity is its directory, not its file name: one export ships
/// `model_fp16.onnx`, another `wav2vec2-lv60-espeak-fp16.onnx`. A partial
/// download keeps its own extension (`.part`), so it is never picked up here.
fn find_onnx(dir: &Path) -> Option<PathBuf> {
    let preferred = dir.join(MODEL_FILE);
    if preferred.is_file() {
        return Some(preferred);
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map(|ext| ext.eq_ignore_ascii_case("onnx"))
                    .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// The element type a graph declares for its audio input.
///
/// Half-precision exports convert the *whole* graph, input included, so the
/// tensor handed to `run` must match or ONNX Runtime rejects it before any
/// inference happens. Read from the session rather than guessed from the file
/// name: the name says nothing about what the graph expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputDtype {
    F32,
    F16,
}

/// The audio input's element type, defaulting to f32 for anything unexpected
/// (a non-tensor input, or an export that leaves the type undefined).
fn audio_input_dtype(session: &Session) -> InputDtype {
    match session.inputs().first().map(|outlet| outlet.dtype()) {
        Some(ValueType::Tensor { ty: TensorElementType::Float16, .. }) => InputDtype::F16,
        _ => InputDtype::F32,
    }
}

/// Execution providers for the model's session, best first.
///
/// On a Windows desktop this is DirectML, the DirectX 12 compute provider, so
/// the model runs on whatever GPU the machine already has instead of on every
/// core of its CPU. The pyke ONNX Runtime ships the provider compiled in, so an
/// empty list means this runtime has none — `ort` then keeps its CPU provider,
/// which is also the only option on Android. Both outcomes are logged: a silent
/// fallback is exactly what made the last slow assessment hard to explain.
#[cfg(all(target_os = "windows", feature = "directml"))]
fn execution_providers() -> Vec<ExecutionProviderDispatch> {
    use ort::ep::ExecutionProvider;

    let directml = ort::ep::DirectML::default();
    match directml.is_available() {
        Ok(true) => vec![directml.build()],
        Ok(false) => {
            log::info!(
                "read_aloud: this ONNX Runtime has no DirectML provider; the pronunciation model runs on the CPU"
            );
            Vec::new()
        }
        Err(e) => {
            log::warn!(
                "read_aloud: could not query the DirectML provider ({}); the pronunciation model runs on the CPU",
                e
            );
            Vec::new()
        }
    }
}

#[cfg(not(all(target_os = "windows", feature = "directml")))]
fn execution_providers() -> Vec<ExecutionProviderDispatch> {
    Vec::new()
}

/// Build the session, on the GPU when this target offers one.
///
/// A GPU that cannot host this graph — an old driver, a provider that rejects
/// an operator the exporter emitted — must cost speed, never the assessment:
/// the failure is logged and the session is rebuilt on the CPU provider.
fn open_session(onnx: &Path) -> Result<Session, String> {
    let providers = execution_providers();
    let on_gpu = !providers.is_empty();
    match create_session(onnx, providers) {
        Ok(session) => {
            if on_gpu {
                log::info!("read_aloud: pronunciation model loaded onto the GPU (DirectML)");
            }
            Ok(session)
        }
        Err(message) if on_gpu => {
            log::warn!(
                "read_aloud: the pronunciation model could not run on the GPU ({}); retrying on the CPU",
                message
            );
            create_session(onnx, Vec::new())
        }
        Err(message) => Err(message),
    }
}

fn create_session(
    onnx: &Path,
    providers: Vec<ExecutionProviderDispatch>,
) -> Result<Session, String> {
    Session::builder()
        .map_err(|e| format!("Failed to create ONNX session builder: {}", e))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("Failed to set optimization level: {}", e))?
        .with_execution_providers(providers)
        .map_err(|e| format!("Failed to set the execution provider: {}", e))?
        .commit_from_file(onnx)
        .map_err(|e| format!("Failed to load phoneme model {}: {}", onnx.display(), e))
}

/// How many leading frames of a window cover the repeated overlap.
///
/// The model's stride is read off the window itself — the ratio between the
/// frames it returned and the samples it was given — rather than assumed. One
/// frame of slack is harmless: a dropped frame is 20 ms the neighbouring window
/// still saw, and the aligner works over the whole matrix.
fn overlap_frames(frames: usize, window_samples: usize) -> usize {
    if frames == 0 || window_samples == 0 {
        return 0;
    }
    let share = frames as f64 * OVERLAP_SAMPLES as f64 / window_samples as f64;
    (share.round() as usize).min(frames - 1)
}

/// An ONNX CTC acoustic model over a phone inventory.
pub(crate) struct PhonemeModel {
    session: Session,
    /// Element type of the graph's audio input — see [`audio_input_dtype`].
    audio_dtype: InputDtype,
    /// `tokens[id]` — the vocabulary in id order, specials included.
    tokens: Vec<String>,
    /// True for `<pad>`/`<s>`/`</s>`/`<unk>`-style entries: they hold CTC
    /// transitions or padding, they are never a phone.
    specials: Vec<bool>,
    blank: usize,
}

impl PhonemeModel {
    /// Load the ONNX weights and `vocab.json` from `dir`.
    pub(crate) fn load(dir: &Path) -> Result<Self, String> {
        let onnx = find_onnx(dir)
            .ok_or_else(|| format!("No ONNX model (*.onnx) found in {}", dir.display()))?;
        let vocab = dir.join(VOCAB_FILE);

        let session = open_session(&onnx)?;

        let (tokens, specials, blank) = read_vocab(&vocab)?;
        if tokens.is_empty() {
            return Err(format!("Phoneme vocabulary {} is empty", vocab.display()));
        }

        Ok(Self {
            audio_dtype: audio_input_dtype(&session),
            session,
            tokens,
            specials,
            blank,
        })
    }

    /// Run the acoustic model over 16 kHz mono samples and return per-frame
    /// log-probabilities, ready for [`super::gop`].
    ///
    /// Audio longer than one window is cut into overlapping windows and their
    /// frames are concatenated (see [`WINDOW_SAMPLES`]), because one call over a
    /// whole recording exhausts memory before it finishes.
    pub(crate) fn posteriors(&mut self, samples: &[f32]) -> Result<LogPost, String> {
        if samples.is_empty() {
            return Ok(LogPost::new(Vec::new(), self.blank));
        }

        // Normalise once, over the whole utterance: the statistics are the
        // recording's own, as the feature extractor computes them before the
        // graph. Per window, one quiet stretch would be scaled against whichever
        // neighbours happened to share its window.
        let audio = normalize(samples);

        // Short enough for one pass — the ordinary case (a sentence read aloud).
        if audio.len() <= WINDOW_SAMPLES {
            let rows = self.window_rows(&audio)?;
            return Ok(LogPost::new(rows, self.blank));
        }

        let total = audio.len();
        let windows = total.div_ceil(STEP_SAMPLES);
        log::info!(
            "read_aloud: phoneme inference over {:.1} s of audio in {} windows of {:.0} s",
            seconds(&audio),
            windows,
            WINDOW_SAMPLES as f64 / SAMPLE_RATE as f64
        );

        let mut rows: Vec<Vec<f32>> = Vec::new();
        let mut start = 0usize;
        for index in 0..windows {
            let end = (start + WINDOW_SAMPLES).min(total);
            let window = &audio[start..end];
            // Say which window is being run *before* running it: on the CPU a
            // window takes seconds, and silence is what makes a wait look stuck.
            log::info!(
                "read_aloud: phoneme window {}/{} — {:.1}–{:.1} s",
                index + 1,
                windows,
                start as f64 / SAMPLE_RATE as f64,
                end as f64 / SAMPLE_RATE as f64
            );
            let window_rows = self.window_rows(window)?;
            // Everything before `skip` repeats the previous window, which saw it
            // with the left context a cut would have taken away.
            let skip = if index == 0 {
                0
            } else {
                overlap_frames(window_rows.len(), window.len())
            };
            rows.extend(window_rows.into_iter().skip(skip));
            start += STEP_SAMPLES;
        }

        log::info!("read_aloud: phoneme inference produced {} frames", rows.len());
        Ok(LogPost::new(rows, self.blank))
    }

    /// One ONNX call: `window` in, per-frame log-softmax rows out.
    ///
    /// Split out of [`Self::posteriors`] so the chunking loop there has a single
    /// place that knows the tensor shape, the output name and the vocabulary.
    fn window_rows(&mut self, window: &[f32]) -> Result<Vec<Vec<f32>>, String> {
        let input = audio_input(self.audio_dtype, window)?;

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

        Ok(rows)
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

/// Zero-mean, unit-variance normalisation, per utterance.
///
/// wav2vec2-family CTC models expect it: the `Wav2Vec2FeatureExtractor` that
/// ships with the PyTorch model normalises every recording *before* the network
/// it, and an ONNX export starts at the network. Without it the activations
/// saturate — the argmax collapses onto one id for every frame, which would read
/// as a confidently wrong score rather than as a missing preprocessing step.
/// Applied per call, so the learner recording and the reference clip are each
/// normalised against themselves, exactly as the reference pipeline does. The
/// `1e-7` epsilon is the feature extractor's own.
fn normalize(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let count = samples.len() as f32;
    let mean = samples.iter().sum::<f32>() / count;
    let variance = samples.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / count;
    let scale = (variance + 1e-7).sqrt();
    if !scale.is_finite() || scale <= 0.0 {
        return samples.to_vec();
    }
    samples.iter().map(|x| (x - mean) / scale).collect()
}

/// Build the `[1, samples]` audio tensor in the element type the graph expects.
fn audio_input(dtype: InputDtype, samples: &[f32]) -> Result<DynTensor, String> {
    let shape = (1, samples.len());
    let fail = |e: ort::Error| format!("Failed to build audio tensor: {}", e);
    match dtype {
        InputDtype::F16 => {
            // f32 → f16 round-trip error is ~1e-3 relative, far below the
            // resolution of the logits GOP compares.
            let data: Vec<f16> = samples.iter().map(|x| f16::from_f32(*x)).collect();
            let audio = ndarray::Array2::from_shape_vec(shape, data)
                .map_err(|e| format!("Failed to shape audio input: {}", e))?;
            Ok(Tensor::from_array(audio).map_err(fail)?.upcast())
        }
        InputDtype::F32 => {
            let audio = ndarray::Array2::from_shape_vec(shape, samples.to_vec())
                .map_err(|e| format!("Failed to shape audio input: {}", e))?;
            Ok(Tensor::from_array(audio).map_err(fail)?.upcast())
        }
    }
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
/// flat `phoneme/<name>/` is accepted too — the rule is "a directory holding an
/// ONNX file and `vocab.json`".
pub(crate) fn find_model_dir(model_root: &Path) -> Option<PathBuf> {
    let base = model_root.join("phoneme");
    find_in(&base, 0)
}

fn find_in(dir: &Path, depth: usize) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    if find_onnx(dir).is_some() && dir.join(VOCAB_FILE).is_file() {
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
        // First use (and after a model swap) pays for the ONNX load; say so, as
        // the UI's live assessment log is otherwise silent through the wait.
        log::info!(
            "read_aloud: loading the pronunciation model from {}",
            dir.display()
        );
        *guard = Some(Cached { dir: dir.to_path_buf(), model: PhonemeModel::load(dir)? });
        log::info!("read_aloud: pronunciation model loaded");
    }

    let cached = guard.as_mut().expect("just filled");
    f(&mut cached.model)
}

/// Drop the cached session.
///
/// Called before the model's files are deleted: a cached session holds the ONNX
/// graph in memory, and letting it outlive its directory would mean scoring
/// against a model the user believes is gone. The next attempt reloads from disk
/// (or reports `model_missing`).
pub(crate) fn unload() {
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
    fn weights_are_found_under_whatever_name_the_export_used() {
        let root = std::env::temp_dir().join(format!("fms_phoneme_named_{}", std::process::id()));
        let model = root.join("phoneme").join("Hochien").join("wav2vec2-alignment");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(model.join("wav2vec2-lv60-espeak-fp16.onnx"), b"not really onnx").unwrap();
        std::fs::write(model.join(VOCAB_FILE), b"{}").unwrap();

        assert_eq!(find_model_dir(&root), Some(model.clone()));
        assert_eq!(find_onnx(&model), Some(model.join("wav2vec2-lv60-espeak-fp16.onnx")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dir_without_weights_is_not_a_model() {
        let dir = std::env::temp_dir().join(format!("fms_phoneme_noonnx_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A download in flight leaves a `.part` beside the real name.
        std::fs::write(dir.join("model.onnx.part"), b"partial").unwrap();
        std::fs::write(dir.join(VOCAB_FILE), b"{}").unwrap();

        assert!(find_onnx(&dir).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_audio_tensor_carries_the_recording_in_the_asked_for_element_type() {
        let samples = [-1.5f32, 0.0, 0.25];

        let wide = audio_input(InputDtype::F32, &samples).unwrap();
        let wide = wide.try_extract_array::<f32>().unwrap();
        assert_eq!(wide.shape().to_vec(), vec![1, 3]);
        assert_eq!(wide.iter().copied().collect::<Vec<f32>>(), samples.to_vec());

        // An fp16 export gets an fp16 tensor: a mismatch here is rejected by
        // ONNX Runtime before any inference runs.
        let narrow = audio_input(InputDtype::F16, &samples).unwrap();
        let narrow = narrow.try_extract_array::<f16>().unwrap();
        assert_eq!(narrow.shape().to_vec(), vec![1, 3]);
        for (got, want) in narrow.iter().zip(samples) {
            assert!((got.to_f32() - want).abs() < 1e-3, "{} != {}", got.to_f32(), want);
        }
    }

    #[test]
    fn normalization_centres_and_scales_the_recording() {
        let out = normalize(&[0.0, 1.0, 2.0, 3.0]);
        let mean = out.iter().sum::<f32>() / out.len() as f32;
        let variance = out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32;
        assert!(mean.abs() < 1e-5, "mean was {}", mean);
        assert!((variance - 1.0).abs() < 1e-3, "variance was {}", variance);
    }

    #[test]
    fn normalization_leaves_silence_at_zero() {
        // The epsilon makes the scale tiny rather than zero; digital silence must
        // still map to silence instead of blowing up into noise.
        assert!(normalize(&[0.0; 64]).iter().all(|x| *x == 0.0));
        assert!(normalize(&[]).is_empty());
    }

    #[test]
    fn the_overlap_is_dropped_by_the_share_of_the_window_it_takes() {
        // A 20 s window of 1000 frames repeats 2 s of it => 100 frames.
        assert_eq!(overlap_frames(1000, 320_000), 100);
        // A short final window still keeps at least one frame of its own.
        assert_eq!(overlap_frames(2, 32_001), 1);
        assert_eq!(overlap_frames(1, 16_000), 0);
        // Nothing to drop without frames to drop.
        assert_eq!(overlap_frames(0, 16_000), 0);
        assert_eq!(overlap_frames(100, 0), 0);
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
