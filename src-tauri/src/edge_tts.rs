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

/// Synthesize text and return the raw audio bytes (reused by the web service).
pub(crate) async fn synthesize_to_bytes(
    text: &str,
    voice: &str,
    rate: Option<String>,
    volume: Option<String>,
    pitch: Option<String>,
) -> Result<Vec<u8>, String> {
    log::debug!("edge_tts synthesize_to_bytes: voice={}, text_len={}", voice, text.len());
    let client = EdgeTtsClient::new().map_err(|e| e.to_string())?;

    let options = SpeakOptions {
        voice: voice.to_string(),
        rate: rate.unwrap_or_else(|| "+0%".to_string()),
        volume: volume.unwrap_or_else(|| "+0%".to_string()),
        pitch: pitch.unwrap_or_else(|| "+0Hz".to_string()),
        boundary: Boundary::Word,
        ..SpeakOptions::default()
    };

    let result = client
        .synthesize(text, options)
        .await
        .map_err(|e| e.to_string())?;

    Ok(result.audio)
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

    let options = SpeakOptions {
        voice: args.voice,
        rate: args.rate.unwrap_or_else(|| "+0%".to_string()),
        volume: args.volume.unwrap_or_else(|| "+0%".to_string()),
        pitch: args.pitch.unwrap_or_else(|| "+0Hz".to_string()),
        boundary: Boundary::Word,
        ..SpeakOptions::default()
    };

    let result = client
        .synthesize(&args.text, options)
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

    let options = SpeakOptions {
        voice: args.voice,
        rate: args.rate.unwrap_or_else(|| "+0%".to_string()),
        volume: args.volume.unwrap_or_else(|| "+0%".to_string()),
        pitch: args.pitch.unwrap_or_else(|| "+0Hz".to_string()),
        boundary: Boundary::Word,
        ..SpeakOptions::default()
    };

    let result = client
        .synthesize(&args.text, options)
        .await
        .map_err(|e| e.to_string())?;

    // Encode audio bytes as base64
    use base64::{Engine as _, engine::general_purpose};
    let audio_base64 = general_purpose::STANDARD.encode(&result.audio);

    Ok(TtsPreviewResult { audio_base64 })
}
