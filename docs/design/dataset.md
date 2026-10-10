# Datasets

## What a dataset is

A dataset is one folder on the filesystem that contains everything needed for one body of learning material: the audio, the transcripts of that audio, and the structured data derived from both. There is no central registry, no import step into an application database, and no per-device state inside the folder. A dataset is complete in itself, which is precisely why it can be zipped, copied to a USB stick, moved between machines, or shipped over the network to a follower device without any coordination beyond the filesystem.

Five kinds exist, and each is a module under `src-tauri/src/datasets/` that mirrors its on-disk root: dictation, card, book, read-aloud, wiki. They share the discovery, identity, and transport machinery but not the content model.

## Design principles

- **The folder is the object.** Reading, writing, syncing, and deleting all operate on a directory. Nothing worth keeping lives outside it.
- **Identity is the uuid in `info.json`, never the path.** The same dataset can sit in different places on different machines; every lookup resolves a uuid by scanning roots, so a rename or a move is free.
- **Derived data is disposable.** Waveforms, sentence-split caches, full-text indexes, and the cue rows themselves can all be regenerated from the authored files. Losing them costs time, not information.
- **Files and rows move by different mechanisms.** Media and text files move as content hashes and snapshot tarballs; edits inside `data.sqlite3` move as journal rows. The transport excludes the database from hashing exactly because the journal owns it.
- **Progress is not content.** Where a user got to with a dataset lives in the app database, not the dataset, so the same dataset folder can be shared by several users and several devices without dragging private state along.
- **One module per type, one root per module.** A new dataset type means a new submodule plus a new `DatasetType` variant, and it becomes syncable the moment `find_dataset_dir_typed` can resolve it.

## Where datasets live

The base directory is workspace-scoped: with a workspace selected, everything derives from `<data_root>/fms-app/workspaces/<ws-uuid>/`; without one it falls back to the configured value or the app-data default. Datasets never mix across workspaces.

```
<workspace>/
├── app.sqlite3          # progress, XP, sync state, pairing — see below
├── datasets/
│   ├── dictation/
│   │   ├── meta.json        # only when linked locations are configured
│   │   └── <dataset dir>/
│   ├── card/
│   │   ├── meta.json
│   │   ├── fts5.sqlite3     # FTS index over every card dataset in this location
│   │   └── <dataset dir>/
│   ├── book/
│   ├── read_aloud/
│   └── wiki/
└── recordings/
```

Each type root is a location that can carry additional linked directories, recorded in that root's `meta.json` as `linked_dirs: [{ name, path }]` and managed through `dataset_list_dirs` / `dataset_add_dir` / `dataset_remove_dir`. This is how a user keeps a big dictation library on an external drive while the workspace stays on the SSD. `dataset_roots(settings, type)` returns the type root plus every linked directory that still exists, and every scan and lookup iterates that list, so linked locations are first-class rather than a special case.

Synced copies always land under the *default* type root, named after the uuid, regardless of where the hub's own copy sits.

Deleting a dataset never destroys it. `dataset_delete`, `card_dataset_delete`, `book_delete`, `read_aloud_delete` and the wiki equivalent all move the whole directory into `<data_dir>/fms-app/trash/<timestamp>_<name>` through the shared `datasets::move_to_trash`, the rule wiki datasets and workspaces followed first. The four dataset-level deletes return the trash path (as `trashed_to` over MCP) so the caller can say where the data went; an agent is allowed to delete without a confirmation prompt precisely because this exists. The fast path is a rename; because a linked location can sit on another volume than the app-data dir, where Windows refuses to rename, a failed rename copies to a `.partial` path and publishes it only after the whole tree is written — so a failed backup leaves the dataset untouched and can never be mistaken for a completed one. Same-second deletions of same-named folders get a counter suffix rather than colliding.

This covers dataset directories only. Deletes *inside* a dataset (a book chapter, a read-aloud attempt, a media file, the generated subtitles or database) still remove files directly: they are row-scoped operations whose data is either regenerable or already backed by the sync row log, and trashing every one of them would fill the trash with fragments. The remaining gap is listed under "Known rough edges".

