# Read Aloud

Read Aloud is a speaking exercise: you record yourself reading a passage, and FmS uses speech‑to‑text to score how closely what you said matches the written text. It's designed to build pronunciation and fluency, and to let you track improvement over time.

Three words are used throughout, with one meaning each: an **attempt** is one submission (recording plus transcript plus scores), a **recording** is the audio that attempt captured, and a **standard audio** is a reference clip the *dataset* owns, which pronunciation is measured against.

## Getting set up

Read Aloud works with a **Read Aloud** dataset — a collection of texts plus a place to store your recordings. The recordings are saved in your workspace's **recordings directory** (set in [Settings](workspaces_accounts_settings.md)). Before recording, make sure your microphone is chosen and tested in Settings → **Microphone**.

Open **Read Aloud** from the sidebar and pick a dataset (and, within it, the text you want to read). Use the toolbar to manage datasets and locations.

## Practicing a text

For the selected text you'll see the passage and a **record** control. When the text has a **standard audio** clip, a player for it sits directly above the passage — listen first to learn the pacing, then record:

1. Read the passage aloud while recording. A **live recognition** panel at the bottom of the page shows the transcript as you speak, so you can see immediately whether the model is hearing you — and whether it is hearing the right thing.
2. Stop when finished. FmS transcribes your recording and compares it to the target text.
3. A **score** badge shows how close you were, colour‑coded so you can see at a glance whether it was strong or needs work.

The live panel is read‑only: it is there to catch a silent microphone or a model in the wrong language while there is still time to start over. The attempt itself always ends with **Stop**.

The **practice panel** holds the text, the recorder, and the scoring view.

Once a text is open, the toolbar's **Refresh** re‑reads everything about it — its stored fields, its standard audio and its attempt history — and it means the same thing on the attempts page and on one open attempt, so a clip or a recording that changed elsewhere shows up without leaving the page.

## Standard audio (needed for pronunciation)

Accuracy — how close your words were — needs nothing but the text. **Pronunciation** — whether it *sounded* right — needs something to compare you against: a clip of the text read clearly and correctly. That clip is called the text's **standard audio**, and a text with none cannot be scored for pronunciation at all; the card says so instead of inventing a number.

To make one, open the practice toolbar's **Manage** button for the text and use the **Standard audio** card at the bottom of that page. If you have unsaved edits, **Save** first: a clip describes the text as it is stored, so while the draft differs those controls are greyed out. There are two ways:

- **Generate with Edge TTS** — synthesises the text in the voice this dataset's **language** prefers. The language is a property of the dataset (set when it is created, and editable from the picker here); the *voice* for that language is set once, on the [Edge TTS](ai_chat_agents.md#edge-text-to-speech) page, and every generation that needs a German voice gets the same one instead of whatever happens to sort first. The line above the button says which voice is in use and why — `German · de-DE-ConradNeural (preferred)` — and its **Change** link takes you to that page. The two pickers beside the button remain a one-off override: **language**, then a **voice** of that language; changing the language here also saves it to the dataset, which is what makes the preference reachable in the first place. This is the quick route, and it is also the accurate one for per‑word detail, because the synthesis reports exactly when each word is spoken.
- **Upload audio** — pick a recording of the text read by a person (your teacher, a native speaker, yourself being careful), then mark the part that reads the text: click or drag on the waveform to place the playhead where the reading begins and press **Set start**, let it play on and press **Set end**; or drag the two handles; or type the two times under the waveform, as `1:02.5` or `62.5`. The arrows either side of each button move that edge 100 ms at a time. If the file holds more than the text — a whole lesson, an interview — press **Guess from speech** instead: the speech model listens to the whole file, matches what it hears against the text, and moves both edges onto the reading. A note says how much of the text it heard; it refuses rather than guess when too little was heard, and it takes as long as the file is long. Then press **Save clip**. Only the selection is stored; nothing else of the file is kept. Per‑word detail for an upload is estimated in proportion to the time each word takes, so the overall score is unaffected but a single word's score is approximate.

