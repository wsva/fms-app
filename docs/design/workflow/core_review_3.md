# Review round 3: adjustments to your answer plan

## Context and how to use this

This responds to your answer file (`core_answer_from_agent_20261011200000.md`). Most of your plan is accepted. This note records (1) what is accepted as-is, (2) two changes to the plan, and (3) concrete specs for the group-2 work so you can implement without guessing.

Working method is unchanged: check the real code before acting, agree/disagree/modify per item, implement with tests, update `core.md` to match the code, and report per item. If you disagree with anything here, say why and propose your own design. Decision points are marked.

Invariants (do not weaken): events are the intended source of truth; verification only demotes; adoption is opt-in, non-vacuous and gated on satisfied dependencies; every status change costs an event; a probe or adoption that changes nothing writes nothing.

---

## 1. Accepted as in your plan

- **N1:** you are right and the round-2 severity was wrong. Recompute re-derives `ready` (non-terminal) so reopened dependents settle on `pending`. Remaining work is only the `invalidated` event's hard-coded `"to":"ready"` and the doc wording. Fix both, and **add the invariant property test anyway** (see 3.6) so the behavior is locked in.
- **N2 (`status()` finding):** accepted as the real high-severity item. `status()` persisting a reaped lease with no event breaks "status is read-only" and "state is a replay of events".
- **N3, N6, N7:** accepted as you described. For N3, also fix the false `workflow_blocked` event that `record` writes for a run that is actually finished.
- **N4:** deferral accepted (option (a) would wrongly reject `sync_media`; subject-key option over-builds for the current vocabulary). Two cheap additions are required, see 2.4.
- **N5, F13 (schema part), F1, F2, F6, F11:** deferral to group 3 accepted, with the bundling change in section 4.

---

## 2. Doc-only work (group 1), with additions

2.1 **N6, N7:** as you planned.

2.2 **Replay claim: fix every occurrence, not only one.** The claim that state is rebuildable from events appears in at least four places: *Design principles*, *Goal*, the *events.jsonl* section ("the ordered log can rebuild `state.json` from scratch"), and *Verification* ("`state.json` stays exactly a replay of `events.jsonl`"). Search the whole file for `replay`, `rebuild`, `source of truth`, `derived` and make them consistent. Interim wording: `state.json` is authoritative today; the event log is an audit trail; rebuild-by-replay is a planned capability (F1). Mark the downgrade explicitly as **temporary**, with a pointer to F1, so it does not become the design.

2.3 **Single-writer note.** The file calls the format "sync-friendly" and names device ids. Until F2 is decided and done, qualify it: single writer per run, single device; copying a run folder between machines is a hand-off, not concurrent use. Put this in *Concurrency* and where "sync-friendly" is claimed.

2.4 **N4 caveats (required because the fix is deferred):**
- Add one sentence to *Validation* / *Verification*: the vacuity rule is **structural**, not semantic; it checks that a guard of the right class is present, not that the guard refers to the same subject as the `per_subject` check.
- Add a **template-lint test** that every built-in template's outcome step names a `subject_present` id (and that the one deliberate exception, `sync_media`, is listed by name as an exception with its reason). This turns the current convention into a checked rule.

2.5 **N1 wording:** say cascaded dependents are reopened and then re-derived by recompute (landing on `pending` unless their dependencies are still satisfied); stop saying they are reopened "to `ready`".

---

## 3. Code work (group 2): pull part of N2 forward

Your plan defers the `interrupted` event, claim token and interrupt budget to the F1 round. **None of them needs `seq`**, and without them the read-only change just moves the problem. Do them in group 2. Heartbeat renewal can stay in group 3.

### 3.1 `status()` becomes read-only; the reap gets a real home

- `status()` and `next()` never write. They may *report* an expired lease (for example a `lease_expired: true` field and a hint) computed from `now`.
- The reap is persisted only by a **mutating call**. Decision point: which calls. Recommended: `advance`, `record`, and `intervene` reap any expired claim they touch under their normal write path, and an explicit `reap(run)` command exists for an agent that wants to clear stalled claims without doing other work. Document the list.
- Each reap appends an **`interrupted`** event, in the same operation that rewrites state. `detail`: `{ attempt, claim_id, holder, expired_at, timeout_secs }`. Add `interrupted` to the event vocabulary and to the doc's "every status change costs an event" list.
- Time is an explicit input: `recompute(definition, state, now)` already takes `now` per your answer. Keep that, and word the doc as "pure given `now`".

**Acceptance:** `status`/`next` leave `state.json` and `events.jsonl` byte-identical (test with an expired lease); a mutating call reaps and appends exactly one `interrupted` event per reaped step; a second identical call writes nothing.

### 3.2 Claim token

