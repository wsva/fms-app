# FmS Sync Design v2 — one journal, four surfaces

This document is the **target architecture** for sync across the three parties a real user has: the **local hub**, the **local followers**, and the **website**. It physically supersedes nothing: [`sync.md`](./sync.md) stays the authority for the LAN cluster mechanics it already specifies (journal shape, writeback kind classification, tombstones, snapshot rules, trust zones, build order for the desktop/follower pair), and [`../app_dev_guide_20261013_063113.md`](../app_dev_guide_20261013_063113.md) stays the record of what the deployed website does *today*. v2 changes what the website is *for*, and adds the rules that make a device talking to both hubs safe.

**Design order matters here.** v2 is written against the ideal contract, not against the endpoints that exist. Where the website falls short of the contract, the gap is website work to be scheduled — not an app-side workaround. The app must not invent a second semantics because a route is missing.

## 0. The one-sentence model

**One journal per cluster, on one authority; the website is a mirror of that journal plus an inbox for edits made off it, and a registry of immutable published versions that no live edit can touch.**

Everything in this document is a corollary of that sentence, and every invariant in §5 exists to keep it true.

## 1. Scope and non-goals

In scope: dataset files, dataset rows, per-user app data (dictation progress, XP), card rows and tags, publishing/sharing, device identity across three parties, the failure modes each of them creates, and the **no-local-hub mode** where the website is the only authority (§16).

Out of scope (unchanged from v8 and [`sync.md` §6](./sync.md)): CRDTs and per-field merge, conflict copies, automatic failover, off-LAN relay, E2EE beyond per-account isolation, multi-cluster topologies, and merging of diverged workspaces.

Also out of scope: making the website a good general-purpose editor. It needs to be good enough to *view, annotate and correct*, not to do everything a desktop node can do — see §15 question 1.

## 2. Vocabulary

| Term | Definition |
| --- | --- |
| **Cluster** | One user's devices plus the authority that owns their state. Identified by `cluster_id`. Belongs to exactly one account on the website. |
| **Authority** (was: hub) | The single node that owns the cluster journal. May be a desktop (`authority_kind = desktop`) or the website (`authority_kind = cloud`). See §4. |
| **Follower** | Any node that edits locally and converges through the journal: a second PC, a phone, or the website workspace when the authority is a desktop. |
| **Journal** | Append-only, per-scope, monotonic `seq` record of row changes. `sync_log` on a desktop authority ([`change_log.rs`](../../src-tauri/src/sync/change_log.rs)), its exact analogue on a cloud authority. |
| **Scope** | The key a journal entry is filed under: a dataset uuid, or `@app` for per-user app data ([`change_log::APP_SCOPE`](../../src-tauri/src/sync/change_log.rs)). |
| **Mirror** | The website's copy of cluster state, kept current from the journal + changed files. Read-only by construction: it is display and re-render material, never the merge input. |
| **Inbox** | Ordered proposals from editors who do not own the journal (the website UI; optionally an offline follower). Only the authority converts a proposal into a journal entry. |
| **Registry** | Immutable published versions of datasets, with ownership, visibility and subscriptions. The only surface other users read. |
| **Provenance** | Per-dataset record of where a local copy came from and what it tracks: `{ upstream_kind, upstream_origin, cursor, published_version }` (§6.1). |
| **content_id** | A dataset's full identity: `(overall_hash, row_hash@seq)` — file set *and* row state. Neither alone is enough. |
| **Capability (`features[]`)** | What a host actually serves, advertised by `/api/v1/cluster`: `registry_reads`, `app_journal`, `dataset_journal`, `chat`. Authority is bounded by this list, never by the host's kind. |
| **Practice client** | A follower in no-local-hub mode (§16): registry-sourced read-only datasets, `@app`-scope writes, no dataset journal. |

## 3. Why the current design is not enough

Three facts, all verified, that v2 must fix:

1. **Absence-based pruning deletes across hubs.** The sync round prunes any dataset the *connected* catalog does not list ([`client.rs#L2237-L2245`](../../src-tauri/src/sync/client.rs)), and pruning is `remove_dir_all` across every type root ([`client.rs#L1733-L1741`](../../src-tauri/src/sync/client.rs)). The website catalog is visibility-filtered by construction, so pointing a machine at the website destroys its LAN-synced copies. Without provenance there is no safe way to ever talk to two places.
2. **One cursor column, two number spaces.** `dataset_sync_state` is `(dataset_uuid, overall_hash, synced_at, bytes, file_count)` plus a `cursor INTEGER` added by migration ([`client.rs#L377-L409`](../../src-tauri/src/sync/client.rs)), and the one snapshot function writes `x-snapshot-seq` into it whatever hub answered ([`client.rs#L782-L845`](../../src-tauri/src/sync/client.rs)). The comment at that site documents the header as "the hub's change-log seq" — which it is on a desktop hub, and is instead the website's catalog counter. The app-wide scope has the same problem one table over: `app_sync_state.cursor_seq`. A cursor read back as `after=` against the other kind of host is a false position at best, a skipped range at worst.
3. **Two conflict engines for the same rows.** Card sync always targets the website origin regardless of the configured hub ([`cards/sync.rs`](../../src-tauri/src/datasets/cards/sync.rs)), while card kinds are also journaled to the desktop hub with a different LWW key (`updated_at` row CAS vs `edit_time` vs live row). The same edit can be resolved twice, differently.

And one contract violation on the website side: publish means *pin*, but the website's `manifest`/`snapshot` are computed from the **live extracted directory** and a card push bumps that same catalog row ([guide §1, §3.3, §3.6](../app_dev_guide_20261013_063113.md)). A subscriber therefore receives the owner's private edits. That breaks the promise behind "share a dataset" and is what the registry plane (§8) exists to fix.

## 4. Topology and roles

```
                          ┌─────────────── Website ───────────────┐
   Local Hub ── push ───► │ Mirror   (cluster state, read-only)   │
   (authority,            │ Inbox    ◄── proposals (site edits)   │
    owns journal)         │ Registry (published versions, pinned) │
      ▲   ▲               └───────────────────────────────────────┘
      │   └──── journal ────► Follower PC / Phone (push+pull)
```

- **A cluster has exactly one authority, and the authority is designated, not emergent.** This generalizes [`sync.md` §3.1](./sync.md): the desktop hub is not the only possible authority, it is the usual one.
- **`authority_kind = desktop`** — the normal home fleet: a PC is always on, the website mirrors and publishes. The website cannot reach into the LAN, so it never pushes; it contributes proposals to the inbox and the hub drains them when it chooses. This is the user's model exactly: *the website workspace behaves like a follower that cannot push*.
- **`authority_kind = cloud`** — the website hosts the journal. It may host a **subset of scopes**: `@app` alone is a complete and useful authority (§16), hosting dataset scopes as well is the full form. This is not a different protocol (§7); it is the same protocol with a different host, which is the reason v2 unifies the two implementations instead of branching on origin.
- **A host's authority is bounded by what it advertises.** `/api/v1/cluster` lists the scopes it serves, and a client neither writes into an unadvertised scope nor pretends the write succeeded (INV-20). The precedent is already in the code: a follower pulls `/app/changes` only when the hub reports `protocol_version >= 3`, and `/app/state` only at `>= 4` ([`client.rs#L1781-L1791`](../../src-tauri/src/sync/client.rs)).
- **No device is an authority by default.** Pairing and the REST server are desktop-gated ([`sync/mod.rs#L18-L22`](../../src-tauri/src/sync/mod.rs)), and mobile builds refuse `role = hub`, so a phone cannot become a fallback journal host. "No local hub" therefore means *no local journal at all*, not "the phone quietly keeps one" — which is exactly why §16 needs a cloud `@app` authority rather than a degraded LAN design.
- **Handover between kinds** is planned, never automatic, and carries an epoch (§5 INV-13): `desktop → cloud` exports the journal + current state and the cloud authority continues the sequence; `cloud → desktop` is the reverse. A node whose epoch is stale is refused. There is still no *failover*: a dead desktop hub is brought back, not replaced (§15 open question 4 keeps handover deliberately expensive).
- **Followers do not subscribe to the website as a second upstream.** One journal, one upstream. A follower may *read the registry* to adopt a shared dataset (§9 flow 5) — an explicit, user-chosen action that leaves provenance pointing at the registry, not at a standing hub.

## 5. Invariants

