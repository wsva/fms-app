# How the goose GDK could help fms-app

> Report generated from the goose Development Kit docs (https://goose-docs.ai/docs/gdk/)
> — Overview, SDK, ACP, and the generated goose ACP Reference — cross-checked against
> this project's current architecture (`src-tauri/src/mcp/`, the Ollama-based
> `ChatPage.tsx`, the STT `models/mod.rs`, and the Android thin-client `sync/client.rs`).

## 1. What the GDK actually is (and what it isn't)

The **goose Development Kit** is not a single SDK — it's **two integration surfaces**
around the same goose agent runtime:

| Surface | What it gives you | Language | Maturity |
|---|---|---|---|
| **SDK (`goose-sdk`)** | In-process library: build a provider, stream completions, emit/parse tool calls, **compact long conversations**, capture request logs. One Rust crate; Python + Kotlin generated via UniFFI. | Rust / Python / Kotlin | **Alpha** (0.x, surface may break) |
| **ACP (Agent Client Protocol)** | Connect to a *separate* goose process over **stdio / HTTP / WebSocket**. Full agent runtime: sessions, extensions (MCP), tools, memory, recipes, schedules, dictation, local inference. | Any (JSON-RPC) | Stable-ish, `unstable/` namespace |

Key point: **fms-app already speaks both sides of this.** The Rust backend already runs
an **MCP server** (`rmcp` StreamableHttp in `mcp/`) — that's the *extension/tool* side.
What the GDK adds is the *agent/orchestration* side that is currently hand-rolled against
Ollama.

## 2. Where it maps onto the current code

### A. Replace the hand-rolled chat/provider layer → `goose-sdk`

Today `ChatPage.tsx` + the Ollama helpers implement a bespoke single-provider loop
(temperature, streaming, model list, vision OCR). The SDK's provider layer gives you,
for free:

- **Multi-provider access** (OpenAI, Anthropic, Groq, Databricks, or *any*
  OpenAI/Anthropic-compatible endpoint via a JSON "declarative provider") — this is
  exactly how you'd point at Ollama/local models without a custom client, and suddenly
  support cloud models too.
- **Structured streaming** with typed chunks (`TextChunk`, `ToolChunk`, `ThinkingChunk`,
  `EndChunk` w/ usage, `ErrorChunk`) — cleaner than the current string streaming.
- **Context compaction** — summarize a conversation to continue past the window. Directly
  useful for long tutoring/chat sessions.

Effort: moderate, contained. Risk: alpha API churn (pin exact version).

### B. Turn fms-app into an **ACP client** to embed a real agent

This is the bigger unlock and aligns with the stated **"agent-friendly design" principle**
(`docs/agent_friendly_design.md`). Instead of only *exposing* tools for an external agent
to drive, you could **embed goose's agent loop inside the app**:

- Spawn `goose acp` (stdio) or connect to `goose serve` (WebSocket/HTTP with
  `X-Secret-Key`) and drive it from a Rust/TS client.
- The existing MCP server then becomes a **goose extension** — every tool already built
  (dataset/wiki/card/log) is instantly usable by a full agent with memory +
  tool-permissions + sessions, no glue code.
- ACP gives you session lifecycle (`session/*`), **tool permissions**
  (`tools/permissions/set`), **steering** an in-flight prompt (`session/steer`), and
  **system-prompt append** — features you'd otherwise re-implement.

### C. Recipes + Schedules = automation/XP pipelines

The ACP surface has `recipes/*` (save/list/encode-as-deeplink/slash-command) and
`schedules/*` (cron jobs, run-now, pause, inspect running job). The `docs/todo.md`
"read aloud/read" scoring flow and daily-review ideas are exactly "a repeatable
multi-step agent job" — a recipe with a schedule, rather than bespoke UI.

### D. Notable overlap: goose's own **dictation & local-inference** ACP APIs

The reference exposes `dictation/transcribe`, `dictation/models/download|list|delete`,
and `local-inference/models/*` (HF search/download/evict, per-model sampling settings).
This is functionally what `models/mod.rs` + `models/download.rs` do for STT. Worth studying
their API shape (and possibly consuming a provider) rather than reinventing model
management — though don't assume drop-in for the Parakeet/`transcribe-rs` stack.

### E. Android thin-client architecture — a strong precedent, not a dependency

The PC↔phone sync (`sync/client.rs`, REST + device pairing + token auth) is architecturally the
**same pattern** goose uses for `goose serve` over HTTP/WebSocket with a secret key and
`--allowed-origin`. Two implications:

- **Validation:** the design is sound and mirrors a mainstream approach.
- **Opportunity:** a paired phone could connect to the PC's `goose serve` WebSocket and
  run agent sessions remotely — the PC is already the "source of truth" for
  wiki/datasets, so it's a natural agent host too.

## 3. Honest caveats

- **Alpha SDK** — pin an exact `0.x`; expect breaking changes; check the API-reference
  version selector on upgrade.
- **ACP = a separate process/binary** (`goose` CLI). Bundling it into a Tauri desktop app
  is fine; on **Android it's a real question** — the thin client already gates out
  native/heavy deps, and shipping a goose runtime there is unlikely. Treat ACP as a
  **desktop/PC-side** capability.
- **Licensing & footprint** — Apache-2.0, Rust, adds a nontrivial dependency tree
  (tokio/axum/etc.) to `src-tauri`.
- **Don't rewrite what works** — the MCP tool layer is a *strength*; the GDK *consumes*
  it, it doesn't replace it.

## 4. Recommended path (lowest risk → highest payoff)

1. **Now / cheap:** Adopt `goose-sdk`'s **provider layer** to replace the Ollama-only chat
   client → multi-provider + streaming + compaction, desktop only. Keep everything else.
2. **Medium:** Add an **ACP client** on the PC side that connects to the *existing* MCP
   server as an extension — gives sessions, tool permissions, and steering "for free" and
   makes the agent-friendly principle real rather than aspirational.
3. **Exploratory:** Prototype **recipes/schedules** for the read-aloud scoring +
   daily-review workflows in `todo.md`.
4. **Research only:** Compare goose's `dictation/*` and `local-inference/*` ACP APIs
   against `models/mod.rs` before investing further in custom STT model management.
