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

**Promotion rule.** Most of this doc is dictation-specific, but the verification mechanism
below (*Binding checks to the template*) is framework, and the engine now honours it — so the
specification lives in [`core.md`](./core.md#verification--scoring-progress-against-evidence)
and this file keeps only the dictation instantiation: which check answers to which step, and what
a probe cannot see at all. That is the rule in action: mechanism is written up here as a proposal,
and moves up the moment it is built — never before, so `core.md` never documents a field the
engine does not honour.

## Where the pieces live

| Concern | Location |
|---|---|
| The template (DAG + `verify:`/`adopt:` binding, single source of truth) | `src-tauri/src/workflow/templates/dictation/dataset_dictation.yaml`, baked in via `include_str!` and served by `workflow_builtin_templates` |
| The scan (read-only checks, each under a stable id) | `src-tauri/src/datasets/dictation/audit.rs` → `dataset_audit` command + MCP tool |
| The check provider + vocabulary that binds the scan to a run | `src-tauri/src/workflow/verify.rs` (the only module naming both checks and steps) |
| The join itself (verdicts, invalidation) | `src-tauri/src/workflow/core/mod.rs::verify` → `workflow_verify` command + MCP tool |
| Adoption (clean evidence ⇒ `completed`, the one upward move) | `src-tauri/src/workflow/core/mod.rs::adopt` → `workflow_adopt` command + MCP tool |
| Refreshing a run's definition, while and only while its whole history was inherited | `src-tauri/src/workflow/core/mod.rs::{history_is_inherited_only, rebind_definition}`, called from `verify.rs::adopt` |
| Rendering the verdicts | `src/lib/workflow/verify.ts` (labels and sentences only — no ids) |
| In-app Run buttons per step | `src/lib/workflow/step-ops.ts` (`STEP_OPS`, keyed by step id) |
| Client mirror of the engine's readiness rules | `src/lib/workflow/steps.ts` (parses the YAML, evaluates `when` against probed facts) |
| The page | `src/components/tools/WorkflowPage.tsx` (+ `WorkflowGraph.tsx`) |

The template is not duplicated anywhere: the frontend fetches it at runtime, so editing the
YAML edits the graph, the Run order, the evidence binding and the agent's instructions at once.

**Why the template's `version` was not bumped when `verify:` was added.** `load_bundle` refuses a
run whose `state.json` name/version differs from its stored `workflow.yaml`. Bumping to 3 would
have made every existing `dictation-<uuid>` run unloadable — a schema addition that is purely
additive should cost nobody their history. Old runs keep working; their stored YAML simply has no
bindings, so every step verifies as `unknown` until the run is recreated.

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
  Run buttons' gates; a verdict joins them as a **second marker beside a node's status**, never
  instead of it, and no amount of clean evidence can paint a node `completed`.
- **Same command, two drivers.** A node's Run button invokes the Tauri command twin of the MCP
  tool an agent would call, then pairs it with `workflow_advance` / `workflow_record` so
  `state.json` stays truthful whichever way the step was performed.
- **One kind is wired.** Book and card templates render their graphs, but the scan and the
  step registry are dictation-only (`isDictation` gates the whole inspector's evidence half).

## Scan status

The Scan button answers one question: *does the folder still agree with its database?* It is
read-only, and it is one IPC call — `dataset_audit(uuid)` returns `info.json`, the six guard
facts, the inventory counts and every finding, so the page never fans out per-artifact queries.
The report is **subject-only**: it says what is wrong, never which step is responsible. Reading
it against a pipeline is a second call on top of the same probe — `workflow_verify(dataset_uuid,
run_id)`, which the page fires right after the audit whenever a run is on disk.

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

Fifteen named ids, though `database_missing` short-circuits the rest: with no `data.sqlite3`
there is nothing to compare, so a report either names that one problem or walks all fifteen —
`database_missing` among them, collapsed to `clean`, because reaching the walk *is* the
measurement that the file exists. Severity is the sort key, so the report leads with what blocks
the pipeline. A check collapses to `clean` when it finds nothing, and the panel folds those behind
a toggle.

| id | level | what it means |
|---|---|---|
| `database_missing` | problem | no `data.sqlite3` (then the only check reported; otherwise it walks as `clean`) |
| `media_none` | info | nothing under `media/` *and* no `listen_media` row — there is no subject to walk |
| `media_new` | problem | file under `media/` with no `listen_media` row |
| `media_gone` | warn | row whose audio is gone |
| `subtitles_missing` | problem | neither a VTT nor cue rows anywhere |
| `subtitles_unreadable` | problem | VTT exists but is not parseable WebVTT |
| `subtitles_not_imported` | problem | VTT exists, its cues never reached the DB |
| `subtitles_out_of_sync` | warn | cue counts differ, or the file is newer than the import |
| `waveforms_missing` | problem | no `waveform/<stem>.json` |
| `waveforms_stale` | warn | audio is newer than the peaks |
| `cues_unadjusted` | problem | cues still on raw STT timings while a waveform is available |
| `cue_sanity` | warn | empty, zero-length, overlapping, out-of-order or past-the-end cues |
| `subtitle_versions` | warn | two active versions without an adjustment note, or inactive ones holding cues |
| `reference_material` | info | whether `book.txt` / `transcript/` exist at all |
| `adjust_blocked_no_waveform` | info | has cues, has no waveform, so adjustment will skip it |

Which step each id answers to is *not* in this table — that is the binding further down, and it
lives in the template where the engine can enforce it.

One distinction in this list decides adoption later on: `database_missing` and `media_none` are
the two ids that answer a question about **existence** — of the database file, and of a subject
to walk at all. `media_none` is judged from both sides (no file *and* no row), so a folder whose
media were deleted along with their rows is not quietly treated as an empty-but-measured dataset.
Every other id counts offenders over an inventory, so on a dataset with no media it comes back
clean *vacuously* — "no media, therefore no media is missing subtitles". That is fine for a
report and nowhere near enough to certify that a step's work was done, which is why the per-file
ids may only be adopted *together with* `media_none`.

Cost and noise are both bounded on purpose: three bulk queries plus one VTT parse per media,
**exact** counts kept while the item list caps at 100 with a `truncated` flag ("fix these and
re-scan for the rest").

Adjustment has exactly one durable trace — `listen_subtitle.note` containing
`adjusted using waveform` — because `mode: new` inserts a second active copy rather than
mutating the original. Everything downstream trusts that marker and nothing else.

### Deliberately absent

- **No Fix buttons.** A finding carries the advice sentence and nothing else; the panel offers no
  action, because a scan is not consent to change data.
- **No repair is ever triggered by a scan.** `dataset_audit` touches no dataset file. The one
  thing a scan may write is a *status* — adoption (rung E), which a step opts into in the
  template and which cannot rewrite a decision a human already made. Invalidation stays a
  separate, explicit act through `workflow_verify(apply=true)` — an agent or command surface, not
  a button in a report.
- **No practice coverage.** Dictation progress lives in the app-level DB, a different subject.

## The duplication this design removed

Requirement knowledge used to be written down three times, in three languages, with no mechanical
link between them:

| Place | Stated | Consumed by |
|---|---|---|
| `audit.rs` | `fix_step` on each check ("`cues_unadjusted` belongs to `adjust_cue_times`") | the report, agents |
| `step-ops.ts` | a gate per step (`hasWaveforms ? undefined : "No waveforms. Generate waveforms first."`) | Run buttons |
| `dataset_dictation.yaml` | `depends_on` + `when:` | the engine, the graph |

So "waveforms are a prerequisite of adjustment" appeared as a `fix_step` string, a hardcoded
English sentence, and a DAG edge — and `fix_step` was dictation vocabulary baked into a report
that should be generic.

The first copy is gone: `AuditCheck` is now `{ id, label, level, count, items, truncated, advice }`
and the mapping lives only in the template's `verify:` blocks, which the engine validates. Two
copies remain on purpose, for now:

- `dataset_dictation.yaml`'s `depends_on` + `when:` schedule the run; `verify:` judges the world.
  They overlap in spirit and differ in kind — a dependency edge cannot say "the waveform you were
  told about has since gone stale".
- `step-ops.ts`'s gate sentences stay as fallback, because the audit short-circuits: with no
  `data.sqlite3` it reports `database_missing` and nothing else, so `blocks` ids like
  `media_new`/`waveforms_missing` come back *undecidable* rather than clean. Fact gates are then
  the only protection. Shrinking them to the binding is a follow-up for when checks stop
  short-circuiting.

## Binding checks to the template

The mapping moved from the report into the definition, where the pipeline already lives. The
engine-level rules (verdicts as projection, down-only invalidation, the dangling-id check, the
no-certification-without-measurement rule) are specified in
[`core.md`](./core.md#verification--scoring-progress-against-evidence); this section is the
dictation instantiation of them.

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
| `adopt` | check ids that must come back measured *and* clean for the step to be adopted as `completed` without running it |

The dictation binding, in full:

| Step | `blocks` | `proves` | `adopt` |
|---|---|---|---|
| `ensure_model` | — | — (no subject-side evidence; legitimately `unknown`) | — |
| `init_dataset` | — | `database_missing` | `database_missing` |
| `sync_media` | `database_missing` | `media_new`, `media_gone` | `database_missing`, `media_new`, `media_gone` |
| `generate_subtitles` | `database_missing`, `media_new` | `subtitles_missing`, `subtitles_unreadable`, `subtitles_not_imported`, `subtitles_out_of_sync` | `media_none` + all four `proves` ids |
| `generate_waveforms` | `media_new` | `waveforms_missing`, `waveforms_stale` | `media_none`, `waveforms_missing`, `waveforms_stale` |
| `detect_reference` | — | — (records facts; nothing to prove) | — |
| `align_cues` / `align_cues_transcript` | `subtitles_not_imported` | — (see *Not everything is verifiable*) | — |
| `adjust_cue_times` | `waveforms_missing`, `subtitles_not_imported` | `cues_unadjusted` | `media_none`, `cues_unadjusted`, `adjust_blocked_no_waveform` |

Five steps adopt, in two shapes. `init_dataset` and `sync_media` claim an **agreement**, and an
agreement holds when there is nothing to disagree about: `database_missing` is one file's
existence, and `media_new` + `media_gone` clean means the folder and the table match in *both*
directions — so an empty dataset genuinely has nothing to sync, and needs no guard. The three
artifact steps claim an **outcome**, which cannot be inherited from an inventory of zero: each
lists `media_none` beside its per-file ids, and `adjust_cue_times` also lists
`adjust_blocked_no_waveform`, because a media skipped for lack of a waveform was not adjusted and
must veto the claim instead of hiding inside a zero count. Adopting `generate_subtitles` because a
media-less dataset has no missing subtitles would put a green tick over work nobody did — the
exact false confidence the rest of this design exists to prevent.

How it is wired, as built:

- **Checks stay in Rust, ids stay opaque to the engine.** `audit.rs` registers each check under a
  stable id (`check_ids()`); `workflow/verify.rs` is the provider that turns a dataset uuid into
  `id → offender count` and refuses a kind it has no provider for with an actionable message. The
  core takes the vocabulary as a parameter and never names a check.
- **The report is subject-only.** `AuditCheck` lost `fix_step`/`fix_label`; grouping findings
  under steps is a join of the definition and the report, done in `workflow::core::verify`. Same
  report, any pipeline.
- **Validation rejects a dangling id.** A typo in `verify:` would silently disable a gate and a
  typo in `adopt:` would silently disable a promotion (the step never turns green, whatever is on
  disk), so both fail the pre-run check like a bad `depends_on` does. Checks named by no step are report-only
  (`cue_sanity`, `subtitle_versions`, `reference_material`), which is an answer, not an oversight.
  The `audit.rs` test that a completed walk reports every registered id is the other half: an id
  the vocabulary has but the walk omits reads "not measured" and blocks certification *and*
  adoption in silence.
- **`workflow_verify(dataset_uuid, run_id, apply?)`** is the surface: a command twin for the page
  and an MCP tool for agents, both in process of the same core function. Without `apply` it joins
  and reports; with it, it may demote. Its payload is the run's `status` response plus
  `verdicts`, a `verification` summary (`drifted`/`gated`/`unbound`/`undecidable`/`applied`) and a
  `verify_hint` sentence aimed at the agent.
- **`workflow_adopt(dataset_uuid, run_id, yaml_text?)`** is its mirror and the engine's only
  upward act: same evidence map, opposite direction. It completes every step whose `adopt:` ids
  all come back present *and* zero, and withholds the rest with a reason (`not measured`,
  `the evidence disagrees`). It creates the run from the template text when none exists — a scan
  needs a `state.json` to score anyway, so there is one source of truth instead of an optimistic
  TypeScript preview the engine later contradicts — and it never moves a step already
  `completed`, `failed`, `skipped` or `running`. Command twin, MCP tool, `adopted` event.
- **A run that never earned its progress follows the template.** `state.json` and
  `workflow.yaml` are version-bound on purpose, so a template edit would otherwise strand every
  dataset scanned before it — including on the bindings that decide adoption. `workflow_adopt`
  therefore rebinds an existing run to the text it is passed when, and only when, the two differ
  *semantically* (a comment tweak is not a change) and `events.jsonl` holds nothing but inherited
  moves: run opening, adoptions, invalidations, refreshes. Surviving steps keep their statuses,
  added ones arrive through the ordinary recompute, vanished ones lose their record, and the act
  costs a `definition_refreshed` event; `adoption.refreshed` tells the caller it happened. A run
  an agent or a human advanced keeps its own definition — the copy beside its state is the reason
  that is still possible.
- **The subject is environment, not output.** No step declares `inputs: { dataset_uuid: … }`: the
  uuid is handed to every command and tool by its caller, and the run directory lives inside the
  dataset it describes. While it was modeled as an output of `init_dataset`, an already-initialized
  dataset was unrunnable *and* unadoptable — a completion recorded with no outputs left every
  dependent `blocked` on a value that could never arrive.
- **Verdicts need a run** — and a scan now always opens one. `workflow_adopt` creates the run when
  no `state.json` exists yet, so badges, grouping and adoption all score against the same
  engine-derived state the page renders. Before that, a dataset that had never been run through the
  page had no verdicts at all.
- **The rules are tested, not just written down.** `workflow::core::tests` runs the seam against
  real run directories: a demotion costs exactly one `invalidated` event and re-applying costs no
  second one, a clean probe over a `ready` step says `verified` and changes nothing, a step whose
  bound id was never measured cannot certify, and `state.json` never contains a verdict. Adoption
  is held to the same standard in the other direction: it completes a step only over
  measured-and-clean evidence, opens its dependent, withholds with a named reason otherwise while
  writing no second event, is idempotent, and cannot overrule a step a human already skipped.
  The refresh is tested the same way: a run that only adopted may be rebound to a newer
  definition and carries its `completed` over, a run whose step was `advance`d may not, and a
  definition that fails validation leaves the run byte-for-byte unchanged.
  `workflow::verify::tests` checks that every built-in template binds only registered ids — the
  one guard against the binding and the vocabulary drifting apart silently — that a dangling
  `adopt:` id is rejected naming the field, and that only a semantic edit counts as a new
  definition. `audit.rs` itself tests that a completed walk reports every registered id and that
  an empty inventory is a finding rather than a clean bill.
- **Other kinds follow for free.** `book/build_book_library.yaml` and `card/setup_card_deck.yaml`
  need only a provider in `workflow/verify.rs`; the engine, the command and the MCP tool do not
  change. The page's `isDictation` gate becomes a dispatch on kind.

### What the page does with it

One call, three renderings, no re-derivation: the page never joins ids to steps in TypeScript.

- **Node badges** (`WorkflowGraph.tsx`) — a second marker beside the status pill: warning glyph for
  `drifted`, success glyph for `verified`, nothing for `unknown`. Its tooltip spells out the
  binding, the findings and the ids this probe could not judge.
- **Per-step evidence block** (top of the Scan result panel) — drifted steps first, then steps
  whose `blocks` evidence is outstanding, each finding labelled with the audit's own words and
  advice. Clicking a row selects the node.
- **Run buttons' hint** — the inspector prefers a drift sentence, then a `blocks` sentence, and
  falls back to `step-ops.ts`'s fact gate only when the binding has nothing to say.

Verdicts are dropped the moment the engine state changes (`refreshEngine`) or the breadcrumb moves
(`resetRunView`): they were scored against a state that no longer holds, and a stale badge is a
lie with a nice colour.

Adoption rides the same scan (`probeAndSync`): after the audit lands, the page calls
`workflow_adopt`, then refreshes from the engine, so a step whose artifact is already on disk
renders as `completed` — the user sees "nothing to do here" instead of a button for work that is
done. The notice names the steps it adopted and says so when the run was rebound to the current
template, because that rewrote the run's own `workflow.yaml`; a withheld step stays exactly as it
was and writes nothing, and a passed definition that will not validate is logged rather than
applied.

## What a scan may do to a status

The ladder runs from harmless to consequential, and each rung is a separate decision:

| Rung | Effect | Persists | Consent | Status |
|---|---|---|---|---|
| **A. Report** | findings in the panel, node badges | nothing | none needed | **built** |
| **B. Gate** | `blocks` findings rendered as "why this is not ready" | nothing | none needed | **built** |
| **C. Feed guards** | a step records probed facts as *outputs*, so `when:` may `skip` | `record` events | automatic — it is the existing `detect_reference` precedent | not built |
| **D. Invalidate** | a `completed` step whose `proves` evidence contradicts it goes back to `ready` | `invalidated` event | opt-in per *call* (`apply=true`), not per run | **built** |
| **E. Adopt** | pre-existing evidence accepted as `completed` | `adopted` event | opt-in per *step* (`adopt:`), written by the template author | **built** |

Rung D is reachable by an agent or the command twin, never by a button. Rung E *is* reached by the
page's scan, and what makes that acceptable is that the consent moved somewhere sturdier than a
click: a step adopts only over ids the definition listed, ones that fail when the artifact is
absent, so the judgement is written down once, reviewable and validated, rather than improvised
per user. Three hard rules keep it honest — all framework, spelled out in
[`core.md`](./core.md#verification--scoring-progress-against-evidence):

- **A probe only ever moves a step down.** A clean probe proves an artifact *exists*, not
  that this run made it. `ensure_model` can be perfect while no transcription ran here; a
  shipped dataset is indistinguishable from a processed one. Invalidation keeps the step's
  recorded `outputs`, so `when:` guards do not change underneath the demotion.
- **A step moves up only where the definition opted in, and never over a human.** No `adopt:` list
  means no promotion, whatever is on disk; a step already `completed`, `failed`, `skipped` or
  `running` is left alone, because someone decided about that one.
- **Every status change costs an event.** Neither rung gets a privileged path into `state.json`,
  so the log stays the audit trail of progress decisions and still tells progress this run earned
  apart from progress it inherited. A probe that changes nothing writes nothing — no
  re-`recompute`, no atomic rewrite, no timestamp churn.

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

1. ✅ Check registry keyed by id; `fix_step`/`fix_label` dropped from `AuditCheck` *(pure refactor, no behaviour change)*.
2. ✅ `verify:` on `Step`, the binding in `dataset_dictation.yaml`, the id-existence rule in `validate.rs`.
   *(Built as `workflow_verify` rather than a `template_id` argument on `dataset_audit` — the carrier of a
   binding is the run's own stored definition, not a template the caller happens to name.)*
3. ✅ Rung A + B in the page: findings grouped under steps, node badges, evidence-derived gate hints.
   `step-ops.ts`'s sentences *kept* as fallback — see *The duplication this design removed*.
4. ✅ Rung D: `invalidated` on explicit `apply=true` — built out of order, ahead of C, because the join
   and the demotion path are one piece of code. Not built: a run-level opt-in flag, and any UI affordance for it.
5. ○ Rung C: let `detect_reference` (and optionally a `verify_dataset` step) record audit facts as outputs.
6. ✅ Rung E: `adopt:` on `Step`, `workflow_adopt`, and adoption folded into the page's scan. Built as
   an act authorized by the *definition* rather than a per-step button — the ask was a graph that
   shows `completed`, not another thing to click. Not built: a second kind's check provider to prove
   the mechanism was never dictation-specific.

## Open questions

- Should the unbound checks (`cue_sanity`, `subtitle_versions`, …) be asserted as a workflow-level `report_only:` list, instead of being an accident of the code — the one place where "no step owns this" becomes documentation?
- ~~Stale runs~~ answered by construction: an old run's stored YAML has no bindings, so it reports every step `unknown` and nothing is invented for it.
- Cost: one scan walks the dataset **twice** — `dataset_audit` for the report, then `workflow_verify`, whose provider runs the same audit to build its evidence map. Fine interactively at today's sizes, but the right fix is to pass the counts in (or memoize per probe), not to make the audit cheaper. Would a `verify:` gate inside `next()` be worth that restructure?
- Invalidation is shallow by design: `recompute` never demotes a `completed` dependent, so sending `sync_media` back to `ready` leaves its downstream steps `completed`. Correct (their own outputs may still exist), or should a demotion mark dependents `blocked`?