- Every claim gets a unique `claim_id` (a counter per step or a random id; recommended: attempt number plus a random suffix), stored on the step record and returned by `next`/`advance`.
- `record` for a step that is `running` must present the `claim_id`. A report with a missing or stale token is rejected with a clear error naming the current claim, and writes nothing.
- Decision point: should a missing token be rejected (strict) or accepted when exactly one live claim exists (lenient, friendlier to existing agent prompts)? Recommended: strict for any step that has been reaped at least once, lenient otherwise, and log which path was taken. If you pick fully strict, update the agent-facing docs and hints so agents know to pass the token.
- Put `claim_id` on `claimed`/`started`/`interrupted`/`completed`/`failed` events, so the group-3 replay can rebuild it.

**Acceptance:** an executor whose lease was reaped and whose step was re-claimed reports `completed` with the old token: rejected, state byte-identical afterwards. A report with the current token succeeds.

### 3.3 Interrupt budget

- A step interrupted repeatedly must not loop forever. Decision point: either count interrupts against `retry.attempts`, or add `max_interrupts` (default small, e.g. 3). Recommended: a separate `max_interrupts`, because an interrupted run is not an error attempt and users may want different limits.
- On exhaustion the step becomes `failed` with a reason naming repeated interruption and the last holder/claim, plus a normal `failed` event.

**Acceptance:** with an injected clock, a step reaped `max_interrupts` times becomes `failed`; a step reaped fewer times stays runnable.

### 3.4 N3: terminal status

- Add the outcome `completed_with_failures` (or equivalent): no `ready`/`running` steps, nothing `blocked` waiting, at least one `failed` step that is not a `stop` failure. Precedence: `stop`-failure → `failed`; any `running`/`ready` → `in_progress`; no remaining work and any `failed` → `completed_with_failures` (list the failed steps in the reason); only `blocked`/`pending` remain → `blocked`; otherwise `completed`.
- Add the matching run-level event and fix `record` so it no longer writes `workflow_blocked` for a finished run. Update hints.
- Resolve the wording mismatch you found: under `stop`, dependents are `blocked` (as readiness derivation says). Make the *Failure* section agree.

**Acceptance:** table-driven test over status combinations and `on_failure` policies; a failed-but-otherwise-done run never reports `blocked` or `completed`.

### 3.5 N1 event field

- The `invalidated` event for a cascaded dependent records the status it actually landed on after recompute (normally `pending`), not a hard-coded `"ready"`. If landing status is only known after recompute, compute it before building the event.

**Acceptance:** cascade test asserts the `to` field equals the resulting status for each affected step.

### 3.6 Invariant property test

Random-DAG property test: after any sequence of `record`/`intervene`/`verify(apply)`/`adopt`/`advance` operations, no step is `ready`, `running` or `completed` while a non-skipped dependency is unsatisfied. This is cheap and protects the property you showed holds today.

### 3.7 UTC timestamps now

You found event times are local and timezone-naive while lease times are UTC. Fix this in group 2, not group 3: local naive timestamps repeat an hour when clocks fall back (Europe: Oct 25), and the log's time order becomes non-monotonic.

- Write all new event and state timestamps as UTC ISO-8601 with a `Z` suffix (`2026-10-11T09:00:00Z`).
- Reading: a timestamp with `Z` is UTC. A legacy timestamp without `Z` is **legacy local**: never reinterpret it, never compare it arithmetically to UTC values; display as-is. Document that old events may not be time-ordered. (A `schema_version` that makes this cleaner arrives with F13 in group 3.)
- Check every place that parses times (lease expiry, "held for N seconds" hints) handles both forms.

**Acceptance:** new events carry `Z`; a mixed log (legacy and new) loads without error; lease arithmetic never mixes the two forms.

---

## 4. Bundling change for group 3

All file-format changes ship together in one version bump so existing runs migrate once:

- F1: `seq` on events, `last_seq` on state, self-sufficient events (outputs, reason, attempt, claim, definition hash), recovery rule, `full_replay`.
- F13: `schema_version` on state and events, compaction design, `actor` on `intervene`, run id format.
- N5: scope the legacy `"false"/"no"/"unknown"` no-value reading to runs below the new `schema_version`.

Group-2 choices must be forward-compatible: put everything replay will need (`claim_id`, attempt, reasons) on the events now, so group 3 does not have to re-version them.

Group 3 also contains heartbeat renewal, F2 (locking) and F6 (unresolved placeholders). See `workflow-review.md` and `workflow-review-2.md` for their full text.

---

## 5. Open question that is the user's, not yours

**F2 depends on a product decision:** is concurrent multi-device use of one run a real requirement? Do not assume. Until the user answers, keep the interim single-writer wording from 2.3 and do not build device-merge logic. When you reach group 3, ask the user, or implement the single-writer lock (advisory lockfile plus compare-and-swap on `last_seq`) and say clearly that multi-device is unsupported.

---

## Report format

Reply with a table: item id, verdict (`agreed+done` / `agreed+deferred` / `disagreed` / `already handled`), one-line note, files and tests touched. Flag every place where you chose a different design than proposed (especially the decision points in 3.1, 3.2 and 3.3) and why. If code and doc disagreed anywhere, list those places.