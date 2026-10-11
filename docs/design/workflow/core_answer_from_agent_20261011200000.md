# Answer to Review round 2 of `core.md`

This is the implementer's response to [`core_review_from_claude_20261011100000.md`](./core_review_from_claude_20261011100000.md).
Every verdict was reached by reading the **real code** first — `workflow/core/engine.rs`, `workflow/core/mod.rs`,
`workflow/core/persist.rs` — and the current `core.md`, not the review prose alone, exactly as the review's
working-method demands. Nothing here has been implemented yet; this file records the agreed evaluation and the
recommended plan of attack.

Round-1 scope recap: the previous fix round closed **F3 / F4 / F5 / F7 / F9 / F10** plus the coupled doc rewrites
**F11 / F12**. **F1, F2, F8, F13** were consciously deferred. Most of Part B below confirms that deferral was the
known debt; Part A adds two genuinely new, real findings (**N2**'s `status()`-reap and **N3**).

## Verdict table

| Item | Sev | Verdict | Core finding (vs. the real code) |
|---|---|---|---|
| **N1** cascade → `ready` | high | **disagree-as-bug / agree-as-doc** | Not a live safety hole: every cascade path runs `recompute` **before** `commit`, and `recompute` re-derives `ready` (it is non-terminal), so a reopened dependent settles on `pending`. `next`/`advance` recompute too. **But** the `invalidated` event hard-codes `"to":"ready"` and `core.md` says "reopened to `ready`" — both misreport the landing state. |
| **N2** lease reaping | high | **agree (mostly open)** | `recompute(…, now)` already takes `now`. But `status()` persists a reaped lease with **no event**; no `interrupted` event exists; no heartbeat **renewal** op (record only takes completed/failed/blocked); reaping consumes no `retry.attempts` (advance never gates on `max_attempts`); no claim token. |
| **N3** terminal hides a failure | med | **agree (real, contained)** | `classify_run`: a `continue`/`skip_dependents` failure with everything else done → `all_done=false` → falls to `any_failed\|\|any_blocked` → reports **`blocked`** (and `record` writes a `workflow_blocked` event) though nothing is waiting. No `completed_with_failures`. |
| **N4** vacuity via unrelated file | med | **modify / defer** | Live templates are already safe — every *outcome* step names `media_none` (`subject_present`). Option (a) would wrongly reject `sync_media`, which deliberately treats an empty sync as complete. (b) (subject-key) is the principled fix but over-builds a 13-id vocabulary. |
| **N5** legacy strings forever | low | **agree + defer** | `eval_when` reads `"false"/"no"/"unknown"` as no-value unconditionally. Scoping needs `schema_version` (F13, does not exist yet). Harmless for the current `has_book`/`has_transcript` semantics. |
| **N6** domain names in `core.md` | low | **agree (fix now, doc-only)** | Present: `${run.dataset_uuid}` in the `params` row; `sync_media`/`database_missing` in Verification; "the dictation template gives transcription four hours"; and `dictation.md` cross-links inside the body rather than the top layer note. |
| **N7** doc drift | low | **agree (fix now, doc-only)** | The verdict-examples list still names only **2** withhold reasons while the rules name **4** (an artifact of F4). Plus the "pure recompute" wording and the pending-vs-blocked mismatch. |
| **F1** events ↔ state diverge | P0 | **agree, was-deferred, now load-bearing** | `core.md` claims "`state.json` can always be rebuilt by replaying the events" — **there is no replay**: `load_bundle` trusts `state.json`; events have no `seq`; `RunState` has no `last_seq`; `status()` writes event-less reaps. The doc currently overpromises. N2 + F2 both *depend* on the `seq` this adds. |
| **F2** lost updates | P0 | **agree, was-deferred** | `write_state_atomic` is temp+rename (crash-safe, **not** mutual exclusion); `append_events` has no lock/CAS. Concurrent writers can clobber. Honest as a documented single-writer limit, not as-is. |
| **F6** literal placeholders | med | **agree, open** | `next`/`advance` return `params` verbatim and never report an `unresolved:[…]`; a dataset-less workspace job leaves `${run.*}` for the agent to run literally. No required/optional marker. |
| **F11** naming / framing | P1 | **partly agree** | Terminal precedence + F12 done. Still open: name the read-only set (and reconcile — `status()` currently writes, `next()` does not); `verified` means "artifact exists", not "this run did it"; specify the `when:` grammar (already exactly `eval_when`, just undocumented). |
| **F13** format hygiene | P2 | **agree, was-deferred** | `now_string()` is **local, tz-naive** while lease `now_ts()` is UTC; no `schema_version`, no compaction; `intervene` records `{op}` with no `actor`. |

## The two that matter, and one disagreement worth naming

**N1 — the reviewer's most confident claim is the one to push back on.** It reasons from the *doc's wording*
("reopened to `ready`") without the code that immediately re-derives it. The engine is correct on safety; only the
event field and the doc lie about the landing state. Fix cost: near-zero. Not a P0.

**N2 — the `status()`-writes-a-reap-with-no-event finding is the real high-severity one**, and it shares a root with
F1: it mutates `state.json` while appending nothing to `events.jsonl`, breaking both "status is read-only" and the
doc's "state.json == replay of events.jsonl" claim. A reap that must not write (or must write an `interrupted`
event) is a genuine correctness gap in the crash path. F1's `seq`/`last_seq` is its prerequisite.

**Doc-vs-code disagreements — fix on whichever side is right:**
- `core.md` replay claim (F1): either implement replay or downgrade it to "state.json is authoritative; replay is a
  future capability." Right now the doc is wrong, not the code.
- N2 / `status()`: pick a lane — document `status` as read-only *and* stop it writing, or accept that it writes *and*
  emit the `interrupted` event. Today it does the worst of both.
- N7 withhold reasons + N6 domain names: the doc is behind the code (F4 added reasons; F12 left names in). Fix the doc.

## Recommendation

Nothing here contradicts the invariants locked in round 1; the open items (F1, F2, F13, N5) are exactly the round-1
deferrals, so the review largely confirms that deferral was the *known* debt while adding two real new ones (N2's
`status`-reap, N3's classification).

Suggested execution order:

1. **Doc-only, safe, immediate** — N6 (scrub domain names from `core.md`), N7 (withhold-reason count + wording),
   the F1/N2 claim correction (stop promising replay + a read-only `status` the code does not honour), and the N1
   event `"to"` field.
2. **Small, contained code fix** — N3 (`completed_with_failures` in `classify_run` + run-level event + hints +
   tests) and the N2 `status()`-reap honesty (make `status` read-only like `next`, deferring full replay to F1).
3. **Larger, next-round** — F1 (`seq` / `last_seq` / `interrupted` / replay) as the foundation, then N2's
   heartbeat + claim-token + interrupt-budget on top of it, then F2 / F6 / F13.

Group 1 removes three outright doc-lies; group 2 is the small correct follow-up; group 3 (F1-first) is where the
real P0 money is and warrants its own round.