## Identity: info.json

Each dataset directory carries an `info.json`, and **one shape describes all five types**: `DatasetInfo` in `src-tauri/src/datasets/info.rs`, at `spec` 2. [`dataset-info.md`](./dataset-info.md) is the contract for that file — the fields and who reads each one, the format tags, the old → new migration mapping, and the reason `sync_url` had to be removed — with [`info.schema.json`](./info.schema.json) as its machine-readable form. None of that is repeated here on purpose. Five per-type structs disagreeing about the *name* of the same concept (`updated` vs `updated_at`, `name` vs `title`) is the disease the unification cured, and a second copy of the field table in this file is the same drift waiting to recur.

What this document does own is the effect the descriptor has on the dataset model:

- **The file is hashed, so it stays declarative.** `info.json` is part of the sync manifest's hashed file set, which is why counts, scores and progress live in `data.sqlite3` instead: a field that moved on every user action would re-hash the dataset and look like content churn to every follower. `updated_at` is the one field allowed to move.
- **`type` is enforced, not just recorded.** `find_dataset_dir_typed` logs and *skips* any folder whose `type` disagrees with the root it was found under. A mistyped dataset is therefore not merely mislabeled — it vanishes from every uuid lookup, which means from `/manifest`, `/snapshot`, `/changes` and `/file` alike. The directory stays the routing authority; the field exists to make the file self-describing to a reader holding only the file.
- **A legacy descriptor is refused, not half-read.** There is no compatibility reader and no automatic migration. `type` and `format` are required with no defaults, so a pre-unification file fails to parse, `read_info_opt` returns `None`, and the folder drops out of discovery as if it did not exist. `PROTOCOL_VERSION` went 4 → 5 with it, so a mixed-version cluster refuses each other rather than silently mis-parsing. Migrating the files on disk is therefore what makes a dataset visible at all — and the contract's *Out of scope* section records which generators are still emitting the old shape.
- **A folder with `media/` but no `info.json` is still listed** (a raw import) under a synthesized dictation descriptor with an empty uuid and `status: not_ready`. It can be browsed, but nothing that resolves a uuid — the database commands, the MCP tools, the sync transport — can address it, and the hub's catalog endpoint skips every uuid-less entry rather than advertising dead rows.
- **Directory naming is not stable across origins.** A hub's own book dataset may live in `readable-book-title/` while the follower's pulled copy lives in `<uuid>/`. Both resolve, because resolution opens every `info.json` and compares uuids. Never build a path from a uuid by hand; call `find_dataset_dir` / `find_dataset_dir_typed`.

## The five types

| Type | Root dir | `format` | Content lives in | Extra files |
|------|----------|--------|------------------|-------------|
| Dictation | `dictation/` | `dictation-v2` | `data.sqlite3` + `media/`, `subtitle/`, `waveform/` | `book.txt`, `book_sentences.txt`, `transcript/` |
| Card | `card/` | `card-v1` | `data.sqlite3` (`card`, `card_review`, `tag`, `card_tag`) | `fts5.sqlite3` at the location root |
| Book | `book/` | `book-v1` | `data.sqlite3` (`book_chapter`, `book_sentence`, `book_sentence_word`) | `media/` for per-sentence audio |
| Read aloud | `read_aloud/` | `read_aloud-v1` | `data.sqlite3` (`read_text`, `read_attempt`) | `media/` for recorded takes |
| Wiki | `wiki/` | `wiki-v1` | the `.md` files themselves — no `data.sqlite3` | `fts5.sqlite3` (derived index) |

Card, book, and read-aloud open their `data.sqlite3` through an initializer that re-applies `CREATE TABLE IF NOT EXISTS` on every call, so a folder copied in from an older build heals itself the first time it is touched. Dictation is the exception: its `open_db` refuses a missing database with "generate the database first" and never creates schema, because for that type the database is a pipeline artifact rather than something to be lazily conjured.

## Per-type layout

### Dictation

The full working set, after the pipeline has run:

