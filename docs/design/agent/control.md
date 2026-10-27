# Agent Control Plane — controlling *all* aspects of the app

Sibling to [integration.md](./integration.md): that file covers how an agent **finds and attaches** to FmS; this one covers what it can **do** once attached, and what is missing for a third-party agent (Codex, Claude Code, goose, WorkBuddy, …) to control the whole app rather than answer one quiz. [mailbox.md](./mailbox.md) is the third sibling: asynchronous cooperation — the request/claim/notice ledger the dock renders.

Status legend, as in `integration.md`: ✅ built · ⚠️ partial · 💡 proposed.

**The requirement being designed against:** an agent should be able to (1) open a page and click a button, (2) use various models, (3) configure and run workflows, (4) generate a TODO list for the user, jump them to the page, and let *them* do it, and (5) everything else a human can do in the UI — which is the standing rule in [../../agent_friendly_design.md](../../agent_friendly_design.md) §1: *"If a human can do it in the UI, an agent must be able to do it through MCP."*

---

## 1. Two control planes, one of them is tiny

| Plane | Size | Where |
|---|---|---|
| **Data / logic** | ~230 tools | [src-tauri/src/mcp/](../../../src-tauri/src/mcp) — `datasets`, `dictation`, `cards`, `books`, `models`, `ai`, `wiki`, `sync`, `system`, `workflow`, `ui`, `todo` |
| **UI control** | 2 generic tools + 5 `app_*` verbs | [mcp/ui.rs](../../../src-tauri/src/mcp/ui.rs) `ui_list_actions` / `ui_invoke_action` over the registry the pages publish; [mcp/system.rs](../../../src-tauri/src/mcp/system.rs) `app_navigate`, `app_open_review`, `app_start_dictation`, `app_notify`, `app_get_ui_state` |

The `ui_*` pair removes the "one backend release per button" problem: a page registers its actions at mount (`agent_report_ui_actions`), `ui_invoke_action` drives one through the same `agent-action` channel, and `agent_report_action_result` carries the outcome back so the tool returns it instead of guessing. The hand-written `app_*` verbs remain for the intents that predate the registry. For an exact count and name list, read `/agent.md` — it is generated from the live router, so any number written in prose (including this one) is a snapshot.

The bridge itself already exists and is agent-neutral: the `app_*` tools `emit("agent-action", …)` and the shell performs the action in [src/app/(app)/home/page.tsx](../../../src/app/(app)/home/page.tsx) L122-130 (`navigate` / `open-review` / `start-dictation` / `notify`). Adding control means adding *verbs to that channel*, not building a channel.

---

## 2. Coverage audit

| # | Requirement | Status | Notes |
|---|---|---|---|
| 1a | Open a page | ✅ | `app_navigate`, the full tab set, error names the valid set |
| 1b | Click a button | ✅ | `ui_list_actions` / `ui_invoke_action` over the page registry, plus the three original intent verbs |
| 2 | Use various models | ✅ desktop · ⚠️ mobile | STT / SaT / TTS / OCR complete; LLM is multi-provider on desktop (§2.2) |
| 3 | Configure and run workflows | ✅ | 13 tools, incl. authoring new YAML pipelines and `workflow_list_actions` |
| 4 | TODO for the user + jump to page | ✅ | `todo_add`/`_list`/`_complete`/`_dismiss`; completing a `human` step's todo resumes that run |
| 5 | Everything else | ⚠️ | see §2.2 (cloud model listing) and §7 |

### 2.1 Navigation and intents — ✅ / ❌

`app_navigate` covers the full tab set and its refusal is already the house pattern for actionable errors (`"unknown tab '{}'. Valid tabs: …"`, plus the special case explaining that `agent` is a configuration page and not a view an agent should drive). Beyond that there are exactly three intents, each hand-coded with its own param struct. There is no generic "press this control", no way to set a field, no way to read what a control would do.

### 2.2 Models — ✅ desktop

