# Review round 2 of `core.md` (Persistent Workflow State Machine)

## How to use this document

You are the reader and the implementer. This is the second review of `core.md`, written by a reviewer who has seen only the updated spec, not the code. Round 1 (`workflow-review.md`) produced fixes that are now in the doc. This round has two parts:

- **Part A: new problems (N1 to N7)** that the round-1 changes introduced or exposed. Two of them (N1, N2) are high severity and touch the central promise ("never report a false `completed`, resume safely from files").
- **Part B: round-1 findings that are still open** (F1, F2, F6, F11, F13), restated compactly. Their full original wording is in `workflow-review.md`; the essentials are repeated here so this file is self-contained.

Working method for every item:

1. **Check the real implementation first** (`workflow/core/` is the engine, `workflow/` is the binding layer). The spec may be ahead of or behind the code. If an item is already handled in code, say so. If the code and the doc disagree, tell me which is right and fix the other.
2. **Decide: agree, disagree, or modify.** Each item gives the problem, why it matters, a proposed change and acceptance criteria. Where an item says "decision point", several designs are valid: pick one and write down why. If you disagree, say why and propose your own fix. Do not implement blindly.
3. **If you agree, implement, add the listed tests, and update `core.md`** so doc and code match. Keep `core.md` free of domain concepts (see N6).
4. Report per item: `agreed+done`, `agreed+deferred`, `disagreed (reason)`, or `already handled`.

Suggested order: N1 → N2 → F1 → F2 → N3 → N4 → remaining Part B → N5 to N7. F1 (events and sequence numbers) is a prerequisite for the clean fix of N2 and N5.

Invariants you must not weaken: `events.jsonl` is the source of truth and `state.json` a derived snapshot; verification may only demote; adoption is the only upward move and is opt-in, non-vacuous and gated on satisfied dependencies; every status change costs an event; a probe or adoption that changes nothing writes nothing.

---

# Part A: new problems

## N1 (high). The cascade reopens dependents to `ready`, which contradicts readiness derivation

**Where.** *Transitions* (`completed → ready` ... "cascade") and *Readiness derivation* ("reopened to `ready` in the same atomic pass").

**Problem.** When `transcribe` is demoted, its completed dependents (`normalize_text`, `validate`, `export`) are reopened to `ready`. But their upstream is no longer satisfied, so by the doc's own derivation rule they should be `pending`, not `ready`. A `ready` step is by definition eligible to run, so `next()` could hand out `validate` while `transcribe` is itself `ready`. Recompute will not repair this, because it preserves recorded statuses and the cascade has just written them.

**Why it matters.** It recreates the exact bug the cascade was added to remove (downstream work running on a stale upstream), only less visibly.

**Proposed change.**
- Cascade reopens dependents to `pending`. Only the root of the demotion goes to `ready` (or whatever its own derivation says).
- After the cascade, run the ordinary recompute in the same atomic write so every step lands on its derived status (a dependent whose upstream is still satisfied stays as derived).
- Decision point: a cascaded dependent that is currently `running`. Options: (a) release its lease and reopen it, recording that its inputs went stale; (b) let it finish, then reopen it on completion. Recommended: (a), plus a claim-token check (see N2) so the old executor's late report is rejected. Document your choice.
- Keep `outputs` on every reopened step so `when:` guards still resolve, as for the root.
- Keep `cascaded_from` on each `invalidated` event.

**Acceptance criteria.**
- Property test over random DAGs: after any sequence of operations, "no step is `ready` or `completed` while a non-skipped dependency is unsatisfied" holds.
- Example-graph test: demote `transcribe`; assert `normalize_text`, `validate`, `export` are `pending` (not `ready`), `transcribe` is `ready`, and the independent `generate_waveform` branch is untouched.
- The cascade is one atomic state write with one event per affected step.

## N2 (high). Lease reaping makes recompute impure, unjournaled, unbounded and racy

**Where.** *Resumability & crash safety* (interrupted steps, lease is a crash detector), *Failure, retry & propagation* (Timeout).

**Problem.** Several separate issues share one mechanism:
1. Reaping depends on wall-clock time, so the claim "the recompute is a pure function of the definition + current statuses" is false once leases are in play. It also weakens the doc's own argument for refusing to withhold gated steps inside `next()` (non-reproducibility).
2. A reap changes `running → pending` but no event is named for it. That breaks "every status change costs an event" and breaks replay.
3. A step whose executor crashes every time is reaped forever. Reaping consumes no `retry.attempts`.
4. After a reap, the original executor may still report `completed` and overwrite or confuse the second claim of the same step.
5. "There is no heartbeat renewal call" forces long steps to declare huge timeouts, which in turn delays crash detection.

**Why it matters.** This is the crash-recovery path, the one place correctness matters most.

