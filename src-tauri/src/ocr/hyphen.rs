//! Line-break hyphen resolution for the TSV rebuild.
//!
//! The image alone cannot say whether `Sprach-` / `verarbeitung` was one word
//! broken at the margin or a compound written with a hyphen (`E-` / `Mail`).
//! The answer comes from a Hunspell dictionary of the document's language: the
//! joined form is folded into one word only when the dictionary knows it, so
//! `Sprachverarbeitung` joins while `EMail` (from `E-` + `Mail`) does not.
//!
//! Dictionaries are downloaded per language rather than bundled (they are a few
//! hundred KB to 19 MB each and carry their own licences). `spellbook` reads the
//! Hunspell `.aff`/`.dic` pair directly, compounding rules included, so no system
//! Hunspell is required.
//!
//! A page is recognised with a language *set* (`deu+eng`), so every installed
//! dictionary of that set is consulted: a word found in any of them joins. That
//! is what makes a German text with English terms (`Workflow-` + `Editor`) work
//! without the user having to pick one dictionary per page.
//!
//! Without a dictionary the resolver falls back to a shape heuristic that is
//! right far more often than not: a typesetter only breaks a word whose
//! continuation is lowercase, so a capitalised continuation is kept as a real
//! hyphen.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use spellbook::Dictionary;

use crate::app_paths;

use super::tsv::Candidate;

/// Directory under the app data root that holds one folder per language.
const DICT_SUBDIR: &str = "ocr_dicts";
const AFF_FILE: &str = "index.aff";
const DIC_FILE: &str = "index.dic";

/// A downloadable Hunspell dictionary.
///
/// `version` pins the upstream npm package so a given app build always judges
/// words the same way; bumping it is a deliberate change, not a side effect.
pub struct DictionarySource {
    /// Hunspell/npm language code — also the on-disk folder name.
    pub code: &'static str,
    /// Tesseract traineddata codes that map onto this dictionary.
    pub tesseract: &'static [&'static str],
    /// Label for the UI.
    pub label: &'static str,
    /// Pinned `dictionary-<code>` npm version (the wooorm/dictionaries set).
    pub version: &'static str,
    /// SPDX expression of the dictionary data's licence.
    pub license: &'static str,
    /// Bytes of the pinned `.aff` + `.dic` (measured once, for the pinned
    /// version). The UI shows it so a 19 MB Greek download is not a surprise.
    pub approx_bytes: u64,
}

