# Product Strategy: The Workshop and the Gym

How FmS should be shaped as a product — who it is for, what each surface is *for*,
what we measure, what we say no to, and the order to do it in. This is a framing
document: it is the test a new surface or feature is checked against, not an
implementation spec. Implementation lives in the design docs it names
(`tab-strip.md`, `hub-mcp-compute-offload.md`, `read-aloud-score.md`, `sync.md`).

It is grounded in the code as it stands: the shell (`src/app/(app)/home/page.tsx`),
the page host (`src/components/layout/PageHost.tsx`), the tab strip
(`src/lib/shell/tabs.ts`), the mobile navigation (`src/lib/mobile-nav.ts`,
`src/components/layout/BottomNav.tsx`), the refresh registry
(`src/lib/refresh/page-refresh.tsx`), the read-aloud practice flow
(`src/components/read_aloud/`), the feature gates in `src-tauri/Cargo.toml`, and the
generated Android project (`src-tauri/gen/android/`). §10 records the specific code
facts, with paths, so every claim here can be re-checked, and §13 is a background primer
for a reader who cannot open the repository.

> **Reading this cold, without access to the repository?** Go to **Appendix A (§13)**
> first. Everything below assumes you know what FmS is, what its surfaces are called and
> what runs on which device; the appendix supplies all of that, and no later section
> repeats it.

**Revision.** §4–§8 and §12 were amended in November 2026 after an external product review
of the first draft. **§14** records what was taken from that review, what was refused and
why, and what the reviewer got wrong about this codebase; the code facts that settled those
points are §10.7–§10.11. Where this document now contradicts its own first draft, the
amended text is the position.

---

## 1. What this document is, and is not

**Is.** A statement of the product's core split, the jobs it serves, the shape each
platform should take as a consequence, the metrics the team should be able to quote,
a list of things we deliberately decline, and a litmus test for future features. For a
reader with no access to the code it is also self-contained, via Appendix A (§13).

**Is not.** A navigation implementation spec (that is `tab-strip.md`, amended by §4
and §10 below), an API contract, a schedule, or a promise of the roadmap dates in §8.
Where this document contradicts an existing design doc about *product shape*, this one
wins; where it contradicts one about *mechanism*, that doc wins and this one should be
corrected.

---

## 2. The thesis

> **FmS is a local-first speech-practice system with two bodies: a workshop and a
> gym.** The workshop turns text and audio into practice material you trust. The gym
> is where you actually speak, for five quiet minutes at a time.

The product succeeds when the gym is effortless *because the workshop did the
thinking*. It fails when the gym starts asking the learner workshop questions.

This split is not new thinking and it is not a slide. It is already the law of the
backend: `desktop` gates the align, adjust, audit, report, text-split, OCR, capture,
trash and sync-server modules, and Android builds
`--no-default-features --features stt`. **The Rust layer has already decided what
belongs on a phone. The frontend has never been told.** Almost every usability problem
worth fixing lives in that gap.

The corollary that makes this a strategy rather than a styling opinion: the workshop
and the gym have *different customers*. The curator and the learner are often the same
person in this household, but they are different modes of that person, with different
patience, different attention, and different failure costs. Losing half-typed dictation
answers is an annoyance at a desk. Losing a recording you just made, standing up, on a
train, is the reason the app gets deleted.

---

## 3. Three jobs, one codebase

| Who | Job to be done | Native surface | What they must never have to understand |
|---|---|---|---|
| **Curator** | Turn a book or a podcast into practice material I trust: subtitles, cues, waveforms, standard audio, alignment | PC, the full cockpit | — (this surface *should* be dense; it is a tool) |
| **Learner** | Get better at speaking, on a habit, in the time and place I have | Phone: Today, Library, Progress, Device — with Practice pushed full-screen over them | datasets, models, cue alignment, sync topology, hub vs follower, base vs overlay |
| **Operator** | Run the workshop and prep the learner's day, from outside the UI | MCP, equal to the UI | — (the agent is a first-class user, per `docs/agent_friendly_design.md`) |

The error this document exists to prevent is treating those as "the same app with more
or fewer tabs". They are different products sharing a data model. The phone is not a
remote control for the PC, and it is not a shrunken PC.

Evidence that we currently ship one product's IA to all three jobs:
`PageHost` knows twenty page kinds; `mobile-nav.ts` puts thirteen of them in the
phone's navigation, and the drawer organises them **by capability group** — "Cross
Device", "Tools" — which is how a workshop is filed, not how a learner thinks.

---

## 4. The phone: what to design

### 4.1 Five surfaces, one job each

| Surface | Job | Shape |
|---|---|---|
| **Today** | "What next, and how much of it is done" — due card reviews, the texts my last attempts went badly on, in-progress material, whatever the curator or an agent filed for me | A **view** over the rows §4.4 names, never a fourth store. No calendar: a rolling daily window and one setting, *minutes per day*. Ends in a deliberate **"You're done for today"** state, with an optional keep-going. |
| **Practice** | Do one exercise: microphone, reference text, waveform, stop, score. This is where every second of the product's value is created, so it takes the whole viewport and the whole attention budget | **Entered by push, hides the bottom bar, exits by pop** to where it came from. It is not a destination in the navigation |
| **Library** | Choose material myself: Read Aloud, Dictation, Cards, Books, Wiki | For the learner who wants the choice, not the one who wants to be told |
| **Progress** | XP, the weekly goal, the three axes over time, the attempts worth replaying | |
| **Device** | Workspace, pairing, dataset sync, models, storage, chat, settings, and a collapsed *diagnostics* holding logs | Operator surfaces stay reachable; they are not resident |

Two rules about that list, both of which the first draft of this document got wrong.

**Practice is a flow, not a place.** A guard has to be remembered by every page that exists
and every page that gets added later; a full-screen flow that hides the navigation cannot
be left by accident. Modelling practice as a modal push removes a whole failure class —
*leave Read Aloud for Logs mid-recording and you cannot get back* — at its source, rather
than detecting it after the fact. That reported symptom was only possible because the phone
had been given the cockpit's navigation.

**Dictation and Cards are the phone's cheapest wins, so they stay on the bar.** They need
no microphone, no model and no hub, and the code already agrees: the phone's default bar is
`["dictation", "cards"]` (§10.9). The first draft's four-surface IA dropped both; the
review's proposed "Library" would have demoted them one level deeper. Either is a
regression against the funnel §6 depends on.

What the flow does **not** fix, stated plainly: it cannot cover the case where leaving was
deliberate — measurement in flight, and I chose to go and read the logs. That case is served
by making measurement state visible from the database (§10.2, §8 Phase 0), not by trapping
the learner. **A modal you cannot leave is a hostage, not a safeguard.**

With this shape the phone's navigation is not a tab strip. There is nothing to recycle, no
ceiling to hit, and "go back" is unambiguous: out of Practice, into Today.

### 4.2 Compute is decided, not asked

