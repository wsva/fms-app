/// Static model registry containing all supported speech-to-text models.
/// Models are sourced from blob.handy.computer CDN with HuggingFace manual fallback.

/// Engine type determines how the model is loaded and used for transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum EngineType {
    /// Whisper-family GGUF models via transcribe-cpp
    TranscribeCpp,
    /// NVIDIA Parakeet ONNX models
    Parakeet,
    /// Moonshine base ONNX models
    Moonshine,
    /// Moonshine streaming ONNX models
    MoonshineStreaming,
    /// SenseVoice ONNX models
    SenseVoice,
    /// GigaAM ONNX models
    GigaAM,
    /// Canary ONNX models
    Canary,
    /// Cohere ONNX models
    Cohere,
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineType::TranscribeCpp => write!(f, "Whisper GGUF"),
            EngineType::Parakeet => write!(f, "Parakeet"),
            EngineType::Moonshine => write!(f, "Moonshine"),
            EngineType::MoonshineStreaming => write!(f, "Moonshine Streaming"),
            EngineType::SenseVoice => write!(f, "SenseVoice"),
            EngineType::GigaAM => write!(f, "GigaAM"),
            EngineType::Canary => write!(f, "Canary"),
            EngineType::Cohere => write!(f, "Cohere"),
        }
    }
}

/// Model definition with all metadata needed for download, loading, and UI display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelDef {
    /// Unique identifier for the model
    pub id: &'static str,
    /// Display name
    pub name: &'static str,
    /// Short description
    pub description: &'static str,
    /// Engine type determines loading and transcription logic
    pub engine: EngineType,
    /// Direct download URL from blob.handy.computer
    pub blob_url: &'static str,
    /// HuggingFace repository URL for manual download fallback
    pub hf_repo_url: &'static str,
    /// SHA256 hash for integrity verification (if available)
    pub sha256: Option<&'static str>,
    /// Model size in MB
    pub size_mb: u64,
    /// Whether the model is a directory (tar.gz archive) or single file
    pub is_directory: bool,
    /// Supported language codes
    pub languages: &'static [&'static str],
    /// Accuracy score (0.0 to 1.0)
    pub accuracy_score: f32,
    /// Speed score (0.0 to 1.0)
    pub speed_score: f32,
    /// Whether the model supports translation to English
    pub supports_translation: bool,
}