```
<dataset>/
├── info.json
├── data.sqlite3
├── media/         audio/video, sub-directories allowed and mirrored
├── subtitle/      one .vtt per media, same relative path with a new extension
├── waveform/      one .json per media, same mirroring
├── transcript/    optional .txt per media, imported into listen_transcript
├── book.txt       optional reference text for alignment
└── book_sentences.txt   generated sentence split of book.txt
```

`media/` may contain any nesting; `subtitle/`, `waveform/`, and `transcript/` mirror the media's path relative to `media/` (`A1.1/Lektion_6.mp3` → `A1.1/Lektion_6.vtt`), which is what `listen_media.source` stores: the media-relative path, sub-directories included. Recognised extensions are `mp3 wav flac ogg m4a m4b aac opus` plus the video containers `mp4 m4v mov mkv webm`, of which only the audio track is decoded.

Book and read-aloud datasets use a much smaller shape — `info.json`, `data.sqlite3`, `media/` — and store audio as a path relative to the **dataset directory** (`media/foo.wav`), unlike dictation, which stores it relative to `media/`. Wiki is a free-form tree of `.md` files (plus images and media that the viewer streams) with the derived `fts5.sqlite3` beside them.

### Path conventions

Two different bases exist and mixing them up is the classic bug in this area:

| Value | Relative to | Used by |
|-------|-------------|---------|
| `listen_media.source` | `media/` | dictation playback, sibling subtitle/waveform/transcript lookup, favorite clips |
| `audio_path` | dataset directory (`media/...`) | book sentences, read-aloud attempts |
| `rel_path` | dataset directory, forward slashes | wiki files, and the wiki REST browse responses |

The Favorites dataset is an ordinary dictation dataset and obeys the dictation convention: its clip rows store the bare file name in `listen_media.source`. It is identified by the reserved uuid `dictation-favorites` rather than a flag — the reason and the consequences for sync merging are in [dataset-info.md](./dataset-info.md) — and it reuses the *source cue's* uuid as the primary key of the copied cue row, which is what makes adding the same cue twice a no-op instead of a duplicate.

Wiki paths additionally pass `sanitize_rel`, which rejects absolute paths, any `..` component, and any colon-bearing component, so a request can never escape the dataset directory.

## The dictation database

`data.sqlite3` holds six tables plus the sync tombstones:

| Table | Role |
|-------|------|
| `listen_media` | one row per media file; `source` is the media-relative path |
| `listen_subtitle` | a subtitle *track*: `name`, `track_type` (`stt` / `manual` / `aligned` / `adjusted`), `model_uuid`, `version`, `is_active` |
| `listen_subtitle_cue` | the cues, versioned in place |
| `listen_subtitle_version` | one row per version: `change_type`, description, added/modified/deleted counts, `created_by` |
| `listen_transcript` | per-media transcript text imported from `transcript/` |
| `listen_note` | created by the schema, used by nothing — see rough edges |
| `tombstones` | appended by `ensure_tombstones`; travels with the snapshot |

One media can carry several tracks (different STT models, a manual correction, an aligned variant), and exactly one is `is_active`, which is the track the dictation page practices. Cues use the effective-range versioning pattern rather than whole-track copies: a cue row records `version_created` and, once replaced, `version_superseded`, so the current state is `version_superseded IS NULL`, an old state is reconstructible by range, and re-running STT over unchanged audio duplicates nothing.

Timestamps are text, primary keys are TEXT UUIDs everywhere, and card data uses `deleted_at` for soft deletes rather than removing rows, so a delete can be journalled and can conflict.

