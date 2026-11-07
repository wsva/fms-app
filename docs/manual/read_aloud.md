# Read Aloud

Read Aloud is a speaking exercise: you record yourself reading a text aloud, and FmS uses speech‑to‑text to score how closely what you said matches the written words. It's designed to build pronunciation and fluency, and to let you track improvement over time.

Three words are used throughout, with one meaning each: an **attempt** is one submission (recording plus transcript plus scores), a **measurement** is one speech model's scoring of that attempt, a **recording** is the audio that attempt captured, and a **standard audio** is a reference clip the *dataset* owns, which pronunciation is measured against.

## Getting set up

Read Aloud works with a **Read Aloud** dataset — a collection of texts plus a place to store your recordings. The recordings are saved in your workspace's **recordings directory** (set in [Settings](workspaces_accounts_settings.md)). Before recording, make sure your microphone is chosen and tested in Settings → **Microphone**.

Open **Read Aloud** from the sidebar and pick a dataset (and, within it, the text you want to read). Use the toolbar to manage datasets and locations.

## Practicing a text

For the selected text you'll see its content and a **record** control. When the text has a **standard audio** clip, a player for it sits directly above the content — listen first to learn the pacing, then record:

1. Read the text aloud while recording. A **live recognition** panel at the bottom of the page shows the transcript as you speak, so you can see immediately whether the model is hearing you — and whether it is hearing the right thing.
2. Stop when finished. Your recording is **saved first**, before anything measures it, so a scoring run that fails, is cancelled or sits minutes behind a cold model never costs you the take. It is in [the attempts history](#attempts-and-history) straight away, marked **not measured** until its numbers arrive.
3. FmS transcribes your recording and compares it to the target text. On a **Follower** — a phone, or a second PC beside your **Hub** — a dialog asks *where* it should be measured: **On the Hub**, where the full models run, or **On this device**. **Not now** is always the way out: the take stays saved and unmeasured, and [**Assess again**](#attempts-and-history) scores it whenever you like.
4. A **score** badge shows how close you were, colour‑coded so you can see at a glance whether it was strong or needs work.

The live panel is read‑only: it is there to catch a silent microphone or a model in the wrong language while there is still time to start over. The attempt itself always ends with **Stop**.

The **practice panel** holds the text, the recorder, and the scoring view.

Once a text is open, the toolbar's **Refresh** re‑reads everything about it — its stored fields, its standard audio and its attempt history — and it means the same thing on the attempts page and on one open attempt, so a clip or a recording that changed elsewhere shows up without leaving the page.

## Standard audio (needed for pronunciation)

Accuracy — how close your words were — needs nothing but the text. **Pronunciation** — whether it *sounded* right — needs something to compare you against: a clip of the text read clearly and correctly. That clip is called the text's **standard audio**, and a text with none cannot be scored for pronunciation at all; the card says so instead of inventing a number.

To make one, open the practice toolbar's **Manage** button for the text and use the **Standard audio** card at the bottom of that page. If you have unsaved edits, **Save** first: a clip describes the text as it is stored, so while the draft differs those controls are greyed out. There are two ways:

- **Generate with Edge TTS** — synthesises the text in the voice this dataset's **language** prefers. The language is a property of the dataset (set when it is created, and editable from the picker here); the *voice* for that language is set once, on the [Edge TTS](ai_chat_agents.md#edge-text-to-speech) page, and every generation that needs a German voice gets the same one instead of whatever happens to sort first. The line above the button says which voice is in use and why — `German · de-DE-ConradNeural (preferred)` — and its **Change** link takes you to that page. The two pickers beside the button remain a one-off override: **language**, then a **voice** of that language; changing the language here also saves it to the dataset, which is what makes the preference reachable in the first place. This is the quick route, and it is also the accurate one for per‑word detail, because the synthesis reports exactly when each word is spoken.
- **Upload audio** — pick a recording of the text read by a person (your teacher, a native speaker, yourself being careful), then mark the part that reads the text: click or drag on the waveform to place the playhead where the reading begins and press **Set start**, let it play on and press **Set end**; or drag the two handles; or type the two times under the waveform, as `1:02.5` or `62.5`. The arrows either side of each button move that edge 100 ms at a time. If the file holds more than the text — a whole lesson, an interview — press **Guess from speech** instead: the speech model listens to the whole file, matches what it hears against the text, and moves both edges onto the reading. A note says how much of the text it heard; it refuses rather than guess when too little was heard, and it takes as long as the file is long. Then press **Save clip**. Only the selection is stored; nothing else of the file is kept. Per‑word detail for an upload is estimated in proportion to the time each word takes, so the overall score is unaffected but a single word's score is approximate.

A text can have several clips — try voices, or keep a human reading beside a synthesised one — and exactly one is **in use**: the circle beside a clip chooses the one pronunciation is measured against. Use **Play** to compare and the bin icon to delete; a clip whose audio file has not synced to this device yet says so, so an unplayable row is never mistaken for a bad one. A new clip always uses the voice resolved at the moment it is made, so changing the preference later never re‑voices an existing clip — delete and regenerate it if you want the new voice.

Each clip is labelled with where it came from — **Edge TTS**, **Upload**, or **Aligned**. An **Aligned** clip is one you made on the **Init Audios** page (the texts view's overflow menu): rather than a file of its own, it remembers that this text runs from one second to another of a longer recording, and its note says so — `Align · lesson01.mp3 · 31.51–44.58 s`. The short clip is cut out of that recording the first time you open the text to hear or score it, so a clip applied a minute ago may have no audio on disk yet; that is normal and it becomes playable as soon as the page opens it. The longer recording is yours and is not part of the dataset, so if it is moved or deleted, the clip says it cannot be cut any more instead of playing silence — align the text again against the recording's new place.

That page gives every text two verbs of its own. **Re-guess** places it again; **Save** stores the span as the text's standard audio. Both sit in one bar under the span's **Start** and **End** rows, above the text, so they are beside the times they change; the transport shares that bar — **Play span** (Space) plays the span, **±0.5 s** plays half a second either side of it. The list on the left is only a list, one line per text. There is no verdict to give in between: a span the speech model placed *is* the answer, and the two edges stay editable until you save. A text that already has a clip says **Clip exists** after its title and again above the content, and the button beside Save is **Delete clip**: changing the span of a saved text means deleting its clip first, then saving. **Save all clips** stores every placed text that has none and steps over the rest.

If you later edit the text, its clip is marked **Stale**: the words no longer match what the clip says, so pronunciation stops being scored until you make a new clip. That is deliberate — grading you against a target for a text you are no longer reading would be worse than a blank. Nothing is regenerated behind your back.

Clips belong to the dataset, so they travel with it: another device that syncs the dataset gets the same standard audio, and your score means the same thing there. An aligned clip travels as its audio, not as its span: the other device has no copy of your long recording, so the clip is cut here before the dataset is shared or synced.

## Comparing what you said to the text

The **diff view** lines up the written text against what was recognised from your recording, so you can see exactly which words you missed or mispronounced rather than just getting a single number. This is the most useful part of the exercise — it turns a score into concrete feedback.

## Every model's score, and the one that counts

The score depends on the speech model that heard you: the same take transcribed by two models can come out tens of points apart, because the comparison is against whichever transcript that model produced. That is information, not noise, so FmS keeps all of it. Each attempt carries one **measurement** per model — its score, and where relevant its pace and pronunciation — and lists them under the result, best first. Your **final score is the best** of those measurements, and that is the number XP is awarded on.

Two consequences are worth knowing:

- **Measuring with a weaker model cannot lower your score.** Running **Assess again** on another model adds a row to the list; it never replaces a better number you already earned. Right after a take, the badge beside the diff is the number *that* model gave — so it can read lower than the attempt's final score, which is the best of the list and what the history and XP use. The list under the result names every model that has heard the take and marks the best.
- **Edit the text and the list starts over.** Scores measured against the old words are not comparable to scores measured against the new ones, so changing a text's reference clears its breakdown rather than ranking numbers that mean different things.

## Attempts and history

Every recording you make is saved as an **attempt** — saved the moment you press **Stop**, before anything is scored, so nothing you recorded can be lost to a failed or abandoned measurement. The **attempts panel** lists your previous attempts for a text so you can:

- Read all three scores — accuracy, fluency and pronunciation — straight off each card, without opening the attempt. A score that could not be measured reads `—`, never a stand‑in number; hover it for the reason. A take whose measurement is still outstanding — you chose **Not now**, or the scoring did not finish — carries a **not measured** chip, and its transcript reads **not transcribed yet** until something runs over it.
- Play back earlier recordings and compare them to now.
- Watch your scores trend upward as you repeat a text.
- **Assess again** an earlier take: it re‑transcribes the recording and recomputes its accuracy, fluency and pronunciation in place — the same measurement a fresh take gets, so it is also how a **not measured** attempt finally gets its numbers. It is also how you score a take with a *different* model: each model's measurement is kept beside the others, and the best of them stays the attempt's final score. The recording travels to your other devices via sync (the `attempt_audio` plane), so once a sync round has delivered it the action works wherever the attempt is — if it is still missing, run a sync to fetch the take. The new result syncs back too: after a round, the re-assessed score and metrics appear on every device that holds the attempt, the newest assessment winning. An **Ask agent** feedback report rides the same row, so it reaches your other devices as well — and because it quotes the numbers it was written from, re-assessing a take clears the now-stale report on every device (asking again is one click).
- Navigate between attempts and, where relevant, navigate to the sentences that most need work.

Assessment can take a while (transcription first, then the phoneme model). An **assessment log** docked at the bottom of the attempts page tails each stage as it runs, so the wait is never silent; collapse it to a single line or close it once the run finishes.

Above the history sits a **Standard audio check** card. It scores the text's standard audio itself — the clip your pronunciation is measured against — through the same three axes a take goes through, so you can tell a bad reference from a bad reading. Nothing it produces is stored: no attempt is added and no XP is awarded. Read it as a health check: **accuracy** near 100% means the clip says what the text says, **pronunciation** high means the clip aligns cleanly against itself (a low one points at the phoneme model), and **fluency** paces the clip's own recording. A low accuracy is the finding that matters — that clip would mark down everyone who reads the text correctly, so regenerate or replace it in **Manage**.

Re‑reading the same text is encouraged — repetition with feedback is how fluency builds.

## Creating and editing texts

You can add your own Read Aloud datasets and texts. A dataset dialog manages the collection, and each text is edited on its own page, opened with **Manage** in the practice toolbar (or **New Text** for a fresh one). The page is a short stack of cards, so you can see what you are changing:

- **Details** — title and note.
- **Text to read** — the content you record, in the same Markdown editor used in [Wiki](wiki.md).
- **Scoring reference** — a **normalized reference**: a spelling‑out of numbers and abbreviations, which is what scoring and the generated standard audio read when it is set. Under the box, *Scoring will read* names which of the two the scorer will actually use and shows it, so you never have to guess between them. Two buttons fill the box for you: **Generate with Ollama** asks a local model, and **Copy prompt** copies the exact prompt to paste into ChatGPT or any other LLM — its answer goes back into the box. Either way it is still a draft, stored only when you **Save**; and when the text already has recordings, **Measure** scores what is in the box against them so you can see whether it actually helps.
- **Standard audio** — the reference clips below.

The first three are one draft: **Save** writes them and leaves you on the page (the header confirms it), **Close** discards them. Standard audio is separate — clips take effect immediately.

Beside the cards is a **Sections** list. The leftmost button in the toolbar shows and hides it: on a PC it is open to begin with, on a phone it slides in over the page and closes again once you pick something. Choose a section to jump to it — as you scroll, the list marks the card you are reading. It jumps rather than hides, because the cards are one draft: the note on the audio card about saving first only makes sense next to the fields it is about.

A whole book is better delegated than typed: an agent can extract the text, split it, import it and make the standard audio, and [AI Chat & Agents](ai_chat_agents.md#example-build-a-read-aloud-dataset-from-an-ebook) has a prompt that did exactly that — ending with one probe clip for you to approve before the rest.

## XP

Completing read‑aloud attempts contributes to your progress, and XP earned here (like the other modes) is recorded centrally and can sync across devices when you use a Hub. XP follows the *measurement*, not the recording: a take earns its +1 (at 60%) and +1 more (at 80%) when its score is computed, so an attempt you left **not measured** earns nothing until **Assess again** gives it a number.

## Related

- Choose and manage the speech‑to‑text model behind scoring: [Models](models.md).
- Reading for comprehension instead of speaking: [Books](books.md).
