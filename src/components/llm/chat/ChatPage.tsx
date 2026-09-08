"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Send, Bot, User, Loader2, Mic, Square } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { startRecording, stopRecording, type VoiceState } from "@/lib/voice-input";
import {
  type ChatMessage,
  type OllamaModelInfo,
  type LlmChatResponse,
} from "@/lib/llm/types";

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function LLMChatPage() {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [selectedModel, setSelectedModel] = useState("");
  const [availableModels, setAvailableModels] = useState<OllamaModelInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [voiceState, setVoiceState] = useState<VoiceState>("idle");
  const [voiceError, setVoiceError] = useState("");
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

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
      // Auto-select first model if none selected
      if (res.installed.length > 0 && !selectedModel) {
        setSelectedModel(res.installed[0].name);
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
        <h1 className="text-lg font-semibold flex-1">LLM Chat</h1>
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

      {/* Messages area */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {messages.length === 0 && (
          <div className="flex flex-col items-center justify-center h-full text-center text-text-tertiary">
            <Bot size={48} className="mb-4 opacity-50" />
            <p className="text-sm">
              {availableModels.length > 0
                ? "Start a conversation with your local AI model."
                : "Install a model from the Models tab to start chatting."}
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
              className={`max-w-[70%] px-4 py-2.5 rounded-2xl text-sm whitespace-pre-wrap ${
                msg.role === "user"
                  ? "bg-accent-bg text-white"
                  : "bg-bg-muted text-text-primary"
              }`}
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
