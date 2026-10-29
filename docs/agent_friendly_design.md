# Agent-Friendly Design Principle

> **Design an app that is not only user-friendly, but also agent-friendly.**

This is a first-class design principle, equal in weight to user-friendliness. Every feature,
API, and data model should be designed with two users in mind: the **human** sitting at the
screen, and the **AI agent** operating through MCP, CLI, or API.

---

## Why This Matters

AI agents (Goose, coding assistants, automation scripts) are increasingly the primary
interface to software. An agent-friendly app:

- Reduces the cognitive load on the agent → better tool selection → fewer mistakes
- Enables automation of workflows the human UI supports but doesn't expose efficiently
- Makes the app composable — agents can chain operations the human does manually
- Future-proofs the app for richer AI integration without rebuilding the API surface

---

## Design Rules

### 1. Every feature must be MCP-accessible

If a human can do it in the UI, an agent must be able to do it through MCP. The MCP tool
set is the **complete programmatic interface** to the app. The UI is a convenience layer on
top of the same capabilities.

**Implication:** When adding a new Tauri command, simultaneously add the corresponding MCP
tool. Do not treat MCP as an afterthought.

### 2. Structured responses, not plain text

MCP tool responses should return **JSON** whenever the data has structure. Agents parse JSON
reliably; they guess at free-text unreliably.

**Good:**
```json
{ "status": "ok", "uuid": "abc-123", "name": "My Dataset", "media_count": 42 }
```

**Bad:**
```
Dataset created: My Dataset (abc-123) with 42 media files
```

Exceptions: truly simple confirmations ("Deleted.") and log output are fine as plain text.

### 3. Destructive operations: backup, don't block

Don't add confirmation dialogs or extra parameters that block agents. Instead, implement a
**backup/trash mechanism** so mistakes are recoverable:

- Before destructive operations (delete dataset, clear database, overwrite subtitles), copy
  the affected data to a trash/backup directory.
- The backup is timestamped and can be restored.
- Agents can operate freely without fear of permanent data loss.

**Implication:** The trash directory lives at `<data_dir>/fms-app/trash/`. Destructive
operations move data there before proceeding. A future `restore_from_trash` tool can undo.

### 4. Workflow tools over atomic tools

Group related atomic operations into **workflow-level tools** that match what agents actually
want to accomplish. Expose the atoms only when agents need fine-grained control.

**Example:** Instead of exposing `listen_get_media` + `listen_get_subtitles` +
`listen_get_cues` as three separate tools, expose `dictation_get_data` which returns all
three in one response.

**Counter-example:** `dataset_delete_subtitles` and `dataset_delete_database` stay separate
because agents may want to delete one without the other.

### 5. Tool descriptions are contracts

Each MCP tool's `description` is the agent's only signal for when to use it. Descriptions
must be:

- **Specific:** state what it does, what it returns, and what it requires
- **Composable:** mention related tools ("Use `dataset_list` first to get UUIDs")
- **Honest:** state limitations ("Requires a downloaded model")

### 6. Discoverability: agents should be able to figure out state

Before acting, agents need to know the current state. Provide **status/overview tools** that
give agents a quick orientation:

- `model_status` — what models are available, which is loaded
- `settings_get` — current configuration
- `web_service_get_status` — is the service running
- `auth_status` — am I logged in

These are cheap calls that save agents from guessing and failing.

### 7. Errors should be actionable

When a tool returns an error, the message should tell the agent **what to do next**:

**Good:** `"Model not loaded. Use model_load with uuid='...' first."`

**Bad:** `"Model not loaded"`

### 8. A write's success value must prove the write

`{"status": "ok"}` is not evidence. An agent cannot tell a call that persisted
382 texts from one that persisted nothing, and in practice it does not
try — the read-aloud log shows a full 382-text upload reporting `ok` while it
had written zero rows, because the real error came back as a plain string the
caller's success check did not recognise.

So every write returns something only a real write could produce:

- a count of what now exists — `texts_total`, `media_count`;
- a count of what is left — `remaining`;
- or the identity the server assigned — `uuid`, `order_num`.

**Implication:** if the only way to know a write landed is a second read, the
tool's return value is incomplete.

### 9. A bulk write returns a partition that sums to the input

A bulk call reports `saved` and `failed` (plus `skipped` for input it decided
not to write), and `saved + failed + skipped == sent` **always**. Per-item
errors carry the caller's own index so a failure can be mapped back to the item
that caused it.

This is what makes "partial" a readable answer instead of a lost one: without
the partition, a batch that lost one row out of 382 looks exactly like a batch
that lost all of them, and neither looks like success or failure.

**Implication:** every loop-over-items tool takes one call instead of N, and
returns the same partition shape as its single-item sibling.

---

## Implementation Priorities

The MCP tool set should cover these domains, in order:

1. **Dataset pipeline** — list, import, generate subtitles/waveforms/database, align, adjust
2. **Model management** — status, download, load, unload, delete, transcribe
3. **Dictation practice** — get data, save progress, edit cues
4. **LLM integration** — connection check, model list, chat
5. **TTS** — voice list, synthesize
6. **Book management** — CRUD books, chapters, sentences, words, audio
7. **System** — settings, auth, web service, OCR, capture, logs

---

## Measuring Agent-Friendliness

A feature is agent-friendly when:

- [ ] An agent can discover the feature through tool descriptions alone
- [ ] The agent can determine preconditions (what state is needed) from status tools
- [ ] The tool returns structured data the agent can act on
- [ ] Errors tell the agent how to recover
- [ ] Destructive operations are reversible (backup/trash)
- [ ] A write's return value proves the write (a count, or a server-assigned id)
- [ ] A bulk write returns `saved` / `failed` that sum to the input
- [ ] The feature is composable with other tools in workflows
