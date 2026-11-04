//! The wiki's translate road: the app asks, an agent does the work, and the
//! file itself is the answer.
//!
//! A document translation is a *procedure*, not a completion — read the doc,
//! split it into blocks, decide what must not be translated, write a sibling
//! file, check it landed. That is agent-shaped work, so the wiki has **one**
//! road where the books have two ([`super::book_mail`]): no built-in LLM call,
//! no chunking, no markdown parser in Rust. An agent splits blocks; the app
//! never has to.
//!
//! Three decisions make this file differ from the books' artifact road, and all
//! three come from the same question — *what does the app actually need to
//! know?*
//!
//! - **The mailbox as a message centre, not a lock.** No claim is required, so
//!   no 30-minute hold cap sits under a job that can run for an hour. A claim
//!   is *tolerated*: an agent that claims closes through its claim like any
//!   other delivery, and the generic brief's "deliver with the domain tool"
//!   stays true for it.
//! - **Nothing here verifies the result, and nothing here looks at the file.**
//!   For a wiki the file *is* the data — the ledger row is a log of who said
//!   what — so a stat would only confirm that some file exists at that path,
//!   which is less than "delivered" implies and more than nothing: the worst
//!   place to stand. The row's `reason` says who vouches for the work instead.
//! - **The output name is emitted, never derived by the reader.** `target_rel`
//!   is computed here and written as a literal into the ask, the payload and the
//!   close. An agent cannot aim a request at a file nobody asked for.

use serde::Serialize;
// Only the close reads the payload back, and the close is a desktop tool; the
// request side writes it with a fully qualified `serde_json::json!`.
#[cfg(feature = "desktop")]
use serde_json::Value;
use tauri::State;

use super::wiki::{find_dataset_dir, is_markdown, sanitize_rel};
use crate::mail::{self, Mail, NewRequest, Refusal};
use crate::settings::SettingsState;

// ============================================================
// Identity
// ============================================================

/// The channel prefix, and the guard on the closing tool: `agent_mail` is not a
/// generic activity log, so `wiki_translate_done` may only end a wiki row.
pub(crate) const WIKI_CHANNEL: &str = "wiki";

/// The room: one document of one wiki dataset. Everything asked about the same
/// file shares it, so a second language is a second row in the same channel.
pub(crate) fn channel_id(dataset_uuid: &str, source_rel: &str) -> String {
    format!("{WIKI_CHANNEL}:{dataset_uuid}:{source_rel}")
}

/// The thing produced: one document in one language. A re-ask for the same pair
/// replaces the untaken row rather than queueing a second copy (§2.2).
pub(crate) fn subject_key(dataset_uuid: &str, source_rel: &str, lang_code: &str) -> String {
    format!("{WIKI_CHANNEL}:{dataset_uuid}:{source_rel}:{lang_code}")
}

// ============================================================
// The output name
// ============================================================

/// A language tag, lowercase: `de`, `zh`, or a subtagged `zh-cn` — nothing
/// else.
///
/// Written by the picker in the frontend's `WikiTranslate.tsx`, validated here
/// rather than trusted, because this string becomes part of a filename.
/// Deliberately *syntax* only: no language list lives in Rust, so there is no
/// second copy of the frontend's eight names to drift against.
fn valid_lang_code(code: &str) -> bool {
    let mut parts = code.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    if primary.len() < 2 || primary.len() > 3 {
        return false;
    }
    if !primary.bytes().all(|b| b.is_ascii_alphabetic() && b.is_ascii_lowercase()) {
        return false;
    }
    match parts.next() {
        // Two subtags (`zh-Hans-CN`) is a shape this app never asks about.
        None => parts.next().is_none(),
        Some(sub) => {
            parts.next().is_none()
                && sub.len() >= 2
                && sub.len() <= 8
                && sub.bytes().all(|b| b.is_ascii_digit() || (b.is_ascii_alphabetic() && b.is_ascii_lowercase()))
        }
    }
}

