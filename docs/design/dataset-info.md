# The `info.json` descriptor

One shape describes every dataset type. This document is the contract; the code that
enforces it is [`src-tauri/src/datasets/info.rs`](../../src-tauri/src/datasets/info.rs),
and the machine-readable form is
[`info.schema.json`](./info.schema.json) (JSON Schema 2020-12).

It **supersedes the `info.json` sections of** [../dataset.md](../dataset.md) and
[../dataset-schema.md](../dataset-schema.md). Those files stay as they are and still
describe the per-type content models correctly — only their descriptor sections are
out of date.

## Why one shape

Five structs used to own this file: `DatasetInfo` (dictation), `CardDatasetInfo`,
`BookInfoFile`, `ReadAloudInfoFile`, `WikiDatasetInfo`. They disagreed on the *name*
of the same concept — `updated` versus `updated_at`, `name` versus `title` — and on
the spelling of the layout tag (`cards-v1`, `reading-v1`, `read-aloud-v1`,
`dictation-v1/v2`, `wiki-v1`). The consequences were concrete, not aesthetic:

- the sync manifest read only `v["updated"]`, so **every book and read-aloud dataset
  advertised an empty timestamp**;
- every reader needed a type-specific parse plus a fallback, and
  `find_card_dataset_dir` parsed the file twice;
- `touch_info` re-serialized from a struct, so **any field it did not model was
  destroyed on the next cue adjust**;
- nothing checked uuid uniqueness, and two datasets of different types were found
  sharing one uuid — fatal, because uuid is the key of the whole REST protocol;
- three timestamp formats coexisted in real files (`…35Z`, `…519+00:00`,
  `…524600+00:00`), so no string comparison of stamps was reliable.

## The fields

| field | type | who reads it |
|---|---|---|
| `spec` | number (`2`) | Tells a reader which contract the file was written against. Bump only on a removal or a change of meaning — a new field is invisible to old readers thanks to `extra`. |
| `uuid` | string | Every lookup, the sync journal, the REST routes (`/api/v1/datasets/{uuid}/…`), `assert_uuid_free`. **Opaque**: unique per dataset and nothing more, never format-validated — the reserved Favorites id and the website-era ids (`de_a1_by_system`) are slugs, not RFC 4122 UUIDs. |
| `type` | slug | `find_dataset_dir_typed` asserts it against the directory the dataset was found under, and warns + skips on mismatch. The directory stays the routing authority; `type` only makes the file self-describing for a reader that has the file and nothing else. |
| `format` | `"<type-slug>-v<N>"` | The per-type internal layout: `cards/mod.rs` gates on `format == "card-v1"`. Split out of the old `structure` so `type` can be the directory verbatim while `format` still carries a version. |
| `name` | string | Every listing and every title in the UI. Books used to call this `title`; the file, `BookMeta`, `book_create` and `book_rename` all say `name` now. |
| `description` | string | Human-facing blurb, shown under the dataset name. Provenance does not belong here — that is what a `README.md` inside the dataset is for. |
| `language` | string (`""` when unknown) | Typed, not a label, because code branches on it: `simple_words` maps a language to the card datasets that count for it. BCP 47 primary subtag (`de`, `en`, `vi`). |
| `created_at` | stamp | Written once. `touch_info` never touches it, so a subtitle regeneration does not pretend the dataset is younger than it is. |
| `updated_at` | stamp | `sync::rest::compute_manifest` ships it, and every mutation path bumps it through `touch_info`. The only field that is expected to move. |
| `sharing` | object | `visibility` (default `"private"`) drives the badges and the website's catalog; `owner_id` gates pushes (`card_sync_all` refuses anything whose owner is not the logged-in user); `subscribers` is a **mirror** — local code reads it and never appends, because appending would dirty the dataset for every follower. |

Both stamps are `now_stamp()`: UTC, second precision, `Z` suffix
(`2026-10-01T18:18:18Z`). Anything else is a migration artifact.

Anything this build does not model is preserved verbatim in a flattened `extra` map
and written back untouched. That is what makes adding a field a pure addition and
`touch_info` safe.

## File name and the constant

