"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Send, Bot, User, Loader2, ChevronDown, ChevronUp, Wand2, ZoomIn, ZoomOut, X } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { startRecording, stopRecording, type VoiceState } from "@/lib/voice-input";
import { VoiceMicButton } from "@/components/voice/VoiceMicButton";
import {
  type ChatMessage,
  type OllamaModelInfo,
  type LlmProvider,
  type LlmChatChunk,
  type LlmChatDone,
  type LlmChatError,
} from "@/lib/llm/types";
import {
  PROMPT_TEMPLATES,
  PROMPT_LANGUAGES,
  type PromptLanguage,
  buildSystemPrompt,
} from "@/lib/llm/prompt-templates";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Correlation id for a single streaming request. */
function newRequestId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `req-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function LLMChatPage() {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [selectedModel, setSelectedModelState] = useState("");

  // Wrap setter to persist to localStorage.
  const setSelectedModel = useCallback((model: string) => {
    setSelectedModelState(model);
    if (typeof window !== "undefined" && model) {
      localStorage.setItem("llm_selected_model", model);
    }
  }, []);

  // Restore cached model on mount (client-side only).
  useEffect(() => {
    const cached = localStorage.getItem("llm_selected_model");
    if (cached) setSelectedModelState(cached);
  }, []);
  const [availableModels, setAvailableModels] = useState<OllamaModelInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [voiceState, setVoiceState] = useState<VoiceState>("idle");
  const [voiceError, setVoiceError] = useState("");
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // ---- Font zoom ----
  const [fontSize, setFontSize] = useState(14);

  // ---- Temperature ----
  const [temperature, setTemperature] = useState(0.7);

  // ---- Provider config (from global settings) ----
  const [provider, setProvider] = useState<LlmProvider>("ollama");
  const [configuredModel, setConfiguredModel] = useState("");

  // ---- Prompt builder state (maps a template onto the system prompt) ----
  const [showPromptBuilder, setShowPromptBuilder] = useState(false);
  const [promptLanguage, setPromptLanguage] = useState<PromptLanguage>("en");
  const [selectedTemplate, setSelectedTemplate] = useState("");

  // The system prompt derived from the selected template (null = none).
  const activeSystem = selectedTemplate
    ? buildSystemPrompt(selectedTemplate, promptLanguage)
    : null;
  const activeTemplateName =
    PROMPT_TEMPLATES.find((t) => t.id === selectedTemplate)?.name[promptLanguage] ?? "";

  // ---- Load provider config from global settings ----

  const loadProviderConfig = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const g = await invoke<{
        llm_provider?: string;
        llm_model?: string;
      }>("settings_get_global");
      const p = ((g.llm_provider || "ollama") as LlmProvider);
      setProvider(p);
      setConfiguredModel(g.llm_model || "");
    } catch (e) {
      console.error("Failed to load provider config:", e);
    }
  }, []);

  useEffect(() => {
    loadProviderConfig();
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    listen("settings-changed", () => loadProviderConfig()).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [loadProviderConfig]);

  // ---- Fetch installed models (Ollama only) ----

  const fetchModels = useCallback(async () => {
    if (!isTauri() || provider !== "ollama") return;
    try {
      const running = await invoke<boolean>("llm_check_connection");
      if (!running) return;
      const res = await invoke<{
        installed: OllamaModelInfo[];
      }>("llm_list_models");
      setAvailableModels(res.installed);
      // Prefer cached model if it's still installed; otherwise first model.
      if (res.installed.length > 0) {
        const cached = localStorage.getItem("llm_selected_model");
        const stillInstalled = cached && res.installed.some((m) => m.name === cached);
        if (!selectedModel && !stillInstalled) {
          setSelectedModel(res.installed[0].name);
        }
      }
    } catch (e) {
      console.error("Failed to list models:", e);
    }
  }, [provider, selectedModel, setSelectedModel]);

  useEffect(() => {
    fetchModels();
  }, [fetchModels]);

  // For cloud providers, default the model to the configured value.
  useEffect(() => {
    if (provider !== "ollama" && !selectedModel && configuredModel) {
      setSelectedModel(configuredModel);
    }
  }, [provider, configuredModel, selectedModel, setSelectedModel]);

  // ---- Auto-scroll ----

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // ---- Send message (streaming) ----

  async function handleSend() {
    if (!input.trim() || !selectedModel || loading) return;

    const userMessage: ChatMessage = { role: "user", content: input.trim() };
    const history = [...messages, userMessage];
    const requestId = newRequestId();

    // Show the user message plus an empty assistant placeholder that fills as
    // tokens stream in.
    setMessages([...history, { role: "assistant", content: "" }]);
    setInput("");
    setLoading(true);

    // Append text to the trailing assistant placeholder.
    const appendChunk = (text: string) => {
      setMessages((prev) => {
        const copy = [...prev];
        const last = copy[copy.length - 1];
        if (last && last.role === "assistant") {
          copy[copy.length - 1] = { ...last, content: last.content + text };
        }
        return copy;
      });
    };
    const setAssistantError = (msg: string) => {
      setMessages((prev) => {
        const copy = [...prev];
        const last = copy[copy.length - 1];
        if (last && last.role === "assistant") {
          copy[copy.length - 1] = { ...last, content: `Error: ${msg}` };
        }
        return copy;
      });
    };

    const unlisteners: (() => void)[] = [];
    try {
      unlisteners.push(
        await listen<LlmChatChunk>("llm-chat-chunk", (e) => {
          if (e.payload.id !== requestId) return;
          appendChunk(e.payload.text);
        })
      );
      unlisteners.push(
        await listen<LlmChatDone>("llm-chat-done", (e) => {
          if (e.payload.id !== requestId) return;
          setLoading(false);
        })
      );
      unlisteners.push(
        await listen<LlmChatError>("llm-chat-error", (e) => {
          if (e.payload.id !== requestId) return;
          setAssistantError(e.payload.message);
          setLoading(false);
        })
      );

      await invoke("llm_chat_stream", {
        id: requestId,
        model: selectedModel,
        messages: history,
        system: activeSystem,
        temperature,
      });
    } catch (e) {
      console.error("Chat error:", e);
      setAssistantError(e instanceof Error ? e.message : String(e));
    } finally {
      unlisteners.forEach((fn) => fn());
      setLoading(false);
    }
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  // ---- Voice input ----

  async function handleVoiceToggle() {
    if (voiceState === "processing") return;
    if (voiceState === "recording") {
      stopRecording();
      return;
    }
    setVoiceError("");
    // Auto-load STT model if not running
    if (isTauri()) {
      try {
        const status = await invoke<{ active_version: string | null }>("model_get_status");
        if (!status.active_version) {
          setVoiceState("processing");
          await invoke("model_start");
        }
      } catch {
        setVoiceError("Failed to load STT model");
        return;
      }
    }
    await startRecording({
      onStateChange: setVoiceState,
      onResult: (text) => {
        setInput((prev) => (prev ? prev + " " : "") + text.trim());
        inputRef.current?.focus();
        setVoiceState("idle");
      },
      onError: (err) => {
        setVoiceError(err);
        setVoiceState("idle");
      },
    });
  }

  // ---- Render ----

  const modelReady = provider === "ollama" ? availableModels.length > 0 : !!selectedModel;

  return (
    <main className="flex-1 flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-border-default flex items-center gap-4">
        <h1 className="text-lg font-semibold flex-1 flex items-center gap-2">
          LLM Chat
          <span className="text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 rounded bg-bg-muted border border-border-default text-text-tertiary">
            {provider}
          </span>
        </h1>
        <div className="flex items-center gap-3">
          {/* Temperature control */}
          <div className="flex items-center gap-2" title="Temperature: controls response randomness">
            <span className="text-xs text-text-tertiary">Temp:</span>
            <div className="flex gap-0.5">
              {[0, 0.3, 0.7, 1.0].map((t) => (
                <button
                  key={t}
                  type="button"
                  onClick={() => setTemperature(t)}
                  className={`px-2 py-1 text-xs rounded transition-colors ${
                    temperature === t
                      ? "bg-accent text-white"
                      : "bg-bg-muted text-text-secondary hover:bg-bg-hover"
                  }`}
                >
                  {t}
                </button>
              ))}
            </div>
          </div>
          {/* Font zoom controls */}
          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={() => setFontSize((s) => Math.max(10, s - 2))}
              disabled={fontSize <= 10}
              className="p-1.5 rounded-lg bg-bg-muted border border-border-default text-text-secondary hover:bg-bg-hover transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              title="Zoom out"
            >
              <ZoomOut size={14} />
            </button>
            <span className="text-xs text-text-tertiary w-8 text-center">{fontSize}</span>
            <button
              type="button"
              onClick={() => setFontSize((s) => Math.min(32, s + 2))}
              disabled={fontSize >= 32}
              className="p-1.5 rounded-lg bg-bg-muted border border-border-default text-text-secondary hover:bg-bg-hover transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              title="Zoom in"
            >
              <ZoomIn size={14} />
            </button>
          </div>
          {provider === "ollama" ? (
            <select
              className="px-3 py-1.5 text-sm rounded-lg bg-bg-muted border border-border-default text-text-primary cursor-pointer"
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
            >
              {availableModels.length === 0 && (
                <option value="">No models installed</option>
              )}
              {availableModels.map((m) => (
                <option key={m.name} value={m.name}>
                  {m.name}
                </option>
              ))}
            </select>
          ) : (
            <input
              className="px-3 py-1.5 text-sm rounded-lg bg-bg-muted border border-border-default text-text-primary w-48"
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
              placeholder="Model name"
            />
          )}
        </div>
      </div>

      {/* Messages area */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {messages.length === 0 && (
          <div className="flex flex-col items-center justify-center h-full text-center text-text-tertiary">
            <Bot size={48} className="mb-4 opacity-50" />
            <p className="text-sm">
              {modelReady
                ? "Start a conversation with your AI model."
                : provider === "ollama"
                ? "Need to install and start Ollama first."
                : "Set a model name (and API key in Settings) to start chatting."}
            </p>
          </div>
        )}

        {messages.map((msg, i) => {
          const isLast = i === messages.length - 1;
          const streamingEmpty = msg.role === "assistant" && msg.content === "" && loading && isLast;
          return (
            <div
              key={i}
              className={`flex gap-3 ${msg.role === "user" ? "justify-end" : "justify-start"}`}
            >
              {msg.role === "assistant" && (
                <div className="w-8 h-8 rounded-full bg-accent-bg/20 flex items-center justify-center shrink-0">
                  <Bot size={16} className="text-accent-bg" />
                </div>
              )}
              <div
                className={`max-w-[70%] px-4 py-2.5 rounded-2xl whitespace-pre-wrap ${
                  msg.role === "user"
                    ? "bg-accent-bg text-white"
                    : "bg-bg-muted text-text-primary"
                }`}
                style={{ fontSize: `${fontSize}px` }}
              >
                {streamingEmpty ? (
                  <Loader2 size={16} className="animate-spin text-text-tertiary" />
                ) : (
                  msg.content
                )}
              </div>
              {msg.role === "user" && (
                <div className="w-8 h-8 rounded-full bg-bg-muted flex items-center justify-center shrink-0">
                  <User size={16} className="text-text-secondary" />
                </div>
              )}
            </div>
          );
        })}

        <div ref={messagesEndRef} />
      </div>

      {/* Input area */}
      <div className="p-4 border-t border-border-default">
        {/* Prompt builder toggle */}
        <button
          type="button"
          onClick={() => setShowPromptBuilder(!showPromptBuilder)}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 mb-3 rounded-lg bg-bg-muted border border-border-default text-xs font-medium text-text-secondary hover:bg-bg-hover transition-colors"
        >
          <Wand2 size={14} />
          Prompt Builder
          {showPromptBuilder ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
        </button>

        {/* Prompt builder panel — maps a template onto the system prompt */}
        {showPromptBuilder && (
          <div className="mb-3 p-3 rounded-lg bg-bg-muted border border-border-default space-y-3">
            {/* Language + Template row */}
            <div className="flex flex-wrap gap-2">
              <select
                value={promptLanguage}
                onChange={(e) => setPromptLanguage(e.target.value as PromptLanguage)}
                className="px-2 py-1.5 rounded-md bg-bg-input border border-border-default text-xs text-text-primary"
              >
                {PROMPT_LANGUAGES.map((lang) => (
                  <option key={lang.value} value={lang.value}>
                    {lang.label}
                  </option>
                ))}
              </select>
              <select
                value={selectedTemplate}
                onChange={(e) => setSelectedTemplate(e.target.value)}
                className="flex-1 px-2 py-1.5 rounded-md bg-bg-input border border-border-default text-xs text-text-primary"
              >
                <option value="">No template (plain chat)</option>
                {PROMPT_TEMPLATES.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name[promptLanguage]}
                  </option>
                ))}
              </select>
            </div>

            <p className="text-xs text-text-tertiary">
              The template becomes the model&apos;s system prompt. Type the text you want
              worked on in the chat box below — it is sent as your message.
            </p>

            {/* System prompt preview */}
            {activeSystem && (
              <div className="space-y-1">
                <p className="text-xs text-text-tertiary">System prompt:</p>
                <div className="p-2 rounded-md bg-bg-card border border-border-default text-xs text-text-secondary max-h-24 overflow-y-auto whitespace-pre-wrap">
                  {activeSystem}
                </div>
              </div>
            )}
          </div>
        )}

        {/* Active system prompt chip */}
        {activeSystem && !showPromptBuilder && (
          <div className="mb-2 inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-accent-bg/15 border border-accent-bg/30 text-xs text-text-secondary max-w-full">
            <Wand2 size={12} className="shrink-0 text-accent-bg" />
            <span className="truncate">{activeTemplateName}</span>
            <button
              type="button"
              onClick={() => setSelectedTemplate("")}
              className="shrink-0 hover:text-text-primary"
              title="Clear system prompt"
            >
              <X size={12} />
            </button>
          </div>
        )}

        {voiceError && (
          <p className="text-xs text-error-text mb-2">{voiceError}</p>
        )}
        <div className="flex gap-2 items-end">
          <textarea
            ref={inputRef}
            className="flex-1 px-4 py-2.5 rounded-lg bg-bg-muted border border-border-default text-text-primary text-sm resize-none focus:outline-none focus:border-accent"
            placeholder={
              selectedModel
                ? "Type a message... (Enter to send, Shift+Enter for newline)"
                : provider === "ollama"
                ? "Install a model first to start chatting"
                : "Set a model name to start chatting"
            }
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            rows={1}
            disabled={!selectedModel || loading}
          />
          <VoiceMicButton
            state={voiceState}
            surface="composer"
            onPress={handleVoiceToggle}
            disabled={!selectedModel}
          />
          <button
            className="p-2.5 rounded-lg bg-accent-bg text-white hover:opacity-90 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-opacity"
            onClick={handleSend}
            disabled={!input.trim() || !selectedModel || loading}
          >
            <Send size={18} />
          </button>
        </div>
      </div>
    </main>
  );
}