Today a follower is shown a choice: "Run on the Hub" or "This device". That question
depends on which models are downloaded, what the device is capable of, and what a hub
even is. It is an internal concept wearing a dialog.

The rule: **measure wherever the app can do it fastest, and say afterwards what it
did.**

- Hub reachable → use it, silently.
- No hub, model ready → measure locally.
- Neither → save the take unmeasured and say so in one sentence. `read_aloud_save_take`
  (v0.1.33) already makes this safe: the recording is durable before anything is
  scored, so "not measured yet" is a state, not a loss.
- **Bound the wait.** Probe reachability in ~3 s; if the hub accepts a job, expect an
  answer within ~20 s, then fall back to local if a model is ready, else save unmeasured.
  A learner must never be left holding a spinner with no state to read.
- **Store what measured it.** `measured_by` plus the model id on every attempt, and the
  progress chart marks the points where the scoring model changed. The three axes come
  from different machinery and are not equally available on every device (§10.7), so a
  trend line that silently mixes a hub model with a phone model is not a trend.
- **Re-measure is a verb** — in the UI and through MCP — so an unmeasured take can be
  scored later, when a hub or a model becomes available.
- Provenance stays visible ("Scored on the Hub"). **The choice goes.**

### 4.3 Four words for the shell, so pages stop inventing their own

The first draft argued this vocabulary out in a design conversation and never wrote it
down. It belongs here, because every page that wants to survive navigation will ask the
same four questions, and the answers must live in the shell rather than in thirteen
components (`lib/refresh/page-refresh.tsx` today asks one page at a time, which is how a
recording ended up looking "clean").

| Word | What it answers | Who owns it |
|---|---|---|
| **Hold** | *Is something here worth more than the page it sits on?* `{kind: "unsaved" \| "in-flight", label, takenAt}` — an identity, not a boolean, and it **expires** (or heartbeats), because a leaked hold is a tab that can never be recycled and an Android freeze does not run the cleanup | The page declares it; the shell reads it before it replaces anything. It replaces both `canRefresh` and the card editor's separate overlay guard |
| **Locator** | *If this page is unmounted, can it come back to the same place?* A per-kind path the shell can hand back on reopen — dataset → text → view. Unmounting becomes a cost, not a loss | The shell's address layer (`lib/shell/tab-subject.ts`), which already decides `replaceState` and must stay the one that decides |
| **Stack** | *What does Back mean?* The visit history of instances, skipping ids that are gone | The reducer (`lib/shell/tabs.ts`). On the phone it is also what the system gesture pops (§10.11) |
| **Ceiling** | *How many pages may be open?* A memory **budget** that evicts clean, restorable instances — not a wall that refuses navigation with "close one first" (§10.4) | The shell, with a different policy per platform: desktop may recycle by default because its chips make that visible; the phone pushes because it has no chrome to show |

The ordering rule that keeps this honest: **a hold is a nicety once Phase 0 exists.**
Durability (save the take, surface `measuring` from the DB) is what makes the loss
impossible; the hold is what makes the waste visible while it is being avoided. Build the
hold first and you have built a guard rail over a cliff; build it second and it is a
label. That is also why the desktop keeps its tab strip and the phone never will — the
strip is the *legibility layer* for a policy that has to be safe before showing it helps.

### 4.4 Today reads the rows it already has

Both the first draft and the review that corrected it reached for a new synced object — a
`queue_item` table carrying `kind`, `position`, `reason`, `est_minutes`, `expires_at`. The
code says that object already exists. `agent_todo` (`src-tauri/src/todo.rs`) is per-user
(`user_key`), agent-writable (`todo_add`), deep-linked (`target_json` replays the same
agent-action jump `ui_invoke_action` uses), status-carrying (`open` / `completed` /
`dismissed`, terminal but kept so a `human` step can tell *did it* from *skipped it*),
workflow-wired (`source_run_id`, `step_id` — ticking it resumes the parked run), and **it
already syncs**: `"todo"` is listed in `APP_SCOPE_KINDS` and classifies as a `State` change
(§10.8).

So the rule, and it is a refusal as much as a design decision:

- **Today is a view over tasks plus a rule engine, not a table.** The rules (due reviews →
  weak texts → in-progress material → new material) *file* tasks; the override layer (the
  curator, an agent, a `human` step) files the same rows with a different `source`.
- What is genuinely missing is five columns on `agent_todo` — `position`, `kind`,
  `est_minutes`, `reason`, `expires_at`. Five columns beats a protocol bump (§10.8) and
  beats a second MCP family that does what `todo_*` does.
- **`reason` is mandatory.** An unexplained list is a menu with extra steps, and an agent
  that cannot read *why* an item is there cannot argue with it or improve it.
- Vocabulary: **Today** names the surface, **task** names the row. *queue* and *plan* are
  not adopted as nouns for either — the repo settled `task` vs `todo` in one commit and is
  not going to reopen it from a strategy doc (§13.5, `AGENTS.md`).

### 4.5 Local-first is the positioning, not an implementation detail

Your recordings never leave your house. Models run on your machine. Sync is
device-to-device with an Ed25519 pairing you approved by eye (`sync/pairing.rs`), not a
cloud account. This is the only thing in this category a learner cannot get from a
subscription app, and today it appears nowhere a user reads. It belongs in onboarding
copy and in the README's first line.

It also belongs **before the microphone prompt**. The OS asks for permission cold; a
priming screen that says *your recordings never leave your device* is the first place the
positioning reaches the learner, and it sits directly in front of the funnel's most brutal
cliff (§6). One screen, no setting, no dialog.

And it belongs as a plain statement in *Device*: local-first means the hub is the backup,
so a lost phone with nothing synced is lost progress. Say so, and show last-synced time.

---

## 5. The PC: what to design

Leave the cockpit alone. It has earned its density: multi-instance tabs, pinning,
audit and full-report surfaces, workflow DAGs with `verify` and `adopt`, the model
catalog. Professionals want chrome, and the curator is a professional doing a
long-running task.

Two changes only, both about the seam between the bodies:

- **Make the handoff explicit.** The curator's work ends with "this dataset is ready to
  practise". The UI should name which datasets are **Ready for practice** — and say what is
  still half-baked: subtitles but no cues, cues but no waveform, texts with no standard
  audio. That state exists (`dataset_status`, the pipeline stages) but it is written for the
  curator's own consumption; Today is the consumer that most needs it named. The word
  *publish* is deliberately not used for this: the website already publishes XP and a
  dataset registry (§13.7), and one verb cannot mean two network hops.
- **Design the agent as a butler, not a dock.** The MCP surface is roughly 180 tools
  across a dozen tool domains (`mcp/datasets.rs`, `cards.rs`, `books.rs`, `system.rs`,
  …), with a generated manual (`mcp/manual.rs`), policy tiers
  (`mcp/policy.rs`) and `fms://` resources. It is the most differentiated thing here and
  the least product-shaped. Its product form is small: *plan my next week, find the
  exercises I keep failing, grade what I recorded today, tell me what is broken in this
  dataset.* That requires the agent to act on the learner's **tasks** — the rows behind
  Today — not on 180 atomic verbs. The workflow engine is most of that machinery, and the
  `human` step already files a task and resumes on its completion; what is missing is the
  ordering and the rules that make a *day*, not a new object (§4.4).