Text order is chronological only if every stored value has the same width and the same suffix, so a stamp's *shape* is a storage contract, not a style choice — and there are two shapes for two different jobs. Descriptor stamps (`info.json`) use `now_stamp()`: UTC, second precision, `Z`; they are only displayed and hashed, so second precision is enough. Row stamps use `now_row_stamp()`: UTC, **millisecond** precision, `Z`, because `card.updated_at` decides which side of a conflict wins and is compared two ways that no Rust-side parsing can reconcile — as text inside SQLite (`WHERE updated_at > ?1`) and in Rust. Milliseconds plus `Z` is the one shape the three producers agree on byte for byte: `chrono`, JavaScript's `Date.toISOString()` (what the frontend binds into `updated_at`), and SQLite's `strftime('%Y-%m-%dT%H:%M:%fZ','now')`. `canonical_stamp` re-shapes a value before storing it, and an unparseable one is kept verbatim rather than replaced by a plausible instant — a garbage stamp should stay visibly garbage. Real data used to carry three shapes at once (`…35Z` from Python, `…519+00:00` from Rust with microseconds, `…524600+00:00` with nanoseconds), which made any string comparison of stamps unreliable; `stamps_cmp` is what code compares through now.

## Authored, derived, and shipped

| Artifact | Status | Rebuild | In a sync snapshot? |
|----------|--------|---------|---------------------|
| `media/` files | authored | — | yes, hashed |
| `subtitle/*.vtt` | derived from STT, then hand-editable | regenerate subtitles | yes, hashed |
| `book.txt` | authored | — | yes, hashed |
| `book_sentences.txt` | derived | split book (Rust or bundled NLTK script) | yes, hashed |
| `waveform/*.json` | derived | generate waveforms | yes, hashed |
| `data.sqlite3` | derived *and* edited | build database | shipped whole by snapshot, then maintained by the row log; excluded from hashing |
| `fts5.sqlite3` (wiki) | derived | built on first search | **never** — refused by hash, tar and file read |
| `fts5.sqlite3` (cards) | derived | `card_fts_rebuild` | **never** — normally not even inside a dataset dir, and named in the same predicate if some peer drops one there |
| dictation progress, XP | device/user state | — | lives in `app.sqlite3`, not the dataset |

The rule worth restating: a derived index belongs to the copy it indexes. Shipping one would spend bandwidth on something the receiver can rebuild, and it would put a machine-local artifact into the content hash that the whole sync round is keyed on.

Enforcement is one predicate: `datasets::is_derived_index` is the only place the
exclusion is written down, and the four sites that need it — the manifest hash,
the snapshot tar, the `/file` read guard, the wiki path guard — ask it rather than
carrying their own literal. The owning modules keep their consts
(`wiki::FTS_DB`, `cards::FTS_DB`) because they also *open* those files, and both
now hold the same name: `fts5.sqlite3`. That is safe because a location root is
never a dataset dir, so no tree can contain both indexes. The topology stays
different on purpose: card search spans a whole location, so its index sits above
the datasets; wiki search spans one dir's files, so its index sits beside them.

A third index costs one line in the predicate, not a new literal in four places.
The card index used to be `search.sqlite3`; every location has been moved to the
shared name, so the old one is ordinary content now.

## The processing pipeline

The Studio page exposes the pipeline as ordered stages against one dataset; the backend functions are the same whether called by a human or by an agent.

| Stage | Command | Needs |
|-------|---------|-------|
| Create dataset | `dataset_create` | a location |
| Import media | `dataset_import_media` (copy or symlink) | a source directory |
| Import whole dataset | `dataset_import` | a folder containing `media/` |
| Generate subtitles | `dataset_generate_subtitles` / `_subtitle_single` | media + a loaded STT model |
| Build database | `dataset_generate_database` | media (+ subtitles) |
| Generate waveforms | `dataset_generate_waveform` / `_single` | media (and a database to mirror into) |
| Write subtitles to DB | `dataset_write_subtitles_to_db` | database + VTT files |
| Sync cue times | `dataset_sync_cue_times` | database + VTT files |
| Split book | `dataset_parse_book` | `book.txt` |
| Align cues | `dataset_align_cues` / `dataset_align_cues_transcript` | database + reference text |
| Write transcripts | `dataset_write_transcripts` | `transcript/` + database |
| Adjust cue times | `dataset_adjust_cue_time` | database + waveforms |
| Delete artifacts | `dataset_delete_subtitles` / `_waveforms` / `dataset_delete_database` | — |

