# Hub MCP Compute Offload — a follower scores its take on the hub, over `/mcp`

Design for the situation the [message bus blueprint](./message-bus.md) was written to serve,
solved **without a bus**: an Android device records a read-aloud attempt, calls the hub's
existing MCP surface to run the heavy scoring on the hub's models, saves the result locally,
and lets today's sync do the rest. It answers message-bus.md §11's main open question — *"is a
second server on the hub worth it for 2–4 personal devices, versus a bespoke REST/MCP call for
the pilot?"* — in favor of the bespoke MCP call, and defers NATS to the phases where its
absence is actually felt (§9).

The one idea this document must keep true: **the hub serves compute, the phone stays the
author.** No remote call writes durable user state; every result lands through the same
submit + `@app` journal path a local score would.

---

## 0. The one-sentence model

**A follower posts one MCP request, with the recording inline, to the hub it already trusts;
the hub runs the models it already has, replies with derived scores it persists nowhere, and
the follower saves the attempt exactly as it saves one scored on-device today.**

## 1. Why MCP-over-hub is enough for this

Three facts about the current build make a new transport unnecessary for the pilot:

1. **`/mcp` is already served and already guarded.** The hub's axum listener nests the rmcp
   service ([`sync/server.rs`](../../src-tauri/src/sync/server.rs), `start_inner`) and wraps
   it in the same `zone_guard` as `/api/v1` — one port (35711), one trust-zone model, one
   runtime toggle (`mcp_enabled`, reported in `/api/v1/status` so a peer distinguishes
   "surface off" from "address wrong"). No sidecar, no new inbound surface, none of the
   per-platform packaging cost of message-bus.md §9.
2. **The scoring tool already exists.** [`read_aloud_assess`](../../src-tauri/src/mcp/books.rs)
   scores all three axes *and persists nothing* — the perfect server-side verb: idempotent by
   construction, no trash/undo question, no identity question, safe at the `read` policy tier.
3. **The result path already exists.** A saved attempt rides the `read_attempt_save` journal
   event and the WAV rides the attempt-audio plane (`PUT /api/v1/app/attempts/{uuid}/audio`).
   Nothing in this design adds a write mechanism.

What the bus would add — queue-group dispatch to multiple serving nodes, `fms.notify.*` wake
signals, a workflow-executor seam — is Phase 4/5 machinery for a fleet this design does not
have. A phone cannot hold an inbound connection through Doze anyway (message-bus.md §11
concedes this), so the follower-side pull model is not a downgrade *for the phone*.

## 2. The gap this design closes: STT itself, not just the phoneme axis

Today's phone pipeline ([`useRecorder.ts`](../../src/components/books/useRecorder.ts),
[`PracticePanel.tsx`](../../src/components/read_aloud/PracticePanel.tsx)) does this **on the
device**:

```
record → WAV (16 kHz mono, base64)
       → model_transcribe_detailed   ← the Parakeet model, loaded on the phone
       → save the take, then measure it (§4.3; `read_aloud_submit` is both at once)
                                     ← accuracy + fluency; pronunciation = model_missing (desktop-gated)
       → journal @app + audio outbox (next sync round)
```

So the phone's real cost is not only the missing pronunciation axis — it is *loading and
running a hundreds-of-MB STT model to produce the transcript that every axis is scored from*.
Note that `read_aloud_assess` does **not** run STT: it takes `recognized` + `words` as inputs.
Offloading only `read_aloud_assess` would still leave the model on the phone. The shed-the-
whole-work shape needs one composed tool (§4.1):

```
read_aloud_assess_take: WAV in → transcribe → score three axes → compact result out
```

Two call shapes exist because the *audio* has two natural homes. **Inline** (Variant B, the
live path): the WAV rides the request as base64, one round trip, no sync dependency — you can
score on a hub you have not synced with since. **Pointer** (the replay path): an attempt whose
row and WAV already reached the hub (`attempt_uuid`) is re-scored from the hub's canonical
`<read_aloud_audio>/<uuid>.wav` — no bytes cross the wire twice, and it upgrades attempts
scored before the hub had the models.

## 3. The flow (the live path)

