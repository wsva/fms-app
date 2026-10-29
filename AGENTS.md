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
│   ├── mod.rs              # Group root: the stage-module declarations, the re-export facade the rest
│   │                       #   of the crate names (`datasets::find_dataset_dir`, `datasets::DatasetInfo`),
│   │                       #   and the one cross-stage command (`dataset_advance_to_stage2`)
│   ├── roots.rs            # `DatasetType`, where each type lives (`meta.json` linked dirs, download
│   │                       #   roots), the `dataset_*_dir` commands
│   ├── registry.rs         # Dataset discovery + the summary/detail DTOs, import/update/delete, the
│   │                       #   base/overlay handover (`dataset_layer_*`)
│   ├── paths.rs            # uuid → directory: base (read-only) vs writable overlay, and the overlay fork
│   ├── media.rs            # Media enumeration, the `subtitle/`+`waveform/` mirror paths, trash, the
│   │                       #   create/init/sync commands, and the favourites dataset
│   ├── subtitle.rs         # Stage 2a: STT pass into `subtitle/*.vtt` and the VTT sentence shaping that
│   │                       #   decides cue boundaries (desktop parts `cfg`-gated)
│   ├── waveform.rs         # Stage 2b: symphonia peaks into `waveform/*.json`, which the dictation page
│   │                       #   draws without decoding the audio again
│   ├── db.rs               # Stage 3: the `data.sqlite3` schema, VTT parsing, cue generation
│   ├── ids.rs              # The `<type-slug>_<family-key>` id grammar: mint/split/family helpers that link the datasets of one work (no storage; advisory only)
│   ├── info.rs             # The unified `info.json` descriptor every dataset type shares
│   ├── dictation/
│   │   ├── mod.rs          # Dictation commands: media/subtitle/cue queries, progress (app-level DB)
│   │   ├── align.rs        # Cue alignment (multi-pass anchor DP): `dataset_align_cues` (shared
│   │   │                   #   `book.txt`/`book_sentences.txt`) + `dataset_align_cues_transcript` — desktop
│   │   ├── audit.rs        # Read-only dataset audit: 15 id-keyed checks over media/, subtitle/, waveform/, cues
│   │   │                   #   (`dataset_audit`; its check ids are the vocabulary `verify:` binds against) — desktop
│   │   └── adjust.rs       # Cue-time adjustment: energy envelope, silence snapping — desktop
│   ├── cards/
│   │   ├── mod.rs          # Card CRUD, tags, SM-2 review, FTS5 search, multi-location discovery
│   │   └── sync.rs         # Bidirectional card dataset sync (last-write-wins, per-dataset sync_state)
│   ├── book.rs             # Book chapters/sentences/words and their audio
│   ├── book_artifacts.rs   # Book "understanding layer": artifacts (explain/simplify/digest/argument_map/
│   │                       #   questions/note/assessment), glossary terms, quiz→XP results; LLM + Edge-TTS generation
│   ├── read_aloud/         # Read-aloud datasets: recorded takes, STT scoring, XP awards
│   │   ├── mod.rs          # Dataset dir + DB, sync, the command wrappers, shared path helpers
│   │   ├── source_align/   # Align page: recording folder → word timings → spans → clips
│   │   │                   #   (`index` register/peaks, `place` the matcher, `proposals` the
│   │   │                   #   span file + review view, `writes` apply; commands in `mod.rs`)
│   │   └── …               # assess, audit, edges, feedback, fluency, gop, import,
│   │                       #   normalize, phoneme, standard_audio, tts_ref, vad
│   ├── report.rs           # The whole read-only picture of one dataset in one call: identity,
│   │                       #   audit, every workflow run scored against it, STT model state,
│   │                       #   actionable hint (`dataset_full_report`) — desktop
│   ├── tools.rs            # Book splitting (`dataset_parse_book`, via the vendored SaT model —
│   │                       #   `textsplit::DEFAULT_MODEL_ID`), Python script execution (`write_transcripts.py`) — desktop
│   └── textsim.rs          # Ratcliff-Obershelp similarity, shared by align + read_aloud
│
├── models/                 # Whole group is `stt`-gated
│   ├── mod.rs              # The `model_*` command wrappers, each a few lines over a submodule body
│   ├── state.rs            # `ActiveModel`, `ModelState`, status/download DTOs, default + selected
│   │                       #   model preferences (what auto-load consults)
│   ├── engine.rs           # Load/unload/delete/cancel + `ensure_model_loaded`
│   ├── transcribe.rs       # Text / segment / word-level passes, chunking for long audio, and the
│   │                       #   guard that lends the model out of `ModelState` for one call
│   ├── download_stt.rs     # CDN blob download for STT models
│   ├── download_hf.rs      # HuggingFace catalog download for the non-STT models (OCR/TTS/split/phoneme)
│   ├── download.rs         # Streaming downloader + hub providers (the transport both of the above use)
│   ├── catalog.rs          # HuggingFace catalog of non-STT models (OCR, TTS, text-split)
│   ├── catalog_stt.rs      # STT model catalog
│   └── index.rs            # Unified `{model_root}/index.json` over every downloaded model
│
├── textsplit/              # Vendored SaT (Segment any Text) sentence segmentation on `ort` +
│   │                       #   `tokenizers`; `textsplit`-gated (a `desktop` subset) — desktop
│   ├── mod.rs              # `SaT` engine, cached session (`TextSplitState`), the `text_split*`
│   │                       #   commands, and the shared `split_text` seam the Split Book stage uses
│   ├── config.rs           # Model config (chunk/stride ids) read from `config.json`
│   ├── const.rs            # SaT constants (boundary index, default threshold, special token ids)
│   ├── extract.rs          # Overlapping-chunk inference, hat/uniform weighting, thresholding
│   ├── model.rs            # ONNX session wrapper
│   └── utils.rs            # Offset/sentence post-processing helpers
│
├── sync/                   # Cross-device sync; owns `PROTOCOL_VERSION`
│   ├── mod.rs              # Group declarations + wire protocol version
│   ├── client.rs           # Snapshot pull + writeback queue (all platforms; the Android thin client)
│   ├── discover.rs         # PC auto-discovery: UDP probe, multicast, Tailscale
│   ├── change_log.rs       # Hub-side append-only change journal (`sync_log` table in the app DB)
│   ├── server.rs           # PC axum HTTP service hosting REST + MCP, enforces trust zones — desktop.
│   │                       #   Hub-only: `auto_start` refuses a follower and `apply_role` reconciles on every
│   │                       #   role change, so a follower stops listening (and stops advertising)
│   ├── rest.rs             # `/api/v1` dataset snapshot + batched-writeback endpoints — desktop
│   └── pairing.rs          # Ed25519 device pairing and trust zones — desktop
│
├── ai/
│   ├── mod.rs
│   ├── llm.rs              # Ollama: connection check, model list/pull/delete, chat + streaming
│   ├── chat.rs             # Cross-device chat thread persisted on the PC hub
│   └── goose_llm.rs        # Goose SDK LLM bridge — desktop. (An ACP client that dialled
│                           #   `goose serve` lived beside it and was removed: FmS takes the
│                           #   attach direction only, so nothing here dials an agent)
│
├── mcp/                    # Built-in MCP server (rmcp), nested at `/mcp` — desktop
│   ├── mod.rs              # `DatasetMcpServer`, params shared by 2+ domains, `tool_router()`
│   │                       #   merging the domain routers, the `ServerHandler` impl (tools + resources +
│   │                       #   prompts), `create_mcp_service()`
│   ├── policy.rs           # Not a domain: the tier classification + `mcp_audit` journal every route
│   │                       #   is wrapped in by `tool_router()`. Unknown names default to `write`, so a
│   │                       #   new tool is never silently a read — add it to the right list here
│   ├── manual.rs           # Not a domain either: the generated agent manual. One body, three renderings —
│   │                       #   `/agent.md`, `/agent.json`, `app_bootstrap`/`agent_manual_get` and
│   │                       #   `get_info().instructions`. The tool index is read off the live router, so
│   │                       #   there is no hand-written tool list to keep in step
│   ├── datasets.rs         # dataset discovery/CRUD + subtitle & waveform pipeline (26 tools)
│   │                       #   `dataset_audit` is subject-only — per-step reading is `workflow_verify`/`workflow_adopt`;
│   │                       #   `dataset_full_report` composes audit + runs + model state for one call
│   ├── dictation.rs        # cue adjust/align, dictation progress, favourites, subtitle versions (15)
│   ├── cards.rs            # card CRUD, tags, FTS5 search, SM-2 review, online sync (24)
│   ├── books.rs            # book chapters/sentences/words + read-aloud texts & scoring (24)
│   ├── models.rs           # STT model status/download/load/default/delete/transcribe (7)
│   ├── ai.rs               # Ollama/cloud chat & model management, provider switch, Edge TTS, cross-device chat (11)
│   ├── wiki.rs             # wiki dirs, read/write/delete, search, index (9)
│   ├── sync.rs             # web service, pairing registry, PC scan/connect/pull, incremental sync (15)
│   ├── system.rs           # settings, auth, logs, OCR, screenshot, app UI control (the `app_*` verbs the
│   │                       #   dock and the shell listen for), the `app_events` timeline, the trash undo
│   │                       #   loop, MCP audit journal (28)
│   ├── workflow.rs         # persistent workflow state machine: list/create/status/reap/next/get_definition/
│   │                       #   advance/record/intervene/verify/adopt/builtin_templates, plus list_actions — the
│   │                       #   step vocabulary the Workflow page publishes (13)
│   ├── ui.rs               # generic UI action registry: list/invoke the actions the frontend published via
│   │                       #   `agent_report_ui_actions`; `ui_invoke_action` correlates back through
│   │                       #   `agent_report_action_result` (2)
│   ├── todo.rs             # agent TODO domain: add/list/complete/dismiss the user-facing tasks the `tasks`
│   │                       #   tab shows; completing a workflow `human` step's todo resumes that run (4)
│   ├── resources.rs        # MCP *resources* (not a tool domain): `fms://status`, `fms://docs/start`,
│   │                       #   `fms://docs/principles`, `fms://docs/domain/{datasets,cards,workflow,sync}`,
│   │                       #   `fms://workflows/templates`, and the `fms://datasets/{uuid}/layout`
│   │                       #   template — every body derived at read time from the tool registry
│   └── prompts.rs          # MCP *prompts* (not a tool domain): the `docs/design/agent/goose_recipe/*.yaml`
│                           #   recipes, `include_str!`-embedded and parsed on demand, so the recipe files
│                           #   stay the source of truth and any MCP client gets them without goose
│
├── workflow/              # Persistent workflow state machine (file-based, resumable DAG) — desktop
│   ├── mod.rs             # group root: the `run_base()` storage seam + re-export the run-scoped API
│   ├── commands.rs        # the `workflow_*` Tauri command twins (the page drives the same core fns MCP does)
│   ├── templates.rs       # built-in definitions baked in via `include_str!` (`templates/<kind>/*.yaml`)
│   ├── verify.rs          # the binding layer: check vocabulary + evidence providers; owns the verify and adopt joins
│   └── core/              # domain-free framework (steps/deps/statuses, not what a step does)
│       ├── mod.rs         # schema (Definition/RunState/Event) + public API (list/create/status/next/advance/
│       │                  #   record/intervene/verify/adopt + rebinding a run whose progress was only inherited)
│       ├── validate.rs    # pre-run structural validation: unique ids, refs exist, acyclic, `verify:`/`adopt:` ids registered
│       ├── engine.rs      # readiness recompute, transitions, retry/propagation, lease recovery, data flow
│       └── persist.rs     # run-dir resolution, YAML/JSON parse, atomic state write, append-only events (+ read back)
│
├── app_paths.rs            # Platform-correct base dirs (desktop `dirs`, mobile app-private)
├── settings.rs             # Two-tier settings: global JSON + per-workspace JSON overlay
├── workspace.rs            # Workspace registry, selection, claim/auto-login
├── auth.rs                 # OAuth2 login, token storage, workspace identity
├── logger.rs               # Rotating file log + in-memory buffer + `log_*` commands
├── native_log.rs           # The two sinks `log` does not reach: native stderr → log pipe (Windows,
│                           #   console-less builds only) + a panic hook; see the module doc for why
│                           #   a GUI release build otherwise dies silently
├── db.rs                   # rusqlite wrappers with write-contention logging, adjustment guard
├── audio.rs                # Audio decoding via Symphonia
├── wiki.rs                 # Wiki documents; own root + `meta.json` linkage (not a dataset type)
├── xp.rs                   # XP ledger in the app-level SQLite
├── todo.rs                 # Agent TODO list in the app-level SQLite: user-facing tasks an agent or a
│                           #   workflow `human` step leaves for the learner; workspace-scoped, syncs via
│                           #   `@app` writeback, and completing one resumes the waiting workflow run
├── simple_words.rs         # "Known simple" words sourced from selected card datasets
├── edge_tts.rs             # Edge TTS voices + synthesis
├── ui_state.rs             # What the user is looking at, cached for agents: the webview reports a
│                           #   context patch (`agent_report_ui_state`) and its action registry
│                           #   (`agent_report_ui_actions`), which `app_get_ui_state` reads back
│                           #   without a webview round-trip — desktop
├── capture.rs              # Screenshot / clipboard image capture — desktop
├── trash.rs                # The app trash read back: `app_trash_list` / `app_restore` over
│                           #   `<data_root>/trash/<timestamp>_<name>`. The writer is
│                           #   `datasets::media::move_to_trash`; this is the undo side — desktop
└── ocr/                    # Tesseract OCR — desktop
    ├── mod.rs              # Path discovery, plain-text recognition (`ocr_recognize`), language list,
    │                       #   TSV recognition and the `ocr_hyphen_dictionary_*` commands
    ├── tsv.rs              # `ocr_recognize_tsv`: TSV layout → paragraphs, line joins, dash spacing;
    │                       #   line-break hyphens are recorded as candidates, never decided here
    └── hyphen.rs           # Hyphen decisions against the downloaded Hunspell dictionaries of the page's
                            #   language set (`spellbook`): 15-language catalog, install/delete/status,
                            #   `selection`/`mapped_sources`, decide (any dictionary joins) + apply,
                            #   shape-heuristic fallback for languages with no dictionary
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
  a `mod.rs` (this is why `wiki.rs` and `xp.rs` are still flat)
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
5. Decide its **policy tier** in `mcp/policy.rs`. Reads go in `READ_TOOLS`; anything that deletes
   but is undoable (app trash, soft delete, re-downloadable, derived index) goes in `RECOVERABLE`;
   screen-driving tools go in `UI_CONTROL`. Unlisted names fall through to `write`, and a
   `#[cfg(test)]` guard fails the build if a name carrying a destructive word
   (`delete`, `remove`, `clear`, …) lands anywhere but `destructive` without being listed in
   `RECOVERABLE` with a reason. Under `mcp_policy = "enforce"` the destructive and human-handoff
   tiers are refused; the default `"permissive"` only journals
