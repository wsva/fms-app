# Agent Control Plane — controlling *all* aspects of the app

Sibling to [ideas.md](./ideas.md): that file covers how an agent **finds and attaches** to FmS; this one covers what it can **do** once attached, and what is missing for a third-party agent (Codex, Claude Code, goose, WorkBuddy, …) to control the whole app rather than answer one quiz.

Status legend, as in `ideas.md`: ✅ built · ⚠️ partial · 💡 proposed.

**The requirement being designed against:** an agent should be able to (1) open a page and click a button, (2) use various models, (3) configure and run workflows, (4) generate a TODO list for the user, jump them to the page, and let *them* do it, and (5) everything else a human can do in the UI — which is the standing rule in [../../agent_friendly_design.md](../../agent_friendly_design.md) §1: *"If a human can do it in the UI, an agent must be able to do it through MCP."*

---

## 1. Two control planes, one of them is tiny

| Plane | Size | Where |
|---|---|---|
| **Data / logic** | ~100 tools | [src-tauri/src/mcp/](../../../src-tauri/src/mcp) — `datasets`, `dictation`, `cards`, `books`, `models`, `ai`, `wiki`, `sync`, `system`, `workflow` |
| **UI control** | **6 verbs** | [mcp/system.rs](../../../src-tauri/src/mcp/system.rs) L280-378: `app_version`, `app_navigate`, `app_open_review`, `app_start_dictation`, `app_notify`, `app_get_ui_state` |

There are **no `ui_*` tools at all**. Every UI capability today is a hand-written verb in Rust, which means each new controllable button costs a backend release. That asymmetry is the whole problem, and §3 is the fix.

The bridge itself already exists and is agent-neutral: the `app_*` tools `emit("agent-action", …)` and the shell performs the action in [src/app/(app)/home/page.tsx](../../../src/app/(app)/home/page.tsx) L122-130 (`navigate` / `open-review` / `start-dictation` / `notify`). Adding control means adding *verbs to that channel*, not building a channel.

---

## 2. Coverage audit

| # | Requirement | Status | Notes |
|---|---|---|---|
| 1a | Open a page | ✅ | `app_navigate`, 18 valid tabs, error names the valid set |
| 1b | Click a button | ❌ | 3 intent verbs only (`open_review`, `start_dictation`, `notify`) |
| 2 | Use various models | ⚠️ | STT / SaT / TTS / OCR complete; **LLM is Ollama-only** |
| 3 | Configure and run workflows | ✅ | 12 tools, incl. authoring new YAML pipelines |
| 4 | TODO for the user + jump to page | ❌ | the concept does not exist; nearest neighbour is `chat_send` |
| 5 | Everything else | ⚠️ | see §5-§7 |

### 2.1 Navigation and intents — ✅ / ❌

`app_navigate` covers the full tab set and its refusal is already the house pattern for actionable errors (`"unknown tab '{}'. Valid tabs: …"`, plus the special case explaining that the agent has no tab because it lives in a dock). Beyond that there are exactly three intents, each hand-coded with its own param struct. There is no generic "press this control", no way to set a field, no way to read what a control would do.

### 2.2 Models — ⚠️ LLM only

- ✅ **STT**, full lifecycle: `model_status` (carries an actionable `hint`, and tells a pipeline-driven agent *not* to call it because `dataset_full_report` already reports `stt_model`), `model_download`, `model_load`, `model_set_default`, `model_unload`, `model_delete`, `model_transcribe`.
- ✅ **Text segmentation**: `text_split`, `text_split_unload` (SaT, `textsplit`-gated).
- ✅ **TTS**: `tts_list_voices`, `tts_synthesize` (Edge).
- ✅ **OCR**: `ocr_recognize`, `ocr_recognize_tsv`, `ocr_hyphen_dictionary_status/download/delete`, language list.
- ⚠️ **LLM**: `llm_check_connection`, `llm_list_models`, `llm_pull_model`, `llm_delete_model`, `llm_chat` — **all Ollama**. Yet [settings.rs](../../../src-tauri/src/settings.rs) already carries `llm_provider` (`ollama | openai | anthropic | groq | databricks`), `llm_api_key`, `llm_model`. The provider dimension exists in config and is absent from the tool surface, and `settings_set`'s description enumerates only the directory/model fields — so an agent cannot discover it either.

💡 Fix: `llm_chat` accepts a `provider` / `model` override, plus a scoped `llm_set_provider`. **Precondition:** `settings_get` currently serializes the entire `AppSettings`, including `llm_api_key` and `goose_acp_secret` — redact before exposing provider configuration to third-party agents (also `ideas.md` §8 step 2).

Reframe worth keeping in mind: the agent *is* an LLM. "Use various models" almost always means letting the agent dispatch **app work** to a chosen model — a cheap one for text normalization, a strong one for read-aloud scoring — not replacing the agent's own inference.

### 2.3 Workflows — ✅ the strongest part of the surface

