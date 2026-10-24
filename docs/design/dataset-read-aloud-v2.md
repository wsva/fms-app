# Read-Aloud Datasets v2 — Sources & Generated Texts

> Status: **proposal**. Builds on [dataset-read-aloud.md](./dataset-read-aloud.md) (the v1 practice loop) and obeys every rule of [dataset.md](./dataset.md). Nothing here changes the record → transcribe → score → XP loop; it changes *where the texts come from*.

## The idea

v1 treats a read-aloud dataset as a bag of hand-authored texts: the learner (or an agent) types each `read_text.content` directly. v2 adds a **source layer** — the raw material a text is *derived from* — so a dataset can be seeded from real documents and regenerated on demand.

**The relation is strictly 1:1. One source produces exactly one text.**

Four grounding decisions shape the whole design:

1. **One source → one text.** A source is a bundle of input files; the app performs a single transformation over that bundle and emits a single text. Multiplicity lives on the *input* side (many files, fused), never on the output side.
2. **Sources are never shared, anywhere.** They do not publish to followers and they do not follow the owner to another device — only the *generated texts* (`read_text` rows) sync. A source is the owner's private recipe, on one machine, not content.
3. **The owner preprocesses off-app.** Converting PDFs / DOCX / images / audio to **markdown** is the owner's job, done with whatever tool they like *before* the app sees it. `preprocess/` accepts `.md` and nothing else — the app never parses a binary format, and never has to guess how a foreign format should be read. This removes the single biggest would-be dependency (a server-side document extractor) from v2 entirely.
4. **A source is a fixed five-part directory** (`original/`, `preprocess/`, `workflow/`, `generate/`, `meta.json`) so the recipe, its state, and its output are all legible in one place.

Restated as the ask: a source is a directory holding several files in several formats. The owner reduces them to text, and a local LLM (e.g. `gemma4:e4b`) rewrites that text into a single readable passage at a target CEFR level (e.g. B2).

A dataset with a hundred texts is a dataset with a hundred sources. "One document, many texts" is therefore not a feature of the source — it's the owner creating several sources, which is the same outcome arrived at through a step the owner can see, edit, and undo. See [Non-goals](#non-goals) for why the output-side fan-out was deliberately dropped.

What must NOT change: the text stays the reference a transcript is scored against, and a generated text is **derived and disposable** exactly like `normalized_content` already is (`read_aloud.rs:67-71`, `effective_reference` at `read_aloud.rs:214`). A source is a *seed*, not a second authority, and if the learner edits a generated text by hand, the edit wins — the link back to the source becomes provenance, not a live binding.

## The source directory

Because a source is never shared and never parsed by the app, it lives **entirely on the filesystem** — there is no `read_source` table in `data.sqlite3`. Its identity is `meta.json`; its *state* is a workflow run sitting next to it in `workflow/`. The DB stays 100% shareable content. Layout:

```
<datasets_dir>/read_aloud/<name>/
├── info.json                 # DatasetInfo, type "read_aloud", format "read_aloud-v2"
├── data.sqlite3
│   ├── read_text             # v1 columns + nullable source_uuid, gen_* (§Lineage)
│   ├── read_standard_audio   # reference clips per text (§Standard audio)
│   └── read_attempt          # legacy, migration-read only (unchanged)
├── media/
│   ├── <legacy_uuid>.wav     # v1 attempt audio — read-only, never written
│   └── <text_uuid>/          # standard audio, one directory per text
│       └── <audio_uuid>.wav
└── sources/                  # ← OWNER-LOCAL, EXCLUDED FROM SYNC/PUBLISH
    └── <name>/               # owner-chosen label, e.g. rechtsphilosophie-authority/
        ├── meta.json         # the source's identity + lineage (§Meta) — NOT in the DB
        ├── original/         # owner's raw files, any format — the app never reads these
        ├── preprocess/       # owner-authored .md only — the ONLY thing the app reads as input
        ├── workflow/         # THE authoritative lifecycle state, one run per source
        │   ├── workflow.yaml # immutable definition (the template this run was created from)
        │   ├── state.json    # per-step Status (§Meta) — replaces any hand-rolled status
        │   └── events.jsonl  # append-only transition audit log
        └── generate/         # the LLM's draft output, written by the app
```