A text can have several clips — try voices, or keep a human reading beside a synthesised one — and exactly one is **in use**: the circle beside a clip chooses the one pronunciation is measured against. Use **Play** to compare and the bin icon to delete; a clip whose audio file has not synced to this device yet says so, so an unplayable row is never mistaken for a bad one. A new clip always uses the voice resolved at the moment it is made, so changing the preference later never re‑voices an existing clip — delete and regenerate it if you want the new voice.

If you later edit the text, its clip is marked **Stale**: the words no longer match what the clip says, so pronunciation stops being scored until you make a new clip. That is deliberate — grading you against a target for a passage you are no longer reading would be worse than a blank. Nothing is regenerated behind your back.

Clips belong to the dataset, so they travel with it: another device that syncs the dataset gets the same standard audio, and your score means the same thing there.

## Comparing what you said to the text

The **diff view** lines up the written text against what was recognised from your recording, so you can see exactly which words you missed or mispronounced rather than just getting a single number. This is the most useful part of the exercise — it turns a score into concrete feedback.

## Attempts and history

Every recording you make is saved as an **attempt**. The **attempts panel** lists your previous attempts for a text so you can:

- Read all three scores — accuracy, fluency and pronunciation — straight off each card, without opening the attempt. A score that could not be measured reads `—`, never a stand‑in number; hover it for the reason.
- Play back earlier recordings and compare them to now.
- Watch your scores trend upward as you repeat a passage.
- **Assess again** an earlier take: it re‑transcribes the recording and recomputes its accuracy, fluency and pronunciation in place. The recording is device‑local, so the action only appears where that recording lives.
- Navigate between attempts and, where relevant, navigate to the sentences that most need work.

Assessment can take a while (transcription first, then the phoneme model). An **assessment log** docked at the bottom of the attempts page tails each stage as it runs, so the wait is never silent; collapse it to a single line or close it once the run finishes.

Above the history sits a **Standard audio check** card. It scores the text's standard audio itself — the clip your pronunciation is measured against — through the same three axes a take goes through, so you can tell a bad reference from a bad reading. Nothing it produces is stored: no attempt is added and no XP is awarded. Read it as a health check: **accuracy** near 100% means the clip says what the text says, **pronunciation** high means the clip aligns cleanly against itself (a low one points at the phoneme model), and **fluency** paces the clip's own recording. A low accuracy is the finding that matters — that clip would mark down everyone who reads the passage correctly, so regenerate or replace it in **Manage**.

Re‑reading the same passage is encouraged — repetition with feedback is how fluency builds.

## Creating and editing texts

You can add your own Read Aloud datasets and texts. A dataset dialog manages the collection, and each text is edited on its own page, opened with **Manage** in the practice toolbar (or **New Text** for a fresh one). The page is a short stack of cards, so you can see what you are changing:

- **Details** — title and note.
- **Text to read** — the passage you record, in the same Markdown editor used in [Wiki](wiki.md).
- **Scoring reference** — a **normalized reference**: a spelling‑out of numbers and abbreviations, which is what scoring and the generated standard audio read when it is set. Under the box, *Scoring will read* names which of the two the scorer will actually use and shows it, so you never have to guess between them. Two buttons fill the box for you: **Generate with Ollama** asks a local model, and **Copy prompt** copies the exact prompt to paste into ChatGPT or any other LLM — its answer goes back into the box. Either way it is still a draft, stored only when you **Save**; and when the text already has recordings, **Measure** scores what is in the box against them so you can see whether it actually helps.
- **Standard audio** — the reference clips below.

The first three are one draft: **Save** writes them and leaves you on the page (the header confirms it), **Close** discards them. Standard audio is separate — clips take effect immediately.

Beside the cards is a **Sections** list. The leftmost button in the toolbar shows and hides it: on a PC it is open to begin with, on a phone it slides in over the page and closes again once you pick something. Choose a section to jump to it — as you scroll, the list marks the card you are reading. It jumps rather than hides, because the cards are one draft: the note on the audio card about saving first only makes sense next to the fields it is about.

## XP

Completing read‑aloud attempts contributes to your progress, and XP earned here (like the other modes) is recorded centrally and can sync across devices when you use a Hub.

## Related

- Choose and manage the speech‑to‑text model behind scoring: [Models](models.md).
- Reading for comprehension instead of speaking: [Read a Book](read_a_book.md).