/// Languages we can resolve hyphens for. Compound-heavy languages (German,
/// Dutch, the Nordic ones) are the ones that need this most, but the Latin and
/// Slavic sets earn their place too: a scanned book hyphenates in any language.
///
/// `approx_bytes` is what the pinned package actually weighs on the CDN, so the
/// manager can show sizes before anything is downloaded.
pub const CATALOG: &[DictionarySource] = &[
    DictionarySource {
        code: "de",
        tesseract: &["deu", "ger"],
        label: "Deutsch",
        version: "3.0.0",
        license: "GPL-2.0 OR GPL-3.0",
        approx_bytes: 1_137_393,
    },
    DictionarySource {
        code: "en",
        tesseract: &["eng"],
        label: "English",
        version: "4.0.0",
        license: "MIT AND BSD",
        approx_bytes: 554_848,
    },
    DictionarySource {
        code: "fr",
        tesseract: &["fra", "fre"],
        label: "Français",
        version: "3.0.0",
        license: "MPL-2.0",
        approx_bytes: 1_429_003,
    },
    DictionarySource {
        code: "es",
        tesseract: &["spa"],
        label: "Español",
        version: "4.0.0",
        license: "GPL-3.0 OR LGPL-3.0 OR MPL-1.1",
        approx_bytes: 873_337,
    },
    DictionarySource {
        code: "it",
        tesseract: &["ita"],
        label: "Italiano",
        version: "2.0.0",
        license: "GPL-3.0",
        approx_bytes: 1_365_774,
    },
    DictionarySource {
        code: "pt",
        tesseract: &["por"],
        label: "Português",
        version: "4.0.0",
        license: "LGPL-3.0 OR MPL-2.0",
        approx_bytes: 5_457_481,
    },
    DictionarySource {
        code: "nl",
        tesseract: &["nld", "dut"],
        label: "Nederlands",
        version: "2.0.0",
        license: "BSD-3-Clause OR CC-BY-3.0",
        approx_bytes: 2_535_898,
    },
    DictionarySource {
        code: "sv",
        tesseract: &["swe"],
        label: "Svenska",
        version: "4.0.0",
        license: "LGPL-3.0",
        approx_bytes: 2_362_780,
    },
    DictionarySource {
        code: "da",
        tesseract: &["dan"],
        label: "Dansk",
        version: "6.0.0",
        license: "GPL-2.0 OR LGPL-2.1 OR MPL-1.1",
        approx_bytes: 3_801_084,
    },
    DictionarySource {
        code: "pl",
        tesseract: &["pol"],
        label: "Polski",
        version: "2.0.0",
        license: "GPL-3.0 OR LGPL-3.0 OR MPL-2.0",
        approx_bytes: 4_952_403,
    },
    DictionarySource {
        code: "cs",
        tesseract: &["ces", "cze"],
        label: "Čeština",
        version: "4.0.0",
        license: "GPL-2.0",
        approx_bytes: 3_760_848,
    },
    DictionarySource {
        code: "ru",
        tesseract: &["rus"],
        label: "Русский",
        version: "3.0.0",
        license: "BSD-3-Clause",
        approx_bytes: 3_544_427,
    },
    DictionarySource {
        code: "uk",
        tesseract: &["ukr"],
        label: "Українська",
        version: "4.0.0",
        license: "GPL-3.0",
        approx_bytes: 8_652_483,
    },
    DictionarySource {
        code: "el",
        tesseract: &["ell", "gre"],
        label: "Ελληνικά",
        version: "4.0.0",
        license: "GPL-2.0 OR LGPL-2.1 OR MPL-1.1",
        approx_bytes: 19_442_025,
    },
    DictionarySource {
        code: "tr",
        tesseract: &["tur"],
        label: "Türkçe",
        version: "2.0.0",
        license: "MIT",
        approx_bytes: 9_358_006,
    },
];

/// How the caller wants ambiguous hyphens treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Decide from a dictionary, falling back to the shape heuristic.
    Auto,
    /// Fold every line-break hyphen into one word.
    JoinAll,
    /// Leave every line-break hyphen in place.
    KeepAll,
}

impl Policy {
    /// Parse the wire value used by the Tauri command and the MCP tool.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|value| value.trim().to_ascii_lowercase()).as_deref() {
            Some("join") | Some("join_all") => Policy::JoinAll,
            Some("keep") | Some("keep_all") => Policy::KeepAll,
            _ => Policy::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Auto => "auto",
            Policy::JoinAll => "join",
            Policy::KeepAll => "keep",
        }
    }
}

// ---------------------------------------------------------------------------
// Dictionary store
// ---------------------------------------------------------------------------

/// Parsed dictionaries, cached per language.
///
/// Parsing the German dictionary takes a few hundred milliseconds, so it must
/// happen once per run and off the async runtime. `None` caches "not
/// installed" as well, so a machine without dictionaries never re-stats the
/// disk on every recognition.
#[derive(Clone, Default)]
pub struct HyphenState {
    loaded: Arc<Mutex<HashMap<String, Option<Arc<Dictionary>>>>>,
}

