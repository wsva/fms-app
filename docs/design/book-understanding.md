# Book Understanding Layer

Comprehension artifacts for difficult (philosophy, dense non‑fiction) books. Every
feature — Edge‑TTS listen, LLM explain/simplify, chapter digest, argument map,
comprehension quiz, user notes, agent note assessment, and a per‑book glossary — is
modelled as **data attached to a scope** inside the book's own `data.sqlite3`. The
same rows are written by two doors: the built‑in Tauri commands (over the existing
`llm_chat` core and `edge_tts`) and the MCP tools (so a third‑party agent can drive
generation). Backend lives in `src-tauri/src/datasets/book_artifacts.rs`; frontend in
`src/lib/read/artifacts.ts`, `src/hooks/useBookData.ts`, and
`src/components/read_book/UnderstandingPanel.tsx`.

## Scopes and anchors

An artifact is attached to a **scope**:

| scope_type  | scope_uuid                             | source text                                             |
|-------------|----------------------------------------|---------------------------------------------------------|
| `paragraph` | the paragraph's `paragraph_break` uuid  | the sentences since the previous break, in `order_num`  |
| `chapter`   | the chapter uuid                        | every paragraph of the chapter joined with `\n\n`       |
| `sentence`  | a `book_sentence` uuid                  | that sentence's `content`                               |

The paragraph scope reuses the existing `paragraph_break` sentinel row in
`book_sentence` as its stable anchor — no new paragraph table.

### Anchor backfill (not journaled)

Older chapters may end without a trailing `paragraph_break`, leaving the last
paragraph with no anchor. `open_book_db` runs an idempotent
`backfill_paragraph_anchors`: for each chapter whose sentences are non‑empty and whose
highest‑`order_num` row is not a break, it appends one (new uuid, empty content, next
`order_num`).

**Decision: the backfill is not written to the sync change‑log.** Each device
self‑heals on open, exactly like read‑aloud's idempotent `ALTER TABLE` migration.
Journaling it would make every adoption of a shared base book fork its overlay and
flood the queue with rows the other devices would create anyway. Convergence holds
because an artifact is only ever created *after* its anchor exists locally, so the
anchor a given artifact references is present on the device that made it and is
reproduced identically (by the same deterministic rule) on every other device that
opens the book.

### Delete cascades

Artifacts die with their anchor:

- `book_delete_chapter` / `delete_chapter_subtree_tx` also `DELETE FROM book_artifact`
  for the chapter uuid and for every `book_sentence` uuid in that chapter.
- `book_delete_sentence` also deletes artifacts scoped to that uuid — which covers
  both a break row's paragraph artifacts and any sentence‑scoped ones. The existing
  audio‑file removal is unchanged.

So the frontend's "delete break only" action (merge paragraph with the next) removes
that paragraph's audio and generated artifacts; the confirmation dialog says so.

## Schema

Added to the `open_book_db` `CREATE TABLE` batch (`IF NOT EXISTS`, so existing books
pick it up on next open):

```sql
CREATE TABLE IF NOT EXISTS book_artifact (
    uuid         TEXT PRIMARY KEY,
    book_uuid    TEXT NOT NULL,
    scope_type   TEXT NOT NULL,            -- 'paragraph' | 'chapter' | 'sentence'
    scope_uuid   TEXT NOT NULL,
    kind         TEXT NOT NULL,            -- see the kind matrix below
    content      TEXT NOT NULL DEFAULT '', -- markdown; 'questions' = JSON array
    source       TEXT NOT NULL DEFAULT '', -- 'ollama' | 'agent' | 'user'
    model        TEXT,
    parent_uuid  TEXT,                     -- assessment -> the note it assesses
    content_hash TEXT NOT NULL DEFAULT '', -- FNV-1a 64 hex of source at generation ('' for notes)
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_book_artifact_scope ON book_artifact(scope_type, scope_uuid);

CREATE TABLE IF NOT EXISTS book_term (
    uuid        TEXT PRIMARY KEY,
    book_uuid   TEXT NOT NULL,
    term        TEXT NOT NULL,
    definition  TEXT NOT NULL DEFAULT '',
    source      TEXT NOT NULL DEFAULT '',
    model       TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(book_uuid, term)
);

CREATE TABLE IF NOT EXISTS book_quiz_result (
    uuid          TEXT PRIMARY KEY,
    book_uuid     TEXT NOT NULL,
    artifact_uuid TEXT NOT NULL,
    user_id       TEXT NOT NULL DEFAULT '',
    total         INTEGER NOT NULL DEFAULT 0,
    correct       INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_book_quiz_result_artifact ON book_quiz_result(artifact_uuid);
```

## Kind matrix