Twelve tools in [mcp/workflow.rs](../../../src-tauri/src/mcp/workflow.rs): `workflow_list`, `_create`, `_status`, `_reap`, `_next`, `_get_definition`, `_advance`, `_record`, `_intervene`, `_verify`, `_adopt`, `_builtin_templates`. (`AGENTS.md` still lists eleven — `workflow_reap` is undocumented there; fix the drift.)

`workflow_create` **takes a YAML definition**, so an agent can author a brand-new pipeline, not merely run a template — validated by `workflow/core/validate.rs` (unique ids, existing refs, acyclic, `verify:`/`adopt:` ids registered). Execution is lease-based single-flight (`workflow_advance` → perform → `workflow_record`, with `claim_id` protecting against a reaped claim clobbering a live one), and `workflow_verify` joins live probes to each step's `verify:` binding so a probe can retract a claim but never promote one.

Two real gaps:

- 💡 **The action vocabulary is invisible to the agent.** A step's `action` must resolve to a real command, but that vocabulary lives in the *frontend* registry [src/lib/workflow/step-ops.ts](../../../src/lib/workflow/step-ops.ts) (`KNOWN_COMMANDS`, `getStepOp`) — which also powers the Workflow page's "Add step" dropdown. An agent authoring YAML has to guess. Expose it over MCP (or fold it into `app_bootstrap`), generated from one source per `ideas.md` §5.0.
- ⚠️ **`verify:` providers exist for dictation only** — `workflow_verify` explicitly errors for other dataset kinds. Coverage gap, not a defect.

### 2.4 Human tasks — ❌

No task surface exists. The nearest neighbours are all agent-facing or state-only: workflow steps (executed by tools), card review due lists, dictation progress, the XP ledger.

But the app already has the *idea* of handing work to the human — `chat_send`'s own description says: *"Use it to hand something to the user's phone: a reminder, a generated file, the result of a longer job."* That is an unstructured, non-actionable TODO. §4 makes it structured and actionable.

---

## 3. 💡 UI action registry — the fix for "click a button"

One tool per control is the wrong shape: 18 pages × dozens of controls, and every new button needs a Rust release. Register the actions in the UI, invoke them generically.

**The pattern already exists twice in this codebase**, so this is an extension of a house idiom rather than an invention:

1. `step-ops.ts` — `KNOWN_COMMANDS` + `getStepOp` map a step id to a command, args and form fields. A frontend registry of executable actions, already used to render a picker.
2. `agent-prefill` — the UI→agent direction. [WorkflowPage.tsx](../../../src/components/tools/WorkflowPage.tsx) `sendToDock` emits it (L993-1000) to hand a generated prompt to the dock, which prefills its composer.

**Proposed shape:**

```
# registered by each page at mount
{ id: "cards.start-review", label, tab, kind: "read" | "mutate" | "destructive",
  params_schema: {…}, when: <availability predicate> }
```

Two tools: `ui_list_actions` (the registry, with schemas) and `ui_invoke_action { id, params }`, routed over the **existing** `agent-action` channel and the existing shell bridge. Nothing new in transport, and the `kind` field feeds the policy tiers in §6.

**Two constraints to design around, both already visible in the code:**

- **`agent-action` is fire-and-forget.** `app_get_ui_state` works around this by caching UI state in Rust (`agent_report_ui_state` writes, the tool reads). Start with *observe, don't return*: invoke, then read the effect back through existing read tools (`app_get_ui_state`, `dataset_get`, `workflow_status`). Add a correlation-id response path — a `agent_action_result` command the webview calls, with a Rust-side timeout — only for actions whose outcome genuinely cannot be observed.
- **The registry only exists after the webview mounts.** `ui_list_actions` before mount returns empty, which is indistinguishable from "no actions". It must return an actionable message ("the UI is not mounted yet; `app_get_ui_state` reports whether it is") rather than `[]`.

**A favourable consequence of the existing architecture:** pages are display-toggled, not unmounted (`display: none/flex`), so a page registers its actions once at mount and they **survive navigation**. The registry is therefore "everything the app can do right now", not "what the visible page can do" — which is what makes `ui_list_actions` useful as a capability manifest.

**Discovery must not be duplicated:** the registry feeds `ui_list_actions`, the action index in `/agent.md`, and `app_bootstrap` — one source, three renderings (`ideas.md` §5.0). A hand-written list of action ids in a doc would drift on the first rename.

---

## 4. 💡 TODO domain — human-in-the-loop as a first-class step

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

## 5. 💡 Reading the app: context, screen, and events

### 5.1 `app_get_ui_state` cannot see the dataset — fix first

`active_dataset_uuid` is **always null**: the dock hardcodes it ([AgentDock.tsx](../../../src/components/agent/AgentDock.tsx) L400-403, `agent_report_ui_state({ activeTab, activeDatasetUuid: null })`). So an agent knows the *tab name* and nothing else about what the user is looking at. That single hardcode caps every "do something with what I'm on" request — the most common thing a user will actually say.

Extend the report to `{ tab, dataset_uuid, media_uuid, cue_index, selection, scroll }`. Smallest change on this list, largest unlock; do it before anything else.

### 5.2 Screenshots are not a UI-inspection tool today

