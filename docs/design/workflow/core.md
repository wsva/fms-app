# Persistent Workflow State Machine

A generic, file-based framework for describing and running multi-step processes that an AI agent can drive — and **resume after any interruption without relying on conversation history**.

Nothing here is domain-specific. The framework knows about *steps*, *dependencies* and *statuses*; it does not know what a step actually does. Actions are symbolic names bound to pluggable executors (an MCP tool, a Tauri command, a sidecar script, or "hand the step back to the agent"). The examples below use a dataset-processing pipeline purely to illustrate the file formats.

> **This directory splits by layer, not by feature.** `core.md` (this file) is the engine spec and must never name a domain concept; sibling documents describe one pipeline each — [`dictation.md`](./dictation.md) for the built-in dictation pipeline, its read-only audit and the Workflow page. Mechanism invented while solving a pipeline's problem is written up in that pipeline's doc and promoted here once the engine actually honours it.

## Design principles

- **Definition vs. state vs. events are separate concerns.** What *should* happen is immutable intent; what *has* happened is derived truth; the event log is the audit trail.
- **File-based and diffable.** Plain-text YAML/JSON/JSONL — human-readable, agent-readable, version-controllable, sync-friendly. No hidden in-memory or database-only state.
- **Resumable and idempotent.** Re-reading the files and re-running the evaluation loop always yields the same "what to do next", so a crashed or restarted agent continues exactly where it left off.
- **Action-agnostic.** The engine schedules and records; executors do the work. Adding a new kind of work never changes the engine.
- **Agent-drivable.** A small generic command surface (load / status / next / advance / intervene) is all an agent needs; every response is structured JSON with actionable hints.

---

## Goal

A persistent workflow state machine: the **workflow definition** describes what should happen and the dependencies, while the **state** records what has actually happened. This is a very good pattern for an AI agent because the agent can resume after interruption without relying on conversation history.

The event log is the ultimate source of truth; `state.json` is a materialized snapshot that can always be rebuilt by replaying the events. Keeping state derived from events means a partial write or a crash never leaves the run in an unrecoverable place.

---

## Files

One directory per workflow run holds three files:

```
workflow.yaml    ← what should happen   (definition — immutable intent)
state.json       ← current status        (snapshot — derived, resumable)
events.jsonl     ← everything that happened (append-only audit log)
```

| File | Role | Written by | Mutable? |
|---|---|---|---|
| `workflow.yaml` | The DAG of steps + how to run them | author / generator | No (edit → bump `version`) |
| `state.json` | Per-step status + run metadata | engine (atomic rewrite) | Yes (fully overwritten each step) |
| `events.jsonl` | Ordered log of every transition | engine (append-only) | Append only, never rewritten |

---

## workflow.yaml — the definition

A definition is a named, versioned set of **steps** forming a directed acyclic graph (DAG) via `depends_on`.

### Step schema

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Unique step id; the key used in `state.json` and events |
| `description` | no | Human/agent-readable intent |
| `action` | yes | Symbolic executor name (resolved at run time, not by the engine) |
| `depends_on` | no | Ids that must reach a *satisfied* state before this step is eligible |
| `when` | no | Condition/guard; if false the step is `skipped` (not failed) |
| `inputs` | no | Named references to other steps' outputs (see *Data flow*) |
| `outputs` | no | Named values this step produces, stored in state |
| `params` | no | Static arguments passed to the executor |
| `retry` | no | `{ attempts, backoff }` — how many times to re-run on failure |
| `timeout` | no | Max run duration; exceeding it ⇒ `failed` (or retry) |
| `on_failure` | no | `stop` \| `continue` \| `skip_dependents` (default `stop`) |

Only `id` and `action` are mandatory; everything else has a sensible default so a minimal definition stays small.

### Example (illustrative only — a dataset pipeline)

> This is one *instance* of the format. The framework places no constraints on what these actions are; `download`, `transcribe`, `generate_waveform`, etc. are just names bound to executors elsewhere.