impl HyphenState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The dictionary for `code`, loading it on first use. Blocking.
    pub fn dictionary(&self, code: &str) -> Option<Arc<Dictionary>> {
        let mut cache = match self.loaded.lock() {
            Ok(cache) => cache,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(entry) = cache.get(code) {
            return entry.clone();
        }

        let entry = load_from_disk(code).map(Arc::new);
        cache.insert(code.to_string(), entry.clone());
        entry
    }

    /// Drop one cached dictionary (after a download or a delete).
    pub fn forget(&self, code: &str) {
        if let Ok(mut cache) = self.loaded.lock() {
            cache.remove(code);
        }
    }
}

/// Read and parse `<data>/ocr_dicts/<code>/index.{aff,dic}`.
fn load_from_disk(code: &str) -> Option<Dictionary> {
    let dir = dict_dir(code);
    let aff = std::fs::read_to_string(dir.join(AFF_FILE)).ok()?;
    let dic = std::fs::read_to_string(dir.join(DIC_FILE)).ok()?;

    match Dictionary::new(&aff, &dic) {
        Ok(dictionary) => {
            log::info!(
                "[OCR] loaded {} dictionary ({} words, aff {} bytes, dic {} bytes)",
                code,
                dic.lines().next().unwrap_or("?").trim(),
                aff.len(),
                dic.len()
            );
            Some(dictionary)
        }
        Err(error) => {
            log::warn!("[OCR] dictionary {code} failed to parse: {error:?}");
            None
        }
    }
}

pub fn dict_dir(code: &str) -> PathBuf {
    app_paths::data_subdir(DICT_SUBDIR).join(code)
}

/// Catalog entry for a bare Hunspell code.
pub fn source(code: &str) -> Option<&'static DictionarySource> {
    CATALOG.iter().find(|entry| entry.code == code)
}

