# Message Bus Design v1 — a NATS coordination plane for derived compute

Working blueprint for adding a **NATS message bus** to FmS as the transport that lets a
user's devices *coordinate* work — chiefly, running derived, re-computable jobs (speech
recognition, sentence splitting, read-aloud assessment) on whichever machine holds the
model, instead of on the phone. Motivated by the observation (November 2026) that the
Android thin client cannot run the pronunciation (`phoneme`) axis at all — it is
`#[cfg(feature = "phoneme")]`-gated desktop — so offloading the scoring to a PC both
**saves phone work** and **upgrades the result** to the full three axes.

**Nothing here is implemented yet.** This is the target shape, written so the decision can
sit and be reconsidered. Where it differs from what ships today, the gap is future work,
not a bug in the current design.

The one idea this document must never blur: **the bus is the *transport*; the hub journal
stays the *authority*.** Every invariant in §5 exists to keep that sentence true. It does
not replace sync, the writeback journal, device pairing, trust zones, or the mailbox — see
§1 non-goals and §7.

---

## 0. The one-sentence model

**NATS is a signaling fabric that dispatches *derived* work across a cluster and carries the
result back; it moves no canonical state, stores nothing durable, and the hub remains the
single authority for the journal.**

Corollaries: content-in / derived-out calls are clusterable for free (transcribe, split,
assess-scoring); anything that durably *changes* user state stays on the existing
journal path ([`sync-v2.md`](./sync-v2.md)); and a NATS subject is never the source of
truth for "who owes what to whom" ([`mailbox.md`](./agent/mailbox.md)).

## 1. Scope and non-goals

**In scope**

- Running derived, idempotent, re-computable jobs on a remote node: STT
  (`model_transcribe` / `model_transcribe_detailed`), sentence splitting (`text_split`),
  and read-aloud three-axis assessment (`assess_attempt`) as the *pilot*.
- Broker lifecycle owned the way the web service already is: run on the hub desktop only,
  reconciled on every role change.
- A **Message Bus** observation page (peers, subjects, live traffic) under Cross Device.
- Fronting the Tasks queue and the agent mailbox with a *wake/notification* signal, so the
  dock badge stops polling. (Fronting, not replacing — see §7.)

**Non-goals (out of scope, explicitly)**

- **Canonical writes over the bus.** Cue edits, XP awards, dictation progress, todo/mailbox
  row *state* never travel as a NATS payload to be applied by peers. That is split-brain,
  and it breaks INV-13 (single hub authority).
- **Replacing the mailbox or Tasks.** They are a durable ledger plus a lifecycle state
  machine; a message bus keeps no state and owns no transitions. §7.
- **A message queue as a file-transfer system.** NATS's own guidance is "store the file,
  pass an ID." Audio bytes ride the existing HTTP file plane; the bus carries the pointer. §6.3.
- **Mesh/peer-to-peer topology, hub election, or failover.** One broker, on the hub. A dead
  hub is brought back, not replaced (same doctrine as [`sync-v2.md`](./sync-v2.md) §15).
- **JetStream as a second job engine** in the first phases. Durable, resumable jobs are
  already the workflow state machine's job; running both invites two lifecycles that
  disagree. §6.4.

## 2. Vocabulary

