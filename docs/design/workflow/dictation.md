# Dictation Workflow — the pipeline, the page, and scanning status

This is the **domain** half of the workflow docs. [`core.md`](./core.md) specifies the
domain-free engine (definitions, statuses, the three files, the agent surface) and never
mentions audio. This document describes the one pipeline that exists end-to-end — turning a
raw media folder into a practice-ready dictation dataset — plus the Workflow page that renders
it and the read-only **scan** that reconciles a dataset's files against its database.

| Doc | Owns | Changes when |
|---|---|---|
| [`core.md`](./core.md) | Steps, DAG, statuses, `state.json`/`events.jsonl`, the `workflow_*` surface | The engine changes |
| This doc | The dictation template, its steps' artifacts, the audit checks, the page's scan UX | A check is added, a step is renamed, the report shape changes |

**Promotion rule.** Most of this doc is dictation-specific, but *Proposed: bind checks to a
template* and *Proposed: what a scan may do to a status* below contain mechanism that belongs
to the framework. The split is deliberate: mechanism stays here as a proposal until it is
built, then moves up into [`core.md`](./core.md) and leaves behind only the dictation
instantiation. That way `core.md` never documents a field the engine does not honour.

## Where the pieces live

| Concern | Location |
|---|---|
| The template (DAG, single source of truth) | `src-tauri/src/workflow/templates/dictation/dataset_dictation.yaml`, baked in via `include_str!` and served by `workflow_builtin_templates` |
| The scan (read-only checks) | `src-tauri/src/datasets/dictation/audit.rs` → `dataset_audit` command + MCP tool |
| In-app Run buttons per step | `src/lib/workflow/step-ops.ts` (`STEP_OPS`, keyed by step id) |
| Client mirror of the engine's readiness rules | `src/lib/workflow/steps.ts` (parses the YAML, evaluates `when` against probed facts) |
| The page | `src/components/tools/WorkflowPage.tsx` (+ `WorkflowGraph.tsx`) |

The template is not duplicated anywhere: the frontend fetches it at runtime, so editing the
YAML edits the graph, the Run order and the agent's instructions at once.

## The pipeline

`ensure_model` and `sync_media` head two independent branches that the tail joins:

| Step | Action | Waits on | Guard | Artifact it produces |
|---|---|---|---|---|
| `ensure_model` | `model_status` → `model_download` → `model_load` | — | — | a loaded STT model (outside the dataset) |
| `init_dataset` | `dataset_init_dir` | — | — | `info.json` + empty `data.sqlite3`, minted uuid |
| `sync_media` | `dataset_sync_media` | `init_dataset` | — | `listen_media` rows matching `media/` exactly |
| `generate_subtitles` | `dataset_generate_subtitles` | `ensure_model`, `sync_media` | — | `subtitle/*.vtt` **and** the active subtitle + cues (version 1) |
| `generate_waveforms` | `dataset_generate_waveform` | `sync_media` | — | `waveform/*.json` |
| `detect_reference` | `dataset_list_files` | `generate_subtitles` | — | recorded outputs `has_book` / `has_transcript` |
| `align_cues` | `dataset_align_cues` | `detect_reference` | `has_book` | cue text rewritten against `book.txt` |
| `align_cues_transcript` | `dataset_align_cues_transcript` | `detect_reference` | `has_transcript` | cue text rewritten against `transcript/*.txt` |
| `adjust_cue_times` | `dataset_adjust_cue_time` | `generate_subtitles`, `generate_waveforms` | — | a second active subtitle whose `note` records the adjustment |

Two shape decisions are worth keeping in mind when editing it:

- **There is no "generate database" step.** Subtitle generation writes straight into
  `data.sqlite3`, so the cue inventory is a side effect of `generate_subtitles` rather than a
  stage of its own.
- **The optional branches skip, never fail.** `detect_reference` exists purely so its recorded
  outputs can be resolved by `when:`. A guard that reads a value the run actually observed is
  what makes "no `book.txt`" a clean `skipped` instead of a `blocked` on an unresolvable
  reference.

