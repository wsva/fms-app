# FmS Sync Design v1 (actual-situations edition)

This document replaces [sync_design.md](./sync_design.md) as the working blueprint.
v8 is a good general reference, but it was written before fms-app had any sync and
assumes machinery we do not have (seq counters, row-level change logs, per-file hash
stores). This version starts from **what is actually implemented today**, names the
gaps, and proposes the smallest evolution that closes them — for datasets and for
device chat, across our real fleet: **two PCs and an Android phone, one user, one
trusted home LAN + Tailscale**.

Nothing here is mandatory. Where v8 and reality disagree, reality wins.

---

## 1. What exists today (implemented, in this codebase)

### 1.1 Topology

- **Hub-and-spoke, PC as hub.** The desktop `web_service` (port **35711**) serves a
  REST API (`/api/v1`); Android thin clients and other PCs are callers. There is no
  elected leader concept yet — the "hub" is simply the machine whose address you
  typed or discovered (`settings.pc_url`).
- **Pairing & trust** (`sync/pairing.rs`, `sync/server.rs::zone_guard`):
  - Loopback trusted only with a Host-header check (DNS-rebinding guard);
    Tailscale (100.64.0.0/10) trusted outright; LAN/Tailscale-other-subnet require
    a signed request.
  - Each follower device holds an **Ed25519 identity keypair** (`device_id` +
    `device_seed` in global settings) and signs every request
    (`x-fms-device` / `x-fms-ts` / `x-fms-sig` over `"{ts}\n{METHOD}\n{path}"`).
  - Approval is a confirm dialog on the PC owner's side (`/pair/request` →
    poll `/pair/status`); the device registry lives in the app DB, revocable from
    Settings and via MCP (`pairing_list_devices`, `pairing_revoke_device`).
- **Discovery** (`sync/discover.rs`): UDP probe to broadcast+multicast on port **35712**
  for LAN, Tailscale /24 TCP probe as fallback, manual address entry as last resort.
  Self-discovery is filtered out.

### 1.2 Dataset sync

Unit of sync: one **dataset directory** (`<datasets>/<type>/<uuid>`, type ∈
`dictation | card | book`). Flow today (`sync/client.rs::dataset_sync_snapshot` ⇄
`sync/rest.rs`):

1. `GET /api/v1/datasets/{uuid}/manifest` → `{ overall_hash, total_bytes,
   file_count, dataset_type }`.
2. If `overall_hash` equals the local `dataset_sync_state.overall_hash`, stop
   (nothing to do).
3. Otherwise flush the writeback queue, then `GET .../snapshot` streams the **whole
   dataset directory as one tar.gz** (PC side: WAL checkpoint → tar → gzip level 1 →
   streamed, never written to disk on the PC).
4. Phone/desktop extracts to `<uuid>.tmp`, atomically swaps it in, records the hash.

Server side is type-agnostic: `datasets_list` aggregates dictation, card, and book
datasets; `find_dataset_dir_typed` resolves any uuid.

### 1.3 Writeback (follower → hub)

- Mobile write commands (dictation progress, XP, cue save/delete, card
  save/delete/review/tags, book chapter/sentence/word save/delete) enqueue a JSON
  row change into `writeback_queue` (app DB) under `#[cfg(not(feature = "desktop"))]`.
  Desktop writes go straight to the DB — **so a second PC currently cannot feed the
  queue at all; PC↔PC sync is pull-only.**
- `POST /api/v1/sync/changes` sends batches of 50; the hub `replay()`s each `kind`
  by invoking the same command fn used by the UI, and returns per-change acks.
  Acked ids are deleted from the queue; failures stay queued. Replays are
  last-write-wins by `queued_at`.
- Identity: per-user rows (dictation progress, XP) carry `user_key`
  (`workspace_identity`) and are rebound to the device's `bound_user_id` on replay;
  shared dataset content (cues, cards, books) is device-trust-only.
- **Not writeback-eligible by design**: binary/management ops — media/audio writes,
  dataset create/delete/move, STT/model ops.
- **There is no push side for desktop and no pull-side row log at all**: a follower
  learns about hub changes only through the whole-directory `overall_hash`, so any
  change anywhere in a dataset re-downloads everything (a multi-GB audiobook set
  moves wholesale for one edited subtitle).

### 1.4 Device chat

