"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Globe, Loader2, Copy, Check } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import type { WebServiceConfig, WebServiceStatus } from "@/lib/web_service/types";

const DEFAULT_CONFIG: WebServiceConfig = {
  port: 8787,
  stt: true,
  dataset: true,
  tts: true,
};

export default function WebServicePage() {
  const [config, setConfig] = useState<WebServiceConfig>(DEFAULT_CONFIG);
  const [status, setStatus] = useState<WebServiceStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const s = await invoke<WebServiceStatus>("web_service_get_status");
      setStatus(s);
      setConfig((prev) => ({ ...prev, port: s.port }));
    } catch (e) {
      console.error("Failed to load web service status:", e);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const running = status?.running ?? false;

  async function handleStart() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const s = await invoke<WebServiceStatus>("web_service_start", { config });
      setStatus(s);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleStop() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const s = await invoke<WebServiceStatus>("web_service_stop");
      setStatus(s);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(text);
      setTimeout(() => setCopied(null), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }

  function setToggle(key: keyof WebServiceConfig, value: boolean) {
    setConfig((prev) => ({ ...prev, [key]: value }));
  }

  const enabledCount = [config.stt, config.dataset, config.tts].filter(Boolean).length;

  return (
    <main className="flex-1 flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-border-default flex items-center gap-3">
        <Globe size={20} className="text-text-secondary" />
        <h1 className="text-lg font-semibold flex-1">Web Service</h1>
        <span
          className={`inline-flex items-center gap-2 text-xs px-2.5 py-1 rounded-full border ${
            running
              ? "border-green-500/40 text-green-500 bg-green-500/10"
              : "border-border-default text-text-tertiary bg-bg-muted"
          }`}
        >
          <span
            className={`w-2 h-2 rounded-full ${running ? "bg-green-500" : "bg-text-tertiary"}`}
          />
          {running ? "Running" : "Stopped"}
        </span>
      </div>

      <div className="flex-1 overflow-y-auto p-4">
        <div className="max-w-3xl mx-auto space-y-6">
          {error && (
            <div className="px-4 py-3 rounded-lg bg-error-bg text-error-text text-sm">
              {error}
            </div>
          )}

          {/* Configuration */}
          <section className="rounded-xl border border-border-default bg-bg-card p-5 space-y-5">
            <div>
              <h2 className="text-sm font-semibold text-text-primary">Server</h2>
              <p className="text-xs text-text-tertiary mt-1">
                Binds to all interfaces (0.0.0.0), so other devices on your network can connect.
              </p>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium text-text-secondary" htmlFor="ws-port">
                Port
              </label>
              <input
                id="ws-port"
                type="number"
                min={1}
                max={65535}
                disabled={running}
                className="w-40 px-3 py-2 rounded-lg bg-bg-muted border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent disabled:opacity-60"
                value={config.port}
                onChange={(e) =>
                  setConfig((prev) => ({ ...prev, port: Number(e.target.value) || 0 }))
                }
              />
            </div>

            <div className="space-y-3">
              <p className="text-sm font-medium text-text-secondary">Services</p>
              <Toggle
                label="STT (Speech-to-Text)"
                description="Transcribe uploaded WAV audio using the selected downloaded model."
                checked={config.stt}
                disabled={running}
                onChange={(v) => setToggle("stt", v)}
              />
              <Toggle
                label="Dataset file server"
                description="Browse and download files inside your datasets from any browser."
                checked={config.dataset}
                disabled={running}
                onChange={(v) => setToggle("dataset", v)}
              />
              <Toggle
                label="TTS (Text-to-Speech)"
                description="Synthesize speech from text via Edge TTS and stream audio back."
                checked={config.tts}
                disabled={running}
                onChange={(v) => setToggle("tts", v)}
              />
            </div>

            <div className="flex items-center gap-3 pt-1">
              {!running ? (
                <button
                  onClick={handleStart}
                  disabled={busy || enabledCount === 0}
                  className="inline-flex items-center gap-2 px-4 py-2 rounded-lg bg-accent text-white text-sm font-medium disabled:opacity-50 disabled:cursor-not-allowed hover:opacity-90 transition-opacity"
                >
                  {busy && <Loader2 size={16} className="animate-spin" />}
                  Start server
                </button>
              ) : (
                <button
                  onClick={handleStop}
                  disabled={busy}
                  className="inline-flex items-center gap-2 px-4 py-2 rounded-lg bg-bg-muted border border-border-default text-text-primary text-sm font-medium disabled:opacity-50 hover:opacity-90 transition-opacity"
                >
                  {busy && <Loader2 size={16} className="animate-spin" />}
                  Stop server
                </button>
              )}
              {enabledCount === 0 && !running && (
                <span className="text-xs text-text-tertiary">
                  Select at least one service to start.
                </span>
              )}
              {running && (
                <span className="text-xs text-text-tertiary">
                  Stop the server to change port or services.
                </span>
              )}
            </div>
          </section>

          {/* Endpoints */}
          {running && status && (
            <section className="rounded-xl border border-border-default bg-bg-card p-5 space-y-5">
              <div>
                <h2 className="text-sm font-semibold text-text-primary">Endpoints</h2>
                <p className="text-xs text-text-tertiary mt-1">
                  Open these in a browser or call them from another app.
                </p>
              </div>

              <div className="space-y-2">
                {status.local_url && (
                  <UrlRow label="This device" url={status.local_url} onCopy={copy} copied={copied} />
                )}
                {status.lan_url && (
                  <UrlRow label="LAN address" url={status.lan_url} onCopy={copy} copied={copied} />
                )}
              </div>

              <div className="space-y-4 pt-1">
                {status.stt && (
                  <ServiceCard
                    title="STT"
                    items={[
                      { method: "GET", path: "/stt", note: "Browser test form" },
                      { method: "POST", path: "/stt", note: "multipart field 'file' = 16kHz mono WAV → { text }" },
                    ]}
                  />
                )}
                {status.dataset && (
                  <ServiceCard
                    title="Datasets"
                    items={[
                      { method: "GET", path: "/datasets", note: "JSON list of datasets" },
                      { method: "GET", path: "/datasets/{uuid}", note: "HTML file index" },
                      { method: "GET", path: "/datasets/{uuid}/list", note: "JSON file list" },
                      { method: "GET", path: "/datasets/{uuid}/file/{path}", note: "Download / stream a file" },
                    ]}
                  />
                )}
                {status.tts && (
                  <ServiceCard
                    title="TTS"
                    items={[
                      { method: "GET", path: "/tts/voices", note: "JSON list of voices" },
                      { method: "GET", path: "/tts?text=...&voice=...", note: "Stream audio/mpeg" },
                      { method: "POST", path: "/tts", note: "JSON { text, voice, rate?, volume?, pitch? } → audio/mpeg" },
                    ]}
                  />
                )}
              </div>
            </section>
          )}
        </div>
      </div>
    </main>
  );
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function Toggle({
  label,
  description,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label
      className={`flex items-start gap-3 p-3 rounded-lg border transition-colors ${
        checked ? "border-accent/50 bg-accent/5" : "border-border-default bg-bg-muted"
      } ${disabled ? "opacity-60 cursor-not-allowed" : "cursor-pointer"}`}
    >
      <input
        type="checkbox"
        className="mt-1 accent-accent"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="flex-1">
        <span className="block text-sm font-medium text-text-primary">{label}</span>
        <span className="block text-xs text-text-tertiary mt-0.5">{description}</span>
      </span>
    </label>
  );
}

function UrlRow({
  label,
  url,
  onCopy,
  copied,
}: {
  label: string;
  url: string;
  onCopy: (t: string) => void;
  copied: string | null;
}) {
  return (
    <div className="flex items-center gap-3">
      <span className="text-xs text-text-tertiary w-24 shrink-0">{label}</span>
      <a
        href={url}
        target="_blank"
        rel="noreferrer"
        className="text-sm text-accent hover:underline font-mono truncate"
      >
        {url}
      </a>
      <button
        onClick={() => onCopy(url)}
        className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-mid-gray/20 transition-colors shrink-0"
        title="Copy URL"
      >
        {copied === url ? <Check size={14} /> : <Copy size={14} />}
      </button>
    </div>
  );
}

function ServiceCard({
  title,
  items,
}: {
  title: string;
  items: { method: string; path: string; note: string }[];
}) {
  return (
    <div className="rounded-lg border border-border-default overflow-hidden">
      <div className="px-3 py-2 bg-bg-muted text-sm font-medium text-text-primary border-b border-border-default">
        {title}
      </div>
      <ul className="divide-y divide-border-default">
        {items.map((it, i) => (
          <li key={i} className="px-3 py-2 flex items-center gap-3 text-xs">
            <span className="font-mono px-1.5 py-0.5 rounded bg-bg-muted text-text-secondary border border-border-default shrink-0">
              {it.method}
            </span>
            <code className="font-mono text-text-primary shrink-0">{it.path}</code>
            <span className="text-text-tertiary truncate">{it.note}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
