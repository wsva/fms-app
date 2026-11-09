//! OCR through a vision model served by Ollama (`ocr_recognize_ollama`).
//!
//! Tesseract wants an upright, evenly lit, single-column scan; a photo of a book
//! page taken at an angle, a dense footnote apparatus or ornate type is where it
//! turns to noise and a vision model keeps reading. `glm-ocr` was measured to
//! transcribe pages Tesseract mangled, which is why this engine exists beside
//! the two Tesseract paths (TSV rebuild, plain text).
//!
//! The engine is deliberately model-agnostic: any model that accepts an image
//! works, so a better OCR model (or a general VLM) drops in by name once it is
//! pulled into Ollama. What the model *answers* is markdown with LaTeX
//! superscripts, so every answer goes through [`clean`] before it is handed
//! back — see that module for the rules.
//!
//! Models run one request at a time here: Ollama serialises them anyway, and a
//! vision model on the CPU is slow enough that a second concurrent page would
//! only make the first slower.

use std::time::Duration;

use serde::Deserialize;

use super::clean::{self, Cleaned};

/// What one Ollama OCR run returns.
#[derive(serde::Serialize)]
pub struct OllamaOcrResult {
    /// The answer after [`clean`]: what the user copies.
    pub text: String,
    /// The model's own answer, before the cleanup — what the engine said.
    pub raw: String,
    /// Reference/footnote marks the cleanup removed.
    pub references: usize,
    /// Other markup the cleanup unwrapped or dropped.
    pub markup: usize,
    /// The model that produced the answer.
    pub model: String,
}

/// The instruction that goes with the image for a *general* vision model.
///
/// It asks for the two things the cleanup cannot recover: the page's own
/// paragraphing, and the language it is printed in (a VLM asked nothing tends to
/// translate into English, which is exactly wrong for a dictation text).
///
/// Purpose-built OCR models do not want prose: their task picker was trained on
/// a handful of exact phrases, and a sentence-shaped prompt makes them answer
/// worse, not better. Those get their own preset — see [`default_prompt_for`].
pub const DEFAULT_INSTRUCTIONS: &str = "\
Transcribe every piece of text in this image exactly as it is printed, in reading order.

Rules:
- Answer with the transcription only: no preamble, no summary, no commentary, no markdown \
fences, no headings, no bold or italic markers.
- Keep the language the text is printed in. Never translate.
- One paragraph per paragraph on the page. Do not reflow paragraphs into a single line, and \
do not merge separate paragraphs.
- Keep a word split across a line break as printed, hyphen included.
- Leave out footnote and reference markers (superscript numbers): they are not part of the \
sentence.
- Leave out page numbers, running heads and marginalia.";

/// GLM-OCR's task selector: the phrase is matched, not read.
///
/// The model was trained with `Text Recognition:` / `Table Recognition:` /
/// `Formula Recognition:` as its whole prompt, so plain text is what this asks
/// for. It also fixes the language question on its own — GLM-OCR transcribes
/// what is printed instead of answering in English the way a general VLM does.
pub const GLM_TASK_TEXT: &str = "Text Recognition:";

/// Ollama's default context is 4096 tokens, which an image plus a page of text
/// overflows: the overflow does not degrade gracefully, it garbles or truncates
/// the answer. 16k is the figure the GLM-OCR guides settle on.
const NUM_CTX: u32 = 16384;

/// The prompt to send when the caller supplied none, chosen by model name.
///
/// The rule is deliberately narrow — one exact family — so a model added later
/// falls through to the prose prompt, which any general VLM understands. When a
/// new purpose-built OCR model arrives, give it a branch here rather than
/// weakening [`DEFAULT_INSTRUCTIONS`] for everyone else.
pub fn default_prompt_for(model: &str) -> &'static str {
    if model.to_ascii_lowercase().contains("glm-ocr") {
        GLM_TASK_TEXT
    } else {
        DEFAULT_INSTRUCTIONS
    }
}

#[derive(Deserialize)]
struct ChatReply {
    message: Option<ReplyMessage>,
}

#[derive(Deserialize)]
struct ReplyMessage {
    content: String,
}

/// Run one page through `model` and return the cleaned text.
///
/// `instructions` replaces [`DEFAULT_INSTRUCTIONS`] for callers that know their
/// page (a table, a form, a page in two columns).
pub async fn recognize(
    base_url: &str,
    model: &str,
    image_base64: &str,
    instructions: Option<&str>,
) -> Result<OllamaOcrResult, String> {
    if model.trim().is_empty() {
        return Err("no Ollama model selected — pick one on the OCR page".to_string());
    }
    if image_base64.trim().is_empty() {
        return Err("no image to recognize".to_string());
    }

    let prompt = instructions
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| default_prompt_for(model));

    // A cold vision model spends tens of seconds loading before it reads a
    // single pixel; the request timeout has to cover the load, not just the page.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;

    let body = serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": prompt,
            // Ollama takes bare base64, not a data URL.
            "images": [bare_base64(image_base64)],
        }],
        "stream": false,
        "options": {
            // A transcription is not a creative act: sampling can only invent words.
            "temperature": 0,
            // See `NUM_CTX`: below this, image + page does not fit.
            "num_ctx": NUM_CTX,
        },
    });

    let started = std::time::Instant::now();
    let resp = client
        .post(format!("{}/api/chat", base_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed to reach Ollama at {}: {}", base_url, e))?;

    let status = resp.status();
    if !status.is_success() {
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!("Ollama OCR failed ({}): {}", status, detail.trim()));
    }

    let reply: ChatReply = resp
        .json()
        .await
        .map_err(|e| format!("Unreadable reply from Ollama: {}", e))?;

    let raw = reply.message.map(|m| m.content).unwrap_or_default();
    if raw.trim().is_empty() {
        return Err(format!(
            "{} returned no text — is it a vision model? (pull one that accepts images, e.g. glm-ocr)",
            model
        ));
    }

    let Cleaned {
        text,
        references,
        markup,
    } = clean::clean(&raw);

    log::info!(
        "[OCR] ollama {}: {} raw chars, {} output chars, {} references and {} markup marks removed in {:.1}s",
        model,
        raw.chars().count(),
        text.chars().count(),
        references,
        markup,
        started.elapsed().as_secs_f32()
    );

    Ok(OllamaOcrResult {
        text,
        raw,
        references,
        markup,
        model: model.to_string(),
    })
}

/// The base64 payload of an image, whether it arrived as a data URL or bare.
fn bare_base64(image_base64: &str) -> &str {
    match image_base64.find(',') {
        Some(at) => &image_base64[at + 1..],
        None => image_base64,
    }
}

#[cfg(test)]
mod tests {
    use super::{default_prompt_for, DEFAULT_INSTRUCTIONS, GLM_TASK_TEXT};

    #[test]
    fn glm_ocr_gets_its_task_phrase() {
        assert_eq!(default_prompt_for("glm-ocr:bf16"), GLM_TASK_TEXT);
        assert_eq!(default_prompt_for("GLM-OCR"), GLM_TASK_TEXT);
    }

    #[test]
    fn any_other_model_gets_the_prose_prompt() {
        assert_eq!(default_prompt_for("qwen2.5vl:7b"), DEFAULT_INSTRUCTIONS);
        assert_eq!(default_prompt_for("llava"), DEFAULT_INSTRUCTIONS);
    }
}
