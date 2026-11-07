//! Reading a GGUF file's metadata header without loading the model.
//!
//! Two reasons this exists at all:
//!
//! 1. **Capability before download.** transcribe-cpp takes streaming, translation,
//!    language detection and the supported-language set from the model's own
//!    metadata at load time, and those same keys sit in the first kilobytes of the
//!    file. Reading them is therefore not a guess about the format — it is asking
//!    the artifact the question the engine will ask it later, just earlier and
//!    without the weights.
//! 2. **A discovered file has no catalog row.** Nothing here invents a row: it
//!    answers "what do these bytes claim" so a caller can show it honestly.
//!
//! Timestamp granularity is deliberately *not* read here. transcribe-cpp's shared
//! metadata loader does not read it either, because the ceiling is
//! variant-specific and applied by each family's own `load()` (`stt.capability.timestamps`
//! exists for canary and `stt.capability.word_timestamps` for granite, but neither
//! is the answer for every variant). The header is silent on it, so the field
//! stays `None` and the loaded engine remains the only authority — see
//! [`crate::models::transcribe::ActiveModel::engine_capabilities`].
//!
//! The reader is a cursor over a *prefix* of the file: GGUF stores no length for
//! the metadata block, so a parse that runs out of bytes reports how many it
//! needed and [`read_header`] grows its buffer and tries again. Values that are
//! not asked for are skipped without being materialized, which is what keeps
//! probing cheap: tokenizer vocabularies are arrays of hundreds of thousands of
//! strings, and a capability probe never has to look at them.

use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::path::Path;

/// The four ASCII bytes `GGUF`, read little-endian.
const MAGIC: u32 = 0x4655_4747;
/// Versions the reader walks. v1 has no tensor-info alignment field to skip and
/// is not produced by any converter worth offering; a newer version may lay the
/// file out differently, so it is refused rather than mis-read.
const VERSION_V2: u32 = 2;
const VERSION_V3: u32 = 3;
/// A header with more pairs than this is corrupt, not a model.
const MAX_KV_PAIRS: u64 = 1_000_000;
/// Same idea for array lengths, one order of magnitude tighter: the largest
/// legitimate value we would want from a header is a language list.
const MAX_ARRAY_LEN: u64 = 1_000_000;
/// First read size, then doubled. Model metadata blocks are a few kilobytes to
/// a few megabytes (tokenizer vocabularies are skipped, not read, but they still
/// occupy the block).
const FIRST_READ_BYTES: usize = 1 << 20;
/// Give up growing past this: a metadata section this large is either corrupt or
/// a model whose capabilities are the least interesting thing about it.
const MAX_READ_BYTES: usize = 64 << 20;

// Metadata keys transcribe-cpp reads for ASR models. Named here so a rename
// upstream is a one-line diff in one file.
const KEY_ARCH: &str = "general.architecture";
const KEY_NAME: &str = "general.name";
const KEY_VARIANT: &str = "stt.variant";
const KEY_LANGUAGES: &str = "general.languages";
const KEY_STREAMING: &str = "stt.capability.streaming";
const KEY_TRANSLATE: &str = "stt.capability.translate";
const KEY_LANG_DETECT: &str = "stt.capability.lang_detect";

const PROBE_KEYS: &[&str] = &[
    KEY_ARCH,
    KEY_NAME,
    KEY_VARIANT,
    KEY_LANGUAGES,
    KEY_STREAMING,
    KEY_TRANSLATE,
    KEY_LANG_DETECT,
];

/// A metadata value. Only the shapes a header can hold are represented; the
/// signed/unsigned split is kept because the file says which one it wrote.
#[derive(Debug, Clone, PartialEq)]
pub enum GgufValue {
    Uint(u64),
    Int(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Str(String),
    Array(Vec<GgufValue>),
}

/// The key/value block at the front of a GGUF file. Tensor info and weight data
/// are not parsed and not needed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GgufMetadata {
    kv: BTreeMap<String, GgufValue>,
}

impl GgufMetadata {
    fn get(&self, key: &str) -> Option<&GgufValue> {
        self.kv.get(key)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(GgufValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key) {
            Some(GgufValue::Bool(b)) => Some(*b),
            // Converters disagree on whether a capability flag is a bool or a 0/1
            // int; both mean the same thing, and reading only one loses headers.
            Some(GgufValue::Int(i)) => Some(*i != 0),
            Some(GgufValue::Uint(u)) => Some(*u != 0),
            _ => None,
        }
    }