Building and running these stages is a desktop activity: the mobile handler list registers only the CRUD and read side of `datasets` (`dataset_list`, `dataset_get`, `dataset_create`, `dataset_update`, `dataset_delete`, and the location commands), so a phone edits and practices a dataset it pulled but never builds one. The gating is uneven in kind — subtitle and waveform generation are compile-time `#[cfg(feature = "desktop")]`, while the database, align, adjust, and book commands compile everywhere and are simply not offered to the mobile frontend. Anything that needs Symphonia, an ONNX model, or Python can only run on the desktop; anything that is plain SQLite work could in principle be exposed there.

Every long-running stage emits `dataset-progress` with the dataset uuid, current file, index, total, and stage name, which is what the Studio rows render. The stage numbering encodes order because the stages are gated on each other's artifacts, not because the data model requires it: `dataset_generate_database` drops and recreates the file, so a subtitle track that was aligned and adjusted by hand must be rebuilt afterwards, not before.

Building the database assigns a fresh uuid to every media file and re-imports cues as version 1 with `track_type = 'stt'`. That is correct for a first build and destructive for a practiced dataset — see rough edges.

## What deliberately lives outside the dataset

`<workspace>/app.sqlite3` holds everything that is about a *user* rather than about the material: `listen_dictation` (per `user_id` + `media_uuid` + `subtitle_uuid`), the XP tables, `sync_log`, `writeback_queue`, `dataset_sync_state`, `app_sync_state`, and the paired-device registry. Because dictation progress is keyed by media and subtitle uuids, it follows the material across copies — a dataset rebuilt on another machine still shows the same rows as done — as long as the uuids survived.

Two consumers reach into datasets without owning them: `simple_words.rs` sources "known simple words" from the selected card datasets, and the XP ledger records the `dataset_uuid` an award was earned against (books reading earn with none).

## Datasets and device sync

Sync is defined entirely in terms of datasets, and the coupling points are few and deliberate:

- `GET /api/v1/datasets` advertises `uuid`, `name`, `updated`, `dataset_type`, `media_count`, `status` for all five types, skipping uuid-less raw imports. `dataset_type` on the wire is the *root directory name* (`dictation` | `card` | `book` | `read_aloud` | `wiki`) — `rest.rs` hardcodes it per type loop rather than reading `info.type` — and it tells a follower which local root to unpack into. The wire key stays `updated` while the file field is `updated_at`, and every type now feeds it from `info.updated_at`; before the descriptor was unified, books and read-aloud reported an empty stamp because the manifest read a key their `info.json` did not have.
- `?lite=1` drops the per-dataset counts, because the incremental round only needs uuid, type, and `updated`. `media_count` means different things per type — media count, card count, text count, markdown file count, and always 0 for books.
- The manifest hashes every file except the databases and the derived full-text indexes, so a row edit inside `data.sqlite3` never forces a whole-file re-download; the row log carries it instead.
- A snapshot is a tar of the whole directory, with `data.sqlite3` replaced by a `VACUUM INTO` copy taken inside a consistent snapshot transaction (`wal_checkpoint` alone would not block concurrent writers and could archive a torn image), and the `-wal`/`-shm` sidecars excluded because the receiver has none. It unpacks to `<datasets>/<type>/<uuid>` through a `.tmp` directory and a rename, so a failed download never leaves a half dataset behind.
- Edits made on a follower queue in `writeback_queue` and are replayed on the hub; `commit_change` is the single choke point, role-aware, and wiki page saves and deletes journal as `wiki_file_save` / `wiki_file_delete` keyed by relative path with last-write-wins.
- `prune_local_dataset` removes a dataset the hub stopped offering by looking in all five type roots — the type list is duplicated there and must be kept in step with `DatasetType`.
- A type is syncable exactly when `find_dataset_dir_typed` resolves it; adding a sixth type means adding it there, to `dataset_roots` callers, and to the prune list.

`PROTOCOL_VERSION` lives in `sync/mod.rs` and is sent as the `x-fms-protocol` header on every request.

## The agent-facing surface

