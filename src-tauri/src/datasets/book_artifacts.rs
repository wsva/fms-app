//! Understanding layer for book datasets: comprehension *artifacts* (explanation,
//! simplification, digest, argument map, quiz, note, assessment), a per-book
//! *glossary* of recurring concepts, and the quiz -> XP feedback loop.
//!
//! Every artifact is attached to a **scope** — a paragraph (its `paragraph_break`
//! row uuid), a chapter (chapter uuid) or a single sentence — and stored in the
//! book's own `data.sqlite3`. Generation is either built-in (over [`llm_complete`])
//! or done by a third-party agent through the MCP tools in `mcp/books.rs`, which
//! write the very same rows. Staleness follows the read-aloud `normalized_content`
//! rule: an artifact stores a hash of the source text it was generated from, so a
//! later edit *flags* it stale instead of deleting it. Notes are user-authored and
//! never stale.
//!
//! The schema (tables `book_artifact`, `book_term`, `book_quiz_result`) lives in
//! [`super::book::open_book_db`]; this module owns the reads/writes/generation.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Emitter, State};
use uuid::Uuid;

use super::book::{
    find_book_dir, find_book_dir_write, open_book_db, save_sentence_row, AudioWriteResult,
    BookSentence,
};
use super::now_stamp;
use crate::ai::llm::{llm_complete, ChatMessage};
use crate::settings::SettingsState;
use crate::sync::change_log::commit_change;
use crate::xp::{xp_award_internal, XpAwardResult};

// ============================================================
// Types
// ============================================================

/// A persisted understanding artifact. Mirrors the `book_artifact` table.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookArtifact {
    pub uuid: String,
    pub book_uuid: String,
    /// `paragraph` | `chapter` | `sentence`.
    pub scope_type: String,
    /// break-row uuid / chapter uuid / sentence uuid.
    pub scope_uuid: String,
    /// `explanation`|`simplification`|`digest`|`argument_map`|`questions`|`note`|`assessment`.
    pub kind: String,
    /// Markdown; for `questions` a JSON array of `{type,stem,choices?,correct?,model_answer?,explanation}`.
    pub content: String,
    /// `ollama` | `agent` | `user`.
    pub source: String,
    #[serde(default)]
    pub model: Option<String>,
    /// assessment -> the note it assesses.
    #[serde(default)]
    pub parent_uuid: Option<String>,
    /// FNV-1a 64-bit hex of the source text at generation time ('' for notes).
    #[serde(default)]
    pub content_hash: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A [`BookArtifact`] plus the computed staleness flag returned to readers.
#[derive(Clone, Debug, Serialize)]
pub struct BookArtifactView {
    #[serde(flatten)]
    pub artifact: BookArtifact,
    /// True when a generated artifact's source text has changed since it was made.
    pub stale: bool,
}

/// A glossary entry. Mirrors the `book_term` table.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookTerm {
    pub uuid: String,
    pub book_uuid: String,
    pub term: String,
    pub definition: String,
    /// `ollama` | `agent` | `user`.
    pub source: String,
    #[serde(default)]
    pub model: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Response of [`book_record_quiz_result`]: the stored attempt, the learner's best
/// score for the quiz, and the XP award outcome.
#[derive(Clone, Debug, Serialize)]
pub struct QuizResultResponse {
    pub uuid: String,
    pub best_correct: i64,
    pub best_total: i64,
    pub xp: XpAwardResult,
}

/// Kinds produced by [`generate_artifact`] (regenerated in place, hash-tracked).
const GENERATED_KINDS: &[&str] = &[
    "explanation",
    "simplification",
    "digest",
    "argument_map",
    "questions",
];

/// Kinds that are user-authored or note-derived and therefore never stale.
fn is_never_stale(kind: &str) -> bool {
    matches!(kind, "note" | "assessment")
}

// ============================================================
// Source text + hashing
// ============================================================

/// FNV-1a 64-bit, hex-encoded. Hand-rolled so no new dependency is added; only
/// used to detect that a scope's text changed, never as a security hash.
pub(crate) fn hash_text(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)
}