- **`original/`** — drop anything here (pdf, docx, png, mp3…). The app treats it as an opaque archive; it is the owner's record of what the source *was*. Never read, never synced.
- **`preprocess/`** — the owner's handoff: one or more **`.md`** files, and nothing else. Plain text is valid markdown, so an owner with nothing to mark up just drops their text in a `.md` and loses nothing; headings, lists, and tables are available when the source has real structure worth telling the model about. These files are the LLM's actual input, concatenated in sorted file-name order, and several of them is *how* a source carries "various files in various formats" while still producing one text.
- **`generate/`** — the app writes the candidate draft here (`generate/text.md`), rotating the draft it replaces to `generate/text.prev.md` first. It never touches `preprocess/` or `original/`. The draft stays here until the owner **adopts** it into `read_text.content`.
- **`workflow/`** — an ordinary run dir of the existing state machine, filed *per source* rather than in the dataset's shared `workflows/`. It holds the engine's own three files (`workflow.yaml`, `state.json`, `events.jsonl`) exactly as [`workflow::core::persist`](../../src-tauri/src/workflow/core/persist.rs) writes them, so `workflow_status` / `workflow_next` / `workflow_advance` work on it unchanged.

### Name vs. identity

One source directory produces exactly one text, and the directory is addressed by a **name the owner chooses**, not by its uuid — a uuid as a path is unreadable in a file manager and meaningless in a diff. Identity lives in `meta.json.uuid` and nowhere else, which buys three properties:

- **Renaming a source dir is free.** It touches no database row, because `read_text.source_uuid` references the uuid, never the path — the same reason a dataset folder can be renamed without breaking anything that knows its uuid. Co-locating the run inside the source is what makes this genuinely better than a shared `workflows/` dir: the run *travels with* the source, so a rename or a move keeps definition, state, and audit attached to the material they describe.
- **Discovery is scan-then-read**, exactly like datasets themselves: `find_dataset_dir` walks a root and matches on `read_info_opt(&path).uuid` (`read_aloud.rs:130-149`). A source resolves the same way across `sources/*/meta.json`. Consequence: anything that caches a source's *path* must re-resolve it — only the uuid is a durable handle.
- **Collisions have a defined answer.** Two directories carrying the same `meta.json.uuid` is corruption, so refuse the duplicate at create (the `assert_uuid_free` discipline, `datasets/mod.rs`) and on read report both paths rather than silently picking one — a silent first-wins would strand the other source's texts.

Name rules: sanitized through the existing `sanitize_name` (`read_aloud.rs:563`), no path separators, and unique among siblings — a taken name gets the `-<uuid8>` suffix `create_dataset` already uses (`read_aloud.rs:651-654`). Uniqueness is compared **case-folded**, because case-insensitive filesystems are the norm here. `meta.title` remains the free-form display name the UI shows; the directory name is only its filesystem-safe projection.

## Meta

`sources/<name>/meta.json` carries the source's identity and lineage only. It deliberately does **not** carry status — `workflow/state.json` owns that, and a second state machine would just disagree with the first.

```jsonc
{
  "uuid": "…",                    // THE identity — deliberately not the dir name
  "title": "Rechtsphilosophie — authority",
  "level": "B2",                  // target CEFR; optional dataset default overrides
  "workflow": {                   // mirrors RunState.workflow (WorkflowRef) — the version binding
    "name": "build_read_aloud_text",
    "version": 1
  },
  "gen_model": "gemma4:e4b",      // model tag used for the last generate
  "gen_prompt_hash": "…",         // versioned prompt template + params, for reproducibility
  "preprocess_hash": "…",         // hash over sorted preprocess/*.md (§Pipeline cache)
  "text_uuid": "…",               // the read_text row produced on accept (may dangle on a peer)
  "created_at": "…Z",
  "updated_at": "…Z"
}
```