---

## 6. What we would be measured on

Curator-first products are always demoed at the interesting part. The numbers say
otherwise.

| Metric | Definition | Why it is the one |
|---|---|---|
| **Time-to-first-scored-attempt** | Fresh install → a real score on screen | The whole funnel. Today it requires a dataset (import, market, or STT + waveform + DB generation) *and* a model download. If the honest median is over twenty minutes, activation is dead and nothing else in this document matters. |
| **Sessions per week, D7 / D28** | On the phone | Practice is a habit product. The retention curve *is* the product. |
| **Mic-permission conversion** | Percentage of first sessions that reach a recorded take | The most brutal cliff in any speaking app. |
| **Take-loss rate** | Takes started but never durably saved | Target: zero. It is the one metric that measures trust rather than engagement, and it is the failure that makes an app get deleted (§2). |
| **Dead-end rate** | Navigation states where Back or exit does nothing | Target: zero on the phone. Cheap to instrument, and it is the direct consequence of a shell designed for a desk. |
| **Measurement outcome mix** | Shares of takes measured locally / on the hub / saved-unmeasured, and how many unmeasured ones are later measured | The direct health metric for the compute-offload architecture. |
| **Curator cost** | Minutes from "a folder of audio and text" to "Ready for practice" | The supply side of the habit loop. At thirty minutes of pipeline-poking, Today runs dry. |

Deliberately **not** tracked: screens visited, tab count, per-feature adoption of the
workshop surfaces. Those rise when a tool is confusing, not when it is good.

### 6.1 Measuring all of that without telemetry

This document asks for numbers and §13.8 promises no analytics, no telemetry, no crash
reporting. Both stand, but only if the answer is local:

- A **metrics ledger** — an append-only table in the app DB (install time, first mic grant,
  first take, first score, session starts, measurement outcome). The §6 numbers are then
  computed on-device.
- **Readable by the user**, in Device ▸ Diagnostics, and through MCP. It is their data; a
  local-first app has no other kind.
- **One manual export** — "copy anonymous usage report", text or JSON, for a tester to send.
  Never automatic.

The existing `app_events` tool is *not* this. It reads a 1000-entry in-memory log ring that
evicts unread entries (§10.11), so it cannot answer a week-old question; and at this user
count, D7/D28 comes from a handful of known testers and interviews, not from aggregates.

### 6.2 The funnel, honestly

The first draft's target — *a scored attempt in under two minutes with no import, no model
download and no account* — is not achievable as written, because scoring needs a model and
the smallest useful one is 146 MB (§10.7). The review that caught this proposed "record
first, score when ready", which is right, and which the save-first architecture already
supports. The promise should be **graded**, because an axis shown as absent is more
trustworthy than a proxy score:

| Stage | What the learner can do | What must be downloaded |
|---|---|---|
| 0 | **Cards and Dictation** — a real first win, scored against cues and SM-2, no mic, no model | nothing |
| 1 | **Read Aloud**: record, save the take, get accuracy + fluency | one STT model (~31–146 MB, background, on Wi-Fi, with visible progress) |
| 2 | the pronunciation axis on the same take | the phoneme model (+339 MB) |

The first win is the recording; the score is a reward that arrives. **Ship a starter pack**
to make stage 0 and stage 1 reachable with zero configuration: a small built-in read-aloud
dataset with cues, clips and waveforms already generated, *plus* a card deck and a short
dictation set. That is a content decision, it is cheap, and it is the highest-leverage item
here.

Its provenance is a decision too, and one the app has to get right before it ships audio:
public-domain text (Gutenberg/Wikisource-class sources) with the licence recorded per item in
`info.json`, and public-domain or CC0 recordings. **Generating the shipped audio with Edge TTS
is a licensing risk, not a convenience** — it is an unofficial Microsoft endpoint whose output
we would be redistributing (§13.3). Maintain the pack as a versioned dataset in the release
assets, not in git history.

---

## 7. What we say no to

Seven refusals, each with the reason, because a "no" without a reason gets relitigated
every quarter.

1. **The tab strip as a cross-platform default.** `MULTI_INSTANCE` is empty, so today
   the strip delivers "tabs you can close and pin" — while its replace-by-default
   policy is exactly what destroys a recording mid-take, and its auto-pin machinery
   created invisible, unclosable tabs on the phone. A power feature whose cost is paid
   by every other platform. **Keep it on desktop; give the phone push/pop**, which is
   cheaper, and push Practice as a full-screen flow that hides the bar (§4.1), which is
   cheaper still. (Mechanism detail in §10.3; the transport question is §10.11.)
2. **User-visible dataset layering.** Base vs overlay, the fork, family ids, the
   `info.json` structure — curator-only, PC-only. It has no business in a phone drawer.
3. **Any flow that asks the user an engineering question.** Which STT model, which TTS
   voice, which machine measures this, which layer do I write to. Internal concepts must
   not become dialogs; they become one-line statements of what was done instead.
4. **iOS as a second phone target** until the Android phone product is a habit loop.
   iOS costs signing, review, and a second audio stack, and buys the same learner twice.
   The unsigned IPA in CI stays an engineering artefact, not a user promise.
5. **A second list object.** No `queue_item` table beside `agent_todo` (§4.4). Two
   synced, per-user, agent-writable, deep-linked lists whose difference is ordering is
   the shape this document refuses everywhere else: one concept, two names, no rule
   saying which one a learner's screen reads.
6. **A crash-recovery promise we cannot currently keep.** The manifest declares
   `RECORD_AUDIO` and `MODIFY_AUDIO_SETTINGS` — no `FOREGROUND_SERVICE`, no `WAKE_LOCK`
   (§10.10). "Your take survives a force-kill" needs a foreground service with a
   persistent notification, which is a different and larger piece of Android plumbing than
   a JS timer that keeps flushing while the Activity is stopped. Until that exists the
   durability promise is **leaving a take mid-flight never loses it** (finalize and save),
   not *a power cut never loses it*. Saying the second thing and shipping the first is
   worse than saying nothing.
7. **Blocking navigation as the safety mechanism.** A hold that refuses to let the learner
   leave, with no escape, converts a defensive design into a hostage (§4.1). Durability
   first, then the option to leave; the guard is the nicety, not the fix.

---

## 8. Sequencing

The first draft opened with navigation. The review's strongest single objection was correct
and is accepted: **guarding navigation is defence in depth for a bug whose real fix is "do
not throw the bytes away"** — so durability now comes first, and it is a Phase 0 that
applies to the desktop too.

