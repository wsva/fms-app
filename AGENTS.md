# AGENTS.md

This file provides guidance to AI coding assistants working with code in this repository.

## Project Overview

fms-app is a desktop application for language learning through dictation and listening exercises. Users import datasets containing audio files, generate subtitles via STT models, and practice dictation by transcribing what they hear.

The project is a Tauri v2 desktop port of a Next.js web application (reference implementation at `learning/fms/`, a git clone of https://github.com/wsva/fms).

## Development Commands

**Prerequisites:** Rust (latest stable), pnpm, Node.js 20+

```bash
# Install dependencies
pnpm install

# Full-stack development (Tauri + Next.js)
pnpm tauri dev

# Frontend only (Next.js dev server on port 30000)
pnpm dev

# Production build
pnpm tauri build

# Rust compilation check
cd src-tauri && cargo check --offline

# TypeScript type check
npx tsc --noEmit
```

**Important:** The Next.js dev server runs on **port 30000** (not the default 3000). Tauri's `devUrl` is configured to match. A stale Next.js process on port 30000 will cause Tauri to fail to start the frontend.

## Architecture Overview

### Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Tauri 2.x, rusqlite (bundled SQLite) |
| Frontend | React 19, Next.js 15 (App Router, static export) |
| UI | HeroUI v3 (`@heroui/react`) |
| Styling | Tailwind CSS v4 with CSS custom properties |
| State | React hooks + useImmer |
| Package manager | pnpm |

### Backend Structure (`src-tauri/src/`)

Modules are grouped by domain. Each group's `mod.rs` declares its children and
carries their `#[cfg(feature = "...")]` gates, so `lib.rs` only gates a group when
the *whole* group is gated (`models`). The `datasets/` tree mirrors the on-disk
`<datasets_dir>/{dictation,card,book,read_aloud}/` layout.

```
src-tauri/src/
├── lib.rs                  # Tauri setup, plugin + command registration (desktop & mobile run())
├── main.rs
│
├── datasets/               # One submodule per dataset type + the shared core
│   ├── mod.rs              # dataset_roots()/DatasetType/meta.json discovery, dataset CRUD,
│   │                       #   subtitle generation, waveform generation (Symphonia peaks), DB generation
│   ├── dictation/
│   │   ├── mod.rs          # Dictation commands: media/subtitle/cue queries, progress (app-level DB)
│   │   ├── align.rs        # Cue alignment (multi-pass anchor DP): `dataset_align_cues` (shared
│   │   │                   #   `book.txt`/`book_sentences.txt`) + `dataset_align_cues_transcript` — desktop
│   │   ├── audit.rs        # Read-only dataset audit: 14 id-keyed checks over media/, subtitle/, waveform/, cues
│   │   │                   #   (`dataset_audit`; its check ids are the vocabulary `verify:` binds against) — desktop
│   │   └── adjust.rs       # Cue-time adjustment: energy envelope, silence snapping — desktop
│   ├── cards/
│   │   ├── mod.rs          # Card CRUD, tags, SM-2 review, FTS5 search, multi-location discovery
│   │   └── sync.rs         # Bidirectional card dataset sync (last-write-wins, per-dataset sync_state)
│   ├── book.rs             # Book chapters/sentences/words and their audio
│   ├── read_aloud.rs       # Read-aloud datasets: recorded takes, STT scoring, XP awards
│   ├── tools.rs            # Book splitting (`dataset_parse_book`, Rust or bundled `split_book.py`
│   │                       #   via NLTK), Python script execution (`write_transcripts.py`) — desktop
│   └── textsim.rs          # Ratcliff-Obershelp similarity, shared by align + read_aloud
│
├── models/                 # Whole group is `stt`-gated
│   ├── mod.rs              # STT model management: download, status, transcription via `transcribe-rs`
│   ├── download.rs         # Streaming model download with progress events
│   ├── catalog.rs          # HuggingFace catalog of non-STT models (OCR, TTS)
│   ├── catalog_stt.rs      # STT model catalog
│   └── index.rs            # Unified `{model_root}/index.json` over every downloaded model
│
├── sync/                   # Cross-device sync; owns `PROTOCOL_VERSION`
│   ├── mod.rs              # Group declarations + wire protocol version
│   ├── client.rs           # Snapshot pull + writeback queue (all platforms; the Android thin client)
│   ├── discover.rs         # PC auto-discovery: UDP probe, multicast, Tailscale
│   ├── change_log.rs       # Hub-side append-only change journal (`sync_log` table in the app DB)
│   ├── server.rs           # PC axum HTTP service hosting REST + MCP, enforces trust zones — desktop
│   ├── rest.rs             # `/api/v1` dataset snapshot + batched-writeback endpoints — desktop
│   └── pairing.rs          # Ed25519 device pairing and trust zones — desktop
│
├── ai/
│   ├── mod.rs
│   ├── llm.rs              # Ollama: connection check, model list/pull/delete, chat + streaming
│   ├── chat.rs             # Cross-device chat thread persisted on the PC hub
│   ├── goose_llm.rs        # Goose SDK LLM bridge — desktop
│   └── agent_acp.rs        # Goose ACP client over WebSocket to `goose serve` — desktop
│
├── mcp/                    # Built-in MCP server (rmcp), nested at `/mcp` — desktop
│   ├── mod.rs              # `DatasetMcpServer`, params shared by 2+ domains, `tool_router()`
│   │                       #   merging the domain routers, `ServerHandler` impl, `create_mcp_service()`
│   ├── datasets.rs         # dataset discovery/CRUD + subtitle & waveform pipeline (24 tools)
│   │                       #   `dataset_audit` is subject-only — per-step reading is `workflow_verify`/`workflow_adopt`
│   ├── dictation.rs        # cue adjust/align, dictation progress, favourites, subtitle versions (15)
│   ├── cards.rs            # card CRUD, tags, FTS5 search, SM-2 review, online sync (24)
│   ├── books.rs            # book chapters/sentences/words + read-aloud texts & scoring (24)
│   ├── models.rs           # STT model status/download/load/default/delete/transcribe (7)
│   ├── ai.rs               # Ollama chat & model management, Edge TTS, cross-device chat (10)
│   ├── wiki.rs             # wiki dirs, read/write/delete, search, index (9)
│   ├── sync.rs             # web service, pairing registry, PC scan/connect/pull, incremental sync (15)
│   ├── system.rs           # settings, auth, logs, OCR, screenshot, app UI control (15)
│   └── workflow.rs         # persistent workflow state machine: list/create/status/get_definition/next/advance/
│   │                       #   record/intervene/verify/adopt/builtin_templates (11)
│
├── workflow/              # Persistent workflow state machine (file-based, resumable DAG) — desktop
│   ├── mod.rs             # group root: the `run_base()` storage seam + re-export the run-scoped API
│   ├── commands.rs        # the `workflow_*` Tauri command twins (the page drives the same core fns MCP does)
│   ├── templates.rs       # built-in definitions baked in via `include_str!` (`templates/<kind>/*.yaml`)
│   ├── verify.rs          # the binding layer: check vocabulary + evidence providers; owns the verify and adopt joins
│   └── core/              # domain-free framework (steps/deps/statuses, not what a step does)
│       ├── mod.rs         # schema (Definition/RunState/Event) + public API (list/create/status/next/advance/record/intervene/verify/adopt)
│       ├── validate.rs    # pre-run structural validation: unique ids, refs exist, acyclic, `verify:`/`adopt:` ids registered
│       ├── engine.rs      # readiness recompute, transitions, retry/propagation, lease recovery, data flow
│       └── persist.rs     # run-dir resolution, YAML/JSON parse, atomic state write, append-only events
│
├── app_paths.rs            # Platform-correct base dirs (desktop `dirs`, mobile app-private)
├── settings.rs             # Two-tier settings: global JSON + per-workspace JSON overlay
├── workspace.rs            # Workspace registry, selection, claim/auto-login
├── auth.rs                 # OAuth2 login, token storage, workspace identity
├── logger.rs               # Rotating file log + in-memory buffer + `log_*` commands
├── db.rs                   # rusqlite wrappers with write-contention logging, adjustment guard
├── audio.rs                # Audio decoding via Symphonia
├── wiki.rs                 # Wiki documents; own root + `meta.json` linkage (not a dataset type)
├── xp.rs                   # XP ledger in the app-level SQLite
├── simple_words.rs         # "Known simple" words sourced from selected card datasets
├── edge_tts.rs             # Edge TTS voices + synthesis
├── capture.rs              # Screenshot / clipboard image capture — desktop
└── ocr.rs                  # Tesseract OCR — desktop
```

Feature gates: `default = ["stt"]`, `desktop = ["stt", ..., "server"]`. Android builds with
`--no-default-features --features stt`, which is the same module set as a plain `cargo check`.
Verify both: `cargo check --offline` and `cargo check --offline --features desktop`.

### Frontend Structure (`src/`)

```
src/
├── app/
│   ├── layout.tsx          # Root layout, theme init script, globals.css import
│   ├── page.tsx            # Single-page shell with sidebar tab navigation
│   ├── globals.css         # Theme variables (4 themes), Tailwind v4 @theme block
│   ├── datasets/           # (unused route, datasets rendered in page.tsx)
│   └── settings/           # (unused route, settings rendered in page.tsx)
├── components/pages/
│   ├── datasets.tsx        # Dataset import, management, pipeline stages
│   ├── dictation.tsx       # Dictation practice page (media player, waveform, cue cards)
│   ├── models.tsx          # STT model download and management
│   ├── settings.tsx        # App settings (dataset directory, model directory)
│   └── listen/
│       ├── CueEditor.tsx   # Cue editing component (dictation + edit modes, time editor)
│       ├── Subtitle.tsx    # Subtitle display component
│       └── WaveformCanvas.tsx  # Canvas-based waveform visualization
└── lib/
    ├── types.ts            # Shared types: Cue, ListenMedia, ListenSubtitle, etc.
    └── listen/             # Dictation utilities (utils.ts, subtitle parsing, lcs.ts)
```

### Navigation Pattern

The app uses a **single-page shell** (`src/app/page.tsx`) with sidebar tabs. Pages are React components rendered with `display: none/flex` toggling — not Next.js routes. All page components live in `src/components/pages/`.

### Dataset Structure

Datasets are directories on the filesystem. Each dataset contains:

```
<dataset>/
├── info.json          # Metadata: uuid, name, description, version, structure
├── data.sqlite3       # SQLite DB: listen_media, listen_subtitle, listen_subtitle_cue
├── media/             # Audio/video files (mp3, wav, m4a, m4b, ogg, flac, mp4, webm)
├── subtitle/          # VTT subtitle files (one per media)
├── waveform/          # JSON waveform files (one per media, generated in-app via Symphonia)
├── book.txt           # (optional) Full text for reference/alignment
└── transcript/        # (optional) Transcript text files
```

### Database Architecture

Two separate SQLite databases:

1. **Dataset DB** (`<dataset>/data.sqlite3`) — media, subtitles, cues per dataset
2. **App DB** (`dirs::data_dir()/fms-app/app.sqlite3`) — dictation progress (cross-dataset)

Key tables in dataset DB: `listen_media`, `listen_subtitle`, `listen_subtitle_cue`
Key table in app DB: `listen_dictation`

### Dataset Processing Pipeline

Datasets go through staged processing (triggered from the Datasets page):

1. **Import** → read `info.json`, register dataset
2. **Generate Subtitles** → STT transcription of media files → VTT files
3. **Generate Waveforms** → Symphonia peak detection (`audio::generate_waveform`) → JSON waveform files
4. **Generate Database** → parse subtitles, write cues to `data.sqlite3`

### Theme System

Four themes: Light (default), Dark, Solarized, Gruvbox. Implemented via:

- CSS custom properties defined in `globals.css` under `[data-theme="..."]` selectors
- Tailwind v4 `@theme` block maps CSS vars to utility classes (e.g., `--color-bg-card` → `bg-bg-card`)
- Theme persisted in `localStorage` under key `theme`
- Theme init script in `layout.tsx` `<head>` prevents flash of wrong theme

When adding new colors, define the CSS variable in all 4 theme blocks in `globals.css` AND add it to the `@theme` block.

### HeroUI v3 Notes

- **No Provider needed** — HeroUI v3 does not use a context provider
- **CSS import required** — `@import "@heroui/react/styles"` in `globals.css` for component styling
- **Tabs API** — uses `Tabs.ListContainer`, `Tabs.List`, `Tabs.Tab`, `Tabs.Panel` sub-components
- Component sub-properties: `ListContainer`, `List`, `Panel` (not `TabListContainer`/`TabPanel`)

## Key Conventions

### Rust Backend

- Commands use `State<'_, SettingsState>` to access settings and resolve dataset paths
- `datasets::find_dataset_dir()` resolves the filesystem path from settings + dataset UUID
- Error handling: return `Result<T, String>` — errors become frontend toast messages
- `open_db()` opens dataset-level SQLite; `open_app_db()` opens the app-level SQLite
- Inner functions are extracted for reuse across commands (see Tauri command extraction pattern)
- New files go into the matching group directory, not the crate root; a group's children are
  declared `pub(crate) mod` in its `mod.rs` (a private `mod` would be invisible outside the group)
- A directory earns its place only with 2+ files or a planned split — don't wrap a single file in
  a `mod.rs` (this is why `wiki.rs`, `xp.rs`, `ocr.rs` are still flat)
- `mcp/` is the exception to the `pub(crate) mod` rule: its submodules are plain private `mod`s,
  since nothing outside the group names them — it only exposes `create_mcp_service`

### Frontend

- Guard all Tauri API calls with `isTauri()` runtime check
- Use `invoke<ReturnType>("command_name", { params })` for IPC
- State management: `useState` for simple state, `useImmer` for complex nested state (cues)
- Path alias: `@/` maps to `./src/`
- File paths in Tauri use `convertFileSrc()` for asset protocol URLs
- Flex layouts that need overflow scrolling require `min-h-0` on flex children

### Adding a New Tauri Command

1. Write the function in the appropriate module under `src-tauri/src/` (see the group tree above)
   with `#[tauri::command]`
2. Register it in `src-tauri/src/lib.rs` inside `tauri::generate_handler![...]` — in **both** the
   desktop `run()` and, if it compiles on Android, the `mobile_invoke_handler!` list
3. Call it from frontend with `invoke("command_name", { params })`

## Agent-Friendly Design Principle

> **Design an app that is not only user-friendly, but also agent-friendly.**

This is a first-class design principle. Every feature must be MCP-accessible: if a human can do it in the UI, an agent must be able to do it through MCP. Full spec: [`docs/agent_friendly_design.md`](./docs/agent_friendly_design.md).

Key rules:
- **Structured JSON responses** for all MCP tools (not plain text)
- **Backup before destroy** — trash mechanism instead of confirmation parameters
- **Workflow tools over atomic tools** — group related operations into meaningful workflows
- **Actionable errors** — tell the agent what to do next
- **Status/discovery tools** — agents need to determine state before acting

When adding a new Tauri command, simultaneously add the corresponding MCP tool.

### Adding an MCP Tool

1. Add the parameter struct and the `#[tool(name = "...", description = "...")]` method to the
   `src-tauri/src/mcp/<domain>.rs` submodule that owns the domain (see the tree above)
2. Nothing else to register: each submodule's `#[tool_router(router = <domain>_router, vis = "pub(crate)")]`
   impl block is already merged by `mcp::tool_router()`. Only a **new** submodule needs a `mod`
   declaration plus a `+ Self::<domain>_router()` line in `mcp/mod.rs`
3. Tool names must be unique across all submodules — `ToolRouter::merge` silently overwrites a
   duplicate, so a collision costs you a tool with no compile error
4. A parameter struct used by two domains belongs in `mcp/mod.rs` as `pub(crate) struct`, imported
   via `use super::{...}`

## External Dependencies

- **transcribe-rs** — Rust ONNX runtime for STT model inference (Parakeet family)
- **symphonia** — Audio decoding (all formats) and waveform peak generation (replaces the former `audiowaveform` sidecar)
- **rusqlite** — SQLite with bundled mode (no system SQLite required)