/// Everything a page's Tesseract language selection implies for hyphen
/// decisions: the catalog entries it maps to (each with its parsed dictionary
/// when installed), plus the selected codes no dictionary covers.
pub struct Selection {
    /// In selection order, so `deu+eng` reads German first.
    pub entries: Vec<(&'static DictionarySource, Option<Arc<Dictionary>>)>,
    /// Tesseract codes with no dictionary in the catalog (CJK, Arabic, Hebrew…).
    /// Their scripts do not hyphenate, so the shape rule is all that is needed;
    /// listing them lets the UI say so instead of staying silent.
    pub uncovered: Vec<String>,
}

impl Selection {
    /// The dictionaries that can actually decide a hyphen on this page.
    pub fn loaded(&self) -> Vec<&Dictionary> {
        self.entries
            .iter()
            .filter_map(|(_, dictionary)| dictionary.as_deref())
            .collect()
    }
}

/// Resolve a Tesseract language spec (`deu+eng`) to the dictionaries it maps to.
///
/// Every matching entry is kept — including the ones that are not installed, so
/// the UI can offer them — and the codes nothing covers are reported separately.
pub fn selection(lang: &str, state: &HyphenState) -> Selection {
    let (sources, uncovered) = mapped_sources(lang);

    Selection {
        entries: sources
            .into_iter()
            .map(|source| (source, state.dictionary(source.code)))
            .collect(),
        uncovered,
    }
}

/// The catalog side of [`selection`], without touching the disk.
pub fn mapped_sources(lang: &str) -> (Vec<&'static DictionarySource>, Vec<String>) {
    let mut sources: Vec<&'static DictionarySource> = Vec::new();
    let mut uncovered = Vec::new();

    for code in lang
        .split('+')
        .map(str::trim)
        .filter(|code| !code.is_empty())
    {
        match CATALOG
            .iter()
            .find(|entry| entry.tesseract.contains(&code))
        {
            // `deu+ger` (or the same code twice) is still one dictionary.
            Some(source) if sources.iter().any(|seen| seen.code == source.code) => {}
            Some(source) => sources.push(source),
            None => uncovered.push(code.to_string()),
        }
    }

    (sources, uncovered)
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Decide every candidate: any installed dictionary first, shape heuristic
/// second.
///
/// Every dictionary of the page's language set is consulted, so a German text
/// quoting an English term (`Workflow-` + `Editor`) joins on the English entry
/// even though German does not know it.
pub fn resolve(candidates: &mut [Candidate], dictionaries: &[&Dictionary], policy: Policy) {
    for candidate in candidates.iter_mut() {
        let (join, reason) = match policy {
            Policy::JoinAll => (true, "policy"),
            Policy::KeepAll => (false, "policy"),
            Policy::Auto => decide(candidate, dictionaries),
        };

        candidate.join = join;
        candidate.reason = reason;
    }
}

fn decide(candidate: &Candidate, dictionaries: &[&Dictionary]) -> (bool, &'static str) {
    // A hyphen glued to a digit (`1-` + `2`) is never a broken word.
    if candidate
        .left
        .chars()
        .last()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        return (false, "digit");
    }

    if !dictionaries.is_empty() {
        return if dictionaries
            .iter()
            .any(|dictionary| knows(dictionary, &candidate.joined))
        {
            (true, "dictionary")
        } else {
            (false, "unknown-word")
        };
    }

    // No dictionary at all: a broken word continues in lowercase (`Sprach-` +
    // `verarbeitung`), a real compound keeps the other part's case (`E-` +
    // `Mail`).
    if starts_lowercase(&candidate.right) {
        (true, "lowercase")
    } else {
        (false, "uppercase")
    }
}

fn starts_lowercase(word: &str) -> bool {
    word.chars()
        .find(|ch| ch.is_alphabetic())
        .is_some_and(|ch| ch.is_lowercase())
}

/// Does the dictionary know this word? Punctuation the OCR attached to the
/// fragments (`Verarbeitung,`, `„Verarbeitung`) is not part of the word, and
/// lowercase OCR output must still match a capitalised dictionary entry.
fn knows(dictionary: &Dictionary, word: &str) -> bool {
    let trimmed = word.trim_matches(|ch: char| !ch.is_alphanumeric());

    !trimmed.is_empty() && dictionary.checker().check_lower_as_title(true).check(trimmed)
}

/// Byte index of the hyphen of `candidate` at or after `from`.
///
/// An earlier occurrence of `left` that is not followed by the hyphen and a
/// space — the same fragment inside a longer word, say — must not end the
/// search: the candidate itself is still further on, and giving up there would
/// leave it silently unrepairable in the UI.
fn locate(text: &str, candidate: &Candidate, from: usize) -> Option<usize> {
    if candidate.left.is_empty() {
        return None;
    }

    let mut search = from;

    while let Some(found) = text[search..].find(candidate.left.as_str()) {
        let after_left = search + found + candidate.left.len();

        if text[after_left..].starts_with(candidate.hyphen) {
            let after_hyphen = after_left + candidate.hyphen.len_utf8();
            if text[after_hyphen..].starts_with(' ') {
                return Some(after_left);
            }
        }

        search = after_left;
    }

    None
}

/// Fold the decided candidates into the rebuilt text.
///
/// Candidates arrive in reading order and the text holds each one as
/// `left` + hyphen + one space + `right`, so a single forward scan finds them
/// all without storing offsets — offsets would not survive the user editing the
/// text.
pub fn apply(text: &str, candidates: &[Candidate]) -> String {
    let mut removals: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0;

    for candidate in candidates {
        // A candidate the text no longer matches is left alone rather than
        // corrupting the sentence it now belongs to.
        let Some(after_left) = locate(text, candidate, cursor) else {
            continue;
        };

        // Only the hyphen and the space it replaced are dropped: the word after
        // the break stays exactly where it was.
        if candidate.join {
            removals.push((after_left, candidate.hyphen.len_utf8() + 1));
        }

        // Resume after the next word so a repeated fragment cannot match the
        // same region twice.
        cursor = after_left + candidate.hyphen.len_utf8() + 1;
        if text[cursor..].starts_with(candidate.right.as_str()) {
            cursor += candidate.right.len();
        }
    }

    if removals.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, len) in removals {
        out.push_str(&text[last..at]);
        last = at + len;
    }
    out.push_str(&text[last..]);
    out
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// What the UI and the MCP tools see for one language.
#[derive(Debug, Clone, Serialize)]
pub struct DictionaryStatus {
    pub code: &'static str,
    pub label: &'static str,
    pub version: &'static str,
    pub license: &'static str,
    pub tesseract: &'static [&'static str],
    pub installed: bool,
    /// Bytes on disk — 0 when the dictionary is not installed.
    pub size_bytes: u64,
    /// What the download costs, so the UI can warn before a 19 MB one.
    pub approx_bytes: u64,
}

