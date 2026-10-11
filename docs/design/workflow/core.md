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
| `workflow.yaml` | The DAG of steps + how to run them | author / generator — the engine rewrites it only when rebinding an *inherited-only* run (see *Refreshing a run's definition*) | No (edit → bump `version`) |
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
| `verify` | no | `{ blocks, proves }` — which external checks this step answers to. The two are not equally powerful: `proves` is the only list that can retract a `completed`, `blocks` only annotates a verdict and reports a gate (see *Verification*) |
| `adopt` | no | Check ids that must come back measured-and-clean for this step to be adopted as `completed` without running it (see *Verification*) |
| `params` | no | Static arguments passed to the executor, verbatim by the engine; the binding layer substitutes `${run.dataset_uuid}` (see *Agent interface*) |
| `retry` | no | `{ attempts, backoff }` — how many times to re-run on failure |
| `timeout` | no | Lease seconds — how long a claim on this step is honoured before `recompute` treats it as interrupted (default 300, see *Resumability*). Not a work budget: nothing here stops a running action. |
| `on_failure` | no | `stop` \| `continue` \| `skip_dependents` (default `stop`) |
| `on_skipped_dependency` | no | `satisfy` \| `block` \| `skip` (default `satisfy`) — how a dependency that ended `skipped` counts here: clears it, leaves this step waiting, or skips this step too (see *Readiness derivation*) |

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
- An `inputs:` or `when:` `${steps.X.outputs.Y}` reference may only name `X` if `X` is a **transitive ancestor** of the reading step (upstream via `depends_on`). A reference to a sibling or a descendant races on a value that may never be produced — it would strand a `when:` guard `blocked` forever — so it is refused here, not left to the run.
- Every `verify` / `adopt` id is one the domain's check provider actually registers — a typo would silently disable a gate or a promotion, so it fails like a bad `depends_on` does. The vocabulary is passed in from the binding layer as `(id, kind)` pairs; the engine only sees opaque strings plus a structural vacuity *class* (`per_subject` / `subject_present` / `file_exists`, see *Verification*).
- An `adopt:` list is **non-vacuous**: if it names any `per_subject` check (one that counts offenders over a collection and reads clean on an empty one), it must also name a check that can report an offender on an empty subject (`subject_present` or `file_exists`). Otherwise adopting on it would certify work nobody did.
- The graph is **acyclic**.
- `timeout`, when present, is positive (`timeout: 0` would reap a claim the instant it is taken). `retry.backoff`, when present, reads as a lenient duration (`30s` / `5m` / `1h`) or is empty — a typo like `30x` would otherwise read as "no delay".
- Every `action` resolves to a registered executor (fail fast, not mid-run). *(Skipped in this build: it is agent-in-the-loop, so actions are symbolic names the agent resolves at run time.)*
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
| `blocked` | Cannot proceed for a *non-error* reason. Two kinds, tagged in state: a **`dependency`** block is derived from the graph (an upstream failed, is blocked, or is skipped under a `block` policy) and the recompute lifts it the moment the upstream is fixed; an **`external`** block was set by a caller (`record blocked`) awaiting input/approval/an outside resource and is a judgement the graph must never overwrite | no |
| `skipped` | Deliberately not run. Tagged with its cause: `when_false` (a live guard reading — re-derived whenever its inputs change), `manual` (a `record`/`intervene` decision — terminal until a human changes it), or `propagated` (an upstream skip/`skip_dependents` reached it — re-derived with the upstream) | yes (a `manual` skip; the others are re-derived) |

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
- `blocked → ready` — the blocking condition clears. A `dependency` block clears by itself on the next recompute once the upstream is fixed; an `external` block clears only on an explicit `intervene unblock`.
- `completed → ready` — **invalidated** by evidence (see *Verification*), or by **cascade**: demoting a step reopens every already-`completed` transitive dependent whose upstream is no longer satisfied, so a green node never sits on top of a stale one. Never by a clean probe.
- `pending | ready → completed` — **adopted**: the step's `adopt` evidence is already clean on disk (see *Verification*). The only upward move, and only for a step that asked for it.
- `any → skipped` — `when` false, or propagation from an upstream skip/ignore.

`completed`, `failed` (retries exhausted) and a **`manual`** `skipped` are terminal for the run unless an agent intervenes (retry / unblock / reset). A `when_false` or `propagated` skip is *not* — it is a live derivation, re-evaluated every pass, so resetting an upstream that changes the guard reopens it.

### Readiness derivation

Statuses are **derived**, not guessed. On every evaluation the engine walks the graph in topological order and recomputes, preserving anything that is a recorded decision rather than a derivation:

- `completed`, `failed` and a live-`running` claim are preserved (a terminal judgement, or work in flight).
- A `blocked` step is preserved **only** when the block is `external` — a caller parked it, and the graph may not silently reopen it. A `dependency` block is re-derived (and lifted when the upstream heals).
- A `skipped` step is preserved only when it is `manual` (or a legacy skip with no recorded reason). A `when_false` or `propagated` skip is re-derived every pass.
- A dependency is **satisfied** when it is `completed`, or `skipped` under the dependent's `on_skipped_dependency` policy: `satisfy` clears it (default), `block` leaves the dependent waiting, `skip` skips the dependent too.
- A step with all dependencies satisfied and `when` true ⇒ `ready`; a `when` that resolves false ⇒ `skipped` (`when_false`); a dependency `failed`/`blocked`, or `skipped` under a `block` policy ⇒ `blocked`; otherwise ⇒ `pending`.

**The invariant** *("completed ⇒ its dependencies are satisfied")* is enforced as a **cascade**: whenever a step moves *down* (a `verify` demotion, a terminal `failed`, or an `intervene reset`/`retry`), every already-`completed` transitive dependent whose upstream is no longer satisfied is reopened to `ready` in the same atomic pass, each journalled with an `invalidated` event naming `cascaded_from`. Without it, demoting `transcribe` would leave `validate`/`export` green on top of a stale result — the exact false-`completed` the verification seam exists to catch.

The recompute is a pure function of the definition + current statuses, so it is stable and reproducible across restarts.

---

## Verification — scoring progress against evidence

Readiness is derived from the graph; it says nothing about whether the *artifacts a step produced* still exist. Verification is the one seam where the outside world is allowed to speak about a step, and the framework deliberately keeps it thin:

- **The definition declares the relationship**, per step: `verify: { blocks: [check ids], proves: [check ids] }`. The two lists are not symmetric, and the asymmetry is the design: `proves` is about the output *this step claims*, so it is the only list that can judge a `completed` step and the only one that can ever change a status. `blocks` is about the *preconditions*, so it can only say "not yet" about a step that has claimed nothing. A step with no `verify:` is legitimately `unknown` — the absence of a binding is a statement, not a gap.
- **`blocks` is advice, and only advice.** Offenders on a bound `blocks` id come back in the verdict beside the offending counts, and the step is listed in `verification.gated` — but only while it is `pending` or `ready`, because a gate on a step that already claims completion is a statement about someone else's files. Nothing is written, no status moves, and `next()` still hands the step back as ready: readiness is derived from the graph alone, so **holding off on a gated step is the caller's job**, and a step bound by `blocks` alone can only ever read `unknown` — nothing is claimed about its output, so nothing can agree or disagree with it, whatever the gate says. Withholding a gated step inside `next()` is the tempting alternative; it is refused because a decision that depends on a filesystem reading no one persisted would make the loop non-reproducible, which is the one property *Readiness derivation* exists to guarantee. Do not confuse the `blocks:` list with the `blocked` *state* — that one is derived from the graph (a failed or blocking-policy dependency) and is written to `state.json`; this one never is.
- **Checks are not engine concepts.** A check needs the domain's data access, so it lives in a domain module that registers it under a stable id and hands the engine a `id → offender count` map. The core never names a check, a file kind or a repair sentence; it only compares opaque ids against the binding. Adding a new pipeline means a new provider, not an engine change.
- **Verdicts are a projection.** `verified` / `drifted` / `unknown` are recomputed on every call and never written to `state.json`, so `state.json` stays exactly a replay of `events.jsonl` and a verdict means "as of this probe". Render it beside a node's status, never instead of it.
- **A probe that could not measure cannot certify.** If any id a step binds was absent from the evidence map (another domain's vocabulary, a short-circuited report), a would-be `verified` downgrades to `unknown`. A clean bill of health over ids nobody measured is a lie.
- **A probe only ever moves a step down.** With `apply` set, a `completed` step whose `proves` evidence reports offenders goes back to `ready` (outputs kept, so `when:` guards still resolve). A clean probe proves an artifact *exists*, never that this run made it — so verification has no upward path, no promotion, ever.
- **Adoption is the one upward move, and a step has to ask for it.** `adopt: [check ids]` says: "if every one of these ids comes back measured *and* with zero offenders, record me `completed` — the artifact is already on disk, and making the user run me again would claim credit for work nobody did." Opt-in per step and deliberately rare, because **vacuity** is the trap: a check that counts offenders over an empty subject (no media ⇒ no media is missing subtitles) is vacuously clean. So each check id carries a structural **class** the binding layer assigns (`per_subject`, `subject_present`, `file_exists`) and validation refuses an `adopt` list of only `per_subject` ids unless it also names a guard that reports an offender on an empty subject — a `subject_present` id (fails when the whole subject is absent) *or* a `file_exists` id (a single file's existence, which can never be vacuously true; this is how a `sync_media`-style list guarded by `database_missing` stays honest).
- **Adoption runs in topological order and only over satisfied dependencies.** A step is adopted only once every dependency is already `completed`/`skipped`-under-policy/adopted-earlier-this-pass; otherwise it is withheld with reason `dependency not satisfied`, so a green tick never lands on top of an unstarted parent. And a step whose declared `outputs` are read back by a dependent is withheld with reason `outputs not derivable` unless the adopt call supplies those values — inheriting the step but not its data would strand the dependent in a silent block.
- Adoption never touches a step already `completed`, `failed`, `skipped` or `running` — a human's judgement outranks the filesystem — and it withholds *with a reason* (`not measured`, `the evidence disagrees`, `dependency not satisfied`, `outputs not derivable`) rather than silently doing nothing.
- **Every status change costs an event.** Invalidation appends `invalidated` and adoption appends `adopted`, like any other transition; neither gets a privileged path into `state.json`. A probe or adoption that changes nothing writes nothing.

The honesty rules — probes down-only, promotion only where a step opted in over non-vacuous evidence, and never over an id nobody measured — are what make it safe to let a read-only probe influence a run at all. The dictation instantiation (which check answers to which step, and what a probe cannot see at all) is in [`dictation.md`](./dictation.md#binding-checks-to-the-template).

### The same rules in plain terms

The rules above are dense; this is the same content said plainly. The check ids are the illustrative ones from *Verification by example* below — no pipeline named here is a real one.

A run is a checklist, each item ticked when it finishes — and a tick only ever says *"this finished at that moment."* Nothing re-opens it. So if `transcribe` went green on Monday and someone deletes its transcripts on Tuesday, the checklist still reads green, `next()` finds nothing to do, and an agent obediently does nothing. **Verification is the spot check that catches that.**

- **Who does the checking.** A separate piece of code looks at the real files and returns numbers — `transcript_missing: 2`. The engine has no idea what a transcript is: it receives a list of check names with an offender count each and matches opaque strings against the binding in the YAML.
- **What each list is for.** `proves: [transcript_missing]` means "findings here say my result has gone bad". `blocks: [audio_missing]` means "findings here say I should not have started yet" — which the engine reports and does not enforce.
- **The three labels.** *verified* — the check ran and found nothing. *drifted* — it ran, found offenders, and so contradicts the green tick. *unknown* — there is no binding, or the check never ran: an honest "I can't tell", never a pass. They are shown beside the status and are not saved anywhere.
- **A spot check may only move a step backwards.** A `completed` step found drifted goes back to `ready` so it can be redone. Files that look fine do **not** turn a step green: their existence never proves *this run* made them — they may be someone else's work from before, or shipped with the subject. The reasoning is the asymmetry: wrongly saying "done" stops all work, while wrongly saying "not done" costs only some extra work.
- **The one exception is adoption.** A step may opt in with `adopt: [subject_absent, waveform_missing]`, meaning "if every one of these comes back measured *and* clean, mark me done without running me — the result is already on disk". That is what keeps a subject prepared by hand, or by another device, from being reprocessed.
- **The trap is a vacuous pass.** "No audio file is missing waveforms" is satisfied by a folder holding no audio at all — no files, so no offenders. Adopt on that and a green tick lands over nothing. So every check is tagged `per_subject` / `subject_present` / `file_exists`, and an `adopt` list must name, beside its `per_subject` checks, a guard that reports an offender on an empty subject (`subject_present`, or a single-file `file_exists` check) — validation rejects the list outright otherwise. Adoption is also refused if any listed check did not run, if a dependency has not been satisfied, or if the step feeds a dependent an output the call did not supply.

> In one line: verification lets the real files **veto** a green tick, and lets them **grant** one only for the steps that asked for it, only when the evidence is solid.

### Why this seam exists

A status answers *"what did this run do?"*, not *"what is true on disk right now?"* `completed` is recorded the instant an executor returns success, and nothing in the graph ever re-opens that claim against the world. Left alone, three ordinary events silently turn a green step into a lie:

- **Out-of-band change.** A human, another run, or a sync deletes, edits or regenerates an artifact *after* the step completed. The state still says *done*; the files say otherwise.
- **Interruption without a clean result.** A machine moved or a lease expired, and the output the step is credited for never landed — or landed somewhere now gone.
- **Inherited credit.** A run was adopted or created on a subject already processed, so its `completed` reflects someone else's work rather than a transition this run earned.

For a framework whose whole promise is *resume from files, ignore the conversation*, a stale `completed` is the most dangerous value a step can hold: `next()` reports nothing to do and the agent obeys. Getting "done" wrong costs more than getting "not yet" wrong, because a false `completed` **stops** work while a false `pending` merely starts it — the failure mode verification exists to catch is over-confidence, not under-reporting.

The tempting alternatives are each rejected on principle:

- **Fold the checks into `depends_on` / `when`.** A dependency edge is a statement about ordering *within one run*, evaluated once at scheduling time; it cannot re-say "the artifact that satisfied you three days ago has since gone stale". Readiness and truth are two different axes, so they stay two different mechanisms.
- **Trust the executor's return code.** That code was a claim about a single moment, and the world is mutable afterward.
- **Let a probe re-mark a step `completed` when the files look right.** That is precisely the lie the honesty rules forbid: clean files prove an artifact *exists*, never that *this run* made it. Promotion therefore stays the one explicit, opt-in, non-vacuous `adopt:` path — never a by-product of a probe.

So verification is the *single* place the outside world is allowed to speak about a step, and the design's effort goes into making that narrow permission safe to grant: it stays thin (opaque ids and a count map), a projection (never written into `state.json`), and directional (it demotes freely but promotes only where a step asked and the evidence genuinely rules out vacuity). Every rule above is downstream of keeping this one seam honest.

### Verification by example

The rules above are deliberately abstract. This walks one *illustrative* binding through every outcome the engine can produce. It reuses the `process_dataset` steps from earlier and invents generic check ids (`audio_missing`, `transcript_missing`, `waveform_missing`, `subject_absent`) — none of this is a real pipeline; it is only here to show what each verdict and each move *means*.

Suppose two steps carry a binding (and `download`, from the example, carries none):

`````yaml
  - id: transcribe
    action: transcribe
    depends_on: [extract_audio]
    verify:
      blocks: [audio_missing]        # findings here mean "not yet"
      proves: [transcript_missing]   # findings here mean "my output no longer holds"

  - id: generate_waveform
    action: generate_waveform
    depends_on: [extract_audio]
    verify:
      proves: [waveform_missing]
    adopt: [subject_absent, waveform_missing]   # opt in to the one upward move
`````

A probe hands the engine a single evidence map — `id → offender count` — say `{ transcript_missing: 0, waveform_missing: 2, subject_absent: 0 }`. Every verdict below is a projection of that map against these bindings, recomputed on each call and never written to `state.json`.

**1. No binding ⇒ `unknown`, and that is an answer.** `download` binds nothing, so no probe can say anything about it. Its verdict is `unknown` — not a gap to fill but a deliberate statement that the engine holds no subject-side evidence for this step.

**2. `proves` clean *and measured* ⇒ `verified`.** `transcribe` binds `proves: [transcript_missing]` and the map reports it at `0`. The id was measured and came back clean, so `transcribe` reads `verified` — *as of this probe*. Nothing is persisted.

**3. `proves` with offenders ⇒ `drifted`; `apply` demotes.** `generate_waveform` binds `proves: [waveform_missing]` and the map reports `2` ⇒ verdict `drifted`. On its own that is only a badge beside the node's status. If the call sets `apply` *and* the step was `completed`, the engine moves it back to `ready` (keeping its recorded `outputs`, so `when:` guards still resolve), appends one `invalidated` event naming `waveform_missing`, and rewrites state. A second identical probe finds it already `ready` and writes nothing.

**4. A clean id nobody measured cannot certify.** Imagine a short-circuited probe that returns `{ subject_absent: 0 }` and omits `transcript_missing` entirely. `transcribe`'s only bound id is absent from the map, so its would-be `verified` downgrades to `unknown`. "Zero offenders" over an id nobody counted is not a clean bill of health — it is a lie, and the engine refuses to tell it.

**5. Verification never moves a step up.** Even with every `proves` id clean, a `pending` `transcribe` stays `pending`. A clean probe proves the artifact *exists*, never that *this run* made it — a shipped dataset is indistinguishable from a processed one. The only upward move is adoption, and a step has to ask for it.

**6. Adoption, and the vacuity trap.** `generate_waveform` opted in with `adopt: [subject_absent, waveform_missing]`. Adoption fires only when *every* listed id is present in the map *and* zero:

| Evidence map | `subject_absent` | `waveform_missing` | Result |
|---|---|---|---|
| `{subject_absent: 0, waveform_missing: 0}` | measured, clean | measured, clean | **adopted** → `completed`, one `adopted` event |
| `{subject_absent: 1, waveform_missing: 0}` | offender | clean | **withheld** — nothing to walk |
| `{waveform_missing: 0}` | absent | clean | **withheld** — id not measured |
| `{subject_absent: 0, waveform_missing: 2}` | clean | offenders | **withheld** — evidence disagrees |

Row 2 is the whole reason `subject_absent` sits on the list. `waveform_missing` counts offenders over the audio files, so with *zero* audio files it comes back `0` **vacuously** — "no audio, therefore no audio is missing waveforms". Adopting on that alone would paint a green tick over work nobody did. `subject_absent` is the id that *fails when the subject itself is missing*: it reports an offender exactly when there is nothing to walk, which turns a bundle of vacuous zeros into a real claim.

Adoption also never touches a step already `completed`, `failed`, `skipped` or `running` — a human's judgement outranks the filesystem — and it withholds *with a reason* (`not measured`, `the evidence disagrees`) rather than silently doing nothing.

**7. A `blocks` finding gates, and gating writes nothing.** Suppose the map also reports `audio_missing: 1`. `transcribe` binds it under `blocks`, so the verdict payload names the offender and `transcribe` appears in `verification.gated` — and that is the entire effect. Its status is untouched, no event is appended, and if it was `ready` then `next()` still offers it, because a probe is not the scheduler. Read the gate as what the evidence says about *starting* the step; the caller decides.

### Refreshing a run's definition

A run stores the definition text it was created from, and `load_bundle` enforces that `state.json` and `workflow.yaml` still agree — deliberately, so progress is never judged against intent that changed underneath it. That also makes a template edit a one-way door: every run opened before the edit would keep its older binding forever, and a check added to the vocabulary later would silently stop applying to datasets scanned earlier.

The engine allows exactly one narrow exception, decided by the run's own history rather than by a parameter:

- `history_is_inherited_only(run)` is true when `events.jsonl` holds nothing but the run opening, adoptions, invalidations and refreshes — no step was ever advanced, recorded, skipped, retried or reset. Such a run is a *projection* of a template, not a piece of work.
- Only for such a run may the binding layer call `rebind_definition(run, yaml)`: validate the new text first, carry the status of every step that still exists under the same id, let the added ones arrive through the ordinary recompute, drop the records of the ones that vanished, and append `definition_refreshed` naming what was dropped.
- A run holding earned progress keeps its definition, full stop. Nothing rewrites intent because a newer file happened to exist.

Whether two texts really differ is the caller's judgement, but it should be semantic: a comment-only edit to a template must not rewrite a run's file and journal a refresh that changed no behaviour.

---

## The evaluation loop

Deterministic and idempotent:

1. Load `workflow.yaml` + `state.json`.
2. Recompute derived statuses from the dependency graph (*Readiness derivation*).
3. Select all `ready`, unclaimed steps — there may be several (parallel branches).
4. For each: **claim** (set `running`, write a lease/heartbeat), execute its bound action, capture the result.
5. **Append** the event(s) to `events.jsonl`, then **atomically rewrite** `state.json`.
6. Repeat until there are no `ready` and no `running` steps.

The run is **terminal** when nothing is runnable, decided in this strict precedence (a `stop` failure outranks apparent completion, so a halted run never reads as *done*):

1. any step `failed` under `on_failure: stop` ⇒ workflow **failed**;
2. otherwise any step `running` or `ready` ⇒ **in_progress** (not terminal — work remains or is in flight);
3. otherwise every step `completed`/`skipped` ⇒ workflow **completed**;
4. otherwise (only `blocked` steps and non-`stop` failures remain) ⇒ workflow **blocked** (waiting, resumable).

Because step 2 is a pure recompute, re-entering the loop from any persisted state produces the same next actions — the property that makes resume safe.

---

## Resumability & crash safety

This is the reason the framework exists.

- **State is the resume point; events are the audit.** A restarted agent reads `state.json`, ignores any prior conversation, recomputes, and continues.
- **Atomic writes.** `state.json` is written to a temp file then renamed, so a crash mid-write never leaves a half-written snapshot. Events are appended and flushed.
- **Interrupted steps.** A step left `running` whose lease has expired on resume is treated as *interrupted*: `recompute` returns it to `pending` (and so to `ready`) with a reason naming the timeout it ran out of. Policy: re-run it (executors should be **idempotent**) or mark it `failed`. The lease/heartbeat distinguishes "still running elsewhere" from "died mid-flight".
- **A lease is a crash detector, not a work budget.** The default is short on purpose — 300 s without a report and the claim is presumed dead, so a crashed agent does not hold a pipeline hostage. That default is wrong for anything that legitimately runs for minutes, and being wrong is expensive in the other direction: the engine reaps work that is still happening, the step goes back to `ready`, and the same files are processed twice while the first executor is still running. A long step therefore declares its own `timeout:` (the dictation template gives transcription four hours). There is no heartbeat *renewal* call: the claim is stamped at `advance` and the only way to extend it is to report.
- **A claimed-and-unreported step is a stall, and is named as one.** `next` returns `claimed[]` alongside an empty `steps[]`, `advance` refuses a live claim quoting how long it has been held, and the dataset report's hint says "stalled" with the call that clears it. Left unnamed this reads to a waiting agent as "the app is busy", which is how a session ends its turn and leaves the run claimed forever.
- **Idempotent executors.** Because a step may be re-run after an interruption, actions should be safe to repeat (or record enough checkpoint data to resume). The framework does not assume atomicity of the work itself.
- **Version binding.** `state.json` records the definition `name` + `version` (and optionally a content hash). If the loaded definition differs, the state is **stale**: require an explicit migration or reset rather than silently applying old state to a new graph.

---

## Data flow between steps (optional)

Steps can pass values forward without the engine knowing what they mean:

- A step declares `outputs`; the engine stores the produced references in `state.json` under that step.
- A later step declares `inputs` referencing them, e.g. `${steps.transcribe.outputs.text}` or `${steps.download.outputs.path}`.
- The executor reads its resolved inputs and writes its outputs; the framework only records the references.
- **A `completed` must carry every name the step declares.** A missing output is not a partially-recorded step, it is a dependent that can never be scheduled: its `when:` guard or `inputs:` reference resolves against a name that never arrived and sits `blocked`, with nothing in `events.jsonl` explaining why. So `record` refuses such a completion and writes nothing — the step stays `running`, and the corrected call lands on it with its attempt intact. Values may arrive under `detail.outputs` or straight on `detail`; a value that genuinely does not exist is recorded as JSON `null` — the typed *no-value* marker, distinct from an absent key and from the string `"null"`. A `null` satisfies the presence contract (the name is carried) and resolves a dependent's `when:` guard to **false**; runs written before this marker that used the strings `"unknown"`/`"false"`/`"no"` are still read as no-value, so existing state is not stranded.

If a workflow prefers, steps can instead communicate through an external store (filesystem, DB) and let the framework track **status only** — data flow is an opt-in convenience, never a requirement.

---

## Failure, retry & propagation

- **Retry** — `retry: { attempts, backoff }`: `failed → ready` until attempts are exhausted, then terminal `failed`.
- **Timeout** — a step's `timeout:` is its lease: an unreported `running` claim older than it is reaped back towards `ready` (see *Resumability & crash safety*). It never makes a step `failed` by itself, because nothing in this build measures how long an action took.
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
| `next(run)` | The runnable step(s) with resolved action + inputs, each with its `lease_secs`, plus `claimed[]` — the steps held by an unreported claim, which is often why nothing is runnable |
| `advance(run, step)` | Execute the bound action, record the result, re-evaluate |
| `record(run, step, event, detail)` | Report an outcome (esp. when the agent itself did the work) |
| `verify(run, apply?)` | Score the run against measured evidence; with `apply` it may only demote (see *Verification*) |
| `adopt(run, evidence)` | Complete the steps whose `adopt` ids this probe returns measured-and-clean (see *Verification*) |
| `history_is_inherited_only(run)` / `rebind_definition(run, yaml)` | The guard and the act behind *Refreshing a run's definition* — only a run whose whole history was inherited may be pointed at newer intent |
| `intervene(run, step, op)` | `retry` / `unblock` / `skip` / `reset` for manual recovery |

Hints are actionable, e.g. *"step `validate` is blocked because dependency `normalize_text` failed; retry `normalize_text` or skip `validate`."* An `advance` on a step that is not `ready` names what it waits on rather than sending the caller off to `status`, and an `advance` or `next` on a live claim quotes the step's lease (`lease_secs`) and how long it has been held.

**`next` and `advance` hand back an executable call, not a riddle.** `params` are copied verbatim by the engine, which knows steps and dependencies but never what a step does — so the *binding* layer resolves the `${run.*}` placeholders that name the run's own subject and folds the resolved value back into the response root, so an agent is never handed an action with no argument to run it on. What `${run.dataset_uuid}` concretely means for the dictation pipeline — read back out of that dataset's `info.json`, and the workspace-job case where there is no subject so the placeholder stays as written rather than resolving to something invented — is documented in [`dictation.md`](./dictation.md). A dataset-less job has no subject, so nothing is resolved.

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

Event vocabulary (extensible): `claimed`, `started`, `completed`, `failed`, `retried`, `blocked`, `unblocked`, `skipped`, `resumed`, `invalidated`, `adopted`, `definition_refreshed`, plus run-level `workflow_started` / `workflow_completed` / `workflow_failed` / `workflow_blocked`. A record may carry an optional `detail` object (error message, attempt number, output refs, actor/device id) — an `invalidated` event names the checks that contradicted the step and the actor that applied it, or (for a cascade reopening) the dependency in `cascaded_from`; a `blocked`/`unblocked` pair distinguishes an `external` block from a derived one; an `adopted` event names the checks that authorized the promotion; and a `definition_refreshed` event names the new step count and the steps that vanished, so a move in either direction — or a change of the intent itself — is always attributable.

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

Per-step fields are all optional except `status`: `started_at`, `finished_at`, `attempt`, `lease`/`heartbeat`, `outputs`, a short `reason` (for `blocked`/`failed`) that surfaces directly in `status()` hints, and — for the two derived states — `blocked_by` (`dependency` | `external`, deciding whether a recompute may lift the block) and `skip_reason` (`when_false` | `manual` | `propagated`, deciding whether a recompute may reopen the skip).

---

## Implementation sketch

Kept deliberately light — the engine is small and domain-free.

- **A self-contained module** (e.g. `src-tauri/src/workflow.rs`): parse → build DAG → evaluate readiness → run loop → persist. It contains **no** domain knowledge; executors are injected.
- **Storage:** one directory per run holding the three plain-text files, under a configurable location (e.g. the workspace dir). Plain text keeps them diffable, agent-readable and sync-friendly.
- **Parsing:** `serde_yaml` → a typed `Definition`; `serde_json` for state; line-append + flush for events. Atomic state writes via temp-file + rename.
- **Engine core:** topological readiness recompute, an async run loop (tokio), and an executor registry `action_name → Executor`. Single-flight claims + leases give crash-safe concurrency.
- **Pluggable executors** — the only place "what work means" lives. An executor can be: (a) a call to an existing MCP tool / Tauri command, (b) a sidecar script (the `resources/tools` pattern), or (c) *deferred to the agent* — the engine returns the step and the agent reports the result via `record`. The core is identical for all three, so the same framework serves deterministic pipelines and agent-in-the-loop reasoning.
- **Agent surface:** expose the commands above as MCP tools (`workflow_*`) so any agent can drive a run — structured JSON responses and actionable errors, matching the project's agent-friendly conventions.
- **Two layers, one seam.** The engine (`workflow/core/`) stays domain-free: it takes opaque check ids and a `id → count` evidence map. The binding layer (`workflow/`) owns run locations, the check vocabulary and the providers, so verification for a new pipeline is a provider there and no change here.
- **Optional later:** a small UI (list runs, per-step status, retry/unblock/skip buttons). Not required for the framework to work.

### Explicitly out of scope

No domain-specific step types, no dataset assumptions, and no heavy workflow-engine features (cron scheduling, distributed workers, BPMN, visual editors). The goal is a **resumable, agent-drivable DAG runner** whose entire state lives in three text files.