These are the contract. Each one is testable, and [§13](#13-how-we-know-it-is-right) says how. They are grouped by the property they protect — a new invariant belongs in exactly one group, and if it does not fit any of them the group list is what should grow.

**Journal authority** — who mints `seq`, and when a stale writer is refused.

| # | Invariant | Why it exists |
| --- | --- | --- |
| INV-1 | **Only the authority mints journal `seq`.** No other node, and nothing on the website, assigns a sequence number. | Two sequence writers is two truths. |
| INV-2 | **The journal is append-only and `seq` is never reused.** `AUTOINCREMENT`, retention records `pruned_up_to`, entries are never rewritten in place. | Reused rowids make a stale cursor look current ([`sync.md` §3.2](./sync.md)). |
| INV-3 | **Every change carries a stable `change_id` and is idempotent on it.** Acks may be lost; retries must be no-ops. | Counter deltas applied twice are silent corruption. |
| INV-13 | **Authority handover increments an epoch; a node holding a stale epoch is refused on write.** | Split brain is unrecoverable corruption, not a bug. |

**Convergence** — how two nodes that disagree end up identical.

| # | Invariant | Why it exists |
| --- | --- | --- |
| INV-4 | **A rejected change returns the winner's payload.** The loser applies it with no enqueue and no log. | Otherwise the loser advances its cursor past a row it never got. |
| INV-5 | **Conflicts resolve against durable current state** — the live row's `updated_at` and the `tombstones` table — never against a prunable log entry. | `sync_log` records vanish at retention; the row does not. |
| INV-6 | **All stamps are canonical fixed-width UTC and compared as instants**; a stamp more than a few minutes in the future is clamped to the receiver's `now`. | Text order lies about mixed formats; one wrong clock otherwise wins forever. |
| INV-14 | **A pull never clobbers an unsent local edit** (pending entry with a later `edit_time` is skipped), **and a newer pulled entry drops the pending one.** Dropping is LWW by design; every drop is counted and surfaced on the status surface (§19.2) so silent loss cannot accumulate unnoticed. | Offline work is not undone by convergence — and when it must lose, the user sees it. |
| INV-16 | **Derived totals are recomputed from rows, never accumulated** (`lifetime_xp = SUM(amount)`). | Dedup *is* convergence; a skipped duplicate otherwise leaves the total permanently ahead. |
| INV-23 | **A journaled change carries a *result*, not an *input*.** Replaying it must be a no-op or a first application — never a re-execution whose outcome depends on the receiver's history. | A stored fold (`{card_uuid, quality}`) applied twice silently compounds ([§17.1](#171-the-defect-that-forced-this)). XP already obeys this by logging the absolute total; every append-only kind must. |

**Boundaries** — what crosses which line, and who is allowed to write where.

| # | Invariant | Why it exists |
| --- | --- | --- |
| INV-7 | **Nothing is deleted because of absence from a catalog that is not its own upstream.** Prune is provenance-scoped. | §3 fact 1. |
| INV-8 | **A published version is immutable to every client, and a registry read never resolves to mutable workspace state.** The operator retains a metadata-only takedown primitive (§11.5) that marks a version unavailable without rewriting it. | Share means share — for users, and a sanctioned abuse path for the operator. |
| INV-17 | **Nothing crosses the publish boundary implicitly.** Mirroring and publishing are separate, explicit acts. | §3's contract violation. |
| INV-19 | **One upstream per scope per device.** A device's `@app` writes go to exactly one authority; mirroring from that authority onward is not a second upstream. | Two hosts accepting the same ledger means two sequence spaces over one set of rows; derived totals would *hide* the divergence instead of surfacing it. |
| INV-20 | **A host is written to only for scopes it advertises** in `features[]`. An unadvertised scope is refused with an actionable error, never silently dropped or queued forever. `features[]` is re-read at the start of every round, on any 4xx that names a scope, and on every epoch change (§19.1). | Today three `HubKind::Website` guards short-circuit app-data sync with `Ok(0)`/skip: the queue grows and nothing reports it (§16.2). An origin test degrades silently; a capability test cannot — but only while the cache is fresh. |
| INV-22 | **Withdrawal never deletes a practice copy.** If a publisher unpublishes or narrows visibility, an adopter's local dataset is kept and marked `unpublished`, and its progress keeps pointing at it. | A catalog that *shrinks* is the common case, not an edge case; INV-7 covers the hub switch, this covers the ordinary passage of time. |

**Durability** — what must survive a crash, a reinstall, and a retention prune.

| # | Invariant | Why it exists |
| --- | --- | --- |
| INV-9 | **No runtime state is ever written inside a dataset directory.** Local state lives in the app DB. | `overall_hash` covers every non-DB file ([`sync.md` §3.2](./sync.md)); a stray cache file dirties every follower. |
| INV-10 | **Write order is dataset DB first, journal append second**, and a crash between them is detected by the logical row hash and healed by **at most one** automatic resync per round. | Log-first loses rows; a naive file-hash drift check loops forever. |
| INV-11 | **A snapshot is internally consistent and names its own position**: read `seq` before packing, return it in a header, and have the receiver set its cursor from it. Snapshot content comes from a `VACUUM INTO` / backup-API copy, never the live file. | Torn DB + "did I already have that change?" ([`sync.md` §3.3, §8](./sync.md)). |
| INV-15 | **Any node can rebuild full state from one snapshot plus journal replay** — no capability restricted to the original machine. | Recoverability is the point of the journal. |
| INV-21 | **Every `@app` kind is either state-readable or non-prunable.** A kind the state route enumerates is exportable in full via `after=0` / state read regardless of retention; a kind it does not enumerate must not be pruned on the authority. Today `dictation` and `xp` satisfy the first arm and `card_review` and the read-attempt events violate the second (§17.6 item 2) — that gap is a phase-7a blocker, not an accepted steady state. | A user must be able to leave a cloud authority for a PC, or a desktop authority for a cloud, without losing a year of practice. Journal *pages* are pruned; the state read is what makes exit lossless. |

**Negotiation** — what a host promises before it writes anything.

| # | Invariant | Why it exists |
| --- | --- | --- |
| INV-12 | **Version and capability negotiation precede any mutation.** `GET /api/v1/cluster` first; a host lacking the feature refuses the write rather than half-applying it. | Three devices never upgrade in one moment. |
| INV-18 | **A change is applied by the same code path a UI action uses** (the replayed command function), never by raw SQL, so invariants the command enforces survive sync. | Sync that bypasses the domain layer invents a second, divergent domain. |

## 6. State model

### 6.1 Per-node sync state

Today's `dataset_sync_state` + `app_sync_state` ([`client.rs#L377-L409`](../../src-tauri/src/sync/client.rs)) gain provenance and lose the overload. `overall_hash`/`synced_at`/`bytes`/`file_count` stay as they are; `cursor` keeps one meaning — **a journal `seq` belonging to the named upstream** — and a `registry`-sourced copy tracks `published_version` instead:

```sql
dataset_sync_state(
  dataset_uuid    TEXT PRIMARY KEY,
  overall_hash    TEXT NOT NULL,   -- content_id half 1: the file set we hold
  row_hash        TEXT,            -- content_id half 2: logical row hash at `cursor`
  upstream_kind   TEXT NOT NULL DEFAULT 'desktop_hub',  -- desktop_hub | cloud_hub | registry | local
  upstream_origin TEXT NOT NULL DEFAULT '',            -- host origin, or the registry account
  cursor          INTEGER,         -- journal seq in *upstream's* number space; NULL = needs resync
  published_version INTEGER,      -- registry copies only: the version we adopted
  availability    TEXT NOT NULL DEFAULT 'listed',  -- 'listed' | 'withdrawn' (INV-22). Adoption is atomic — the row is written only after the snapshot lands — so there is no 'adopting' intermediate.
  synced_at TEXT NOT NULL, bytes INTEGER NOT NULL DEFAULT 0, file_count INTEGER NOT NULL DEFAULT 0
);
-- app_sync_state grows the same upstream_kind / upstream_origin pair, so the
-- `@app` scope cannot follow one host's seq space while a dataset follows another's.
```

`upstream_kind` + `upstream_origin` are what make INV-7 checkable: the round prunes only rows whose recorded upstream is the host it just polled.

### 6.2 Authority-side tables (unchanged, restated for the record)

Built and unchanged, restated for the record ([`change_log.rs#L71-L111`](../../src-tauri/src/sync/change_log.rs)):

```sql
sync_log(seq INTEGER PRIMARY KEY AUTOINCREMENT, dataset_uuid TEXT,   -- scope, incl. '@app'
         object_id TEXT, kind TEXT, op TEXT, edit_time TEXT, user_key TEXT, payload TEXT);
sync_prune_marks(dataset_uuid TEXT PK, pruned_up_to INTEGER);         -- retention gap signal
applied_changes(change_id TEXT PK, applied_at TEXT);                  -- replay dedup, pruned by wall time
file_hash_cache(dataset_uuid, path, size, mtime, sha256);             -- manifest cost control
tombstones(object_id TEXT PK, table TEXT, deleted_at TEXT);           -- per dataset DB
writeback_queue(id TEXT PK /* = change_id */, kind TEXT, ...)         -- follower outbound
```

Note where idempotency actually lives: `change_id` is **not** a journal column — it is the queue-row uuid, carried on the wire and deduped against `applied_changes` by [`already_applied` / `mark_applied`](../../src-tauri/src/sync/change_log.rs). Journal scoping goes through [`scope_for`](../../src-tauri/src/sync/change_log.rs) so a change is never logged where no puller looks, and the re-entrancy flag [`is_applying_remote`](../../src-tauri/src/sync/change_log.rs) is the no-enqueue/no-log guard for replayed changes. Kind classification (`state` coalesces, `append_only` and `counter` never do) is [`ChangeClass`](../../src-tauri/src/sync/change_log.rs). Details stay in [`sync.md` §3.2, §3.6](./sync.md).

### 6.3 Website-side storage

Three stores, corresponding to three surfaces, and the names matter because the deployed website currently has two of them conflated:

| Store | Contents | Written by | Read by |
| --- | --- | --- | --- |
| `live/<type>/<uuid>/` | the mirror: current cluster state, extracted | journal push + changed files | the owner (UI rendering, edit baseline) |
| `versions/<type>/<uuid>/v<N>.tar.gz` + `v<N>.manifest.json` | pinned registry artifacts, immutable | `POST /publish` | subscribers and the public |
| `inbox/<type>/<uuid>/` | proposals, each with `change_id`, proposal `seq`, author | site edits, offline follower proposals | the authority, on drain |

`snapshot_seq`-style catalog counters become **per-scope journal cursors**, identical in shape to the desktop's `sync_log`, so one client implementation reads both.

## 7. One protocol, two hosts

The largest simplification in v2: **the desktop hub and the cloud hub serve the same `/api/v1` contract.** Today the app has a LAN route set in [`sync/rest.rs`](../../src-tauri/src/sync/rest.rs) and the website has a hand-rolled approximation of part of it; the client branches on [`hub_kind`](../../src-tauri/src/sync/mod.rs). In v2 `hub_kind` survives only as (a) auth selection and (b) a cached copy of what `/api/v1/cluster` advertised. The route set, the journal shape, the cursor semantics and the conflict rules are identical, which means one conformance suite and one client.

| Call | Purpose | Notes |
| --- | --- | --- |
| `GET /api/v1/cluster` | who am I talking to | `authority_kind`, `cluster_id`, `epoch`, `protocol_version`, `features[]` = the scopes this host serves (INV-12, INV-20) |
| `GET /api/v1/datasets?lite=1&surface=workspace\|registry` | catalog | `workspace` = my cluster's state, `registry` = published + visible to me |
| `GET /api/v1/datasets/{uuid}/manifest` | content identity | `overall_hash`, per-file `files{}`, `row_hash`, `seq` |
| `GET /api/v1/datasets/{uuid}/changes?after=S` | journal page, dataset scope | `{ entries, pruned_up_to, host_seq, resync_required }` |
| `GET /api/v1/app/changes?after=S`, `GET /api/v1/app/state` | journal + state, `@app` scope | per-user progress, XP, reviews and attempts. `after=0` must reach the beginning (INV-21); state is **paged to the end by cursor**, never capped with a `truncated` flag, or a large ledger re-fetches the same page every round. Both routes already exist on a desktop authority ([`sync/mod.rs#L89-L100`](../../src-tauri/src/sync/mod.rs)). |
| `POST /api/v1/changes` | **push journal entries** | authority → mirror (fill the website's copy); follower → authority (writeback). Body tags the origin and carries `change_id`s; dedup on `applied_changes`. |
| `POST /api/v1/inbox` | **propose a change** | editors without the journal. Validated, queued, never applied to `live/` for readers other than the owner. |
| `GET /api/v1/inbox?scope=&after=S` | drain proposals | same page shape as `/changes`, plus author + proposal seq |
| `POST /api/v1/inbox/ack` | report verdicts | `accepted[]`, `rejected[{change_id, winner_seq, winner_payload}]` (INV-4) |
| `GET /api/v1/file?dataset=&path=`, `GET .../snapshot` | bytes | `Range` honoured; `snapshot` is bootstrap/resync only; `data.sqlite3*` never through `/file` |
| `POST /api/v1/publish` | pin a version | `{uuid, version, visibility, tag_uuids}` from the current `content_id`; `visibility ∈ {private, unlisted, public}`; writes `versions/`, never `live/` (INV-8, INV-17) |
| `GET /api/v1/registry/{uuid}/versions`, `GET .../manifest?version=N`, `GET .../snapshot?version=N` | published reads | every registry read takes a version; the *current* version is resolved server-side from the pin, not from the live dir |
| `POST /api/v1/subscriptions` | subscribe / unsubscribe | REST, so the app can do what the market page does today |
| `GET /api/v1/chat/messages?after_id=`, `GET /api/v1/chat/attachment/{uuid}` | chat | hosted by the authority only |

Rules that apply to all of them:

- **Auth by identity, not by origin.** Device-HMAC for a cluster node; account Bearer for a human on the website; an offline follower's proposals are signed by its device key *and* wrapped in the cluster grant, so the authority can attribute them. Never both header sets on one request.
- **Signature covers method, path, query and body hash** (already required by [`sync.md` §3.3](./sync.md)).
- **Every error is typed and actionable**: 401 (attach/refresh a credential), 403 (identified, not allowed — a re-login fixes nothing), 404 (not on this host), 409 (ownership or epoch conflict, with the winner), 413 (limit named with its value), 423/503 (retryable, with backoff). Mapping already in [`hub_http_error`](../../src-tauri/src/sync/client.rs).
- **Content-addressed file storage** in `live/` and `versions/`: a media file already present under its sha256 is never re-transferred, which is what makes mirror refresh cheap and de-duplicates across versions.

## 8. The four surfaces in operation

### 8.1 Journal (convergence)

Exactly as [`sync.md` §3.2–§3.6](./sync.md): one append choke point ([`change_log::append`](../../src-tauri/src/sync/change_log.rs), the `write_and_log()` of [`sync.md` §9 step 2](./sync.md)) reached **after** the dataset DB write, logical row-hash drift check, tombstones kept forever, retention recording `pruned_up_to` ([`prune_log`](../../src-tauri/src/sync/change_log.rs)), snapshot as bootstrap and escape hatch. v2 adds nothing to the journal itself except that its *host* may be the website.

### 8.2 Mirror (cluster state on the website)

The hub pushes journal entries + changed files after each round. Purpose: the website can render and edit the user's real state, show history, and keep an off-site copy. It is **not** a merge input: a node that wants the truth reads the journal, not the mirror. Refresh is incremental by construction (INV-2 means "which entries since my cursor" is always the cheapest complete answer), which is how the design survives 50 MB proxies and multi-GB audiobook sets without a special "small dataset" rule.

### 8.3 Inbox (edits made off the journal)

The only path by which a non-authority edits become cluster state:

1. The website UI writes to `live/` **optimistically** so the page stays responsive, and appends the same change to `inbox/` with a `change_id` and the author identity.
2. The hub drains: `GET /inbox?after=S` → for each proposal, apply through the **normal command path** (INV-18) under the standard conflict rules (INV-5, INV-14), which mints a journal `seq` → `POST /inbox/ack`.
3. The website rolls `live/` back to the authoritative row on rejection (INV-4) and re-renders. Nothing a subscriber can see was ever affected (INV-17).
4. Drain cadence is the hub's: app start, on a timer, on `Sync now`, and on network restore. The drain holds a per-cluster advisory lock; overlapping triggers coalesce onto the running drain rather than starting a second one, so `change_id` dedup (INV-3) is a safety net, not the primary defense.

This subsumes today's card-sync round trip: `POST /api/card/sync/push` is a proposal and `pull` is mirror reading, so **card rows get one conflict engine instead of two** (§3 fact 3). The app's card client keeps its cursor in the same per-upstream slot as everything else, and its target is chosen by the dataset's provenance rather than hard-wired to the website origin.

Optimistic write to `live/` is the one deliberate exception to "the mirror is read-only", and it is bounded: it is display state for the owner only, it is always corrected by the ack, and no registry read can observe it.

### 8.4 Registry (publish / share / adopt)

- **Publish** pins the *current* `content_id` as `v<N>`, records `version → (overall_hash, row_hash, file manifest)`, sets visibility, and is the only thing that makes a dataset visible to others.
- **Immutability**: `versions/` entries are never rewritten. A later edit to the workspace does not propagate; the owner publishes again.
- **Adoption** installs a dataset whose provenance is `{ upstream_kind: 'registry', published_version: N }`. It is now the adopter's dataset: their edits journal in *their* cluster and never flow back. Two clusters can legitimately hold the same uuid with different content; ownership is per-account, and a publish of someone else's uuid is a `409` answered by reminting (`dataset_remint_uuid`), which already exists.
- **Subscribing is a REST call**, so in-app and in-browser behave the same.

## 9. Flows

1. **Phone edits a cue (normal case).** Local write through the command fn → `writeback_queue` → `POST /changes` → hub applies via the same fn, appends journal → phone acked, queue entry dropped. Unchanged from [`sync.md` §1.3, §3.3](./sync.md).
2. **User edits a card on the website.** Optimistic `live/` write + `POST /inbox`. Hub drains on its next round, resolves LWW against the live row, journals, acks. Website re-renders if it lost.
3. **Hub publishes.** `POST /publish` from the current `content_id` → `versions/…/v<N>` + pin. Subscribers' reads resolve to `v<N>`, never to `live/`.
4. **Website shows XP.** Hub pushes `@app` journal entries; the website recomputes `lifetime_xp` from the ledger rows (INV-16). One display path, no accumulation.
5. **New user adopts a shared dataset.** `GET /datasets?surface=registry&lite=1` → `manifest` → `snapshot?version=N` → install with registry provenance → practice offline → edits stay in their own cluster.
6. **Laptop with no hub reachable.** Reads the registry and adopts; edits stay in the local queue and drain when the hub returns. Sending them to the website inbox instead is the optional phase-7 path (§15 q5), and it is proposals only — the laptop never pulls a second journal.
7. **Retention gap.** Follower cursor `< pruned_up_to` → `resync_required` → flush queue, snapshot, set cursor from the header (INV-11), replay forward.
8. **Authority handover desktop → cloud.** Hub stops writes, exports journal + `pruned_up_to` + tombstones + epoch, uploads state; website opens the journal at `epoch+1`, `cluster_id` preserved; every follower re-resolves `/api/v1/cluster` and continues from its cursor because the sequence is continuous. A stale-epoch writer is refused (INV-13).

## 10. Failure modes

| Failure | Design's answer |
| --- | --- |
| Clock skew | INV-6 clamp; ties to the authority |
| Lost ack / retry storm | INV-3 dedup by `change_id` |
| Crash between row write and journal append | INV-10 logical row hash → one resync |
| Torn DB in a snapshot | INV-11 backup-API copy |
| Journal reset / host reinstall | `host_seq < cursor` alarm → resync, never backwards advance |
| Hub switched (desktop ↔ cloud, or LAN ↔ website) | provenance-scoped pruning and cursors (§6.1), so no cross-hub deletion, no false cursor |
| Publisher edits after sharing | impossible to affect subscribers (INV-8) |
| Two users publish the same uuid | `409` + explicit remint, never silent overwrite |
| Website and hub both edited a row while offline | one conflict engine, LWW at the authority, loser told the winner (INV-4) |
| Proposal drains twice | `change_id` dedup on the authority |
| Split-brain after handover | epoch (INV-13) |
| Large media over a small proxy | content-addressed files + `Range`, whole archives only for bootstrap |
| Publisher withdraws a dataset a device is practicing | kept and marked `withdrawn` (§6.1 `availability`); progress stays valid (INV-22) |
| Host lacks the scope a client wants to write | `features[]` says so → refused with an actionable error, not queued forever (INV-20) |
| Phone offline for months in practice mode | `@app` queue drains on reconnect; dedup by `change_id` makes the flood safe (INV-3) |
| Two authorities both accept one device's `@app` writes | refused — one upstream per scope (INV-19) |

## 11. Limits, operations, and lifecycle

### 11.1 Quotas

Quotas are per account (storage, proposals pending, mirror lag), and every limit is reported in `/api/v1/cluster` so a client can size batches instead of discovering caps by failure.

### 11.2 Cloud authority serialization

A cloud authority must serialize per-scope journal appends across instances — a single-instance in-process lock is not enough once the website is the journal host. Use one writer per cluster (sticky routing or a shared lease) or refuse writes with `503 retry-after`. A lease flap mid-append surfaces as `409 epoch conflict`, which forces the client to re-read `/api/v1/cluster` (INV-20) and retry; the app never assumes the lease is stable across a round.

### 11.3 Content-addressed store GC

`live/` blobs are reference-counted against the trees that name them; a blob no `live/` tree references is collectable. `versions/` blobs are permanent by INV-8, except when §11.5 marks a version unavailable, at which point the bytes may be reclaimed after the operator's retention window. GC is a website-side concern only: a client never sees a blob disappear that a manifest it holds still names.

### 11.4 Retention

Website-side retention mirrors the desktop's: journal entries pruned after 30 days with `pruned_up_to` recorded, tombstones forever, versions forever (they are the user's published work). `@app` kinds not enumerated by the state route are the one exception to journal pruning (INV-21) — they are not prunable at all until §17.6 item 2 closes.

### 11.5 Takedown

An operator retains a takedown primitive that marks a published version `unavailable` without rewriting the artifact: subscribers keep their local copies (INV-22), reads on that version return `410 Gone`, and the pin resolves to the next-lower available version or none. This is the only sanctioned mutation of `versions/`, and it is metadata-only — the bytes stay, so an overturned takedown restores the version by flipping the flag back.

### 11.6 Account lifecycle

- **Password rotation** does not invalidate device keys; a device key is revoked only by explicit user action, and a revoked key gets `401 re-pair` at the next round.
- **Account suspension** freezes writes (`403 suspended`, retryable only by support) while reads on public registry entries continue; `@app` queue growth on the client is surfaced on the status surface (§19.2), not silently dropped.
- **Account deletion** unpins every published version (INV-22 protects adopters), revokes every device key, and schedules the mirror + inbox + `versions/` blobs for reclamation after the operator retention window. Journal export via `GET /api/v1/app/state` remains available to the user for the same window (INV-21 exit story).
- **Storage quota exceeded mid-round** returns `413 quota` with current usage in the body; the client stops the round, keeps the queue, and surfaces the state rather than retrying.

## 12. Delta from what exists

| Today | v2 |
| --- | --- |
| `HubKind` branches the route set and the client | one route set; origin selects auth + cached capability only |
| Website `snapshot`/`manifest` serve the live dir | registry reads serve pinned `versions/`; `live/` is the owner's mirror |
| Card sync is a bespoke website round trip, always to the website origin | a surface of the journal: proposals in, mirror out, target by provenance |
| Whole-archive upload is the only hub → website write | journal + changed-file push; upload stays for bootstrap and explicit publish |
| `dataset_sync_state.cursor` overloaded, pruning global | provenance + per-upstream cursor; prune scoped |
| No REST subscribe | `POST /api/v1/subscriptions` |
| `x-snapshot-seq` as an ad hoc counter | per-scope journal cursors, same shape as `sync_log` |
| `HubKind::Website` guards skip the app-data plane entirely | capability check against `features[]`; the three skips become one code path (§16.2) |
| `card_review` / `read_attempt_save` journal under the dataset scope | `@app` scope keyed by `(user_key, object)`: a subscriber's practice history is never a write into someone else's content (§16.3) |
| First write to a downloaded base forks it into a writable overlay | overlay creation is an authority-with-dataset-journal affordance; a practice client is refused instead (§16.3) |
| `card_sync_full` resolves a card dataset with the **read** resolver and writes pulled rows through it | refuse the round trip for a read-only base rather than write through the guard — prospective hygiene, until an adoption flow actually fills the download root (§16.3) |
| `card_review` journals `{card_uuid, quality}` — a fold that is re-executed on replay | journals the event **with its outcome**, `INSERT OR IGNORE` by event uuid: replay idempotent, echo harmless (INV-23, §17.2) |
| `card_review` is one row per card in the **dataset** DB, with no user column; a rating also writes `card.familiarity` and bumps `card.updated_at` | per-user review events in the app DB under `@app`; practice stops mutating shared content, and a published deck no longer ships the publisher's schedule (§17) |

## 13. How we know it is right

- **One conformance suite, two hosts.** The same test vectors run against `sync/rest.rs` (desktop) and the website routes: identical responses for identical journal states. A host that passes is contract-complete; nothing in the app can "work on one and silently half-work on the other". The suite is a phase-2 deliverable, checked into `src-tauri/tests/sync_conformance/`, and CI runs it against both hosts on every sync-touching PR — a host that fails is treated as broken, not as a known difference.
- **Property tests** on the invariants: replay a journal twice → byte-identical state (INV-3); random interleavings of two nodes converging → same `content_id` (INV-1, INV-5); a crash injected between the two writes of INV-10 → exactly one resync, no data loss; stamp fuzzing across formats and offsets (INV-6).
- **Differential check** for the file contract: the manifest hash recipe must stay byte-equal across TypeScript and Rust for the same tree — it already is verified equal ([guide §4.1](../app_dev_guide_20261013_063113.md)), so keep it as a CI assertion, not a one-time experiment.
- **Fault drills**: hub switched mid-life, hub offline a month, cursor past `pruned_up_to`, 413 on a publish, handover and hand-back.
- **Mode drills** (§16.5): no-hub phone end-to-end, publisher withdrawal, eviction then re-download with progress intact, and the migration out to a desktop authority.

## 14. Build order

Each phase is shippable and keeps every existing device usable. Phases 0 and 1 are both app-only and are worth doing whatever else we decide: phase 0 closes the *live* card-review double-apply defect §17.1 documents, and phase 1 removes the false-cursor and cross-hub-prune hazards §3 documents. Phase 0 ships first because a rating today corrupts the author's own schedule on every round; the hazards phase 1 closes only bite once a device talks to two hosts.

0. **Card review plane moves to the user.** Review events into the app DB under `@app`, replay made idempotent, `card` rows no longer written by practice (§17). App-only, no website work — and this is the *live* defect: a rating today double-applies on the LAN and reaches the website as a content edit that subscribers are permitted to push.
1. **App-side safety floor.** Provenance + per-upstream cursors + prune scoping (§6.1); card-sync target chosen by provenance; refuse prune-by-absence against any host that is not a dataset's recorded upstream; and refuse the card round trip for a deck under the download root (§16.3, third item) — prospective rather than live, since nothing populates that root yet, but it is the guard the adoption flow will need the day it exists.
2. **Contract hardening.** `GET /api/v1/cluster` with `authority_kind`, `epoch`, `features[]`; both hosts serve it; the client negotiates before writing (INV-12). The conformance suite (§13) lands here.
3. **Registry split on the website.** `versions/` + per-version reads + `POST /publish` + `POST /subscriptions` (INV-8, INV-17). Fixes §3's contract violation.
4. **Mirror push, incremental.** Journal + changed-file push replaces whole-archive upload for refresh; content-addressed file store; `snapshot` demoted to bootstrap.
5. **Inbox.** Proposal queue, drain, ack-with-winner; fold the card round trip into it and delete the second conflict engine.
6. **App-data display on the website.** `@app` mirror, recomputed totals, history surface for progress as well as XP.
7. **Cloud authority, split in two.**
   - **7a — `@app`-only authority.** The website serves `POST/GET /api/v1/app/changes` + `GET /api/v1/app/state` and advertises `app_journal` in `features[]`; the four origin guards in §16.2 become capability checks. This alone enables the **no-local-hub mode** (§16) — the cheapest full win in this plan, and the one that serves a user with no PC at all. Closing §17.6 item 2 (state read over `card_review_event`) is part of this phase, not deferred past it (INV-21).
   - **7b — dataset scopes hosted too.** Full journal hosting + handover with epoch, so multiple devices can *edit* shared state against the cloud (INV-13, INV-15). Nothing in 7a depends on it.
8. **Optional.** Website editing beyond card types (needs a per-type row write API), follower fallback proposals from a signed device, per-account quotas in the UI.

## 15. Open questions

1. **Does the website edit anything but cards?** The inbox is type-agnostic, but a cue/book/read-aloud edit needs the website to know the row shape and the command's invariants. Cheap for cards (already built), expensive per extra type. Decide the target set for phase 5 rather than designing a general row API now.
2. **Chat on a cloud authority.** The store is already workspace-scoped (`<workspace>/chat`), so it moves with the authority, but attachments need the same content-addressed store. Worth including in phase 7 or explicitly deferring.
3. **Storage model for `versions/`.** Full tar.gz per version is simple and matches today's `_archive`; content-addressed blocks with a per-version manifest is far cheaper in space. v2 needs immutable reads either way; the choice is capacity, not correctness.
4. **How expensive should handover be?** Deliberately a manual, epoch-bumping operation now. If users turn out to want phone-primary sync, the alternative is `authority_kind = cloud` as the *default* for new accounts, which is a product decision, not a protocol one.
5. **Do followers of a desktop authority ever mirror directly to the website?** The design says no (INV-1). The one case that hurts is a laptop that has been offline for months and comes back huge. Answer is still "through the hub", with the hub's queue as the arbiter — but worth confirming before phase 6.
6. **How much history does the website render?** The `@app` mirror makes XP and progress displayable, but "history" is open-ended (ledger vs daily rollups vs per-dataset stats). Decide the exact surface, not the general case.
7. **~~Does a practice client get any local writing at all?~~ Answered** by §17: the second option is taken for reviews — they live as `@app` rows keyed against the shared object and never inside its directory. Still open for the *note-taking* case (`read_text_save`-class kinds): does a practice client get its own notes on a shared text?
8. **Where does a practice-mode market live in the UI?** Download/evict/search is a different shape from the dataset management page the desktop shows. Design it as its own surface rather than hiding controls on the management page, or every gated control becomes an accidental affordance.

## 16. The no-local-hub mode (practice client)

A user with **no PC at all**: the phone is the only device, the website is the authority, datasets come from the registry and are never controlled from the phone, and the app exists to practice.

**This is the cheapest mode in the design, and the reason is structural.** Read-only datasets mean **no dataset-scope journal**: no dataset conflict engine, no file-level incremental transfer, no tombstones over someone else's content. The authority hosts exactly one scope — `@app` — and the machinery for that scope already exists and runs in production between a desktop hub and its followers. What is missing is a host for it on the website and four origin tests that say "the website cannot have this".

### 16.1 What already exists

| Capability | State | Evidence |
| --- | --- | --- |
| Login + Bearer-auth round against the website origin | built | [`hub_request`](../../src-tauri/src/sync/client.rs) website branch; `sync_round_inner`'s `is_website` path |
| Registry-style read + whole-directory download | built | catalog `lite=1` → manifest drift → `dataset_sync_snapshot`. Note what this *requires of a publisher*: the snapshot is the directory as the desktop left it, so `data.sqlite3` and `waveform/` must already be built — a phone cannot make them, since subtitle and waveform generation are desktop-gated ([`mod.rs#L1763-L1765, L2058`](../../src-tauri/src/datasets/mod.rs)) |
| Read-only **by location**, enforced in the backend and at the MCP boundary | built | [`download_root` / `is_under_download_root`](../../src-tauri/src/datasets/mod.rs#L271-L291), refused in [`mcp/datasets.rs#L635-L641`](../../src-tauri/src/mcp/datasets.rs) |
| App-scope journal **client**: push, pull, paged history backfill, idempotent replay | built | [`writeback_flush_inner`](../../src-tauri/src/sync/client.rs#L580-L589), [`pull_app_scope`](../../src-tauri/src/sync/client.rs#L1847-L1858), `backfill_app_history` + `/app/state` paging |
| App-scope journal **desktop host**: `/app/changes` (protocol v3), `/app/state` (v4) | built | [`sync/mod.rs#L89-L100`](../../src-tauri/src/sync/mod.rs) |
| Progress rows live outside the dataset directory, keyed by user + media | built | [`listen_dictation`](../../src-tauri/src/datasets/dictation/mod.rs#L119-L122) in the app DB |
| App-scope journal on the **website** host | **missing** | the four guards below |

So the mode is *three routes on the website, four guard edits here, and three things to settle first* (§16.3) — not a new subsystem.

### 16.2 The four gap sites

All of them are deliberate today, because the deployed website serves no row journal (dev guide §6). They are origin tests, which is why they cannot be lifted until the host advertises the scope:

1. [`writeback_flush_inner`](../../src-tauri/src/sync/client.rs#L580-L589) returns `Ok(0)` for a website origin — progress and XP never leave the phone and the queue grows silently (INV-20).
2. [`pull_app_scope`](../../src-tauri/src/sync/client.rs#L1847-L1858) returns early with `applied: 0` — a reinstall or a second phone recovers nothing.
3. `backfill_app_history` returns early ([`client.rs#L2007-L2010`](../../src-tauri/src/sync/client.rs#L2007-L2010)) — no route to the past.
4. The round's website branch is **file-level only by construction** ([`client.rs#L2175-L2197`](../../src-tauri/src/sync/client.rs#L2175-L2197)); `pull_app_scope` is called in the LAN arm alone ([`client.rs#L2388-L2390`](../../src-tauri/src/sync/client.rs#L2388-L2390)). In a mode with no dataset journal, the app scope is the *only* thing a round should pull.

Website work: `POST /api/v1/app/changes` (per-user keys, dedup by `change_id`, totals derived from rows) and the two reads above, paged by cursor to the end. Auth is the account's Bearer — no pairing, since [`pairing.rs` and `rest.rs`](../../src-tauri/src/sync/mod.rs#L18-L22) are desktop-gated and a phone has no device key to present. Advertise `features[] = [registry_reads, app_journal]` and *not* `dataset_journal`, and the client's four guards become one capability test each.

### 16.3 Three things to settle before it ships

1. **Refuse, do not fork.** [`find_dataset_dir_write`](../../src-tauri/src/datasets/mod.rs#L1589-L1595) forks a downloaded base into a writable overlay on the first write. Under an authority that hosts dataset scopes that is right — the fork becomes journaled state. In this mode an overlay is a dead end: it diverges from the shared base, cannot be published (no dataset journal exists), and no desktop will ever pick it up. Same call, different verdict: **refuse with an actionable error** ("read-only shared copy; your practice is recorded separately"). See §15 question 7 for the alternative.
2. **Reviews and attempts belong to the user, not the dataset.** [`is_app_scope_kind`](../../src-tauri/src/sync/change_log.rs#L186-L190) covers only `dictation` and `xp`, so `card_review` and `read_attempt_save` journal under the **dataset** scope — per-user history filed as a write into someone else's shared content. For `card_review` this understates the problem: the state is not per-user *representable* today (no user column) and its journal payload is a fold, so replay double-applies it. §17 is the accepted model; `read_attempt_save` needs the same treatment. **Both are now resolved:** `card_review` (§17) and `read_attempt_save`/`read_attempt_delete` are per-user events in the app DB under `@app`, and `is_app_scope_kind` covers all of them.
3. **A downloaded base must never be a write target — and one route today skips the guard.** [`card_sync_full`](../../src-tauri/src/datasets/cards/sync.rs#L437-L444) resolves its directory with the **read** resolver and then opens that DB and applies pulled rows, so a card deck living under the download root would be written straight into the frozen base, past the fork/refuse logic.

   Two verified facts make this **prospective, not live**:

   - `dataset_sync_snapshot` extracts into `type_root` = `<datasets>/<type>/<uuid>` ([`client.rs#L361-L366`](../../src-tauri/src/sync/client.rs#L361-L366)) — a writable root, not the download root.
   - **No current code path populates `download_root`** ([`is_under_download_root`](../../src-tauri/src/datasets/mod.rs#L283-L293) is only ever read, never written to), so the read-only-by-location machinery is armed for an adoption flow that does not exist yet.

   The half that *is* live is different and worse: because a rating writes `card.familiarity` and bumps `card.updated_at` (§17.1), the card push channel ships **practice state as content edits**, and the website accepts them by design — `canPushDataset` admits the owner **or a subscriber** ([guide §3.6](../app_dev_guide_20261013_063113.md)), and a successful push bumps the catalog `snapshot_seq` for every follower. The fix is §17, not a provenance gate: a co-editor editing *content* is the site's intent; a subscriber's studying appearing as an edit is not.

### 16.4 Storage, eviction, and the way out

- **Eviction is free — ship it in the same phase.** Because runtime state is never inside the dataset dir (INV-9), deleting a directory to reclaim phone storage loses bytes only: progress and XP are untouched, and a re-download rejoins them by media uuid. A phone facing multi-GB shared audiobooks needs `downloaded / N MB on device / evict / re-download` more than it needs any other control in this mode.
- **Leaving the mode must be a dump, not a merge.** When this user eventually buys a PC, `GET /api/v1/app/state` (INV-21) hands the new authority the whole history, and datasets are re-adopted from the registry — no dataset journal is involved in either step, so the migration is trivial *by construction* rather than by negotiation. Build the export with that exit in mind, and never let `@app` retention be the only copy of a row.
- **Practice-ready has to be visible before download.** The catalog must say whether a dataset's cues and waveforms are built, or a phone's market will serve downloads that open with an empty waveform and no cues and the user cannot tell why. `ready` / `not_ready` is already computed locally from the presence of `data.sqlite3` ([`mod.rs#L654-L657`](../../src-tauri/src/datasets/mod.rs)); the registry read should expose the same field, derived at publish time rather than probed per download.
- **Never two upstreams.** If a device in this mode later gains a desktop hub, its `@app` writes go to the hub and the hub mirrors up (INV-19); the phone's queue must not drain to both.

### 16.5 Acceptance

A phone with no hub configured anywhere, in order:

1. Log in; `/api/v1/cluster` reports `authority_kind = cloud`, `features[] = [registry_reads, app_journal]`, and no `dataset_sync_state` row claims a desktop upstream.
2. Browse the registry, preview one dictation and one card dataset, download both; each is marked practice-ready in the listing before download (cues + waveforms present, nothing built locally).
3. Try to edit a cue, create a dataset, download a card deck and review it into the publisher's rows: all **refused**, with an error that names the mode; no overlay directory appears and nothing is written under the download root.
4. Practice for a week offline; the queue grows and is reported on the status surface, not silently.
5. Reconnect: progress and XP land on the website and match the phone exactly (dedup on `change_id`, totals derived).
6. Uninstall and reinstall, log in: history backfill restores progress and XP to the same values.
7. Evict the largest dataset to free space, practice the remainder, re-download: progress was never lost, because it was never in the directory.
8. The publisher withdraws the dataset: the local copy survives, is marked `withdrawn`, and remains practicable (INV-22).
9. Point the same phone at a desktop hub: the app-scope queue drains there, and the website stops receiving writes for that scope (INV-19).

### 16.6 Requirement mapping

| # | Requirement | How it is met | Cost |
| --- | --- | --- | --- |
| 1 | Website workspace acts as the hub | `authority_kind = cloud` with `features[] = [app_journal]` only — an authority over one scope | website: 3 routes; app: 4 guard edits |
| 2 | No local hub exists | No local journal is possible (§4: pairing/rest desktop-gated, mobile refuses `role = hub`), so the design does not pretend otherwise | nothing |
| 3 | Android is a follower of the website hub | Bearer-auth round already runs this path; the app scope is added to the website branch | app: round wiring |
| 4 | Only shared datasets are usable | registry reads + adoption; `upstream_kind = registry` provenance | already built |
| 5 | No dataset control, practice only | read-only by location, and in this mode **refuse instead of fork**; reviews as `@app` rows so practice never touches the base (§17); generation is desktop-gated anyway | app: one refuse branch + the review plane move (§16.3) |

## 17. Card datasets: three classes of data, one owner each

A "card dataset" bundles three different things, and arguing about who owns "the deck" fails because they do not have one owner. The PC hub being the only truth is correct for exactly one of the three rows:

| Class | Rows | Owner | Journal scope | Why |
| --- | --- | --- | --- | --- |
| **Content** | `card`, `tag`, card-tag links | the authority that owns the deck (a desktop hub, or the cloud in practice mode) | dataset uuid | authored and edited; published as a version |
| **Review state** | the current SM-2 values for one `(user, card)` | **the user** | `@app` | produced by practicing, not by editing |
| **Review history** | one row per rating event | **the user** | `@app` | append-only, insert-once, never contested |

### 17.1 The defect that forced this

`card_test_submit` journals `{ card_uuid, quality }` — an **input** ([`cards/mod.rs#L1725-L1732`](../../src-tauri/src/datasets/cards/mod.rs)) — and both replay paths turn it back into a live `card_test_submit` call ([`rest.rs#L1174-L1177`](../../src-tauri/src/sync/rest.rs), [`client.rs#L1618-L1621`](../../src-tauri/src/sync/client.rs)). SM-2 is a fold onto whatever state that node already holds, so replay is not idempotent and no dedup ledger can save it:

1. **The author applies its own review twice, every round.** The round pushes first, the ack deletes the queue row **without advancing the cursor** ([`client.rs#L672-L679`](../../src-tauri/src/sync/client.rs#L672-L679)), then the pull applies every entry above the cursor with no self-author test ([`client.rs#L2340-L2357`](../../src-tauri/src/sync/client.rs#L2340-L2357)) and only then moves the cursor. The pending-edit guard cannot help: its own comment says the row is "reconciled once our push lands and echoes back", which is true of an upsert and false of a fold. One rating becomes two — familiarity `+2`, intervals compounded (1→3→8), ease penalized twice, a card retiring in 3 ratings instead of 6.
2. **Every node derives a different schedule**, because each folds the same rating onto its own history. Not a conflict — a permanent divergence.
3. **The drift check turns divergence into churn.** Differing rows make the logical row hash differ from the authority's, which reads as drift → `resync_required` → snapshot pull → and since `card_review` lives *inside the dataset DB*, the snapshot overwrites the follower's own practice state with the authority's fold of it.

Two further leaks, both from the same conflation:

- `card_review` is **one row per card with no user column** ([schema](../../src-tauri/src/datasets/cards/mod.rs#L507-L517)), so two people reviewing one deck overwrite a single shared schedule. A shared deck is not practiceable by two users at all.
- A rating writes **`card.familiarity` and bumps `card.updated_at`** on the content row ([`cards/mod.rs#L1709-L1714`](../../src-tauri/src/datasets/cards/mod.rs#L1709-L1714)), so practice mutates shared content — and because the website content channel selects rows by `updated_at > last_synced_at`, studying a deck pushes it as an *edit*.

XP already solved this exact problem: the hub logs the **absolute total after applying**, so a pull converges instead of re-adding (`sync.md` §3.6). INV-23 is that rule stated generally.

### 17.2 The accepted model

- **`card_review_event` in the app DB**: `uuid PK, user_id, dataset_uuid, card_uuid, quality, reviewed_at, familiarity, interval_days, ease_factor, repetitions, next_review_at` — one row per rating, **carrying its outcome**. The uuid is minted by the device that took the rating, so `INSERT OR IGNORE` on that uuid *is* the whole idempotency story, and it is why the kind's `AppendOnly` classification finally matches its behavior.
- **`card_review_latest` view**: the newest event per `(user_id, dataset_uuid, card_uuid)`, selected by `MAX(reviewed_at || '#' || uuid)`. Including the uuid in the sort key is what makes the winner the *same row* on every node rather than "usually the same". The string-max trick works only because `reviewed_at` is fixed-width UTC (INV-6), so lexicographic order equals instant order; a variable-width or local-time stamp would silently break the tiebreak and re-introduce path dependence.
- **State is read, never re-derived on replay.** Two devices rating one card offline keep both events and agree on which one is current. Convergence is set equality — no conflict engine, no coalescing, and no path dependence.
- **Journal**: kind `card_review` joins [`is_app_scope_kind`](../../src-tauri/src/sync/change_log.rs#L186-L190), so `scope_for` files it under `@app`; `object_id_for` returns `{user_key}:{event uuid}` — unique per event, so the pending-edit guard can never suppress the whole kind by matching an empty id (the trap that bit `dictation` and `xp`).
- **Effective familiarity** for every read is `COALESCE(my latest review, c.familiarity)`: the shipped column stays as the deck's seed value — imports and online exports legitimately carry it — but practice no longer writes it, so a dataset's content stops changing when someone studies.
- **Cross-DB reads**: the card connection `ATTACH`es the app DB and joins `app.card_review_latest`, because the due/fresh split and the `RANDOM() * (6 - familiarity)` weighting belong in SQL rather than in a Rust loop over every card. The `user_id` is bound as a parameter in each query.
- **Migration** is lazy and one-way-per-dataset: on first open, a card DB with `card_review` rows and no events recorded for that dataset seeds one event per row for the local user (quality `NULL`, state preserved) and stamps a marker so it never re-runs. The old table is **left in place**, not dropped, so a downgrade to an older build still finds its own state.

`card_test_stats` keeps sharing its exact predicates with `card_test_get` — the queue counts must describe the pool that actually serves cards, or the overdue-starvation hint in the Review tab misleads.

### 17.3 What it buys

- **A published deck becomes practiceable by anyone.** No fork, no overlay, no write into the base: the directory a phone downloads stays byte-identical to the published version, so `overall_hash` and the registry contract keep holding, and each subscriber's schedule is their own. This is the concrete unlock for §16 and it answers §15 question 7.
- **Real history for the first time.** The old table stored only the latest state; the event log is a study record, so a history/stats surface needs no new plumbing.
- **Content edits and practice stop competing** for the same rows, which removes the cross-user collision *and* the practice-mutates-content push from the website channel.
- **Eviction is safe** (INV-9, §16.4): deleting the directory now loses bytes only, never a schedule.

### 17.4 Mixed versions

Deliberately a payload **superset**, not a new kind: the new event payload keeps `card_uuid` and `quality` and adds `event_uuid`, `user_id`, `reviewed_at` and the resulting state. An old hub replays it through the old fold (its own behavior, unchanged); a new node that meets a legacy payload (no `event_uuid`) folds it and logs a warning. Concretely: the un-upgraded hub strips fields it does not know (`event_uuid`, `user_id`, `reviewed_at`, resulting state) when it re-mints a follower's push, so a new node reading back its own change finds a legacy payload and folds locally. The limit is honest: a mixed fleet keeps the old semantics **until the hub upgrades** — upgrade the authority first, and treat `card_review` replay on a mixed fleet as best-effort until then. No `PROTOCOL_VERSION` bump is needed because nothing is refused, only folded rather than stored.

### 17.5 Cost, and what is not changed

| Area | Work |
| --- | --- |
| app DB | 1 table + 1 view + a lazy per-dataset migration marker |
| cards read paths | `card_test_get`, `card_test_stats`, the browse list and its familiarity filter move to the per-user join; the review draw gets a `user_id` binding |
| replay | one arm in each host dispatching to [`review::apply_remote`](../../src-tauri/src/datasets/cards/review.rs) (insert-or-ignore by event uuid) with a legacy fold fallback |
| journal | `is_app_scope_kind` + `object_id_for` (2 lines each) |
| website | **nothing** — its card channel carries cards and tags only, so its contract is untouched; review sync to a cloud authority still waits on phase 7a |
| not changed | the `card` table, `card_save`/`card_set_tags` semantics, snapshots, publishing, the download root |

### 17.6 What shipped, and the three open ends

Built with the change (`datasets/cards/review.rs` is the whole review plane):

- `card_review_event` + `card_review_latest` view + `card_review_migrated` marker in the app DB, ATTACHed onto a card connection so one statement can read content and my schedule together.
- Legacy rows are folded into events once per `(dataset, user)` on first open — an upgrading user keeps their intervals, and the old table is never written again (a downgrade still finds its own state there).
- `card_test_submit` appends an event and writes **nothing** to the deck: no `card_review` upsert, no `UPDATE card SET familiarity/updated_at`, no `touch_info`. It resolves the deck with the **read** resolver, so practising a read-only copy neither forks it nor dirties a published hash.
- `card_test_get`, `card_test_stats`, `card_list` and `card_get` read the acting user's newest event, so "due / fresh / mature / familiarity" mean *mine* on every device and for every deck, including one I only subscribe to.
- The journal carries the outcome under `@app` with object id `{user_key}:{event_uuid}`; both replay arms store the event and the hub fans it out onward (the follower's echo is suppressed by the `ApplyGuard`, so the author's own row landing back is a no-op `INSERT OR IGNORE`).
- Mixed versions: the payload keeps `card_uuid` + `quality`, so an un-upgraded hub still replays with its old semantics; a new node handed a legacy payload folds it locally and logs a warning. No `PROTOCOL_VERSION` bump.

Deliberately **not** done, each with the reason it is safe to leave:

1. **Publishing does not strip `card_review`.** A deck shared from a PC still ships the publisher's legacy rows, and `migrate_legacy` seeds them as the borrower's own state — the same inheritance leak §17.4 names, in the one place it cannot be argued away by the schema. The honest fix is at publish time (drop the table from the exported DB, as the manifest already excludes derived files); until then, seeding is a *generous* default rather than a wrong one, because the borrower's first own rating overrides the inherited value per card.
2. **`GET /api/v1/app/state` does not serve review events.** It backfills `dictation` and `xp` only, so a device whose app cursor is older than the hub's prune mark gets every retained `card_review` row from the journal pull but cannot recover the pruned ones. `needs_history` still reports the gap rather than hiding it, and the events are append-only, so a future state read over `card_review_event` is additive — no wire change to what is shipped now.
3. **`read_attempt_save` now follows the same model (done).** Attempts are per-user events in the app DB (`read_attempt_event` in `datasets/read_aloud.rs`), journaled under `@app` with object id `{user_key}:{uuid}` and replayed by `INSERT OR IGNORE`; the recording moved out of the dataset's `media/` into a per-user `<workspace>/read_aloud_audio/` dir, so practising never forks an adopted base, never dirties `read_text`, and never ships one user's takes to every subscriber. `read_attempt_delete` is the first `@app`-scoped delete: it propagates via the journal only (no dataset tombstone, which would mutate a downloaded base). Recordings are device-local — the row syncs, the WAV does not — exactly as a card review carries no media.

## 18. Change-kind catalogue

Every journaled kind, its class, its scope, and the rule that makes its replay safe. A new kind must appear here before it ships; a kind missing from this table is a defect, and a kind whose class and payload disagree is the defect §17.1 documents.

| Kind | Class | Scope | Payload contract | Notes |
| --- | --- | --- | --- | --- |
| `dictation` | `State` (coalesces) | `@app` | current progress row for `(user_key, media_uuid)` | absolute, LWW by `edit_time` |
| `xp` | `Counter` (never coalesces) | `@app` | **absolute `lifetime_xp` after applying**, not a delta | INV-16, INV-23; a skipped duplicate leaves the total correct |
| `card_review` | `AppendOnly` (never coalesces) | `@app` (post-§17; dataset scope before) | one event with its outcome: `{event_uuid, user_id, card_uuid, quality, reviewed_at, resulting state}` | INV-23; `INSERT OR IGNORE` by `event_uuid` is the whole idempotency story |
| `read_attempt_save` | `AppendOnly` | `@app` (post read-aloud plane; dataset scope before) | one attempt event with its outcome: `{uuid, user_id, dataset_uuid, text_uuid, audio_path, recognized, score, created_at}` | `INSERT OR IGNORE` by `uuid`; recording is device-local (not in the payload), so replay never writes the dataset |
| `read_attempt_delete` | `AppendOnly` | `@app` | `{uuid, dataset_uuid}` | delete the owner's event from the app DB; journal-only (no dataset tombstone) |
| `card`, `tag`, `card_tag` | `State` | dataset | current row + `updated_at` | LWW against the live row (INV-5) |
| `cue` | `State` | dataset | current cue row | LWW |
| `subtitle`, `media` | `State` | dataset | row + content hash | LWW |
| `book_*`, `read_aloud_*` | `State` | dataset | current row | LWW |
| tombstone (any kind) | n/a | dataset or `@app` | `object_id`, `deleted_at` | kept forever (INV-5) |

Rules the table enforces, all corollaries of §5:

- Every `AppendOnly` kind carries a result, not an input (INV-23). If replaying it needs to fold, it is misclassified.
- Every `Counter` kind stores the post-apply absolute, not the delta (INV-16).
- Every kind has exactly one scope. A kind that appears under two scopes is the defect §16.3 item 2 documents.
- Object ids are unique per event and never empty — the pending-edit trap that bit `dictation` and `xp` before §17.2.
- A new kind is added here in the same PR that adds it to [`is_app_scope_kind`](../../src-tauri/src/sync/change_log.rs) / [`scope_for`](../../src-tauri/src/sync/change_log.rs), or the review is incomplete.

## 19. Observability and status surface

Sync fails silently more often than it fails loudly. Every invariant that says "refused", "counted", or "surfaced" needs a place the user — and the agent — can actually see it. This section is the contract those references point at.

### 19.1 What a round logs

Every round emits one structured log line with: `authority_kind`, `epoch`, `features[]` as fetched at the start of the round, scopes attempted, and per scope `{pushed, pulled, dropped_pending, refused, deduped, resync_required}` plus cursor before/after and `pruned_up_to` if the journal reported it. Individual rejections log at `warn` with the typed error from §7. `features[]` is re-fetched on every round, on any 4xx that names a scope, and on every epoch change (INV-20).

### 19.2 What the status surface shows

A single screen, reachable from settings and mirrored by an MCP tool so an agent can read it, that names:

- the authority (`kind`, `epoch`, `origin`) and every scope's advertised capability (INV-20);
- per-scope queue depth and the oldest pending `edit_time`;
- **dropped-pending count** (INV-14 losses — a nonzero value is a red flag, not a statistic);
- last-round result and the typed error of any refused write;
- `resync_required` history with the reason (retention gap, epoch bump, host reinstall);
- `needs_history` gaps (§17.6 item 2) rather than hiding them;
- provenance of every dataset (`upstream_kind`, `upstream_origin`, `cursor`, `availability`).

### 19.3 Operator-side metrics

The website authority exports per-cluster counters mirroring §19.2 (queue depth, drain latency, inbox age, publish rate, takedown count). A follower never exports anything; it only reads its own state. The desktop hub logs the same counters to the rotating app log so a support bundle carries them.

### 19.4 "The queue keeps growing" — a diagnosis contract

It is always one of:

1. the authority does not advertise the scope (INV-20, §16.2);
2. the account is suspended or over quota (§11.6);
3. auth is failing (`401 re-pair`, §11.6);
4. the network is unreachable and the retry budget is exhausted;
5. the journal was reset (`host_seq < cursor`, §10) and resync is required.

The status surface names the cause rather than showing a spinner. An MCP tool exposes the same five-cause diagnosis so an agent can act on it without a human reading a log.

## 20. Requirement traceability

The seven product requirements this design was written against (stated by the product owner, in their order), and where each is answered:

| # | Requirement | Answer | Guarded by |
| --- | --- | --- | --- |
| 1 | Website is a sharing storage center: A uploads a dataset to share, B searches for it and downloads it | §8.4 registry — publish pins `v<N>`, adopt installs a copy under B's own provenance | INV-8, INV-17 |
| 2 | Website is editable, but edits never reach the shared files; a website workspace acts like a follower that cannot push | §8.3 inbox + §4 — optimistic on the owner's mirror, proposed to the inbox, only the authority journals it | INV-1, INV-17 |
| 3 | Uploading a *shared* dataset and uploading a *working* dataset are completely separate | §8.2 vs §8.4 — mirror push refreshes only the owner's copy; `POST /publish` is what makes anything visible to others | INV-17, INV-8 |
| 4 | The hub can pull updates from the website workspace and merge them in | §8.3 step 2 + §9 flow 2 — the hub drains the inbox through its own conflict rules | INV-5, INV-14, INV-18 |
| 5 | The local hub stores and controls all of a user's data | §4 — exactly one authority per cluster owns the journal, and every merge resolves there | INV-1, INV-5, INV-13 |
| 6 | The hub pushes XP up, so the website shows the latest | §8.2 + §9 flow 4 — `@app` journal entries mirrored, totals recomputed from rows | INV-16, INV-3 |
| 7 | Whether other local followers also sync with the website | §4 — no second upstream; registry reads only, as an explicit adopt (§9 flow 6). Left open in §15 q5 | INV-1, INV-7 |