**Proposed change.**
- Make time an explicit input: `recompute(definition, state, now)`. The pure part (given `now`) stays pure and testable. Call sites obtain `now` once per pass. Document that `status` and `next` are read-only and may *report* an expired lease (as `interrupted`) without writing; the write happens in a mutating call (`advance`, `record`, or an explicit `reap`).
- Add an `interrupted` event, emitted on reap, with `detail { attempt, lease_holder, expired_at, timeout_secs }`. Replay must reproduce the reap.
- Count interrupts against the step's budget: either consume `retry.attempts`, or add `max_interrupts` (default small, e.g. 3). When exhausted the step becomes `failed` with a reason naming repeated interruption. Decision point: pick one and document.
- Add a claim token: every claim gets a unique `claim_id` (or the attempt number) stored on the step and echoed by `next`/`advance`. `record` must present it, and a report with a stale token is rejected with a clear error and writes nothing. This closes the late-report race.
- Add a heartbeat: `record(step, "heartbeat")` (or `renew`) appends a `heartbeat` event and extends `lease.expires_at`. Throttle (no event if called more often than N seconds, or coalesce) so the log does not balloon. Executors for long steps should call it; the default `timeout` can then stay short.
- State plainly that `timeout` is a lease, never a work budget (already stated); keep that.

**Acceptance criteria.**
- Test with an injected clock: expired lease → `interrupted` event, status returns to a runnable state, replay reproduces it.
- Test: a step interrupted `max_interrupts` times becomes `failed`.
- Test: stale-claim `record` is rejected and state is byte-identical afterwards.
- Test: heartbeat extends the lease and a second reap does not occur while heartbeats continue.
- `status`/`next` on an expired lease return the same bytes on disk before and after (read-only).

## N3 (medium). Terminal status can hide a failure

**Where.** *The evaluation loop*, terminal precedence list.

**Problem.** Rule 1 only treats a `failed` step as terminal-failure when it is under `on_failure: stop`. With `on_failure: continue` or `skip_dependents`, a run with one `failed` step and everything else `completed`/`skipped` falls through to rule 4 and is reported as `blocked` ("waiting, resumable"). But nothing is waiting: the run is finished and one step failed.

**Proposed change.** Add a distinct terminal outcome, for example `completed_with_failures` (or report `failed` with a reason). Precedence suggestion: `stop`-failure → `failed`; any `running`/`ready` → `in_progress`; no remaining work and any `failed` → `completed_with_failures` (list the failed steps); only `blocked`/`pending` remain → `blocked`; otherwise `completed`. Also add the matching run-level event (`workflow_completed_with_failures` or equivalent) to the vocabulary. Update the Failure section's wording so it is consistent (it says dependents "stay pending/blocked" under `stop`; readiness derivation says they are `blocked`: pick one).

**Acceptance criteria.** Table-driven test over combinations of statuses and `on_failure` policies; the failed-but-otherwise-done run never reports `blocked` or `completed`.

## N4 (medium). The vacuity guard can be satisfied by an unrelated file

**Where.** *Validation* (non-vacuous `adopt:`), *Verification* (check classes).

**Problem.** The rule now accepts a `file_exists` id as the guard for a `per_subject` check. But a file existing says nothing about whether the *collection* the `per_subject` check walks is non-empty. Example: the database file exists, there are zero media files, a `per_subject` "no media missing X" check reads `0` vacuously, and the `file_exists` guard also reads clean. Adoption then certifies work over nothing. The structural class tags cannot detect this, because the problem is which subject each check refers to.

**Proposed change.** Decision point; pick one:
- (a) Require a `subject_present` id for any `adopt` list containing a `per_subject` id. A `file_exists` id suffices only when the step's whole output is that single file (no `per_subject` ids in the list).
- (b) Let each check declare its **subject key** (an opaque string, e.g. `media`, `database`). The validator then requires that for every `per_subject` id in the list, a `subject_present` or `file_exists` id with the **same subject key** is also present. The engine still sees only opaque strings.

Option (b) is more precise and keeps legitimate cases working. Recommended: (b), unless the binding vocabulary is too small to justify it, in which case (a).

**Acceptance criteria.** Validator tests: rejects `per_subject` guarded only by a `file_exists` of a different subject; accepts the same-subject pairing; rejects `per_subject`-only lists. Add a regression test for the "database present, zero media" case.

## N5 (low). Legacy "no value" strings are read as no-value forever

**Where.** *Data flow* (typed `null` marker).

**Problem.** The `null` marker is good, but `"unknown"`, `"false"` and `"no"` are still read as no-value for all runs. A genuine string output with one of those values is misread, silently making a dependent's `when:` false.