## The page

Four breadcrumb levels, each one a full-width list: **kind → dataset → template → workflow**.
There is no back/forward chrome — the breadcrumb *is* the navigation, and a run view is
reachable only through its dataset, which is the only context in which its run directory makes
sense. Runs are addressed as `dictation-<uuid>` (`runIdFor`), matching the on-disk
`<dataset>/workflows/<run_id>/` layout so a run travels with the dataset when it syncs.

Three rules the page obeys:

- **Never fabricate progress.** Before a run exists the graph is honestly all-`pending`, and
  the client projection derives only what the engine's `recompute` could derive from the DAG
  itself — guard skips and dependency readiness. On-disk facts colour the *Scan* panel and the
  Run buttons' gates; they never paint a node `completed`.
- **Same command, two drivers.** A node's Run button invokes the Tauri command twin of the MCP
  tool an agent would call, then pairs it with `workflow_advance` / `workflow_record` so
  `state.json` stays truthful whichever way the step was performed.
- **One kind is wired.** Book and card templates render their graphs, but the scan and the
  step registry are dictation-only (`isDictation` gates the whole inspector's evidence half).

## Scan status

The Scan button answers one question: *does the folder still agree with its database?* It is
read-only, and it is one IPC call — `dataset_audit(uuid)` returns `info.json`, the six guard
facts, the inventory counts and every finding, so the page never fans out per-artifact queries.

### What it compares

Media are the spine. Each file under `media/` is matched to its rows by
`listen_media.source` (the media-relative path with forward slashes), and its derived
artifacts follow the stem: `media/a/b.mp3` ⇒ `subtitle/a/b.vtt`, `waveform/a/b.json`,
`transcript/a/b.txt`. Cues are read through the versions that actually play
(`version_superseded IS NULL`), per active subtitle.

**Subtitles are judged from the rows, not the file.** A dataset may ship its cues entirely
inside `data.sqlite3` and carry no VTT at all — that is normal and is not a defect. Only the
absence of *both* is reported. This rule also changed an earlier bug: cue-time adjustment is a
property of the rows, so it is now checked for database-only media too, which the file-driven
loop used to skip.

### The checks

Fourteen named ids, though `database_missing` short-circuits the rest: with no `data.sqlite3`
there is nothing to compare, so a report either names that one problem or walks all thirteen.
Severity is the sort key, so the report leads with what blocks the pipeline. A check collapses
to `clean` when it finds nothing, and the panel folds those behind a toggle.

| id | level | what it means | the step that repairs it |
|---|---|---|---|
| `database_missing` | problem | no `data.sqlite3` (the only check reported) | `init_dataset` |
| `media_new` | problem | file under `media/` with no `listen_media` row | `sync_media` |
| `media_gone` | warn | row whose audio is gone | `sync_media` |
| `subtitles_missing` | problem | neither a VTT nor cue rows anywhere | `generate_subtitles` |
| `subtitles_unreadable` | problem | VTT exists but is not parseable WebVTT | `generate_subtitles` |
| `subtitles_not_imported` | problem | VTT exists, its cues never reached the DB | `generate_subtitles` |
| `subtitles_out_of_sync` | warn | cue counts differ, or the file is newer than the import | `generate_subtitles` |
| `waveforms_missing` | problem | no `waveform/<stem>.json` | `generate_waveforms` |
| `waveforms_stale` | warn | audio is newer than the peaks | `generate_waveforms` |
| `cues_unadjusted` | problem | cues still on raw STT timings while a waveform is available | `adjust_cue_times` |
| `cue_sanity` | warn | empty, zero-length, overlapping, out-of-order or past-the-end cues | none — the cue editor |
| `subtitle_versions` | warn | two active versions without an adjustment note, or inactive ones holding cues | none — play or delete |
| `reference_material` | info | whether `book.txt` / `transcript/` exist at all | `detect_reference` |
| `adjust_blocked_no_waveform` | info | has cues, has no waveform, so adjustment will skip it | `generate_waveforms` |

Cost and noise are both bounded on purpose: three bulk queries plus one VTT parse per media,
**exact** counts kept while the item list caps at 100 with a `truncated` flag ("fix these and
re-scan for the rest").

Adjustment has exactly one durable trace — `listen_subtitle.note` containing
`adjusted using waveform` — because `mode: new` inserts a second active copy rather than
mutating the original. Everything downstream trusts that marker and nothing else.

### Deliberately absent

- **No Fix buttons.** A finding names the repairing step in prose and carries `fix_step` for
  agents; the panel offers no action, because a scan is not consent to change data.
- **No status changes.** The scan touches no `state.json`. See the proposals below.
- **No practice coverage.** Dictation progress lives in the app-level DB, a different subject.

## The problem with the design as it stands

Requirement knowledge is currently written down three times, in three languages, with no
mechanical link between them:

| Place | States | Consumed by |
|---|---|---|
| `audit.rs` | `fix_step` on each check ("`cues_unadjusted` belongs to `adjust_cue_times`") | the report, agents |
| `step-ops.ts` | a gate per step (`hasWaveforms ? undefined : "No waveforms. Generate waveforms first."`) | Run buttons |
| `dataset_dictation.yaml` | `depends_on` + `when:` | the engine, the graph |

So "waveforms are a prerequisite of adjustment" appears as a `fix_step` string, a hardcoded
English sentence, and a DAG edge. Adding a check means remembering all three, and a second
pipeline cannot reuse any of it — `fix_step` is dictation vocabulary baked into a report that
should be generic.

## Proposed: bind checks to a template

Move the mapping from the report into the definition, where the pipeline already lives.

```yaml
  - id: adjust_cue_times
    action: dataset_adjust_cue_time
    depends_on: [generate_subtitles, generate_waveforms]
    verify:
      blocks:  [waveforms_missing]                       # do not attempt yet
      proves:  [cues_unadjusted]                         # findings ⇒ my output no longer holds
```

| Field | Meaning |
|---|---|
| `verify.blocks` | check ids whose findings mean "not yet" |
| `verify.proves` | check ids whose findings mean "this step's output is missing, partial or drifted" |

The dictation binding, in full:

| Step | `blocks` | `proves` |
|---|---|---|
| `ensure_model` | — | — (no subject-side evidence; legitimately `unknown`) |
| `init_dataset` | — | `database_missing` |
| `sync_media` | `database_missing` | `media_new`, `media_gone` |
| `generate_subtitles` | `database_missing`, `media_new` | `subtitles_missing`, `subtitles_unreadable`, `subtitles_not_imported`, `subtitles_out_of_sync` |
| `generate_waveforms` | `media_new` | `waveforms_missing`, `waveforms_stale` |
| `detect_reference` | — | — (records facts; nothing to prove) |
| `align_cues` / `align_cues_transcript` | `subtitles_not_imported` | — (see *Not everything is verifiable*) |
| `adjust_cue_times` | `waveforms_missing`, `subtitles_not_imported` | `cues_unadjusted` |

Consequences:

- **Checks stay in Rust, ids stay opaque to the engine.** A check needs SQL and the filesystem,
  so it cannot be YAML; which checks matter to which step is pipeline knowledge, so it should
  be. A domain module registers checks under stable ids; the engine validates the names and
  hands the binding to whoever asks.
- **`fix_step` / `fix_label` leave `AuditCheck`.** The report becomes subject-only — "here is
  what is wrong" — and grouping under steps becomes a function of the definition. Same report,
  any pipeline.
- **Validation rejects a dangling id.** A typo in `verify:` would silently disable a gate, so it
  fails the pre-run check like a bad `depends_on` does. Checks named by no step are report-only
  (`cue_sanity`, `subtitle_versions`), which is an answer, not an oversight.
- **`dataset_audit(uuid, template_id?)`** groups findings per step when a template is given and
  returns the flat report when it is not, so Studio and the agent surface keep working.
- **`workflow_verify(run, [step])`** joins the agent surface, returning
  `verified` / `drifted` / `unknown` plus findings, and changing nothing.
- **Other kinds follow for free.** `book/build_book_library.yaml` and
  `card/setup_card_deck.yaml` get their own check providers under the same protocol; the page's
  `isDictation` gate becomes a dispatch on kind.

### Verdicts are a projection, not state

Probing is cheap and repeatable, so its result is **not persisted** — `state.json` stays
exactly a replay of `events.jsonl`, and a verdict means "as of the last probe" instead of
becoming a fact with no provenance. It renders as a second marker beside the node's status,
never instead of it.

## Proposed: what a scan may do to a status

Everything above can stay read-only forever. If we let it influence progress, the ladder runs
from harmless to consequential, and each rung is a separate decision:

| Rung | Effect | Persists | Consent |
|---|---|---|---|
| **A. Report** *(built)* | findings in the panel, node badges | nothing | none needed |
| **B. Gate** | `blocks` findings rendered as "why this is not ready" | nothing | none needed |
| **C. Feed guards** | a step records probed facts as *outputs*, so `when:` may `skip` | `record` events | automatic — it is the existing `detect_reference` precedent |
| **D. Invalidate** | a `completed` step whose `proves` evidence vanished goes back to `ready` | `invalidated` event | opt-in per run |
| **E. Adopt** | pre-existing evidence accepted as `completed` | `adopted` event | always explicit, one click per step |

Two hard rules keep this honest, and they belong to the framework rather than to dictation:

- **Verification only ever moves a step down.** A clean probe proves an artifact *exists*, not
  that this run made it. `ensure_model` can be perfect while no transcription ran here; a
  shipped dataset is indistinguishable from a processed one.
- **Every status change costs an event.** The scan gets no privileged path into `state.json`,
  so the log stays the audit trail of progress decisions. A probe that changes nothing writes
  nothing.

### Not everything is verifiable, and that is the correct answer

Dictation has three concrete cases where inventing a check would manufacture false confidence:

- `ensure_model` — the artifact is outside the dataset, so there is nothing subject-side to look at;
- `align_cues` vs `align_cues_transcript` — both rewrite the same cues, and afterwards the rows
  are identical. Evidence can say "the cues look aligned", never "aligned by *this* step";
- adjustment — the note marker says *that* it happened, not the mode, the strategy, or which
  waveform it used, so `proves` can only mean "some adjustment exists".

So `proves` lists sufficient evidence only, an unbound step is legitimately `unknown`, and
writing that judgement down once in the definition is what stops every consumer of the report —
the node badge, the readiness hint, the agent's verify tool — re-inferring it and
contradicting each other.

## Build order

1. Check registry keyed by id; drop `fix_step`/`fix_label` from `AuditCheck` *(pure refactor, no behaviour change)*.
2. `verify:` on `Step`, the binding in `dataset_dictation.yaml`, the id-existence rule in `validate.rs`, `dataset_audit(uuid, template_id?)`.
3. Rung A + B in the page: group findings under steps, badge the nodes, replace `step-ops.ts`'s hand-written gate sentences with the binding.
4. Rung C: let `detect_reference` (and optionally a `verify_dataset` step) record audit facts as outputs.
5. Rung D: `invalidated` with the run's opt-in flag.
6. Rung E: the explicit *Adopt the existing state* action, plus a second kind's check provider to prove the mechanism was never dictation-specific.

## Open questions

- Should `verify:` live on steps only, or also as a workflow-level `report_only:` list — making the two unbound checks an assertion in the definition instead of an accident of the code?
- When a run's definition version changes underneath it, verdicts computed from the new binding are not comparable to the old run. Does a stale run show `unknown`, or refuse verification until migrated?
- Cost: the audit walks every media file. Cheap enough interactively today, but a `verify:` gate that re-probes on every `next()` needs either a short TTL or an explicit re-scan gesture. Which?
