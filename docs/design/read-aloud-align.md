# Read-Aloud Alignment

## The idea

An audiobook dataset is one book and the recordings of it. The texts are the book in reading order; the recordings are chapter files that live **outside** the dataset, in a folder the user already had. Alignment answers one question per text: *which seconds of which recording are this text?* Everything else the page shows — the waveform, the preview, the confidence, the clip row — is derived from that answer.

The model is small, and the vocabulary is half of it:

| word | means |
|---|---|
| **recording** | one audio file outside the dataset (`source_uuid`). A *take* is the learner's own reading — a different thing entirely. |
| **text** | one row of `read_text`: one unit to practise, in reading order. |
| **span** | the seconds one text occupies inside one recording, with the confidence that it was heard there. |
| **clip** | a text's standard audio: the `read_standard_audio` row pronunciation is measured against. An aligned clip remembers its span; its wav is cut later, on first need. |
| **save** | span → clip row, a few hundred row writes. **Cut** is the expensive half: clip row → wav, and only when something needs to hear it. |

Two sentences carry the rest: **a span is the truth and a clip is a cache of it**, and **a save never overwrites a clip**.

## The four decisions

### 1. Delete first

`read_aloud_align_apply` skips any text that already owns a clip and returns it in `skipped` with that clip's uuid. The reason is not caution: a clip cut from an older span holds the seconds its own wav was cut from, so a second row under the same text would leave two claims about one text, and the one pronunciation is measured against would be whichever the default flag happened to name.

Changing a saved span is therefore **delete the clip, then save again**. The rule is enforced by the clip's existence — not by a flag some writer has to remember to reset, which is exactly what the old `applied_uuid` column was, and why it is gone.

### 2. No verdict

There is nothing to accept or reject. A span the model placed *is* the answer; its two edges stay editable until they are saved. A row's `status` is `proposed` or `refused` and nothing else — `refused` means "no span exists", and its `note` says why (too few words heard, read across two files, no recording left). The old `accepted`/`rejected` state was written as `accepted` on every row the guess created, so the ledger it claimed to keep never had a second value: it was a column that could only disagree with the data.

### 3. The derived index never syncs

Everything under `<dataset>/align/` is regenerable from the recordings plus the texts, so none of it is journalled, and `is_derived_index` keeps the directory out of the sync manifest. A cached transcript is one device's minutes of model time, not content, and the recordings stay where the user keeps them. Because it is derived, the shape of `proposals.json` is not a compatibility surface either — which is why old files are only expected to *parse* past removed fields, not to be migrated.

Being derived does not make the cache disposable. Its identity is the **sha256 of the recording's bytes**, so the whole `align/` directory travels with the dataset directory it sits in: copy `<name>/` and the audiobook folder to another machine, pick the new folder, and every chapter whose bytes arrived intact is reused without a single model call. Portability comes from copying, not from syncing — the sync manifest is unchanged, and two machines never write each other's cache.

### 4. One writer per fact

| fact | owner | never written by |
|---|---|---|
| where a span is | `proposals.json` — written by `guess`, `guess_one` and `move_span` | `apply` |
| what has been stored | `read_standard_audio` rows — written by `apply`, and by the TTS/upload paths | the align cache |
| what a recording says | `<content_hash>.json` — written by `transcribe`; `manifest.json` and `sources.json` only copy its answer forward | everything else |

`apply` writing no proposal file is why a save cannot disturb a guess, and why the page's four counts are derived from two sources — `status` for placed/refused, the clip map for unsaved/saved — instead of from a stored tally. `unsaved` is exactly `is_placed() && !owns_clip`: what a save would still store. The four overlap the way the filter chips do (a refusal can own a TTS clip), and that is intentional.

## Storage

```
<datasets_dir>/read_aloud/<name>/
├── align/                        # derived, device-local, excluded from the sync manifest
│   ├── sources.json              # the binding: folder, playing order, and path + size +
│   │                             #   mtime + content_hash per file
│   ├── manifest.json             # the portable table: content_hash → file_name, size,
│   │                             #   duration, words
│   ├── <content_hash>.json       # one recording: word timings + 20 ms peak envelope,
│   │                             #   named after the sha256 of its bytes
│   └── proposals.json            # one row per text: the span, its confidence, its status
└── data.sqlite3
    └── read_standard_audio       # uuid PK, text_uuid, is_default, text_sha,
                                  #   origin ('align'), source_uuid, start_secs, end_secs,
                                  #   boundaries (the cut wav's edges), note
```

Two files carry an identity and they answer different questions. **`sources.json` is the binding** — which folder this device is pointed at, in which order the files play, and where each one was scanned — so it is per device by nature. **`manifest.json` is the cache's own table of contents**, keyed by content hash, and it is what lets a copied `align/` describe itself without a binding: duration and word count are read from it rather than by parsing a multi-megabyte transcript.

