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

/** Response from llm_list_models. */
export interface LlmInstalledModelsResponse {
  installed: OllamaModelInfo[];
}

/** Response from llm_chat. */
export interface LlmChatResponse {
  content: string;
  prompt_tokens: number;
  completion_tokens: number;
}

// ---------------------------------------------------------------------------
// Provider layer
// ---------------------------------------------------------------------------

/** Inference provider selected in global settings. */
export type LlmProvider =
  | "ollama"
  | "openai"
  | "anthropic"
  | "groq"
  | "databricks";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