/// `notes/idea.md` + `de` → `notes/idea_de.md`.
///
/// The source's directory is kept: moving the output into a `translations/`
/// subtree would break the relative image links and the `.md` links that both
/// the source and its translation carry.
pub(crate) fn target_rel(source_rel: &str, lang_code: &str) -> Result<String, String> {
    if !valid_lang_code(lang_code) {
        return Err(format!(
            "'{lang_code}' is not a language code. Use a tag like `de` or `pt-BR` — it becomes \
             part of the file name."
        ));
    }
    let stem = stem_of(source_rel).ok_or_else(|| format!(
        "only a `.md` file can be translated, and '{source_rel}' is not one."
    ))?;
    // Asking to translate `idea_de.md` into German again would produce a German
    // document from a German document under a doubled name. Another language is
    // fine — a translation of a translation is a legitimate thing to ask for.
    if stem.rsplit('_').next().is_some_and(|tail| tail.eq_ignore_ascii_case(lang_code)) {
        return Err(format!(
            "'{source_rel}' is already a translation into '{lang_code}'. Ask about the source \
             file instead — its name is this one without the `_{lang_code}`."
        ));
    }
    let target = format!("{stem}_{lang_code}.md");
    if target == source_rel {
        return Err(format!("'{source_rel}' would overwrite itself; choose another language."));
    }
    Ok(target)
}

/// The path without its `.md`, or `None` when there is no `.md` to remove.
fn stem_of(rel: &str) -> Option<&str> {
    if !is_markdown(rel) {
        return None;
    }
    let dot = rel.rfind('.')?;
    Some(&rel[..dot])
}

// ============================================================
// The ask
// ============================================================

/// The handoff text an agent reads — the one canonical prompt of this road.
///
/// It opens by deviating from the generic brief, because both
/// `src/lib/agent/mail.ts`'s `promptOf` and `mail_brief` prepend *"claim this…
/// deliver with the domain tool, citing your claim_id"*. Say plainly that
/// claiming is optional, that the app will not read the result, and that the
/// agent must therefore check its own write.
fn ask_body(source_rel: &str, target: &str, lang_code: &str, lang_name: &str) -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d");
    format!(
        "Translate one wiki document, block by block, into {lang_name} (`{lang_code}`).\n\
         \n\
         Read this whole text before you act, and mind three things it says that\n\
         are not the usual mailbox contract:\n\
         \n\
         - Claiming is **optional**. `mail_claim` holds a lease for ten minutes,\n\
           hard-capped at thirty from the claim, so claim only if you can keep it\n\
           alive; otherwise work from this text directly. Either way the item is\n\
           finished with `wiki_translate_done`, citing this row's mail uuid (and\n\
           your claim_id if you hold one).\n\
         - The app will **not** verify your result. Before you report, list the\n\
           directory and read the file back: you are the only reader here.\n\
         - The destination is **fixed below**. Do not choose another name, another\n\
           extension or another folder — the app records this path as the answer.\n\
         \n\
         If the `wiki_*` tools are not in your list, your client has not mounted\n\
         FmS: point it at the app's `/mcp` endpoint (Tools → Agent shows the\n\
         address) and drive the same JSON-RPC over HTTP.\n\
         \n\
         Source    : `{source_rel}`   (wiki_dataset_read_file, `rel` = that path)\n\
         Write to  : `{target}`       (wiki_dataset_write_file, `rel` = that path)\n\
         Language  : {lang_name} (`{lang_code}`)\n\
         \n\
         1. Read the source.\n\
         2. Work top-level block by block, in the source's order: emit the original\n\
            block, then its translation. Never reword, reorder or summarise the\n\
            original, and never merge two blocks into one.\n\
         3. Not prose — pass these through byte for byte: fenced code blocks, math,\n\
            image targets, and the URLs inside links. Do translate heading text and\n\
            link text.\n\
         4. Make the file's first line\n\
            `> Translated from `{source_rel}` into {lang_name} by an agent on {today}.`\n\
            then the first block pair. The learner reads that line to decide whether\n\
            to overwrite the file, so keep it factual.\n\
         5. If `{target}` already exists, replace it through the trash so the old\n\
            version stays undoable: `wiki_dataset_list_dir` to confirm, then\n\
            `wiki_dataset_delete_file`, then `wiki_dataset_write_file`. Do not write\n\
            straight over it, and do not ask for confirmation — the trash *is* the\n\
            confirmation.\n\
         6. `wiki_dataset_list_dir` again and `wiki_dataset_read_file` what landed:\n\
            check the name, the header line, and that every block has its pair. Fix\n\
            it before you report, not after.\n\
         7. Close the item with `wiki_translate_done`."
    )
}

