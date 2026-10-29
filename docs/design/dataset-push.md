# Dataset push (follower → hub)

A follower that pulled a dataset from the LAN hub can *author* it: import
media, cut cues, fix cue times, add cards. Today that work only lives on the
follower. `dataset_push_to_hub` ships the whole dataset directory back to the
hub, so the hub's copy (and every other follower mirroring it) becomes the
newer one.

The feature is deliberately **not** a second sync protocol. It is one more
write on the existing `/api/v1` surface, behind the existing trust zones, with
one extra advertised capability (`dataset_ingest`), and it reuses the existing
file plane: `manifest` → per-file `sha256` diff → `/file` in reverse.

## Why the DB travels

Everywhere else `data.sqlite3` is excluded from the content plane
(`sync::rest::is_db_file`): the row journal owns rows, so a single edited cue
must not force a whole-DB download. A push is the one case where the DB **is**
the authored content — hand-adjusted cue times exist nowhere else — so it
crosses on its own route, frozen with the SQLite backup API (the same
`freeze_db` the website upload uses). Regenerating the DB from the VTTs on the
hub would silently discard that work.

## Wire contract

All routes are under `/api/v1`, signed by the device key like every other
`/api/v1` call (`x-fms-device`/`-ts`/`-sig`). The two `PUT` routes stream a
body that is **not** part of the signature: the client sends `x-fms-stream: 1`
and puts the content digest in the query, which *is* signed. `zone_guard`
leaves such a body alone instead of buffering it (multi-GB media).

### `POST /datasets/{uuid}/ingest` — declare

```jsonc
{
  "run": "3f9a…",             // client-generated, [A-Za-z0-9]{1,40}
  "dataset_type": "dictation",
  "files": [{ "path": "subtitle/a.vtt", "sha256": "…", "size": 4021 }],
  "deletes": ["waveform/old.json"],   // paths the local tree no longer has
  "db": { "path": "data.sqlite3", "sha256": "…", "size": 9_830_400 }
}
```

Response:

```jsonc
{
  "run": "3f9a…",
  "need": ["subtitle/a.vtt"],   // declared paths the hub does not already have
  "need_db": true,
  "creates": 1, "updates": 0, "deletes": 1, "unchanged": 214,
  "upload_bytes": 4021,
  "content_gen": 7,             // hub revision the client must echo at commit
  "hub_file_count": 215,
  "staging": ".fms-incoming/<uuid>__3f9a…"
}
```

Declare is what makes the push cheap: a media file already on the hub is never
uploaded again, because its `sha256` matches the hub manifest entry.

Side effects: a staging tree is built at
`<dataset type root>/.fms-incoming/<uuid>__<run>/tree/`, and every hub file
the push does not replace or delete is **hard-linked** into it (copy as a
fallback) — so the merge costs directory entries, not bytes, and
`rename` can install it atomically on the same volume.

### `PUT /datasets/{uuid}/ingest/file?run=…&path=…&sha=…`

Raw bytes. `content-type: application/octet-stream`, `x-fms-stream: 1`.
The hub streams them to `staging/tree/<path>`, hashing as it goes, and answers
`409` when the digest does not match `sha` (the file is removed).

### `PUT /datasets/{uuid}/ingest/db?run=…&sha=…`

Same shape for the frozen `data.sqlite3`.

### `POST /datasets/{uuid}/ingest/commit`

```jsonc
{ "run": "3f9a…", "content_gen": 7 }
```

The hub re-verifies every declared path's digest in the staged tree, requires
that the staged `info.json` parses, carries this uuid and `dataset_type`, and
declares the same type the push was opened for. Then it:

1. moves into the app trash (`datasets::media::move_to_trash`) exactly what the
   push **destroys** — each live file the push replaces or deletes, plus the old
   `data.sqlite3` when a DB rides along. Files carried forward by hard link are
   not trashed: that would duplicate bytes to no purpose;
2. renames the live dataset dir aside, renames `staging/tree` into its place,
   and deletes the aside copy (restoring it if the second rename fails);
3. drops the staging run;
4. bumps `content_gen` in the app DB;
5. answers `{ content_gen, overall_hash, file_count, total_bytes, trashed_to,
   created, dataset_type }` — `trashed_to` lists one entry per trashed path.