    /// A string array, when every element is a string. A mixed or non-string
    /// array is `None` — "this key does not answer the question", not "empty".
    pub fn get_string_array(&self, key: &str) -> Option<Vec<String>> {
        match self.get(key) {
            Some(GgufValue::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        GgufValue::Str(s) => out.push(s.clone()),
                        _ => return None,
                    }
                }
                Some(out)
            }
            _ => None,
        }
    }
}

/// Why a header did not yield metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GgufError {
    /// Not the `GGUF` magic — a `.bin` GGML file, a tarball, or garbage.
    NotGguf,
    /// The file could not be opened or read at all (missing, locked, no access).
    /// Distinct from [`GgufError::NotGguf`]: "unknown" must not be reported as
    /// "this is not a model".
    Unreadable(String),
    /// A format version this reader does not lay out correctly.
    UnsupportedVersion(u32),
    /// The buffer ended mid-value; `needed` is the total bytes the parse wants.
    /// The caller is expected to read more and retry, not to treat this as damage.
    Truncated { needed: usize },
    /// Structurally impossible regardless of length: an absurd pair or array
    /// count, or a value type tag that does not exist.
    Malformed(&'static str),
}

impl std::fmt::Display for GgufError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GgufError::NotGguf => write!(f, "not a GGUF file"),
            GgufError::Unreadable(why) => write!(f, "cannot read file: {why}"),
            GgufError::UnsupportedVersion(v) => write!(f, "unsupported GGUF version {v}"),
            GgufError::Truncated { needed } => write!(f, "header needs {needed} bytes"),
            GgufError::Malformed(why) => write!(f, "malformed GGUF header: {why}"),
        }
    }
}

impl std::error::Error for GgufError {}

/// A forward-only cursor that turns running off the end into [`GgufError::Truncated`]
/// carrying the total length the parse asked for.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], GgufError> {
        let end = self.pos.checked_add(n).ok_or(GgufError::Malformed("length overflow"))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(GgufError::Truncated { needed: end })?;
        self.pos = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, GgufError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, GgufError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b[..8].try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, GgufError> {
        Ok(self.u64()? as i64)
    }

    fn f32(&mut self) -> Result<f32, GgufError> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f64(&mut self) -> Result<f64, GgufError> {
        let b = self.take(8)?;
        Ok(f64::from_le_bytes(b[..8].try_into().unwrap()))
    }

    /// A GGUF string: `u64` length then UTF-8 bytes. Invalid UTF-8 is malformed
    /// rather than replaced, because a capability key we cannot read is not a
    /// capability key we should report.
    fn string(&mut self) -> Result<String, GgufError> {
        let len = self.u64()?;
        let bytes = self.take(usize::try_from(len).unwrap_or(usize::MAX))?;
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| GgufError::Malformed("string value is not valid UTF-8"))
    }
}

/// Byte width of a scalar value type, or `None` for the types that carry their
/// own length (string, array) and for unknown tags.
fn scalar_size(value_type: u32) -> Option<usize> {
    match value_type {
        T_UINT8 | T_INT8 | T_BOOL => Some(1),
        T_UINT16 | T_INT16 => Some(2),
        T_UINT32 | T_INT32 | T_FLOAT32 => Some(4),
        T_UINT64 | T_INT64 | T_FLOAT64 => Some(8),
        _ => None,
    }
}

const T_UINT8: u32 = 0;
const T_INT8: u32 = 1;
const T_UINT16: u32 = 2;
const T_INT16: u32 = 3;
const T_UINT32: u32 = 4;
const T_INT32: u32 = 5;
const T_FLOAT32: u32 = 6;
const T_BOOL: u32 = 7;
const T_STRING: u32 = 8;
const T_ARRAY: u32 = 9;
const T_UINT64: u32 = 10;
const T_INT64: u32 = 11;
const T_FLOAT64: u32 = 12;

