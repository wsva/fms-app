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

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudCreateParam {
    name: String,
    #[serde(default)]
    description: String,
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
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudDatasetParam {
    dataset_uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudTextParam {
    dataset_uuid: String,
    text: JsonValue,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudDeleteTextParam {
    dataset_uuid: String,
    uuid: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
struct ReadAloudListAttemptsParam {
    dataset_uuid: String,
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

    #[tool(name = "read_aloud_list", description = "List all read-aloud datasets. Returns UUID, name, description, path, and timestamps.")]
    async fn read_aloud_list(&self) -> Result<String, String> {
        log::info!("[MCP] read_aloud_list");
        let settings = self.app.state::<SettingsState>();
        let items = datasets::read_aloud::read_aloud_list(settings).await?;
        Ok(serde_json::to_string_pretty(&items).unwrap_or_default())
    }

    #[tool(name = "read_aloud_create", description = "Create a new read-aloud dataset (a collection of texts to practise reading aloud). Returns the dataset metadata with UUID.")]
    async fn read_aloud_create(&self, Parameters(param): Parameters<ReadAloudCreateParam>) -> Result<String, String> {
        log::info!("[MCP] read_aloud_create: name={}", param.name);
        let settings = self.app.state::<SettingsState>();
        let result = datasets::read_aloud::read_aloud_create(settings, param.name, Some(param.description)).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }

    #[tool(name = "read_aloud_update", description = "Update a read-aloud dataset's name and/or description by UUID.")]
    async fn read_aloud_update(&self, Parameters(param): Parameters<ReadAloudUpdateParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::read_aloud_update(settings, param.uuid, param.name, param.description).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset updated"}).to_string())
    }

    #[tool(name = "read_aloud_delete", description = "Delete a read-aloud dataset by moving its whole directory (texts, attempts, recordings) into the app trash folder — not permanently removed, so this is recoverable. Returns `trashed_to`.")]
    async fn read_aloud_delete(&self, Parameters(param): Parameters<ReadAloudUuidParam>) -> Result<String, String> {
        log::warn!("[MCP] read_aloud_delete: uuid={}", param.uuid);
        let settings = self.app.state::<SettingsState>();
        let trashed_to = datasets::read_aloud::read_aloud_delete(settings, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Dataset moved to trash", "trashed_to": trashed_to}).to_string())
    }

    #[tool(name = "read_aloud_list_texts", description = "List all texts in a read-aloud dataset, ordered, with best score and attempt count. normalized_content (when set) is the LLM-normalized reference used for scoring.")]
    async fn read_aloud_list_texts(&self, Parameters(param): Parameters<ReadAloudDatasetParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let items = datasets::read_aloud::read_aloud_list_texts(settings, param.dataset_uuid).await?;
        Ok(serde_json::to_string_pretty(&items).unwrap_or_default())
    }

    #[tool(name = "read_aloud_save_text", description = "Create or update a text. Pass text as JSON with fields: uuid, dataset_uuid, order_num, title, note, content, and optionally normalized_content. The note holds source/location info. normalized_content is a derived, LLM-normalized reference for scoring (numbers/abbreviations spelled out to match STT output); it is null until produced, and editing content resets it to null automatically.")]
    async fn read_aloud_save_text(&self, Parameters(param): Parameters<ReadAloudTextParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let text: datasets::read_aloud::ReadText = serde_json::from_value(param.text.into())
            .map_err(|e| format!("Invalid text JSON: {}", e))?;
        datasets::read_aloud::read_aloud_save_text(settings, param.dataset_uuid, text).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Text saved"}).to_string())
    }

    #[tool(name = "read_aloud_delete_text", description = "Delete a text. Per-user attempt history is left untouched in the app DB (a content edit never removes practice data).")]
    async fn read_aloud_delete_text(&self, Parameters(param): Parameters<ReadAloudDeleteTextParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::read_aloud_delete_text(settings, param.dataset_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Text deleted"}).to_string())
    }

    #[tool(name = "read_aloud_list_attempts", description = "List all recorded attempts for a text (newest first) with score, transcript, resolved audio URL, and the stored assessment (`metrics`: accuracy counts, fluency, pronunciation, weakest words, pauses) when one was computed. Metrics are absent for takes whose recording is gone or that predate assessment.")]
    async fn read_aloud_list_attempts(&self, Parameters(param): Parameters<ReadAloudListAttemptsParam>) -> Result<String, String> {
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

    #[tool(name = "read_aloud_assess", description = "Score an attempt across all three axes without persisting anything: accuracy (the WIP 0-100 score, identical to read_aloud_score), fluency (words/min, articulation rate, pauses, longest run — needs `words` timings from model_transcribe_detailed and the WAV), and pronunciation (CTC phoneme GOP against the text's standard audio — the dataset-owned reference clip, never a network call). Each axis degrades honestly: a missing phoneme model, an absent standard clip or an attempt too short to align reports `unavailable_reason` (no_reference / stale_reference / model_missing / too_short) instead of a fabricated score. Use this to preview a result before read_aloud_submit; call read_aloud_standard_audio_list first when no_reference comes back.")]
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
        Ok(serde_json::to_string_pretty(&assessment).unwrap_or_default())
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

    #[tool(name = "read_aloud_standard_audio_generate", description = "Synthesise a text with Edge TTS and store the result as one of its standard-audio clips — the reference pronunciation is measured against. `voice` defaults to the voice configured in settings. The first clip of a text becomes its default and takes effect immediately; a text with no clip cannot have its pronunciation scored, so call this after editing a text or when read_aloud_standard_audio_list reports `stale`.")]
    async fn read_aloud_standard_audio_generate(
        &self,
        Parameters(param): Parameters<ReadAloudStandardAudioGenerateParam>,
    ) -> Result<String, String> {
        log::info!(
            "[MCP] read_aloud_standard_audio_generate: text={}",
            param.text_uuid
        );
        let settings = self.app.state::<SettingsState>();
        let voice = param
            .voice
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| datasets::read_aloud::tts_voice(settings.inner()));
        let clip = datasets::read_aloud::standard_audio::generate(
            settings.inner(),
            &param.dataset_uuid,
            &param.text_uuid,
            &voice,
            param.note.as_deref(),
        )
        .await?;
        Ok(serde_json::to_string_pretty(&clip).unwrap_or_default())
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
