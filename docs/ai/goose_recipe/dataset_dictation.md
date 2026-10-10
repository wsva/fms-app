# Process a Dictation Dataset (engine-driven)

> **How to run this.** The other files in this folder are `.yaml` goose *recipes*
> (`goose run --recipe …`). This one is markdown, so it is a goose *instruction file*:
>
> ```
> goose run -i docs/ai/goose_recipe/dataset_dictation.md --with-streamable-http-extension http://127.0.0.1:35711/mcp
> ```
>
> Flags checked against goose 1.53.0, where `-i` is `--instructions` and `-r` on
> `goose session` means *resume*, not recipe. Interactive instead: `goose session
> --with-streamable-http-extension http://127.0.0.1:35711/mcp`, then point it at this file.
> To turn it into a `-r` recipe, wrap the body below in `instructions: |-` under
> `version` / `title` / `description`, add `activities: []` and `parameters: []`, and check it
> with `goose recipe validate`.
>
> It differs from the other recipes in one way that matters: it drives the
> **workflow engine** rather than calling pipeline tools in a fixed order, so the
> run is resumable, the app's Workflow page shows the same state the agent sees,
> and steps the dataset already satisfies are never re-run.

## Goal

Take one dictation dataset from whatever state it is in to a fully prepared one —
media registered, subtitles generated *and* imported, waveforms, cues aligned to
`book.txt` / `transcript/` when they exist, cue times snapped to silence — while
recording every transition in a workflow run stored inside the dataset.

The pipeline definition is `dataset_dictation` (built into the binary). Read it with
`workflow_builtin_templates`; the file in the repo
(`src-tauri/src/workflow/templates/dictation/dataset_dictation.yaml`) is the same text,
but an agent should never read files out of the source tree when a tool returns them.

## Prerequisites

- fms-app desktop running. It starts its web service on launch, and the MCP server is
  mounted on that at `http://127.0.0.1:35711/mcp` (Streamable HTTP, rmcp) — that URL is what
  goose connects to. If the port was taken, `web_service_get_status` reports the bound one.
- A dataset uuid (from `dataset_list`), or a folder path holding `media/` when the
  dataset does not exist yet — `init_dataset` mints the uuid.
- An STT model for the transcription step: `model_status` → `model_download` → `model_load`.
  `dataset_full_report` tells you whether one is loaded.

## Workflow

### Step 0 — one report instead of five calls

Call **`dataset_full_report`** `{ uuid }`. It is read-only and writes nothing.

Read `hint` first — it names the next call. Then, in this order:

- `audit.facts` — the six guard facts (`has_media`, `has_subtitles`, `has_waveforms`,
  `has_database`, `has_book`, `has_transcript`) that a step's `when:` reads.
- `audit.checks[]` — every check with a stable `id`, an exact `count`, up to 100
  offending files in `items`, and `advice`. **`count: 0` means measured and clean; an
  id that is absent from the list means not measured and proves nothing.**
- `audit.media_on_disk` vs `audit.media_in_db`, `subtitles_in_db`, `cues_in_db`.
- `workflow.runs[]` — one entry per run recorded inside the dataset: `steps` (per-step
  status), `verdicts` (per-step verdict), `ready` / `blocked` / `failed`, the engine's
  `hints`, and `next.steps` with each runnable step's `action` and its `params` / `inputs`
  already resolved.
- `stt_model.active_version` — `null` means transcription will fail.

### Step 1 — get a run to work in

**If `workflow.run_count` is 0:**

1. `workflow_builtin_templates` → take the `dataset_dictation` entry's `yaml`.
2. `workflow_adopt` `{ dataset_uuid, run_id: "dictation-<uuid>", definition_yaml: <that yaml> }`.

`workflow_adopt` opens the run *and* marks `completed` every step whose `adopt:` checks
were all measured clean — so a dataset prepared by hand, by another device or by an
earlier session shows its real state instead of offering work already done. Read:

- `adoption.adopted` — the steps it marked completed without running.
- `adoption.withheld[]` — the steps it refused, each with `why`: `"not measured"` (the
  check could not run) or `"the evidence disagrees"` (the check found offenders).
- `adoption.refreshed` / `adoption.refresh_error` — whether the run was rebound to the
  current template text. Only a run whose history holds nothing but inherited progress
  can be rebound; a run that earned progress keeps the definition it was created from.
- `adopt_hint` — one sentence saying the same.

Use `run_id: "dictation-<uuid>"`: that is the id the app's Workflow page uses, so the
agent and the page share one run instead of creating two.

**If a run already exists**, do not create another. If `workflow.runs[0].error` is set the
run is unreadable — tell the user before recreating or removing
`<dataset>/workflows/<run_id>`.

### Step 2 — reconcile the record with the disk

`workflow_verify` `{ dataset_uuid, run_id }` (no `apply`, so it writes nothing) → `verdicts`:

- `verified` — the artifacts this step claims exist and agree with each other.
- `drifted` — a `proves` check has offenders, so a `completed` step is contradicted.
- `unknown` — the step binds no checks, or a bound id was not measured (`undecidable`
  lists which).

If `verification.drifted` is non-empty: show the user which steps drifted and the
offending files (from `audit.checks[].items`), then call `workflow_verify` again with
`apply: true`. That is the only way a `completed` step returns to `ready`, and it costs an
`invalidated` event. Never mark a drifted step complete yourself — re-run its action.

### Step 3 — run what is runnable, one step at a time