Where the source *is* in its lifecycle is read from `state.json`'s per-step [`Status`](../../src-tauri/src/workflow/core/mod.rs) (`pending | ready | running | completed | failed | blocked | skipped`) — the vocabulary the engine already implements, including retry, lease recovery, and propagation. Two of those map onto this domain usefully: `blocked` is the honest way to say "waiting on the owner to preprocess", and `failed` plus `attempt` is how a bad LLM call is retried without inventing a status for it.

Keeping both files off the database is deliberate: source lifecycle is owner-local churn that should never journal, fork, or publish — exactly the failure mode decision #2 exists to avoid.

## Lineage (what DOES sync)

The shareable side stays minimal and honest. Add only nullable provenance columns to `read_text` (journaling through the existing `save_text_row` choke point, `read_aloud.rs:820`):

```
read_text   (v1 columns unchanged, plus…)
├── source_uuid     nullable — the source that produced this text (NULL = hand-authored, the v1 case)
├── gen_model       "gemma4:e4b" — which local model produced it
├── gen_level       CEFR level it was generated for
├── gen_prompt_hash hash of the effective prompt + params
└── generated_at    nullable stamp (NULL = never generated, i.e. authored)
```

Because the relation is 1:1, `read_text.source_uuid` is **uniquely constrained** (a partial unique index over non-NULL values). Two texts claiming one source is a corruption the schema should refuse, not a state the UI has to reason about.

On a peer that adopted the published dataset, `source_uuid` points at a source dir that does not exist there — that is fine and already an established pattern: a device-local attempt's `audio_path` resolves to `None` on another device (`resolve_audio_url`, `read_aloud.rs:226`). Provenance is informational, never a required join.

## Standard audio (shareable content)

Pronunciation is measured against a clip of the text read by a canonical voice, and that clip belongs to the **dataset**, not to the machine that made it: several clips per text, exactly one of them the default, and the scorer reads only the default. This is v1 shape in one respect — `media/` finally holds something again — and new in another: the clips are a table, because ordering, the default flag and word timings are relational.

```
read_standard_audio
├── uuid        PRIMARY KEY
├── text_uuid   the text the clip reads
├── note        free-form provenance (origin, voice, how it was cropped) — never parsed
├── boundaries  TTS word timings, JSON; NULL ⇒ an upload, which has none
├── text_sha    sha256 of the effective reference the clip was made from
├── is_default  0/1 — enforced on write, tolerated on read
└── created_at  tie-breaker for "the earliest remaining"
```

The row and the file cannot disagree, because the row's identity *is* its path: `media/<text_uuid>/<uuid>.wav`, 16 kHz mono (the phoneme model's input rate, and small). There is no `wav_path`, no `kind` and no `duration` column — the first would be a second copy of the identity, the second is `boundaries IS NULL`, and the third is decoded from the wav when a text's clips are listed. Only what cannot be derived is stored.

Two tolerances, both deliberate. **One default is enforced on write** (the first clip inserted for a text is default; `set_default` clears the others in one transaction; deleting the default promotes the earliest remaining) **and tolerated on read** (`ORDER BY is_default DESC, created_at, rowid`, then the first row whose wav exists) — a sync replay must never fail on a partial unique index, so the invariant is a habit on the write path rather than a constraint the database can refuse. **A missing wav is not an error**: the row is filtered out, so a metadata/file race reads as "no standard audio", never as a broken entry.

`text_sha` is what makes staleness cheap. It covers the *effective* reference (`normalized_content` when non-empty, else `content`), so a clip made before an edit no longer matches and the scorer reports `stale_reference` instead of grading the learner against a phone target for words that are no longer there. There is no invalidation pass and no migration: a comparison, on every score.