/// What the page gets back: the row, the text to paste, and whether an earlier
/// ask for the same document and language was replaced.
#[derive(Debug, Clone, Serialize)]
pub struct TranslateAsk {
    pub mail: Mail,
    /// The body with the same `# subject / _channel · mail:` header the dock's
    /// `Copy` and `mail_brief` render, so a prompt pasted from the page and one
    /// pasted from the dock read alike — and carry the uuid, which only the
    /// dock's rendering would otherwise add.
    pub prompt: String,
    /// The uuid of the ask this one replaced (T2). A row an agent is holding is
    /// refused with `claim_required` instead, so this never silent-drops work.
    pub replaced: Option<String>,
}

/// File the ask (T1). The whole app-side half of the feature.
///
/// Everything the ledger does for itself comes from `mail::request`: one open
/// row per subject, a supersede prelude for an untaken one, and a refusal while
/// a live lease holds it. `origin` is `user` because the learner pressed the
/// button, exactly as the books' artifact ask records it.
pub(crate) fn request_translation(
    settings: &SettingsState,
    dataset_uuid: &str,
    source_rel: &str,
    lang_code: &str,
    lang_name: &str,
) -> Result<TranslateAsk, String> {
    let rel = sanitize_rel(source_rel)?;
    let code = lang_code.trim();
    let name = lang_name.trim();
    if name.is_empty() {
        return Err("a target language name is required — it is the line the reader sees".into());
    }
    let dir = find_dataset_dir(settings, dataset_uuid)?;
    let path = dir.join(&rel);
    if !path.is_file() {
        return Err(format!(
            "File does not exist: {rel}. List the dataset with `wiki_dataset_list_dir` and pass \
             the path of the open document."
        ));
    }
    let target = target_rel(&rel, code)?;

    let payload = serde_json::json!({
        "dataset_uuid": dataset_uuid,
        "source_rel": rel,
        "target_rel": target,
        "lang_code": code,
        "lang_name": name,
        "action": { "type": "navigate", "tab": "wiki" },
    })
    .to_string();
    let channel = channel_id(dataset_uuid, &rel);
    let subject = subject_key(dataset_uuid, &rel, code);
    let label = format!(
        "translate · {} → {name}",
        stem_of(&rel).map(|s| s.rsplit('/').next().unwrap_or(s)).unwrap_or(&rel)
    );
    let body = ask_body(&rel, &target, code, name);

    let mut conn = mail::open(settings)?;
    let request = NewRequest::new(&channel, &subject, &label)
        .body(&body)
        .payload(&payload)
        .resource(Some("llm"))
        .refer(Some("wiki"), Some(dataset_uuid))
        .origin(mail::ORIGIN_USER);
    let receipt = mail::request(&mut conn, mail::now_ms(), &request).map_err(refusal_text)?;

    let prompt = format!(
        "# {}\n_channel: {} · mail: {}\n\n{}",
        receipt.mail.subject, receipt.mail.channel_id, receipt.mail.uuid, receipt.mail.body
    );
    Ok(TranslateAsk {
        mail: receipt.mail,
        prompt,
        replaced: receipt.replaced,
    })
}

/// One shape for "the ledger said no", so a refusal reaches the page and the
/// agent with both halves — the code to branch on, the sentence to act on.
fn refusal_text(r: Refusal) -> String {
    match r.successor {
        Some(successor) => format!(
            "{}: {} (it was replaced by request {successor})",
            r.code, r.message
        ),
        None => format!("{}: {}", r.code, r.message),
    }
}

// ============================================================
// The close
// ============================================================