`workflow_next` `{ dataset_uuid, run_id }` → `steps[]`, each with `id`, `action`, `params`,
resolved `inputs` and `declared_outputs`. Then, per step:

1. `workflow_advance` `{ dataset_uuid, run_id, step, agent_id: "goose" }` — claims a lease,
   so two agents cannot run the same step, and hands back the action to perform.
2. Perform the action (table below).
3. `workflow_record` `{ dataset_uuid, run_id, step, event: "completed",
   detail: { outputs: { … } } }` — include **every** name in `declared_outputs`, even when
   the value is the string `"false"`: a dependent step's `when:` guard reads those outputs,
   and a missing one leaves the dependent `blocked` forever.
4. On error: `workflow_record` with `event: "failed"` and `detail: { reason: "<the tool's
   message>" }`. Steps that declare `retry` go back to `ready` on their own until the
   attempts run out; the rest become `failed` immediately.

Repeat until `workflow_next` returns no steps. Do not run a `blocked` step — it is waiting
on a dependency's output. A step whose `when:` guard is false is `skipped` by the engine;
never force it with `workflow_intervene`.

### Step 4 — action table

| step | action tool | params | record as `outputs` |
|---|---|---|---|
| `ensure_model` | `model_status` → `model_download` → `model_load` | `version` | `model_version` |
| `init_dataset` | `dataset_init_dir` | `path` (folder already holding `media/`) | `dataset_uuid`, `media_count` |
| `sync_media` | `dataset_sync_media` | `uuid` | `added_count`, `removed_count` |
| `generate_subtitles` | `dataset_generate_subtitles` | `uuid` | — |
| `generate_waveforms` | `dataset_generate_waveforms` | `uuid` | — |
| `detect_reference` | `dataset_list_files` | `uuid` | `has_book`, `has_transcript` |
| `align_cues` | `dataset_align_cues` | `uuid` | — |
| `align_cues_transcript` | `dataset_align_cues_transcript` | `uuid` | — |
| `adjust_cue_times` | `dataset_adjust_cue_time` | `uuid`, `mode`, `strategy` | — |

Per-action notes:

- `init_dataset` takes a **path**, not a uuid — it mints the uuid, writes `info.json` and
  creates an empty `data.sqlite3`. Every later step uses the uuid.
- `ensure_model` has no `verify:` binding, so its status comes only from what you record.
  Report the loaded `model_version` explicitly.
- `generate_subtitles` transcribes **and** imports: an active `listen_subtitle` plus
  version-1 cues per media, written idempotently. There is no "build database" step in this
  pipeline; `dataset_write_subtitles_to_db` is an offline repair tool, not a stage.
- `detect_reference` decides two guards. Report `has_book` / `has_transcript` as the strings
  `"true"` / `"false"` from `dataset_list_files` (and from `audit.facts`, which already
  knows both).
- `align_cues` and `align_cues_transcript` are independent of each other, not a sequence:
  each is guarded by its own material (`book.txt` / `transcript/`), and both run when both
  exist. Each sets the cue `reference` field; neither changes cue text.
- `adjust_cue_times` defaults to `mode: "new"` — adjusted cues are written as a new subtitle
  version alongside the originals. Use `mode: "in_place"` only when the user asks, and
  `force: true` only deliberately (it re-adjusts subtitles that already carry the adjustment
  note). Strategies: `noise_floor` (default), `dual_bound`, `peak_relative`, `otsu`; try
  another only if the result is poor, and say which one you used in `detail`.
- If `adjust_cue_times` reports media skipped for lack of a waveform, `generate_waveforms`
  is not actually finished — the audit's `adjust_blocked_no_waveform` check will name them.

### Step 5 — close

1. `workflow_verify` `{ dataset_uuid, run_id, apply: true }` once at the end: a step the
   evidence contradicts must not stay green.
2. `dataset_full_report` again, and tell the user: media / subtitle / cue counts, the checks
   that still have offenders (with the files), the per-step status, and what remains open
   and why.
3. If `workflow.runs[0].run_status` is `completed`, say so plainly and stop.

## Recovery

- After any restart or crash call `workflow_status` `{ dataset_uuid, run_id }`: it recomputes
  derived statuses and reaps expired leases, so a step a dead agent left `running` becomes
  runnable again. This is what makes the run resumable — read it before doing anything else.
- `workflow_intervene` `{ dataset_uuid, run_id, step, op }` — `retry` (failed → ready,
  attempt counter reset), `unblock` (blocked → pending), `skip`, `reset` (wipe the step's
  state). These are manual overrides: ask the user before `skip` or `reset`, and never use
  them to get past a check that is telling the truth.
- Every transition is journalled in `<dataset>/workflows/<run_id>/events.jsonl`. `completed`
  after `started` is progress the run earned; `adopted` and `invalidated` are progress it
  inherited or had taken away. `workflow_list` summarises runs without loading them.

## Rules

- **A check that could not be measured is not clean.** `count: 0` proves something only when
  the subject exists — that is what the `media_none` check is for, and why an empty dataset
  adopts nothing.
- **Never invent a status.** Statuses come from `workflow_record`, `workflow_verify` and
  `workflow_adopt`. What you decide is which action to run and what its outputs were.
- **Read-only first.** `dataset_full_report` and `workflow_verify` without `apply` never
  write; call them as often as you like.
- **One step at a time.** Subtitle generation and cue adjustment can take minutes on a large
  dataset. Record each step before starting the next, so an interruption leaves a resumable
  run rather than an unknown one.
- **Stop on error and report it.** Tool messages are written to be actionable — they say what
  to call next.