**Proposed change.** Scope the legacy reading: apply it only to runs whose state was written before the marker existed (needs the `schema_version` field from F13; until that exists, add it now). New runs treat those strings as ordinary strings. Provide a one-time migration (on load of an old run, rewrite legacy strings to `null` and journal it) or keep read-time compatibility keyed on `schema_version`.

**Acceptance criteria.** Test: new-schema run with output `"false"` is a real string and `when:` sees it as a non-null value (decide and document how `when:` treats the string `"false"`: recommended, as an ordinary non-empty string). Old-schema run with `"false"` is still read as no-value.

## N6 (low). Domain names are still in `core.md`

**Where.** The `params` row (`${run.dataset_uuid}`); the line about `sync_media` / `database_missing`; "the dictation template gives transcription four hours"; "the dataset report's hint"; `src-tauri/src/workflow.rs`; mentions of Tauri commands as executors; the link to `dictation.md` in the layer note and in the Verification section.

**Problem.** The file declares it must never name a domain concept, yet it does, in several places. Layer-purity rules rot once they have exceptions.

**Proposed change.** Replace each with generic wording and keep the concrete names in `dictation.md`. E.g. `params`: "the binding layer resolves `${run.*}` placeholders". Replace `sync_media`-style examples with the document's own illustrative ids (`waveform_missing`, `subject_absent`). Say "a long-running step declares a longer `timeout`" without naming the pipeline. Keep illustrative examples clearly labelled as illustrative (the `process_dataset` example is fine because it is declared so). Cross-links to sibling docs are fine only in the layer note at the top.

**Acceptance criteria.** A grep of `core.md` for the domain vocabulary list (dictation, dataset_uuid, info.json, subtitles, sync_media, database_missing, Tauri, src-tauri) returns nothing outside clearly marked illustrative blocks. Consider adding that grep as a CI check.

## N7 (low). Minor inconsistencies and duplication

- *Failure* says that under `stop` the dependents "stay `pending`/`blocked`"; *Readiness derivation* says a failed/blocked dependency makes the dependent `blocked`. Make them agree.
- The evaluation loop still says "Because step 2 is a pure recompute"; with N2's time input, reword to "pure given `now`".
- "Verdict examples" item 6 still lists only two withhold reasons (`not measured`, `the evidence disagrees`) while the rules list four. Update it.
- The "plain terms" subsection restates the dense rules and will drift. Mark which one is authoritative (recommended: the dense rules; the plain section is explanatory and non-normative), or generate one from the other.
- Add one line describing the expected caller loop for `blocks` gating: `verify`, check `verification.gated`, then `next`. The advice-only design is accepted (it preserves reproducibility), but an agent that never calls `verify` never sees a gate, so the spec must say who is responsible.

---

# Part B: round-1 findings still open

## F1 (P0). Event log and state snapshot can diverge; "rebuild from events" is not actually possible

**Problem.** The doc says `state.json` can be rebuilt from `events.jsonl`, but events do not carry enough to rebuild `outputs`, `reason`, `attempt`, `lease` or `started_at`; `definition_refreshed` does not store the new definition or its hash; and the write order (append event, then rewrite state) can leave state behind the log after a crash with no way to detect it, because events have no sequence number and state records no last-applied event. A crash mid-append can leave a torn last line.

**Proposed change.**
- Add a gap-free, monotonically increasing `seq` (starts at 1) to every event, and `last_seq` to `state.json`.
- Make events self-sufficient for replay: `outputs` on `completed`; `reason` and `attempt` on `failed`/`blocked`/`interrupted`; lease info on `claimed`/`started`/`heartbeat`; the definition hash on `workflow_started` and `definition_refreshed` (and either store the definition text or keep content-addressed copies such as `definitions/<hash>.yaml` so replay can resolve it).
- Recovery rule on every load: `events.last_seq == state.last_seq` → normal; `events.last_seq > state.last_seq` → replay the tail (application must be idempotent) and rewrite state; `events.last_seq < state.last_seq` → corruption, refuse to run with a clear error; a torn trailing line is ignored for replay and truncated or quarantined on the next write, never silently parsed around.
- Write order stays (append and flush the event, then atomic state rewrite); the recovery rule is what makes it safe.
- Provide `full_replay(run)` and use it in tests.

**Acceptance criteria.**
- Property test: after random operation sequences, `replay(events)` equals `state.json` (ignoring derived-only fields).
- Fault injection: kill the process between the event append and the state rewrite; reload converges with no lost or duplicated transition.
- Torn-trailing-line test; `last_seq > events.last_seq` rejected with an actionable error.

## F2 (P0). Lost updates from concurrent writers

