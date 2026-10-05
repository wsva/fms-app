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
// goose-sdk provider layer (chat streaming)
// ---------------------------------------------------------------------------

/** Inference provider selected in global settings. */
export type LlmProvider =
  | "ollama"
  | "openai"
  | "anthropic"
  | "groq"
  | "databricks";

/** A streamed text delta emitted as the `llm-chat-chunk` event payload. */
export interface LlmChatChunk {
  id: string;
  text: string;
}

/** Final token usage emitted as the `llm-chat-done` event payload. */
export interface LlmChatDone {
  id: string;
  prompt_tokens: number;
  completion_tokens: number;
}

/** Mid-stream failure emitted as the `llm-chat-error` event payload. */
export interface LlmChatError {
  id: string;
  message: string;
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
