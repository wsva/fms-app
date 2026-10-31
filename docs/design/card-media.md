# Card Media Attachments

Images, audio, and video attached to a card's `question`, `answer`, `note`, or
`suggestion` field. This is the layout, the anchor grammar, and the one rule
that makes the rest of the design fall out cleanly.

## The division of truth

**The directory is the truth about bytes; the row is the truth about meaning.
Neither is authoritative for both.**

- Bytes live as ordinary files at `<dataset>/media/<card-uuid>/<original name>`.
  A plain file manager shows every attachment of every card; garbage collection
  is free; and the three existing sync planes already walk the dataset tree, so
  media ships without any new transport.
- A thin `card_media` table (in the dataset's `data.sqlite3`) is the metadata
  index over that directory. It holds what a directory cannot express: the
  anchor handle, the kind, which field it belongs to, the order, and the
  playback behaviour.

Storing media as SQLite BLOBs was rejected: the sync `is_db_file` check would
reject the DB once it carried a 40 MB video, and every card edit would rewrite
the whole file in the row journal. Files keep the dataset legible and the diff
cheap.

Because the two truths can drift (a file with no row, or a row whose bytes have
not landed yet), `card_media_register` reconciles them on editor open. It never
deletes: a file the table does not know becomes an unanchored attachment
(`imported`); a row whose bytes are gone is reported (`missing`), never dropped,
so switching devices is free and reversible.

## Layout

```
<card dataset>/
├── info.json
├── data.sqlite3          # card, card_tag, card_media, …
└── media/
    └── <card-uuid>/
        ├── diagram.png       # original names kept; collisions get -2, -3, …
        └── answer.mp3
```

The directory is named by the **card uuid**, so browsing one card's media and
deleting a card's whole attachment set are both a single path. A blank card in
the editor mints its uuid up front (`card_media_add` can then land bytes before
the first save — SQLite foreign keys are not enforced in this database, so the
media row can precede the card row). `card_save` honors a non-empty uuid, so the
pre-minted id is the one the saved card keeps.

## The `card_media` table

Added to `create_card_schema`, which runs `CREATE TABLE IF NOT EXISTS` on every
`open_card_db` — so existing datasets pick the table up on next open with no
migration code.

| Column | Meaning |
|---|---|
| `uuid` | Row identity; everything except the anchor keys on this. |
| `card_uuid` | Owning card (`FK → card`, `ON DELETE CASCADE`). |
| `field` | `question` \| `answer` \| `note` \| `suggestion` — where the token lives. |
| `handle` | The `{{anchor}}` slug, unique among the card's **live** rows. |
| `rel_path` | `media/<card_uuid>/<name>` — dataset-relative, forward-slashed. |
| `kind` | `image` \| `audio` \| `video`. |
| `title` | Alt text / caption. |
| `sort` | Order within the field's unanchored tail strip. |
| `autoplay` | Per-attachment auto-play flag (defaults off). |
| `play_on` | `tap` \| `reveal` — `reveal` fires when the card is flipped. |
| `duration_ms`, `width`, `height`, `size` | Probed on add; nullable. |
| `deleted_at` | Tombstone — soft delete, so sync can carry it (as `card`/`card_tag` do). |
| `created_at`, `updated_at` | Row stamps. |

`present` is not a column: `card_media_list` marks each row from the filesystem
at read time, and the render seam turns `present: false` into a placeholder that
names the file.

## The anchor grammar

Position inside the text is a `{{handle}}` token.

- **Grammar:** `handle` matches `^[a-z0-9][a-z0-9-]{0,31}$`. Mints from the
  slugified file stem, with a `-N` suffix on collision.
- The row `uuid` is the identifier; `handle` is only the human-legible anchor
  text. Renaming a handle rewrites `{{old}}` → `{{new}}` in the owning field in
  the same transaction, so the token and the row cannot disagree.
- **Rendering** splits the field text on anchors and interleaves: prose segments
  go through the site's existing renderer, media segments render as a native
  element. This is why `html.ts` / `sanitizeUrl` (shared with the wiki) are not
  touched. Placement is **block-level** — a token inside a sentence becomes its
  own block at that point, not a glyph mid-line (a v1 limitation).
- **Stripping** (`strip_media_anchors` in Rust, `stripAnchors` in TS — one rule,
  two sides of the FFI): the anchor is removed before `compute_question_hash`
  and before FTS indexing, so (a) re-attaching or renaming a handle never
  changes a card's dedup identity, and (b) search matches prose, never `png` or
  `{{`. Emptiness checks (`review_eligible`, `incomplete`) read the **raw**
  length on purpose: a media-only card's raw text is the token itself, so it is
  already eligible and not counted incomplete.

## Portable extensions

Cards restrict media to what an Android WebView reliably plays — narrower than
the wiki viewer set on purpose:

| Kind | Accepted |
|---|---|
| image | png, jpg, jpeg, gif, webp |
| audio | mp3, m4a, wav, ogg |
| video | mp4, webm |

`mov` and `m4b` are excluded even though the wiki lists them. Classification
lives in one place per side of the boundary: `datasets/media_kind.rs` (Rust) and
`src/lib/media/kind.ts` (frontend).

## Sync

Media rows are mutable dataset content, so they journal like `card_media_*`
kinds — `card_media_save` (coalescing, keyed by `uuid`) and `card_media_delete`
(keyed by `media_uuid`) — through the same five places every journal kind
touches (`ChangeClass::of`, `object_id_for`, `table_for_kind`, and both apply
paths in `sync/client/round.rs` and `sync/rest.rs`). They scope under the
dataset uuid, never `@app`.

Bytes arrive on a different plane from the row, so the pull side tolerates
**row-before-file**: a missing row resolves through `card_media_register`-style
re-link on file name, and until then the reader sees the placeholder.

## Surfaces

- **Editor** (`CardEditorContext.tsx`): the Attach toolbar (Image/Audio/Video,
  file dialog + native drag-drop) copies bytes and writes `{{handle}}` at the
  caret of the focused field; the Library drawer lists this card's rows with
  place / reorder / delete.
- **Review** (`CardsPage.tsx`): question, answer, and note render through
  `MediaField` with `reveal={reviewFlipped}`, so a `play_on: reveal` clip fires
  at the flip.
- **List / search rows:** `stripAnchors` plus a paperclip chip — never decode a
  full image for a grid row.
- One clip plays at a time, app-wide, via `src/hooks/useMediaPlay.ts`.

## MCP

Five tools mirror the commands — `card_media_list`, `card_media_add`,
`card_media_update`, `card_media_delete`, `card_media_register` — with
structured JSON responses. `card_media_add` takes a `source_path`, so an agent
can attach a file it just generated (an Edge-TTS clip, an OCR'd screenshot),
which is what makes "build a listening card" an agent-doable job. Policy tiers:
`card_media_list`/`card_media_register` are reads, `card_media_delete` is
recoverable (row soft-deleted, bytes kept for card-delete undo), and add/update
are writes.

## Code map

| Concern | Where |
|---|---|
| Schema, `MEDIA_DIR`, `strip_media_anchors`, fork copy | `src-tauri/src/datasets/cards/mod.rs` |
| Commands, handle minting, probes, reconcile | `src-tauri/src/datasets/cards/media.rs` |
| Extension classification | `src-tauri/src/datasets/media_kind.rs`, `src/lib/media/kind.ts` |
| Anchor split / strip / `resolveSrc` | `src/lib/media/anchors.ts` |
| One-at-a-time playback | `src/hooks/useMediaPlay.ts` |
| Render | `src/components/cards/MediaField.tsx`, `MediaBlock.tsx`, `MediaLibrary.tsx` |
| MCP + policy | `src-tauri/src/mcp/cards.rs`, `src-tauri/src/mcp/policy.rs` |