| Phase | Content | Why this position |
|---|---|---|
| **0 — Stop losing work** | (a) **Finalize and save on leave**: unmount, background, or replace during a recording stops the capture and files the take as an unmeasured attempt via `read_aloud_save_take`, instead of `abandonRef` dropping it (§10.1). (b) **Measurement state in the DB**: an attempt reads `measuring` after a remount, and finishing while the learner is elsewhere raises a notice with a link to it (§10.2). (c) **Practice pushed full-screen, bar hidden.** (d) The dirty guard also false while recording or measuring, so a *desktop* replace cannot recycle a held tab. (e) The local metrics ledger (§6.1). | Every one of these is a change of policy over machinery that exists, and none needs the new IA. Together they make the reported bug impossible rather than detectable. |
| **1 — Don't break the habit** | Phone push/pop with a real Back (transport per §10.11); hub-by-default scoring with `measured_by`, timeouts, fallback and re-measure (§4.2); starter pack and the graded first-run flow (§6.2), including mic priming. | These are the moments where a learner loses work or has to think. Smallest surface area, largest trust effect. |
| **2 — One job per screen** | Today / Library / Progress / Device with Practice as the pushed flow (§4.1); Today v0 as **rules over existing tasks**, no new synced object (§4.4); minutes-per-day and the "done for today" state; operator surfaces behind diagnostics. | The IA only pays for itself once the floor no longer opens under you. Today v0 stays computed from rows the app already has, so learner value arrives before the data work. |
| **3 — Make the two bodies talk** | "Ready for practice" as an explicit curator action (§5); the five columns on `agent_todo` (`position`, `kind`, `est_minutes`, `reason`, `expires_at`) and its MCP exposure; tasks from agents and `human` steps surface as Today items. | Names the workshop's output a consumer, which finally makes curator cost measurable. Note §10.8: a *new* synced table would be a wire-protocol change, which is the concrete reason Phase 3 extends rather than invents. |
| **4 — Butler verbs on tasks** | Plan the week, pick the failed items, grade what I recorded — acting on the task rows Today reads, with `reason` strings and undo; MCP stays the transport. | An agent acting on a plan is a product. An agent holding 180 atomic tools is a demo. |
| **Deferred, named** | Crash/force-kill recovery (needs a foreground service, §7.6); a local daily reminder (needs `POST_NOTIFICATIONS` and a notification plugin — the deep-link scheme that would make its tap useful already works on mobile, §10.10); closing the learning loop (weakest-word highlighting, standard-clip playback, immediate re-record, delta between attempts); spacing read-aloud failures as well as cards; a weekly goal in place of a pure daily streak. | Each is real and each has a dependency this document now states, so deferring them is a decision rather than an oversight. The learning-loop item is the one a practice-science reviewer should argue is Phase 1, not "later" — a score that nothing acts on is a number. |

---

## 9. The risk worth putting in a deck

**Scope gravity.** This app is simultaneously a dataset studio, an STT workbench, an
OCR tool, a flashcard system, a wiki, a sync server, a TTS studio, a workflow engine and
an agent host — and every one of those has real code behind it, so none of them are
vaporware. The danger is not that any of them is badly built. It is that **the learner's
five minutes is the smallest feature in a product that measures itself by breadth.**

The standing job is to keep insisting: the phone's Practice screen is the product, the
workshop is the supply chain, the agent is the staff. And a learner who never opens
*Device* should still be able to get better at speaking today.