fn read_value(cur: &mut Cursor<'_>, value_type: u32) -> Result<GgufValue, GgufError> {
    Ok(match value_type {
        T_UINT8 => GgufValue::Uint(cur.take(1)?[0] as u64),
        T_INT8 => GgufValue::Int(cur.take(1)?[0] as i8 as i64),
        T_UINT16 => GgufValue::Uint({
            let b = cur.take(2)?;
            u16::from_le_bytes([b[0], b[1]]) as u64
        }),
        T_INT16 => GgufValue::Int({
            let b = cur.take(2)?;
            i16::from_le_bytes([b[0], b[1]]) as i64
        }),
        T_UINT32 => GgufValue::Uint(cur.u32()? as u64),
        T_INT32 => GgufValue::Int(cur.u32()? as i32 as i64),
        T_UINT64 => GgufValue::Uint(cur.u64()?),
        T_INT64 => GgufValue::Int(cur.i64()?),
        T_FLOAT32 => GgufValue::F32(cur.f32()?),
        T_FLOAT64 => GgufValue::F64(cur.f64()?),
        T_BOOL => {
            let byte = cur.take(1)?[0];
            GgufValue::Bool(match byte {
                0 => false,
                1 => true,
                // A flag that is neither is a broken file, not a truthy value.
                _ => return Err(GgufError::Malformed("bool value is not 0 or 1")),
            })
        }
        T_STRING => GgufValue::Str(cur.string()?),
        T_ARRAY => {
            let elem_type = cur.u32()?;
            let len = cur.u64()?;
            if len > MAX_ARRAY_LEN {
                return Err(GgufError::Malformed("array length out of range"));
            }
            let mut items = Vec::with_capacity(len as usize);
            for _ in 0..len {
                items.push(read_value(cur, elem_type)?);
            }
            GgufValue::Array(items)
        }
        _ => return Err(GgufError::Malformed("unknown value type tag")),
    })
}

/// Move the cursor past a value without building it. This is the whole reason a
/// probe is cheap: the tokenizer arrays are skipped by length.
fn skip_value(cur: &mut Cursor<'_>, value_type: u32) -> Result<(), GgufError> {
    if let Some(size) = scalar_size(value_type) {
        let _ = cur.take(size)?;
        return Ok(());
    }
    match value_type {
        T_STRING => {
            cur.string()?;
            Ok(())
        }
        T_ARRAY => {
            let elem_type = cur.u32()?;
            let len = cur.u64()?;
            if len > MAX_ARRAY_LEN {
                return Err(GgufError::Malformed("array length out of range"));
            }
            for _ in 0..len {
                skip_value(cur, elem_type)?;
            }
            Ok(())
        }
        _ => Err(GgufError::Malformed("unknown value type tag")),
    }
}

/// Parse the metadata block from `bytes`, which may be the whole file or only a
/// leading prefix. A prefix that ends mid-block returns
/// [`GgufError::Truncated`].
///
/// Only `wanted_keys` are materialized; everything else is skipped.
pub fn parse_header(bytes: &[u8], wanted_keys: &[&str]) -> Result<GgufMetadata, GgufError> {
    let mut cur = Cursor::new(bytes);
    if cur.u32()? != MAGIC {
        return Err(GgufError::NotGguf);
    }
    let version = cur.u32()?;
    if version != VERSION_V2 && version != VERSION_V3 {
        return Err(GgufError::UnsupportedVersion(version));
    }
    // Tensor count: v3 widened it to u64. Nothing here reads tensor info, but
    // the field has to be stepped over at the right width.
    if version == VERSION_V3 {
        let _ = cur.u64()?;
    } else {
        let _ = cur.u32()?;
    }
    let n_kv = cur.u64()?;
    if n_kv > MAX_KV_PAIRS {
        return Err(GgufError::Malformed("pair count out of range"));
    }

    let mut kv = BTreeMap::new();
    for _ in 0..n_kv {
        let key = cur.string()?;
        let value_type = cur.u32()?;
        if wanted_keys.contains(&key.as_str()) {
            kv.insert(key, read_value(&mut cur, value_type)?);
        } else {
            skip_value(&mut cur, value_type)?;
        }
    }
    Ok(GgufMetadata { kv })
}

/// Read the metadata block of a GGUF on disk, growing the prefix it reads until
/// the block fits.
pub fn read_header(path: &Path) -> Result<GgufMetadata, GgufError> {
    read_header_with(path, PROBE_KEYS)
}

/// [`read_header`] with an explicit key list, for callers (and tests) that want a
/// key the probe does not ask for.
pub fn read_header_with(path: &Path, wanted_keys: &[&str]) -> Result<GgufMetadata, GgufError> {
    let mut file = std::fs::File::open(path).map_err(|e| GgufError::Unreadable(e.to_string()))?;
    let len = file_len(&file);
    let mut size = FIRST_READ_BYTES.min(len);
    loop {
        let mut buf = vec![0u8; size];
        file.rewind().map_err(|e| GgufError::Unreadable(e.to_string()))?;
        let got = file.read(&mut buf).map_err(|e| GgufError::Unreadable(e.to_string()))?;
        buf.truncate(got);
        match parse_header(&buf, wanted_keys) {
            Err(GgufError::Truncated { needed }) => {
                match next_read_size(size, needed, len) {
                    Some(grown) => size = grown,
                    // Nothing left to grow into: the header really does run past
                    // the file, or past what we are willing to hold in memory.
                    None => return Err(GgufError::Truncated { needed }),
                }
            }
            other => return other,
        }
    }
}

