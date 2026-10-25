# Agent Integration Ideas — the self-describing app, zero-config connect

This file collects **proposals** for FmS's agent-facing surface. It is not a spec: [../../agent_friendly_design.md](../../agent_friendly_design.md) holds the standing principles, and this directory already holds real artifacts ([goose_recipe/](./goose_recipe/), [prompt/](./prompt/)). Its sibling [control.md](./control.md) covers the other half — what an agent can *do* once attached. Everything below is marked by status:

- ✅ built and working in this codebase today
- ⚠️ partial — the mechanism exists but is incomplete or undocumented to agents
- 💡 proposed, not built

**The scenario being designed for:** a user who is not an AI/agent expert installs some agent product — goose, Claude Code, Codex, OpenCode, WorkBuddy, something that does not exist yet — and types *"connect to FmS and give me a quiz on my due cards"*. They have not read a manual. They do not know what a port number is. Nobody configures anything.

Three commitments follow, and they are the whole document:

- **C1 — the user never types an address.** Connection configuration is *removed* from Settings. Today Settings → "Agent (Goose ACP)" exposes three hand-filled fields; that UI is deleted, and what replaces it is a **read-only Connect page** the user copies *from* (§4.1).
- **C2 — the manual is an endpoint, not a document.** Everything an agent needs to attach itself is served by the *running* app, generated from live state, so it cannot drift and needs no maintenance per agent product.
- **C3 — discovery is out of scope (§6).** The card states the address; the agent goes there. No mDNS, no port probing, no PATH probing, no writing other products' config files.

Then the user makes exactly one choice, which is a *rendering* choice, not a configuration one:

1. **Attach mode** — keep using the third-party agent's own UI; FmS is just its tool server.
2. **Hosted mode** — the agent renders inside FmS's built-in Agent Dock.

**Scope boundary:** this file stops at *attachment* — being found, being understood, being trusted. Capability after attachment (UI control, model choice, workflow authoring, human tasks, and the tool policy that makes broad control safe) is [control.md](./control.md).

---

## 1. What exists today