- ✅ **STT**, full lifecycle: `model_status` (carries an actionable `hint`, and tells a pipeline-driven agent *not* to call it because `dataset_full_report` already reports `stt_model`), `model_download`, `model_load`, `model_set_default`, `model_unload`, `model_delete`, `model_transcribe`.
- ✅ **Text segmentation**: `text_split`, `text_split_unload` (SaT, `textsplit`-gated).
- ✅ **TTS**: `tts_list_voices`, `tts_synthesize` (Edge).
- ✅ **OCR**: `ocr_recognize`, `ocr_recognize_tsv`, `ocr_hyphen_dictionary_status/download/delete`, language list.
- ✅ **LLM, multi-provider on desktop**: [ai/llm.rs](../../../src-tauri/src/ai/llm.rs) validates against `LLM_PROVIDERS` (`ollama | openai | anthropic | groq | databricks`), `llm_chat` takes a per-call `provider` override, `llm_set_provider` switches the saved default, and the desktop path routes every call through the `goose_llm` provider layer. `settings_get` redacts credentials (`SECRET_FIELDS` → `***` plus `has_<field>`), so an agent can read the *shape* of the config without reading the keys.
- ⚠️ **Remaining, all small**: `llm_list_models` / `llm_pull_model` / `llm_delete_model` are Ollama-only because the other providers have no local model store — their descriptions should say that rather than fail obliquely. `llm_chat_stream` uses only the saved provider (no per-call override), so an agent that overrides `llm_chat` should not expect the stream to follow.
- On the non-desktop build the whole LLM surface is Ollama-only by design ([`llm_chat` L299-312](../../../src-tauri/src/ai/llm.rs#L299-L312)); a cloud `provider` override there is refused by name instead of being silently ignored.

Reframe worth keeping in mind: the agent *is* an LLM. "Use various models" almost always means letting the agent dispatch **app work** to a chosen model — a cheap one for text normalization, a strong one for read-aloud scoring — not replacing the agent's own inference.

### 2.3 Workflows — ✅ the strongest part of the surface

Thirteen tools in [mcp/workflow.rs](../../../src-tauri/src/mcp/workflow.rs): `workflow_list`, `_create`, `_status`, `_reap`, `_next`, `_get_definition`, `_advance`, `_record`, `_intervene`, `_verify`, `_adopt`, `_builtin_templates`, `_list_actions`.

`workflow_create` **takes a YAML definition**, so an agent can author a brand-new pipeline, not merely run a template — validated by `workflow/core/validate.rs` (unique ids, existing refs, acyclic, `verify:`/`adopt:` ids registered). Execution is lease-based single-flight (`workflow_advance` → perform → `workflow_record`, with `claim_id` protecting against a reaped claim clobbering a live one), and `workflow_verify` joins live probes to each step's `verify:` binding so a probe can retract a claim but never promote one.

The step-action vocabulary is no longer a guess: `workflow_list_actions` publishes the ids and commands the Workflow page registered (from `src/lib/workflow/step-ops.ts`), and validation refuses an invented id by name.

One real gap:

- ⚠️ **`verify:` providers exist for dictation only** — `workflow_verify` explicitly errors for other dataset kinds. Coverage gap, not a defect.

### 2.4 Human tasks — ✅

`todo_add`, `todo_list`, `todo_complete`, `todo_dismiss` in [mcp/todo.rs](../../../src-tauri/src/mcp/todo.rs), backed by the app-level SQLite table and workspace-scoped exactly like dictation progress — so a todo an agent leaves on the desktop appears on the phone. Following an item replays the same `agent-action` payload `app_navigate` emits, and a workflow `human` step enqueues a todo instead of invoking a tool; completing it resumes the waiting run.

---

## 3. ✅ UI action registry — the fix for "click a button"

One tool per control is the wrong shape: 18 pages × dozens of controls, and every new button needs a Rust release. The registry inverts it — pages publish what they can do, and two generic tools drive the published set.

**The pattern already existed twice in this codebase**, so this was an extension of a house idiom rather than an invention:

1. `step-ops.ts` — `KNOWN_COMMANDS` + `getStepOp` map a step id to a command, args and form fields. A frontend registry of executable actions, already used to render a picker.
2. `agent-prefill` — the UI→agent direction that existed when FmS still dialled an agent: WorkflowPage emitted it to hand a generated prompt to the dock's composer. Both the event and the ACP client behind it were removed on 2026-10-25; the lesson that outlived it is that a *named intent on an event* is a cheaper seam than a command per button, which is what the registry below generalises.

**Implemented shape:**

```
# registered by each page at mount
{ id: "cards.start-review", label, tab, kind: "read" | "mutate" | "destructive",
  params_schema: {…}, when: <availability predicate> }
```

Two tools: `ui_list_actions` (the registry, with schemas) and `ui_invoke_action { id, params }`, both in [mcp/ui.rs](../../../src-tauri/src/mcp/ui.rs), routed over the **existing** `agent-action` channel and the existing shell bridge. Nothing new in transport, and the `kind` field feeds the policy tiers in §6.

**The two constraints, and how they were resolved:**

- **`agent-action` is fire-and-forget.** `agent_report_action_result` is the correlation path: the page reports the outcome, `ui_invoke_action` returns it with a timeout. Where the effect is observable (`app_get_ui_state`, `dataset_get`, `workflow_status`), reading it back is still the cheaper answer.
- **The registry only exists after the webview mounts.** `ui_list_actions` before mount returns an actionable message ("the UI is not mounted yet; …") rather than `[]`, because the two are indistinguishable to an agent.

**A favourable consequence of the existing architecture:** pages are display-toggled, not unmounted (`display: none/flex`), so a page registers its actions once at mount and they **survive navigation**. The registry is therefore "everything the app can do right now", not "what the visible page can do" — which is what makes `ui_list_actions` useful as a capability manifest.

**Discovery must not be duplicated:** the registry feeds `ui_list_actions`, the action index in `/agent.md`, and `app_bootstrap` — one source, three renderings (`integration.md` §5.0). A hand-written list of action ids in a doc would drift on the first rename.

---

## 4. ✅ TODO domain — human-in-the-loop as a first-class step

**The model: a TODO item is a pending `agent-action` with a human in the loop.** That framing matters because it means no new navigation machinery — the item's target is the exact payload `app_navigate` / `app_open_review` / `app_start_dictation` already emit.

```
{ uuid, title, detail,
  target: { tab, params },        # the agent-action payload, verbatim
  priority, due?, source_run_id?, # which workflow run created it
  status, created_at, completed_at, user_key }
```

- **Tools** (with Tauri command twins, per the parity rule in `AGENTS.md`): `todo_add`, `todo_list`, `todo_complete`, `todo_dismiss`.
- **UI**: a panel listing open items; clicking one performs the jump and marks it done. Where it lives is an open question (§9).
- **Storage**: app-level SQLite beside `listen_dictation` and the XP ledger, workspace-scoped.
- **Sync**: writeback-eligible. Per `docs/design/sync.md` §1.3 the excluded class is *"binary/management ops — media/audio writes, dataset create/delete/move, STT/model ops"*; a todo is a row change carrying `user_key`, exactly like dictation progress. So a todo created on the PC **appears on the phone** — which is what makes "agent plans on desktop, learner practises on phone" work.

**The composition that makes this worth building:** a workflow step whose action is `human` **enqueues a todo instead of invoking a tool**, and the run waits for `todo_complete`. Today the engine can only express machine steps, so a pipeline like *split text → generate TTS reference audio → **learner records a take** → score it → award XP* cannot be authored end-to-end — the human part falls outside the DAG. For a language-learning app, human steps are not a convenience; they are a category the product needs (record audio, listen and judge, read aloud, decide a hyphenation). This single addition turns the workflow engine into a plan the *user* can execute, which is requirement 4 in its strongest form.

---

## 5. ✅ Reading the app: context, screen, and events

### 5.1 ✅ `app_get_ui_state` reports the dataset

The dock no longer hardcodes `activeDatasetUuid`: [src/lib/agent/ui-context.ts](../../../src/lib/agent/ui-context.ts) is a module singleton that merges context patches and forwards the delta (debounced 200 ms), so each component claims only the keys it owns — Dictation reports the dataset, media and cue; the shell reports the tab. Wire contract: a key present with a value sets it, present with `null` clears it, absent leaves it untouched. The reported set is `{ active_tab, active_dataset_uuid, active_media_uuid, cue_index, selection, scroll }`.

### 5.2 ✅ A see-then-act loop

`capture_screenshot { minimize }` defaults to `minimize=false`: the app window is left alone and the primary monitor is captured as it looks, so the returned `data:image/png;base64` URL shows the app's own UI and feeds straight into `ocr_recognize`. `minimize=true` still exists for capturing the desktop underneath, and its description warns that on Windows it opens the native snip overlay — which parks an agent until a human drags a region.

### 5.3 ✅ One event timeline instead of per-domain polling

`app_events { after, limit? }` reads the same in-memory ring the `log_*` tools use, so one cursor follows subtitle passes, sync pushes and workflow steps together. `evicted_since_cursor` is non-zero when the caller fell behind the 1000-entry ring, and `log_read_file_history` is the archive — which is what makes the cursor safe to use instead of polling each domain in turn.

---

## 6. ✅ Policy — the prerequisite for broad control

This is the caveat that had to land *with* full app control, not after it. It did.

**What the problem was:** `sync/server.rs::zone_guard` treats loopback (with a local `Host` header) as **Trusted with no credentials at all** — `AuthContext::trusted()`, documented as *"this is the agent path"*. Meanwhile FmS's only approval policy lived **in the ACP client** (the seven-word destructive list `delete|remove|clear|drop|revoke|reset|rebuild`, preferring `allow_once`/`allow_always`) and therefore only ran in dock mode — so a third-party agent in attach mode bypassed it completely, and "control all aspects" from attach mode used to mean *any local process can delete datasets*. A trusted network position is not the same claim as "may destroy". (The ACP client was removed on 2026-10-25; the policy below lives on `/mcp` and was the part worth keeping.)

**What landed** ([mcp/policy.rs](../../../src-tauri/src/mcp/policy.rs)):

- Policy is enforced **on `/mcp`**, not in the dock. Five tiers — `read`, `write`, `destructive`, `ui-control`, `human-handoff` — and `tool_router()` wraps every route, so a new tool is never silently a read. A `#[cfg(test)]` guard fails the build when a destructive-sounding name is left unclassified.
- Two modes: `permissive` (the default — classify and journal, deny nothing) and `enforce`, which refuses the destructive and human-handoff tiers and additionally refuses a `settings_set` that would loosen `mcp_policy`.
- The undo loop is closed: `app_trash_list` + `app_restore` over the app trash, and the trash-backed deletes return `trashed_to` — which is what makes the permissive default defensible.
- Every call is journalled in `mcp_audit`; `app_audit_get` reads it back.
- ⚠️ Still global, not per-agent: the mode is one setting. The per-agent tier idea was going to ride on the hosted-mode host record; with that removed (`integration.md`), one global mode is the whole of it.

---

## 7. Other ideas, small and high-leverage

- ❌ ~~`app_prefill_prompt { text, send? }`~~ — it queued work *into* the dock, so it went with the ACP client. `app_notify` is the remaining push channel: one note to the user, shown as a toast and kept in the dock.
- ✅ **Keep the `app_navigate` error style everywhere.** Naming the valid set and the next call is what turns a failure into a lesson; it is the cheapest way to teach an agent that has never seen the app.
- ✅ **`workflow_list_actions`** — the step-action vocabulary from `step-ops.ts`, so authored YAML validates first try (§2.3).
- 💡 **MCP notifications** later, if `rmcp`'s server surface allows — the alternative to `app_events` polling.

---

## 8. Sequencing

1. ✅ **`active_dataset_uuid` + extended `ui_state` report** (§5.1).
2. ✅ **`/mcp` policy tiers + redact secrets from `settings_get`** (§6).
3. ✅ **UI action registry**: `ui_list_actions` / `ui_invoke_action`, with the `agent_report_action_result` correlation path (§3).
4. ✅ **TODO domain** + the `human` workflow step (§4).
5. ✅ `llm_chat` provider override + `llm_set_provider`; `workflow_list_actions` (§2.2, §2.3).
6. ✅ `capture_screenshot` without minimizing; `app_events(after=)`; `app_trash_list` / `app_restore`. (`app_prefill_prompt` shipped here too and was later removed with the ACP client.)
7. Deferred: MCP notifications, a per-page action authoring UI — and the attachment-side work that remains is sequenced in [integration.md](./integration.md) §8 (the stdio↔HTTP bridge). Hosted mode and its per-agent tiers are **dropped**.

Track 1 is complete and reported per-tool in `/agent.md`; track 2's remaining work is the attachment side, sequenced in [integration.md](./integration.md) §8.

---

## 9. Open questions

- **Where does the TODO panel live?** Its own tab (defensible: it is user-facing, unlike the agent, which deliberately has no tab), a dock section, or a home widget? A tab costs a `Sidebar` `TabId`, a shell mount, and `app_navigate`'s `VALID` array (18 → 19).
- **Who sets an agent's policy tier?** Moot while the policy is global. If per-agent tiers ever return, the first-connect confirm dialog would offer `read+write` / `full` and the choice would be stored per agent.
- **Should `ui_invoke_action` be allowed to trigger destructive UI actions at all?** Alternative: destructive operations must go through data tools, which have trash. Keeps one undo path instead of two.
- **Does a `human` step block its run?** Blocking is correct for record-then-score pipelines; parallel is correct for "practise these 20 cards while I finish the subtitles". Probably per-step, declared in the YAML.
- **Mobile scope.** The dock and `/mcp` are desktop-only; todos should be fully mobile (they sync, and the phone is where the learner is), while `ui_*` actions only exist where a webview hosts the registry. Decide explicitly rather than inheriting the desktop gate.