/// The prefix to read next after a parse asked for `needed` bytes, or `None` to
/// stop and report the truncation.
///
/// It grows two ways at once: geometrically, so a header truncated by one byte
/// does not turn a probe into dozens of reads, and at least up to what the parse
/// asked for, so a big-but-legitimate block is reached in one step. It never
/// grows past the file, past [`MAX_READ_BYTES`], or into a size it already
/// tried — a corrupt length in the header can ask for `usize::MAX`, and a reader
/// that obeyed it would allocate that.
fn next_read_size(size: usize, needed: usize, file_len: usize) -> Option<usize> {
    if needed > MAX_READ_BYTES {
        return None;
    }
    let grown = size
        .saturating_mul(2)
        .max(needed)
        .min(MAX_READ_BYTES)
        .min(file_len);
    (grown > size).then_some(grown)
}

fn file_len(file: &std::fs::File) -> usize {
    file.metadata().map(|m| m.len() as usize).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The pre-load capability probe
// ---------------------------------------------------------------------------

use crate::models::catalog_stt::ModelCapabilities;

/// What a GGUF header on disk says about itself. Every field is optional: a
/// community-converted file may omit the capability keys entirely, and
/// "the header does not say" must stay distinguishable from "the header says no"
/// — blanking a known catalog value on a failed read is how a probe makes the
/// data worse.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GgufProbe {
    pub architecture: Option<String>,
    pub display_name: Option<String>,
    pub variant: Option<String>,
    pub languages: Option<Vec<String>>,
    pub streaming: Option<bool>,
    pub translate: Option<bool>,
    pub lang_detect: Option<bool>,
}

impl GgufProbe {
    fn from_metadata(meta: &GgufMetadata) -> Self {
        Self {
            architecture: meta.get_str(KEY_ARCH).map(str::to_string),
            display_name: meta.get_str(KEY_NAME).map(str::to_string),
            variant: meta.get_str(KEY_VARIANT).map(str::to_string),
            languages: meta.get_string_array(KEY_LANGUAGES),
            streaming: meta.get_bool(KEY_STREAMING),
            translate: meta.get_bool(KEY_TRANSLATE),
            lang_detect: meta.get_bool(KEY_LANG_DETECT),
        }
    }

    /// Read the header of `path`, or `None` when it is not a readable GGUF.
    ///
    /// Cached by path: `model_status` is polled, and re-reading a megabyte of
    /// header per poll buys nothing. A file replaced on disk under the same path
    /// is a download completing, which invalidates through [`GgufProbe::forget`].
    pub fn read(path: &Path) -> Option<Self> {
        probe_cache::cached(path, || read_header(path).ok().map(|m| Self::from_metadata(&m)))
    }

    /// Drop a cached probe (used when the file behind the path changes).
    pub fn forget(path: &Path) {
        probe_cache::invalidate(path);
    }

    /// Overlay what the bytes say onto what the catalog advertised.
    ///
    /// `timestamps` is carried through untouched: no header key answers it (see
    /// the module docs), so only the engine after load can.
    pub fn apply_to(&self, advertised: &ModelCapabilities) -> ModelCapabilities {
        ModelCapabilities {
            timestamps: advertised.timestamps,
            streaming: self.streaming.unwrap_or(advertised.streaming),
            translate: self.translate.unwrap_or(advertised.translate),
            lang_detect: self.lang_detect.unwrap_or(advertised.lang_detect),
            max_chunk_secs: advertised.max_chunk_secs,
        }
    }
}

/// Probe results are cached per path for the life of the process.
mod probe_cache {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::GgufProbe;

    thread_local! {
        static CACHE: RefCell<HashMap<PathBuf, Option<GgufProbe>>> = RefCell::new(HashMap::new());
    }

