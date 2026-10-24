# Read Aloud

Read Aloud is a speaking exercise: you record yourself reading a passage, and FmS uses speech‑to‑text to score how closely what you said matches the written text. It's designed to build pronunciation and fluency, and to let you track improvement over time.

Three words are used throughout, with one meaning each: an **attempt** is one submission (recording plus transcript plus scores), a **recording** is the audio that attempt captured, and a **standard audio** is a reference clip the *dataset* owns, which pronunciation is measured against.

## Getting set up

Read Aloud works with a **Read Aloud** dataset — a collection of texts plus a place to store your recordings. The recordings are saved in your workspace's **recordings directory** (set in [Settings](workspaces_accounts_settings.md)). Before recording, make sure your microphone is chosen and tested in Settings → **Microphone**.

Open **Read Aloud** from the sidebar and pick a dataset (and, within it, the text you want to read). Use the toolbar to manage datasets and locations.

## Practicing a text

For the selected text you'll see the passage and a **record** control:

1. Read the passage aloud while recording. A **live recognition** panel at the bottom of the page shows the transcript as you speak, so you can see immediately whether the model is hearing you — and whether it is hearing the right thing.
2. Stop when finished. FmS transcribes your recording and compares it to the target text.
3. A **score** badge shows how close you were, colour‑coded so you can see at a glance whether it was strong or needs work.

The live panel is read‑only: it is there to catch a silent microphone or a model in the wrong language while there is still time to start over. The attempt itself always ends with **Stop**.

The **practice panel** holds the text, the recorder, and the scoring view.

## Standard audio (needed for pronunciation)

Accuracy — how close your words were — needs nothing but the text. **Pronunciation** — whether it *sounded* right — needs something to compare you against: a clip of the text read clearly and correctly. That clip is called the text's **standard audio**, and a text with none cannot be scored for pronunciation at all; the card says so instead of inventing a number.

To make one, open the practice toolbar's **Manage** button for the text and use the **Standard audio** section. There are two ways:

- **Generate with Edge TTS** — synthesises the text in the voice chosen on the [Edge TTS](ai_chat_agents.md#edge-text-to-speech) page (pick another voice in the dialog if you like). This is the quick route, and it is also the accurate one for per‑word detail, because the synthesis reports exactly when each word is spoken.
- **Upload audio** — pick a recording of the text read by a person (your teacher, a native speaker, yourself being careful), then drag the two handles on the waveform to the part that reads the text and press **Save clip**. Only the selection is stored; nothing else of the file is kept. Per‑word detail for an upload is estimated in proportion to the time each word takes, so the overall score is unaffected but a single word's score is approximate.

A text can have several clips — try voices, or keep a human reading beside a synthesised one — and exactly one is the **Default**, the only one pronunciation is measured against. Use **Set default** to switch, **Play** to compare, and the bin icon to delete. The texts list marks a text that has one with a small audio icon.

If you later edit the text, its clip is marked **Stale**: the words no longer match what the clip says, so pronunciation stops being scored until you make a new clip. That is deliberate — grading you against a target for a passage you are no longer reading would be worse than a blank. Nothing is regenerated behind your back.

Clips belong to the dataset, so they travel with it: another device that syncs the dataset gets the same standard audio, and your score means the same thing there.

## Comparing what you said to the text

The **diff view** lines up the written text against what was recognised from your recording, so you can see exactly which words you missed or mispronounced rather than just getting a single number. This is the most useful part of the exercise — it turns a score into concrete feedback.

## Attempts and history

Every recording you make is saved as an **attempt**. The **attempts panel** lists your previous attempts for a text so you can:

- Play back earlier recordings and compare them to now.
- Watch your scores trend upward as you repeat a passage.
- Navigate between attempts and, where relevant, navigate to the sentences that most need work.

Re‑reading the same passage is encouraged — repetition with feedback is how fluency builds.

## Creating and editing texts

You can add your own Read Aloud datasets and texts. A dataset dialog manages the collection, and each text can be edited (the same Markdown editor used in [Wiki](wiki.md)), so you can import or write the passages you want to practice speaking. A text can also carry a **normalized reference** — a spelling‑out of numbers and abbreviations — which is what scoring and the generated standard audio read when it is set.

## XP

Completing read‑aloud attempts contributes to your progress, and XP earned here (like the other modes) is recorded centrally and can sync across devices when you use a Hub.

## Related

- Choose and manage the speech‑to‑text model behind scoring: [Models](models.md).
- Reading for comprehension instead of speaking: [Read a Book](read_a_book.md).