A cache written before content hashes existed is named `<source_uuid>.json` (`sha256(file_name)`). It is adopted, not rebuilt: `promote` renames it to its content hash, so the payload — the words and the peaks, which describe the same bytes either way — is untouched. `source_uuid` itself stays `sha256(file_name)` in `read_standard_audio` and `proposals.json`, so no stored row needs a migration.

An aligned row is a standard-audio row like any other — the span is three columns, not a new table. `origin` says it was cut from a recording, so the UI can label it **Aligned** and the packer knows the wav is a cache. `text_sha` is what makes a clip **Stale** when the text is edited, and a stale clip stops pronunciation scoring rather than grading against a text nobody is reading.

## The eight commands

Every step is idempotent and resumable, and each has an MCP twin in `mcp/books.rs`.

| command | does | writes |
|---|---|---|
| `read_aloud_align_sources` | connect (or list) the recordings folder: natural-sorted, every cache whose bytes were already heard is reused, nothing deleted | `sources.json` + `manifest.json` |
| `read_aloud_align_transcribe` | cache one file's word timings + peaks; repeat until `remaining` is 0 | `<content_hash>.json` + `manifest.json` |
| `read_aloud_align_transcript` | one cached transcript, word by word (read-only) | — |
| `read_aloud_align_guess` | place every text along one global token stream | `proposals.json` |
| `read_aloud_align_guess_one` | re-place one text, neighbours untouched | `proposals.json` (one row) |
| `read_aloud_align_proposals` | rows + texts + preview + the four counts (read-only) | — |
| `read_aloud_align_move_span` | move one placed span by hand (nudges, typed edges) | `proposals.json` (one row) |
| `read_aloud_align_apply` | store placed spans whose text has no clip; `text_uuid` for one, omitted for all | `read_standard_audio` only |

`register` → `transcribe` (once per file) → `guess` → `apply` is the whole pipeline a human drives in the Advanced panel and an agent drives through these tools. The only write that produces playable audio is the cut, and it happens lazily inside `standard_audio::materialize` the first time a text is opened, scored or packed.

`register` is a **connect**, and it reports what the connection found: `{total, reused, adopted, adopted_unverified, new, missing, orphaned, reordered}`. `reused` were these exact bytes, `adopted` were a legacy cache whose file name hash still matches, `new` still needs listening, `missing` are registered files the folder no longer holds, and `orphaned` are cached transcripts no current entry claims. `reordered > 0` means the playing order changed, so the spans were placed along a stream that no longer exists. Nothing is deleted and `proposals.json` is never rewritten, which is why picking the wrong folder is undone by picking the previous one again — the panel offers exactly that button.

## Where this lives in the code

| file | holds |
|---|---|
| `datasets/read_aloud/source_align/mod.rs` | the module doc, the constants, `round_ms`, the eight `#[tauri::command]` wrappers |
| `source_align/index.rs` | `SourceEntry`/`SourceIndex`/`SourceTranscript`, the paths and the JSON, registration, transcription, peaks, preview |
| `source_align/place.rs` | the matcher: `Found`/`Guess`/`Search`/`Prepared`/`Placed`, `prior_position`, the two passes, `run`, `run_one` |
| `source_align/proposals.rs` | `Proposal`/`ProposalSet`, load/write, the review view (`ProposalView`, `ClipView`), `list`, `counts`, `move_span` |
| `source_align/writes.rs` | `apply` |
| `datasets/read_aloud/standard_audio.rs` | the clip rows: `store_span`, `guess_span` (the no-folder fallback), `materialize` (the cut) |
| `datasets/read_aloud/edges.rs` | `place_edge` — the single rule both guessers use to find the pause an edge belongs in |

On the frontend, `components/read_aloud/source_align/` is chrome plus three hooks: `SourceAlignWorkspace` owns the layout and the selection, `useSpanPlayer` the one audio element, `useSpanDrafts` the proposals and every write (`guess`, `guessOne`, `saveOne`, `saveAll`, `deleteClip`, `moveSpan`, and the debounced nudge flush), `useAlignSources` the folder, the transcription and the transcript pane. `ClipActions.saveBlocker` is the single predicate for "can this span be saved": `refused`, `has-clip`, `busy`, or nothing.

## Known rough edges

- **A rename costs the proposals, not the cache.** A span names `source_uuid` (a hash of the file name), so renaming a chapter means its clip can no longer be cut and its texts must be placed again. The bytes did not change, so the transcript is reused and re-placing costs no listening. A *moved* folder is followed: when the stored path is not a file, the index falls back to `directory + file_name`, which is also what makes a copied dataset connect on the new machine.
- **Two matchers still exist.** `standard_audio::guess_span` places one text against a decoded file, and the align stream places all of them against one token stream. They share `edges::place_edge`, but not the matcher or its vocabulary, so the same text can land differently depending on which button was pressed.
- **Reading order is an assumption.** A dataset whose texts are not in the recordings' order is placed by the fallback cursor walk when more than three quarters refuse; a *mostly* ordered dataset sits in between, and there is no way for the guess to notice.
- **`proposals.json` is per device.** A span guessed on one machine is not a fact on another until it is saved, and saving is what travels (as the clip).