`````yaml
name: process_dataset
version: 1

steps:
  - id: download
    description: Download the source dataset
    action: download_dataset

  - id: extract_audio
    description: Extract audio from all media files
    action: extract_audio
    depends_on:
      - download

  - id: transcribe
    description: Transcribe audio
    action: transcribe
    depends_on:
      - extract_audio

  - id: normalize_text
    description: Normalize transcription text
    action: normalize_text
    depends_on:
      - transcribe

  - id: generate_waveform
    description: Generate waveform data
    action: generate_waveform
    depends_on:
      - extract_audio

  - id: validate
    description: Validate the processed dataset
    action: validate
    depends_on:
      - normalize_text
      - generate_waveform

  - id: export
    description: Export final dataset
    action: export
    depends_on:
      - validate
`````

Note the shape: `transcribe → normalize_text` and `generate_waveform` are two independent branches that both hang off `extract_audio`, and `validate` is a **join** that waits for both. The engine discovers this from `depends_on` alone.

### Validation (checked before a run starts)

- Step `id`s are unique.
- Every `depends_on` / `inputs` reference points at an existing step id.
- The graph is **acyclic**.
- Every `action` resolves to a registered executor (fail fast, not mid-run).
- `name` and `version` are present.

A definition that fails validation is rejected outright; the run never starts.

---

## States

Every step is in exactly one state:

| State | Meaning | Terminal? |
|---|---|---|
| `pending` | In the definition, but dependencies not yet satisfied — not eligible | no |
| `ready` | All dependencies satisfied and `when` is true — eligible, not yet claimed | no |
| `running` | An executor has claimed it; work in progress (or interrupted) | no |
| `completed` | Executor returned success; outputs recorded | yes |
| `failed` | Executor errored and retries are exhausted | yes (retry ⇒ `ready`) |
| `blocked` | Cannot proceed for a *non-error* reason: waiting on input/approval/an external resource, or a dependency failed under a blocking policy | no |
| `skipped` | Deliberately not run: `when` false, or an upstream skip propagated | yes |

`blocked` vs `failed` matters: **failed** means work was attempted and errored; **blocked** means the step is waiting on something outside its own execution and may become `ready` again with no code change.

### Transitions

```
            deps satisfied / when true
 pending ───────────────────────────────► ready
   │                                        │ claimed
   │ when false / upstream skip             ▼
   └──────────────────────────► skipped   running ──┬──► completed
                                        ▲           ├──► failed ──(retries left)──► ready
                                        │           └──► blocked
                          unblocked ────┘                    │
                                                             └──(abandoned)──► skipped
```

- `pending → ready` — dependencies satisfied and `when` true.
- `ready → running` — an executor claims the step (single-flight; see *Concurrency*).
- `running → completed | failed | blocked` — executor outcome.
- `failed → ready` — automatic retry while `attempts` remain.
- `blocked → ready` — the blocking condition clears (input arrives, dependency is retried and succeeds, manual unblock).
- `any → skipped` — `when` false, or propagation from an upstream skip/ignore.

`completed`, `failed` (retries exhausted) and `skipped` are terminal for the run unless an agent intervenes (retry / unblock / reset).

### Readiness derivation

Statuses are **derived**, not guessed. On every evaluation the engine recomputes:

- A dependency is **satisfied** when it is `completed`, or `skipped` *and* the dependent's policy allows a skip to satisfy it.
- A step with all dependencies satisfied and `when` true ⇒ `ready`.
- A step with a dependency that is `failed` or `blocked` ⇒ `blocked` (or `skipped` under `on_failure: skip_dependents`).
- Otherwise ⇒ `pending`.

This is a pure function of the definition + current statuses, so it is stable and reproducible across restarts.

---

## The evaluation loop

Deterministic and idempotent:

1. Load `workflow.yaml` + `state.json`.
2. Recompute derived statuses from the dependency graph (*Readiness derivation*).
3. Select all `ready`, unclaimed steps — there may be several (parallel branches).
4. For each: **claim** (set `running`, write a lease/heartbeat), execute its bound action, capture the result.
5. **Append** the event(s) to `events.jsonl`, then **atomically rewrite** `state.json`.
6. Repeat until there are no `ready` and no `running` steps.