The client stores `overall_hash` as its new baseline, so the next round sees no
drift. `409` on a `content_gen` mismatch — someone else committed between this
client's declare and its commit; the client re-runs the push (it is idempotent).

### `GET /datasets/{uuid}/db`

A point-in-time copy (`VACUUM INTO`) of the hub DB with
`x-content-sha256` and `x-content-gen` headers. This is the read side of a
push: `manifest.content_gen` rising is how a *follower* learns that a whole-DB
replacement happened, and this route delivers it.

## Invariants

* **The hub's read-only base is never written.** A dataset under
  `download/` (`datasets::is_under_download_root`) refuses the push with an
  actionable error: fork it into a writable root first. Same rule the
  incremental refresh and `dataset_evict` already enforce.
* **Never a partial dataset.** Bytes land in staging; the live dir is replaced
  by two renames. A failed push leaves the hub's copy exactly as it was.
* **Never a silent overwrite of newer hub work.** The client compares the hub
  manifest's `overall_hash` with the baseline it recorded when it last synced
  (`dataset_sync_state.overall_hash`). A mismatch aborts with
  `hub_changed` unless the caller passes `allow_hub_changes: true`. There is no
  merge — a push is "my copy becomes the hub's copy", and saying so is the
  caller's job.
* **Backup before destroy.** Every path the commit overwrites or removes —
  including the old DB — goes to the app trash before the install, so
  `app_trash_list`/`app_restore` can put it back. Unchanged files are hard-linked
  into the new tree instead, so a push of a 2 GB dataset that edits one cue
  cannot fill the trash with a second copy of the media.
* **One commit per dataset at a time** (process-local guard) — two pushes
  cannot interleave their renames.
* **The DB is not content.** It never appears in `files`/`deletes` (the hub
  refuses those names), and it never appears in `overall_hash`, so a push that
  only changes cue times leaves the file manifest unchanged — which is exactly
  why `content_gen` exists.
* **Capability, not version.** `dataset_ingest` is advertised in
  `HUB_FEATURES`; it is additive and negotiated, so `PROTOCOL_VERSION` stays 6
  (INV-20: a client refuses an unadvertised capability, never probes for it).
* **LAN only.** The website hub has its own upload path
  (`hub_dataset_upload`, OAuth bearer, versioned registry). Pushing a live
  directory at the website is refused with a pointer to it.
* **An overlay travels as a standalone dataset.** The headline case — pull a
  dataset, build it here, push it back — is a copy that the first local write
  forked into an overlay (`datasets/paths.rs`), so `info.json` carries
  `layer.role = "overlay"` and the base's uuid. That block describes this
  device's parentage, not the dataset, so the push flattens the descriptor
  before hashing (the same write `layer_flatten` performs). Normalising on the
  hub instead would leave the two copies permanently one `info.json` apart, and
  every later diff would see drift that never resolves.

## What a follower sees after a push

`check_file_drift` already detects non-DB drift for every mirrored dataset and
reconciles it file by file. A DB-only push leaves the file manifest untouched,
so the round additionally compares `manifest.content_gen` with the
`dataset_content_state` row it recorded last time; when the hub's is higher it
fetches `GET /datasets/{uuid}/db` and installs it atomically. Row-journaled cue
edits never trigger it (their `content_gen` does not move), so the cheap path
stays cheap.

## Surfaces

| Surface | Where |
| ------- | ----- |
| `dataset_push_preview` / `dataset_push_to_hub` / `dataset_push_all` | Tauri commands (`sync/client/push.rs`), registered in both `lib.rs` handler lists |
| Same three, as MCP tools | `mcp/sync.rs`; `dataset_push_preview` is `READ_TOOLS`, the two writes stay `Tier::Write` (everything they replace is trashed) |
| "Push" on a dataset card, and "Push all" in the toolbar | `DatasetsSyncPage.tsx` (follower with a LAN hub only) |
| Progress | `dataset-sync-progress` with `kind: "push"` — the download bar filters it out, so the two planes share one event name without colliding |

Every write path reports `hub_changed` as `Err("hub_changed: …")` rather than a
boolean, so an agent that ignored the preview still gets a refusal it can act
on; `dataset_push_all` never passes `allow_hub_changes`, because a batch cannot
collect a per-dataset consent.