/// Complete list of all supported models.
pub const MODELS: &[ModelDef] = &[
    // ========================================================================
    // Whisper GGUF Models (transcribe-cpp)
    // ========================================================================
    ModelDef {
        id: "whisper-small",
        name: "Whisper Small",
        description: "Fast and fairly accurate",
        engine: EngineType::TranscribeCpp,
        blob_url: "https://blob.handy.computer/ggml-small.bin",
        hf_repo_url: "https://huggingface.co/ggerganov/whisper.cpp",
        sha256: None,
        size_mb: 465,
        is_directory: false,
        languages: &["en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su"],
        accuracy_score: 0.60,
        speed_score: 0.85,
        supports_translation: true,
    },
    ModelDef {
        id: "whisper-medium",
        name: "Whisper Medium",
        description: "Good accuracy, medium speed",
        engine: EngineType::TranscribeCpp,
        blob_url: "https://blob.handy.computer/whisper-medium-q4_1.bin",
        hf_repo_url: "https://huggingface.co/ggerganov/whisper.cpp",
        sha256: None,
        size_mb: 469,
        is_directory: false,
        languages: &["en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su"],
        accuracy_score: 0.75,
        speed_score: 0.60,
        supports_translation: true,
    },
    ModelDef {
        id: "whisper-turbo",
        name: "Whisper Turbo",
        description: "Balanced accuracy and speed",
        engine: EngineType::TranscribeCpp,
        blob_url: "https://blob.handy.computer/ggml-large-v3-turbo.bin",
        hf_repo_url: "https://huggingface.co/ggerganov/whisper.cpp",
        sha256: None,
        size_mb: 1549,
        is_directory: false,
        languages: &["en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su"],
        accuracy_score: 0.80,
        speed_score: 0.40,
        supports_translation: false,
    },
    ModelDef {
        id: "whisper-large",
        name: "Whisper Large",
        description: "Good accuracy, but slow",
        engine: EngineType::TranscribeCpp,
        blob_url: "https://blob.handy.computer/ggml-large-v3-q5_0.bin",
        hf_repo_url: "https://huggingface.co/ggerganov/whisper.cpp",
        sha256: None,
        size_mb: 1031,
        is_directory: false,
        languages: &["en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su"],
        accuracy_score: 0.85,
        speed_score: 0.30,
        supports_translation: true,
    },
    ModelDef {
        id: "breeze-asr",
        name: "Breeze ASR",
        description: "Optimized for Taiwanese Mandarin with code-switching support",
        engine: EngineType::TranscribeCpp,
        blob_url: "https://blob.handy.computer/breeze-asr-q5_k.bin",
        hf_repo_url: "https://huggingface.co/MediaTek-Research/breeze-asr",
        sha256: None,
        size_mb: 1030,
        is_directory: false,
        languages: &["zh", "en"],
        accuracy_score: 0.85,
        speed_score: 0.35,
        supports_translation: false,
    },
    // ========================================================================
    // Parakeet Models (transcribe-rs ONNX)
    // ========================================================================
    ModelDef {
        id: "parakeet-v2",
        name: "Parakeet V2",
        description: "English only. The best model for English speakers",
        engine: EngineType::Parakeet,
        blob_url: "https://blob.handy.computer/parakeet-v2-int8.tar.gz",
        hf_repo_url: "https://huggingface.co/nvidia/parakeet-ctc-0.6b-en",
        sha256: Some("ac9b9429984dd565b25097337a887bb7f0f8ac393573661c651f0e7d31563991"),
        size_mb: 451,
        is_directory: true,
        languages: &["en"],
        accuracy_score: 0.85,
        speed_score: 0.85,
        supports_translation: false,
    },
    ModelDef {
        id: "parakeet-v3",
        name: "Parakeet V3",
        description: "Fast and accurate. Supports 25 European languages",
        engine: EngineType::Parakeet,
        blob_url: "https://blob.handy.computer/parakeet-v3-int8.tar.gz",
        hf_repo_url: "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
        sha256: Some("43d37191602727524a7d8c6da0eef11c4ba24320f5b4730f1a2497befc2efa77"),
        size_mb: 456,
        is_directory: true,
        languages: &["bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk"],
        accuracy_score: 0.80,
        speed_score: 0.85,
        supports_translation: false,
    },
    // ========================================================================
    // Moonshine Models (transcribe-rs ONNX)
    // ========================================================================
    ModelDef {
        id: "moonshine-base",
        name: "Moonshine Base",
        description: "Very fast, English only. Handles accents well",
        engine: EngineType::Moonshine,
        blob_url: "https://blob.handy.computer/moonshine-base.tar.gz",
        hf_repo_url: "https://huggingface.co/usefulsensors/moonshine-base",
        sha256: Some("04bf6ab012cfceebd4ac7cf88c1b31d027bbdd3cd704649b692e2e935236b7e8"),
        size_mb: 55,
        is_directory: true,
        languages: &["en"],
        accuracy_score: 0.70,
        speed_score: 0.90,
        supports_translation: false,
    },
    ModelDef {
        id: "moonshine-tiny",
        name: "Moonshine Tiny",
        description: "Ultra-fast, English only",
        engine: EngineType::MoonshineStreaming,
        blob_url: "https://blob.handy.computer/moonshine-tiny-streaming-en.tar.gz",
        hf_repo_url: "https://huggingface.co/usefulsensors/moonshine-tiny",
        sha256: Some("465addcfca9e86117415677dfdc98b21edc53537210333a3ecdb58509a80abaf"),
        size_mb: 31,
        is_directory: true,
        languages: &["en"],
        accuracy_score: 0.55,
        speed_score: 0.95,
        supports_translation: false,
    },
    ModelDef {
        id: "moonshine-small",
        name: "Moonshine Small",
        description: "Fast, English only. Good balance of speed and accuracy",
        engine: EngineType::MoonshineStreaming,
        blob_url: "https://blob.handy.computer/moonshine-small-streaming-en.tar.gz",
        hf_repo_url: "https://huggingface.co/usefulsensors/moonshine-small",
        sha256: Some("dbb3e1c1832bd88a4ac712f7449a136cc2c9a18c5fe33a12ed1b7cb1cfe9cdd5"),
        size_mb: 99,
        is_directory: true,
        languages: &["en"],
        accuracy_score: 0.65,
        speed_score: 0.90,
        supports_translation: false,
    },
    ModelDef {
        id: "moonshine-medium",
        name: "Moonshine Medium",
        description: "English only. High quality",
        engine: EngineType::MoonshineStreaming,
        blob_url: "https://blob.handy.computer/moonshine-medium-streaming-en.tar.gz",
        hf_repo_url: "https://huggingface.co/usefulsensors/moonshine-medium",
        sha256: Some("07a66f3bff1c77e75a2f637e5a263928a08baae3c29c4c053fc968a9a9373d13"),
        size_mb: 192,
        is_directory: true,
        languages: &["en"],
        accuracy_score: 0.75,
        speed_score: 0.80,
        supports_translation: false,
    },
    // ========================================================================
    // SenseVoice Models (transcribe-rs ONNX)
    // ========================================================================
    ModelDef {
        id: "sense-voice",
        name: "SenseVoice",
        description: "Very fast. Chinese, English, Japanese, Korean, Cantonese",
        engine: EngineType::SenseVoice,
        blob_url: "https://blob.handy.computer/sense-voice-int8.tar.gz",
        hf_repo_url: "https://huggingface.co/FunAudioLLM/SenseVoiceSmall",
        sha256: Some("171d611fe5d353a50bbb741b6f3ef42559b1565685684e9aa888ef563ba3e8a4"),
        size_mb: 152,
        is_directory: true,
        languages: &["zh", "en", "ja", "ko", "yue"],
        accuracy_score: 0.65,
        speed_score: 0.95,
        supports_translation: false,
    },
    // ========================================================================
    // GigaAM Models (transcribe-rs ONNX)
    // ========================================================================
    ModelDef {
        id: "gigaam-v3",
        name: "GigaAM v3",
        description: "Russian speech recognition. Fast and accurate",
        engine: EngineType::GigaAM,
        blob_url: "https://blob.handy.computer/giga-am-v3-int8.tar.gz",
        hf_repo_url: "https://huggingface.co/sberbank-ai/GigaAM",
        sha256: Some("d872462268430db140b69b72e0fc4b787b194c1dbe51b58de39444d55b6da45b"),
        size_mb: 151,
        is_directory: true,
        languages: &["ru"],
        accuracy_score: 0.85,
        speed_score: 0.75,
        supports_translation: false,
    },
    // ========================================================================
    // Canary Models (transcribe-rs ONNX)
    // ========================================================================
    ModelDef {
        id: "canary-flash",
        name: "Canary 180M Flash",
        description: "Very fast. English, German, Spanish, French. Supports translation",
        engine: EngineType::Canary,
        blob_url: "https://blob.handy.computer/canary-180m-flash.tar.gz",
        hf_repo_url: "https://huggingface.co/nvidia/canary-180m-flash",
        sha256: Some("6d9cfca6118b296e196eaedc1c8fa9788305a7b0f1feafdb6dc91932ab6e53f7"),
        size_mb: 146,
        is_directory: true,
        languages: &["en", "de", "es", "fr"],
        accuracy_score: 0.75,
        speed_score: 0.85,
        supports_translation: true,
    },
    ModelDef {
        id: "canary-1b",
        name: "Canary 1B v2",
        description: "Accurate multilingual. 25 European languages. Supports translation",
        engine: EngineType::Canary,
        blob_url: "https://blob.handy.computer/canary-1b-v2.tar.gz",
        hf_repo_url: "https://huggingface.co/nvidia/canary-1b-v2",
        sha256: Some("02305b2a25f9cf3e7deaffa7f94df00efa44f442cd55c101c2cb9c000f904666"),
        size_mb: 691,
        is_directory: true,
        languages: &["bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk"],
        accuracy_score: 0.85,
        speed_score: 0.70,
        supports_translation: true,
    },
    // ========================================================================
    // Cohere Models (transcribe-rs ONNX)
    // ========================================================================
    ModelDef {
        id: "cohere",
        name: "Cohere",
        description: "A large, slower, but very accurate multilingual model",
        engine: EngineType::Cohere,
        blob_url: "https://blob.handy.computer/cohere-int8.tar.gz",
        hf_repo_url: "https://huggingface.co/CohereForAI/canary-labse",
        sha256: Some("ea2257d52434f3644574f187dcdcf666e302cd11b92866116ab8e14cd9c887f0"),
        size_mb: 1708,
        is_directory: true,
        languages: &["en", "fr", "de", "it", "es", "pt", "el", "nl", "pl", "zh", "ja", "ko", "vi", "ar"],
        accuracy_score: 0.90,
        speed_score: 0.60,
        supports_translation: false,
    },
];

/// Find a model definition by ID.
pub fn find_model(id: &str) -> Option<&'static ModelDef> {
    MODELS.iter().find(|m| m.id == id)
}

/// Get all model IDs.
#[allow(dead_code)]
pub fn model_ids() -> impl Iterator<Item = &'static str> {
    MODELS.iter().map(|m| m.id)
}
