// ---------------------------------------------------------------------------
// LLM types — mirror Rust backend types in llm.rs (Ollama-based)
// ---------------------------------------------------------------------------

/** A single chat message (user or assistant). */
export interface ChatMessage {
  role: "user" | "assistant" | "system";
  content: string;
}

/** Model info returned by Ollama /api/tags. */
export interface OllamaModelInfo {
  name: string;
  size: number | null;
  digest: string | null;
  details: {
    family: string | null;
    parameter_size: string | null;
    quantization_level: string | null;
  } | null;
}

/** Recommended model definition from our static catalog. */
export interface RecommendedModel {
  ollama_name: string;
  display_name: string;
  description: string;
  parameters: string;
  size_mb: number;
  languages: string[];
}

/** Response from llm_list_models. */
export interface LlmInstalledModelsResponse {
  installed: OllamaModelInfo[];
  recommended: RecommendedModel[];
}

/** Progress event emitted during llm_pull_model. */
export interface PullProgressPayload {
  status: string;
  total: number | null;
  completed: number | null;
}

/** Response from llm_chat. */
export interface LlmChatResponse {
  content: string;
  prompt_tokens: number;
  completion_tokens: number;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
