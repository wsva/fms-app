/// Catalog of recommended LLM models available through Ollama.
/// Ollama handles the actual model management (pull, delete, inference).

/// Recommended LLM model definition — used for the "available models" UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmModelDef {
    /// Ollama model tag (e.g. "gemma4:e4b")
    pub ollama_name: &'static str,
    /// Display name shown in the UI
    pub display_name: &'static str,
    /// Short description
    pub description: &'static str,
    /// Number of parameters (e.g. "4B")
    pub parameters: &'static str,
    /// Approximate download size in MB
    pub size_mb: u64,
    /// Supported language codes
    pub languages: &'static [&'static str],
}

/// Complete list of recommended LLM models.
pub const MODELS: &[LlmModelDef] = &[
    LlmModelDef {
        ollama_name: "gemma4:e4b",
        display_name: "Gemma 4 E4B",
        description: "Efficient 4B model — fast on CPU, good for quick explanations",
        parameters: "4B",
        size_mb: 3300,
        languages: &["en", "de", "fr", "es", "it", "pt", "zh", "ja", "ko", "hi"],
    },
    LlmModelDef {
        ollama_name: "gemma4:12b",
        display_name: "Gemma 4 12B",
        description: "Higher quality model for complex reasoning and detailed explanations",
        parameters: "12B",
        size_mb: 7600,
        languages: &["en", "de", "fr", "es", "it", "pt", "zh", "ja", "ko", "hi"],
    },
    LlmModelDef {
        ollama_name: "gemma4:27b",
        display_name: "Gemma 4 27B",
        description: "Top quality model for advanced reasoning and nuanced tasks",
        parameters: "27B",
        size_mb: 16000,
        languages: &["en", "de", "fr", "es", "it", "pt", "zh", "ja", "ko", "hi"],
    },
];

/// Find a recommended model by its Ollama name.
#[allow(dead_code)]
pub fn find_model(ollama_name: &str) -> Option<&'static LlmModelDef> {
    MODELS.iter().find(|m| m.ollama_name == ollama_name)
}