    /// A failed probe is cached too: a file that is not GGUF does not become
    /// GGUF by asking twice, and re-reading it on every status poll is pure
    /// waste.
    pub fn cached<F: FnOnce() -> Option<GgufProbe>>(path: &Path, probe: F) -> Option<GgufProbe> {
        let key = path.to_path_buf();
        CACHE.with(|c| {
            if let Some(found) = c.borrow().get(&key) {
                return found.clone();
            }
            let value = probe();
            c.borrow_mut().insert(key, value.clone());
            value
        })
    }

    pub fn invalidate(path: &Path) {
        CACHE.with(|c| {
            c.borrow_mut().remove(path);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::catalog_stt::TimestampGranularity;

    fn put_u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn put_u64(out: &mut Vec<u8>, v: u64) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn put_string(out: &mut Vec<u8>, s: &str) {
        put_u64(out, s.len() as u64);
        out.extend_from_slice(s.as_bytes());
    }

    /// A header with `pairs`, as a converter would write it.
    fn header(pairs: &[(&str, GgufValue)]) -> Vec<u8> {
        header_at_version(VERSION_V3, pairs)
    }

    fn header_at_version(version: u32, pairs: &[(&str, GgufValue)]) -> Vec<u8> {
        let mut out = Vec::new();
        put_u32(&mut out, MAGIC);
        put_u32(&mut out, version);
        // Tensor count: widened to u64 in v3, and a v2 header has no padding or
        // alignment field either — both are stepped over at the right width.
        if version == VERSION_V3 {
            put_u64(&mut out, 0);
        } else {
            put_u32(&mut out, 0);
        }
        put_u64(&mut out, pairs.len() as u64);
        for (key, value) in pairs {
            put_string(&mut out, key);
            match value {
                GgufValue::Bool(b) => {
                    put_u32(&mut out, T_BOOL);
                    out.push(*b as u8);
                }
                GgufValue::Int(i) => {
                    put_u32(&mut out, T_INT32);
                    put_u32(&mut out, *i as u32);
                }
                GgufValue::Str(s) => {
                    put_u32(&mut out, T_STRING);
                    put_string(&mut out, s);
                }
                GgufValue::Array(items) => {
                    put_u32(&mut out, T_ARRAY);
                    put_u32(&mut out, T_STRING);
                    put_u64(&mut out, items.len() as u64);
                    for item in items {
                        if let GgufValue::Str(s) = item {
                            put_string(&mut out, s);
                        }
                    }
                }
                other => unreachable!("test header does not build {:?}", other),
            }
        }
        out
    }

    fn sample_pairs() -> Vec<(&'static str, GgufValue)> {
        vec![
            (KEY_ARCH, GgufValue::Str("parakeet".into())),
            (KEY_NAME, GgufValue::Str("Parakeet TDT 0.6B v3".into())),
            (
                KEY_LANGUAGES,
                GgufValue::Array(vec![GgufValue::Str("en".into()), GgufValue::Str("de".into())]),
            ),
            (KEY_STREAMING, GgufValue::Bool(false)),
            (KEY_TRANSLATE, GgufValue::Bool(true)),
        ]
    }

    #[test]
    fn reads_the_keys_a_model_card_claims() {
        let bytes = header(
            &sample_pairs()
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<Vec<_>>(),
        );
        let meta = parse_header(&bytes, PROBE_KEYS).unwrap();
        let probe = GgufProbe::from_metadata(&meta);
        assert_eq!(probe.architecture.as_deref(), Some("parakeet"));
        assert_eq!(probe.display_name.as_deref(), Some("Parakeet TDT 0.6B v3"));
        assert_eq!(
            probe.languages.as_deref(),
            Some(&["en".to_string(), "de".to_string()][..])
        );
        assert_eq!(probe.streaming, Some(false));
        assert_eq!(probe.translate, Some(true));
        // Absent from the header, so unknown — not `false`.
        assert_eq!(probe.lang_detect, None);
    }

    #[test]
    fn anything_that_is_not_gguf_is_refused() {
        // A GGML .bin starts with its own magic and would mis-parse as garbage.
        assert_eq!(
            parse_header(&[1, 2, 3, 4, 5, 6, 7, 8], PROBE_KEYS),
            Err(GgufError::NotGguf)
        );
        assert_eq!(
            parse_header(&header_at_version(99, &[]), PROBE_KEYS),
            Err(GgufError::UnsupportedVersion(99))
        );
        // v2 is the other version converters write, and it parses: the tensor
        // count is read as u32 there, so getting that width wrong truncates.
        let v2 = header_at_version(
            VERSION_V2,
            &[(KEY_ARCH, GgufValue::Str("whisper".into()))],
        );
        assert_eq!(
            parse_header(&v2, PROBE_KEYS)
                .unwrap()
                .get_str(KEY_ARCH),
            Some("whisper")
        );
    }

    #[test]
    fn a_short_prefix_asks_for_exactly_what_it_needs() {
        let bytes = header(&sample_pairs().iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>());
        let cut = bytes.len() - 4;
        match parse_header(&bytes[..cut], PROBE_KEYS) {
            Err(GgufError::Truncated { needed }) => assert!(needed > cut && needed <= bytes.len()),
            other => panic!("expected a truncated read, got {other:?}"),
        }
        // The full buffer parses; the error was length, not content.
        assert!(parse_header(&bytes, PROBE_KEYS).is_ok());
    }

    /// Vocabulary arrays are the reason a probe must skip rather than read: a
    /// 250k-entry string array is megabytes of allocations for no answer.
    #[test]
    fn unrequested_values_are_stepped_over_not_materialized() {
        let mut bytes = Vec::new();
        put_u32(&mut bytes, MAGIC);
        put_u32(&mut bytes, VERSION_V3);
        put_u64(&mut bytes, 0);
        put_u64(&mut bytes, 2);
        put_string(&mut bytes, "tokenizer.ggml.tokens");
        put_u32(&mut bytes, T_ARRAY);
        put_u32(&mut bytes, T_STRING);
        put_u64(&mut bytes, 50_000);
        for i in 0..50_000 {
            put_string(&mut bytes, &format!("tok{i}"));
        }
        put_string(&mut bytes, KEY_ARCH);
        put_u32(&mut bytes, T_STRING);
        put_string(&mut bytes, "whisper");

        let meta = parse_header(&bytes, PROBE_KEYS).unwrap();
        assert_eq!(meta.get_str(KEY_ARCH), Some("whisper"));
        assert!(meta.get("tokenizer.ggml.tokens").is_none());
    }

    #[test]
    fn a_mixed_array_is_unknown_rather_than_empty() {
        let mut bytes = Vec::new();
        put_u32(&mut bytes, MAGIC);
        put_u32(&mut bytes, VERSION_V3);
        put_u64(&mut bytes, 0);
        put_u64(&mut bytes, 1);
        put_string(&mut bytes, KEY_LANGUAGES);
        put_u32(&mut bytes, T_ARRAY);
        put_u32(&mut bytes, T_INT32);
        put_u64(&mut bytes, 3);
        for i in 0..3 {
            put_u32(&mut bytes, i);
        }
        let meta = parse_header(&bytes, PROBE_KEYS).unwrap();
        assert_eq!(meta.get_string_array(KEY_LANGUAGES), None);
    }

    /// A header that claims a value longer than we will hold must be reported as
    /// truncated, not obeyed: growing *towards* an absurd `needed` is how a
    /// corrupt file would turn a status poll into a multi-gigabyte allocation.
    #[test]
    fn an_absurd_claimed_length_stops_the_growth() {
        // Still short of the ceiling: double the step, but at least `needed`.
        assert_eq!(next_read_size(1 << 20, 3_000_000, 1 << 30), Some(3_000_000));
        // Past the ceiling: stop.
        assert_eq!(next_read_size(1 << 20, MAX_READ_BYTES + 1, 1 << 30), None);
        // Past the end of the file: stop, rather than re-reading the same prefix
        // and looping forever on the same error.
        assert_eq!(next_read_size(500, 10_000, 500), None);
        // An empty file never grows.
        assert_eq!(next_read_size(0, 16, 0), None);
    }

    /// The reconciliation rule: the header refines, it never erases.
    #[test]
    fn probe_overlays_without_blanking_advertised_values() {
        let advertised = ModelCapabilities {
            timestamps: Some(TimestampGranularity::Token),
            streaming: false,
            translate: true,
            lang_detect: true,
            max_chunk_secs: Some(240.0),
        };
        let read = GgufProbe {
            translate: Some(false),
            ..Default::default()
        };
        let merged = read.apply_to(&advertised);
        assert!(!merged.translate, "the header outvotes the card");
        assert!(merged.lang_detect, "a key the header omits keeps the card's answer");
        assert_eq!(merged.timestamps, advertised.timestamps, "granularity is not the header's to give");
        assert_eq!(merged.max_chunk_secs, advertised.max_chunk_secs);

        let nothing = GgufProbe::default();
        assert_eq!(nothing.apply_to(&advertised), advertised);
    }
}