**Problem.** Temp-file-plus-rename gives crash safety, not mutual exclusion. Two writers (engine plus an agent's `record`, or two devices) that each read, modify and rename silently overwrite each other. The doc calls the format "sync-friendly" and mentions device ids but defines no concurrency control. The *Concurrency* section is unchanged, and the new lease text covers only crash detection.

**Proposed change.**
- Per-run exclusive lock (advisory lock on a lockfile in the run directory) around every mutating operation, plus compare-and-swap on `last_seq`: the caller states the `seq` it based its decision on, and a stale write is rejected with a `conflict` error carrying the current `seq` and a re-read hint.
- Read-only operations (`status`, `next`, `verify` without `apply`) take no lock and write nothing.
- Add per-step `on_interrupt: rerun | fail` (default `rerun`) if you do not adopt the interrupt budget from N2; otherwise fold it into that decision.
- Decision point: if multi-device sync is a real requirement, a lockfile does not work over a sync folder. Either state clearly that concurrent multi-device writers are unsupported (single writer per run, hand-off by copying the folder), or design for it (per-device event files merged by `seq` or a hybrid logical clock). Pick one and document it in the Concurrency section and in the "sync-friendly" claim at the top.

**Acceptance criteria.** Two writers racing on one run: one wins, the other gets a clean `conflict`, never a silent overwrite. A live lease held by another holder blocks a second claim. Lock released on process crash (verify with a killed holder).

## F6 (P1). Unresolved placeholders can be executed literally; two substitution layers share one syntax

**Problem.** A workspace job with no subject leaves `${run.*}` as written, and an agent may run the literal text. Separately the engine resolves `${steps.*}` and the binding layer resolves `${run.*}` with no stated namespaces, order or escape. The round-1 fix moved the explanation to `dictation.md` but added no mechanism.

**Proposed change.** Every `next`/`advance` response carries `unresolved: [...]` (empty when none). `advance` refuses to execute while a **required** placeholder is unresolved (step authors may mark a placeholder optional). Reserve namespaces: `${steps.*}` (engine), `${run.*}` (binding); anything else is a validation error. Resolve `${steps.*}` first, then `${run.*}`. Define an escape (e.g. `$${` for a literal `${`).

**Acceptance criteria.** Unresolved required placeholder blocks execution; escape works; unknown namespace rejected at validation; resolution order tested.

## F11 (P1, partly done). Naming and framing

- `verified` still means only "the artifact exists", which is misleading on a step that never ran. Consider `consistent` or `present`, or prominently document that it is a statement about artifacts, not about the run.
- State explicitly which commands are read-only (`status`, `next`, `verify` without `apply`) and which mutate and take the lock (`verify` with `apply`, `adopt`, `record`, `advance`, `intervene`, `rebind_definition`).
- Terminal precedence is done; fold N3 into it.
- Still undefined: the `when:` expression grammar (small, side-effect-free, over `steps.<id>.outputs.*` and run params only; how it treats `null`, missing keys and the string `"false"`; evaluated on every recompute of that step). Specify it.

## F13 (P2). Format hygiene and operations

- Timestamps have no timezone. Use UTC ISO-8601 (`2026-10-01T10:00:00Z`). Include a read-compat note for old runs.
- Add `schema_version` to `state.json` and to event records, separate from the workflow `version` (also needed by N5).
- Event-log growth: define compaction (snapshot at `seq N`, archive older events, keep `full_replay` working from the snapshot).
- `intervene`: require an `actor`, record it, and define `skip` and `reset` on a `running` step (release the lease and invalidate the claim token, then apply; or refuse).
- Run id format and an archival/cleanup policy.
- Extend the event vocabulary with what these changes introduce: `interrupted`, `heartbeat`, the new run-level outcome from N3, and cascade details.

---

## Consolidated test checklist for this round

1. Cascade lands dependents on `pending`; independent branches untouched; invariant holds on random DAGs (N1).
2. Injected-clock lease expiry → `interrupted` event, replay reproduces it; interrupt budget exhausts to `failed`; stale-claim report rejected; heartbeat extends lease; `status`/`next` never write (N2).
3. Terminal status table including `continue`/`skip_dependents` failures (N3).
4. Vacuity validator: `per_subject` guarded by an unrelated `file_exists` is rejected; same-subject pairing accepted; "database present, zero media" regression (N4).
5. Legacy no-value strings only honoured for old `schema_version` (N5).
6. Grep-based check that `core.md` contains no domain vocabulary outside marked examples (N6).
7. Crash between event append and state rewrite converges; torn trailing line; `replay(events) == state` (F1).
8. Two racing writers → one `conflict`; lock freed after a killed holder (F2).
9. Unresolved required placeholder refuses execution; escaping; namespace validation (F6).

## Report format

When finished, reply with a table: item id, verdict (`agreed+done` / `agreed+deferred` / `disagreed` / `already handled`), a one-line note, and the files and tests touched. Flag every place where you chose a different design than proposed, and say why. If code and doc disagreed anywhere, list those places explicitly.