- Store lives **only on the hub PC**: `<workspace>/chat/messages.sqlite3` +
  `chat/attachments/<att-uuid>--<name>` (`ai/chat.rs`).
- The thread is a single shared conversation; `sender_device`/`sender_name` mark
  provenance; `created_at` (ISO-8601 UTC ms) doubles as the poll cursor
  (`chat_list_messages(after=...)`).
- Desktop commands talk to the store directly and emit a `chat-message` Tauri event.
- Android registers the *same command names* but relays to `POST /api/v1/chat/message`
  (multipart with file parts) / `GET /api/v1/chat/messages` /
  `GET /api/v1/chat/attachment/{uuid}` over native reqwest — never WebView fetch
  (CORS).
- **Chat is online-only right now**: send while the hub is unreachable fails and the
  user retries. Retry safety already exists — client-generated message uuids, and
  `insert_message` is idempotent on the uuid, so a lost-response retry cannot
  duplicate.
- Attachments on Android are lazy-downloaded into `chat/cache/`.

### 1.5 Agent-friendliness

All of the above has MCP twins (`mcp/sync.rs`; chat in `mcp/ai.rs`): `pc_sync_scan / pc_sync_connect /
pc_sync_status / pc_sync_pull / pc_sync_pull_all`, pairing tools, chat tools. Keep
this parity rule for everything added below.

---

## 2. Requirements from the actual situation

1. **Three devices, one user**: Leader PC (always on, hub, web service), Laptop PC
   (second full copy), Phone (thin client, storage- and battery-constrained).
2. **Datasets must sync both directions eventually**, including from a second PC —
   today only the phone can push edits back.