Secondary risks, named so they get owned: content supply (curator cost is the only
thing that keeps Today full); Android audio friction (the WAV-playback cutoff fixed in
v0.1.32 is the kind of bug that costs a session and leaves no log); hub availability (a
learner with an unreachable PC must still be able to practise, which is what "saved
unmeasured" is for); and model download size against a phone's data plan.

---

## 10. Evidence: the code facts behind §4, §7 and §8

Verified in this repository, with the current path. These are the specific findings a
navigation decision has to survive; they are recorded here so the strategy above does
not rest on anecdote.

### 10.1 A replace during recording throws the audio away

`src/components/books/useRecorder.ts` releases the microphone *and drops the take* on
unmount — "the pane went away — the take was released". Leaving Read Aloud mid-recording
therefore destroys captured audio, not merely a view. The save-first work in v0.1.33 does
not help this case, because the save happens when the take *ends*.

### 10.2 A replace during measurement is harmless to the data

`src/lib/read_aloud/rerecognize.ts` (`measureAttemptOnHub`) is a plain module function.
React unmount does not cancel its promise, so the hub's reply is still written and the
row is still scored. Only the *knowledge* is lost: no overlay, no result panel, no
notice. The user comes back unable to tell whether it ran.

### 10.3 The recycling policy is real and the dirty guard is narrow

`src/lib/shell/tabs.ts` `planOpen` reuses the focused tab's slot when it is unpinned and
"clean", and a replace unmounts. `src/components/read_aloud/ReadAloudPage.tsx` defines
clean as *the text editor's draft differs* (`canRefresh: () => !dirty`), so a recording
in progress or a measurement in flight does not protect the page. `pinned` starts true
only for the landing dictation tab (`initialTabStrip`), which is why half-typed dictation
answers survive today — a pin doing a job meant for a guard. Auto-pin is never released,
because `togglePin` is a manual flip.

### 10.4 The phone has no tab chrome and therefore no exit

`src/app/(app)/home/page.tsx` renders `titleBar = mobile ? null : …`, and the chips,
pin buttons and close buttons live only inside `TitleBar`/`TitleBarTabs`. `MAX_OPEN_TABS
= 8`, and exceeding it refuses navigation with "close one first" — an instruction that
cannot be followed on a phone.

### 10.5 The phone has no back gesture to lose

Nothing in `src-tauri/gen/android/app/src` handles `onBackPressed`, `canGoBack` or
`goBack`, and no Rust code listens for a window close event; the shell writes the address
with `history.replaceState` (`src/lib/shell/tab-subject.ts`), so no history exists for
either the WebView or the system gesture to pop. `MainActivity.kt` is tracked and already
carries two hand-applied patches marked "re-apply after `tauri android init` regenerates
this file" — so a Kotlin back hook is possible, at a known maintenance cost.

### 10.6 The capability split already exists in the build

`src-tauri/Cargo.toml` gates the workshop modules behind `desktop`; Android builds with
`--no-default-features --features stt`. The backend cannot do most curator work on a
phone. The frontend offers thirteen of twenty page kinds anyway, filed by capability.

### 10.7 What scoring actually needs, and what it costs a data plan

Read out of `src-tauri/src/models/catalog_stt.rs` and `catalog.rs`. STT models by download
size: Moonshine Tiny **31 MB** and Base 55, Small 99, Canary 180M Flash **146** (En/De/Es/Fr,
translation too), GigaAM v3 151, SenseVoice 152, Parakeet V2 451 / **V3 int8 456** (25
European languages incl. German), Canary 1B v2 691, Whisper small 465 → large 1.5 GB. The
phoneme model is separate: `wav2vec2-alignment` **603 MB** fp16, its int8 build **339 MB**.

The consequence for §6.2 is that the three axes are *not* one download. Accuracy and
fluency need an STT model and VAD; pronunciation needs the phoneme model on top, and
`read_aloud/phoneme.rs` says so — "the only part of pronunciation scoring that needs a model
file", everything downstream in `gop.rs` being arithmetic. A 146 MB model can therefore
produce a real, partial first score for German; nothing small enough to bundle in an APK
produces all three.

### 10.8 The app-data sync plane is a whitelist, so a new table is a protocol change

This is the fact that decides §4.4, and it is not obvious from `sync.md`:

- `sync/change_log.rs` keeps `APP_SCOPE_KINDS` as a **literal list** — `dictation`, `xp`,
  `card_review`, `read_attempt_save`, `read_attempt_delete`, **`todo`** — and it is the
  single source for both `is_app_scope_kind` and the writeback flush's `kind IN (...)`
  filter.
- `ChangeClass::of(kind)` classifies explicitly and **defaults an unknown kind to
  `AppendOnly`**. Append-only replay is `INSERT OR IGNORE` on the row's own uuid, so a
  mutable table added without a `State` entry would sync its creations and silently lose
  every in-place edit. The trap is real and invisible.
- `sync/rest.rs`'s `/app/state` backfill carries **named per-table cursors**
  (`after_dictation`, `after_xp`, `after_reviews`, `after_attempts`) and a response struct
  with one field per table. A new synced table means both.
- `PROTOCOL_VERSION` is `6` and `HUB_FEATURES` is an advertised list; a capability a host
  does not list is refused, never attempted.

So a `queue_item` table is a wire-protocol change spanning hub *and* follower, while
`agent_todo` — already `"todo"` in that list and already `State` — needs five columns and
no version bump. That is the whole argument of §4.4 in four bullets.

### 10.9 The phone already chose Dictation and Cards

`src/lib/mobile-nav.ts`: `MAX_BAR_MENUS = 4`, `DEFAULT_NAV_CONFIG.bar = ["dictation",
"cards"]`, and `NAV_MENUS` lists thirteen ids across the zones root / cross-device / tools /
bottom. `Sidebar.tsx`'s `MOBILE_VISIBLE_TABS` gates them, with the comment that *"models" is
enabled to verify local STT on Android*. Two things follow: the review's charge that a
four-surface IA "drops half the learner's modes" is fact — those two surfaces are one tap
away today and need no mic, model or hub — and any bar redesign inherits a user preference
stored in `localStorage` under `bottom-nav-config`, with a rename map (`read-book` →
`books`) proving ids are a published surface.

### 10.10 What the Android build does and does not declare

`src-tauri/gen/android/app/src/main/AndroidManifest.xml` permissions: `INTERNET`,
`ACCESS_NETWORK_STATE`, `CHANGE_WIFI_MULTICAST_STATE`, **`RECORD_AUDIO`**,
`MODIFY_AUDIO_SETTINGS`, `WRITE_EXTERNAL_STORAGE` (API ≤28), with microphone
`required="false"`. There is **no `FOREGROUND_SERVICE`, no `WAKE_LOCK`, no
`POST_NOTIFICATIONS`**, and `Cargo.toml` pulls no notification plugin — so §7.6 and the
deferred reminder in §8 are new plumbing, not configuration. What *is* already there: the
`fms-app://` deep-link scheme registered and handled in the mobile `run()`
(`src-tauri/src/lib.rs`), which is the half of a notification that would otherwise be hard.

### 10.11 Two absences the review assumed away

**A back transport.** §10.5 stands as written, and one more fact sharpens it: the lock pins
`tauri 2.12.1` (`Cargo.lock`), whose `WindowEvent` in `tauri-runtime-2.12.1/src/window.rs`
has **no back variant** — the enum stops at `Moved` / `CloseRequested`. Upstream
`tauri-apps/tauri#8142` ("app just closes on back in a SPA") was closed in October 2025 with
a back-button event, and the community workaround in that thread is exactly the Kotlin patch
this document used to worry about. So "use `pushState` and the system gesture will pop" is a
**device experiment, not a plan step**: it has to be run against a real build before Phase 1
is scoped, and the fallback is a third hand-patch to `MainActivity.kt`.

Also worth stating for anyone tempted to make the shell history-driven: the address bar
deliberately uses `history.replaceState` (`src/lib/shell/tab-subject.ts`) so "a browse must
not become a back-step for every keystroke". If the phone starts pushing entries, that same
code has to decide push-vs-replace per platform — which belongs in the shell's locator layer
(§4.3), not in thirteen pages.

**A durable event store.** `mcp/system.rs`'s `app_events` reads the in-memory log ring —
1000 entries, `evicted_since_cursor` reported when it drops unread, with the advice to use
`log_read_file_history` for the archive. It is a *watch a running job* tool. It cannot answer
"median time-to-first-score last week", which is why §6.1 asks for a table instead.

---

## 11. How to use this document

Before designing a surface or a feature, answer three questions in writing:

1. **Which body is this for** — workshop, gym, or the seam between them?
2. **What must the user not have to think about** in order to use it? (If the answer is
   "nothing", check whether the feature is asking them to decide something the app knows
   better.)
3. **What does it cost the learner when it goes wrong** — a wasted minute, a wasted
   session, or a deleted app?

Two standing rules, both already law elsewhere in this repo and restated here because
they are product rules, not style rules:

- **One noun per concept** (`AGENTS.md`, Vocabulary): text, content, reference, span,
  clip, task. A synonym is a second name a learner — or an agent — has to map.
- **Everything user-visible is MCP-visible** (`docs/agent_friendly_design.md`), and
  errors must be actionable: say what to do next, not that something failed.

---

## 12. Settled, and still open

The questions below are referred to by **name**, not by number: a numbered cross-reference
points at the wrong question the moment one is answered and removed, and half of this
document's authority rests on its pointers holding.

**Settled** — by the review, by this revision, or by a code fact. Recorded so they do not
return as new ideas:

1. The phone does not keep replace-by-default: push/pop, and Practice is a pushed
   full-screen flow that hides the bar (§4.1).
2. A hold that blocks navigation is not the safety mechanism; durability is. Finalize and
   save, then let the learner leave (§7.7, §8 Phase 0).
3. A restore point is **shallow**: same screen, same text or dataset, plus any recovered or
   pending takes. Not scroll position, not an unsubmitted take parked in a component — the
   take's bytes belong on disk, which gives most of the same value for far less machinery.
4. The learner's day is neither a workflow nor a new table. It is the task rows, ordered,
   with a reason (§4.4).
5. The starter pack is content shipped in the release assets — public-domain text, openly
   licensed audio, licence recorded per item — and it contains cards and dictation as well
   as read aloud (§6.2).

**Still open**, in the order they should be settled:

1. **The Back experiment** (§10.11). Its outcome decides Phase 1's shape, not its priority.
   If the gesture cannot be routed at all, does the phone get an explicit *up* affordance in
   the chrome instead — and what should "back" mean for a learner who entered Practice from
   Library, then opened a word lookup?
2. **How deep a locator goes, per page.** Read Aloud (dataset → text → view) is plausible;
   Dictation (dataset → media → cue) is a different cost. Which pages earn one first, and who
   says so when a new page appears?
3. **The daily rules**: which sources, what weights, and whether *minutes per day* is one
   budget or a budget per mode. The practice-science objection in §8 lives here.
4. **Whether the learning loop is Phase 1 after all** — highlight the weakest spans, play the
   standard clip for that span, re-record immediately, show the delta. A score that nothing
   acts on is a number, and this is the one deferral the document is not confident about.
5. **Score trust.** Are the 60/80 XP thresholds right for real learner recordings, and should
   a "is it quiet enough?" check precede recording, so background noise is never blamed on
   the learner?
6. **The learner with no PC.** The review's sharpest blind spot: a phone-only user cannot
   curate and receives nothing from a hub. Starter pack plus an optional, listed registry
   fetch is a different distribution story. In scope, or explicitly out?
7. **A local daily reminder.** The habit case is real; the cost is a permission and a plugin
   this build does not have (§10.10), and the local-first reading of it has not been argued.
   §13.10 currently states "no notification engine" as a fact, not as a principle — that is
   worth turning into a decision one way or the other.

---

## 13. Appendix A: background primer — what FmS is, for a reader without the code

Written to be read first by someone who cannot open the repository: a reviewer, a new
contributor, another model. Nothing here argues anything; it is the shared ground the
argument above stands on. Source of truth for most of it: `README.md`, `AGENTS.md`,
`docs/manual/` and `docs/design/`.

### 13.1 The app in one paragraph

**FmS** — *Fremdsprachen machen Spaß*, "foreign languages are fun" — is a **local-first
language lab**. You bring material you already own (a textbook's MP3s, an audiobook, a
podcast, a lecture recording, a chapter of prose) and it becomes practice: **dictation**,
**books**, **read aloud**, **cards**, and a **wiki** that ties them together. Local
speech-to-text cuts audio into cues; a reference text is scored against what you said;
review scheduling, progress and XP stay on your disk. No upload, no per-minute bill, no
account. The stated audience is the intermediate German learner (roughly B1–C1, working
towards Goethe-Zertifikat or TestDaF) who wants to train on real speech rather than
textbook clips; the method works for any language with a usable local STT model. MIT
licensed, open source at `github.com/wsva/fms-app`, a Tauri port of the web app at
`github.com/wsva/fms`.

### 13.2 Status, so suggestions land on the real thing

- Current release **v0.1.33** (November 2026). A `v*` tag is the only automatic trigger
  for the release pipeline, which builds five targets: Windows NSIS `.exe`, Linux
  AppImage/`.deb`/`.rpm`, Android arm64 `.apk`, macOS arm64 `.dmg` (unsigned), iOS arm64
  `.ipa` (unsigned, needs re-signing to install).
- It is **a single-user desktop tool that grew a multi-device story**. There is no cloud
  service of ours; the desktop *is* the server when a phone asks.
- **The app ships no content.** A learner must point it at a folder, download a model,
  import a dataset and run the preparation pipeline before there is anything to do.
  This is the funnel problem behind §6 and the reason for the starter-pack recommendation.
- Roughly 180 MCP tools, a user manual, the five modes and the toolkit are built and in
  use — breadth is not the scarce resource here (§9).

### 13.3 The stack

| Layer | What is used |
|---|---|
| Desktop shell | Rust, **Tauri v2**, `rusqlite` (bundled SQLite), `symphonia` for audio decode and waveform peaks |
| Frontend | **React 19** on **Next.js 15**, statically exported; **HeroUI v3**, Tailwind CSS v4 with CSS custom properties (four themes), pnpm |
| Navigation | One shell route, `/home`. Pages are siblings toggled by `display`, not routes; a small tab strip (`lib/shell/tabs.ts`) keeps several mounted. `src/app/listen_speak/*` are dead redirect stubs |
| Speech-to-text | **Parakeet** ONNX via `transcribe-rs` (V3 int8: 25 languages incl. German, 456 MB; V2: English), **Moonshine** (English, 31–192 MB), **Canary 180M Flash** (En/De/Es/Fr, 146 MB), **SenseVoice** (152 MB), **Whisper** GGML via `transcribe-cpp` (90+ languages, 465 MB–1.5 GB). Full size list: §10.7 |
| Other models | **SaT** sentence segmentation (`ort` + `tokenizers`), **Tesseract** OCR with 15 Hunspell dictionaries for line-break hyphens, a **phoneme model** for pronunciation scoring (339 MB int8 / 603 MB fp16 — the one axis that needs its own model, §10.7), **Edge TTS** for generated reference audio (an unofficial Microsoft endpoint, no key) |
| LLM features | Local **Ollama** or a cloud provider through the goose SDK layer — used *by features* (digests, word explanations, quizzes, OCR clean-up, reference normalization, feedback), never as a chat window |
| Build gates | `default = ["stt"]`; `desktop` adds the workshop modules; **Android builds `--no-default-features --features stt`** |

### 13.4 What is on screen

**Five practice modes**, each with its own dataset type and its own sync behaviour:

| Mode | The action | What it stores |
|---|---|---|
| **Dictation** | Type or speak what you hear, cue by cue, then reveal the reference | media, VTT subtitles, cue-level rows, per-cue progress |
| **Books** | Read a split text with per-sentence audio, look words up in context, build an *understanding layer* (explain, simplify, digest, argument map, glossary, quiz) | chapters/sentences/words, clips, artifacts |
| **Read Aloud** | Record yourself reading a text and be scored on **accuracy, fluency, pronunciation**; XP tiers at 60 and 80 | texts, standard clips, attempts with metrics |
| **Cards** | SM-2 review of vocabulary kept from your own material, FTS5 search, tags | card rows, review state |
| **Wiki** | Syncable Markdown trees, full-text search, editable on the phone | documents under their own root |

**The pipeline that feeds them:** import → *Generate Subtitles* (STT) → *Generate
Waveforms* → *Generate Database*, plus transcript/cue alignment, auditing and book
splitting — one click per stage on a dataset page, or stepwise on the **Workflow** page,
which is a persistent file-based DAG with `verify` and `adopt` joins and `human` steps.

**The toolkit** (why the app stays installed): OCR of a screenshot or photo, PDF reader,
Split Text, Edge TTS, voice input in any text field, Simple Words (the words you already
know), device-to-device Chat, Tasks, Workspaces, Models, Settings, Logs.

**Navigation inventory:** `PageHost` knows twenty page kinds. The phone exposes thirteen
of them (Dictation, Books, Read Aloud, Cards, Wiki, Workspaces, Devices & Hub, Datasets
Sync, Chat, Tasks, Models, Logs, Settings) across a bottom bar of at most four plus
"More", and a drawer that groups them *by capability* ("Cross Device", "Tools"). Workflow,
OCR, Split Text, PDF Reader, Simple Words, Edge TTS and the Agent page are desktop-only.

### 13.5 Vocabulary — please use these nouns

The project enforces **one noun per concept** across code, UI copy, MCP descriptions and
docs; a synonym is a second name a learner or an agent has to map. Renaming things for
the sake of plain English is a known, specific cost here.

| Concept | Word |
|---|---|
| A unit of read-aloud material (title, note, what you read) | **text** — never "passage" |
| What you read aloud inside it | **content** |
| The spelled-out form scoring reads against | **reference** |
| The seconds a text occupies inside one recording | **span** |
| A text's standard audio | **clip** |
| One recording you made, and its score | **take** / **attempt** |
| The three numbers | **accuracy, fluency, pronunciation** |
| A unit of work an agent or a `human` step leaves the learner | **task** (the storage layer and the `todo_*` commands are named `todo`; the UI and docs say task) |
| The phone's "what next" screen, and the rows it shows | **Today**, and **task** — *queue* and *plan* are deliberately not nouns here (§4.4) |
| A folder of material on disk | **dataset** (of type dictation / card / book / read_aloud) |
| One row of dictation, with a time range | **cue** |
| The machine that serves the library, and the ones that follow it | **hub**, **follower** |
| A learner's context on a machine (settings, progress, datasets) | **workspace** |
| In the navigation design only: a mounted page, its protection, and a claim on it | **instance**, **pin**, **hold**, **locator**, **stack**, **ceiling** (§4.3) |

### 13.6 How data is stored

- **A dataset is a directory**, not a row. `<datasets_dir>/{dictation,card,book,read_aloud}/<name>/`
  with `info.json` (identity, `uuid`, structure), `data.sqlite3` (`listen_media`,
  `listen_subtitle`, `listen_subtitle_cue`), `media/`, `subtitle/*.vtt`, `waveform/*.json`,
  optional `book.txt` and `transcript/`.
- **Two SQLite databases.** The dataset DB (material and cues) and one app-level DB per
  workspace (cross-dataset dictation progress, the XP ledger, tasks). Deliberate: progress
  is yours, not the dataset's.
- **Read-aloud takes are audio files plus a journaled row**; a destructive action moves
  the thing into `<data_root>/trash/<timestamp>_<name>` instead of deleting it — no
  confirmation parameter, because undo is the safety mechanism.
- **Sync** (design docs `sync.md`, `sync-v2.md`) moves dataset content as snapshots with
  per-row append-only journal entries and last-write-wins on a *strictly newer* timestamp
  revision; personal state moves as its own writeback queue. Media travels on a separate
  upload plane.
- **The personal-state plane is a whitelist, not a generic table carrier.** Per-user data
  syncs by *change kind* (`"dictation"`, `"xp"`, `"card_review"`, `"read_attempt_save"`,
  `"todo"`), each declared in one literal list and classified as `State` / `AppendOnly` /
  `Counter`. An unlisted kind defaults to `AppendOnly`, whose replay is `INSERT OR IGNORE`
  — so a new mutable table invented here costs a wire-protocol bump and can lose edits
  silently. If you are tempted to propose a new synced table, read §10.8 first: the usual
  answer is a column on a row that already syncs.
- A dataset can be **base + overlay**: a read-only shared copy with a writable fork on top.

### 13.7 Multi-device, in one paragraph

A **workspace** is either the hub (authoritative copy, serving datasets over the network)
or a follower. Roles belong to the workspace, not the app; **a phone is always a
follower**; the service starts and stops with the role, so there is nothing to install on
a server. Devices announce themselves on the LAN (plus a direct TCP sweep and Tailscale)
and pair once: a short code shown on both screens, approved on the hub, Ed25519 from then
on. Loopback and tailnet peers are trusted without pairing; anything else needs a paired
signature. The embedded MCP/HTTP server listens on port **35711** with a UDP beacon on
**35712**. A conflict keeps the later edit; an unpushed local edit is never overwritten.
Signing in to the companion website (`lusworkshop.site`) is optional and unrelated to
sync — it publishes XP and a dataset registry.

### 13.8 Local-first boundaries

**Local-first, not local-only, and audio never leaves the device.** Import, STT,
waveforms, cue generation, alignment, dictation, review, scoring, voice input, OCR, book
splitting and all practice state are on-device. Only *text* can leave, and only through
switches the user turns on: Edge TTS (the text to be spoken → Microsoft), an LLM provider
if a cloud key was configured, device sync (to your own paired device, over LAN/tailnet),
the optional website, model downloads from Hugging Face or a CDN, and the update check
against `api.github.com`. No telemetry, no analytics, no crash reporting. Keys and the
login token are redacted when a tool reads settings back.

### 13.9 The agent-first bet

The project's stated bet is that **the agent will be the most important software on a
personal computer**, so agent-friendliness is weighted the same as human usability, and a
decision that serves only one is a decision not yet made. In practice: everything a human
can click is also an MCP tool; the app documents itself (`/agent.md`, `/agent.json`, MCP
resources like `fms://status` and `fms://docs/...`, prompt recipes); writes return JSON
that *proves* them; errors name the next call to make; every tool carries a tier (`read`,
`write`, `destructive`, `ui-control`, `human-handoff`) and every call is journaled, so
`mcp_policy = "enforce"` can refuse the dangerous ones. Work that needs a person — record
this take, judge this reference clip — is left as a **task** deep-linked to the exact
screen, and a workflow parked on a `human` step resumes when it is ticked. There is a
second, practical reason: the jobs between sessions (find the folder, import, wait, nurse
a failed stage, cut clips) are exactly why a learner stops opening the app, and an agent
can take them.

**On the phone there is no agent surface at all** — `mcp/`, the workflow engine, the dock,
OCR and the alignment modules are desktop-gated. That asymmetry is §2's split, expressed
as Cargo features.

### 13.10 What this app deliberately is not

So suggestions do not assume machinery that does not exist and would break a principle if
it did:

- **Not a cloud service.** No backend of ours, no accounts-as-a-dependency, no server-side
  transcription, no shared multi-user collaboration. A workspace is one person's.
- **Not a chat app.** LLM features are called *from* features; there is no chat window by
  design ("for open questions, use your own model or an attached agent").
- **Not routed.** One shell route with display-toggled pages; no per-page URLs, no
  deep-linked navigation stack. That is a decision, with a cost, and §4.1 and §10.5 are
  two of the costs.
- **Not a content provider.** It ships nothing to practise on (§6's funnel).
- **Not a gamified habit app** in the Duolingo sense: XP exists, tiers are +1 at ≥60 and
  +1 more at ≥80, there are no leagues and no streak-shaming. There is also **no
  notification engine** — the build declares no `POST_NOTIFICATIONS` and pulls no
  notification plugin (§10.10). That is presently a *fact*, not a principle, and §12's
  *local daily reminder* lists it as an open decision rather than a refusal.
- **Not a mobile workshop.** The phone records, reads, reviews, searches and syncs. It does
  not build datasets.

### 13.11 Constraints a suggestion has to respect

1. **Local-first** — audio and progress on the user's disk; any network call must be
   optional, named and listed in the privacy table.
2. **Agent-equal** — if a human can do it, an MCP tool can do it, with a structured
   response and an actionable error.
3. **One noun per concept** (§13.5).
4. **Nothing lost by accident** — trash and undo before destroy, not a confirmation dialog.
5. **The desktop owns the workshop; the phone owns the practice.** A suggestion that moves
   curation to the phone fights the build gates.
6. **The phone must work when the hub is asleep.** Hub compute is a fast path, never a
   dependency for a take already saved.
7. **Single shell, no routes** — changing that is allowed only with an argument, because
   state preservation across tab switches is load-bearing.

### 13.12 What would be most useful from an outside reviewer

A first review was received in November 2026 and is answered in **§14**; the asks below are
what survived it, sharpened. Read §13 first, then §4–§8.

- **Challenge the workshop/gym split (§2).** If there is a better cut of these surfaces,
  say what it is and what it fixes that this one does not.
- **Design the daily rules, not the data object (§4.4; §12's *daily rules*).** What goes into Today, in
  what order, for someone with no schedule — and what makes "done for today" feel like an
  ending rather than a deficit. The rows and their sync already exist; the judgement does not.
- **Argue the learning loop's priority (§12's *learning loop*).** Highlight the weak spans, play the standard
  clip for that span, re-record immediately, show the delta. If that is what makes practice
  work it may belong in Phase 1, and this document would like to be told it is wrong.
- **Stress-test the refusals (§7).** Especially keeping the tab strip off the phone, killing
  the hub-vs-device choice, and refusing a second list object — recent, and reversible.
- **Attack the funnel (§6.2).** The graded 0 / ~146 MB / +339 MB ladder, the starter pack's
  content and its licensing, and the mic-permission cliff.
- **Name the blind spots** — retention mechanisms, practice science, or the learner with no
  PC at all (§12's *learner with no PC*).
- **Flag anything in §4/§5 that trades away the agent surface or the local-first promise**,
  since those are load-bearing for the whole project rather than preferences.

Claims in §10 were read out of the code and are stated as facts; weigh them accordingly,
and say which one you would resolve differently.

---

## 14. Amendments after the external review

The first draft was sent to another model with no repository access, with Appendix A
attached so it could be reviewed cold. The reply came back as an instruction document for a
coding agent: verdicts on the open questions, a phone IA, a queue data model, a funnel plan,
a sequencing with acceptance criteria, and a checklist (§11) of claims it knew it had not
verified. That structure was useful, and it is why §10 now carries the facts that settled
four of its recommendations.

**Taken.** In its own words, and where each landed:

| The review said | Verdict | Now in |
|---|---|---|
| The strategy over-fixes navigation and under-fixes the data loss at its source — make the take durable | **Accepted, and it re-ordered the plan.** Split into "finalize and save on leave" (cheap, real) and "survive a force-kill" (needs plumbing we lack) | §7.6, §7.7, §8 Phase 0 |
| Four surfaces drop half the learner's modes; Cards and Dictation are phone features | **Accepted — a plain error.** The code's default phone bar is exactly those two | §3, §4.1, §10.9 |
| Practice should be a full-screen pushed flow that hides the nav, not a destination | **Accepted.** It is the in-flight hold expressed as routing, so no future page has to remember a guard | §4.1, §8 Phase 0 |
| Hub-by-default needs bounded waits, `measured_by`, and a re-measure verb | **Accepted.** Comparability across scoring models is a learner-facing property, not a detail | §4.2 |
| §6's metrics contradict §13.8's no-telemetry promise; answer it locally | **Accepted.** A ledger table, readable in Diagnostics, exportable by hand | §6.1, §8 Phase 0 |
| "Score in two minutes with no download" is not achievable as stated | **Accepted and rewritten** as a graded ladder, with the sizes that make it true | §6.2, §10.7 |
| Learning science is missing: the feedback loop, spacing beyond cards, a weekly goal, score trust | **Accepted as named deferrals, with one challenge left standing** — the loop may be Phase 1 work | §8, §12 (*the learning loop*, *score trust*) |
| "Publish" is already taken by the website registry | **Accepted.** The curator's verb is *Ready for practice* | §5, §6 |
| Add take-loss rate and dead-end rate | **Accepted** | §6 |

**Refused, with the reason.** Each traces to a claim the review itself flagged as
unverified — not to a judgement that was wrong on its own terms:

| The review said | Why not |
|---|---|
| Store a new `queue_item` table in the app DB and ride the personal-state writeback "without a sync-protocol change" (its own V6) | The writeback plane is a **whitelist of kinds** with named per-table backfill cursors; a new mutable table is a `PROTOCOL_VERSION` bump, and forgetting its `ChangeClass` entry loses every edit in silence. `agent_todo` already syncs and needs five columns (§4.4, §10.8) |
| Try the web-side route first: `pushState` + `popstate` gives the system gesture history to pop, Kotlin only as fallback | Unverifiable from the repo and unlikely — tauri 2.12.1 exposes no back event, and upstream handled the case by *closing* the app unless the native layer was patched. It is a device experiment, not a plan step (§10.11) |
| Flush audio chunks to a temp file every few seconds and offer "Recover last recording" after a force-kill | No `FOREGROUND_SERVICE`, no `WAKE_LOCK`: a timer in a frozen WebView is not a recorder. Promising crash recovery before that service exists is promising what we cannot keep (§7.6, §10.10) |
| "Never block. Make the loss impossible instead" — as an absolute | Right as a priority, wrong as a rule. The flow removes accidental exits; a *deliberate* exit must stay possible, and a modal with no escape is a hostage (§4.1, §7.7) |
| Adopt **queue** as the one noun for the learner's list and add it to the vocabulary table | One noun per concept is enforced in `AGENTS.md`, and `task` vs `todo` was just settled. A third name for the same rows is the cost the review's own constraint table warns about (§4.4, §7.5) |
| A local metrics ledger is new work | True, and cheaper than framed — but its reason stands: `app_events` is an ephemeral 1000-entry ring, so it cannot answer a week-old question (§6.1, §10.11) |
| Four phone tabs, with Practice pushed over them and Library holding Dictation / Cards / Books / Read Aloud / Wiki | Half right. The pushed flow is the best idea in the review; putting the two no-model modes behind a Library tab demotes what today is one tap away (§4.1) |

**The methodological lesson.** A cold reviewer is worth having precisely because they find
the assumption you stopped seeing — and they cannot tell you which of their own premises
are false. Every refusal above came from a claim that review marked unverified; its verdicts
on the §12 questions (push/pop, shallow restore, no blocking, content in release assets)
survived contact with the code intact. So the next round sends the same primer with §10
filled in *first*, and asks the reviewer to answer against the code facts rather than
around them.