/// What closing produced, in the ledger's own words.
///
/// Gated with the close itself: a mobile build files asks and never closes them,
/// so this shape has no caller there.
#[cfg(feature = "desktop")]
#[derive(Debug, Clone, Serialize)]
pub struct Closed {
    pub mail: Mail,
    /// `delivered` — closed on the caller's live claim (T5 + T7, reason left
    /// as the claim path leaves it). `reported` — closed on the caller's word
    /// alone, which the row marks with `reason: reported_done`.
    /// `already_delivered` — a retry, or a second agent finished the same work;
    /// nothing was written and nothing was harmed.
    pub outcome: &'static str,
}

/// The agent says the file is written. Record that, and only that.
///
/// The two paths differ in what the app is prepared to vouch for, which is why
/// they differ in the `reason` they leave behind rather than in the status:
///
/// 1. **A live claim** (`claim_id` offered and current): T5 then T7, so the row
///    closes as any delivery does and the generic brief reads true.
/// 2. **No claim, or one that lapsed mid-work**: [`mail::close_by_report`].
///    A translation that took an hour is a success, not three expired leases,
///    so falling through is the whole point — and the row still says somebody
///    reported it rather than the app seeing it.
///
/// A row a *different* agent is holding is refused with `claim_required`: an
/// unowned close must not stamp over a live lease.
#[cfg(feature = "desktop")]
pub(crate) fn close_translation(
    settings: &SettingsState,
    mail_uuid: &str,
    claim_id: Option<&str>,
    who: &str,
) -> Result<Closed, String> {
    let conn = mail::open(settings)?;
    let now = mail::now_ms();
    let row = mail::read_one(&conn, mail_uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("unknown: no request {mail_uuid}"))?;

    // The subject guard, not an artifact guard: without it this tool is
    // `mail_finish_anything`, a hatch that closes book jobs without their
    // guarded write.
    let prefix = format!("{WIKI_CHANNEL}:");
    if !row.channel_id.starts_with(&prefix) {
        return Err(format!(
            "wrong_subject: request {mail_uuid} sits on channel '{}', which is not a wiki \
             translation, so nothing was closed. A book artifact delivers with \
             `book_save_artifact`; a wiki ask closes with `wiki_translate_done` naming its own \
             mail uuid.",
            row.channel_id
        ));
    }

    match row.status.as_str() {
        mail::DELIVERED => {
            return Ok(Closed { mail: row, outcome: "already_delivered" });
        }
        mail::WAITING_USER => {
            // T18's from-column stops at the open states, and this is the reason:
            // the row is in the user's turn, not the worker's, so a report would
            // close a question that is still owed an answer.
            return Err(format!(
                "still_asking: request {mail_uuid} is waiting on the user's answer, so nothing \
                 was closed. Ask it through the dock (Ctrl+Space); the answer moves the row back \
                 to `queued` and the report will land then."
            ));
        }
        mail::CANCELLED | mail::FAILED => {
            return Err(format!(
                "orphan_refused: request {mail_uuid} is {} ({}). A cancel means cancel — press \
                 Translate again if the file is still wanted, then cite the new uuid.",
                row.status,
                row.reason.as_deref().unwrap_or("no reason recorded")
            ));
        }
        _ => {}
    }

    // The destination the app computed when it filed the ask. There is no
    // `result_rel` parameter on the tool, so an agent cannot aim a request at a
    // path nobody asked for.
    let parsed: Value = serde_json::from_str(&row.payload_json).unwrap_or(Value::Null);
    let result_ref = parsed
        .get("target_rel")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let live = matches!(row.status.as_str(), mail::CLAIMED | mail::DELIVERING)
        && row.expires_at.map(|e| e > now).unwrap_or(false);

    if let Some(claim) = claim_id.map(str::trim).filter(|c| !c.is_empty()) {
        // Path 1 — the caller holds the lease this row names.
        let began = mail::begin_delivery(&conn, mail_uuid, claim, now).map_err(|e| e.to_string())?;
        if began == 1 {
            let closed = mail::finish_delivery(&conn, mail_uuid, claim, &result_ref, now)
                .map_err(|e| e.to_string())?;
            let mail = current(&conn, mail_uuid)?;
            return Ok(Closed {
                mail,
                outcome: if closed == 1 { "delivered" } else { "written_unconfirmed" },
            });
        }
        // Not the current claim: lapsed, moved, or never taken. Fall through to
        // the report path rather than losing a finished translation.
    }

    if live {
        return Err(format!(
            "claim_required: request {mail_uuid} is being worked on by {}. Pass your claim_id \
             with the mail uuid to close it as your delivery, or hand it back with \
             `mail_release`.",
            row.claimed_by.as_deref().unwrap_or("another agent")
        ));
    }

    // Path 2 — an unheld row: the app records the report, nothing more.
    let changed = mail::close_by_report(&conn, mail_uuid, who, &result_ref, now)
        .map_err(|e| e.to_string())?;
    let mail = current(&conn, mail_uuid)?;
    Ok(Closed {
        mail,
        outcome: if changed == 1 { "reported" } else { "already_delivered" },
    })
}