```
Android                                            Hub PC  (/mcp)
────────                                           ─────────────────
1. record take → WAV in memory (base64)
2. (no local STT — the work is shed)
3. local read_aloud_save_take { dataset_uuid,
   text_uuid, wav_base64, recognized } → attempt
   row + WAV in app dirs, journaled, unmeasured
   (§4.3) — the take is now durable
4. MCP tools/call  read_aloud_assess_take
   { dataset_uuid, text_uuid, content, wav_base64 } ──► 5. model_transcribe_detailed(wav)
                                                           pronunciation vs the default clip
                                                           accuracy + fluency from the timings
   ◄── { recognized, words, assessment, headline }   (nothing persisted server-side)
6. local read_aloud_rerecognize onto the saved
   attempt's uuid, with the pre-computed
   assessment (§4.5) → the same row overwritten in
   place, axes stored → XP for the number measured
   here, not at the save
7. next sync round (unchanged): row rides @app,
   WAV rides the attempt-audio outbox
```

**Save first, measure second.** Step 3 is local and fast, so the recording is on disk and in
the attempts list before any network or model is touched. A hub call cancelled minutes in, a
model that will not load, or an app closed mid-flight then leaves a recorded — if unmeasured —
attempt rather than a lost one, and the row's "Assess again" is the way to finish it. The UI
says so in one place each: the attempt card carries a *not measured* chip, and the status
overlay has a single exit ("Not now") because with the take already saved there is nothing
left to rescue or throw away. **XP belongs to the measurement, never to the save** — an
unmeasured take earns nothing until something produces its number, whether that is seconds
later or days later on a retry.

**Degradation is a non-event.** Any failure in 4–6 — hub asleep, `mcp_enabled` off, model or
clip absent, request timed out — falls back to the current on-device pipeline (local STT if a
model is installed, accuracy/fluency, pronunciation unavailable). The user never sees an error
state that does not already exist as "the score you get on the phone today"; the remote call
is an *upgrade attempt*, never a hard dependency.

**Why the phone stays the author.** Saving on the follower keeps `user_key`/identity
semantics simple (no "hub writes an event as the user"), keeps LWW free of phone↔PC clock
skew around a hub-side write, and makes the XP ledger the phone's single decision point.
Re-assessment of the same `attempt_uuid` with a richer result converges through the existing
last-write-wins replay (`apply_remote_save`) — the message-bus invariants INV-B1 (no canonical
state on the transport) and INV-B7 (the ledger stays the truth) hold here trivially, because
the transport is a plain request/reply and the ledger is untouched.

## 4. Changes this design needs

### 4.1 One hub-side tool: `read_aloud_assess_take` (new)

A composition of existing functions, persisting nothing: `model::transcribe_detailed` on the
decoded WAV, then the body of `read_aloud_assess` fed with its own output. Params mirror
`ReadAloudAssessParam` minus `recognized`/`words` (it produces them). The reply carries the
compact `Assessment` (~2 KB, the existing stored-metrics budget), the transcript, the word
timings, and the `headline` line, so both the page and an agent can render it.

- Refuses with an actionable error, in this order: `audio_too_large` (see §6),
  `model_missing` (name which model, point at `model_download`), `no_reference` /
  `stale_reference` (name the text, point at `read_aloud_standard_audio_list`), `hub_offline`
  is the caller's problem, not the tool's.
- Add it to `READ_TOOLS` in [`mcp/policy.rs`](../../src-tauri/src/mcp/policy.rs) — it computes
  and changes nothing. (It does burn CPU; a simple in-flight cap of one concurrent take is
  enough at this scale; the workflow engine is where queueing lives if it ever matters.)

### 4.2 A follower-side MCP client seam (new, small)

The phone needs to speak MCP's streamable-HTTP framing exactly once: `initialize` →
capture `Mcp-Session-Id` → `tools/call`, SSE-body decoding, one transparent re-initialize on
session loss. [`mcp/bridge.rs`](../../src-tauri/src/mcp/bridge.rs) is a working reference
implementation of precisely this loop (it is a stdio→HTTP proxy to `/mcp`), and it documents
every quirk. Land it as one command, `hub_mcp_call { tool, args }`, in `sync/client/`, reusing
`hub_request` — which means **device-signature auth rides every call**, so it works from
outside the LAN trust zone too (phone on cellular via Tailscale), and the hub-side `mcp_audit`
journal records the call like any other agent's. Do not hand-roll a bespoke REST twin for the
built-in path: one door, human UI and agents through it — the agent-friendly principle
inverted (the *app* becomes a first-class client of its own agent surface).