`capture_screenshot` exists and returns a `data:image/png;base64` URL — but its description says it **minimizes the app window** first (native snipping tool on Windows, full-screen elsewhere). So it cannot show the agent the app's own UI. Capturing without minimizing, paired with the existing `ocr_recognize`, would give a see-then-act loop for state that no read tool exposes. Low priority, but it is the only way to verify rendered output.

### 5.3 One event timeline instead of per-domain polling

Agents poll today: `chat_messages(after=…)`, `workflow_status`, `dataset_full_report`. 💡 `app_events(after=cursor)` over the existing in-memory `LogBuffer` (which `log_*` tools already read) gives one timeline. The `after=` cursor convention is established by `chat_messages`.

---

## 6. 💡 Policy — the prerequisite for broad control

This is the caveat that must land *with* full app control, not after it.

**The problem, precisely:** `sync/server.rs::zone_guard` treats loopback (with a local `Host` header) as **Trusted with no credentials at all** — `AuthContext::trusted()`, documented as *"this is the agent path"*. Meanwhile FmS's only approval policy, `ai/agent_acp.rs::auto_approve_option` (the seven-word destructive list `delete|remove|clear|drop|revoke|reset|rebuild`, preferring `allow_once`/`allow_always`), lives **in the ACP client** and therefore only runs in dock mode. A third-party agent in attach mode bypasses it completely.

So today, "control all aspects" from attach mode means: **any local process can delete datasets.** A trusted network position is not the same claim as "may destroy".

**Proposal:**

- Enforce policy **on `/mcp`**, not in the dock. Tiers: `read` · `write` · `destructive` · `ui-control` · `human-handoff`. The registry's `kind` field (§3) and the tool names supply the classification.
- Per-agent, from the connection record in `ideas.md` §2 — not global. Default `read + write`; `destructive` and process-spawn require the confirm dialog (`ideas.md` §3).
- Additive to `AuthContext`, not a replacement: the trust zone answers *who may talk to us*, the policy answers *what they may do*.
- 💡 Close the undo loop: trash already exists for datasets and books, and `agent_friendly_design.md` §3 anticipates *"a future `restore_from_trash` tool"*. `app_trash_list` + `app_restore` let an agent recover its own mistakes — which is what makes a permissive default defensible.
- Journal every destructive call with the agent id. The workflow engine's append-only `events.jsonl` is the precedent for the format.

---

## 7. Other ideas, small and high-leverage

- 💡 **`app_prefill_prompt { text, send? }`** — expose the existing `agent-prefill` event as an MCP tool, so a third-party agent can queue work *into the dock*. Makes the dock a shared scratchpad between agents, for free.
- ✅ **Keep the `app_navigate` error style everywhere.** Naming the valid set and the next call is what turns a failure into a lesson; it is the cheapest way to teach an agent that has never seen the app.
- 💡 **`workflow_list_actions`** (or fold into `app_bootstrap`) — the step-action vocabulary from `step-ops.ts`, so authored YAML validates first try (§2.3).
- 💡 **MCP notifications** later, if `rmcp`'s server surface allows — the alternative to `app_events` polling.

---

## 8. Sequencing

1. 💡 **`active_dataset_uuid` + extended `ui_state` report** (§5.1). Tiny; unblocks every context-aware request.
2. 💡 **`/mcp` policy tiers + redact secrets from `settings_get`** (§6). Safety prerequisite — must not follow broad control.
3. 💡 **UI action registry**: `ui_list_actions` / `ui_invoke_action`, observe-don't-return (§3).
4. 💡 **TODO domain** + the `human` workflow step (§4).
5. 💡 `llm_chat` provider/model override; workflow action vocabulary (§2.2, §2.3).
6. 💡 `capture_screenshot` without minimizing; `app_events(after=)`; `app_prefill_prompt`; `app_trash_list` / `app_restore`.
7. Deferred: correlation-id action results, MCP notifications, the visual see-then-act loop, per-page action authoring UI.

Steps 1-2 are small and independent; 3-4 are the substantive features and can be built in parallel by different people since they touch different layers (registry = frontend + two tools; todo = new domain + DB table + panel).

---

## 9. Open questions

- **Where does the TODO panel live?** Its own tab (defensible: it is user-facing, unlike the agent, which deliberately has no tab), a dock section, or a home widget? A tab costs a `Sidebar` `TabId`, a shell mount, and `app_navigate`'s `VALID` array (18 → 19).
- **Who sets an agent's policy tier?** Proposed: the first-connect confirm dialog offers `read+write` / `full`, stored on the connection record. Per-agent, not global.
- **Should `ui_invoke_action` be allowed to trigger destructive UI actions at all?** Alternative: destructive operations must go through data tools, which have trash. Keeps one undo path instead of two.
- **Does a `human` step block its run?** Blocking is correct for record-then-score pipelines; parallel is correct for "practise these 20 cards while I finish the subtitles". Probably per-step, declared in the YAML.
- **Mobile scope.** The dock and ACP are desktop-only; todos should be fully mobile (they sync, and the phone is where the learner is), while `ui_*` actions only exist where a webview hosts the registry. Decide explicitly rather than inheriting the desktop gate.
