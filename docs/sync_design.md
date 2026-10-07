# Sync Design v8 (LAN, PCs and Android)

Scope: one user, one trusted LAN, several PCs and Android phones. No backup or recovery features. Everything not listed here is deliberately left out.

Changes from v7: no version columns, no optimistic concurrency, no conflict copies, no heartbeat thresholds, no min-ack compaction, no separate Join/Resync flows. Conflicts are settled by edit timestamp.

## 1. Model

- **Cluster**: one workspace, one user, one leader. A random `cluster_id` is created with the leader; nodes refuse to talk to a different one.
- **Leader**: one PC. Holds all data, runs the web service, and is the only authority. It should be a machine that is usually on.
- **Follower**: any PC or phone. No web service. Holds the **catalog** plus the datasets in its **scope**.
- **Roles are immutable.** A follower's scope can change; its role cannot.
- **Dataset**: the unit of partial sync. Its rows, its per-dataset DB (for example `data.sqlite3`), its media, and its linked-location content are synced together or not at all. This is how a phone holds only some data.
- **Catalog**: a small index of all datasets (`dataset_id`, name, size, last_changed, deleted flag). Every follower holds all of it, so the UI can show datasets that are "not downloaded".
- **Seq**: the leader's change counter. Every accepted change gets the next number.
- **Change log**: `(seq, dataset_id, object_id, op, edit_time)` with `op` = upsert or delete. Deletes are tombstones. Large files are never in the log; entries reference them by `{file_id, hash, size}` and followers fetch missing hashes.
- **Node state** (never synced, never overwritten by a pull): scope, per-dataset cursor, pending queue, leader address, pairing key, location path mapping.

## 2. Pairing and Discovery

- **Pairing**: the leader shows a short code (or QR). The follower enters it and receives a pairing key, which it stores and sends with every request. Keys are per node and revocable on the leader.
- **Security**: on a trusted home LAN, plain HTTP plus the pairing key is enough. On a shared LAN, serve HTTPS with a self-signed certificate that followers pin at pairing time.
- **Finding the leader**: followers remember the leader's address. If it fails, they look for the leader by mDNS (service name includes `cluster_id`), and the user can type an address as a last resort.

## 3. Sync Protocol

All requests carry the pairing key and `cluster_id`. The leader serves only the datasets in the caller's scope.

| Call | Purpose |
|---|---|
| `GET /catalog` | The dataset index |
| `GET /snapshot?dataset=D` | A consistent copy of dataset D at seq P: its DB plus a file list (hashes). Taken in one read transaction |
| `GET /changes?dataset=D&after=S` | Log entries for D after seq S |
| `GET /file?hash=H` | One file's bytes |
| `POST /push` | The follower's queued changes |

**Subscribe to a dataset (also used for first join and for resync):**
1. `GET /snapshot` for D at seq P, then fetch any hashes not already present.
2. Set cursor(D) = P.
3. Pull `GET /changes?after=cursor` and advance the cursor.

**Normal sync** (run on app start, on a network change, on a timer of a few minutes, and right after a local edit):
1. Push the pending queue.
2. For each dataset in scope: pull changes after its cursor, apply them, advance the cursor.
3. Refresh the catalog.

**Unsubscribe**: push the queue for D first, then delete the local copy of D. The leader's data is untouched.

**Resync (manual "resync from leader" per dataset)**: push the queue, then subscribe again from a fresh snapshot. This is the single escape hatch for every odd state: cursor too old, leader's seq went backwards (leader restored by hand), or local corruption.

Phones: sync on Wi-Fi only by default, and keep scopes small to save storage and battery.

## 4. Writes and Conflicts

- **Offline**: a follower is fully usable for its datasets whenever the leader is unreachable. Edits go into the pending queue and are pushed at the next sync. "Unreachable" simply means the last request failed; no thresholds.
- **Queue**: keyed by `object_id`, so repeated edits to one object keep only the latest. Create followed by delete cancels out.
- **Conflict rule: the later `edit_time` wins**, per row. The leader keeps whichever version has the later `edit_time` and ignores the other. Ties go to the leader's version. Each edit carries the time from the device that made it, so devices should keep their clocks on automatic time (this is the one assumption of the design).
- **Delete vs. edit**: the later `edit_time` wins, with one exception: an edit to an object whose **dataset or parent** was deleted is dropped, and the user sees a short notice. This keeps orphaned children from reappearing.
- **Files**: stored by hash, so two versions of a file never overwrite each other; the row that points to a file follows the same later-wins rule.

## 5. Safety Checks

- A follower whose `cluster_id` does not match refuses to sync.
- If the leader's `seq` is lower than a follower's cursor, the follower stops syncing that dataset and tells the user to run "resync from leader".

## 6. Log Retention

- Log entries and tombstones older than a fixed window (for example 30 days) are deleted by the leader.
- A follower whose cursor is older than the oldest remaining entry is told to resync that dataset (section 3). Its queue is pushed first, so nothing queued is lost.
- Files in the content store are removed only when no row or snapshot references their hash.

## 7. Status Screen

Per node: role, scope, per dataset the last successful sync time, and the number of queued changes. This is the main tool for diagnosing problems.

## 8. Build Order

1. `cluster_id`, roles, node state outside the synced tree, pairing, and the catalog.
2. `seq`, the change log, `/changes`, and push with later-`edit_time`-wins (this matches how writeback replay already behaves).
3. Subscribe/unsubscribe/resync through snapshots and the file-by-hash fetch.
4. mDNS discovery and the sync triggers (start, network change, timer, after edit).
5. The status screen, log retention, and the parent-deleted drop rule.
6. Optional later: HTTPS pinning, opening an out-of-scope dataset online without subscribing.