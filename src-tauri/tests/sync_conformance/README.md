# Sync conformance suite

Data-driven checks for the sync wire contract in [`docs/design/sync-v2.md`](../../../docs/design/sync-v2.md)
(§13). Every invariant the hub journal must uphold lives in one `vectors/*.json`
file; `main.rs` replays each vector against the real journal engine and asserts
the outcome.

## Why it drives the journal, not HTTP

Phase 1 boots no Tauri app and opens no socket. The vectors run through the
`#[doc(hidden)] fms_app_lib::conformance` seam, which wraps
`sync::change_log` — the single authority that produces the `ChangesPage` both
`GET /api/v1/datasets/{uuid}/changes` and `GET /api/v1/app/changes` serialize.
Pinning that output pins the wire shape those routes emit, without the fragility
of standing up an `AppHandle` in a test.

**Phase-2 rule:** the same vectors must pass unchanged against the website
routes. When the website grows its journal (phase 2, §14), the harness gains a
second backend that points `read_page_json` at the cloud host; a vector that
passes on the desktop hub but fails on the website is a parity defect, not a
vector to relax.

## Running

```sh
cargo test --test sync_conformance
```

CI runs this as its own step (see `.github/workflows/build.yml`) so a change to
`src-tauri/src/sync/**` that breaks the contract fails the build.

## Adding a vector

Drop a new `vectors/<name>.json`. The runner dispatches on `check`:

| `check`                 | invariant                                  | fields                                        |
| ----------------------- | ------------------------------------------ | --------------------------------------------- |
| `page_shape`            | §7 journal page shape                      | `scope`, `seed`, `after`, `expect.{keys,entry_keys,resync_required,entry_count}` |
| `resync`                | INV-11 NULL cursor below the mark resyncs  | `scope`, `seed`, `prune_mark`, `after`, `expect.{resync_required,entry_count}` |
| `prune_forces_resync`   | INV-5 a delete is never lost to retention  | `scope`, `seed`, `prune.keep_days`, `after`, `expect.{seed_op,resync_required}` |
| `features`              | INV-20 scopes a host advertises            | `expect.features`                             |

Common fields:

- `scope` — `"dataset"` (default) or `"app"` (the `@app` journal key).
- `seed` — rows appended before the read: `{object_id, kind, edit_time, user_key, payload}`.
- `after` — the follower cursor sent to the read (`-1` is the NULL sentinel).
- `expect` — the assertions for that `check`.

Keep `notes` filled in: it is where the invariant reference and any phase-1 vs
phase-2 naming caveat (e.g. `hub_seq` today, `host_seq` in §7's target shape) is
recorded.

## Scope for phase 1

This suite is scaffolding, not yet a contract. It is desktop-only; there is no
website backend, no cross-host parity check, and no fault drill yet. Those land
in phase 2 when the website serves the journal.
