# Tools

The **Tools** group in the sidebar holds FmS's utilities — helpers that support the practice modes or work with content on its own. This page covers the standalone tools; the ones with bigger roles of their own (Workflow, Models, sync, chat) are linked at the end.

## OCR

**Tools → OCR** recognises text inside images, so you can turn a screenshot, a photo of a page, or a scanned exercise into editable text.

- **Load an image** from a file, **capture a screenshot** / clipboard image directly, or drop an image onto the page.
- Choose the **language(s)** to recognise, then run recognition.
- Two result modes share the same image and languages, with **TSV** as the default:
  - **TSV** — Tesseract's layout dump, rebuilt in Rust: words are regrouped into lines and paragraphs, and dashes (— – ―) are spaced off their neighbours. Every word hyphenated across a line break is listed under the result: it is joined into one word when a dictionary of the selected languages knows it (`Sprach-` + `verarbeitung` → `Sprachverarbeitung`), and kept as printed otherwise (`E-` + `Mail`). Flip a single decision, use **Join all** / **Keep all**, or install a missing dictionary with one click from the list — without one, a shape rule takes over (a lowercase continuation joins, a capitalised one is kept). It needs no model, and **Raw TSV** shows the layout it worked from.
  - **Plain** — Tesseract's flat text, kept for pages the layout rebuild cannot order reliably (two columns, right-to-left scripts) and for poetry or code, where line breaks carry meaning. A **Fix with LLM** pass can then clean up obvious errors.
- The result is shown as editable text you can copy; the source image stays visible beside it, and the text size can be adjusted in the result footer. `Ctrl/⌘+Enter` runs recognition, `Ctrl/⌘+Shift+C` copies the result.

OCR needs a recognition model — if it says one is missing, download it from [Models](models.md). (Desktop feature.)

### Hyphen dictionaries

The TSV rebuild repairs words broken across a line break with a **Hunspell dictionary**, and one is downloaded per language — 15 are available (German, English, French, Spanish, Italian, Portuguese, Dutch, Swedish, Danish, Polish, Czech, Russian, Ukrainian, Greek, Turkish). They are data under their own licences, so they are not bundled with the app.

**Dictionaries** in the toolbar lists them all with their version, licence and download size; install or remove any of them there. You do not have to install one per page: the languages you select for recognition are exactly the dictionaries consulted, so `deu+eng` uses both — a German text quoting `Workflow-` + `Editor` still joins correctly when only the English dictionary knows the word. Installed dictionaries are used automatically and live in the app data folder; deleting one just means the shape rule decides again. Languages with no dictionary of their own (Japanese, Chinese, Arabic…) are shown as using the shape rule, which is all their scripts need — they do not hyphenate at the margin.

## Split Text

**Tools → Split Text** runs the **SaT** sentence‑segmentation model on a block of text: paste or upload text (`.txt`, `.md`, `.vtt`, `.srt`) and FmS breaks it into clean sentences, which you can copy all at once or one at a time. **Threshold**, **Strip whitespace** and **Split on newlines** tune the result — a lower threshold produces more breaks, and your settings are remembered between sessions. `Ctrl/⌘+Enter` splits, `Ctrl/⌘+Shift+C` copies every sentence. It's the same engine used to prepare Book datasets, exposed here as a standalone utility — handy for tidying a raw text before turning it into a book or a wiki. The model download is managed on the [Models](models.md) page. (Desktop feature.)

## PDF Reader

**Tools → PDF Reader** opens and reads PDF documents inside FmS, so reference material, scanned books, or handouts are available without leaving the app.

## Simple Words

**Tools → Simple Words** maintains your list of **known simple words**, per language, sourced from the card datasets you select. Marking a word as already‑known tells features like **word generation** and dictation helpers to skip vocabulary you've mastered, so they focus on what's actually new to you. You can add words, browse the set, and pick the active language and source datasets here. It works hand‑in‑hand with [Cards](cards.md) and [Books](books.md).

## Logs

**Tools → Logs** shows the app's own activity log — informational messages and errors — in a scrollable view. It's the first place to look when something misbehaves: dataset processing failures, sync errors, and model problems leave traces here. Entries are kept in a rotating log file as well as this in‑app view. (Desktop feature.)

## Tools that have their own pages

A few items in the Tools group are large enough to deserve dedicated chapters:

- **Workspaces** — profiles and multi‑user setup → [Workspaces, Accounts & Settings](workspaces_accounts_settings.md).
- **Workflow** — the step‑by‑step dataset processing pipeline → [Datasets & the Processing Pipeline](datasets.md).
- **Models** — speech‑to‑text and other model management → [Models](models.md).
- **Devices & Hub**, **Datasets Sync** — multi‑device sync → [Using FmS on Multiple Devices](multiple_devices.md).
- **Cross‑Device Chat**, **Edge TTS** — AI and voice → [AI Chat, Agents & Voice](ai_chat_agents.md).

## On Android and the website

Several Tools are desktop‑only because they need local file access or local models: **OCR**, **Split Text**, **Simple Words**, **Edge TTS**, **Workflow**, and **Logs** do not appear in the Android thin client. The website has its own smaller tool set (Plan, Learning Stats, PDF Reader) described in [The FmS Website](website.md).