| kind           | scope          | source        | content                                              | stale? |
|----------------|----------------|---------------|------------------------------------------------------|--------|
| `explanation`  | para/chap/sent | ollama/agent  | markdown explanation                                 | yes    |
| `simplification`| para/chap/sent| ollama/agent  | markdown plain‑language rewrite                       | yes    |
| `digest`       | chapter (or para)| ollama/agent| markdown **premise outline** (thesis → premises → conclusions), not prose | yes |
| `argument_map` | para/chapter   | ollama/agent  | markdown claim / premise / objection / reply outline  | yes    |
| `questions`    | para/chapter   | ollama/agent  | JSON array of `mcq` + `open` (see below)              | yes    |
| `note`         | para/chap/sent | user          | markdown, the learner's own note                      | never  |
| `assessment`   | same as its note| ollama/agent | markdown feedback; `parent_uuid` → the note           | never  |

Generated kinds are **unique per (scope, kind)**: regenerating replaces the existing
row in place rather than appending. Notes are the only multi‑row kind per scope.

### `questions` JSON shape

The `questions` body is a JSON array mixing two element shapes:

```jsonc
{ "type": "mcq",  "stem": "...", "choices": ["a","b","c","d"], "correct": 2, "explanation": "..." }
{ "type": "open", "stem": "...", "model_answer": "...", "explanation": "..." }
```