Nothing extra is needed to share them. `read_standard_audio_save` / `_delete` journal as dataset-scoped change kinds beside `read_text_save` / `_delete`; the hub's snapshot is a whole `data.sqlite3` copy, which `sanitize_snapshot_db` passes through (it drops only `card_review` and `read_attempt`); and the wavs ride the ordinary file channel, hashed into `compute_manifest` and fetched by hash. Deleting a text cascades — its rows go with it and `media/<text_uuid>/` is removed.

The clips themselves are made in the practice view's **Manage** dialog: *Generate with Edge TTS* (word timings included, so per-word attribution is exact) or *Upload audio* and crop (no timings, so per-word attribution is time-proportional; the aggregate score is unaffected). An upload's crop is the only edit offered — re-cropping means re-picking the file — and no source copy is kept, because an Edge TTS clip is one click to regenerate and an uploaded one still exists wherever the user picked it.

## The pipeline

One source, run end to end, is short precisely because the app does not extract anything and does not segment anything — and because the four phases below are not a bespoke flow chart, they are the steps of this source's own workflow run, driven by the engine in `workflow/`:

1. **`preprocess` — the owner's step, `blocked` until they act.** The owner converts their `original/` files into `preprocess/*.md`. No app code can observe a file save, so the transition needs an explicit signal: `read_aloud_source_scan` validates the extension rule, recomputes `preprocess_hash`, and records the step complete via `workflow_record`. If the owner wants five texts, this is where they decide to make five sources.
2. **`generate` — one LLM call, `running` → `completed`.** Compare `meta.preprocess_hash` against the hash the last run recorded; unchanged means this is a pure cache and the step can be skipped. Otherwise feed every `preprocess/*.md` file as one context blob (sorted file-name order, which is what `preprocess_hash` commits to) and make **one** LLM call, rotating any existing draft to `generate/text.prev.md` before writing `generate/text.md`, and recording `gen_model` / `gen_prompt_hash`. Inference goes through `ai::llm` (Ollama `/api/chat`, `ollama_url` from settings) or the `ai::goose_llm` bridge. Model tag and level come from `meta`.
3. **`verify` — measured, not trusted.** The checks in [Quality gating](#quality-gating) run through `workflow_verify`; passing is a precondition for the next step, not advice.
4. **`accept` — the human checkpoint, which is `workflow_adopt`.** The draft is presented against the current text; adopting it writes the source's `read_text` (`content`, `source_uuid`, `gen_*`) through `save_text_row`, and the row's uuid is recorded in `meta.text_uuid`.

Because the run is the state, a half-cooked source needs no special handling: it is simply a run with a `blocked` step, resumable by the owner or handed to an agent, with `events.jsonl` already recording how it got there. Building a many-text dataset is N independent runs of one template, which is the shape the engine already handles well.

One LLM call per source is the point: it is the reason this design is controllable. A small local model asked to split, fuse, *and* level-adjust in one prompt degrades badly; asked to rewrite one blob to one level, it behaves. Because output is **non-deterministic**, regeneration is *traceable, not byte-identical*: `preprocess_hash` + `gen_prompt_hash` + `gen_model` make an "undo last regenerate" or "why does this read differently than last week" answerable.

### The accept gate (the safety property)

Generation is a **draft producer, never an auto-writer**:

- `generate` writes only into `generate/text.md` — never into `read_text.content`.
- The owner/agent reviews draft vs. current and **adopts** explicitly; adopt promotes the draft into `content` through `save_text_row` (journaling exactly once, like any text write), keeping the text uuid stable across regenerate so a learner's attempts stay attached.
- A `read_text` that already has attempts or was hand-edited is never silently replaced. Declining leaves `content` untouched; the source link and the run both stay for a later retry.

This is the engine's existing `verify` → `adopt` join rather than a new mechanism ([`workflow/verify.rs`](../../src-tauri/src/workflow/verify.rs) owns both), which is the reason the accept gate can be described as a property of the design instead of a caveat: the state machine already refuses to complete a step whose `verify:` binding has not passed.

This preserves the v1 invariant that `content` is the scoring authority and every score is interpretable against the text the learner actually read.

### Staleness

When `preprocess/` no longer matches the `preprocess_hash` the run recorded, the source is stale: its adopted text no longer reflects its own input. Detection is automatic, and so is the state change — but **staleness is not a status**. There is no `status = "stale"` field to keep in sync, because the engine already has the exact mechanism:

1. `read_aloud_source_scan` recomputes the hash, sees the drift, and calls `workflow_intervene` with `op = "reset"` on the `preprocess` step.
2. `reset` restores that step to `StepState::default()` (`core/mod.rs:939`) **and** runs `engine::reopen_stale_completed` (`:955`), which walks the completed dependents and reopens them — so `generate` and `accept` cascade back through the dependency invariant with an `invalidated` event each, already journalled.

The owner therefore sees a run that has visibly *un-completed itself*, with `reason` strings naming the dependency that stopped being satisfied and an audit trail of when and why — strictly more informative than a badge reading "stale". The reset-and-cascade is the only path that reopens a step: `recompute` preserves `completed` and `failed` as terminal judgements (`engine.rs:293`), so nothing drifts back on its own between scans.

Any "stale" indicator in the UI is a **derived projection** — compare the current `preprocess/` hash against the run's recorded one at read time, render the result, store nothing. A persisted flag would reintroduce exactly the second authority the `workflow/` dir was added to remove.

### Quality gating

The LLM cannot be trusted to honor a length or stay faithful to its input, so both are *measured* rather than assumed — registered as `verify:` check ids in `workflow/verify.rs` and bound to the generate step in the template:

- **`read_aloud_length_in_band`** — the draft falls inside a word-count band appropriate to one recording.
- **`read_aloud_source_fidelity`** — `textsim::similarity_score` between the draft and the concatenated `preprocess/` text clears a floor, which is the drift/hallucination gate a level rewrite needs.
- A failing check does not block the draft write; it blocks **adopt** and surfaces the reason through `workflow_verify`, so the owner reviews rather than inherits a bad text.

## CEFR level targeting

`level` is a first-class parameter, not a prompt string buried in code:

- **Where it lives**: `meta.level` per source (owner-local); optionally a dataset default on `info.json` (a new `#[serde(default)]` field on the shared `DatasetInfo`), and `gen_level` recorded on the produced text.
- **Why recorded per text**: re-running a source at a different level replaces the *same* text's content, so a learner may pass an A2 read and later a B2 read of what is now one text. If they genuinely want both as separate material, that is two sources — again, multiplicity by source count.
- **Prompt contract**: a fixed, versioned template per (language, level) baked as `include_str!` under `src-tauri/src/workflow/templates/read_aloud/`, so `gen_prompt_hash` is stable and reviewable. The template demands: preserve the source's factual content, target ~N words, output only readable text (no preamble).

## Sync, publishing, and the invariants

Decision #2 ("never share sources") turns the old payload-weight tension into a single rule:

- **`sources/` is excluded from every content hash, snapshot, and publish path.** The app's existing "exclude a rebuildable artifact" seam is `is_derived_index` (`info.rs:82`), whose logged caveat is that an exclusion *silently drops real data* — and which deliberately matches a **root-level name only**, so a nested file sharing that name stays content. `sources/` is a nested *tree*, so it needs a **prefix** rule, not a name rule: a new `is_owner_local_rel()` alongside it. The hazard is where that rule has to land: today the non-content decision is duplicated across three functions — `sync/rest.rs::is_db_file` (`:374`), `sync/blobpush.rs::is_db_file` (`:62`), `sync/upload.rs::is_db_file` (`:39`) — each of which already delegates to `is_derived_index`. Adding owner-local exclusion to all three by hand is exactly how a leak happens, so the clean move is to lift one shared `is_non_content_rel()` into `datasets/info.rs` covering DB sidecars + derived index + owner-local, and have all three gates call it. This is a security/privacy property, not an optimization: source files can carry licensing and personal data the generated texts don't.
- **A source's workflow run is excluded by that same single rule.** This is the concrete payoff of co-locating `workflow/` inside the source rather than using the dataset's shared `workflows/`: verified today, `workflows` appears in **no** exclusion predicate, so dataset-scoped runs already ride the content hash and ship to followers. For read-aloud that would publish the owner's authoring state — model tags, prompt hashes, step history, and whatever `params` a template carried — straight out of the folder that decision #2 exists to keep private. Putting the run under `sources/` means one rule drops payloads, manifest, *and* pipeline state together, with no second exclusion to remember.
- **Sources are single-device, full stop.** They do not sync owner-to-owner-either: a source lives only on the machine that created it, not on the owner's other devices. This is the deliberate cost of decision #2 and it is accepted rather than worked around — no per-owner private channel, no second exclusion list to keep in step. What that means in practice: regenerating or adding material happens on whichever device holds the `sources/` tree; every *other* device still practises normally, because a text is fully usable without its source. Two consequences to design for rather than discover: a text generated on device A and then edited by hand on device B converges as a text but shows a `source_uuid` that resolves nowhere on either device's own view of A (already the documented dangling-provenance case), and `generate/` drafts plus `events.jsonl` run history are simply not part of the cross-device picture.
- **Generation is authoring, not practising** — so unlike the §17 attempt events (which stay in the app DB), accepting a text legitimately writes dataset content and journals it as an ordinary `read_text_save`. Adding a source, however, writes only owner-local files; the DB is untouched until accept.
- **Adopting a published dataset forks nothing source-wise**: a follower gets the generated texts (with dangling `source_uuid`) and no `sources/` dir at all. Editing/adding a source on the owner's side must go through `find_dataset_dir_write` → `fork_base_to_overlay` (`read_aloud.rs:154`) like any content edit; browsing never forks.

## Workflow integration

The pipeline is not a new mechanism — it **is** a workflow run, created from a built-in template and filed inside the source it describes. New template: `src-tauri/src/workflow/templates/read_aloud/build_read_aloud_text.yaml`, baked in via `include_str!` like `build_book_library.yaml`:

```yaml
name: build_read_aloud_text
version: 1

steps:
  - id: preprocess
    title: Owner preprocesses to markdown
    description: >-
      Blocked until sources/<name>/preprocess/ holds .md only. The scan command is
      what observes the owner's work and records this step.
    action: read_aloud_source_scan

  - id: generate
    title: Generate draft at target level
    action: read_aloud_generate
    depends_on: [preprocess]
    verify: [read_aloud_length_in_band, read_aloud_source_fidelity]

  - id: accept
    title: Adopt draft as the reference text
    action: read_aloud_accept
    depends_on: [generate]
```

Each `action` names a real command, so an agent drives the whole flow with `workflow_create` + the `workflow_*` loop and the Workflow page renders the graph like any other pipeline. The two checks are bound on `generate`, which means the engine — not a bespoke rule — is what refuses to move a draft forward before it is measured; `workflow/validate.rs` already requires every `verify:`/`adopt:` id to be registered.

**The one engine change this needs.** Runs are currently resolved by [`workflow::run_base`](../../src-tauri/src/workflow/mod.rs) (`mod.rs:117`) as either `<dataset>/workflows/` (a dataset scope) or `<workspace>/workflows/` (no scope). A per-source run is a **third scope**, so `run_base` needs to accept a source and join `sources/<name>/workflow`, and `run_base_for` / `dataset_run_bases` (`:132`, `:166`) must also look there when resolving a bare `run_id`. Two things make this cheap rather than invasive: the dir is spelled with the engine's own file names, and "is this a run?" is still `workflow.yaml` existing (`DEFINITION_FILE`), so `workflow_status`/`_next`/`_advance`/`_verify`/`_adopt` work on it untouched. Note the deliberate singular/plural split — `workflows/` is a *container* of runs, `sources/<name>/workflow/` is *one* run.

Two consequences worth accepting knowingly. The `workflow` group is **desktop-gated**, so generation is owner-side authoring only — which holds together, since a follower has no `sources/` to run against anyway; the Android thin client keeps practising and never gains a source UI. And a `run_id` must stay unique within a dataset's scan, so deriving it from the source uuid (not its mutable name) keeps `run_base_for`'s ambiguity branch (`:190`) unreachable.

## Agent surface

Every command has an MCP twin (today the `read_aloud_*` tools live in `mcp/books.rs`). New tools:

- `read_aloud_source_create` — mint a uuid, write it into `meta.json`, create `sources/<name>/`, and `workflow_create` its run from `build_read_aloud_text`. Takes a `name` for the directory and a `title` for display; the name may later change without touching the uuid.
- `read_aloud_source_scan` — the source's only observer, so it carries two duties: validate that `preprocess/` contains only `.md` (any other extension is a scan error, not a silent skip) and recompute `preprocess_hash`; then either record the `preprocess` step complete when fresh input is present, or **reset** it when the hash drifted from what the run recorded ([Staleness](#staleness)). This is the command that turns "the owner edited files" into a state transition. Returns `meta` plus the run's step statuses.
- `read_aloud_generate` — the `generate` step's action: one LLM call into `generate/text.md` (writes **no** `read_text`), returning `{draft, gen_model, gen_level, preprocess_hash, checks}`.
- `read_aloud_accept` — the `accept` step's action: promote the draft into `read_text.content` via `save_text_row`, guarded by the step's `verify:` results.
- `read_aloud_source_list` / `_remove` — enumerate sources by scanning `sources/*/meta.json` (keyed by uuid, reported with the dir name), and trash source dirs — which takes the run with them (owner-local; trash, not delete, per the standing "backup before destroy" rule).

Lifecycle is **not** re-exposed here: `workflow_status` / `workflow_next` / `workflow_advance` / `workflow_verify` / `workflow_adopt` already read and move this run, and duplicating them as `read_aloud_*` twins would reintroduce the two-sources-of-truth problem the `workflow/` dir exists to remove. These tools are the domain *actions* the steps invoke, not a second view of the state machine.

Errors are actionable and responses structured, per `docs/agent_friendly_design.md` (e.g. "`gemma4:e4b` not found in Ollama — pull it via `llm_pull_model` or set `gen_model` to an installed model").

Because a directory name is a mutable label, **every source tool keys on `source_uuid`** — the durable handle — not on the name. Resolving a name as a convenience alias is acceptable only when it matches exactly one source; ambiguous or stale names are an error, never a silent pick.

## Migration & format version

- Bump `FORMAT_READ_ALOUD` → `read_aloud-v2` (`info.rs:62`). A v1 folder stays valid: `open_db`'s `CREATE TABLE IF NOT EXISTS` + idempotent `ALTER TABLE ADD COLUMN` pattern (`read_aloud.rs:166-204`) self-heals a copied folder, so adding the nullable `read_text` columns is the same non-destructive move. Hand-authored v1 texts keep `source_uuid = NULL` and behave exactly as before. `sources/` simply does not exist until the owner creates one.
- The `info.json` optional default-level field must parse to a graceful default so older descriptors still read (shared `DatasetInfo` across all types → `#[serde(default)]`).

## Recommendations (condensed)

1. **Hold the 1:1 relation in the schema**, not just in prose — a partial unique index on `read_text.source_uuid` makes "one source, two texts" unrepresentable.
2. **Keep sources entirely out of the database and out of the sync hash** — filesystem `meta.json` only, with one explicit, named `sources/` exclusion consulted by every publish path. This is a privacy guarantee, not just simplicity.
3. **The app reads only `preprocess/*.md`** — one accepted extension, so validation is trivial and a stray file is an error rather than an ignored file. No binary parsing and no segmentation anywhere in v2. Owner preprocesses off-app.
4. **Let the workflow engine own source state — `meta.json` must not carry a `status`.** One `state.json` per source, in the engine's vocabulary, is the only authority; a parallel status field would just disagree with it.
5. **Express staleness as `intervene(reset)` on `preprocess`**, and let `reopen_stale_completed` cascade it — the engine already reopens completed dependents and journals `invalidated`, so no new status and no stored flag is needed.
6. **Place the run under `sources/`, not the shared `workflows/`** — that is what keeps authoring state out of the publish path for free, since `workflows` is excluded from nothing today.
7. **`content` remains authority; generation is a draft producer.** Adopt is the gate (`workflow_adopt`), the text uuid stays stable, and an attempted or hand-edited text is never overwritten.
8. **One LLM call per source**, with length and fidelity *measured* by `verify:` checks instead of trusted from the prompt.
9. **Identity is `meta.json.uuid`, the directory name is a label** — the DB references the uuid only, so renaming a source is a no-op for every text it produced.
10. **Record lineage** (`source_uuid`, `gen_model`, `gen_level`, `gen_prompt_hash`, `preprocess_hash`) for reproducibility — informational, allowed to dangle on peers, and expected to dangle on the owner's other devices too.
11. **Consolidate the three duplicated `is_db_file` gates into one `is_non_content_rel()`** before adding the owner-local exclusion, so a fourth concept does not have to be hand-copied three times.

## Non-goals

**Output-side fan-out (one source → many texts) is intentionally out of scope for v2.** It was dropped because controlling it is genuinely hard and the hard parts all fall outside the app's reach:

- Asking the LLM to segment *and* rewrite means the LLM decides **how many** texts exist, so every regenerate churns the output set and its boundaries.
- Any positional identity (`segment_index`) breaks under re-run — an attempt recorded against segment 3 silently re-attaches to different content — so a fan-out design needs content-addressed, stable identities before it is safe.
- Length and CEFR level are two constraints fighting each other (simplification shrinks text), and small local models honor both poorly in one call.
- Overlapping-window segmenters like SaT (`src-tauri/src/textsplit/`) duplicate boundary sentences into adjacent segments, producing near-identical texts.

None of that is a blocker, it is just unearned complexity: the same outcome is available today by creating several sources, where the split is visible, editable, and reversible by the owner. Revisit fan-out only if an assisted **proposal** step turns out to be worth building — one that runs `text_split` and writes *candidate* `preprocess/` files for the owner to curate, so the app still never auto-fans-out and the fan-out unit stays the file.

## Open questions

Resolved since the first draft: sources never sync even between one owner's devices (see [Sync](#sync-publishing-and-the-invariants)); a superseded draft rotates to `generate/text.prev.md`; and staleness is a `reset`-and-cascade on the run rather than a stored status (see [Staleness](#staleness)). What remains genuinely undecided:

- **How much UI in v2's first cut?** The pipeline's *state* needs none — the Workflow page renders the run from `state.json`, and `workflow_*` already drives it. Two things have no existing surface: creating a source (though `read_aloud_source_create` plus a file manager covers an owner who works that way) and **reviewing a draft against the current text before adopting**, which is the one genuinely new screen and is where a human judgment is supposed to happen. A built-in `preprocess/` editor would *contradict* the off-app preprocessing decision, so it is out either way.
- **`text.prev.md` lifetime**: rotate exactly one generation back forever, or clear it once a draft is adopted (so "undo" means undo-the-draft, not undo-everything)?
- **Whose hash wins**: if a source is reset by a scan while an agent holds a `running` claim on `generate`, should the reset be refused (lease respected) or should it forcibly reap the claim?
- **Fidelity floor**: what `textsim` score counts as "still faithful to the source" is a guess until real `gemma4:e4b` runs produce data — calibrate before binding it as a hard gate.
