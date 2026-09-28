"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Send, Bot, User, Loader2, Mic, Square, ChevronDown, ChevronUp, Wand2, ZoomIn, ZoomOut } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { startRecording, stopRecording, type VoiceState } from "@/lib/voice-input";
import {
  type ChatMessage,
  type OllamaModelInfo,
  type LlmChatResponse,
} from "@/lib/llm/types";
import {
  PROMPT_TEMPLATES,
  PROMPT_LANGUAGES,
  type PromptLanguage,
  buildPrompt,
} from "@/lib/llm/prompt-templates";

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

  // ---- Prompt builder state ----
  const [showPromptBuilder, setShowPromptBuilder] = useState(false);
  const [promptLanguage, setPromptLanguage] = useState<PromptLanguage>("en");
  const [selectedTemplate, setSelectedTemplate] = useState("");
  const [promptContent, setPromptContent] = useState("");

  // ---- Fetch installed models ----

  const fetchModels = useCallback(async () => {
    if (!isTauri()) return;
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
  }, [selectedModel]);

  useEffect(() => {
    fetchModels();
  }, [fetchModels]);

  // ---- Auto-scroll ----

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // ---- Send message ----

  async function handleSend() {
    if (!input.trim() || !selectedModel || loading) return;

    const userMessage: ChatMessage = { role: "user", content: input.trim() };
    const newMessages = [...messages, userMessage];
    setMessages(newMessages);
    setInput("");
    setLoading(true);

    try {
      const response = await invoke<LlmChatResponse>("llm_chat", {
        model: selectedModel,
        messages: newMessages,
        temperature,
      });
      setMessages([
        ...newMessages,
        { role: "assistant", content: response.content },
      ]);
    } catch (e) {
      console.error("Chat error:", e);
      setMessages([
        ...newMessages,
        {
          role: "assistant",
          content: `Error: ${e instanceof Error ? e.message : String(e)}`,
        },
      ]);
    } finally {
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

  return (
    <main className="flex-1 flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-border-default flex items-center gap-4">
        <h1 className="text-lg font-semibold flex-1">LLM Chat (Ollama)</h1>
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
        </div>
      </div>

      {/* Messages area */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {messages.length === 0 && (
          <div className="flex flex-col items-center justify-center h-full text-center text-text-tertiary">
            <Bot size={48} className="mb-4 opacity-50" />
            <p className="text-sm">
              {availableModels.length > 0
                ? "Start a conversation with your local AI model."
                : "Need to install and start Ollama first."}
            </p>
          </div>
        )}

        {messages.map((msg, i) => (
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
              {msg.content}
            </div>
            {msg.role === "user" && (
              <div className="w-8 h-8 rounded-full bg-bg-muted flex items-center justify-center shrink-0">
                <User size={16} className="text-text-secondary" />
              </div>
            )}
          </div>
        ))}

        {loading && (
          <div className="flex gap-3 justify-start">
            <div className="w-8 h-8 rounded-full bg-accent-bg/20 flex items-center justify-center shrink-0">
              <Bot size={16} className="text-accent-bg" />
            </div>
            <div className="px-4 py-2.5 rounded-2xl bg-bg-muted">
              <Loader2 size={16} className="animate-spin text-text-tertiary" />
            </div>
          </div>
        )}

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

        {/* Prompt builder panel */}
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
                <option value="">Select a template...</option>
                {PROMPT_TEMPLATES.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name[promptLanguage]}
                  </option>
                ))}
              </select>
            </div>

            {/* Content input */}
            <textarea
              value={promptContent}
              onChange={(e) => setPromptContent(e.target.value)}
              placeholder="Paste or type your content here..."
              className="w-full px-3 py-2 rounded-md bg-bg-input border border-border-default text-sm text-text-primary resize-none focus:outline-none focus:border-accent"
              rows={3}
            />

            {/* Preview + Insert button */}
            {selectedTemplate && promptContent && (
              <div className="space-y-2">
                <p className="text-xs text-text-tertiary">Preview:</p>
                <div className="p-2 rounded-md bg-bg-card border border-border-default text-xs text-text-secondary max-h-24 overflow-y-auto whitespace-pre-wrap">
                  {buildPrompt(selectedTemplate, promptLanguage, promptContent)}
                </div>
                <button
                  type="button"
                  onClick={() => {
                    const prompt = buildPrompt(selectedTemplate, promptLanguage, promptContent);
                    if (prompt) {
                      setInput(prompt);
                      inputRef.current?.focus();
                    }
                  }}
                  className="w-full px-3 py-1.5 rounded-md bg-accent text-white text-xs font-medium hover:opacity-90 transition-opacity"
                >
                  Insert into chat
                </button>
              </div>
            )}
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
                : "Install a model first to start chatting"
            }
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            rows={1}
            disabled={!selectedModel || loading}
          />
          <button
            className={`p-2.5 rounded-lg cursor-pointer transition-all ${
              voiceState === "recording"
                ? "bg-error-text text-white animate-pulse"
                : voiceState === "processing"
                  ? "bg-bg-muted text-text-tertiary cursor-wait"
                  : "bg-bg-muted text-text-secondary hover:bg-bg-hover"
            }`}
            onClick={handleVoiceToggle}
            disabled={voiceState === "processing" || !selectedModel}
            title={
              voiceState === "recording"
                ? "Stop recording"
                : voiceState === "processing"
                  ? "Transcribing..."
                  : "Voice input"
            }
          >
            {voiceState === "recording" ? (
              <Square size={18} />
            ) : (
              <Mic size={18} />
            )}
          </button>
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
