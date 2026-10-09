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
struct ReadAloudScoreParam {
    content: String,
    recognized: String,
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

    #[tool(name = "read_aloud_list_texts", description = "List all texts in a read-aloud dataset, ordered, with best score and attempt count.")]
    async fn read_aloud_list_texts(&self, Parameters(param): Parameters<ReadAloudDatasetParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let items = datasets::read_aloud::read_aloud_list_texts(settings, param.dataset_uuid).await?;
        Ok(serde_json::to_string_pretty(&items).unwrap_or_default())
    }

    #[tool(name = "read_aloud_save_text", description = "Create or update a text. Pass text as JSON with fields: uuid, dataset_uuid, order_num, title, note, content. The note holds source/location info.")]
    async fn read_aloud_save_text(&self, Parameters(param): Parameters<ReadAloudTextParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        let text: datasets::read_aloud::ReadText = serde_json::from_value(param.text.into())
            .map_err(|e| format!("Invalid text JSON: {}", e))?;
        datasets::read_aloud::read_aloud_save_text(settings, param.dataset_uuid, text).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Text saved"}).to_string())
    }

    #[tool(name = "read_aloud_delete_text", description = "Delete a text and all its recorded attempts and audio.")]
    async fn read_aloud_delete_text(&self, Parameters(param): Parameters<ReadAloudDeleteTextParam>) -> Result<String, String> {
        let settings = self.app.state::<SettingsState>();
        datasets::read_aloud::read_aloud_delete_text(settings, param.dataset_uuid, param.uuid).await?;
        Ok(serde_json::json!({"status": "ok", "message": "Text deleted"}).to_string())
    }

    #[tool(name = "read_aloud_list_attempts", description = "List all recorded attempts for a text (newest first) with score, transcript, and resolved audio URL.")]
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

    #[tool(name = "read_aloud_score", description = "Compute the similarity score (0-100) between a reference text and a recognized transcript, without persisting anything. Score >= 60 passes.")]
    async fn read_aloud_score(&self, Parameters(param): Parameters<ReadAloudScoreParam>) -> Result<String, String> {
        let score = datasets::read_aloud::read_aloud_score(param.content, param.recognized).await?;
        Ok(serde_json::json!({"score": score, "passed": score >= 60.0}).to_string())
    }

    #[tool(name = "read_aloud_submit", description = "Submit a reading attempt: stores the (optional) recording, scores the recognized transcript against the reference text, saves the attempt, and awards scaled XP (1 XP at score >= 60, +1 more at >= 80). Returns the score, saved attempt, and XP awarded.")]
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
        )
        .await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }
}