impl DictionaryStatus {
    fn of(source: &'static DictionarySource) -> Self {
        let dir = dict_dir(source.code);
        let aff = std::fs::metadata(dir.join(AFF_FILE)).map(|m| m.len()).unwrap_or(0);
        let dic = std::fs::metadata(dir.join(DIC_FILE)).map(|m| m.len()).unwrap_or(0);

        Self {
            code: source.code,
            label: source.label,
            version: source.version,
            license: source.license,
            tesseract: source.tesseract,
            installed: aff > 0 && dic > 0,
            size_bytes: aff + dic,
            approx_bytes: source.approx_bytes,
        }
    }
}

pub fn status_all() -> Vec<DictionaryStatus> {
    CATALOG.iter().map(DictionaryStatus::of).collect()
}

fn unknown_language(code: &str) -> String {
    let known: Vec<&str> = CATALOG.iter().map(|entry| entry.code).collect();
    format!(
        "No line-break dictionary for '{code}'. Available: {}. Use ocr_hyphen_dictionary_status for the current state.",
        known.join(", ")
    )
}

/// Fetch and install one dictionary. Returns the fresh status.
pub async fn download(code: &str) -> Result<DictionaryStatus, String> {
    let source = source(code).ok_or_else(|| unknown_language(code))?;

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Cannot create HTTP client: {e}"))?;

    let aff = fetch(client.clone(), source, AFF_FILE).await?;
    let dic = fetch(client, source, DIC_FILE).await?;

    validate(code, &aff, &dic)?;

    let dir = dict_dir(code);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;

    // The `.dic` is written last: a half-finished download then still reads as
    // "not installed" instead of as a dictionary with no words.
    write_atomic(&dir.join(AFF_FILE), &aff)?;
    write_atomic(&dir.join(DIC_FILE), &dic)?;

    log::info!(
        "[OCR] installed {code} dictionary v{} ({} + {} bytes)",
        source.version,
        aff.len(),
        dic.len()
    );

    Ok(DictionaryStatus::of(source))
}

/// Remove an installed dictionary.
pub fn delete(code: &str) -> Result<(), String> {
    let source = source(code).ok_or_else(|| unknown_language(code))?;
    let dir = dict_dir(source.code);

    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove {}: {error}", dir.display())),
    }
}

/// The whole cause chain, not just reqwest's outer "error sending request":
/// without it a failed download says nothing about DNS, TLS or a reset.
fn describe(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// Download one file, trying the pinned npm CDN first and the GitHub mirror
/// second (a CDN being down should not cost the user the feature).
async fn fetch(
    client: reqwest::Client,
    source: &'static DictionarySource,
    file: &str,
) -> Result<String, String> {
    let urls = [
        format!("https://unpkg.com/dictionary-{}@{}/{file}", source.code, source.version),
        format!(
            "https://cdn.jsdelivr.net/npm/dictionary-{}@{}/{file}",
            source.code, source.version
        ),
        format!(
            "https://cdn.jsdelivr.net/gh/wooorm/dictionaries@main/dictionaries/{}/{file}",
            source.code
        ),
    ];

    let mut failures = Vec::new();

    for url in &urls {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => match response.text().await {
                Ok(body) => return Ok(body),
                Err(error) => failures.push(format!("{url}: {}", describe(&error))),
            },
            Ok(response) => failures.push(format!("{url}: HTTP {}", response.status())),
            Err(error) => {
                log::warn!("[OCR] dictionary mirror failed: {url}: {}", describe(&error));
                failures.push(format!("{url}: {}", describe(&error)));
            }
        }
    }

    Err(format!(
        "Could not download the {} dictionary ({file}). Tried {} mirrors: {}",
        source.label,
        failures.len(),
        failures.join("; ")
    ))
}