### 4.3 The save and the measure are two commands (built)

`read_aloud_save_take { dataset_uuid, text_uuid, wav_base64, recognized }` makes the take
durable and derives nothing: the WAV lands in the per-user audio dir, the attempt row is
journaled as `read_attempt_save` with `score` 0 and no `metrics`, and its uuid is what every
later step names. `read_aloud_rerecognize` (§4.5) is the measurement that writes onto that
row. It takes the optional `assessment` + `via` pair — "these axes were measured from *this*
WAV by `read_aloud_assess_take` on this device's hub; store them as measured" — and with them
this device runs no model pass at all. Provenance travels: the tool result carries the
STT/phoneme model ids and the clip used, and the measurement stores them in `metrics` beside
the scores, so a later reader knows which machine measured what. Without `assessment`, the
measuring call scores locally, exactly as it always did. No new save path; the scaled-XP rule
still keys off the accuracy score, which is unchanged by the axes (message-bus.md §6.1 step 5
holds verbatim) — it is simply awarded by the measuring call.

`read_aloud_submit` survives as those two composed in one call, for an agent that wants one
atomic step and never a visible gap between the save and its numbers. The UI does not use it:
a call that can run minutes must not be the thing standing between the learner and their
recording.

Because saving and measuring are separate writes, one take produces **two** `read_attempt_save`
entries, and the second must still win on replay: the measurement stamps `updated_at` through
`next_revision`, which guarantees a strictly newer revision than the row already carries even
when both writes land in the same millisecond (sync-v2.md §17.7 — a peer accepts a replay only
on a greater revision, so an equal one is a silent no-op).

### 4.4 Capability advertisement (check, don't build)

Extend the `/api/v1/cluster` `features[]` vocabulary (INV-20's existing mechanism, already
carrying `attempt_audio`, `app_journal`, `dataset_journal`) so a follower can pre-check before
prompting "score on the hub": the hub serves `/mcp`, has `stt` compiled, and has the STT +
phoneme models installed. `read_aloud_model_status` and `model_status` are the agent-readable
twins of the same question. A capability not advertised is never attempted — dispatch to a
remote machine should consult install state first, exactly as message-bus.md INV-B4 requires
of a bus request.

### 4.5 The measuring call is one seam, used twice (built)

§4.3's measurement was born as "Assess again": *re-transcribe this attempt's own recording and
overwrite what was derived from it, leaving the recording alone*. The recording is device-local,
so the work has always been bound to the machine holding the WAV; §4.1's tool changes only
where the transcription runs, so the re-assessment needed no new seam: the follower reads the
attempt's WAV off its own disk and posts it inline exactly as a fresh take does, and
`read_aloud_rerecognize` gained the optional `assessment` + `via` pair — passed, this device
stores the measurement and skips its own model passes; absent, it scores locally as before.
Provenance lands in the same `metrics.provenance.via` key, so the attempt detail says "Scored on
the Hub" whichever flow produced it.

Save-first made that one seam serve **both** flows: a fresh take is measured by the very same
call, one command after its row exists. The consequence for the learner is the point of the
change — walking away from a measurement can never lose anything, because the call only ever
writes numbers onto a row that is already saved, and a row with no `metrics` simply reads
*not measured* until something measures it.

Two decisions worth keeping visible. The **pointer path is still unwired** here: an attempt
whose WAV reached the hub is *not* re-scored by `attempt_uuid`, because the follower already
holds the recording and the inline call is one round trip with no sync dependency (§3's
argument, unchanged). And every entry point onto the action — the attempts list's per-row
button, the attempt detail's, the page toolbar's — shares **one** route, dialog pair and busy
owner: two of them running independently would let a hub reply and a local pass overwrite the
same attempt row in whichever order they happened to land.

## 5. Vocabulary

| Term | Definition |
| --- | --- |
| **Take** | The recording one attempt makes (existing word in read-aloud docs). |
| **Serving node** | The machine running the models. In this design: the hub, only. |
| **Compute request / reply** | One `tools/call` to a hub compute tool and its structured result. Deliberately *not* "job" (a task synonym) and not the bus doc's overloaded "run". |
| **Inline path** | Audio rides the request (`wav_base64`) — the live scoring flow. |
| **Pointer path** | Audio is already on the hub; the request carries `attempt_uuid` — the replay/upgrade flow. |