The file stays `info.json`. `meta.json` was rejected — that name is already taken at
the *root* level by the `linked_dirs` registry, and a rename would break every
external generator for no functional gain. All path construction goes through
`INFO_FILE`, so a future rename is a one-line change.

## Favorites is a reserved id, not a flag

`is_favorites: true` is gone. The Favorites dataset is identified by
`FAVORITES_DATASET_UUID = "dictation-favorites"`:

- `find_favorites_dataset` is an ordinary uuid lookup (`None` on a read path never
  creates anything); `ensure_favorites_dataset` mints the skeleton with exactly this
  uuid and skips `assert_uuid_free` for it.
- Consequence, intended: PC and phone Favorites carry the same id, so they **merge
  under sync** — a favorited clip is one clip, not one clip per device.
- `dataset_create` and import reject `"dictation-favorites"` as a user-chosen id, and
  the UI (`DictationPage`) disables "add to Favorites" by comparing the selected
  dataset's uuid, not a boolean.

## `sync_url` is gone

Card sync used to read a base URL out of each dataset's `info.json` and send the
OAuth bearer token to it. That means an imported dataset could redirect your token
off-box. Pull and push now build their URLs from `auth::BASE_URL`
(`src-tauri/src/auth.rs`) — the same origin login is verified against, and one
constant rather than per-dataset data. There is deliberately **no** new setting and
no Settings UI field: relocating the origin stays a one-line code edit.

## Format tags

| type | `format` | was (`structure`) |
|---|---|---|
| `dictation` | `dictation-v2` | `dictation-v1`, `dictation-v2` |
| `card` | `card-v1` | `cards-v1` |
| `book` | `book-v1` | `reading-v1` |
| `read_aloud` | `read_aloud-v1` | `read-aloud-v1` |
| `wiki` | `wiki-v1` | `wiki-v1` |

The rule is the directory slug verbatim — `card`, not `cards`; `read_aloud`, not
`read-aloud` — so `type` and `format` can never contradict the tree on disk.

## Old → new mapping

For whoever migrates a dataset (there is **no compatibility reader and no automatic
migration**: `PROTOCOL_VERSION` went 4 → 5 so a mixed-version cluster refuses each
other rather than silently mis-parsing):

| old | new |
|---|---|
| `structure` | split into `type` + `format` (see the table above) |
| `updated` / `updated_at` | `updated_at`, re-stamped in the `…Z` second-precision form |
| — | `created_at` (was dropped for books and read-aloud): synthesize from the oldest known stamp, or set to migration time |
| `title` (book) | `name` |
| `version` | dropped — it duplicated `structure`, and `spec` + `format` say the same thing with clearer semantics |
| `is_favorites: true` | dropped; that dataset's `uuid` becomes `"dictation-favorites"` |
| `sync_url` | dropped; card sync reads `auth::BASE_URL` |
| `parent_uuid` | dropped — discovery is one level deep per type, and grouping lives on the website catalog |
| `visibility` / `owner_id` / `subscribers` | `sharing.visibility` / `sharing.owner_id` / `sharing.subscribers`, `visibility` defaulting to `"private"` |
| `labels` | dropped — no reader, and it collides with a card's own tags |
| `source` / `origin` / `generator` / `data` | dropped — nothing read them; provenance belongs in a `README.md` |
| `language` | new: fill from the dataset name convention (`de_a1`, `word_en`) where obvious, else `""` |

## Out of scope

`learning/dataset_studio` (`scripts/gen_read_aloud.py`,
`scripts_admin/export_dataset.py`, `scripts_admin/export_cards.py`,
`lib/dataset.py`) was not touched. Recorded consequence: those scripts keep emitting
the pre-migration shape, so a dataset they generate is invisible to the typed parser
until they are updated against [`info.schema.json`](./info.schema.json).

## MCP surface

Because one schema describes every type, the per-type split in the tool surface
became arbitrary:

- `dataset_info_get` / `dataset_info_update` work on any dataset of any type. The
  `sharing` fields are settable; `subscribers` is not.
- `dataset_list` returns the core fields plus `path`, `location` and `status` for
  every type, so an agent needs no per-type schema.
- `dataset_update` and `card_dataset_update` are gone, collapsed into
  `dataset_info_update`. Tool names stay globally unique — `ToolRouter::merge`
  silently overwrites duplicates.