/// All sentences of a chapter in order: `(uuid, order_num, content, sentence_type)`.
fn chapter_rows(conn: &Connection, chapter_uuid: &str) -> Result<Vec<(String, i64, String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT uuid, order_num, content, sentence_type FROM book_sentence \
             WHERE chapter_uuid = ?1 ORDER BY order_num ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([chapter_uuid], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// The text a paragraph anchor covers: the `text` sentences after the previous
/// break up to (excluding) this one.
fn paragraph_text(rows: &[(String, i64, String, String)], break_uuid: &str) -> String {
    let idx = match rows.iter().position(|r| r.0 == break_uuid) {
        Some(i) => i,
        None => return String::new(),
    };
    let mut collected: Vec<&str> = Vec::new();
    for row in rows[..idx].iter().rev() {
        if row.3 == "paragraph_break" {
            break;
        }
        if row.3 == "text" && !row.2.trim().is_empty() {
            collected.push(row.2.as_str());
        }
    }
    collected.reverse();
    collected.join(" ")
}

/// The whole chapter as paragraphs separated by blank lines.
fn chapter_text(rows: &[(String, i64, String, String)]) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for row in rows {
        if row.3 == "paragraph_break" {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else if row.3 == "text" && !row.2.trim().is_empty() {
            current.push(row.2.as_str());
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs.join("\n\n")
}

/// Resolve the source text an artifact of `scope_type`/`scope_uuid` was made from.
fn source_text(conn: &Connection, scope_type: &str, scope_uuid: &str) -> Result<String, String> {
    match scope_type {
        "chapter" => {
            let rows = chapter_rows(conn, scope_uuid)?;
            Ok(chapter_text(&rows))
        }
        "paragraph" => {
            let chapter_uuid: String = conn
                .query_row(
                    "SELECT chapter_uuid FROM book_sentence WHERE uuid = ?1",
                    [scope_uuid],
                    |row| row.get(0),
                )
                .map_err(|_| format!("paragraph anchor {} not found", scope_uuid))?;
            let rows = chapter_rows(conn, &chapter_uuid)?;
            Ok(paragraph_text(&rows, scope_uuid))
        }
        "sentence" => conn
            .query_row(
                "SELECT content FROM book_sentence WHERE uuid = ?1",
                [scope_uuid],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| e.to_string()),
        other => Err(format!("unknown scope_type '{}'", other)),
    }
}

// ============================================================
// Artifact row helpers
// ============================================================

fn save_artifact_row(conn: &Connection, a: &BookArtifact) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO book_artifact \
         (uuid, book_uuid, scope_type, scope_uuid, kind, content, source, model, \
          parent_uuid, content_hash, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        rusqlite::params![
            a.uuid,
            a.book_uuid,
            a.scope_type,
            a.scope_uuid,
            a.kind,
            a.content,
            a.source,
            a.model,
            a.parent_uuid,
            a.content_hash,
            a.created_at,
            a.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn row_to_artifact(row: &rusqlite::Row) -> rusqlite::Result<BookArtifact> {
    Ok(BookArtifact {
        uuid: row.get(0)?,
        book_uuid: row.get(1)?,
        scope_type: row.get(2)?,
        scope_uuid: row.get(3)?,
        kind: row.get(4)?,
        content: row.get(5)?,
        source: row.get(6)?,
        model: row.get(7)?,
        parent_uuid: row.get(8)?,
        content_hash: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const ARTIFACT_COLS: &str = "uuid, book_uuid, scope_type, scope_uuid, kind, content, source, \
     model, parent_uuid, content_hash, created_at, updated_at";

fn query_artifact(conn: &Connection, uuid: &str) -> Result<BookArtifact, String> {
    conn.query_row(
        &format!("SELECT {} FROM book_artifact WHERE uuid = ?1", ARTIFACT_COLS),
        [uuid],
        row_to_artifact,
    )
    .map_err(|e| e.to_string())
}

/// The uuid of an existing generated artifact of the same scope+kind, so a
/// regenerate replaces it in place instead of piling up duplicates.
fn existing_generated_uuid(conn: &Connection, scope_uuid: &str, kind: &str) -> Option<String> {
    conn.query_row(
        "SELECT uuid FROM book_artifact WHERE scope_uuid = ?1 AND kind = ?2 \
         AND parent_uuid IS NULL LIMIT 1",
        rusqlite::params![scope_uuid, kind],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Extract the first JSON array from an LLM response, tolerating ```json fences
/// and surrounding prose.
fn extract_json_array(text: &str) -> Option<Vec<Value>> {
    let trimmed = text.trim();
    let start = trimmed.find('[')?;
    let end = trimmed.rfind(']')?;
    if end <= start {
        return None;
    }
    let slice = &trimmed[start..=end];
    serde_json::from_str::<Vec<Value>>(slice).ok()
}

// ============================================================
// Prompts
// ============================================================

fn system_for_kind(kind: &str) -> &'static str {
    match kind {
        "explanation" => "You are a patient tutor helping a language learner understand a difficult \
            passage (often philosophy). Explain the passage's meaning, its key concepts and any \
            tricky references, in clear Markdown. Reply in the same language as the passage unless \
            told otherwise. Be concise but complete.",
        "simplification" => "You rewrite a difficult passage in simpler language for a language \
            learner, preserving the original meaning and argument. Keep the same language as the \
            passage. Return only the rewritten text in Markdown, no preamble.",
        "digest" => "You summarise a chapter for a language learner. For argumentative or \
            philosophical texts produce a PREMISE OUTLINE (thesis -> supporting premises -> \
            conclusions) rather than prose. Keep the same language as the text. Return Markdown.",
        "argument_map" => "You analyse the argument structure of a passage. Extract, in Markdown: \
            the central Claim, the Premises supporting it, any Objection considered, and the \
            Reply. Keep the same language as the passage. Use short labelled bullets.",
        "questions" => "You write comprehension questions for a language learner about a passage. \
            Return ONLY a JSON array (no prose, no code fences). Mix multiple-choice and open \
            questions. Each element is an object: for multiple-choice \
            {\"type\":\"mcq\",\"stem\":string,\"choices\":[4 strings],\"correct\":integer index,\
            \"explanation\":string}; for open {\"type\":\"open\",\"stem\":string,\
            \"model_answer\":string,\"explanation\":string}. Write questions in the language of \
            the passage.",
        _ => "You are a helpful assistant. Reply in Markdown.",
    }
}

// ============================================================
// Core generation (State-free)
// ============================================================

/// Generate (or regenerate) one artifact for a scope. Resolves the writable dir
/// (forking a downloaded base), builds the source text, prompts the LLM, validates
/// `questions`, stores the row and journals it.
pub(crate) async fn generate_artifact(
    settings: &SettingsState,
    book_uuid: &str,
    scope_type: &str,
    scope_uuid: &str,
    kind: &str,
    model: Option<String>,
    instructions: Option<String>,
) -> Result<BookArtifact, String> {
    if !GENERATED_KINDS.contains(&kind) {
        return Err(format!(
            "kind '{}' cannot be generated (generatable: {})",
            kind,
            GENERATED_KINDS.join(", ")
        ));
    }
    let dir = find_book_dir_write(settings, book_uuid)?;
    let conn = open_book_db(&dir)?;
    let source = source_text(&conn, scope_type, scope_uuid)?;
    if source.trim().is_empty() {
        return Err("the selected scope has no text to work from".into());
    }

    let mut user = format!("Passage:\n\n{}", source);
    if let Some(extra) = instructions.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        user.push_str(&format!("\n\nAdditional instructions: {}", extra));
    }
    let messages = vec![
        ChatMessage { role: "system".into(), content: system_for_kind(kind).into() },
        ChatMessage { role: "user".into(), content: user },
    ];
    let resp = llm_complete(settings, model.clone(), messages, 0.4).await?;

    let mut content = resp.content.trim().to_string();
    if kind == "questions" {
        let arr = extract_json_array(&content).ok_or_else(|| {
            "the model did not return a valid JSON array of questions — try again or pick a \
             stronger model"
                .to_string()
        })?;
        if arr.is_empty() {
            return Err("the model returned an empty question list".into());
        }
        content = serde_json::to_string(&arr).map_err(|e| e.to_string())?;
    }

    let now = now_stamp();
    let uuid = existing_generated_uuid(&conn, scope_uuid, kind).unwrap_or_else(|| Uuid::new_v4().to_string());
    let created_at = conn
        .query_row(
            "SELECT created_at FROM book_artifact WHERE uuid = ?1",
            [&uuid],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| now.clone());
    let artifact = BookArtifact {
        uuid,
        book_uuid: book_uuid.to_string(),
        scope_type: scope_type.to_string(),
        scope_uuid: scope_uuid.to_string(),
        kind: kind.to_string(),
        content,
        source: "ollama".into(),
        model: model.filter(|m| !m.trim().is_empty()),
        parent_uuid: None,
        content_hash: hash_text(&source),
        created_at,
        updated_at: now,
    };
    save_artifact_row(&conn, &artifact)?;
    let _ = commit_change(
        settings,
        "book_artifact_save",
        book_uuid,
        &serde_json::to_value(&artifact).unwrap_or_default(),
    );
    Ok(artifact)
}

/// Assess a user note: prompts for feedback on the note against its source passage
/// and stores an `assessment` artifact linked to the note via `parent_uuid`.
pub(crate) async fn assess_note(
    settings: &SettingsState,
    book_uuid: &str,
    note_uuid: &str,
    model: Option<String>,
) -> Result<BookArtifact, String> {
    let dir = find_book_dir_write(settings, book_uuid)?;
    let conn = open_book_db(&dir)?;
    let note = query_artifact(&conn, note_uuid)?;
    if note.kind != "note" {
        return Err(format!("artifact {} is not a note", note_uuid));
    }
    let source = source_text(&conn, &note.scope_type, &note.scope_uuid).unwrap_or_default();
    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: "You assess a language learner's note about a passage. Judge whether their \
                understanding is correct, point out gaps or misconceptions, and suggest what to \
                revisit. Be encouraging and specific. Reply in Markdown in the language of the note."
                .into(),
        },
        ChatMessage {
            role: "user".into(),
            content: format!("Passage:\n\n{}\n\nLearner's note:\n\n{}", source, note.content),
        },
    ];
    let resp = llm_complete(settings, model.clone(), messages, 0.4).await?;
    let now = now_stamp();
    let uuid = conn
        .query_row(
            "SELECT uuid FROM book_artifact WHERE parent_uuid = ?1 AND kind = 'assessment' LIMIT 1",
            [note_uuid],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| Uuid::new_v4().to_string());
    let created_at = conn
        .query_row(
            "SELECT created_at FROM book_artifact WHERE uuid = ?1",
            [&uuid],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| now.clone());
    let artifact = BookArtifact {
        uuid,
        book_uuid: book_uuid.to_string(),
        scope_type: note.scope_type.clone(),
        scope_uuid: note.scope_uuid.clone(),
        kind: "assessment".into(),
        content: resp.content.trim().to_string(),
        source: "ollama".into(),
        model: model.filter(|m| !m.trim().is_empty()),
        parent_uuid: Some(note_uuid.to_string()),
        content_hash: note.content_hash.clone(),
        created_at,
        updated_at: now,
    };
    save_artifact_row(&conn, &artifact)?;
    let _ = commit_change(
        settings,
        "book_artifact_save",
        book_uuid,
        &serde_json::to_value(&artifact).unwrap_or_default(),
    );
    Ok(artifact)
}

/// Extract recurring concepts from a scope into the book glossary. A user-edited
/// definition (`source='user'`) is never overwritten; generated rows are refreshed.
pub(crate) async fn extract_terms(
    settings: &SettingsState,
    book_uuid: &str,
    scope_type: &str,
    scope_uuid: &str,
    model: Option<String>,
) -> Result<Vec<BookTerm>, String> {
    let dir = find_book_dir_write(settings, book_uuid)?;
    let conn = open_book_db(&dir)?;
    let source = source_text(&conn, scope_type, scope_uuid)?;
    if source.trim().is_empty() {
        return Err("the selected scope has no text to work from".into());
    }
    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: "You extract the key recurring concepts/terms from a passage (philosophical, \
                technical, or foreign-language) that a learner must know. Return ONLY a JSON array \
                (no prose, no code fences) of objects {\"term\":string,\"definition\":string}. The \
                definition is one or two sentences in the learner-friendly form, in the language of \
                the passage. At most 12 terms, most important first."
                .into(),
        },
        ChatMessage { role: "user".into(), content: source },
    ];
    let resp = llm_complete(settings, model.clone(), messages, 0.3).await?;
    let arr = extract_json_array(&resp.content).ok_or_else(|| {
        "the model did not return a valid JSON array of terms — try again or pick a stronger model"
            .to_string()
    })?;

    let now = now_stamp();
    let model_clean = model.filter(|m| !m.trim().is_empty());
    let mut out: Vec<BookTerm> = Vec::new();
    for item in arr {
        let term = item.get("term").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let definition = item.get("definition").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if term.is_empty() {
            continue;
        }
        // Existing row for (book, term)?
        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT uuid, source FROM book_term WHERE book_uuid = ?1 AND term = ?2 LIMIT 1",
                rusqlite::params![book_uuid, term],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .ok();
        let term_row = match existing {
            Some((uuid, src)) if src == "user" => {
                // Never overwrite a user-edited definition.
                BookTerm {
                    uuid,
                    book_uuid: book_uuid.to_string(),
                    term,
                    definition,
                    source: "user".into(),
                    model: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                }
            }
            Some((uuid, _)) => {
                let created_at = conn
                    .query_row(
                        "SELECT created_at FROM book_term WHERE uuid = ?1",
                        [&uuid],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap_or_else(|_| now.clone());
                BookTerm {
                    uuid,
                    book_uuid: book_uuid.to_string(),
                    term,
                    definition,
                    source: "ollama".into(),
                    model: model_clean.clone(),
                    created_at,
                    updated_at: now.clone(),
                }
            }
            None => BookTerm {
                uuid: Uuid::new_v4().to_string(),
                book_uuid: book_uuid.to_string(),
                term,
                definition,
                source: "ollama".into(),
                model: model_clean.clone(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        };
        // For the user-protected case we still return the *stored* definition.
        let stored = if term_row.source == "user" {
            query_term(&conn, &term_row.uuid).unwrap_or_else(|_| term_row.clone())
        } else {
            save_term_row(&conn, &term_row)?;
            let _ = commit_change(
                settings,
                "book_term_save",
                book_uuid,
                &serde_json::to_value(&term_row).unwrap_or_default(),
            );
            term_row.clone()
        };
        out.push(stored);
    }
    Ok(out)
}

fn save_term_row(conn: &Connection, t: &BookTerm) -> Result<(), String> {
    conn.execute(
        "INSERT INTO book_term (uuid, book_uuid, term, definition, source, model, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8) \
         ON CONFLICT(book_uuid, term) DO UPDATE SET \
           definition=excluded.definition, source=excluded.source, model=excluded.model, \
           updated_at=excluded.updated_at",
        rusqlite::params![
            t.uuid, t.book_uuid, t.term, t.definition, t.source, t.model, t.created_at, t.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn row_to_term(row: &rusqlite::Row) -> rusqlite::Result<BookTerm> {
    Ok(BookTerm {
        uuid: row.get(0)?,
        book_uuid: row.get(1)?,
        term: row.get(2)?,
        definition: row.get(3)?,
        source: row.get(4)?,
        model: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

const TERM_COLS: &str = "uuid, book_uuid, term, definition, source, model, created_at, updated_at";

fn query_term(conn: &Connection, uuid: &str) -> Result<BookTerm, String> {
    conn.query_row(
        &format!("SELECT {} FROM book_term WHERE uuid = ?1", TERM_COLS),
        [uuid],
        row_to_term,
    )
    .map_err(|e| e.to_string())
}

// ============================================================
// Artifact commands
// ============================================================

/// List every artifact whose scope lies in a chapter (the chapter itself, its
/// paragraph-break anchors and its sentences), each carrying a computed `stale`.
#[tauri::command]
pub async fn book_list_artifacts(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    chapter_uuid: String,
) -> Result<Vec<BookArtifactView>, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;

    // Scope uuids in this chapter: the chapter + all its sentences.
    let mut scope_uuids: Vec<String> = vec![chapter_uuid.clone()];
    {
        let mut stmt = conn
            .prepare("SELECT uuid FROM book_sentence WHERE chapter_uuid = ?1")
            .map_err(|e| e.to_string())?;
        let sentences: Vec<String> = stmt
            .query_map([&chapter_uuid], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        scope_uuids.extend(sentences);
    }

    let mut views: Vec<BookArtifactView> = Vec::new();
    for scope in &scope_uuids {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM book_artifact WHERE scope_uuid = ?1 ORDER BY created_at ASC",
                ARTIFACT_COLS
            ))
            .map_err(|e| e.to_string())?;
        let rows: Vec<BookArtifact> = stmt
            .query_map([scope], row_to_artifact)
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        for a in rows {
            let stale = if is_never_stale(&a.kind) || a.content_hash.is_empty() {
                false
            } else {
                let current = source_text(&conn, &a.scope_type, &a.scope_uuid)
                    .map(|t| hash_text(&t))
                    .unwrap_or_default();
                !current.is_empty() && current != a.content_hash
            };
            views.push(BookArtifactView { artifact: a, stale });
        }
    }
    Ok(views)
}

/// Write one artifact row, clear any matching agent job, and journal the change.
/// Shared by the [`book_save_artifact`] command (which also emits the refresh
/// event) and the sync replay path in `sync::client::round`, which has no
/// `AppHandle` to emit through — a pull refreshes the reader on its own.
pub(crate) fn save_artifact_inner(
    settings: &State<'_, SettingsState>,
    book_uuid: &str,
    artifact: BookArtifact,
) -> Result<(), String> {
    let dir = find_book_dir_write(settings, book_uuid)?;
    let conn = open_book_db(&dir)?;
    let mut artifact = artifact;
    artifact.book_uuid = book_uuid.to_string();
    if artifact.uuid.trim().is_empty() {
        artifact.uuid = Uuid::new_v4().to_string();
    }
    if artifact.created_at.trim().is_empty() {
        artifact.created_at = now_stamp();
    }
    artifact.updated_at = now_stamp();
    save_artifact_row(&conn, &artifact)?;
    // The artifact satisfies any pending agent request for the same scope+kind,
    // whatever produced it (an agent fulfilling a job, or the built-in generator
    // the user ran instead of waiting). Jobs are ephemeral, so it is deleted.
    let _ = conn.execute(
        "DELETE FROM book_artifact_job WHERE book_uuid = ?1 AND scope_uuid = ?2 AND kind = ?3",
        rusqlite::params![book_uuid, artifact.scope_uuid, artifact.kind],
    );
    let _ = commit_change(
        settings,
        "book_artifact_save",
        book_uuid,
        &serde_json::to_value(&artifact).unwrap_or_default(),
    );
    Ok(())
}

/// Insert or update one artifact (the agent/manual write path). Writing an
/// artifact also clears any outstanding agent job for the same scope + kind and
/// notifies the webview, so a result an attached agent delivers through this same
/// path shows up in the Understanding panel without the user reloading.
#[tauri::command]
pub async fn book_save_artifact(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    book_uuid: String,
    artifact: BookArtifact,
) -> Result<(), String> {
    save_artifact_inner(&settings, &book_uuid, artifact)?;
    let _ = app.emit(
        "book-artifact-changed",
        serde_json::json!({ "book_uuid": book_uuid }),
    );
    Ok(())
}

/// Delete one artifact.
#[tauri::command]
pub async fn book_delete_artifact(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute("DELETE FROM book_artifact WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    let _ = commit_change(
        &settings,
        "book_artifact_delete",
        &book_uuid,
        &serde_json::json!({ "uuid": uuid }),
    );
    Ok(())
}

// ============================================================
// Artifact job mailbox — delegate generation to an attached agent
// ============================================================
//
// FmS never dials an agent, so the Understand panel's buttons cannot *call* a
// third-party agent (WorkBuddy, goose, …) directly. Instead a button writes a job
// row here; an agent attached over MCP discovers it with `book_list_artifact_jobs`
// (which carries the source text), generates with its own model, and delivers
// through the very same [`book_save_artifact`] every other write path uses — which
// clears the job and notifies the panel, so the result lands in the same place a
// built-in generation would. Jobs are ephemeral coordination state: not journaled
// for sync, and deleted once fulfilled or cancelled.

/// A request for an agent to generate an artifact. Mirrors the
/// `book_artifact_job` table, enriched on read with the source text and a
/// staleness flag so an agent can act on it without a second round-trip.
#[derive(Clone, Debug, Serialize)]
pub struct ArtifactJob {
    pub uuid: String,
    pub book_uuid: String,
    /// `paragraph` | `chapter` | `sentence`.
    pub scope_type: String,
    pub scope_uuid: String,
    /// `explanation`|`simplification`|`digest`|`argument_map`|`questions`.
    pub kind: String,
    /// Free-form guidance captured at request time (e.g. output language).
    pub instructions: String,
    /// Model hint from the requester; the agent is free to ignore it.
    pub model: Option<String>,
    /// `pending` today; reserved for a future `claimed` without a migration.
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// The scope's text at read time, so the agent need not fetch it separately.
    pub source_text: String,
    /// True when the scope's text changed since the job was requested.
    pub stale: bool,
}

/// The stored row, before it is enriched with the live source text.
struct JobRow {
    uuid: String,
    book_uuid: String,
    scope_type: String,
    scope_uuid: String,
    kind: String,
    instructions: String,
    model: Option<String>,
    status: String,
    content_hash: String,
    created_at: String,
    updated_at: String,
}

const JOB_COLS: &str = "uuid, book_uuid, scope_type, scope_uuid, kind, instructions, \
     model, status, content_hash, created_at, updated_at";

fn row_to_job(row: &rusqlite::Row) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        uuid: row.get(0)?,
        book_uuid: row.get(1)?,
        scope_type: row.get(2)?,
        scope_uuid: row.get(3)?,
        kind: row.get(4)?,
        instructions: row.get(5)?,
        model: row.get(6)?,
        status: row.get(7)?,
        content_hash: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

/// Attach the live source text and a staleness flag to a stored row.
fn enrich_job(conn: &Connection, row: JobRow) -> ArtifactJob {
    let text = source_text(conn, &row.scope_type, &row.scope_uuid).unwrap_or_default();
    let stale = if row.content_hash.is_empty() || text.is_empty() {
        false
    } else {
        hash_text(&text) != row.content_hash
    };
    ArtifactJob {
        uuid: row.uuid,
        book_uuid: row.book_uuid,
        scope_type: row.scope_type,
        scope_uuid: row.scope_uuid,
        kind: row.kind,
        instructions: row.instructions,
        model: row.model,
        status: row.status,
        created_at: row.created_at,
        updated_at: row.updated_at,
        source_text: text,
        stale,
    }
}

/// Request an agent-generated artifact for a scope. Idempotent per (scope, kind):
/// a second request while one is still open returns the existing job rather than
/// queueing a duplicate.
#[tauri::command]
pub async fn book_request_artifact(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    book_uuid: String,
    scope_type: String,
    scope_uuid: String,
    kind: String,
    instructions: Option<String>,
    model: Option<String>,
) -> Result<ArtifactJob, String> {
    if !GENERATED_KINDS.contains(&kind.as_str()) {
        return Err(format!(
            "kind '{}' cannot be requested from an agent. Valid kinds: {}",
            kind,
            GENERATED_KINDS.join(", ")
        ));
    }
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let text = source_text(&conn, &scope_type, &scope_uuid)?;
    if text.trim().is_empty() {
        return Err("this scope has no text to generate from".into());
    }

    // Reuse an outstanding job for the same scope+kind instead of queueing twice.
    let existing: Option<JobRow> = conn
        .query_row(
            &format!(
                "SELECT {JOB_COLS} FROM book_artifact_job \
                 WHERE book_uuid = ?1 AND scope_uuid = ?2 AND kind = ?3 LIMIT 1"
            ),
            rusqlite::params![book_uuid, scope_uuid, kind],
            row_to_job,
        )
        .ok();
    if let Some(row) = existing {
        return Ok(enrich_job(&conn, row));
    }

    let uuid = Uuid::new_v4().to_string();
    let now = now_stamp();
    conn.execute(
        "INSERT INTO book_artifact_job \
         (uuid, book_uuid, scope_type, scope_uuid, kind, instructions, model, status, \
          content_hash, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,'pending',?8,?9,?9)",
        rusqlite::params![
            uuid,
            book_uuid,
            scope_type,
            scope_uuid,
            kind,
            instructions.unwrap_or_default(),
            model,
            hash_text(&text),
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    let row = conn
        .query_row(
            &format!("SELECT {JOB_COLS} FROM book_artifact_job WHERE uuid = ?1"),
            [&uuid],
            row_to_job,
        )
        .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "book-artifact-changed",
        serde_json::json!({ "book_uuid": book_uuid }),
    );
    Ok(enrich_job(&conn, row))
}

/// List a book's artifact jobs. `status` filters to one state; omit it for every
/// outstanding request, oldest first.
#[tauri::command]
pub async fn book_list_artifact_jobs(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    status: Option<String>,
) -> Result<Vec<ArtifactJob>, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let filter = status.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (sql, params): (String, Vec<String>) = match filter {
        Some(s) => (
            format!(
                "SELECT {JOB_COLS} FROM book_artifact_job WHERE book_uuid = ?1 AND status = ?2 \
                 ORDER BY created_at ASC"
            ),
            vec![book_uuid.clone(), s.to_string()],
        ),
        None => (
            format!(
                "SELECT {JOB_COLS} FROM book_artifact_job WHERE book_uuid = ?1 \
                 ORDER BY created_at ASC"
            ),
            vec![book_uuid.clone()],
        ),
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows: Vec<JobRow> = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), row_to_job)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows.into_iter().map(|r| enrich_job(&conn, r)).collect())
}

/// Cancel an outstanding job (the requester changed their mind, or generated it
/// locally instead). Deletes the row; jobs are ephemeral, so there is nothing to
/// undo and nothing to sync.
#[tauri::command]
pub async fn book_cancel_artifact_job(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute(
        "DELETE FROM book_artifact_job WHERE uuid = ?1 AND book_uuid = ?2",
        rusqlite::params![uuid, book_uuid],
    )
    .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "book-artifact-changed",
        serde_json::json!({ "book_uuid": book_uuid }),
    );
    Ok(())
}

/// Built-in generation entry (explain / simplify / digest / argument_map / questions).
#[tauri::command]
pub async fn book_generate_artifact(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    scope_type: String,
    scope_uuid: String,
    kind: String,
    model: Option<String>,
    instructions: Option<String>,
) -> Result<BookArtifactView, String> {
    let artifact = generate_artifact(
        &settings,
        &book_uuid,
        &scope_type,
        &scope_uuid,
        &kind,
        model,
        instructions,
    )
    .await?;
    Ok(BookArtifactView { artifact, stale: false })
}

/// Assess a user note, storing the feedback as a linked `assessment` artifact.
#[tauri::command]
pub async fn book_assess_note(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    note_uuid: String,
    model: Option<String>,
) -> Result<BookArtifactView, String> {
    let artifact = assess_note(&settings, &book_uuid, &note_uuid, model).await?;
    Ok(BookArtifactView { artifact, stale: false })
}

/// Synthesise a paragraph's text with Edge TTS into the book's `media/` and attach
/// it to the paragraph's break row (reusing the existing playback path).
#[tauri::command]
pub async fn book_paragraph_listen(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    paragraph_uuid: String,
    voice: Option<String>,
) -> Result<AudioWriteResult, String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let text = source_text(&conn, "paragraph", &paragraph_uuid)?;
    if text.trim().is_empty() {
        return Err("this paragraph has no text to read aloud".into());
    }

    let voice = voice
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let configured = settings.settings.lock().unwrap().read_aloud_tts_voice.clone();
            if configured.trim().is_empty() { None } else { Some(configured) }
        })
        .unwrap_or_else(|| crate::datasets::read_aloud::DEFAULT_TTS_VOICE.to_string());

    let bytes = crate::edge_tts::synthesize_to_bytes(&text, &voice, None, None, None).await?;
    let media_dir = dir.join("media");
    std::fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;
    let file_name = format!("{}_tts.mp3", paragraph_uuid);
    std::fs::write(media_dir.join(&file_name), &bytes).map_err(|e| e.to_string())?;
    let rel_path = format!("media/{}", file_name);
    let abs_path = dir.join(&rel_path).to_string_lossy().into_owned();

    // Attach to the break row and journal as a normal sentence save.
    let mut sentence = query_sentence(&conn, &paragraph_uuid)?;
    sentence.audio_path = Some(rel_path.clone());
    sentence.updated_at = now_stamp();
    save_sentence_row(&conn, &sentence)?;
    let _ = commit_change(
        &settings,
        "book_sentence_save",
        &book_uuid,
        &serde_json::to_value(&sentence).unwrap_or_default(),
    );

    Ok(AudioWriteResult { name: file_name, rel_path, abs_path })
}

fn query_sentence(conn: &Connection, uuid: &str) -> Result<BookSentence, String> {
    conn.query_row(
        "SELECT uuid, chapter_uuid, user_id, order_num, content, sentence_type, \
         audio_path, recognized, bg_color, created_at, updated_at \
         FROM book_sentence WHERE uuid = ?1",
        [uuid],
        |row| {
            let audio_path: Option<String> = row.get(6)?;
            Ok(BookSentence {
                uuid: row.get(0)?,
                chapter_uuid: row.get(1)?,
                user_id: row.get(2)?,
                order_num: row.get(3)?,
                content: row.get(4)?,
                sentence_type: row.get(5)?,
                audio_url: None,
                audio_path,
                recognized: row.get(7)?,
                bg_color: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

// ============================================================
// Glossary commands
// ============================================================

#[tauri::command]
pub async fn book_list_terms(
    settings: State<'_, SettingsState>,
    book_uuid: String,
) -> Result<Vec<BookTerm>, String> {
    let dir = find_book_dir(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM book_term WHERE book_uuid = ?1 ORDER BY term COLLATE NOCASE ASC",
            TERM_COLS
        ))
        .map_err(|e| e.to_string())?;
    let items = stmt
        .query_map([&book_uuid], row_to_term)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

#[tauri::command]
pub async fn book_save_term(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    term: BookTerm,
) -> Result<BookTerm, String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let mut term = term;
    term.book_uuid = book_uuid.clone();
    if term.uuid.trim().is_empty() {
        term.uuid = Uuid::new_v4().to_string();
    }
    if term.term.trim().is_empty() {
        return Err("term text is required".into());
    }
    if term.created_at.trim().is_empty() {
        term.created_at = now_stamp();
    }
    term.updated_at = now_stamp();
    save_term_row(&conn, &term)?;
    // Re-read so an ON CONFLICT upsert returns the stored row (its real uuid).
    let stored = conn
        .query_row(
            &format!("SELECT {} FROM book_term WHERE book_uuid = ?1 AND term = ?2", TERM_COLS),
            rusqlite::params![book_uuid, term.term],
            row_to_term,
        )
        .unwrap_or(term.clone());
    let _ = commit_change(
        &settings,
        "book_term_save",
        &book_uuid,
        &serde_json::to_value(&stored).unwrap_or_default(),
    );
    Ok(stored)
}

#[tauri::command]
pub async fn book_delete_term(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    uuid: String,
) -> Result<(), String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute("DELETE FROM book_term WHERE uuid = ?1", [&uuid])
        .map_err(|e| e.to_string())?;
    let _ = commit_change(
        &settings,
        "book_term_delete",
        &book_uuid,
        &serde_json::json!({ "uuid": uuid }),
    );
    Ok(())
}

#[tauri::command]
pub async fn book_extract_terms(
    settings: State<'_, SettingsState>,
    book_uuid: String,
    scope_type: String,
    scope_uuid: String,
    model: Option<String>,
) -> Result<Vec<BookTerm>, String> {
    extract_terms(&settings, &book_uuid, &scope_type, &scope_uuid, model).await
}

// ============================================================
// Feedback loop: quiz -> XP -> (card promotion is frontend-side)
// ============================================================

/// Record a completed quiz attempt: store the score row (journaled with the book)
/// and award XP once per quiz artifact via the shared XP ledger.
#[tauri::command]
pub async fn book_record_quiz_result(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    book_uuid: String,
    artifact_uuid: String,
    total: i64,
    correct: i64,
) -> Result<QuizResultResponse, String> {
    let dir = find_book_dir_write(&settings, &book_uuid)?;
    let conn = open_book_db(&dir)?;
    let user_id = crate::auth::workspace_identity(&settings);
    let uuid = Uuid::new_v4().to_string();
    let now = now_stamp();
    conn.execute(
        "INSERT INTO book_quiz_result (uuid, book_uuid, artifact_uuid, user_id, total, correct, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![uuid, book_uuid, artifact_uuid, user_id, total, correct, now],
    )
    .map_err(|e| e.to_string())?;
    let _ = commit_change(
        &settings,
        "book_quiz_result_save",
        &book_uuid,
        &serde_json::json!({
            "uuid": uuid, "book_uuid": book_uuid, "artifact_uuid": artifact_uuid,
            "user_id": user_id, "total": total, "correct": correct, "created_at": now,
        }),
    );

    let (best_correct, best_total): (i64, i64) = conn
        .query_row(
            "SELECT correct, total FROM book_quiz_result WHERE artifact_uuid = ?1 \
             ORDER BY correct DESC, created_at DESC LIMIT 1",
            [&artifact_uuid],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap_or((correct, total));

    // XP: awarded once per quiz artifact (deduped on reference_id = artifact uuid),
    // amount = the best score so far.
    let xp = xp_award_internal(
        &settings,
        &user_id,
        best_correct.max(0),
        "book_quiz",
        &artifact_uuid,
        Some(book_uuid.as_str()),
    )?;
    if xp.is_new {
        let _ = app.emit("xp-earned", &xp);
    }
    Ok(QuizResultResponse { uuid, best_correct, best_total, xp })
}

/// Replay a quiz-result row from the sync journal without re-awarding XP (XP is
/// journaled separately under `@app`, so replaying it here would double-count).
pub(crate) fn apply_quiz_result_row(
    settings: &SettingsState,
    book_uuid: &str,
    payload: &Value,
) -> Result<(), String> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let get_i = |k: &str| payload.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    let uuid = get("uuid");
    if uuid.is_empty() {
        return Err("quiz result row has no uuid".into());
    }
    let dir = find_book_dir_write(settings, book_uuid)?;
    let conn = open_book_db(&dir)?;
    conn.execute(
        "INSERT OR REPLACE INTO book_quiz_result \
         (uuid, book_uuid, artifact_uuid, user_id, total, correct, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![
            uuid,
            book_uuid,
            get("artifact_uuid"),
            get("user_id"),
            get_i("total"),
            get_i("correct"),
            get("created_at"),
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
