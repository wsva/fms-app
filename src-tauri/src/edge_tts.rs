use serde::{Deserialize, Serialize};
use std::path::Path;
use edge_tts_rust::{EdgeTtsClient, SpeakOptions, Boundary};

/// A voice available from Edge TTS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsVoice {
    pub name: String,
    pub short_name: String,
    pub locale: String,
    pub gender: String,
}

/// Parameters for TTS synthesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsSynthesizeArgs {
    pub text: String,
    pub voice: String,
    pub rate: Option<String>,
    pub volume: Option<String>,
    pub pitch: Option<String>,
    pub output_path: String,
}

/// Result of a TTS synthesis operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsSynthesizeResult {
    pub audio_len: usize,
    pub output_path: String,
}

/// Result of a TTS preview operation (returns audio data as base64).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsPreviewResult {
    pub audio_base64: String,
}

/// A word boundary reported by Edge TTS. Times are seconds from the start of
/// the returned audio.
///
/// These were always available — `Boundary::Word` is requested — and were
/// always thrown away. The read-aloud reference timeline needs them to know
/// which phones belong to which word.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TtsWordBoundary {
    pub text: String,
    pub start: f64,
    pub duration: f64,
}

/// Synthesised audio plus the word boundaries the service reported.
pub(crate) struct TtsAudio {
    pub audio: Vec<u8>,
    pub boundaries: Vec<TtsWordBoundary>,
}

/// Edge TTS reports offsets in 100 ns ticks (Windows FILETIME units).
const TICKS_PER_SECOND: f64 = 10_000_000.0;

/// The audio format every caller here depends on.
///
/// Set explicitly instead of relying on `SpeakOptions::default()`: in
/// edge-tts-rust 0.1.7 the default output format became
/// `webm-24khz-16bit-mono-opus`, while the read-aloud reference cache writes
/// `*.mp3` and decodes it back with Symphonia, and the preview path hands the
/// bytes to an `<audio>` element as mp3.
const MP3_OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// The options every entry point uses: the caller's voice and prosody, mp3
/// output, and **word** boundaries — the reference timeline groups phones by
/// word, so sentence boundaries alone would be useless to it.
fn speak_options(
    voice: impl Into<String>,
    rate: Option<String>,
    volume: Option<String>,
    pitch: Option<String>,
) -> SpeakOptions {
    SpeakOptions {
        voice: voice.into(),
        rate: rate.unwrap_or_else(|| "+0%".to_string()),
        volume: volume.unwrap_or_else(|| "+0%".to_string()),
        pitch: pitch.unwrap_or_else(|| "+0Hz".to_string()),
        boundary: Boundary::Word,
        output_format: MP3_OUTPUT_FORMAT.to_string(),
    }
}

/// Synthesize text and return the raw audio bytes **and** its word boundaries
/// (used by the read-aloud reference timeline).
pub(crate) async fn synthesize_with_boundaries(
    text: &str,
    voice: &str,
    rate: Option<String>,
    volume: Option<String>,
    pitch: Option<String>,
) -> Result<TtsAudio, String> {
    log::debug!("edge_tts synthesize_with_boundaries: voice={}, text_len={}", voice, text.len());
    let client = EdgeTtsClient::new().map_err(|e| e.to_string())?;

    let result = client
        .synthesize(text, speak_options(voice, rate, volume, pitch))
        .await
        .map_err(|e| e.to_string())?;

    let boundaries = result
        .boundaries
        .into_iter()
        .filter(|b| matches!(b.kind, Boundary::Word))
        .map(|b| TtsWordBoundary {
            text: b.text,
            start: b.offset_ticks as f64 / TICKS_PER_SECOND,
            duration: b.duration_ticks as f64 / TICKS_PER_SECOND,
        })
        .collect();

    Ok(TtsAudio { audio: result.audio, boundaries })
}

/// Synthesize text and return the raw audio bytes (reused by the web service).
#[allow(dead_code)]
pub(crate) async fn synthesize_to_bytes(
    text: &str,
    voice: &str,
    rate: Option<String>,
    volume: Option<String>,
    pitch: Option<String>,
) -> Result<Vec<u8>, String> {
    Ok(synthesize_with_boundaries(text, voice, rate, volume, pitch)
        .await?
        .audio)
}

/// List all available voices from Edge TTS.
#[tauri::command]
pub async fn edge_tts_list_voices() -> Result<Vec<TtsVoice>, String> {
    log::debug!("edge_tts_list_voices");
    let client = EdgeTtsClient::new().map_err(|e| e.to_string())?;
    let voices = client.list_voices().await.map_err(|e| e.to_string())?;

    let result: Vec<TtsVoice> = voices
        .into_iter()
        .map(|v| TtsVoice {
            name: v.name.clone(),
            short_name: v.short_name.clone(),
            locale: v.locale.clone(),
            gender: v.gender.clone(),
        })
        .collect();

    Ok(result)
}

/// Synthesize text to audio and save to file.
#[tauri::command]
pub async fn edge_tts_synthesize(args: TtsSynthesizeArgs) -> Result<TtsSynthesizeResult, String> {
    log::info!("edge_tts_synthesize: voice={}, output={}", args.voice, args.output_path);
    let client = EdgeTtsClient::new().map_err(|e| e.to_string())?;

    let result = client
        .synthesize(
            &args.text,
            speak_options(args.voice, args.rate, args.volume, args.pitch),
        )
        .await
        .map_err(|e| e.to_string())?;

    let audio_len = result.audio.len();

    // Write audio bytes to the output file
    let path = Path::new(&args.output_path);
    std::fs::write(path, &result.audio).map_err(|e| format!("Failed to write file: {e}"))?;

    Ok(TtsSynthesizeResult {
        audio_len,
        output_path: args.output_path,
    })
}

/// Synthesize text to audio and return as base64 (for preview without saving).
#[tauri::command]
pub async fn edge_tts_preview(args: TtsSynthesizeArgs) -> Result<TtsPreviewResult, String> {
    log::debug!("edge_tts_preview: voice={}, text_len={}", args.voice, args.text.len());
    let client = EdgeTtsClient::new().map_err(|e| e.to_string())?;

    let result = client
        .synthesize(
            &args.text,
            speak_options(args.voice, args.rate, args.volume, args.pitch),
        )
        .await
        .map_err(|e| e.to_string())?;

    // Encode audio bytes as base64
    use base64::{Engine as _, engine::general_purpose};
    let audio_base64 = general_purpose::STANDARD.encode(&result.audio);

    Ok(TtsPreviewResult { audio_base64 })
}