| Surface | Where | Notes |
|---|---|---|
| ✅ MCP server | [src-tauri/src/mcp/mod.rs](../../../src-tauri/src/mcp/mod.rs) — `rmcp` 3.4, Streamable HTTP | ~230 tools across 12 domain submodules, merged by `DatasetMcpServer::tool_router()` (the live count and names are rendered into `/agent.md`, never written by hand) |
| ✅ MCP resources + prompts | `mcp/resources.rs`, `mcp/prompts.rs` | `fms://status`, `fms://docs/{start,principles,domain/*}`, `fms://workflows/templates`, `fms://datasets/{uuid}/layout`; the prompts are the [goose_recipe/](./goose_recipe/) YAML recipes, parsed on demand |
| ✅ Auto-started endpoint | [src-tauri/src/sync/server.rs](../../../src-tauri/src/sync/server.rs) `build_router` → `.nest_service("/mcp", …)`, `DEFAULT_PORT` 35711 | Runs on app launch with no user action; real bound port via `WebServiceState::build_status().port` |
| ✅ Open pre-auth probes | [src-tauri/src/sync/rest.rs](../../../src-tauri/src/sync/rest.rs) `status`, `cluster` | `GET /api/v1/status` → `version, app_name, dataset_count, protocol_version, role, cluster_id, machine`; reachable even from the untrusted zone (`zone_guard`) |
| ✅ Trust zones | `sync/server.rs::zone_guard`, [src-tauri/src/sync/pairing.rs](../../../src-tauri/src/sync/pairing.rs) | Loopback + local `Host` header ⇒ Trusted, **no credentials** — that is the agent path. Tailscale CGNAT likewise trusted |
| ✅ Human-readable index | `sync/server.rs::index_handler`, `MCP_SECTION_HTML` | The root page already lists endpoints and the MCP tool table — an agent with web fetch can read it today, as HTML prose |
| ⚠️ ACP client (the dock's backend) | [src-tauri/src/ai/agent_acp.rs](../../../src-tauri/src/ai/agent_acp.rs) `agent_connect` | **WebSocket only**, dialed from `AppSettings.goose_acp_url` + `goose_acp_secret`; registers our MCP URL in `session/new` **only if** `agentCapabilities.mcpCapabilities.http` — otherwise the agent is handed `mcpServers: []` and no tools |
| ✅ App-control tools | [src-tauri/src/mcp/system.rs](../../../src-tauri/src/mcp/system.rs) `app_navigate`, `app_open_review`, `app_start_dictation`, `app_notify`, `app_get_ui_state` | Emit `agent-action`; performed by the shell bridge in [src/app/(app)/home/page.tsx](../../../src/app/(app)/home/page.tsx) |
| ✅ Dock UI | [src/components/agent/AgentDock.tsx](../../../src/components/agent/AgentDock.tsx) | Overlay dock, draggable FAB, Ctrl+Space, auto-open on permission, `agent-prefill` extension point. Consumes only `agent-*` events — already agent-neutral |
| ✅ The Connect card | [src/components/agent/AgentPage.tsx](../../../src/components/agent/AgentPage.tsx) → Connect tab | Status half (bound port, zone, dock state), the pointer sentence, the inline block, and copy-only per-product lines. It replaced the three hand-filled `goose_acp_*` fields; one `goose_acp_enabled` checkbox survives on the Surface tab |
| ✅ Config write path for agents | `mcp/system.rs::settings_set` | A **patch**, not a whole-object write: only the fields present are touched, so a partial payload is the correct payload. Credentials are write-only (`***` means "leave it alone"), and `settings_get` redacts every secret in `SECRET_FIELDS` while keeping `has_<field>` discoverable |
| ✅ Recipe / prompt assets | [goose_recipe/](./goose_recipe/), [prompt/](./prompt/) | Goose YAML envelope + portable `instructions:` prose over `dataset_*` tools |

### 1.1 What is still missing

Everything the *capability* track needed is built (see [control.md](./control.md)); what remains is attachment:

- ⚠️➜💡 **Mode 2 (hosted dock) — `agent_host_register` does not exist.** The dock dials a user-started `goose serve` over WebSocket and nothing else, so hosting an arbitrary ACP agent still needs the transport split (`ws | stdio`) plus a host record and a trust dialog (§3, §8 step 5).
- 💡 **No stdio↔HTTP bridge.** `rmcp` is compiled without `transport-io` ([src-tauri/Cargo.toml L158](../../../src-tauri/Cargo.toml#L155-L165)), and the app serves MCP over Streamable HTTP only. An agent that speaks stdio MCP — which includes several CLI products, and every ACP agent that does not advertise `mcpCapabilities.http` — therefore gets no tools at all. A thin proxy fixes both classes at once, which is why it is step 5a rather than per-product work.
- ⚠️ **A single `goose_acp_enabled` checkbox** is the only dialect-setting left on the Settings page; the host list in step 5 removes it.

---

## 2. The inversion: the agent configures itself, and configures us

Today the human is the integration point: they must know the URL, the port, the secret, and each product's config syntax. The proposal keeps the human but shrinks their job to **one copy-paste**, sourced from a page inside the app — see §4.1 and §6. Everything after the paste is the agent acting on instructions the app publishes, including writing FmS's own connection record **through FmS's MCP surface**.

**The flow, from the user's side (one copy, one paste):**

1. User opens the app's **Connect** card, clicks copy, pastes into any agent:
   *"Read http://127.0.0.1:35711/agent.md and connect to FmS as it instructs."*
   The address on that card is the **live bound port**, not a guess — which is the whole reason copy-paste beats probing (§6).
2. Agent `GET`s the manual — generated, live, from the running app.
3. Manual gives the agent its MCP URL, its trust zone echoed back, the tool index, and **both paths**:
   - *attach only* → "you have your own UI; here is your MCP config for your product." Done. Mode 1.
   - *host in the dock* → "call `agent_host_register` with your ACP endpoint (or your launch command), then tell the user to press the Bot button." Mode 2.
4. In mode 2 the agent calls `agent_host_register`; FmS stores the record, applies the trust rule (§3), and `agent_connect` dials it. The dock then renders *that* agent — same `agent-*` events, same permission cards.

**The config record** (replaces the three `goose_acp_*` scalars), one per agent:

```
{ id, display_name,
  transport: "mcp_only" | "acp_ws" | "acp_stdio",
  endpoint:  ws url                       # acp_ws
  command:   { program, args, env, cwd }   # acp_stdio
  learned:   { mcp_http, protocol_version, … }   # from initialize, not typed by a human
  enabled, last_session }
```

Naming: `goose_acp_*` → `agent_host_*`. Keep serde aliases so old `settings.json` still parses — the same technique `pc_token` uses ("Kept only so old persisted settings JSON keeps parsing").

### 2.1 Resolve the two-representation ambiguity first

`AppSettings` carries `goose_acp_*` **and** `GlobalSettings` carries the same three fields; `GlobalSettings::from_settings(&AppSettings)` derives the projection, and `agent_connect` reads `AppSettings` — while the Settings UI edits the *projection*. So there are two copies of a dial target with one writer and one reader that disagree depending on save order. Whatever gets built here must declare **`AppSettings` as the authority the dialer reads**, and route writes through the new scoped command rather than through raw `settings_set`. This is a prerequisite, not a cleanup.

### 2.2 New commands and their MCP twins

Per the standing parity rule in [AGENTS.md](../../../AGENTS.md) ("Adding a new Tauri command → simultaneously add the corresponding MCP tool"):

| Command / tool | Purpose |
|---|---|
| 💡 `app_bootstrap` | Everything an agent needs in one call: identity, version, real endpoints, caller's zone, `capabilities` from `cfg!(feature = …)`, tools-by-domain, resource/prompt index, recommended first calls, current workspace + UI state |
| 💡 `agent_manual_get` | The same body as `/agent.md`, for an agent already inside MCP |
| 💡 `agent_host_register` / `_connect` / `_disconnect` / `_status` / `_unregister` | The mode-2 lifecycle, replacing raw `settings_set` writes of the dial target |
| 💡 `agent_list` | Which agents are attached, which one hosts the dock — feeds the Settings status panel and answers "how do I get back to my own window?" |

Why a scoped command instead of documenting `goose_acp_url` in `settings_set`'s description: `settings_set` is a **whole-object read-modify-write**, so a partial payload fails on missing fields, and a scoped command is the only place the trust rule (§3) can be enforced. Also redact secrets from `settings_get`.

---

## 3. Trust: the one thing that must not be zero-config

Data writes are recoverable (trash, not confirm — agent-friendly §3). But `agent_host_register` makes FmS **dial an arbitrary address or spawn an arbitrary process**. That is a security boundary, and `zone_guard`'s "loopback is the agent path" posture covers agents reaching *in*, not FmS reaching *out*.

Proposed rule, deliberately a partial exception to "don't block agents":

| Registration | Behavior |
|---|---|
| `ws://127.0.0.1` / `localhost` / `::1` | accept silently — same posture as today's default |
| `acp_stdio` (we spawn the agent) | **confirm dialog always** — arbitrary process execution |
| non-loopback endpoint | confirm dialog naming `host:port` |

Reuse the existing pattern rather than inventing one: `pair/request` pops a confirm dialog on the owner's machine and the caller polls `pair/status` ([src-tauri/src/sync/pairing.rs](../../../src-tauri/src/sync/pairing.rs)). Log every register/connect with the peer address.

Keep the interruption to one click and make the refusal actionable ("approve in the dialog, then retry `agent_host_connect`") so it stays agent-friendly in spirit even though it is blocking.

---

## 4. Mode 1 vs Mode 2 — a rendering choice, surfaced in the UI

| | Mode 1 — attach (agent's own window) | Mode 2 — hosted (FmS dock) |
|---|---|---|
| User's UI | the product's native one (plan mode, diffs, slash commands) | the dock, over the page they were already on |
| FmS's role | MCP server only | MCP server **+** ACP client |
| `app_*` control tools | ✅ work — `agent-action` still navigates/toasts | ✅ work |
| Streaming text, thoughts, tool-call cards, plans | ❌ invisible to FmS | ✅ `agent-message-chunk` / `agent-thought` / `agent-tool-call` / `agent-tool-update` / `agent-plan` |
| FmS-side permission card + `auto_approve_option` policy | ❌ not exercised | ✅ [agent_acp.rs L562-592](../../../src-tauri/src/ai/agent_acp.rs#L562-L592) |
| Voice prompt | ❌ | ✅ `VoiceMicButton` |
| Requires the agent to speak ACP | no | **yes** — via stdio or WS |
| Conversation history lives | in the product | in the product's session; the dock transcript is in-memory `useState` only |

Mode 2 **requires** the transport generalization already identified: `agent_connect` is WebSocket-only (`tokio-tungstenite`), while Claude Code / Codex / OpenCode ACP adapters are spawned stdio processes. Split the transport setup (currently [L195-L209](../../../src-tauri/src/ai/agent_acp.rs#L195-L209)) from the `initialize` → `session/new` sequence — everything after L248 is already transport-agnostic.

And the blocker shared by *both* modes and both transports: `rmcp` is compiled without `transport-io` ([src-tauri/Cargo.toml L158](../../../src-tauri/Cargo.toml#L155-L165)), so when an agent does **not** advertise `mcpCapabilities.http` we hand it `mcpServers: []` and it gets no tools at all. A thin stdio↔HTTP bridge sidecar fixes it once for every product. This is worth doing before any per-product work, since it silently degrades mode 1 today.

### 4.1 The Connect page — copy out, never configure in

The page has exactly two jobs: **let the user copy what an agent needs**, and **show what is currently connected**. It contains no address input. The three `goose_acp_*` fields and the "Test connection" button are deleted.

**The copy half — two payloads, because agents differ in what they can do with one line:**

- **The pointer** (default): `Read http://127.0.0.1:<live port>/agent.md and connect to FmS as it instructs.` One line, works with any agent that has web fetch or a shell, and the manual then teaches everything — including mode 2.
- **The inline block** (fallback): a self-contained snippet — MCP URL + transport + the mode-2 instruction + the recommended first calls — for agents that **can attach an MCP server but cannot fetch a URL**. Without this the scenario fails silently for that entire class, so ship both, not just the pretty one-liner.
- Optional convenience lines per known product: `claude mcp add --transport http fms <url>`, a Codex `[mcp_servers.fms]` TOML block, an OpenCode `opencode.json` block, a WorkBuddy `~/.workbuddy/mcp.json` block that rides the stdio bridge (its remote `type` spelling differs between builds) with the trust step in a note. Displayed for copy only — FmS never writes into another product's config files (§6).

**The status half:**

- attached agents — name, transport, capabilities **learned at `initialize`** (never typed), last session
- which agent hosts the dock, with a **one-gesture switch**: *"Shown in: FmS dock ▾ / Claude Code window"* → `agent_host_disconnect` / `agent_host_register`
- no address input anywhere; `default_goose_acp_url` survives only as a fallback probe

**Placement: a Settings section first, not a new tab.** The section already exists ([SettingsPage.tsx "Agent (Goose ACP)"](../../../src/components/settings/SettingsPage.tsx) ~L692-768), is already `!mobile`-gated, and Settings already has the collapsible section sidebar for navigating it. Promoting to a first-class tab costs a Sidebar `TabId`, a shell mount, and `app_navigate`'s `VALID` array (18 → 19) plus its description string — for no functional gain on day one. Note also that once *any* agent is connected, it can bring the user to the card itself via `app_navigate { tab: "settings" }`, so the "user can't find the page" problem largely solves itself after first connect. Promote to a tab only if the card must be reachable mid-workflow before a connection exists.

---

## 5. What the manual must contain

### 5.0 One generator, three renderings

The Connect card, `/agent.md`, and `app_bootstrap` are **the same object rendered three ways**, built from the live `tool_router()` registry, `workflow::templates`, and `WebServiceState::build_status().port`. A fourth hand-written copy — per-product snippet text, or a hard-coded port — would drift on the next port conflict or tool rename, so none is allowed. If the page says 35712 and the manual says 35711, the design has already failed.

The shared body, **generated** and never `include_str!`, so it cannot state a tool that does not exist:

1. identity: name, `git_describe` + `package_version` (already surfaced by `app_version`), `PROTOCOL_VERSION`
2. real endpoints with the **bound** port, never an assumed one
3. the caller's own trust zone, echoed — "you are loopback, no credentials needed"
4. `capabilities`: which feature-gated tool families exist in this build
5. the tool index: count per domain + names, and the recommended first five calls
6. **the two paths**, spelled as calls: attach-only, and `agent_host_register` for the dock
7. the rules that make an agent correct here: datasets are uuid-keyed, prefer trash over destructive confirms, workflow `verify:`/`adopt:` are the per-step read path, `dataset_audit` is subject-only, some tools are desktop-only
8. error conventions: every error names the valid set and the next call — the `app_navigate` error string already does this and is the model
9. MCP `resources/list` + `prompts/list` index
10. per-product snippets + the generic paste sentence

Also deliver item 3-7 through **`initialize` `instructions`**: a few lines of prose that every conforming client receives without asking. Highest leverage per byte in this whole document.

### 5.1 Resources and prompts retire the goose lock-in

- 💡 **Resources**: `fms://docs/start`, `fms://docs/domain/{datasets,cards,workflow,sync}`, `fms://docs/principles`, `fms://status`, `fms://workflows/templates`, `fms://datasets/{uuid}/layout`. An MCP client that supports resources discovers these with zero prior knowledge — this *is* "learn everything from the app".
- 💡 **Prompts**: the [goose_recipe/](./goose_recipe/) YAMLs already have exactly MCP's prompt shape — `title`, `description`, `instructions`, `parameters`. Publish them once as prompts and **every** MCP client renders them as user-invocable commands. The goose envelope stops being a dependency, and no per-product slash-command porting is ever needed.

---

## 6. Copy-out replaces discovery

**Decision: the app's own Connect page is the entry point.** The user copies the address from it; nothing is probed. This is not a simplification of the discovery subsystem — it deletes it, and with it the fragile parts.

| Removed | Why it was fragile |
|---|---|
| mDNS / DNS-SD advertisement + browsing (`_mcp._tcp.local`) | needs to work on Windows/macOS/Linux resolvers, and most agents can't browse for services anyway |
| `/.well-known/` probing as a *requirement* | probing exists only because the address is unknown; the card states it. `/agent.md` stays — as the address written on the card, not a path the agent must guess |
| PATH-probing installed products (`which claude`, `codex`, `opencode`) | binary names and locations are someone else's moving target; a wrong guess is a confusing UI |
| **Writing third-party config files** (`~/.claude.json`, `~/.codex/config.toml`, `opencode.json`) | parsing and merging another product's TOML/JSON risks corrupting its config, and every product changes its schema. Highest risk in the old plan, zero necessity now |

And copy-paste is **strictly more truthful than probing** on the hard case: the default `35711` can be taken, in which case the service binds elsewhere. A probe has to guess, retry, and handle ambiguity; the card just reads the live bound port off `WebServiceState::build_status().port` and states it. The single hardest discovery edge case disappears by construction.

What survives:

- ✅ `/agent.md` as the fetch target — reached by a stated address instead of a guessed one
- ⚠️ Publishing the **real** bound port everywhere it matters (card, manual, `app_bootstrap`, `app_version`) — this is now load-bearing rather than defensive
- 💡 Deep link `fms-app://connect?…` demoted to an optional handoff for re-connect and troubleshooting; the login flow already proves the per-platform registration pattern if it's ever wanted
- **Do not reuse the private UDP beacon** on 35712 (`spawn_discovery`, `"fms-probe"`) for third parties — FmS-to-FmS only, undocumented, and no external agent knows it exists

---

## 7. Limits, stated plainly

- Two distinct preconditions, not one: *can attach an MCP server* and *can fetch a URL*. An agent with the first but not the second is exactly why the Connect card carries the inline block alongside the pointer sentence (§4.1) — otherwise the scenario fails silently for that class. A product with **neither** — a closed GUI, no extension point — cannot be taught; that is a fact about the product, not a gap in this design. The floor for it is deep links, clipboard handoff, and exported prompt files. Declining is a decision.
- Products with no ACP support are mode 1 forever. Correct outcome, no work.
- **Rule for future adapters: only build one for a protocol with two or more independent implementations** (ACP, MCP qualify; a single vendor's private SDK does not). Otherwise each product's quirks leak into `agent_acp.rs` as N branches, which is the treadmill C1 exists to avoid.
- A PTY terminal panel (xterm.js + Rust pty) is the catch-all renderer for TUI-only products: zero per-product code, full native UX, no structured events. Separate decision, recorded here so it is not re-litigated later.

---

## 8. Sequencing

1. ✅ **The Connect card + generated `/agent.md`, from one source** (§5.0), with both payloads (pointer + inline block).
2. ✅ Preconditions: secrets redacted from `settings_get`; the card, the manual and `app_version` all report the **real bound port**.
3. ✅ `app_bootstrap` + `initialize` `instructions` — the same generated body, third rendering.
4. ✅ MCP **resources** (domain docs) and **prompts** (recipes) — which ended the goose-envelope lock-in.
5. 💡 `agent_host_*` commands + profile list + transport generalization (`stdio | ws`), **and the stdio↔HTTP bridge first** — without the bridge, a hosted stdio agent is given an empty `mcpServers` and can touch nothing, which is a worse first impression than no dock. Delete the last `goose_acp_enabled` field; the card's config inputs become the status half.
6. 💡 Trust confirm dialog for stdio spawn and non-loopback endpoints.
7. Deferred: mDNS/DNS-SD, PTY panel, concurrent multi-agent dock sessions — and **permanently dropped**: PATH probing and writing other products' config files.

Decide early in step 5: **one active dock host** (simple; the dock is one panel) or concurrent `HashMap<agent_id, Session>` (the current `AcpClientState` holds a single `session_id: Mutex<Option<String>>`). Prefer one active host and make switching cheap — multi-session is a different product.

This is the **attachment** track. The **capability** track — what the agent can do once connected — is sequenced separately in [control.md](./control.md) §8. The two tracks share exactly one hard dependency: control.md's step 2 (`/mcp` policy tiers + redacting secrets from `settings_get`) must land before attach mode is advertised to third-party agents, because §3's trust rule only governs what FmS dials *out* to, not what an attached agent may do *in*.

---

## 9. Open questions

- Is `/agent.md` served to untrusted peers too? Proposed **yes** — it is protocol information, not data; datasets and writeback stay behind `zone_guard` unchanged.
- Does mode 2 keep the agent's own window open as well, or should the app ask the agent to hide? (Cosmetic, but users ask.)
- Where does the secret go in a stdio world? A spawned agent has no transport secret; a WS one does. `learned` fields must be per-transport, and the confirm dialog is the only place a human sees what is being trusted.
- Should `app_bootstrap` be a tool *and* a resource (`fms://status`)? Probably both — the resource is how non-tool-using clients read it.
- Section or tab for the Connect card? Proposed: **section now, tab later if needed** (§4.1) — but settle it before the copy text exists in two places.
- Does the card need a QR code? Only when the agent runs on a **different** machine (a phone or laptop agent on the LAN); on the same machine copying text is strictly better. Cheap, low urgency — but it is the one case where copy-paste has real friction.
- If the card is the only entry point, what does the user see when the app is on a non-default port *and* the agent is remote? The card states the address, but the trust zone changes (§3) — the card should say which zone the reader is in, or the manual's echoed-zone line becomes the only place they learn it.
