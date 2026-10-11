# Multi-layer datasets (base + overlay)

This document is the contract for the "multi layer of dataset" idea tracked in [../todo.md](../todo.md#L21-L32): a shared dataset is downloaded **read-only**, every local change writes to a **new layer**, the original can be **updated safely** without disturbing local edits, reads **combine** the layers, and layers can be **merged** into one. It is the preparation for online sharing (todo item 11).

**The scenario this is designed for.** User A uploads a dataset to the online server. User B downloads it and uses it; every change B makes is written to *B's own layer*, never to the downloaded copy. When A publishes a new version, the app re-downloads the base and B simply re-combines the new base with B's unchanged layer. Because every read-only dataset arrives from the online server, they all live in one **app-maintained download directory**, and *location* — not ownership — decides read-only (§4.1).

The model is OverlayFS / a Docker image: read-only lower layers + one writable upper layer + a merged view. Nothing here is mandatory; where this doc and reality disagree, reality wins. It builds directly on [dataset-info.md](./dataset-info.md) (the unified `info.json` descriptor) and [sync.md](./sync.md) (hub-and-spoke sync, tombstones, per-file hashes).

---

## 1. Why this fits the existing architecture

Three primitives the idea needs already exist in the codebase:

1. **Immutable, versioned rows.** The cue table carries `version_created` / `version_superseded`, where "current" is `version_superseded IS NULL` ([`datasets/mod.rs`](../../src-tauri/src/datasets/mod.rs#L1993-L1999)). Data is never mutated in place — a new version is appended. That is the mental model an overlay layer needs.
2. **Per-dataset tombstones.** `crate::sync::change_log::ensure_tombstones(conn)` runs at DB creation. In OverlayFS terms a tombstone **is** a whiteout: it lets the top layer shadow a row that still exists in the read-only base.
3. **Sharing metadata.** `info.json` already carries `sharing { visibility, owner_id, subscribers }` ([`datasets/info.rs`](../../src-tauri/src/datasets/info.rs#L184-L194)), and card sync already refuses to push a dataset whose `owner_id` is not the logged-in user ([`datasets/cards/sync.rs`](../../src-tauri/src/datasets/cards/sync.rs#L441-L446)). `owner_id` stays what it is — provenance and the *push* gate — but local read-only is decided by **where the copy lives** (§4.1), not by `owner_id`, so none of the owner-based edge cases apply.
4. **Location-based roots.** Datasets already resolve through `dataset_roots()` = the default dir plus `linked_dirs` from `meta.json` ([`datasets/mod.rs`](../../src-tauri/src/datasets/mod.rs#L249-L271)). The read-only download root is one more such location, flagged read-only (§4.2).

A helpful accident of the current design: **dictation progress and XP live in the app DB, not the dataset** ([sync.md §3.3](./sync.md)). A base-layer update can therefore never destroy practice history — that hazard does not exist here.

---

## 2. The two kinds of data in a dataset

A dataset is two very different kinds of data, and layering each is a different problem:

| Kind | Examples | Layer mechanism | Difficulty |
|---|---|---|---|
| **Files** | `media/`, `waveform/`, `subtitle/`, `transcript/`, `book.txt` | Union by relative path, overlay-first. Media is large and immutable in practice, so the overlay stores only *new/changed* files. | Easy — this is literally OverlayFS. |
| **Rows** | cues, cards, book sentences in `data.sqlite3` | Overlay row wins by uuid; a tombstone shadows a base row; unchanged rows are not duplicated. | Medium — see §5. |

Every dataset type opens exactly one SQLite today (`open_db` per type: dictation, cards, books, read_aloud). That single-DB assumption is what §5 has to preserve.

---

## 3. Directory layout

Two areas, split by trust:

- **Download root** (`<workspace>/datasets/download/<type>/`, app-maintained): every dataset here is a **read-only base** pulled from the online server. The app owns this directory — it downloads, version-checks and re-pulls bases with an atomic swap. Nothing here is ever a write target (§6).
- **Writable area** (the normal `<workspace>/datasets/<type>/`): the user's own flat datasets, plus one **overlay** per downloaded base the user has edited.

```
<workspace>/datasets/
├── dictation/                 # writable user datasets (scanned today) — one per type
│   └── <uuid>/                # a flat dataset, OR the overlay for a downloaded base
│       ├── info.json          # layer { role: "overlay"|"flat", base_uuid, base_version }
│       ├── overlay/           # overlay only: the diffs
│       │   ├── data.sqlite3   # only added/changed rows + tombstones
│       │   └── media/…        # only new/changed files
│       └── merged/            # overlay only: read cache = base ⊕ overlay (§5)
│           ├── data.sqlite3
│           └── media/…        # hardlink/reflink into base + overlay, not a copy
├── card/  book/  read_aloud/  wiki/     # …same shape per type
└── download/                  # READ-ONLY download root (app-maintained)
    └── <type>/                # one subtree per type: dictation/, card/, …
        └── <uuid>/            # a base pulled from the online server
            ├── info.json
            ├── data.sqlite3
            └── media/… subtitle/… waveform/… transcript/… book.txt
```

A **flat** dataset — today's shape, a local dataset the user owns outright — keeps `info.json`, `data.sqlite3` and `media/` at the top level with no `overlay/merged` split. Location (download root vs. writable area) plus the `layer.role` field (§4.3) decide which shape a directory is, so all three coexist.

The overlay carries the **same uuid** as its base. Discovery therefore gains one rule: when a uuid exists both under `download/<type>/` and as an overlay in `<type>/`, `find_dataset_dir*` resolves it to the **overlay** (whose reads open `merged/`); the base alone resolves to itself. Placing the download root as a *sibling* of the type dirs (not `<type>/download`) keeps it collision-free with user datasets — see §4.2.

---

## 4. `info.json` changes

### 4.1 Read-only is decided by location, not `owner_id`

A dataset is **read-only** if and only if it lives under the app-maintained download root (§4.2). Everything else is writable. This is deliberately simpler than an owner-based rule:

- **Same owner, still read-only.** If user B downloads a dataset B also published, the copy under `download/` is still read-only. Read-only is a property of *where the copy lives*, not of who owns it — which is exactly the case an `owner_id` rule gets wrong.
- **`owner_id` keeps its existing job only** — provenance and the card-sync push gate ([`cards/sync.rs`](../../src-tauri/src/datasets/cards/sync.rs#L441-L446)). It is *not* consulted for local read-only, so the whole class of owner-based edge cases (empty `owner_id` on login/logout, a logged-out owner locked out of their own dataset) never arises.
- **Local datasets are untouched.** Anything not under `download/` behaves exactly as today.
- **Moving a base out of the download root is at the user's own risk.** The app only guarantees read-only handling for what it manages inside `download/`. A base the user manually relocates into the writable area becomes an ordinary writable dataset — no overlay protection, no managed updates. The app does not track or defend it.

### 4.2 The download root

Locations already resolve through `dataset_roots(settings, type)` = `<workspace>/datasets/<type>/` plus the `linked_dirs` in that folder's `meta.json` ([`datasets/mod.rs`](../../src-tauri/src/datasets/mod.rs#L249-L271)). The download root is one more root appended there — `<workspace>/datasets/download/<type>/` — flagged *read-only*:

- **Why a sibling of the type dirs, not `<type>/download`.** Discovery scans `<type>/` one level deep and treats each subdir as a dataset; a dataset's **directory name comes from the user's display name** via `sanitize_dir_name`, not from its uuid (`ensure_favorites_dataset` creates a dir literally named `Favorites` for the uuid `dictation-favorites`), and `assert_uuid_free` guards **uuids, not directory names** ([`info.rs`](../../src-tauri/src/datasets/info.rs#L318-L323)). A `download` dir placed *inside* `<type>/` would therefore collide with a user dataset named "download" (or "Download" — Windows is case-insensitive), and nothing today would catch it. Placing `download` as a sibling of the type dirs is collision-free with **no new guard**: user datasets only ever live under `<type>/`, `download` is not a valid type slug, and nothing scans `datasets/` itself for datasets.
- Give `DatasetLinkedDir` a `read_only: bool` (or have `dataset_roots` always append the reserved `download/<type>` root), so the location is flagged read-only and surfaced through `DatasetDirEntry` — which already carries `is_linked`; add `is_read_only`.
- `find_dataset_dir*` returns the path as today; a write command checks whether the resolved path is under a read-only root and, if so, routes to the overlay (§6) instead of writing.
- The app creates and manages `download/` (download, version check, atomic re-pull). The user does not add linked dirs here by hand, and `download/` is excluded from the user-facing "add location" UI.

### 4.3 The `layer` block (overlay only)

The base needs no flag — its location says it is a read-only base. The **overlay** records what it sits on, so reads can union and updates can be detected:

```jsonc
"layer": {
  "role": "overlay",           // "overlay" | "flat" (default "flat"; a base is implied by location)
  "base_uuid": "…",            // the base under download/<type>/ this overlay sits on
  "base_version": "…"          // the base updated_at/hash it was built against — staleness key
}
```

This block lives in the `#[serde(flatten)] extra` map until it is formally modeled, so it round-trips safely on old readers (adding a field is a pure addition — see [dataset-info.md](./dataset-info.md#L50-L52)). `base_uuid` is a *layer* pointer, not a discovery hierarchy — discovery stays one level deep per type, as dataset-info.md requires (the `parent_uuid` it dropped was a grouping hierarchy; this is not).

### 4.4 uuid uniqueness and selector identity

The overlay shares its base's uuid on purpose — one logical dataset, one id — but that, plus foreign uuids arriving from the download root, creates two collision cases that resolution (`find_dataset_dir_typed`) and every selector (`dataset_list`, the card/book pickers) must handle deterministically. Datasets are mounted and listed by uuid, so a repeated uuid is a real hazard, not a cosmetic one.

**Resolution is first-match by root order.** `find_dataset_dir_typed` iterates `DatasetType::ALL` and, per type, the roots from `dataset_roots` in order, returning the first dir whose `info.json` uuid matches ([`datasets/mod.rs`](../../src-tauri/src/datasets/mod.rs#L1362-L1404)). So **append the download root last** — a writable overlay/local dataset for uuid X is then always found before its base.

**Case 1 — base + its overlay (intended).** Same uuid, two dirs, one logical dataset.

- *Resolution*: the overlay wins (writable root scanned first); reads open its `merged/`.
- *Listing*: **dedupe by uuid** — `dataset_list` collapses the pair into one entry (overlay preferred), tagged `downloaded` / `has-local-changes`, so a selector never shows two cards for one id.

**Case 2 — an unrelated local *flat* dataset sharing a base's uuid (prevent, then detect).** Resolution cannot tell "overlay of X" from "independent flat dataset that happens to be X" by uuid alone, so it must never be allowed to arise silently:

- *Prevent at every mint point.* `assert_uuid_free` already scans all roots via `find_dataset_dir_typed` ([`info.rs`](../../src-tauri/src/datasets/info.rs#L311-L334)), so once the download root is registered, create/import refuse a local dataset whose uuid a base claims. Symmetrically, **download refuses a base whose uuid a local dataset already claims** (actionable error: rename the local dataset or pick another) — a publisher's uuid cannot be rewritten, so the local side yields or the download is declined.
- *Overlay creation is exempt* from `assert_uuid_free` (exactly like `ensure_favorites_dataset`): it legitimately writes a second dir with the base's uuid. Its validation is instead that `layer.base_uuid` resolves to a real base under `download/`.
- *Detect, don't guess.* If resolution still finds a **flat** dir and a base for the same uuid (guard bypassed, or a dataset that predates the download), treat it as a conflict — surface it as an audit finding / error rather than silently shadowing the base. The `layer.role` on disk is what distinguishes the two.

**Invariant.** Across the writable roots there is at most one *flat* dataset per uuid; a base under `download/` may have at most one *overlay* sharing its uuid. `assert_uuid_free` enforces the first half; overlay validation (`base_uuid` resolves to a real base) enforces the second.

*Alternative considered:* giving the overlay its **own** uuid (with `base_uuid` pointing at the base) removes the two-dirs-one-uuid case entirely, but then the selector must hide the base and map overlay→content-uuid for update checks. Same-uuid + dedup keeps the "one logical dataset" UX simpler, so it is the default.

---

## 5. Read strategy: materialized merge cache (not query-time union)

Two options:

- **A. Query-time union.** Every read `ATTACH`es base + overlay and unions them. Rejected: every type has its own `open_db` and dozens of query sites (dictation cue queries, card FTS5 search, book queries, read-aloud scoring). All would need to become layer-aware, and FTS5 across two attached DBs is especially awkward.
- **B. Materialized merge cache (adopted).** Maintain a read-only `merged/data.sqlite3` = base ⊕ overlay, rebuilt incrementally when either changes. **Reads hit `merged`; writes hit `overlay`.**

B keeps the existing read path **single-DB and untouched** — no rewrite of the query sites — at the cost of a rebuild step and some cache storage. It also makes "merge multi-layer into one" (todo item 9) nearly free: the merged cache *is* the flattened dataset; a squash is "copy `merged` into a standalone writable dataset, drop the overlay and its `download/` dependency, set `role: flat`."

### 5.1 Merge algorithm (rows)

Overlay-wins by uuid, tombstone-shadows deletes:

1. Start from every base row.
2. For each overlay row with the same uuid: overlay replaces base (later `version_created` / `edit_time` wins, consistent with [sync.md §3.4](./sync.md)).
3. For each tombstone in the overlay: drop the matching base row (the whiteout).
4. Overlay-only rows (new uuids) are added.
5. `merged` is stamped with the `base_version` it was built against, so a stale cache is detectable.

Files merge the same way by relative path: `overlay/<path>` shadows `base/<path>`; a deletion marker in the overlay removes a base file from the merged view. `merged/media/` should hardlink/reflink into base+overlay rather than copy, to avoid duplicating multi-GB audio.

### 5.2 Rebuild triggers

Rebuild `merged` when the overlay is written or the base is re-pulled. Rebuild should be incremental (only the touched uuids/paths), not a full rescan, so a single cue edit does not re-merge a whole audiobook.

---

## 6. Write strategy: route to overlay, never to base

Read-only is enforced by the **write path**, not a filesystem lock: a base under the download root is simply never a write target. When a write command resolves to a path under a read-only root:

1. If no overlay exists, create one in the writable area for the same uuid (`layer { role: "overlay", base_uuid, base_version }` copied from the current base).
2. Apply the write to `overlay/data.sqlite3` (and copy any new media file into `overlay/media/`).
3. Rebuild `merged` (§5.2).

The user never hits a "read-only" wall — editing a downloaded dataset transparently starts a layer. Discovery resolves the uuid to the overlay once it exists (§3), so every read sees base ⊕ overlay through `merged/` without the read path knowing about layers.

---

## 7. Base update safety (todo items 4–6)

The app owns the download root, so an update is a managed re-download:

1. Compare the online version against the base's `updated_at`/hash; if newer, pull the new base into `download/<type>/<uuid>/` with an atomic swap — close/reopen SQLite handles and drop WAL/SHM around the swap (already called out in [sync.md §7](./sync.md)).
2. The overlay lives in the writable area and is untouched → local changes survive.
3. Rebuild `merged`. If the new base changed a row the overlay *also* changed, overlay wins (the contract). Surface "base updated under your edit" as a non-blocking notice.
4. Bump the overlay's `base_version`; a `merged` stamped with an older `base_version` is stale and must be rebuilt.

---

## 8. Interaction with sync

A layered dataset crosses **two independent channels**, and they must not be conflated:

- **Base ← online server.** The base is downloaded and re-pulled from the *online catalog* (user A's upload), which is the account-tied server described in [../idea.md](../idea.md#L212-L214) — **not** the LAN hub-and-spoke service in [sync.md](./sync.md). This channel is pull-only and read-only for B; the app owns it end to end (§4.2, §7). `sharing.visibility` + `subscribers` model who may see it.
- **Overlay ↔ the user's own devices.** The overlay is B's writable data. If B has several devices, it syncs across them through the *existing* hub machinery ([sync.md](./sync.md)) as an ordinary writable dataset — or simply stays local. It is never pushed back to the online server or to A.

Two consequences worth designing deliberately:

1. **The sync unit is no longer one directory.** sync.md §3.2 says "dataset stays the unit (matches the directory layout — do not change it)"; layering splits it into "base dir (online, read-only) + overlay dir (own-device, writable)." The overlay can ride the existing per-directory sync unchanged; only the base needs the new online download/update channel.
2. **The online catalog is a new dependency.** None of it exists yet — today sharing is LAN-only. Phase 1–3 (§10) work against a manually populated `download/` so the layering can be built and tested before the server exists; phase 4 wires up the real upload/download.

This is the largest open design question and should be resolved against sync.md and the future catalog before phase 4.

---

## 9. Explicitly out of scope (v1)

- **Cross-dataset media dedup / a true content-addressed store.** Overlay-by-relative-path is enough; a shared CAS is a separate, larger project.
- **File-level merge.** Media is content-addressed and immutable — nobody edits an mp3 in place (sync.md §3.4). Overlay shadows whole files.
- **Multi-publisher / nested base chains.** One base + one overlay. Docker-style N-layer stacks are not needed for sharing.
- **CRDTs / per-field merge.** Overlay-wins by row is the whole conflict model.
- **Protecting a base the user moves out of `download/`.** Read-only is guaranteed only for what the app manages inside the download root; a manually relocated base is an ordinary writable dataset (§4.1).

---

## 10. Build order

Each phase is independently shippable and keeps the app working:

1. **Download root + read-only-by-location + files-only overlay.** Add the read-only location (§4.2), union `media/`/`subtitle/` overlay-first, and add the `layer` block (§4.3) so editing a base grows a file overlay in the writable area. No DB layering yet. Already useful: "download a shared dataset, add my own media, don't touch the original."
2. **Row overlay via the materialized merge cache (§5).** `merged/data.sqlite3` = base ⊕ overlay, tombstone shadowing, incremental rebuild, writes routed to overlay (§6). Reads keep hitting a single DB — existing queries unchanged.
3. **Flatten/merge command (todo item 9) + base re-pull (§7).**
4. **Wire into online sharing (todo items 1, 11):** publish a base, subscribe → auto-create the read-only base layer; resolve the "sync unit is now two dirs" question (§8) against sync.md.

**Ordering note:** land phase 1 before phase 2 — the file-union and the read-only rule are cheap and hard to retrofit once overlays exist, and phase 2's merge cache assumes them.

---

## 11. MCP surface

Per [../../AGENTS.md](../../AGENTS.md), every user-facing action needs an MCP twin. New tools (names unique across all submodules — `ToolRouter::merge` silently overwrites duplicates):

- `dataset_download` — pull an online dataset into the read-only download root.
- `dataset_update_check` / `dataset_update` — compare the online version and re-pull the base (§7); non-destructive to the overlay.
- `dataset_layer_status` — role, base_uuid, base_version, overlay dirty?, merged stale? (discovery/status before acting).
- `dataset_layer_flatten` — merge overlay into a standalone writable dataset → `role: flat`, no longer dependent on `download/` (todo item 9).
- `dataset_layer_detach` — drop the overlay, revert to the pristine base.

All return structured JSON with actionable errors (e.g. "cannot write: dataset is a read-only base under the download root; an overlay was created at `<path>`").

---

## 12. Impact on existing code

The change is **deep but narrow**: it concentrates at the directory-resolution seam and the write path, not spread across the 121 dataset commands. Counts below are from the current tree.

### 12.1 Why the blast radius is small

1. **One choke point.** Everything resolves a dataset through `find_dataset_dir*` — **88 references** — and the ~**65** `data.sqlite3` path builds almost all follow `let dir = find_dataset_dir(…)?; let db_path = dir.join("data.sqlite3")`. If the *resolver* becomes layer-aware and returns the right directory, those DB-path sites and the query bodies behind them are untouched.
2. **The merged cache (§5) saves the read path.** Reads hit a single `merged/` dir (DB *and* media via hardlink/reflink), so the frontend's `convertFileSrc(dir + relativePath)` and every existing SQL query keep working against one directory. Query-time union — the rejected alternative — would have forced layer logic into all 88 sites.
3. **Backward-compatible by default.** A flat local dataset (no base, no overlay) resolves to *exactly the same directory as today*; the new branches activate only when a `download/` base or an overlay exists. **No migration for existing datasets.**

### 12.2 Affected areas

| Area | Impact | Why |
|---|---|---|
| `dataset_roots` / `DatasetLinkedDir` / `DatasetDirEntry` | Low (additive) | Append `download/<type>` last + a `read_only` / `is_read_only` flag |
| `info.json` `layer` block | Very low | Round-trips through the existing `extra` flatten map; no reader breaks |
| `find_dataset_dir*` (88 refs) | **Medium — the real work** | Split into read-resolution (→ `merged/` or flat) vs write-resolution (→ `overlay/`, creating it if the target is a read-only base). Call sites get *classified*, not rewritten |
| `dataset_list` + selectors | Medium | Dedupe by uuid (§4.4) + `downloaded` / `has-local-changes` badges |
| `assert_uuid_free` + overlay creation | Low | Already scans all roots; overlay creation needs the `ensure_favorites_dataset`-style exemption |
| Write commands (subset of 121 dataset cmds / 165 MCP tools) | Medium | Route through the write-resolver; commands that build media paths or write files directly need auditing |
| `open_db` (3 defs: `db.rs`, `dictation/mod.rs`, `read_aloud.rs`) | Low | Point at the resolved dir; centralizing helps |
| Online download/update channel | Low (isolated) | New module; does not touch LAN sync until phase 4 |
| Frontend | Low | Selector badges + hide `download/` from "add location"; dictation/cards pages unchanged |

### 12.3 The resolver split (phase 1 starting point)

The single most important discipline is a clean read/write split. Proposed signatures beside today's `find_dataset_dir`:

```rust
/// Read target: the effective directory a reader opens.
/// overlay present → its `merged/`; else the base under `download/`; else the flat dir.
/// For a flat local dataset this returns exactly what `find_dataset_dir` returns today.
pub(crate) fn find_dataset_dir_read(settings: &SettingsState, uuid: &str)
    -> Result<PathBuf, String>;

/// Write target: where a mutation lands, creating an overlay on first write to a base.
/// read-only base → ensure `<type>/<uuid>/overlay/` exists (exempt from `assert_uuid_free`,
///   validated by `layer.base_uuid` resolving to a real base) and return it;
/// else the flat/overlay dir, exactly as today.
pub(crate) fn find_dataset_dir_write(settings: &SettingsState, uuid: &str)
    -> Result<PathBuf, String>;
```

Migration is mechanical: each of the 88 call sites is tagged read or write and switched to the matching resolver. `find_dataset_dir` can remain as a thin alias for the read resolver during the transition so the tree keeps compiling site-by-site.

### 12.4 Where bugs will hide

- **Read/write misclassification.** A write resolving to `merged/` is lost; a read resolving to `overlay/` misses base content. This is the primary bug surface and needs reviewed coverage of all 88 sites.
- **Media/file writes.** Reads resolve through `merged/` fine, but a *new* media/subtitle/waveform file must land in `overlay/media/`, never the read-only base — every command that writes files directly needs the write-resolver.
- **Mobile writeback.** Overlay writes on Android must still enqueue to the writeback queue (`#[cfg(not(feature = "desktop"))]`); the resolver change must preserve that path.
- **Audit / report / workflow.** These read dataset dirs and will see `merged/`; verify their existence checks (e.g. "does `data.sqlite3` exist") against the layered shape.

### 12.5 Effort by phase

- **Phase 1** is the only phase that touches shared code broadly (resolver split, `dataset_roots`, `dataset_list` dedup, `assert_uuid_free`). It carries the bulk of the risk and should land carefully; flat local datasets stay untouched throughout.
- **Phase 2** is mostly new code (the merge builder) plus repointing `open_db`; existing queries unchanged.
- **Phases 3–4** are additive.