6. The manual picks the tool up by itself — `mcp/manual.rs` reads the router, never a list. A new
   *domain* does need its name prefix added to `DOMAIN_PREFIXES` there, and a `#[cfg(test)]` guard
   fails the build (with the offending name) until it is

## External Dependencies

- **transcribe-rs** — Rust ONNX runtime for STT model inference (Parakeet family)
- **transcribe-cpp** (Whisper/GGML, desktop) — its vendored ggml defaults to
  `GGML_NATIVE=ON`, which makes ggml's CMake probe the *build* machine's CPU and
  bake that ISA into the objects. v0.1.8 shipped exactly that: AVX-512 from the
  CI runner, and `0xC000001D` (STATUS_ILLEGAL_INSTRUCTION) on every 12th-gen+
  Intel CPU, with no log line because SIGILL has no message. `src-tauri/.cargo/config.toml`
  pins `GGML_NATIVE=OFF` (AVX2 floor, no AVX-512 path) and
  `.github/workflows/build.yml` repeats it in `env` + the rust-cache key. After
  editing that value run `cargo clean -p transcribe-cpp-sys` — cargo does not
  re-configure a dependency for a config-file `[env]` change. Never set it back
  to ON, and never "just bump the version": the binary changes, the bug does not.
- **symphonia** — Audio decoding (all formats) and waveform peak generation (replaces the former `audiowaveform` sidecar)
- **rusqlite** — SQLite with bundled mode (no system SQLite required)