/// The row as the ledger now holds it, after a close moved it.
#[cfg(feature = "desktop")]
fn current(conn: &rusqlite::Connection, uuid: &str) -> Result<Mail, String> {
    mail::read_one(conn, uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("request {uuid} vanished while it was being closed"))
}

// ============================================================
// Command
// ============================================================

/// Ask an agent to translate one wiki document into `lang_code`.
///
/// Files the request and hands back the prompt to paste; writes no file and
/// starts no model. The answer arrives later as a markdown file beside the
/// source, and the agent closes the row with `wiki_translate_done`.
#[tauri::command]
pub async fn wiki_translate_request(
    settings: State<'_, SettingsState>,
    dataset_uuid: String,
    source_rel: String,
    lang_code: String,
    lang_name: String,
) -> Result<TranslateAsk, String> {
    request_translation(
        settings.inner(),
        &dataset_uuid,
        &source_rel,
        &lang_code,
        &lang_name,
    )
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The naming rule the whole road is built on, and the refusals that keep
    /// a learner's typing out of a filename. Each message names the next move,
    /// so the test also pins that the sentence is not just a bare "no".
    #[test]
    fn a_translation_lands_as_a_sibling_in_the_source_directory() {
        assert_eq!(target_rel("notes/idea.md", "de").unwrap(), "notes/idea_de.md");
        assert_eq!(target_rel("idea.md", "zh").unwrap(), "idea_zh.md");
        assert_eq!(target_rel("idea.md", "zh-cn").unwrap(), "idea_zh-cn.md");
        // A translation of a translation is a legitimate ask — into another
        // language, which is why the guard below is per-language, not global.
        assert_eq!(target_rel("notes/idea_de.md", "fr").unwrap(), "notes/idea_de_fr.md");
    }

    #[test]
    fn a_refusal_says_what_to_do_instead() {
        // Not prose, so there is nothing to pair blocks against.
        assert!(target_rel("notes/idea.txt", "de").unwrap_err().contains("`.md`"));
        // Translating a German file into German would aim the output at itself.
        let collision = target_rel("notes/idea_de.md", "de").unwrap_err();
        assert!(collision.contains("already a translation"), "{collision}");
        assert!(collision.contains("source file"), "{collision}");
        // The code reaches a filename, so it is validated, not looked up: only a
        // lowercase primary tag plus at most one lowercase subtag survives.
        for bad in ["DE", "d", "too-long-code", "pt_BR", "zh-hans-cn", ""] {
            assert!(target_rel("notes/idea.md", bad).is_err(), "{bad} accepted");
        }
        assert!(target_rel("notes/idea.md", "de").is_ok());
    }

    /// `channelLine` splits on `:` and assumes three parts, which holds only
    /// because `sanitize_rel` refuses a colon inside a component. Pin the shape
    /// here, where the dependency is documented, rather than in the renderer.
    #[test]
    fn the_channel_has_exactly_three_parts() {
        let channel = channel_id("abcd1234", "notes/idea.md");
        assert_eq!(channel, "wiki:abcd1234:notes/idea.md");
        assert_eq!(channel.split(':').count(), 3);
        assert_eq!(
            subject_key("abcd1234", "notes/idea.md", "de"),
            "wiki:abcd1234:notes/idea.md:de"
        );
    }

    #[test]
    fn an_escape_is_refused_before_anything_is_filed() {
        assert!(sanitize_rel("../escape.md").is_err());
        assert!(sanitize_rel("a:b.md").is_err());
    }
}