Each domain has a matching MCP submodule (`mcp/datasets.rs`, `mcp/dictation.rs`, `mcp/cards.rs`, `mcp/books.rs`, `mcp/wiki.rs`, `mcp/sync.rs`) registered through the merged tool router, so a Tauri command and its MCP tool are the same code path — the tool wrappers add structured JSON results and emit the list-changed events the UI listens for. The convention in AGENTS.md is that a capability never exists on only one side: adding a command means adding its tool in the same change, because the agent's view of datasets is supposed to be the human's, not a subset of it.

Built-in workflow templates are baked into the binary via `include_str!` under `src-tauri/src/workflow/templates/<category>/*.yaml` — grouped by dataset category (`dictation`, `book`, `card`), since a kind of dataset may grow several workflows — and served to both the Workflow page (one tab per category) and the `workflow_builtin_templates` MCP tool as the single shared source of truth. `templates/dictation/dataset_dictation.yaml` expresses the whole dictation pipeline as a resumable DAG (`ensure_model` → `init_dataset` → `sync_media` → `generate_subtitles` → `generate_waveforms` → `detect_reference` → `align_cues` / `align_cues_transcript` → `adjust_cue_times`), which is the intended way for an agent to run a long, partially failing build: the engine records which stage broke and retries that step rather than the dataset. Only the dictation category is wired for per-step in-app execution today; the book and card templates are view/agent-facing — every step names a real MCP tool (`book_*`, `card_*`) so an agent can drive them even though the page has no Run button for those twins yet.

## Known rough edges

These are real, verified against the code, and each is a candidate for cleanup rather than a hypothetical.

- **`format` carries a version number no code reads.** The tag is `<type-slug>-v<N>`, and dictation is the only type whose number is not `v1` — but nothing branches on it. Its only two readers, `cards::find_card_dataset_dir` and the card listing, compare `format == card-v1` purely as a *type* tag, a job `type` now does properly. So the two discovery paths validate different fields for the same purpose: `find_dataset_dir_typed` checks `type` and skips a mismatch, the card scanner checks `format` and never looks at `type`. The two version numbers in the file are unrelated scales that happen to agree — `spec` versions the descriptor, `format` versions the dataset's internal layout, and both read `2` for dictation.
- **Only whole-dataset deletes are reversible.** `move_to_trash` guards the five dataset directories, but the deletes *inside* a dataset (`book_delete_chapter`, `book_delete_sentence`, `book_delete_audio`, `read_aloud_delete_text`, `read_aloud_delete_attempt`, `listen_delete_media`, `dataset_delete_subtitles`, `dataset_delete_database`) call `fs::remove_file`/`remove_dir_all` outright, while `docs/agent_friendly_design.md` §3 names "clear database" and "overwrite subtitles" among the operations that should be backed up first. The same spec section promises a `restore_from_trash` tool that does not exist yet, so recovering anything from the trash is still a manual filesystem operation.
- **Rebuilding the database orphans progress.** `dataset_generate_database` mints fresh media uuids, while `listen_dictation` keys on `media_uuid`. Practice history recorded before a rebuild no longer matches anything afterwards, and the XP ledger still cites the dataset by uuid, so history and progress diverge.
- **One write-only table.** `listen_note` is created by the schema and touched by nothing. (Waveform data lives solely in the `waveform/*.json` files that `listen_get_waveform` reads; the former `listen_waveform` table was populated alongside them but never read, so it has been dropped from the schema.)
- **`status` means different things.** For dictation it is computed (`ready` iff `data.sqlite3` exists); in the sync catalog the other four types report `ready` unconditionally, so a broken card dataset advertises itself as fine.
- **Two `sync_state` concepts with similar names.** `sync_state` inside a card dataset's database tracks the online card-service link; `dataset_sync_state` in `app.sqlite3` tracks the device hub link. Nothing relates them, and the naming invites confusion.

## Decisions worth keeping

Whatever else changes, these are load-bearing and were chosen on purpose: the folder as the unit of everything; uuid identity resolved by scanning rather than by a registry; derived data rebuildable and never transferred; user state kept out of the dataset; one choke point for journalling edits; and a Tauri command paired with an MCP tool for every capability, so the human path and the agent path cannot diverge.