The run is **terminal** when nothing is runnable:
- all steps `completed`/`skipped` ⇒ workflow **completed**;
- any step `failed` with `on_failure: stop` ⇒ workflow **failed**;
- only `blocked` steps remain ⇒ workflow **blocked** (waiting, resumable).

Because step 2 is a pure recompute, re-entering the loop from any persisted state produces the same next actions — the property that makes resume safe.

---

## Resumability & crash safety

This is the reason the framework exists.

- **State is the resume point; events are the audit.** A restarted agent reads `state.json`, ignores any prior conversation, recomputes, and continues.
- **Atomic writes.** `state.json` is written to a temp file then renamed, so a crash mid-write never leaves a half-written snapshot. Events are appended and flushed.
- **Interrupted steps.** A step left `running` whose lease has expired on resume is treated as *interrupted*. Policy: re-run it (executors should be **idempotent**) or mark it `failed`. The lease/heartbeat distinguishes "still running elsewhere" from "died mid-flight".
- **Idempotent executors.** Because a step may be re-run after an interruption, actions should be safe to repeat (or record enough checkpoint data to resume). The framework does not assume atomicity of the work itself.
- **Version binding.** `state.json` records the definition `name` + `version` (and optionally a content hash). If the loaded definition differs, the state is **stale**: require an explicit migration or reset rather than silently applying old state to a new graph.

---

## Data flow between steps (optional)

Steps can pass values forward without the engine knowing what they mean:

- A step declares `outputs`; the engine stores the produced references in `state.json` under that step.
- A later step declares `inputs` referencing them, e.g. `${steps.transcribe.outputs.text}` or `${steps.download.outputs.path}`.
- The executor reads its resolved inputs and writes its outputs; the framework only records the references.

If a workflow prefers, steps can instead communicate through an external store (filesystem, DB) and let the framework track **status only** — data flow is an opt-in convenience, never a requirement.

---

## Failure, retry & propagation

- **Retry** — `retry: { attempts, backoff }`: `failed → ready` until attempts are exhausted, then terminal `failed`.
- **Timeout** — exceeding `timeout` yields `failed` (and retries if configured).
- **`on_failure`** — per step (or a workflow default):
  - `stop` — halt the run; dependents stay `pending`/`blocked`.
  - `continue` — independent branches keep running; only the failed step's dependents are affected.
  - `skip_dependents` — mark the transitive dependents `skipped` and move on.
- **Blocked propagation** — a step whose dependency is `blocked`/`failed` becomes `blocked` (recoverable) rather than `failed`, so clearing the upstream issue resumes the downstream automatically.

---

## Concurrency

- Independent steps (no dependency path between them) may run in parallel; the loop selects *all* `ready` steps each pass. In the example, `transcribe` and `generate_waveform` both depend only on `extract_audio`, so they can run together; `validate` joins them.
- **Single-flight per step** is enforced by the claim/lease: two agents (or two loop iterations) never run the same step twice.
- The engine is otherwise stateless between passes, so concurrency is bounded only by the executor registry's own limits.

---

## Agent interface

An agent never needs to understand the domain — only a small generic command surface (each returns structured JSON with actionable hints):

| Command | Purpose |
|---|---|
| `list` / `load` | Discover runs; load a definition + its state |
| `status(run)` | Definition + current state + what's `ready`, `blocked`, `failed`, and *why* |
| `next(run)` | The runnable step(s) with resolved action + inputs |
| `advance(run, step)` | Execute the bound action, record the result, re-evaluate |
| `record(run, step, event, detail)` | Report an outcome (esp. when the agent itself did the work) |
| `intervene(run, step, op)` | `retry` / `unblock` / `skip` / `reset` for manual recovery |

Hints are actionable, e.g. *"step `validate` is blocked because dependency `normalize_text` failed; retry `normalize_text` or skip `validate`."*

