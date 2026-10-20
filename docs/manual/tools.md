# Tools

The **Tools** group in the sidebar holds FmS's utilities — helpers that support the practice modes or work with content on its own. This page covers the standalone tools; the ones with bigger roles of their own (Workflow, Models, sync, chat) are linked at the end.

## OCR

**Tools → OCR** recognises text inside images, so you can turn a screenshot, a photo of a page, or a scanned exercise into editable text.

- **Load an image** from a file, or **capture a screenshot** / clipboard image directly.
- Choose the **language(s)** to recognise, then run recognition.
- The result is shown as editable text you can copy; a light image editor lets you zoom and adjust the source before recognising.

OCR needs a recognition model — if it says one is missing, download it from [Models](models.md). (Desktop feature.)

## Split Text

**Tools → Split Text** runs the **SaT** sentence‑segmentation model on a block of text: paste or upload text and FmS breaks it into clean sentences, which you can copy back out. It's the same engine used to prepare Book datasets, exposed here as a standalone utility — handy for tidying a raw text before turning it into a book or a wiki. The model download is managed on the [Models](models.md) page. (Desktop feature.)

## PDF Reader

**Tools → PDF Reader** opens and reads PDF documents inside FmS, so reference material, scanned books, or handouts are available without leaving the app.

## Simple Words

**Tools → Simple Words** maintains your list of **known simple words**, per language, sourced from the card datasets you select. Marking a word as already‑known tells features like **word generation** and dictation helpers to skip vocabulary you've mastered, so they focus on what's actually new to you. You can add words, browse the set, and pick the active language and source datasets here. It works hand‑in‑hand with [Cards](cards.md) and [Read a Book](read_a_book.md).

## Logs

**Tools → Logs** shows the app's own activity log — informational messages and errors — in a scrollable view. It's the first place to look when something misbehaves: dataset processing failures, sync errors, and model problems leave traces here. Entries are kept in a rotating log file as well as this in‑app view. (Desktop feature.)

## Tools that have their own pages

A few items in the Tools group are large enough to deserve dedicated chapters:

- **Workspaces** — profiles and multi‑user setup → [Workspaces, Accounts & Settings](workspaces_accounts_settings.md).
- **Workflow** — the step‑by‑step dataset processing pipeline → [Datasets & the Processing Pipeline](datasets.md).
- **Models** — speech‑to‑text and other model management → [Models](models.md).
- **Devices & Hub**, **Datasets Sync** — multi‑device sync → [Using FmS on Multiple Devices](multiple_devices.md).
- **Cross‑Device Chat**, **LLM Chat**, **Edge TTS** — AI and voice → [AI Chat, Agents & Voice](ai_chat_agents.md).

## On Android and the website

Several Tools are desktop‑only because they need local file access or local models: **OCR**, **Split Text**, **Simple Words**, **Edge TTS**, **LLM Chat**, **Workflow**, and **Logs** do not appear in the Android thin client. The website has its own smaller tool set (Plan, Learning Stats, PDF Reader) described in [The FmS Website](website.md).