## 6. Payload reality check (INV-B6's same rule, a different transport)

A 60 s 16 kHz mono take is ~1.9 MB raw, ~2.6 MB as base64 inside a JSON body. HTTP tolerates
this where a 1 MB NATS `max_payload` did not — but three limits must be verified, not assumed:

- the body cap on `/mcp` routes (the attempt-audio plane caps at 32 MB for a reason; the MCP
  service must accept ≥ 4 MB or the inline path dies at the first long take),
- a stated cap on one take's decoded size, with `audio_too_large` as an actionable refusal
  that names the pointer path as the alternative ("sync, then re-score by `attempt_uuid`"),
- memory: base64 + decode both inflate; stream-decode rather than `Vec<u8>` twice if it shows.

The pointer path exists precisely so the inline path is never *forced* — long takes, flaky
links, and re-scoring all belong there.

## 7. What stays deliberately unchanged

- **Sync.** Rounds, the `@app` journal, the audio outbox: unchanged, load-bearing as ever.
  The remote call happens *instead of local compute*, never instead of sync.
- **XP, best_score, trash, mailbox.** Same rows, same rules. A remote-scored attempt is
  indistinguishable from a locally-scored one except in `metrics.provenance`. XP still follows
  the measured score (§4.3), so it lands with the measurement rather than with the save — the
  one behavioural difference the split deliberately introduces, and the reason an unmeasured
  take earns nothing.
- **The Message Bus design.** message-bus.md stands as written; nothing here contradicts its
  invariants — it adopts them over a different transport.

## 8. Testing

- Tool-level: `read_aloud_assess_take` on the hub with a known WAV → same `Assessment` the
  local pipeline produces for the same take (the accuracy axis must match `read_aloud_score`
  to the digit — it already reuses it); absent model/clip → the named `unavailable_reason`,
  never a fabricated score.
- Seam-level: `hub_mcp_call` against a live hub — session initialize/re-initialize, signature
  headers accepted from an untrusted-zone peer (test via non-LAN address), `mcp_audit` line
  present with the requesting device identity.
- End-to-end: record on Android with the hub awake → pronunciation axis populated, XP awarded
  once; hub asleep mid-flow → identical attempt saved via fallback, pronunciation unavailable,
  no error toast beyond the fallback note; same take scored twice → no XP double-award (LWW on
  `attempt_uuid` — the existing replay test covers it).

## 9. Phasing, and when the bus argument reopens

1. **Pilot** — §4.1 + §4.3 + §4.2 minimal (one tool, inline path only), PracticePanel offers
   "score on hub", fallback always on. Re-scoring old attempts adopted the same seam through
   the inline path (§4.5); the pointer path (`attempt_uuid`) is still unwired.
2. **Hardening** — capability pre-check (§4.4), payload limits (§6), provenance surfaced in
   the attempt detail view.
3. **Reopen message-bus.md only when** one of these is actually needed: more than one serving
   node (queue groups), push wake signals with a positive Doze answer for desktop peers, or
   bus-driven workflow fan-out. Until then the bus is an option preserved, not a prerequisite.

## 10. Open questions

- Should `read_aloud_assess_take` also exist as a Tauri command (this-device hub runs the same
  composition locally), or is the MCP tool hub-only by design?
- Timeout budget for one call: STT of a 60 s take on desktop hardware is seconds, but a cold
  model load is minutes — should the tool load-first-and-refuse-if-busy, or carry a
  `load_model` flag like the local pipeline does?
- Does the recorder's *live* transcript preview (currently on-device) also move to the hub
  during recording, or only the final take? (Preview quality vs. per-keystroke HTTP.)

## 11. Related documents

- [`message-bus.md`](./message-bus.md) — the deferred transport; its invariants INV-B1/B4/B5/B6
  are adopted here verbatim, and its §11 open question is what this document answers.
- [`sync-v2.md`](./sync-v2.md) — the journal + authority model this leaves untouched.
- [`dataset-read-aloud-v2.md`](./dataset-read-aloud-v2.md) /
  [`read-aloud-score.md`](./read-aloud-score.md) — the three axes being offloaded.
- [`agent/control.md`](./agent/integration.md) — the `/mcp` surface and trust-zone gating this
  rides on.
