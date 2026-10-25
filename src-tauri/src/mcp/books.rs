//! Book and read-aloud datasets: chapters/sentences/words, plus read-aloud
//! scoring and attempts.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use serde::Deserialize;
use tauri::Manager;

use crate::datasets;
use crate::settings::SettingsState;

use super::{DatasetMcpServer, JsonValue};

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookTitleParam {
    title: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookUuidParam {
    book_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookRenameParam {
    uuid: String,
    title: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookChapterParam {
    book_uuid: String,
    chapter: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteChapterParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookSyncChaptersParam {
    book_uuid: String,
    /// Full desired chapter forest as JSON nodes `{uuid, parent_uuid, order_num, title}`.
    nodes: Vec<JsonValue>,
    dry_run: bool,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookSentenceParam {
    book_uuid: String,
    sentence: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteSentenceParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookWordParam {
    book_uuid: String,
    word: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteWordParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookListSentencesParam {
    book_uuid: String,
    chapter_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookListWordsParam {
    book_uuid: String,
    sentence_uuid: String,
}

// ── Understanding layer: artifacts, glossary, quiz ─────────────────────────

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookListArtifactsParam {
    book_uuid: String,
    chapter_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookArtifactParam {
    book_uuid: String,
    artifact: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteArtifactParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookRequestArtifactParam {
    book_uuid: String,
    /// `paragraph` | `chapter` | `sentence`.
    scope_type: String,
    /// break-row uuid / chapter uuid / sentence uuid.
    scope_uuid: String,
    /// `explanation`|`simplification`|`digest`|`argument_map`|`questions`.
    kind: String,
    /// Free-form guidance for the fulfilling agent (e.g. output language).
    #[serde(default)]
    instructions: Option<String>,
    /// Model hint; the fulfilling agent may ignore it.
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookListArtifactJobsParam {
    book_uuid: String,
    /// Filter to one status; omit for every outstanding request.
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookCancelArtifactJobParam {
    book_uuid: String,
    /// The job uuid, from `book_list_artifact_jobs`.
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookGenerateArtifactParam {
    book_uuid: String,
    /// `paragraph` | `chapter` | `sentence`.
    scope_type: String,
    /// break-row uuid / chapter uuid / sentence uuid.
    scope_uuid: String,
    /// `explanation`|`simplification`|`digest`|`argument_map`|`questions`.
    kind: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookAssessNoteParam {
    book_uuid: String,
    note_uuid: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookParagraphListenParam {
    book_uuid: String,
    /// The paragraph's `paragraph_break` anchor uuid.
    paragraph_uuid: String,
    #[serde(default)]
    voice: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookTermParam {
    book_uuid: String,
    term: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookDeleteTermParam {
    book_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookExtractTermsParam {
    book_uuid: String,
    scope_type: String,
    scope_uuid: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct BookRecordQuizResultParam {
    book_uuid: String,
    artifact_uuid: String,
    total: i64,
    correct: i64,
}

/// Defaults for flags whose "empty" value is `true` — `#[serde(default)]` on a
/// `bool` would mean `false`, which is not what an omitted `include_content`
/// should do.
fn default_true() -> bool {
    true
}

/// Apply `offset`/`limit` to an owned list, preserving its order.
fn page_of<T>(items: Vec<T>, limit: Option<usize>, offset: Option<usize>) -> Vec<T> {
    let start = offset.unwrap_or(0).min(items.len());
    let mut page: Vec<T> = items.into_iter().skip(start).collect();
    if let Some(limit) = limit {
        page.truncate(limit);
    }
    page
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudCreateParam {
    name: String,
    #[serde(default)]
    description: String,
    /// Language tag of the texts (`de`, `de-DE`, `zh-CN`). It decides which
    /// preferred voice a standard-audio generation uses, so set it here rather
    /// than fixing the voice per dataset.
    #[serde(default)]
    language: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudUuidParam {
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudUpdateParam {
    uuid: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Language tag (`de`, `de-DE`). Empty string clears it, omitting leaves it
    /// — an empty language means no preferred voice can apply.
    #[serde(default)]
    language: Option<String>,
}

/// Just the dataset — the shape every read-only probe over a whole read-aloud
/// dataset needs.
#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudDatasetOnlyParam {
    dataset_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudDatasetParam {
    dataset_uuid: String,
    /// Return the summary listing instead of the full rows: one `chars` count
    /// and one `has_standard_audio` flag per text, no content. This is the shape
    /// to orient yourself with — the full listing carries every passage.
    #[serde(default)]
    summary: bool,
    /// Include `content` in the full listing (default true). Ignored by
    /// `summary`, which never carries content.
    #[serde(default = "default_true")]
    include_content: bool,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

/// A text as a caller submits it. Everything the server can derive is optional,
/// so the minimal draft is `{title, content}`; `uuid`+`order_num` let a caller
/// that read a row earlier (the normalize recipe does) write it straight back.
#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudTextDraftParam {
    /// Existing row to update. Omit to create a new text (the server assigns a UUID).
    #[serde(default)]
    uuid: Option<String>,
    /// Position in the dataset. Omit to append after the last text.
    #[serde(default)]
    order_num: Option<i64>,
    /// Shown in the text list. Empty falls back to the first line of `content`.
    #[serde(default)]
    title: String,
    /// Source/location note. Defaults to empty.
    #[serde(default)]
    note: String,
    /// The passage the learner reads. Required, and must not be blank.
    content: String,
    /// Derived spoken-form scoring reference (numbers and abbreviations spelled
    /// out). Null until produced; editing `content` resets it to null.
    #[serde(default)]
    normalized_content: Option<String>,
    /// Display-only cache; per-user scores are recomputed from attempts.
    #[serde(default)]
    best_score: Option<f64>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl ReadAloudTextDraftParam {
    fn into_draft(self) -> datasets::read_aloud::TextDraft {
        datasets::read_aloud::TextDraft {
            uuid: self.uuid,
            order_num: self.order_num,
            title: self.title,
            note: self.note,
            content: self.content,
            normalized_content: self.normalized_content,
            best_score: self.best_score,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    /// What makes this draft unacceptable. Shape errors are caught by the
    /// parameter schema; this is the one content rule left.
    fn validation_error(&self) -> Option<String> {
        if self.content.trim().is_empty() {
            return Some(
                "content is empty — a text needs something to read (title alone is not enough)"
                    .to_string(),
            );
        }
        None
    }
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudSaveTextParam {
    dataset_uuid: String,
    text: ReadAloudTextDraftParam,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudSaveTextsParam {
    dataset_uuid: String,
    /// The passages, in order. `saved + failed` always equals this array's length.
    texts: Vec<ReadAloudTextDraftParam>,
    /// First `order_num` to use. Omit to append after the last text.
    #[serde(default)]
    start_order: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudDeleteTextParam {
    dataset_uuid: String,
    #[serde(alias = "text_uuid")]
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudTextRefParam {
    dataset_uuid: String,
    #[serde(alias = "uuid")]
    text_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudDeleteAttemptParam {
    dataset_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudRerecognizeParam {
    dataset_uuid: String,
    uuid: String,
    recognized: String,
    /// Word-level timings for `recognized`, as returned by
    /// `model_transcribe_detailed`. Without them the attempt keeps accuracy and
    /// pronunciation but reports fluency as not measured.
    #[serde(default)]
    words: Option<Vec<McpTimedWord>>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudScoreParam {
    content: String,
    recognized: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudNormalizeParam {
    /// The passage to normalize (the text's `content`).
    content: String,
    /// Model to run the prompt with (Ollama, or the configured provider). Omit
    /// or leave empty to get the prompt back on its own.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    temperature: Option<f32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudNormalizeMeasureParam {
    dataset_uuid: String,
    text_uuid: String,
    /// What scoring reads today: the stored `normalized_content` when there is
    /// one, the passage otherwise.
    reference: String,
    candidate: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudModelDownloadParam {
    /// Catalog id (a HuggingFace repo id). Empty installs the default espeak
    /// phoneme model that `read_aloud_assess` scores against.
    #[serde(default)]
    id: String,
    /// Download hub: `huggingface`, `hf-mirror` or `modelscope`. Empty means
    /// Hugging Face.
    #[serde(default)]
    provider: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudAssessParam {
    /// The dataset the text belongs to. Pronunciation is measured against the
    /// text's standard audio, which lives in the dataset.
    dataset_uuid: String,
    /// The text being read. Its default standard clip is the reference; without
    /// one the axis reports `no_reference`.
    text_uuid: String,
    /// The reference text (the text's `normalized_content` when it has one).
    content: String,
    /// The transcript to score against it.
    recognized: String,
    /// Base64 of the 16 kHz mono WAV take. Empty keeps accuracy only.
    #[serde(default)]
    wav_base64: String,
    /// Word-level timings from `model_transcribe_detailed` (fluency evidence).
    #[serde(default)]
    words: Option<Vec<McpTimedWord>>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudSubmitParam {
    dataset_uuid: String,
    text_uuid: String,
    #[serde(default)]
    recognized: String,
    /// Optional base64-encoded 16kHz mono WAV of the reading.
    #[serde(default)]
    wav_base64: String,
    /// Optional word-level timings from `model_transcribe_detailed`, matching
    /// `recognized`. Needed for the fluency axis; omit and it is not measured.
    #[serde(default)]
    words: Option<Vec<McpTimedWord>>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudImportTextsParam {
    dataset_uuid: String,
    /// A file to read (the caller has already extracted the epub/PDF).
    #[serde(default)]
    source_path: Option<String>,
    /// The source text itself, instead of a file.
    #[serde(default)]
    source_text: Option<String>,
    /// `auto` (default), `text`, `markdown` or `json`.
    #[serde(default)]
    format: String,
    #[serde(default)]
    chunk: ReadAloudChunkParam,
    /// Written to `info.json`, which decides the preferred voice later.
    #[serde(default)]
    language: Option<String>,
    /// Defaults to the source file's stem.
    #[serde(default)]
    title_prefix: Option<String>,
    /// Defaults to `Imported from <file>`.
    #[serde(default)]
    note: Option<String>,
    /// First `order_num`; omitted appends after the dataset's last text.
    #[serde(default)]
    start_order: Option<i64>,
    /// Report the sizing and write nothing.
    #[serde(default)]
    dry_run: bool,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudChunkParam {
    /// Paragraphs accumulate to about this many characters (default 1100).
    #[serde(default)]
    target_chars: Option<usize>,
    /// A shorter leftover merges into the previous passage (default 450).
    #[serde(default)]
    min_chars: Option<usize>,
    /// A longer paragraph breaks on sentence boundaries (default 1900).
    #[serde(default)]
    max_chars: Option<usize>,
}

impl ReadAloudChunkParam {
    fn to_options(&self) -> datasets::read_aloud::import::ChunkOptions {
        let defaults = datasets::read_aloud::import::ChunkOptions::default();
        datasets::read_aloud::import::ChunkOptions {
            target_chars: self.target_chars.unwrap_or(defaults.target_chars),
            min_chars: self.min_chars.unwrap_or(defaults.min_chars),
            max_chars: self.max_chars.unwrap_or(defaults.max_chars),
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudStandardAudioListParam {
    dataset_uuid: String,
    /// Omit for every clip in the dataset (the texts-list badges).
    #[serde(default)]
    text_uuid: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudStandardAudioGenerateParam {
    dataset_uuid: String,
    text_uuid: String,
    /// Edge TTS voice. Empty uses the voice configured in settings.
    #[serde(default)]
    voice: Option<String>,
    /// Free-form note shown in the clip list (origin, voice, provenance).
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudStandardAudioGenerateAllParam {
    dataset_uuid: String,
    /// Edge TTS voice. Omit to use the voice preferred for the dataset's
    /// language.
    #[serde(default)]
    voice: Option<String>,
    /// Only passages that have no clip yet (default true), so a repeat is safe.
    #[serde(default = "default_true")]
    only_missing: bool,
    /// How many passages this call speaks, in `order_num` order (default 10).
    #[serde(default)]
    limit: Option<i64>,
    /// Report what would happen and write nothing.
    #[serde(default)]
    dry_run: bool,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudVoiceResolveParam {
    dataset_uuid: String,
    /// A voice to test: reports what would happen if you asked for it.
    #[serde(default)]
    voice: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudStandardAudioImportParam {
    dataset_uuid: String,
    text_uuid: String,
    /// A filesystem path to the source audio.
    #[serde(default)]
    source_path: Option<String>,
    /// The file's bytes, base64 — what a mobile picker's `content://` URI must be
    /// read into, since such a URI is not a path.
    #[serde(default)]
    source_base64: Option<String>,
    /// Name of the picked file; the note defaults to `Upload · <file_name>`.
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    note: Option<String>,
    /// Crop start in seconds. Omit to keep the whole source.
    #[serde(default)]
    start: Option<f64>,
    /// Crop end in seconds. Omit to keep the whole source.
    #[serde(default)]
    end: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudStandardAudioGuessParam {
    dataset_uuid: String,
    text_uuid: String,
    /// A filesystem path to the source audio.
    #[serde(default)]
    source_path: Option<String>,
    /// The file's bytes, base64 — for a picker URI that is not a path.
    #[serde(default)]
    source_base64: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudStandardAudioProbeParam {
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    source_base64: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudStandardAudioUuidParam {
    dataset_uuid: String,
    uuid: String,
}

/// Mirrors `read_aloud::fluency::TimedWord`, the shape
/// `model_transcribe_detailed` returns. Declared separately because `schemars`
/// is a server-only dependency while the domain type is platform-neutral.
#[derive(Deserialize, schemars::JsonSchema)]
struct McpTimedWord {
    text: String,
    start: f64,
    end: f64,
}

fn timed_words(
    words: Option<Vec<McpTimedWord>>,
) -> Option<Vec<datasets::read_aloud::fluency::TimedWord>> {
    words.map(|ws| {
        ws.into_iter()
            .map(|w| datasets::read_aloud::fluency::TimedWord {
                text: w.text,
                start: w.start,
                end: w.end,
            })
            .collect()
    })
}

#[tool_router(router = books_router, vis = "pub(crate)")]
impl DatasetMcpServer {
    #[tool(name = "book_list", description = "List all books in the reading library. Returns UUID, title, path, and timestamps.")]
    async fn book_list(&self) -> Result<String, String> {
        log::info!("[MCP] book_list");
        let settings = self.app.state::<SettingsState>();
        let books = datasets::book::book_list(settings).await?;
        Ok(serde_json::to_string_pretty(&books).unwrap_or_default())
    }

    #[tool(name = "book_create", description = "Create a new book in the reading library. Returns the book metadata with UUID.")]
    async fn book_create(&self, Parameters(param): Parameters<BookTitleParam>) -> Result<String, String> {
        log::info!("[MCP] book_create: title={}", param.title);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::book::book_create(settings, param.title).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "book_rename", description = "Rename a book by UUID.")]
    async fn book_rename(&self, Parameters(param): Parameters<BookRenameParam>) -> Result<String, String> {
        log::info!("[MCP] book_rename: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::book::book_rename(settings, param.uuid, param.title).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Book renamed"}).to_string())
    }

    #[tool(name = "book_delete", description = "Delete a book by moving its whole directory (chapters, sentences, words, audio) into the app trash folder — not permanently removed, so this is recoverable. Returns `trashed_to`.")]
    async fn book_delete(&self, Parameters(param): Parameters<BookUuidParam>) -> Result<String, String> {
        log::warn!("[MCP] book_delete: uuid={}", param.book_uuid);
        let settings = self.app.state::<SettingsState>();
        let trashed_to = datasets::book::book_delete(settings, param.book_uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Book moved to trash", "trashed_to": trashed_to}).to_string())
    }

    #[tool(name = "book_list_chapters", description = "List all chapters of a book, ordered.")]
    async fn book_list_chapters(&self, Parameters(param): Parameters<BookUuidParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let chapters = datasets::book::book_list_chapters(settings, param.book_uuid).await?;
        Ok(serde_json::to_string_pretty(&chapters).unwrap_or_default())
    }

    #[tool(name = "book_save_chapter", description = "Create or update a chapter. Pass chapter as JSON with fields: uuid, book_uuid, parent_uuid, order_num, title, status.")]
    async fn book_save_chapter(&self, Parameters(param): Parameters<BookChapterParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let chapter: datasets::book::BookChapter = serde_json::from_value(param.chapter.into())
            .map_err(|e| format!("Invalid chapter JSON: {}", e))?;
        datasets::book::book_save_chapter(settings, param.book_uuid, chapter).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Chapter saved"}).to_string())
    }

    #[tool(name = "book_delete_chapter", description = "Delete a chapter and all its sentences and words.")]
    async fn book_delete_chapter(&self, Parameters(param): Parameters<BookDeleteChapterParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::book::book_delete_chapter(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Chapter deleted"}).to_string())
    }

    #[tool(name = "book_sync_chapters", description = "Reconcile a book's chapter tree from an ordered list of {uuid, parent_uuid, order_num, title} nodes; embedded uuids preserve chapters and their sentences. Pass dry_run=true to preview without writing. Returns a change summary (created/updated/moved/reordered + itemized deletions with sentence counts) and the resulting tree.")]
    async fn book_sync_chapters(&self, Parameters(param): Parameters<BookSyncChaptersParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let nodes: Vec<datasets::book::SyncNode> = serde_json::from_value(param.nodes.into())
            .map_err(|e| format!("Invalid nodes JSON: {}", e))?;
        let result = datasets::book::sync_chapters(&settings, &param.book_uuid, nodes, param.dry_run)?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "book_list_sentences", description = "List all sentences for a chapter, ordered, with resolved audio URLs.")]
    async fn book_list_sentences(&self, Parameters(param): Parameters<BookListSentencesParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let sentences = datasets::book::book_list_sentences(settings, param.book_uuid, param.chapter_uuid).await?;
        Ok(serde_json::to_string_pretty(&sentences).unwrap_or_default())
    }

    #[tool(name = "book_save_sentence", description = "Create or update a sentence. Pass sentence as JSON with fields: uuid, chapter_uuid, order_num, content, sentence_type, audio_path, etc.")]
    async fn book_save_sentence(&self, Parameters(param): Parameters<BookSentenceParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let sentence: datasets::book::BookSentence = serde_json::from_value(param.sentence.into())
            .map_err(|e| format!("Invalid sentence JSON: {}", e))?;
        datasets::book::book_save_sentence(settings, param.book_uuid, sentence).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Sentence saved"}).to_string())
    }

    #[tool(name = "book_delete_sentence", description = "Delete a sentence, its words, and its audio file.")]
    async fn book_delete_sentence(&self, Parameters(param): Parameters<BookDeleteSentenceParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::book::book_delete_sentence(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Sentence deleted"}).to_string())
    }

    #[tool(name = "book_list_words", description = "List vocabulary words saved for a sentence.")]
    async fn book_list_words(&self, Parameters(param): Parameters<BookListWordsParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let words = datasets::book::book_list_words(settings, param.book_uuid, param.sentence_uuid).await?;
        Ok(serde_json::to_string_pretty(&words).unwrap_or_default())
    }

    #[tool(name = "book_save_word", description = "Create or update a vocabulary word. Pass word as JSON with fields: uuid, sentence_uuid, word, word_type, note.")]
    async fn book_save_word(&self, Parameters(param): Parameters<BookWordParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let word: datasets::book::BookSentenceWord = serde_json::from_value(param.word.into())
            .map_err(|e| format!("Invalid word JSON: {}", e))?;
        datasets::book::book_save_word(settings, param.book_uuid, word).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Word saved"}).to_string())
    }

    #[tool(name = "book_delete_word", description = "Delete a vocabulary word.")]
    async fn book_delete_word(&self, Parameters(param): Parameters<BookDeleteWordParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::book::book_delete_word(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Word deleted"}).to_string())
    }

    // ── Understanding layer: artifacts, glossary, quiz ──────────────────────

    #[tool(name = "book_list_artifacts", description = "List understanding artifacts for a chapter (explanations, simplifications, digests, argument maps, quizzes, notes, assessments). Each carries a computed `stale` flag when its source text changed since generation.")]
    async fn book_list_artifacts(&self, Parameters(param): Parameters<BookListArtifactsParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let artifacts = datasets::book_artifacts::book_list_artifacts(settings, param.book_uuid, param.chapter_uuid).await?;
        Ok(serde_json::to_string_pretty(&artifacts).unwrap_or_default())
    }

    #[tool(name = "book_save_artifact", description = "Create or update an artifact. Pass artifact as JSON with fields: uuid (empty to create), scope_type ('paragraph'|'chapter'|'sentence'), scope_uuid, kind ('explanation'|'simplification'|'digest'|'argument_map'|'questions'|'note'|'assessment'), content, source, model?, parent_uuid?, content_hash?.")]
    async fn book_save_artifact(&self, Parameters(param): Parameters<BookArtifactParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let artifact: datasets::book_artifacts::BookArtifact = serde_json::from_value(param.artifact.into())
            .map_err(|e| format!("Invalid artifact JSON: {}", e))?;
        datasets::book_artifacts::book_save_artifact(self.app.clone(), settings, param.book_uuid, artifact).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Artifact saved"}).to_string())
    }

    #[tool(name = "book_delete_artifact", description = "Delete an artifact (a generated explanation/digest/quiz, or a user note). Derived artifacts can be regenerated.")]
    async fn book_delete_artifact(&self, Parameters(param): Parameters<BookDeleteArtifactParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::book_artifacts::book_delete_artifact(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Artifact deleted"}).to_string())
    }

    #[tool(name = "book_list_artifact_jobs", description = "List outstanding requests for an agent to generate understanding artifacts in a book — the mailbox the Understand panel's 'Ask agent' button writes to. Each job carries scope_type, scope_uuid, kind, optional instructions/model, the source_text to generate from, and a stale flag. Fulfil one by calling book_save_artifact with the same scope_uuid + kind and source 'agent'; that writes the artifact, clears the job and refreshes the user's panel. Optional status filter.")]
    async fn book_list_artifact_jobs(&self, Parameters(param): Parameters<BookListArtifactJobsParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let jobs = datasets::book_artifacts::book_list_artifact_jobs(settings, param.book_uuid, param.status).await?;
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "count": jobs.len(),
            "jobs": jobs,
        }))
        .unwrap_or_default())
    }

    #[tool(name = "book_request_artifact", description = "Queue a request for a third-party agent to generate an understanding artifact, instead of the built-in LLM. kind: 'explanation'|'simplification'|'digest'|'argument_map'|'questions'; scope_type: 'paragraph'|'chapter'|'sentence'. Idempotent per (scope, kind). An attached agent picks it up via book_list_artifact_jobs and delivers through book_save_artifact. Returns the job (with the source text).")]
    async fn book_request_artifact(&self, Parameters(param): Parameters<BookRequestArtifactParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let job = datasets::book_artifacts::book_request_artifact(
            self.app.clone(),
            settings,
            param.book_uuid,
            param.scope_type,
            param.scope_uuid,
            param.kind,
            param.instructions,
            param.model,
        )
        .await?;
        Ok(serde_json::to_string_pretty(&job).unwrap_or_default())
    }

    #[tool(name = "book_cancel_artifact_job", description = "Cancel an outstanding artifact job (delete it). Use when a request is no longer wanted; an artifact already produced is untouched.")]
    async fn book_cancel_artifact_job(&self, Parameters(param): Parameters<BookCancelArtifactJobParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::book_artifacts::book_cancel_artifact_job(self.app.clone(), settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Job cancelled"}).to_string())
    }

    #[tool(name = "book_generate_artifact", description = "Generate an artifact with the LLM for a scope. kind: 'explanation'|'simplification'|'digest'|'argument_map'|'questions'. scope_type: 'paragraph'|'chapter'|'sentence'. Optional model and instructions (e.g. output language) override defaults.")]
    async fn book_generate_artifact(&self, Parameters(param): Parameters<BookGenerateArtifactParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let artifact = datasets::book_artifacts::book_generate_artifact(
            settings,
            param.book_uuid,
            param.scope_type,
            param.scope_uuid,
            param.kind,
            param.model,
            param.instructions,
        )
        .await?;
        Ok(serde_json::to_string_pretty(&artifact).unwrap_or_default())
    }

    #[tool(name = "book_assess_note", description = "Have the LLM assess a user note, producing an 'assessment' artifact linked to it via parent_uuid.")]
    async fn book_assess_note(&self, Parameters(param): Parameters<BookAssessNoteParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let artifact = datasets::book_artifacts::book_assess_note(settings, param.book_uuid, param.note_uuid, param.model).await?;
        Ok(serde_json::to_string_pretty(&artifact).unwrap_or_default())
    }

    #[tool(name = "book_paragraph_listen", description = "Synthesise a paragraph's text with Edge TTS into the book's media/ and attach it to the paragraph anchor. paragraph_uuid is the 'paragraph_break' row uuid.")]
    async fn book_paragraph_listen(&self, Parameters(param): Parameters<BookParagraphListenParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result = datasets::book_artifacts::book_paragraph_listen(settings, param.book_uuid, param.paragraph_uuid, param.voice).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "book_list_terms", description = "List the book's glossary terms with definitions.")]
    async fn book_list_terms(&self, Parameters(param): Parameters<BookUuidParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let terms = datasets::book_artifacts::book_list_terms(settings, param.book_uuid).await?;
        Ok(serde_json::to_string_pretty(&terms).unwrap_or_default())
    }

    #[tool(name = "book_save_term", description = "Create or update a glossary term. Pass term as JSON with fields: uuid (empty to create), term, definition, source, model?. Upserts on (book, term); a user-edited definition is preserved when source is 'user'.")]
    async fn book_save_term(&self, Parameters(param): Parameters<BookTermParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let term: datasets::book_artifacts::BookTerm = serde_json::from_value(param.term.into())
            .map_err(|e| format!("Invalid term JSON: {}", e))?;
        let saved = datasets::book_artifacts::book_save_term(settings, param.book_uuid, term).await?;
        Ok(serde_json::to_string_pretty(&saved).unwrap_or_default())
    }

    #[tool(name = "book_delete_term", description = "Delete a glossary term.")]
    async fn book_delete_term(&self, Parameters(param): Parameters<BookDeleteTermParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::book_artifacts::book_delete_term(settings, param.book_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Term deleted"}).to_string())
    }

    #[tool(name = "book_extract_terms", description = "Have the LLM extract recurring philosophical/technical concepts from a scope into the glossary. scope_type: 'paragraph'|'chapter'|'sentence'. Never overwrites user-edited definitions.")]
    async fn book_extract_terms(&self, Parameters(param): Parameters<BookExtractTermsParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let terms = datasets::book_artifacts::book_extract_terms(settings, param.book_uuid, param.scope_type, param.scope_uuid, param.model).await?;
        Ok(serde_json::to_string_pretty(&terms).unwrap_or_default())
    }

    #[tool(name = "book_record_quiz_result", description = "Record a quiz attempt (total/correct) for a 'questions' artifact and award XP for the completion.")]
    async fn book_record_quiz_result(&self, Parameters(param): Parameters<BookRecordQuizResultParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result = datasets::book_artifacts::book_record_quiz_result(
            self.app.clone(),
            settings,
            param.book_uuid,
            param.artifact_uuid,
            param.total,
            param.correct,
        )
        .await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "read_aloud_list", description = "List all read-aloud datasets. Returns UUID, name, description, path, and timestamps.")]
    async fn read_aloud_list(&self) -> Result<String, String> {
        log::info!("[MCP] read_aloud_list");
        let settings = self.app.state::<SettingsState>();
        let items = datasets::read_aloud::read_aloud_list(settings).await?;
        Ok(serde_json::to_string_pretty(&items).unwrap_or_default())
    }

    #[tool(name = "read_aloud_create", description = "Create a read-aloud dataset (a collection of passages to practise reading aloud). Pass `name` and optionally `description` and `language` (a tag like `de` or `de-DE`). The language is what makes generation pick the user's preferred voice for that language, so set it here. Returns the dataset metadata with its UUID; `texts_total` is 0 until texts are added.")]
    async fn read_aloud_create(&self, Parameters(param): Parameters<ReadAloudCreateParam>) -> Result<String, String> {
        log::info!("[MCP] read_aloud_create: name={}", param.name);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::read_aloud::read_aloud_create(
            settings,
            param.name,
            Some(param.description),
            param.language,
        )
        .await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "read_aloud_update", description = "Update a read-aloud dataset's name, description and/or language. `language` is the tag (`de`, `de-DE`) a generation uses to pick a preferred voice; pass an empty string to clear it, omit the field to leave it alone.")]
    async fn read_aloud_update(&self, Parameters(param): Parameters<ReadAloudUpdateParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::read_aloud_update(
            settings,
            param.uuid,
            param.name,
            param.description,
            param.language,
        )
        .await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset updated"}).to_string())
    }

    #[tool(name = "read_aloud_delete", description = "Delete a read-aloud dataset by moving its whole directory (texts, attempts, recordings) into the app trash folder — not permanently removed, so this is recoverable. Returns `trashed_to`.")]
    async fn read_aloud_delete(&self, Parameters(param): Parameters<ReadAloudUuidParam>) -> Result<String, String> {
        log::warn!("[MCP] read_aloud_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let trashed_to = datasets::read_aloud::read_aloud_delete(settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset moved to trash", "trashed_to": trashed_to}).to_string())
    }

    #[tool(name = "read_aloud_list_texts", description = "List a dataset's passages. Default: every row in full, ordered — every `content` on the wire, so prefer `summary` when you only need to orient yourself. `summary: true` returns `{count, total_chars, with_standard_audio, without_standard_audio, texts: [{uuid, order_num, title, chars, attempt_count, best_score, has_standard_audio}]}` with no content. `limit`/`offset` page either shape in `order_num` order (the same order `read_aloud_standard_audio_generate_all` walks); `include_content: false` drops `content` from the full shape.")]
    async fn read_aloud_list_texts(&self, Parameters(param): Parameters<ReadAloudDatasetParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        if param.summary {
            let items = datasets::read_aloud::list_text_summaries(&settings, &param.dataset_uuid)?;
            let count = items.len();
            let total_chars: i64 = items.iter().map(|t| t.chars).sum();
            let with_audio = items.iter().filter(|t| t.has_standard_audio).count();
            let page = page_of(items, param.limit, param.offset);
            return Ok(serde_json::json!({
                "dataset_uuid": param.dataset_uuid,
                "count": count,
                "total_chars": total_chars,
                "with_standard_audio": with_audio,
                "without_standard_audio": count - with_audio,
                "texts": page,
            })
            .to_string());
        }
        let mut items = datasets::read_aloud::list_texts(&settings, &param.dataset_uuid)?;
        if !param.include_content {
            for text in &mut items {
                text.content.clear();
            }
        }
        // No paging and content included: the historical shape, unchanged.
        if param.limit.is_none() && param.offset.is_none() {
            return Ok(serde_json::to_string_pretty(&items).unwrap_or_default());
        }
        let total = items.len();
        let page = page_of(items, param.limit, param.offset);
        Ok(serde_json::json!({ "dataset_uuid": param.dataset_uuid, "count": total, "texts": page }).to_string())
    }

    #[tool(name = "read_aloud_get_text", description = "Read one passage plus its standard audio in one call: the row (content, normalized_content, note, scores) and `standard_audio: {default_uuid, usable, clips}`. Use this instead of listing a whole dataset to inspect a single text.")]
    async fn read_aloud_get_text(&self, Parameters(param): Parameters<ReadAloudTextRefParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let text = datasets::read_aloud::get_text(&settings, &param.dataset_uuid, &param.text_uuid)?;
        let clips = datasets::read_aloud::standard_audio::list(
            &settings,
            &param.dataset_uuid,
            Some(&param.text_uuid),
        )?;
        let default_clip = clips.iter().find(|c| c.is_default);
        Ok(serde_json::json!({
            "status": "ok",
            "text": text,
            "standard_audio": {
                "default_uuid": default_clip.map(|c| c.uuid.clone()),
                "usable": default_clip.map(|c| c.usable).unwrap_or(false),
                "clips": clips,
            },
        })
        .to_string())
    }

    #[tool(name = "read_aloud_save_text", description = "Create or update one passage. Required: `content`. Optional: `title` (defaults to the first line of content), `note`, `normalized_content`, `uuid` (omit to create; pass the uuid of a row you read to update it), `order_num` (omit to append). `created_at`/`updated_at`/`best_score` are server-assigned when omitted, so round-tripping a row you read works unchanged. Returns `{status, uuid, order_num, texts_total}` — `texts_total` is the proof the write landed. For many passages use read_aloud_save_texts, which is one call instead of one per text.")]
    async fn read_aloud_save_text(&self, Parameters(param): Parameters<ReadAloudSaveTextParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        if let Some(error) = param.text.validation_error() {
            return Err(error);
        }
        let draft = param.text.into_draft();
        let order = datasets::read_aloud::next_order_num(&settings, &param.dataset_uuid)?;
        let row = draft.into_row(&param.dataset_uuid, order);
        datasets::read_aloud::save_text_row(&settings, &param.dataset_uuid, &row)?;
        let texts_total = datasets::read_aloud::count_texts(&settings, &param.dataset_uuid);
        Ok(serde_json::json!({
            "status": "ok",
            "uuid": row.uuid,
            "order_num": row.order_num,
            "texts_total": texts_total,
        })
        .to_string())
    }

    #[tool(name = "read_aloud_save_texts", description = "Create or update many passages in one call — the bulk form of read_aloud_save_text. Each item takes the same fields (`content` required; `uuid`/`order_num`/timestamps optional). `start_order` sets the first order_num; omitted means append. Returns `{status, sent, saved, failed, errors: [{index, title, error}], texts_total, order_range}`, where `saved + failed` always equals `sent` — check `failed` and `errors[index]` instead of assuming success. Use this for an imported book: one call, one proof.")]
    async fn read_aloud_save_texts(&self, Parameters(param): Parameters<ReadAloudSaveTextsParam>) -> Result<String, String> {
        log::info!("[MCP] read_aloud_save_texts: dataset={}, {} texts", param.dataset_uuid, param.texts.len());
        let settings = self.app.state::<SettingsState>();
        let dataset_uuid = param.dataset_uuid;
        let sent = param.texts.len();
        let mut order = match param.start_order {
            Some(o) => o,
            None => datasets::read_aloud::next_order_num(&settings, &dataset_uuid)?,
        };
        let mut accepted: Vec<(usize, datasets::read_aloud::ReadText)> = Vec::new();
        let mut errors: Vec<serde_json::Value> = Vec::new();
        for (index, draft) in param.texts.into_iter().enumerate() {
            let title = draft.title.clone();
            if let Some(error) = draft.validation_error() {
                errors.push(serde_json::json!({ "index": index, "title": title, "error": error }));
                continue;
            }
            let row = draft.into_draft().into_row(&dataset_uuid, order);
            order = row.order_num.max(order) + 1;
            accepted.push((index, row));
        }
        let rows: Vec<datasets::read_aloud::ReadText> =
            accepted.iter().map(|(_, row)| row.clone()).collect();
        let batch = datasets::read_aloud::save_text_rows(&settings, &dataset_uuid, &rows)?;
        for (row_index, title, error) in &batch.errors {
            let caller_index = accepted.get(*row_index).map(|(i, _)| *i).unwrap_or(*row_index);
            errors.push(serde_json::json!({ "index": caller_index, "title": title, "error": error }));
        }
        errors.sort_by_key(|e| e.get("index").and_then(|i| i.as_u64()).unwrap_or(0));
        let order_range = if batch.saved == 0 {
            serde_json::Value::Null
        } else {
            let failed_rows: std::collections::HashSet<usize> =
                batch.errors.iter().map(|(i, _, _)| *i).collect();
            let mut orders: Vec<i64> = accepted
                .iter()
                .enumerate()
                .filter(|(i, _)| !failed_rows.contains(i))
                .map(|(_, (_, row))| row.order_num)
                .collect();
            orders.sort_unstable();
            serde_json::json!([orders.first(), orders.last()])
        };
        let texts_total = datasets::read_aloud::count_texts(&settings, &dataset_uuid);
        Ok(serde_json::json!({
            "status": if errors.is_empty() { "ok" } else { "partial" },
            "sent": sent,
            "saved": batch.saved,
            "failed": errors.len(),
            "errors": errors,
            "texts_total": texts_total,
            "order_range": order_range,
        })
        .to_string())
    }

    #[tool(name = "read_aloud_import_texts", description = "Turn a book into passages in one call: pass `source_path` (a file you extracted yourself — this tool does not read epub/PDF) or `source_text`, and it segments, titles and saves them. `format`: auto (default, from the extension and the first character), text, markdown or json. Prose: blank lines end paragraphs, `#` headings are hard boundaries, paragraphs accumulate to `chunk.target_chars` (1100), anything over `chunk.max_chars` (1900) breaks on sentence boundaries, and a stub under `chunk.min_chars` (450) merges backwards. `format: json` takes `[{title, note, content}]` verbatim and reports blank items in `skipped`. `language` is written to info.json so generation picks the right voice. ALWAYS run `dry_run: true` first: it returns `{texts, chars: {min, median, max}, total_chars, estimate_speaking_seconds, preview}` and writes nothing. The real write returns the same `{saved, failed, errors}` partition as read_aloud_save_texts.")]
    async fn read_aloud_import_texts(
        &self,
        Parameters(param): Parameters<ReadAloudImportTextsParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] read_aloud_import_texts: dataset={}, format={}, dry_run={}",
            param.dataset_uuid,
            param.format,
            param.dry_run
        );
        let settings = self.app.state::<SettingsState>();
        let format = datasets::read_aloud::import::SourceFormat::parse(&param.format)?;
        let source = datasets::read_aloud::import::Source::resolve(
            param.source_path.as_deref(),
            param.source_text.as_deref(),
            format,
        )?;
        let out = datasets::read_aloud::import::import_texts(
            settings.inner(),
            &param.dataset_uuid,
            source,
            param.chunk.to_options(),
            param.language.as_deref(),
            param.title_prefix.as_deref(),
            param.note.as_deref(),
            param.start_order,
            param.dry_run,
        )?;
        Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
    }

    #[tool(name = "read_aloud_delete_text", description = "Delete a text. Per-user attempt history is left untouched in the app DB (a content edit never removes practice data).")]
    async fn read_aloud_delete_text(&self, Parameters(param): Parameters<ReadAloudDeleteTextParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::read_aloud_delete_text(settings, param.dataset_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Text deleted"}).to_string())
    }

    #[tool(name = "read_aloud_list_attempts", description = "List all recorded attempts for a text (newest first) with score, transcript, resolved audio URL, and the stored assessment (`metrics`: accuracy counts, fluency, pronunciation, weakest words, pauses) when one was computed. Metrics are absent for takes whose recording is gone or that predate assessment.")]
    async fn read_aloud_list_attempts(&self, Parameters(param): Parameters<ReadAloudTextRefParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let items = datasets::read_aloud::read_aloud_list_attempts(settings, param.dataset_uuid, param.text_uuid).await?;
        Ok(serde_json::to_string_pretty(&items).unwrap_or_default())
    }

    #[tool(name = "read_aloud_delete_attempt", description = "Delete a single recorded attempt and its audio file.")]
    async fn read_aloud_delete_attempt(&self, Parameters(param): Parameters<ReadAloudDeleteAttemptParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::read_aloud_delete_attempt(settings, param.dataset_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Attempt deleted"}).to_string())
    }

    #[tool(name = "read_aloud_rerecognize", description = "Overwrite an existing attempt's transcript with a freshly recognized one and recompute its score against the current reference, in place. The recording is left untouched — pass the transcript produced by re-transcribing the attempt's device-local WAV (via model_transcribe_detailed, whose `words` go straight into this tool's `words` parameter) with the loaded STT model. Use this to refresh a take whose recognition was poor or predates a model change. Returns the updated attempt (score and recomputed metrics included).")]
    async fn read_aloud_rerecognize(&self, Parameters(param): Parameters<ReadAloudRerecognizeParam>) -> Result<String, String> {
        log::info!("[MCP] read_aloud_rerecognize: dataset={}, attempt={}", param.dataset_uuid, param.uuid);
        let settings = self.app.state::<SettingsState>();
        let attempt = datasets::read_aloud::read_aloud_rerecognize(
            settings,
            param.dataset_uuid,
            param.uuid,
            param.recognized,
            timed_words(param.words),
        )
        .await?;
        Ok(serde_json::to_string_pretty(&attempt).unwrap_or_default())
    }

    #[tool(name = "read_aloud_score", description = "Compute the WIP-based ASR score (0-100) and the word-level alignment (wer, mer, hits/substitutions/deletions/insertions, chunks) between a reference text and a recognized transcript, without persisting anything. Score >= 60 passes. Use the chunks to see exactly which words were misread, inserted, or skipped. Also returned inside read_aloud_submit results.")]
    async fn read_aloud_score(&self, Parameters(param): Parameters<ReadAloudScoreParam>) -> Result<String, String> {
        let summary = datasets::read_aloud::read_aloud_score(param.content, param.recognized).await?;
        let mut v = serde_json::to_value(&summary).map_err(|e| format!("Failed to serialize summary: {}", e))?;
        v["passed"] = serde_json::json!(v.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0) >= 60.0);
        Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
    }

    #[tool(name = "read_aloud_normalize", description = "Turn a passage into its spoken-form scoring reference (`normalized_content`): numbers, cardinals, dates, times, currency, percentages, units and abbreviations spelled out the way they are read aloud, with nothing else changed. The prompt is the app's own, so a candidate from any provider has the same shape. Pass `model` to run it in-app (Ollama, or the configured provider); omit it to receive the `prompt` alone and run it through an external LLM. Nothing is written: save the candidate with read_aloud_save_text, and gate it first with read_aloud_normalize_measure.")]
    async fn read_aloud_normalize(&self, Parameters(param): Parameters<ReadAloudNormalizeParam>) -> Result<String, String> {
        log::info!("[MCP] read_aloud_normalize: model={:?}, {} chars", param.model, param.content.len());
        let settings = self.app.state::<SettingsState>();
        let result = datasets::read_aloud::normalize::generate(settings, param.content, param.model, param.temperature).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "read_aloud_normalize_measure", description = "Gate a candidate normalization against the user's own recordings of one text: the best score the reference in use today achieves (`baseline`) versus the candidate's (`candidate`), over the same transcripts. `attempts` says how many were compared — 0 means the numbers prove nothing and the text has no practice history yet. Nothing is written and nothing is saved. Keep the candidate only when it is strictly higher; equal means the mismatch was not spelling, lower means it reworded the passage.")]
    async fn read_aloud_normalize_measure(&self, Parameters(param): Parameters<ReadAloudNormalizeMeasureParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let result = datasets::read_aloud::normalize::measure(
            &settings,
            &param.dataset_uuid,
            &param.text_uuid,
            &param.reference,
            &param.candidate,
        )?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "read_aloud_assess", description = "Score an attempt across all three axes without persisting anything: accuracy (the WIP 0-100 score, identical to read_aloud_score), fluency (words/min, articulation rate, pauses, longest run — needs `words` timings from model_transcribe_detailed and the WAV), and pronunciation (CTC phoneme GOP against the text's standard audio — the dataset-owned reference clip, never a network call). Each axis degrades honestly: a missing phoneme model, an absent standard clip or an attempt too short to align reports `unavailable_reason` (no_reference / stale_reference / model_missing / too_short) instead of a fabricated score. Use this to preview a result before read_aloud_submit; call read_aloud_standard_audio_list first when no_reference comes back. The result also carries a one-line `headline` (accuracy, pace, pauses, pronunciation) beside the full structure, so an agent can report the attempt without parsing it.")]
    async fn read_aloud_assess(&self, Parameters(param): Parameters<ReadAloudAssessParam>) -> Result<String, String> {
        log::info!("[MCP] read_aloud_assess: {} chars", param.content.len());
        let settings = self.app.state::<SettingsState>();
        let assessment = datasets::read_aloud::read_aloud_assess(
            settings,
            param.dataset_uuid,
            param.text_uuid,
            param.content,
            param.recognized,
            param.wav_base64,
            timed_words(param.words),
        )
        .await?;
        // The structure plus the one line an agent can act on without reading
        // it — the whole point of `Assessment::headline`.
        let mut value = assessment.to_value();
        value["headline"] = serde_json::Value::String(assessment.headline());
        Ok(serde_json::to_string_pretty(&value).unwrap_or_default())
    }

    #[tool(name = "read_aloud_standard_audio_assess", description = "Score a text's own standard audio — the reference clip pronunciation is measured against — through the same three axes a take goes through: the clip is transcribed with the loaded STT model and scored against the reference text (accuracy), paced from its own waveform (fluency, needs word timings from a Parakeet model), and aligned against phones derived from itself (pronunciation). This is a self-test of the reference, not practice: nothing is stored, no XP is awarded, and no attempt is created. A healthy clip reads high on all three; low accuracy means the audio does not say what the text says, which would mark down every correct reading. Call read_aloud_standard_audio_list first if it reports that no clip exists.")]
    async fn read_aloud_standard_audio_assess(&self, Parameters(param): Parameters<ReadAloudTextRefParam>) -> Result<String, String> {
        log::info!(
            "[MCP] read_aloud_standard_audio_assess: dataset={}, text={}",
            param.dataset_uuid,
            param.text_uuid
        );
        let settings = self.app.state::<SettingsState>();
        let model_state = self.app.state::<crate::models::ModelState>();
        let result = datasets::read_aloud::assess_standard_audio(
            &settings,
            &model_state,
            &param.dataset_uuid,
            &param.text_uuid,
        )
        .await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "read_aloud_submit", description = "Submit a reading attempt: stores the (optional) recording, scores the recognized transcript against the reference text with the WIP-based ASR metric, assesses fluency and pronunciation from the WAV, saves the attempt, and awards scaled XP (1 XP at score >= 60, +1 more at >= 80). Pass `words` (from model_transcribe_detailed) to get the fluency axis. Returns the score, the full scoring summary (alignment chunks included), the three-axis `assessment`, the saved attempt (with `metrics`), and XP awarded.")]
    async fn read_aloud_submit(&self, Parameters(param): Parameters<ReadAloudSubmitParam>) -> Result<String, String> {
        log::info!("[MCP] read_aloud_submit: dataset={}, text={}", param.dataset_uuid, param.text_uuid);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::read_aloud::read_aloud_submit(
            self.app.clone(),
            settings,
            param.dataset_uuid,
            param.text_uuid,
            param.wav_base64,
            param.recognized,
            timed_words(param.words),
        )
        .await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "read_aloud_model_status", description = "Report whether the pronunciation (phoneme) model that read_aloud_assess and read_aloud_submit score against is installed, the directory it loads from, the download catalog (size + per-model state), and a `hint` naming the next step. This axis is independent of the STT model: a Parakeet model gives fluency, this gives pronunciation. Call this before read_aloud_model_download.")]
    async fn read_aloud_model_status(&self) -> Result<String, String> {
        log::info!("[MCP] read_aloud_model_status");
        let status = datasets::read_aloud::phoneme_model_status();
        let installed = status.installed_dir.is_some();
        let hint = if installed {
            "Pronunciation scoring is available; nothing to do."
        } else {
            "Call read_aloud_model_download to install it (~600 MB). Until then read_aloud_assess still returns accuracy and fluency, with pronunciation.unavailable_reason = model_missing."
        };
        let value = serde_json::json!({
            "installed": installed,
            "installed_dir": status.installed_dir,
            "models": status.models,
            "hint": hint,
        });
        Ok(serde_json::to_string_pretty(&value).unwrap_or_default())
    }

    #[tool(name = "read_aloud_model_download", description = "Download the pronunciation (phoneme) model used by read_aloud_assess / read_aloud_submit. Omit `id` to install the default espeak IPA model (~600 MB). Use `provider` to pick a download hub in regions where huggingface.co is blocked (hf-mirror / modelscope). Check read_aloud_model_status first: installing is only needed when `installed` is false.")]
    async fn read_aloud_model_download(
        &self,
        Parameters(param): Parameters<ReadAloudModelDownloadParam>,
    ) -> Result<String, String> {
        let id = if param.id.trim().is_empty() {
            crate::models::catalog::DEFAULT_PHONEME_MODEL_ID
        } else {
            param.id.trim()
        };
        let provider = if param.provider.trim().is_empty() {
            None
        } else {
            Some(param.provider.trim())
        };
        log::info!("[MCP] read_aloud_model_download: {}", id);
        let state = self.app.state::<crate::models::ModelState>();
        let index = self.app.state::<crate::models::index::ModelIndexState>();
        crate::models::hf_download_inner(self.app.clone(), &state, &index, id, provider).await?;
        let value = serde_json::json!({
            "status": "ok",
            "id": id,
            "message": "Pronunciation model installed; pronunciation is now measured on the next attempt.",
        });
        Ok(serde_json::to_string_pretty(&value).unwrap_or_default())
    }

    #[tool(name = "read_aloud_standard_audio_list", description = "List the standard-audio clips of one text (pass `text_uuid`) or of the whole dataset (omit it, for the list badges). Each clip reports `kind` (tts|upload), `duration`, `is_default`, `stale` and `usable`. Pronunciation is measured against the DEFAULT clip only: until a text has a usable one, read_aloud_assess / read_aloud_submit report pronunciation.unavailable_reason = no_reference, and a clip made from an earlier version of the text reports stale_reference. Next step in either case: read_aloud_standard_audio_generate, or _import for a human recording.")]
    async fn read_aloud_standard_audio_list(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioListParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] read_aloud_standard_audio_list: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let clips = datasets::read_aloud::standard_audio::list(
            settings.inner(),
            &param.dataset_uuid,
            param.text_uuid.as_deref(),
        )?;
        Ok(serde_json::to_string_pretty(&clips).unwrap_or_default())
    }

    #[tool(name = "read_aloud_standard_audio_generate", description = "Synthesise ONE text with Edge TTS and store the result as its standard-audio clip (the reference pronunciation is measured against). `voice` is optional: omit it and the dataset's language decides (see read_aloud_voice_resolve). Returns `{status, clip, voice, voice_locale, voice_source, texts_with_audio, texts_total, remaining, voice_warning?, hint?}` — `remaining` is how many texts still have no clip. The first clip of a text becomes its default and takes effect immediately. PROBE FIRST: generate 2-3 of the first passages, have a human listen, then use read_aloud_standard_audio_generate_all for the rest.")]
    async fn read_aloud_standard_audio_generate(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioGenerateParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] read_aloud_standard_audio_generate: text={}",
            param.text_uuid
        );
        let settings = self.app.state::<SettingsState>();
        let (clip, resolved) = datasets::read_aloud::standard_audio::generate(
            settings.inner(),
            &param.dataset_uuid,
            &param.text_uuid,
            param.voice.as_deref(),
            param.note.as_deref(),
        )
        .await?;
        let (texts_with_audio, texts_total) =
            datasets::read_aloud::standard_audio::audio_totals(settings.inner(), &param.dataset_uuid)?;
        Ok(serde_json::json!({
            "status": "ok",
            "clip": clip,
            "voice": resolved.voice,
            "voice_locale": resolved.locale,
            "voice_source": resolved.source,
            "texts_with_audio": texts_with_audio,
            "texts_total": texts_total,
            "remaining": texts_total - texts_with_audio,
            "voice_warning": resolved.warning,
            "hint": resolved.hint,
        })
        .to_string())
    }

    #[tool(name = "read_aloud_standard_audio_generate_all", description = "Generate standard audio for many texts in one call, walking them in `order_num` order — the same code path as read_aloud_standard_audio_generate. `limit` (default 10) bounds the call to roughly two minutes; `only_missing` (default true) skips texts that already have a clip, so a repeat is safe; `dry_run` returns `{would_generate, voice, voice_source, estimate_seconds}` and writes nothing. A run returns `{generated, failed: [{text_uuid, error}], remaining, ...}`; `remaining` decreasing is the progress bar. To test a voice on a new dataset call it with `limit: 3`, then repeat until `remaining` is 0.")]
    async fn read_aloud_standard_audio_generate_all(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioGenerateAllParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] read_aloud_standard_audio_generate_all: dataset={}, limit={:?}, only_missing={}, dry_run={}",
            param.dataset_uuid,
            param.limit,
            param.only_missing,
            param.dry_run
        );
        let settings = self.app.state::<SettingsState>();
        let out = datasets::read_aloud::standard_audio::generate_all(
            settings.inner(),
            &param.dataset_uuid,
            param.voice.as_deref(),
            param.only_missing,
            param.limit,
            param.dry_run,
        )
        .await?;
        Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
    }

    #[tool(name = "read_aloud_voice_resolve", description = "Report which Edge TTS voice a generation for this dataset would use, and why — before spending a synthesis on finding out. Precedence: an explicit `voice` (`param`), then the voice preferred for the dataset's language (`preference`, exact tag like `de-DE` before the bare `de`), then the language-less `read_aloud_tts_voice` (`setting`), then the built-in multilingual default (`default`). Also returns `dataset_language`, `languages_with_preference`, a `warning` when the voice speaks a different language than the dataset, and a `hint` when the dataset has no language (so no preference can apply — set one with read_aloud_update).")]
    async fn read_aloud_voice_resolve(
        &self,
        Parameters(param): Parameters<ReadAloudVoiceResolveParam>,
    ) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let resolution = datasets::read_aloud::standard_audio::resolve_for_dataset(
            settings.inner(),
            &param.dataset_uuid,
            param.voice.as_deref(),
        )?;
        Ok(serde_json::to_string_pretty(&resolution).unwrap_or_default())
    }

    #[tool(name = "read_aloud_audit", description = "Read-only health check of a read-aloud dataset: the deep version of 'does this look finished?'. Reports named checks with an exact offender `count`, up to 100 offenders (`source` is the text uuid, `detail` says why) and advice: `texts_none` (no passage at all, so every per-text check below proves nothing), `text_empty` (blank content), `text_duplicate` (same content in two rows — usually an import run twice), `standard_audio_missing` (no clip, so pronunciation reports no_reference forever). Also returns `texts`, `texts_with_audio` and the `facts` a workflow's when-guards read. Never writes: repair with read_aloud_standard_audio_generate_all, read_aloud_delete_text or read_aloud_import_texts.")]
    async fn read_aloud_audit(
        &self,
        Parameters(param): Parameters<ReadAloudDatasetOnlyParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] read_aloud_audit: dataset={}", param.dataset_uuid);
        let settings = self.app.state::<SettingsState>();
        let report = datasets::read_aloud::audit::audit(settings.inner(), &param.dataset_uuid)?;
        Ok(serde_json::to_string_pretty(&report).unwrap_or_default())
    }

    #[tool(name = "read_aloud_standard_audio_import", description = "Store an existing recording as a text's standard audio, optionally cut to `[start, end)` seconds. Pass `source_path` (a filesystem path) or, on mobile where a picker yields a content:// URI, `source_base64` + `file_name`. The first clip of a text becomes its default. An upload carries no word timings, so per-word attribution is time-proportional (the aggregate score is unaffected); generate with Edge TTS when exact per-word detail matters. Probe the file with read_aloud_standard_audio_probe first to pick the range.")]
    async fn read_aloud_standard_audio_import(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioImportParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] read_aloud_standard_audio_import: text={}",
            param.text_uuid
        );
        let settings = self.app.state::<SettingsState>();
        let clip = datasets::read_aloud::standard_audio::import(
            settings.inner(),
            &param.dataset_uuid,
            &param.text_uuid,
            param.source_path.as_deref(),
            param.source_base64.as_deref(),
            param.file_name.as_deref(),
            param.note.as_deref(),
            param.start,
            param.end,
        )?;
        Ok(serde_json::to_string_pretty(&clip).unwrap_or_default())
    }

    #[tool(name = "read_aloud_standard_audio_probe", description = "Decode a candidate audio file and return its duration plus a peak envelope (interleaved min/max i8 pairs, one per `seconds_per_pair`) so a range can be chosen before read_aloud_standard_audio_import. Writes nothing. Takes the same `source_path` / `source_base64` + `file_name` pair as the import tool.")]
    async fn read_aloud_standard_audio_probe(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioProbeParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] read_aloud_standard_audio_probe");
        let probe = datasets::read_aloud::standard_audio::probe(
            param.source_path.as_deref(),
            param.source_base64.as_deref(),
            param.file_name.as_deref(),
        )?;
        Ok(serde_json::to_string_pretty(&probe).unwrap_or_default())
    }

#[tool(name = "read_aloud_standard_audio_guess", description = "Find where a text is spoken inside a longer recording, by transcribing the file with the loaded STT model and aligning what it heard to the text. Returns `start` and `end` (seconds, padded and clamped to the file), `matched_words` / `reference_words` / `confidence`, and the `transcript` of the whole file. Writes nothing — feed the two edges to read_aloud_standard_audio_import. Only a model that reports word timings can do this (Parakeet); with any other model the call fails with an actionable message. The whole file is transcribed, so it costs more the longer the file is. Takes the same `source_path` / `source_base64` pair as read_aloud_standard_audio_probe. When it refuses (too few matched words, or the text's start or end was not heard), pick the range by hand instead.")]
async fn read_aloud_standard_audio_guess(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioGuessParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] read_aloud_standard_audio_guess: text={}",
            param.text_uuid
        );
        let settings = self.app.state::<SettingsState>();
        let model_state = self.app.state::<crate::models::ModelState>();
        let guess = datasets::read_aloud::standard_audio::guess_span(
            model_state.inner(),
            settings.inner(),
            &param.dataset_uuid,
            &param.text_uuid,
            param.source_path.as_deref(),
            param.source_base64.as_deref(),
        )?;
        Ok(serde_json::to_string_pretty(&guess).unwrap_or_default())
    }

    #[tool(name = "read_aloud_standard_audio_set_default", description = "Make one clip the default that pronunciation is measured against. Exactly one clip stays default per text.")]
    async fn read_aloud_standard_audio_set_default(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioUuidParam>,
    ) -> Result<String, String> {
        log::info!("[MCP] read_aloud_standard_audio_set_default: {}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::standard_audio::set_default(
            settings.inner(),
            &param.dataset_uuid,
            &param.uuid,
        )?;
        Ok(serde_json::json!({"status": "ok", "message": "Default standard audio set"}).to_string())
    }

    #[tool(name = "read_aloud_standard_audio_delete", description = "Delete one standard-audio clip: its row and its wav. If it was the default, the earliest remaining clip is promoted, so a text with clips always has a default. Deleting a text deletes its clips too.")]
    async fn read_aloud_standard_audio_delete(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioUuidParam>,
    ) -> Result<String, String> {
        log::warn!("[MCP] read_aloud_standard_audio_delete: {}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::standard_audio::delete(
            settings.inner(),
            &param.dataset_uuid,
            &param.uuid,
        )?;
        Ok(serde_json::json!({"status": "ok", "message": "Standard audio deleted"}).to_string())
    }
}