Two execution styles share the same three files:

- **Agent-in-the-loop** — the agent calls `next()`, performs the action itself (often via other tools), then reports back with `record()`. The framework is pure bookkeeping + scheduling. Best when steps are themselves reasoning/LLM tasks.
- **Engine-driven** — the framework runs bound executors automatically and the agent only supervises and handles blocks. Best for deterministic pipelines.

---

## events.jsonl

An append-only event log.

JSONL has no surrounding `[]` and no commas between records. Each record is one transition; the ordered log can rebuild `state.json` from scratch.

`````jsonl
{"time":"2026-10-01 10:00:00","step":"download","event":"started"}
{"time":"2026-10-01 10:03:21","step":"download","event":"completed"}
{"time":"2026-10-01 10:03:25","step":"extract_audio","event":"started"}
{"time":"2026-10-01 10:20:10","step":"extract_audio","event":"completed"}
{"time":"2026-10-01 10:21:00","step":"transcribe","event":"started"}
`````

Event vocabulary (extensible): `claimed`, `started`, `completed`, `failed`, `retried`, `blocked`, `unblocked`, `skipped`, `resumed`, plus run-level `workflow_started` / `workflow_completed` / `workflow_failed` / `workflow_blocked`. A record may carry an optional `detail` object (error message, attempt number, output refs, actor/device id).

---

## state.json

The current snapshot. Keys are **step ids from the definition**; values carry the status plus optional run metadata. A top-level block records which definition version this state belongs to (see *Version binding*).

`````json
{
  "workflow": { "name": "process_dataset", "version": 1 },
  "steps": {
    "download":          { "status": "completed", "finished_at": "2026-10-01 10:03:21" },
    "extract_audio":     { "status": "completed", "finished_at": "2026-10-01 10:20:10" },
    "transcribe":        { "status": "running",   "started_at": "2026-10-01 10:21:00", "lease": "agent-1" },
    "normalize_text":    { "status": "pending" },
    "generate_waveform": { "status": "completed", "outputs": { "path": "waveform/" } },
    "validate":          { "status": "pending" },
    "export":            { "status": "pending" }
  }
}
`````

Per-step fields are all optional except `status`: `started_at`, `finished_at`, `attempt`, `lease`/`heartbeat`, `outputs`, and a short `reason` (for `blocked`/`failed`) that surfaces directly in `status()` hints.

---

## Implementation sketch

Kept deliberately light — the engine is small and domain-free.

- **A self-contained module** (e.g. `src-tauri/src/workflow.rs`): parse → build DAG → evaluate readiness → run loop → persist. It contains **no** domain knowledge; executors are injected.
- **Storage:** one directory per run holding the three plain-text files, under a configurable location (e.g. the workspace dir). Plain text keeps them diffable, agent-readable and sync-friendly.
- **Parsing:** `serde_yaml` → a typed `Definition`; `serde_json` for state; line-append + flush for events. Atomic state writes via temp-file + rename.
- **Engine core:** topological readiness recompute, an async run loop (tokio), and an executor registry `action_name → Executor`. Single-flight claims + leases give crash-safe concurrency.
- **Pluggable executors** — the only place "what work means" lives. An executor can be: (a) a call to an existing MCP tool / Tauri command, (b) a sidecar script (the `resources/tools` pattern), or (c) *deferred to the agent* — the engine returns the step and the agent reports the result via `record`. The core is identical for all three, so the same framework serves deterministic pipelines and agent-in-the-loop reasoning.
- **Agent surface:** expose the commands above as MCP tools (`workflow_*`) so any agent can drive a run — structured JSON responses and actionable errors, matching the project's agent-friendly conventions.
- **Optional later:** a small UI (list runs, per-step status, retry/unblock/skip buttons). Not required for the framework to work.

### Explicitly out of scope

No domain-specific step types, no dataset assumptions, and no heavy workflow-engine features (cron scheduling, distributed workers, BPMN, visual editors). The goal is a **resumable, agent-drivable DAG runner** whose entire state lives in three text files.