`generate_artifact` validates the array (and drops the model's code fences) before
storing. On the client, MCQs are graded against `correct`; open questions are
self‑graded against `model_answer` (LLM‑graded free text is a follow‑up).

## Staleness by hash

Following the read‑aloud `normalized_content` pattern: a generated artifact stores
`content_hash` = FNV‑1a‑64 (hex) of its source text at generation time. `book_list_artifacts`
recomputes the current source hash and returns `stale = (stored != current)` per row.
Nothing is deleted when the source changes — the UI shows an **outdated** wand badge and
offers **Regenerate**. Notes and assessments (`content_hash = ''`) are never stale.

## Glossary

`book_term` holds recurring concepts with markdown definitions, upserted on
`UNIQUE(book_uuid, term)`. `book_extract_terms` runs an LLM pass over a scope's source
text and upserts the terms it finds.

**Never overwrite user edits:** extraction only refreshes rows whose `source` is
`ollama` or `agent`; a row the learner edited (`source = 'user'`) is left untouched. A
manual save always sets `source = 'user'`, so hand edits survive re‑extraction.

Vocabulary words in the sentence drawer are cross‑linked: a word that matches a term
(case‑insensitive) shows a **glossary** badge that reveals the definition inline.

## Feedback loop: quiz → XP → cards

`book_record_quiz_result(book_uuid, artifact_uuid, total, correct)` stores a
`book_quiz_result` row and awards XP through `xp_award_internal` with
`source = "book_quiz"`, `reference_id = artifact_uuid`. XP is deduped on
(user, source, reference_id, dataset), so only a learner's first completion of a given
quiz awards; the amount is the best correct count. XP journals under `@app` per user,
and the command emits the existing `xp-earned` event so the UI toast fires.

Missed questions need **no new command**: the frontend promotes one to a card with the
existing `card_save` into a card dataset the learner picks (question stem → front,
correct answer + explanation → back, `source_dataset_uuid` → the book).

## Overlay vs base layering

Generated artifacts are overlay data by construction. Every write path resolves the
directory with `find_book_dir_write`, which forks a downloaded base book into the
writable overlay on first write. A shared base book therefore stays pristine while
notes, artifacts, glossary terms and quiz results live in the overlay — no extra
layering work is needed. Read paths (`book_list_artifacts`, `book_list_terms`) use
`find_book_dir` and see the merged view.

## Generation core

`generate_artifact(settings, book_uuid, scope_type, scope_uuid, kind, model?, instructions?)`
is State‑free: it resolves the writable dir, builds the source text, composes the
per‑kind system prompt, calls the LLM through `ai::llm::llm_complete` (a small
State‑free helper: `goose_llm::chat_once` on desktop, `ollama_chat` otherwise),
validates `questions`, computes `content_hash`, replaces any existing same‑scope/kind
row, saves, and journals `book_artifact_save`. `assess_note` and `extract_terms` follow
the same shape. The Tauri commands are thin twins over these functions (the workflow
pattern), and the MCP tools call the same commands.

### Prompt inventory

- `explanation` — explain the passage for a language learner.
- `simplification` — rewrite in simpler language, preserving meaning.
- `digest` — for argumentative/philosophical text produce a **premise outline**
  (thesis → supporting premises → conclusions), not a prose summary.
- `argument_map` — extract claim / premise / objection / reply structure.
- `questions` — strict JSON array mixing `mcq` (4 choices, `correct` index,
  `explanation`) and `open` (Socratic, `model_answer`, `explanation`), in the
  passage's language.
- note assessment — feedback on the learner's note against the source.
- term extraction — recurring philosophical/technical concepts with concise definitions.

Output language defaults to the passage/English; the `instructions` argument lets a
caller or agent override (e.g. "summarise in German").

## Sync

Artifacts, glossary terms and quiz results are **dataset content** (State class), not
app‑scope. `sync/change_log.rs` maps `book_artifact_save`/`_delete`,
`book_term_save`/`_delete` and `book_quiz_result_save` to `State`, `object_id_for`
reads `uuid`, and the collection map names them `book_artifacts` / `book_terms` /
`book_quiz_result`. `sync/client/round.rs` and `sync/rest.rs` gained apply arms
mirroring `book_sentence_save`/`_delete` — upsert/delete the row in the target book's
`data.sqlite3` by payload. Quiz replays go through `apply_quiz_result_row`, which
stores the row **without** re‑awarding XP (the award already happened on the origin
device and is deduped anyway). No `PROTOCOL_VERSION` bump: these are dataset‑scoped
State rows relayed opaquely by the hub.

## MCP tools

Thin wrappers over the same core (agent‑friendly: one body, two doors):
`book_list_artifacts`, `book_save_artifact`, `book_delete_artifact`,
`book_generate_artifact`, `book_assess_note`, `book_paragraph_listen`,
`book_list_terms`, `book_save_term`, `book_delete_term`, `book_extract_terms`,
`book_record_quiz_result` (lets an agent‑driven quiz close the XP loop too).

Policy tiers (`mcp/policy.rs`): `book_list_artifacts` and `book_list_terms` are in
`READ_TOOLS`; `book_delete_artifact` and `book_delete_term` are in `RECOVERABLE`
(derived data is regenerable and every delete is journalled, so the row is
reconstructible; a note is the one non‑derived kind and is still recoverable from the
journal) — the required reason comment keeps the destructive‑word `#[cfg(test)]` guard
green. The rest fall through to `write`. The `book_` manual prefix is already
registered, so `mcp/manual.rs` needs no change.

## UI: Understanding split mode

- **Toolbar toggle** (`ReadBookPage.tsx`): an **Understanding** `ToolbarButton`
  (`variant="toggle"`, `Brain` icon) in the contextual read‑actions group, rendered only
  on a chapter page. Clicking while active returns to plain reading. Enabling it with no
  paragraph focused yet focuses the first paragraph that has an anchor.
- **Split layout** (`ReadingView.tsx`, level 3): when understanding is on and not
  mobile, the reading pane becomes a horizontal flex — left = the existing
  `ParagraphList` in its own scroll container, a drag splitter, right = the
  `UnderstandingPanel`. Default split 58/42, or 50/50 when the window is narrower than
  ~1400px and no width has been stored; the ratio persists in `localStorage`
  (`book-understanding-split`). Both columns scroll independently (`min-w-0` +
  `overflow-y-auto`). Mode off = the current full‑width pane, zero visual change.
- **Focus linkage**: tapping a paragraph (or its `Brain` icon) sets the focus; an
  `IntersectionObserver` scroll‑spy moves focus to the paragraph nearest the top while
  understanding mode is on. The focused paragraph gets a ring. The right panel always
  shows the focused paragraph's scope.
- **`UnderstandingPanel.tsx`**: header = focused‑paragraph preview + **Listen**
  (Edge‑TTS synth & play). Tabs: Explain, Simplify, Argument, Quiz, Notes, Chapter
  (Digest + Glossary). Generated markdown reuses `MarkdownViewer` (the Wiki/Cards
  renderer). Stale artifacts show a wand badge on the tab and card; clicking regenerates.
- **Mobile**: no split — the same `UnderstandingPanel` renders in a right slide‑in
  drawer (the Wiki/Read‑Aloud drawer pattern, `mobile` flag), opened by the toolbar
  toggle or by tapping a paragraph.

## Test plan

- `cd src-tauri && cargo check --offline` and `cargo check --offline --features desktop`.
- `npx tsc --noEmit`.
- Manual: open a chapter → backfilled break appears on the last paragraph; toggle
  Understanding → split appears, clicking/scrolling moves focus; generate
  explanation/simplification/argument‑map/digest/quiz; digest of an argumentative
  chapter returns a premise outline; quiz has both MCQ and open questions, finishing
  awards XP, promoting a missed question creates a card; extract terms → glossary shows
  definitions, a user edit survives re‑extract; edit a sentence → its artifacts flip
  stale; add a note → assess it; Listen generates and plays audio; mobile width → panel
  becomes a drawer; delete a chapter → its artifacts are gone.
- Sync: journal round‑trip of the four artifact/term kinds (and quiz result) between two
  devices, plus the existing `sync_conformance` tests.

## Follow‑ups (out of scope for v1)

- LLM‑graded free‑text answers for open questions.
- A workflow‑engine batch template ("digest the whole book overnight"); the MCP tools
  already let an agent drive batch generation ad hoc.
- A per‑book output‑language column (today `instructions` overrides per call).