| Term | Definition |
| --- | --- |
| **Bus** | The NATS server the hub runs, plus the subjects peers publish/subscribe on. Named "Message Bus" to signal *messaging + cooperation*, and deliberately not "Compute" (one use, not the essence) nor "Jobs" (a synonym of the existing **task**). |
| **Peer** | A client connected to the bus (a follower PC, the phone, an agent). Reserve **device** for pairing/identity in Devices & Hub; a *peer* is a bus participant, not an authority. |
| **Subject** | The named channel a message is published to (NATS's own term), e.g. `fms.compute.stt`. Never called a "queue" or "topic" here. |
| **Request** | *This* document's word for one ephemeral derived-compute call and its reply. Chosen so it does not collide with **task** (human unit of work) or **run** (durable workflow execution). One noun per concept, added to the AGENTS.md vocabulary table on landing. |
| **Queue group** | Several peers subscribed to one subject; NATS delivers each message to exactly one member. This is the dispatch/load-balance mechanism — the "scheduler." |
| **Derived work** | Output is re-computable and only ever flows back through the journal: transcribe, split, assess. Clusterable. |
| **Canonical work** | Changes durable user state. Never dispatched over the bus. |
| **Capability (`features[]`)** | Extended list on `/api/v1/cluster`: which compute subjects a peer serves and which models it has installed. A capability not advertised is never attempted (INV-20, already enforced for `app_journal`/`dataset_journal`). |

## 3. Why the current design is not enough

Three verified facts about today's build that motivate this — none of them are defects,
they are simply the ceiling of a *single-authority + thin-client* shape:

1. **The phone's assessment is incomplete by construction.** Pronunciation runs through the
   `phoneme` feature ([`datasets/read_aloud/mod.rs`](../../src-tauri/src/datasets/read_aloud/mod.rs)),
   which is desktop-only, so an Android take reports `model_missing` on that axis. Accuracy
   and fluency work (STT is compiled in via `--features stt`), but fluency requires loading a
   hundreds-of-MB Parakeet model *on the phone* — the exact work the user wants to shed.
2. **No compute is shared.** Each machine does its own heavy passes. The hub already runs an
   axum server, discovery beacon, and pairing, but nothing lets one device *ask another to
   run a model and hand back the result* without going through a full dataset snapshot.
3. **The dock and job surfaces poll.** The mailbox badge is "cheap enough to poll"
   ([`lib/agent/mail.ts`](../../src/lib/agent/mail.ts)); `app_events` is read with an
   `after` cursor; pages re-read on tab focus. Cross-device, that polling has no transport —
   which is the gap a bus signal fills (§7.2).

What already exists and is *reused*, not rebuilt: device identity + trust zones
([`sync/pairing.rs`](../../src-tauri/src/sync/pairing.rs), `zone_guard`), daemon lifecycle
modeled on [`sync/server.rs`](../../src-tauri/src/sync/server.rs) `auto_start`/`apply_role`,
capability negotiation on `/api/v1/cluster`, log piping of a child process's native stderr
([`native_log.rs`](../../src-tauri/src/native_log.rs)), and the resumable-file plane
([`sync/client::pc_fetch_file`](../../src-tauri/src/sync/client/mod.rs),
[`sync/blobpush.rs`](../../src-tauri/src/sync/blobpush.rs)).

## 4. Topology and roles

```
                     hub desktop (runs authority + broker)
   ┌───────────────────────────────────────────────────────────┐
   │  axum /api/v1 (data authority)        nats-server (bus)   │
   │   journal · writeback · file plane     loopback:4222 only │
   └───────▲───────────────────────────────▲──────────────────┘
           │ canonical writes              │ request/reply, queue groups
           │ (unchanged)                   │ (derived work only)
     follower PC / phone ──────────────────┘
        (NATS client, never a broker)
```

- **One broker, on the hub, on loopback only.** Spawning `nats-server` on every device
  yields N disconnected brokers and contradicts single-authority. The phone never runs it
  (a Go binary is not execable in app-private storage under Android's process rules, and a
  phone cannot be a hub anyway).
- **Peers connect as clients.** The natural path is: the hub backend is the *only* local NATS
  client, and it brokers on behalf of paired followers using their Ed25519 identity — so the
  bus never opens a second, unauthenticated LAN surface beside the existing trust zone.
- **The two servers do not overlap.** axum `/api/v1` = authority plane (durable). NATS =
  coordination plane (ephemeral). A request that needs a durable write *lands in the journal*
  through the existing path, not on a subject.
- **Capability bounds dispatch.** A peer advertises which subjects it serves and which models
  are installed; a node without `phoneme` compiled, or without the wav2vec2-phoneme model,
  simply does not answer `fms.compute.read_aloud_assess`.

## 5. Invariants

The contract. Each is testable; a new one belongs in exactly one group.

**Authority**

- **INV-B1 — the bus carries no canonical state.** Any payload that, if applied twice by two
  peers, diverges their durable data is not allowed on a subject. Derived outputs are safe
  (idempotent); journal rows are not.
- **INV-B2 — one broker, role-gated.** `nats-server` starts only when `is_hub()` and stops on
  demotion, exactly like `WebServiceState::apply_role`. A follower serves nothing.
- **INV-B3 — loopback-only broker.** Peers reach the bus through the hub backend, behind the
  existing trust zone; no new unauthenticated inbound port on the LAN/tailnet.

**Capability & dispatch**

- **INV-B4 — never attempt an unadvertised capability.** Dispatch consults `features[]` +
  install state before sending to a peer; a silent failure to route is worse than a refusal.
- **INV-B5 — idempotency key on every request.** A client-generated `(request_id, revision)`
  rides the payload; a re-run after a timeout must not double-award XP or double-write an
  attempt. (Same discipline as the read-aloud `attempt_uuid` and chat's retry uuid.)

**Data plane**

- **INV-B6 — payloads are signals, not blobs.** Max message size (NATS `max_payload`,
  1 MB default) is a *ceiling to respect, not a target*. Files above a small threshold travel
  the resumable HTTP file plane; the subject carries a pointer. A job that would need
  `max_payload` raised to run is a job mis-designed.

**Integration with existing ledgers**

- **INV-B7 — the ledger stays the truth.** A bus notification ("a mail/todo row changed") is a
  hint to re-read SQLite; it never *is* the row. Folder/`owed_by` derivation stays server-side
  so a lane and a header count cannot disagree.
- **INV-B8 — role separation of verbs survives.** The user's take-over/cancel/reply remain
  UI actions, not agent verbs; the bus adds a transport, it does not grant an agent the
  authority to act *as the user*.

## 6. Request lifecycle (the pilot first)

### 6.1 Phase 1 — read-aloud assessment offload (the pilot)

1. Phone records a take → captures WAV + reference (accuracy/fluency are phone-computable,
   but offloading all three is uniform and adds pronunciation).
2. Phone (via the hub backend as NATS client) issues `request` on subject
   `fms.compute.read_aloud_assess`, payload `{request_id, dataset_uuid, text_uuid, audio_ref}`,
   where `audio_ref` is a pointer to the WAV already uploadable through the file plane.
3. Hub (the only peer with `stt` + `phoneme` and the model) request-replies the compact
   `Assessment` (~2 KB, already the stored metrics budget) — well under `max_payload`.
4. The result is written as a `read_attempt_save` event through the existing journal path
   ([`datasets/read_aloud/mod.rs`](../../src-tauri/src/datasets/read_aloud/mod.rs)); the phone
   receives it back on the `@app` changes poll and replays via `apply_remote_save` (LWW). No
   new write plane.
5. XP and best_score behave as today — accuracy `score` is unchanged by the axes.

Prove the shape here (one request type, one reply, one idempotency key) before generalizing.

### 6.2 Generalizing: the derived-artifact requests

Same shape for `fms.compute.stt` (`model_transcribe*`) and `fms.compute.split`
(`text_split`). Note these commands are **already content-in / derived-out with no
`dataset_uuid`** — so those requests need **no dataset sync at all**, just the bytes or text
and a reply. This is the strongest, lowest-risk extension of the bus, and the reason to start
here rather than with anything that touches a dataset.

Caveat worth recording: read-aloud *pronunciation* is not dataset-free — GOP measures against
the text's standard audio clip, which lives in the dataset. The clip must be present on, or
shipped to, the serving node. Accuracy/fluency/transcribe/split have no such dependency.

### 6.3 Audio and the size limit

NATS Core has **no attachment concept** — a message is `subject + headers + opaque bytes`.
The server enforces `max_payload` (default **1 MB**, negotiated to the client at connect). A
60 s 16 kHz mono take is ~1.9 MB, already over the default. `max_payload` is configurable but
raising it fights the tool (NATS holds whole messages in RAM and is explicitly *not* a file
system). Decision: **do not put audio on the bus.** Upload the WAV through the existing
resumable file plane (Range-supported, survives flaky Wi-Fi), pass only a pointer on the
subject. This is also what keeps INV-B6 clean and reuses machinery already built.

### 6.4 Long or batched work

A synchronous request-reply is right for a single take (seconds). Fan-out over a 200-file
dataset is *not* a request — that is where the existing **workflow** state machine's leases,
`claim_id`, heartbeat, and `RetryPolicy`
([`workflow/core/mod.rs`](../../src-tauri/src/workflow/core/mod.rs)) already model a job,
with the notable comment that "this build has no executor." **A bus worker is the missing
executor plugged into that seam**, not a second engine. Do not turn on JetStream and adopt
the workflow engine as "the scheduler" simultaneously — pick one job lifecycle (workflow
durable runs), and keep the bus for the ephemeral request path and the wake signal.

## 7. What the bus must NOT replace (Tasks + mailbox)

This was explored and rejected, so it is written down to prevent re-litigating it:

- **The mailbox is a durable ledger + a transition machine.**
  [`mail/machine.rs`](../../src-tauri/src/mail) answers "may this party perform this move on
  this row?" (T1–T15, supersede preludes, `waiting_user`, promote-on-read) with
  transactional claim/lease (`BEGIN IMMEDIATE`, `claim_id`, same-statement guard). Core NATS
  keeps no state and has no compare-and-set; reproducing "open the dock, see everything
  outstanding" would mean rebuilding a database inside NATS (JetStream + streams + a
  materialized view) and re-implementing the machine on top — then losing the audit trail
  (`mcp_audit`) and the phone reach the `@app` journal gives for free.
- **Tasks is reachable on the phone by design** — the reason it is a tab and not only the
  dock ([`todo.rs`](../../src-tauri/src/todo.rs)). A bus message is not durable and does not
  sync. Replacing it would *remove* mobile reach, the opposite of the pilot's goal.

**What the bus *does* offer these surfaces (§7.2, later phase):** a **wake signal.**
Publish `fms.notify.mail_changed` / `fms.notify.todo_changed` so the dock badge and follower
devices re-read the SQLite ledger on push instead of polling. The row lives in the table; the
subject only says "go look." And a compute *result* is posted back **as** a mail/todo row so
it becomes durable, syncs, and renders in the UI that already exists — the bus dispatches, the
ledger records (INV-B7).

## 8. The Message Bus page

- **Name:** "Message Bus" (sidebar label "Bus"). Rejected alternatives and why: *Compute*
  names one use, not the essence; *Jobs* is a synonym of the existing **task** (violates
  one-noun-per-concept); *Distribution* collides with dataset/content distribution the whole
  app already does; *Chat/Messaging* collides with the human Chat page (this is
  machine-to-machine); *Relay* collides with libp2p relay; *Mesh* implies a topology the
  design forbids.
- **Placement:** Cross Device group ([`Sidebar.tsx`](../../src/components/layout/Sidebar.tsx)),
  beside "Devices & Hub". **Desktop/hub-only** — not in `MOBILE_VISIBLE_TABS`, same gating as
  `pairing_list`.
- **Sections:**
  - **Peers** — who is connected to the bus, and each one's advertised capabilities +
    installed models (from `features[]` + `{model_root}/index.json`).
  - **Subjects & traffic** — live request/reply counts, queue-group membership, in-flight
    requests (with `request_id` + revision, to make INV-B5 observable).
  - It is the *transport you observe*, deliberately distinct from Tasks/Mailbox, which are the
    *durable queues* it feeds.
- **Agent-friendly:** every panel needs an MCP read twin (`bus_peers`, `bus_status`) with
  structured JSON, per the repo's agent-friendly principle. A page a human sees is a page an
  agent can read.

## 9. Packaging the broker (the real cost, stated plainly)

`nats-server` is Go and cannot be embedded in-process — it runs as a **child binary**, and
this repo has twice weighed this and leaned away (audiowaveform removed for Symphonia; the
ACP bridge in [`mcp/bridge.rs`](../../src-tauri/src/mcp/bridge.rs) chose a flag over a
sidecar precisely because `bundle.externalBin` *"needs a per-target-triple artifact in
`src-tauri/binaries/` and a new CI step per platform"*). Budget this explicitly:

- Per-platform artifacts (Windows/Linux/macOS now; Android/iOS never run the broker, only the
  client). CI steps in [`.github/workflows/build.yml`](../../.github/workflows/build.yml).
- Lifecycle modeled on `WebServiceState`: a `NatsState` managed by `auto_start`/`apply_role`
  (role-gated start/stop), a dedicated port (allocated deliberately, the way rmcp runs on its
  own port to dodge the axum version conflict), and the child's stderr piped through
  `native_log.rs` so a GUI release build surfaces a crashed broker instead of dying silently.
- Keep Core NATS only at first; JetStream is a decision deferred (INV/§6.4).

## 10. Phasing

1. **Phase 1 — broker + capability advertisement.** `NatsState` role-gated like
   `WebServiceState`; extend `features[]` on `/api/v1/cluster` with compute capabilities +
   install state. No jobs yet. Reversible, near-zero data risk.
2. **Phase 2 — derived-artifact requests (the pilot).** `fms.compute.stt` / `fms.compute.split`
   request-reply (dataset-free, INV-B6 respected by pointing at the file plane). Prove
   request/reply + queue groups.
3. **Phase 3 — read-aloud assessment offload.** The §6.1 pilot, giving the phone the
   pronunciation axis it cannot compute.
4. **Phase 4 — the wake signal.** `fms.notify.*` so Tasks/mailbox stop polling (fronting, not
   replacing — §7). Message Bus page lands with Phase 2 observation.
5. **Phase 5 — batched fan-out as the workflow executor.** Wire the bus worker into the
   workflow lease/claim seam; the dataset pipeline (`dataset_push_to_hub` already moves derived
   results back) becomes schedulable across peers.

## 11. Open questions (why this is parked)

- **Does the hub-as-sole-local-client broker model survive a hub that is *also* doing heavy
  STT?** Or should a follower PC that's idle run the broker's client loop and take work?
  (Topology tension with "one broker.")
- **JetStream or not, for the durable result?** Today the answer is "write to SQLite + journal"
  (INV-B7), which means JetStream is probably never needed — confirm.
- **Multi-agent contention on a claim.** Queue group gives at-most-once delivery; the
  double-claim guard still has to be a DB transaction. Is that enough, or do we need a lease
  on the bus too?
- **Is a second server on the hub (axum + NATS) worth it for 2–4 personal devices**, versus a
  bespoke REST/MCP call for the pilot and deferring NATS until Phase 5 fan-out? This is the
  main "should we do this at all" question.
- **Android reach of the wake signal.** If the bus is hub-loopback-only and the hub backend
  brokers, how does the phone get `fms.notify.*` pushes without a standing inbound connection
  it cannot hold (Doze)? Possibly the notify phase is desktop-only and the phone keeps polling.

## 12. Related documents

- [`sync-v2.md`](./sync-v2.md) — the journal + authority model this must not disturb
  (INV-13 single authority, INV-20 capability-bounded writes).
- [`agent/mailbox.md`](./agent/mailbox.md) — the ledger + machine the bus fronts but never
  replaces (§7).
- [`workflow/core.md`](./workflow/core.md) — the durable job/lease engine a bus worker plugs
  into as the missing executor (§6.4).
- [`dataset-read-aloud-v2.md`](./dataset-read-aloud-v2.md) /
  [`read-aloud-score.md`](./read-aloud-score.md) — the three-axis assessment the pilot offloads.
- [`agent/integration.md`](./agent/integration.md) / [`agent/control.md`](./agent/control.md) —
  the MCP `app_*` surface and UI-action registry the wake signal would feed.