3. **Cheap incremental sync**: media files are large and immutable in practice
   (subtitles/waveforms/DB rows change; mp3s usually don't). Re-downloading a whole
   dataset for one row edit is the main pain.
4. **Chat must work offline-ish on the phone** and survive hub restarts, without
   inventing a second source of truth.
5. Keep the security model we already built (pairing keys, trust zones) — do not
   add accounts, cloud, or E2EE crypto.
6. No backup/recovery, no multi-cluster, no merging of diverged workspaces (same
   scope cut as v8 §0).

---

## 3. Target model (evolution, not rewrite)

### 3.1 Roles

A role is a **runtime property stored per workspace**, *not* a compile flag. The hub
and the fat follower are the *same desktop binary* (`cfg(feature = "desktop")` only
means "can run the web service"); it cannot tell you which role this machine plays
today. Decide it at runtime from a `role` field so the hub writes straight to its
authority copy while a fat follower writes locally *and* enqueues.

- **Hub** = the desktop whose workspace is designated the authority. Runs the web
  service, holds the authority copy of every dataset and the chat store, appends to
  `sync_log`, and **never enqueues to the writeback queue** (that would echo its own
  writes back to itself). This is today's de-facto role, formalized.
- **Thin follower** = phone. Pulls datasets it wants (`dataset_sync_state` doubles
  as the "downloaded" mark), enqueues everything it edits.
- **Fat follower** = second PC designated a follower. Full local copies of chosen
  datasets; edits go into the writeback queue (now compiled for this role, not for
  `not(desktop)`), hub changes flow down incrementally.
- One **`cluster_id`** and the **`role`** live in the **workspace** (alongside the
  chat store under `<workspace>/`), not global settings — a workspace is the unit
  that has a hub identity. Both are sent with every request, returned by `/status`,
  and put in the **discovery beacon**: every desktop runs the web service, so the
  beacon must say whether *this* one is a hub or a follower, or the phone will pair
  with the laptop. Followers refuse a different `cluster_id` — kills the "whoever's
  URL you typed is the hub" ambiguity and a whole class of mispairing.

### 3.2 Sync unit and state

- Dataset stays the unit (matches §1.2 and the directory layout — do not change it).
- **Per-dataset cursor** replaces the "hash or nothing" check. Two tables, one per
  side — **do not mix follower and hub state** in one row:

  ```sql
  -- HUB-ONLY tables (app DB on the hub)
  sync_log (seq INTEGER PRIMARY KEY AUTOINCREMENT, dataset_uuid TEXT, object_id TEXT,
            op TEXT /* upsert|delete */, edit_time TEXT, user_key TEXT, payload TEXT);
  sync_prune_marks (dataset_uuid TEXT PK, pruned_up_to INTEGER DEFAULT 0);
  applied_changes (change_id TEXT PRIMARY KEY, applied_at TEXT);   -- replay dedup, §3.6

  -- FOLLOWER state: reuse the existing dataset_sync_state, add a nullable cursor
  -- (it already holds dataset_uuid / overall_hash / synced_at)
  ALTER TABLE dataset_sync_state ADD COLUMN cursor INTEGER;   -- NULL = "unknown"
  ```

  `seq` **must be `AUTOINCREMENT`** (or a stored monotonic counter), not a bare
  rowid alias: pruning + `DELETE` can let a plain rowid reuse numbers, which fires
  the "hub seq < cursor" alarm falsely (§3.5). `user_key` records the row owner for
  the audit trail; under the "follower adopts the hub's bound user" identity model
  (§7) it is always the hub's id, so no two-way rebind is needed.

  **The cursor column is nullable and defaults to NULL, never 0.** A `DEFAULT 0` is
  a migration bug: the moment the hub creates `sync_log`, the log is empty and its
  first `seq` counts from that point, but every existing follower copy was made from
  *earlier* hub state. A follower starting at cursor 0 would pull only post-upgrade
  changes and silently miss everything the hub changed before it. NULL means
  "unknown" ⇒ `/changes` returns `resync_required`, so every pre-existing copy does
  exactly one snapshot resync after the upgrade. Expect a **one-time `overall_hash`
  mismatch** too, because its definition now excludes `data.sqlite3` — also resolved
  by that first resync.

  `writeback_queue` gains an `edit_time` column (device-local ISO timestamp of the
  user action). Coalescing is **only valid for `state` kinds** (§3.6): a state queue
  is keyed unique by `(kind, dataset_uuid, object_id)` so repeated edits to one row
  keep only the latest (today every action inserts a new uuid row; 50 stale cue
  revisions all flush). Append-only and counter kinds keep their distinct ids and
  are **never** coalesced.
- **Write order in `write_and_log()` is: dataset DB first, `sync_log` append second.**
  The reverse (log then row) can lose a change: the hub's `seq`/snapshot is taken
  after the log entry exists but before the row is written, so the snapshot copy
  misses the row while the log already counts it. A crash *between* the two writes
  leaves the row un-logged — caught by the logical drift hash below (the row's
  presence/absence changes the row hash, forcing resync).
- The hub appends to `sync_log` at a **single choke point** (below), not at each
  call site.
- The manifest gets **per-file hashes**, but **excludes `data.sqlite3`, `-wal` and
  `-shm`** from that file set — the DB is content that the row log and snapshot own.
  If it were file-hashed too, any row edit would change its hash and the follower
  would file-fetch the whole DB over the row log, and the two would fight. Keep the
  `overall_hash` (over the non-DB file set) as a fast "nothing changed"
  short-circuit. **Cache per-file hashes by `(path, size, mtime)`** — re-hashing
  multi-GB media on every manifest call is prohibitive; only rehash when the stat
  tuple changes.
- **Change-capture choke point, not scattered call sites.** Logging at every write
  command (UI, MCP, replay) is how you miss one. Route all dataset-row mutations
  through one `write_and_log()` helper that applies the write (dataset DB) then
  appends `sync_log` (app DB). Note these are **two files, not one transaction** —
  that is exactly why the DB-first/log-second order above matters and why a drift
  check is needed to catch the crash-between-the-two case.
- **Drift check (must be a *logical* hash, or it loops).** A raw content hash of the
  SQLite *file* can never match between hub and follower — page layout, WAL history
  and `VACUUM INTO` all differ — so a naive compare would fail after every resync and
  trigger another forever. Instead:
  1. Hash **rows**, not bytes: canonical row order, or an incrementally maintained
     XOR of per-row hashes.
  2. Compare **only when the hub's `seq` equals the follower's cursor and the
     follower's queue for that dataset is empty** — i.e. the two are supposed to
     agree. Otherwise every pending local edit looks like drift. The hub returns the
     logical hash *computed at that seq*.
  3. **Cap at one automatic resync per round**, then raise a user-visible warning
     instead of silently looping.

### 3.3 Protocol additions

Keep all existing endpoints; add:

| Call | Purpose |
|---|---|
| `GET /api/v1/datasets/{uuid}/changes?after=S` | Row changes for D after seq S (from `sync_log`); response carries `pruned_up_to` |
| `GET /api/v1/file?dataset=D&path=P` | One non-DB file's bytes. **Must** canonicalize `path` and reject anything resolving outside the dataset dir (path traversal), and honour `Range` so a multi-GB media fetch on phone Wi-Fi can resume. On a **client-side hash mismatch**: refetch once; if it mismatches again, mark the dataset for resync (do not accept a corrupt file) |
| `GET /api/v1/chat/messages?after_id=N` | exists ✅ as `after=C` (timestamp) — add a distinct **`after_id`** integer param for the rowid cursor (§4.3). Do *not* overload `after`: an old phone sends a timestamp and a new hub would read it as an integer |
| `POST /api/v1/sync/changes` | exists ✅ — becomes the universal push (queue→log), accepts follower-enqueued rows too |

**Signature must cover the body and query string.** Today the Ed25519 request signs
only `ts\nMETHOD\npath` ([`sync/client.rs::with_device_auth`](../src-tauri/src/sync/client.rs))
— no body, no query. Now that `/sync/changes` mutates a lot, extend the signed
message to include a **hash of the request body and the query string**, or a captured
POST is replayable verbatim. (`applied_changes` §3.6 already blunts *idempotent*
replay of the same `change_id`, but not of a body whose change_ids are fresh.)

**Normal sync round** (replaces today's manual per-dataset "Pull" click; run on app
start, after a local write, on a timer, and on network change — v8 §3 trigger list
is good):

1. Push: flush `writeback_queue` (for any non-hub role). A failed push must NOT
   block the pull from protecting pending edits — see the apply rules below.
2. Per dataset in local state:
   a. `GET /changes?after=cursor` → apply rows → advance cursor.
   b. Compare `overall_hash`; if it differs, `GET` the per-file hash map, diff
      against local, and `/file`-fetch changed/added non-DB files. (This is how a
      follower learns a **media** file was added or changed — the row log does not
      cover files.) **Prune only files that were in the previous remote set and have
      since vanished** — track `last_remote_file_set` per dataset, so a locally
      generated waveform or imported media that was never on the hub is *not*
      deleted. **Skip pruning entirely when the hash-map fetch was incomplete**
      (partial map ⇒ treat every absent entry as unknown, not deleted).
3. Chat: pull `messages?after_id=rowidCursor`, flush chat outbox (§4).
4. Refresh catalog (`GET /datasets` with a `lite=1` flag that skips per-dataset
   row counts, so this stays one cheap query).

**Apply rules (the pull must never clobber an unsent local edit):**

- The apply path writes rows **without enqueueing and without logging** — otherwise
  hub changes echo back out and bounce between nodes forever.
- **Skip a pulled row if the object has a pending queue entry with a later
  `edit_time`** — your own newer edit wins until it is acked and flushed.
- **Drop a queue entry as soon as a pull applies a newer log entry for the same
  object** — otherwise a stale local edit lingers and re-pushes after the hub
  already has something newer.
- **A rejected push must not leave the loser permanently divergent.** The apply rule
  above skips the winning pulled row *and advances the cursor past it*; if the local
  edit is later rejected, the follower would never see the winner again. So the
  rejected ack (`{ rejected: true, winner_seq }` in §3.4) **carries the winning
  payload**, and the follower applies it with no enqueue and no log, then clears its
  now-losing queue entry.
- Set a pulled row's `updated_at` from the log's `edit_time`, not from replay time,
  so subsequent conflict comparisons are consistent on every node. This requires the
  replayed command functions to **accept an `updated_at`/`edit_time` override** —
  today they compute their own `now` internally (e.g.
  [`card_save`](../src-tauri/src/datasets/cards/mod.rs),
  [`card_test_submit`](../src-tauri/src/datasets/cards/mod.rs)), so last-write-wins silently
  doesn't work until that parameter is threaded through.

**Snapshot consistency:** the current streamer does `PRAGMA wal_checkpoint(TRUNCATE)`
and then tars the live directory ([sync/rest.rs `checkpoint_db`](../src-tauri/src/sync/rest.rs)).
A checkpoint does **not** stop concurrent writers, so once the hub is written to
constantly the tar can capture a **torn** `data.sqlite3`. The fix is to archive a
point-in-time copy made with `VACUUM INTO` / the backup API instead of the live file
— this is why §8 is promoted into the build order (step 4) rather than deferred.

**Snapshot stays** as the bootstrap and the single escape hatch ("resync from hub"):
cursor older than `pruned_up_to`, hub restored by hand, local corruption, DB drift
mismatch. v8's resync flow is right. **Read the hub's `seq` before starting the
snapshot and return it in a header**; the follower sets its cursor to that value, and
replays must be **idempotent** (see `applied_changes`, §3.6) so a change that lands
in both the snapshot and the log is harmless. See §8 for the `VACUUM INTO` change
that makes the snapshot itself non-torn.

**Unsubscribe** (phone, to free storage): flush queue for D, delete
`<datasets>/<type>/<D>`, drop `dataset_sync_state` row; the hub copy is untouched.

### 3.4 Conflicts

Adopt v8's rule with amendments that fit our data:

- **Later `edit_time` wins per row**, ties go to the hub. Clocks on automatic time
  is the assumption, plus one guard:
- **Clamp any incoming `edit_time` more than a few minutes in the future to the
  hub's `now`.** One phone with a wrong clock would otherwise win every conflict
  against every other device forever.
- On the hub, compare an incoming edit against **the live row's own `updated_at`**,
  not "the latest `sync_log` entry" — that log entry can be pruned (§3.5), and the
  row's `updated_at` is the durable current state. `sync_log` is the transport
  journal; the row is the truth.
- Row-level conflicts are the *only* conflicts. Binary files (media, waveforms) are
  content-addressed and immutable; nobody edits an mp3 in place, so file-level merge
  is out of scope. If a file hash diverges, the hub's version wins and the follower
  refetches (log a notice).
- **Deletes use a per-dataset-DB `tombstones` table, not a `deleted_at` column on
  every synced table.** Requiring soft-delete columns would mean touching every
  synced table *and* adding a filter to every existing query — a large hidden change.
  Instead each dataset DB carries `tombstones(object_id, table, deleted_at)`, which
  travels with the dataset snapshot automatically and answers "was this object
  deleted, and when?" for the later-wins delete-vs-edit rule. **Keep tombstones
  forever** — they are tiny — which also dissolves the "how long may a device stay
  offline" question (there is no tombstone GC race to size a window against).
- **Dropped edits: do NOT rewrite `sync_log` entries for losers.** A losing push is
  simply **not given a seq** and its ack returns
  `{ rejected: true, winner_seq, winner_payload }` so the pusher both tells the user
  "your edit from X was overwritten" *and* applies `winner_payload` (§3.3 apply
  rules). Rewriting log rows in place would corrupt the cursor semantics for every
  other follower.

### 3.5 Log retention & safety checks

- Hub prunes `sync_log` records older than 30 days and records the highest pruned
  `seq` in `sync_prune_marks.pruned_up_to`. A follower is told to resync when its
  **cursor < pruned_up_to** — *not* "before the oldest remaining entry" (the oldest
  survivor is usually newer than an up-to-date cursor, so that test misfires).
  `/changes` returns `{ resync_required: true }`; the follower falls back to
  snapshot-resync with its queue flushed first, so nothing queued is lost.
- **Tombstones are never pruned** (§3.4 keeps them forever, they are tiny), so a
  resurrected edit from a long-offline device still collides with the delete and is
  dropped rather than silently recreating the row. The `sync_log` *records* are what
  get pruned at 30 days — the `tombstones` table and the live row `updated_at` are
  the durable conflict inputs, which is why "compare against the log entry" is not
  relied on.
- A queue flush by a follower past `pruned_up_to` happens **before** resync, so an
  old queued edit could overwrite newer state; the §3.4 "compare against the live
  row `updated_at`" rule is what stops that — never compare against a prunable log
  entry.
- `cluster_id` mismatch → refuse (with the trust-on-first-use adoption in §7 for
  not-yet-upgraded devices). Hub `seq` lower than follower cursor → stop and suggest
  resync (the `AUTOINCREMENT` fix in §3.2 is what makes this alarm trustworthy).

### 3.6 Writeback kind classification (state / append-only / counter)

Coalescing and later-wins are only correct for **state** rows. Classify every
writeback `kind` explicitly — getting this wrong silently loses data:

| Class | Kinds | Queue rule | Conflict rule |
|---|---|---|---|
| **state** | cue save/delete, card save/delete/tags, book chapter/sentence/word | coalesce by `object_id`, keep latest | later `edit_time` wins |
| **append-only** | card_review and any history rows | **never** coalesce; safe **only because inserted by uuid** (idempotent PK) | no conflict — all kept |
| **counter** | XP (enqueued as an `amount` **delta**, see `xp.rs`) | never coalesce | sum the deltas |

**Replay deduplication (mandatory, because acks are lossy):** the flush deletes a
queue row only *after* the ack arrives. If that response is lost in transit, the
retry re-sends the same change and the hub would apply a counter **delta twice**.
So every queued change carries a stable `change_id` (its queue uuid), and the hub
replays through `applied_changes` (§3.2): if the `change_id` is
already recorded, skip it (still ack ok). `applied_changes` is pruned by
**`applied_at`** (wall time on the hub), *not* by `edit_time` — an old queued edit
that flushes late must still find its dedup record, otherwise a genuinely-new retry of
that stale change would double-apply. Additionally, the hub writes the **absolute XP
total after applying** into the `sync_log` payload, so a pull converges the follower
to the authoritative number rather than re-adding deltas. Append-only rows are the
only safe-without-dedup case, and only because their PK is the client uuid.

**Counter pull display:** if a pulled absolute XP total simply overwrites the local
value, any *pending* local deltas vanish from the display until they flush (the user
sees their XP drop). Show `hub_total + sum(pending local deltas)` instead; reconcile
to `hub_total` once those deltas are acked.

Deleting a parent (dataset/row) with live children: the later-wins rule drops an
edit to an object whose **dataset or parent** was deleted, with a short user notice —
keeps orphaned children from reappearing (v8 §4).

---

## 4. Chat sync

Design principle: **the hub remains the only chat authority**; the phone gets
offline send/receive without a second writable replica.

1. **Local mirror**: phone persists messages into a local `chat/mirror.sqlite3`
   (same schema + the hub rowid). UI reads the mirror — thread history survives
   hub-off periods. **Upsert mirror rows by message uuid**, so a message that arrives
   both through the outbox (own send) and through a pull never duplicates.
2. **Outbox**: a send while the hub is unreachable inserts into the outbox with a
   client-generated uuid + `edit_time` instead of failing. The UI renders outbox
   messages **at the end of the thread with a "pending" marker** (not hidden), so the
   user sees what they sent even before it reaches the hub. On every sync round, POST
   the outbox to `/api/v1/chat/message`; the hub's uuid-idempotent `insert_message`
   makes retries free; on ack the message graduates from "pending" to its real
   position.
3. **Cursor: use a hub-side insertion counter (rowid/seq), not `created_at`.**
   Today `created_at` is stamped hub-side at insert
   ([`ai/chat.rs::insert_message`](../src-tauri/src/ai/chat.rs)), so it is already
   monotonic — but a cursor that reads a client-supplied `created_at` (the earlier
   draft's "accept client-time as final") would break the poll: a message composed
   offline at 10:00 and flushed at 10:30 lands with `created_at` 10:00, and a peer
   that had already pulled to 10:20 never sees it. Don't put wall-clock on the
   critical path — paginate on an integer id, keep `created_at` for display order.
4. **Chat cursor migration (a real constraint).** `chat_messages` is
   `uuid TEXT PRIMARY KEY`; SQLite **cannot `ALTER` an existing table to add an
   `AUTOINCREMENT` integer PK**, and the implicit rowid is `max+1` (reused after a
   delete). So this needs a one-time **table rebuild**: create a new table with
   `id INTEGER PRIMARY KEY AUTOINCREMENT`, `INSERT … SELECT` copying existing rows
   **ordered by `created_at`** so historical display order and cursor order agree,
   drop the old table, rename. Do it in the same migration that introduces mirroring.
5. **Pull**: replace the current "phone polls REST when the page is open" with the
   §3.3 round (`messages?after=rowidCursor` into the mirror). The desktop keeps event
   push (`chat-message`) — no change needed there.
6. **Attachments** stay hub-resident, lazy-downloaded to `chat/cache/` as today. An
   offline-sent attachment is **copied into an app-private outbox dir at send time**
   (Android content URIs may not survive to flush); flush streams the outbox copy
   through the existing multipart endpoint. Cache pruning: keep newest N MB, drop
   attachments older than the mirror window (phone storage).
7. **Deletions**: if chat messages ever become deletable, they need tombstones in
   the mirror (a plain delete can't be represented by the rowid cursor). Not needed
   today — there is no delete path — but note it before adding one.
8. **PC↔PC chat**: the fat follower mirrors the same way and posts through REST —
   do not write to a local copy of the chat DB. One thread, one store.
9. **Multi-user note**: if a second *person* ever shares the hub, chat needs
   per-workspace threads (store is already `<workspace>/chat`, so this is a
   routing change in `zone_guard`/`sync/rest.rs`, not a schema change). Out of scope now.

---

## 5. UI and status

- **Datasets Sync page** becomes the single sync surface: replaces the manual
  per-dataset Pull/Upload buttons with a "Sync now" round + per-dataset rows
  (downloaded / not downloaded / resync / unsubscribe), pending-queue count, last
  successful round, cluster id, hub address. Mobile-only actions gated as today.
- **Status screen** (v8 §7 — adopted): role, cluster, **`protocol_version`**, hub
  address + reachable?, per-dataset cursor + last sync + queued changes, per-device
  pairing entries. This is the main diagnostic tool, and its JSON twin must be an MCP
  tool (`sync_status_detail`) so agents can diagnose too.
- Chat page: outbox indicator (☐ messages pending) — same pattern as the writeback
  badge.

---

## 6. Explicitly out of scope

- CRDTs / per-field merge / conflict copies (v8 killed these; agreed).
- Server-side scopes / catalogs-as-authority — the hub syncs whatever a follower
  asks for; "scope" is just which datasets have a local copy. Simpler than v8 §1
  and sufficient for one user's three devices.
- **Hub failover or follower↔follower sync.** The hub is one machine; roles are
  immutable (§3.1) and this design has no backup/recovery scope, so **there is no
  failover** — if the hub dies you bring it back, you do not promote a follower.
- Off-LAN relay, push notifications, background sync on Android beyond
  app-foreground + Wi-Fi preference.
- TLS/pinning (revisit only if pairing ever crosses an untrusted network).

---

## 7. Gaps to decide (open before implementation)

- **Identity: not a no-op even for one human.** The hub's `user_key` and each
  device's local user key differ, so "rebind both ways" is fiddly. The simpler
  answer: **the follower adopts the hub's bound user at pairing time**, after which
  every device writes progress under the *same* id and no two-way rebind is needed.
  `sync_log.user_key` (§3.2) still records the owner for the audit trail, but under
  adopted-identity it is always the hub's — decide explicitly whether to keep the
  field or drop it.
- **Upgrading already-deployed devices.** Existing paired phones/PCs have no
  `cluster_id` and no `role`. Use **trust-on-first-use**: a follower adopts the
  hub's `cluster_id` on the next *successful signed* contact instead of refusing it;
  a hub adopts `role = hub` and every existing desktop workspace becomes a `follower`
  on first pairing. **Demoted-PC dataset handling (decide now — step 5 depends on
  it):** do *not* auto-overwrite. On first pairing **keep the follower's local
  datasets untouched and mark them "local, not synced"**, then offer **"adopt hub
  copy" per dataset** as an explicit, overwriting action. This sidesteps the hazard
  that a follower's dictation progress lives inside its `data.sqlite3` (an automatic
  first pull would silently destroy it). Add a
  **`protocol_version` to `/status`**: the three devices will not upgrade in the same
  moment, and the chat-cursor rebuild (§4.4) plus `/changes`/`/file` must be
  rejected against an old hub rather than half-working.
- **Dataset-level ops stay out of the row queue, but the catalog needs a `deleted`
  flag**, or followers keep deleted datasets forever. And say what happens to a
  dataset *created on a fat follower*: today it is never pushed (structural ops
  aren't writeback-eligible). Either add a `dataset_create` op, or make the UI say
  "this dataset is local-only, create it on the hub".
- **Hub identity change.** If the hub workspace is recreated it gets a new
  `cluster_id` and every follower refuses it. Provide an explicit **"forget hub /
  re-pair"** action on followers rather than making users edit settings by hand.
- **Snapshot swap and SQLite.** The atomic dir swap must close/reopen any open
  `data.sqlite3` handles (and drop WAL/SHM) around the swap — state this in the
  snapshot code path so a swapped-in DB isn't shadowed by a stale handle.
- **`/file` security.** Covered in §3.3: path-traversal validation + `Range` are
  mandatory, not optional.

## 8. Snapshot correctness: `VACUUM INTO` (now in-scope, not deferred)

The tar streamer takes `wal_checkpoint(TRUNCATE)` then archives the live DB
([sync/rest.rs](../src-tauri/src/sync/rest.rs)) — safe only while the hub is idle. Under
constant writes the tar can capture a **torn** `data.sqlite3`. The fix: archive a
**point-in-time copy** made with `VACUUM INTO` / the SQLite backup API, which reads a
consistent snapshot without blocking writers. This also opens the door to the larger
simplification — replace the tar.gz entirely with a consistent DB-only copy plus
per-file `/file` fetch, so one mechanism serves bootstrap, resync and incremental,
downloads become resumable (Range), and we stop gzipping already-compressed mp3s
(level 1 on media is ~0% today). At minimum the **torn-DB fix lands in step 4**
(below); the full tar.gz retirement can trail once `/file` is proven.

## 9. Build order

Do the **cheap-but-hard-to-change-later** items first: kind classification (§3.6),
role + `cluster_id` placement in the workspace (§3.1), and the chat rowid cursor +
its table rebuild (§4.3–4.4). They cost little now and are painful once real data has
synced through the wrong assumption.

Each step is independently shippable and keeps the app working:

1. **`cluster_id` + `role` + `protocol_version`** in the workspace; beacon + `/status`
   carry all three; zone_guard verifies. Land the **trust-on-first-use adoption**
   (§7) so existing paired devices upgrade without a hard refuse. Small, unblocks
   everything.
2. **Hub change machinery (§3.2, §3.6):** `sync_log` (AUTOINCREMENT, `user_key`) +
   `applied_changes` + `sync_prune_marks` + a per-dataset-DB `tombstones` table;
   `/changes` + `/file`; single `write_and_log()` choke point (**dataset DB first,
   log second**); **logical** row-hash drift check (not a file hash); manifest gains
   per-file hashes (DB excluded, cached by `(path,size,mtime)`); thread an
   `updated_at`/`edit_time` override through the replayed command fns (§3.3); extend
   the request signature to cover **body + query** (§3.3). Followers unchanged
   (snapshot still works).
3. **Chat rowid cursor** — the table rebuild in §4.4 (ids assigned in `created_at`
   order), then switch `list_messages` / the REST pull to the **`after_id`** integer
   param (§3.3). Independent of dataset sync; do it here while chat is small and the
   migration is cheap.
4. **Upgrade migration + snapshot correctness + incremental pull round** on the
   phone: ship the `cursor`-NULL migration so every existing follower does **exactly
   one snapshot resync** on first post-upgrade contact (§3.2); `VACUUM INTO` archive
   (§8); timer/network triggers; apply rules (§3.3: no-enqueue/no-log,
   skip-pending-newer, drop-queue-on-newer-pull, reject-carries-winner, safe file
   pruning with `last_remote_file_set`); counter pull shows
   `hub_total + pending deltas` (§3.6). **First visible payoff: no more full
   re-downloads, no torn DB, no silent pre-upgrade misses.**
5. **Follower (desktop) writeback** — enqueue/flush for the fat-follower *role*
   (not `not(desktop)`), last-write-wins by `edit_time` vs the live row `updated_at`
   / `tombstones` (§3.4), replay dedup by `change_id` pruned by `applied_at` (§3.6),
   per-role queue UI. **Requires the demoted-PC decision in §7 first** (local
   datasets kept + explicit "adopt hub copy", never auto-overwrite). Turns PC↔PC
   into two-way sync.
6. **Chat mirror + outbox** on the phone (§4.1, 4.2, 4.5); attachment outbox copy.
7. **Status screen + MCP parity + log retention/tombstones (§3.5) + reject acks.**
8. Optional later: HTTPS pinning, full tar.gz→per-file retirement (§8), multi-user
   chat routing.

**Ordering note:** keep incremental pull (step 4) **before** follower writeback (step
5). Until the pull is incremental, any edit changes the hub's `overall_hash`, so
enabling two-way writes first means every edit on one PC drags a whole-dataset
re-download onto the other. If you must ship step 5 early, keep datasets small or
accept that cost.