/// Cheap sanity check on a fresh download: a truncated file parses as nothing
/// useful, and installing one would silently disable hyphen repair.
fn validate(code: &str, aff: &str, dic: &str) -> Result<(), String> {
    if !aff.contains("SET ") || aff.len() < 64 {
        return Err(format!(
            "The {code} affix file looks truncated ({} bytes). Nothing was installed.",
            aff.len()
        ));
    }

    let declared = dic.lines().next().unwrap_or("").trim();
    if dic.lines().count() < 100 || declared.parse::<usize>().is_err() {
        return Err(format!(
            "The {code} word list looks truncated ({} bytes, first line {declared:?}). Nothing was installed.",
            dic.len()
        ));
    }

    Ok(())
}

fn write_atomic(path: &std::path::Path, contents: &str) -> Result<(), String> {
    let temp = path.with_file_name(format!(
        "{}.part",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&temp, contents).map_err(|e| format!("Cannot write {}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("Cannot write {}: {e}", path.display())
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature Hunspell pair: `spellbook` needs real files' worth of
    /// syntax, but three words are enough to exercise the resolver.
    const AFF: &str = "SET UTF-8\nTRY esianrtolcdugmphbyfvkwz\n";
    const DIC: &str = "4\nSprachverarbeitung\nE-Mail\nHaus\nboot\n";

    fn dictionary() -> Dictionary {
        Dictionary::new(AFF, DIC).unwrap()
    }

    fn codes(sources: &[&'static DictionarySource]) -> Vec<&'static str> {
        sources.iter().map(|source| source.code).collect()
    }

    fn candidate(left: &str, hyphen: char, right: &str) -> Candidate {
        Candidate {
            left: left.to_string(),
            hyphen,
            right: right.to_string(),
            joined: format!("{left}{right}"),
            join: false,
            reason: "pending",
        }
    }

    #[test]
    fn joins_a_word_the_dictionary_knows() {
        let mut candidates = vec![candidate("Sprach", '-', "verarbeitung")];
        let dictionary = dictionary();
        resolve(&mut candidates, &[&dictionary], Policy::Auto);

        assert!(candidates[0].join);
        assert_eq!(candidates[0].reason, "dictionary");
        assert_eq!(
            apply("Sprach- verarbeitung ist schwer", &candidates),
            "Sprachverarbeitung ist schwer"
        );
    }

    #[test]
    fn keeps_a_hyphen_whose_joined_form_is_unknown() {
        let mut candidates = vec![candidate("E", '-', "Mail")];
        let dictionary = dictionary();
        resolve(&mut candidates, &[&dictionary], Policy::Auto);

        assert!(!candidates[0].join);
        assert_eq!(candidates[0].reason, "unknown-word");
        assert_eq!(apply("eine E- Mail", &candidates), "eine E- Mail");
    }

    #[test]
    fn joins_a_lowercase_continuation_without_a_dictionary() {
        let mut candidates = vec![candidate("Sprach", '-', "verarbeitung")];
        resolve(&mut candidates, &[], Policy::Auto);

        assert!(candidates[0].join);
        assert_eq!(candidates[0].reason, "lowercase");
    }

    #[test]
    fn keeps_a_capitalised_continuation_without_a_dictionary() {
        let mut candidates = vec![candidate("E", '-', "Mail")];
        resolve(&mut candidates, &[], Policy::Auto);

        assert!(!candidates[0].join);
        assert_eq!(candidates[0].reason, "uppercase");
    }

    #[test]
    fn never_joins_numbers() {
        let mut candidates = vec![candidate("1", '-', "2")];
        resolve(&mut candidates, &[], Policy::Auto);

        assert!(!candidates[0].join);
        assert_eq!(candidates[0].reason, "digit");
    }

    #[test]
    fn policies_override_the_evidence() {
        let dictionary = dictionary();
        let mut candidates = vec![candidate("E", '-', "Mail")];
        resolve(&mut candidates, &[&dictionary], Policy::JoinAll);
        assert!(candidates[0].join);
        assert_eq!(candidates[0].reason, "policy");

        let mut candidates = vec![candidate("Sprach", '-', "verarbeitung")];
        resolve(&mut candidates, &[&dictionary], Policy::KeepAll);
        assert!(!candidates[0].join);
    }

    #[test]
    fn applies_decisions_in_reading_order() {
        // Two candidates, the first kept and the second joined, and the second
        // one's fragment also appears inside the first line's text.
        let mut candidates = vec![
            candidate("Sprach", '-', "verarbeitung"),
            candidate("Sprach", '-', "verarbeitung"),
        ];
        candidates[1].join = true;

        assert_eq!(
            apply("Sprach- verarbeitung und Sprach- verarbeitung", &candidates),
            "Sprach- verarbeitung und Sprachverarbeitung"
        );
    }

    #[test]
    fn restores_the_printed_hyphen_on_keep() {
        let mut candidates = vec![candidate("Sprach", '\u{2010}', "verarbeitung")];
        resolve(&mut candidates, &[], Policy::Auto);
        assert!(candidates[0].join);

        candidates[0].join = false;
        assert_eq!(apply("Sprach\u{2010} verarbeitung", &candidates), "Sprach\u{2010} verarbeitung");
    }

    #[test]
    fn ignores_text_that_no_longer_matches() {
        // The user edited the result: the anchor is gone, so nothing is removed.
        let mut candidates = vec![candidate("Sprach", '-', "verarbeitung")];
        candidates[0].join = true;

        assert_eq!(apply("Sprachverarbeitung", &candidates), "Sprachverarbeitung");
    }

    #[test]
    fn skips_an_earlier_occurrence_of_the_fragment() {
        // "Sprach" also occurs inside "Sprachlichkeit", which is not a break:
        // giving up at that first hit left the real candidate unrepaired, so the
        // UI's Join looked like it did nothing.
        let mut candidates = vec![candidate("Sprach", '-', "verarbeitung")];
        candidates[0].join = true;

        assert_eq!(
            apply("Sprachlichkeit und Sprach- verarbeitung", &candidates),
            "Sprachlichkeit und Sprachverarbeitung"
        );
    }

    #[test]
    fn matches_a_capitalised_word_from_lowercase_ocr() {
        // OCR lowercases a sentence start; the dictionary entry is capitalised.
        let mut candidates = vec![candidate("sprach", '-', "verarbeitung")];
        let dictionary = dictionary();
        resolve(&mut candidates, &[&dictionary], Policy::Auto);

        // "sprachverarbeitung" is not a dictionary entry, but "Sprachverarbeitung"
        // is, and `check_lower_as_title` bridges the two.
        assert!(candidates[0].join);
    }

    #[test]
    fn ignores_punctuation_attached_to_the_fragments() {
        assert!(knows(&dictionary(), "Sprachverarbeitung,"));
        assert!(knows(&dictionary(), "„E-Mail"));
        assert!(!knows(&dictionary(), "EMail"));
    }

    #[test]
    fn any_dictionary_of_the_set_can_decide() {
        // A German page quoting an English compound: German does not know the
        // word, English does, so a `deu+eng` page still joins it.
        let german = dictionary();
        let english = Dictionary::new(AFF, "3\nWorkflowEditor\nWorkflow\nEditor\n").unwrap();

        let mut candidates = vec![candidate("Workflow", '-', "Editor")];
        resolve(&mut candidates, &[&german, &english], Policy::Auto);
        assert!(candidates[0].join);
        assert_eq!(candidates[0].reason, "dictionary");

        // A word no dictionary of the set knows stays as printed.
        let mut candidates = vec![candidate("E", '-', "Mail")];
        resolve(&mut candidates, &[&german, &english], Policy::Auto);
        assert!(!candidates[0].join);
        assert_eq!(candidates[0].reason, "unknown-word");
    }

    #[test]
    fn catalog_codes_are_unique_and_map_to_tesseract() {
        for source in CATALOG {
            assert_eq!(
                CATALOG.iter().filter(|entry| entry.code == source.code).count(),
                1,
                "duplicate catalog code {}",
                source.code
            );
            assert!(
                !source.tesseract.is_empty(),
                "{} has no Tesseract codes",
                source.code
            );
        }

        let state = HyphenState::new();
        let resolved = selection("deu+eng", &state);
        assert_eq!(codes(&mapped_sources("deu+eng").0), vec!["de", "en"]);
        assert!(resolved.uncovered.is_empty());

        // `ger` is the legacy Tesseract code for the same dictionary, and a
        // repeated code must not load it twice.
        assert_eq!(codes(&mapped_sources("ger+deu").0), vec!["de"]);

        // A script with no dictionary of its own is reported, not guessed at.
        let (sources, uncovered) = mapped_sources("jpn+eng");
        assert_eq!(codes(&sources), vec!["en"]);
        assert_eq!(uncovered, vec!["jpn"]);
    }

    /// End-to-end check against the real CDN and real dictionaries: downloads
    /// two languages, then checks that the selection — not the app — decides
    /// which one judges a word.
    ///
    /// Ignored by default — it needs the network and it installs the dictionaries
    /// into the app data directory, which is exactly what a user does anyway.
    /// Run with `cargo test --features desktop --lib ocr -- --ignored`.
    #[test]
    #[ignore = "downloads the German and French dictionaries"]
    fn downloads_dictionaries_and_judges_several_languages() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for code in ["de", "fr"] {
            assert!(runtime.block_on(download(code)).unwrap().installed);
        }

        let state = HyphenState::new();

        // German alone cannot judge a French word — and must not pretend it can.
        let german = selection("deu", &state);
        assert_eq!(german.entries[0].0.code, "de");
        let german_loaded = german.loaded();
        assert_eq!(german_loaded.len(), 1, "the German dictionary should load");

        let mut candidates = vec![
            candidate("Sprach", '-', "verarbeitung"),
            candidate("E", '-', "Mail"),
            candidate("Datenbank", '-', "zugriff"),
            candidate("ordina", '-', "teur"),
        ];
        resolve(&mut candidates, &german_loaded, Policy::Auto);
        assert!(candidates[0].join, "Sprachverarbeitung is a compound noun");
        assert!(!candidates[1].join, "EMail is not a word, E-Mail is");
        assert!(candidates[2].join, "Datenbankzugriff is a compound noun");
        assert!(!candidates[3].join, "German does not know a French word");

        // Add French to the selection and the French word decides — accented
        // fragments included, which is what a scanned French page looks like.
        let both = selection("deu+fra", &state);
        assert_eq!(both.entries.len(), 2);
        assert_eq!(both.loaded().len(), 2, "both dictionaries should load");

        let mut candidates = vec![
            candidate("ordina", '-', "teur"),
            candidate("déve", '-', "loppement"),
        ];
        resolve(&mut candidates, &both.loaded(), Policy::Auto);
        assert!(candidates[0].join, "ordinateur is a French word");
        assert!(candidates[1].join, "développement is a French word");
    }
}
