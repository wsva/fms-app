# Language Learning Platform — Design & Roadmap

A desktop-first language learning platform built around **listening, speaking, reading, and
vocabulary**, deeply integrated with AI and driven by a strong **motivation** loop.

The current implementation is **fms-app**, a Tauri v2 desktop application (Rust backend +
Next.js/React frontend). This document describes the full vision, how it maps onto the existing
codebase, and a phased roadmap to get from here to there.

> **Status legend**
> ✅ Implemented · 🟡 Partial / in progress · ❌ Planned / not started

---

## 1. Guiding Principles

These four points drive every design decision. When a feature idea competes for effort, it is
measured against these.

1. **Practice listening and speaking** — the core skill loop. Passive consumption is not enough;
   the app must force active recall and production.
2. **Learn new words and review them effectively** — vocabulary acquisition with spaced
   repetition, tied to the context in which a word was actually encountered.
3. **Integrate AI** — AI is not a bolt-on chat window; it powers transcription, comprehension
   questions, word explanations, and text analysis across every feature.
4. **Give the user motivation** — *the most important problem.* A learning tool that is not used
   teaches nothing. Retention mechanics (reminders, social, rewards, gamification) are a
   first-class concern, not an afterthought.

---

## 2. Learning Materials

Content the platform can operate on:

1. **Textbook audio + text** — structured curriculum material.
2. **Audiobooks** — long-form native content, split into chapters/sentences.
3. **Internet audio, video, and text/news** — authentic material captured from the web.

All material is normalized into the **dataset** format (see §6) so that every downstream feature
(dictation, comprehension, reading, vocabulary) works on a common data model regardless of source.

---

## 3. Feature Areas

### 3.1 Listening

| Feature | Description | Status |
|---------|-------------|--------|
| **Dictation** | Play media, transcribe what you hear, compare against reference cues with waveform + cue-card UI. | ✅ |
| **Speaking practice** | Voice input transcribed by the local STT model and compared to the target cue (practice pronunciation by speaking instead of typing). Global `Ctrl+C` shortcut + toolbar mic button. | ✅ |
| **Comprehension** | Generate questions from the transcript with AI to test understanding (not just word-for-word recall). | ❌ |

**Dictation flow** is the most mature part of the app:
`src/components/listen_speak/dictation/` (`DictationPage`, `CueEditor`, `Subtitle`,
`WaveformCanvas`) backed by `src-tauri/src/dictation.rs`.

**Comprehension flow (planned):**

1. Get an audio clip (already in a dataset).
2. Run STT → transcript (already available).
3. Send the transcript to the LLM → generate questions + reference answers.
4. Listen and answer; the app grades understanding.

The LLM plumbing already exists (`llm_chat`, Ollama-backed). What's missing is the
question-generation prompt pipeline, a storage table for Q&A sets, and a UI to take the quiz.

### 3.2 Reading

| Feature | Description | Status |
|---------|-------------|--------|
| **Read a book** | Digitalize a book section by section; browse chapters, sentences, and saved words; generate/import per-sentence audio via TTS. | ✅ |
| **Web text / news** | Capture a text from the internet or a newspaper, read it, take notes, learn new words and expressions. | 🟡 |

Reading is implemented under `src/components/read_book/` (`BookManager`, `ReadingView`,
`ParagraphList`, `SentenceDrawer`) backed by `src-tauri/src/book.rs`. Books have chapters →
sentences → words, and sentences can be voiced with Edge TTS (`book_write_audio`,
`book_import_audio`).

**Gap:** ingesting arbitrary web text (clip an article / paste a URL) into a book-like reading
surface with note-taking. Today content must be entered/imported manually.

**Future:** AI text analysis — difficulty grading, expression/collocation extraction, automatic
glosses, sentence breakdowns.

### 3.3 Words / Vocabulary

**Decision:** vocabulary review is delivered by **embedding an external card/SRS website by URL**,
not by building a native flashcard system. The app focuses on *capturing* words in context and
*explaining* them with AI; the actual spaced-repetition review is delegated to a best-in-class
external service loaded in-app.

| Feature | Description | Status |
|---------|-------------|--------|
| **Word saving in context** | Save a word from a book sentence, with its surrounding context. | ✅ (within Read-a-Book) |
| **AI word explanation** | Explain a word (definition, usage, examples) via the LLM. | ❌ |
| **Embedded external card site** | Load an existing web-based card/SRS service by URL inside the app instead of building native pages. | ❌ |
| **Hand-off to external SRS** | Push words saved in the app into the embedded external service. | ❌ |

**Possible idea (no plan yet):** packaging vocabulary into shareable "word datasets" with
milestone/review sections — see §10.

### 3.4 Motivation & Reminders ❌

The most important problem, and currently the least implemented.

**Decision: local-first.** The motivation loop runs entirely on-device first, so it can ship
without waiting for the online backend. Social and reward mechanics are deferred as later ideas.

**Local-first (planned):**
1. **Popups / reminders on PC** — desktop notifications nudging a daily practice streak.
2. **Gamification** — inspiration from video-game design: streaks, XP, levels, achievements,
   visible progress bars, milestones tied to dataset sections. All persisted locally.

**Possible ideas (no plan yet — would depend on the online layer, §5.2):**
3. **Friends & user network** — social accountability, shared progress, light competition.
4. **Profits / rewards** — tangible incentives for sustained use.

---

## 4. AI Strategy

AI capabilities are sourced three ways, and the app should gracefully use whichever is available:

1. **Self-deployment in LAN** 🟡 — use old/spare PCs on the local network to run heavy compute
   (STT/TTS/LLM). The `web_service` module already exposes STT, TTS, and dataset endpoints over
   LAN (`local_url`, `lan_url`) via an axum server, so one machine can serve others. What's
   missing is discovery/orchestration (a client machine auto-finding and offloading to a LAN
   compute node).
2. **Built into the package** ✅ — bundled local models: STT via `transcribe-rs` (ONNX, Parakeet
   family) and TTS via `edge-tts`. Works fully offline on the local machine.
3. **External APIs** ✅ — Ollama or public AI services for the LLM. Implemented through
   `llm_chat` (check connection, list/pull/delete models, chat).

**Model lifecycle** (download with progress/resume, start/stop, delete, version select) is
implemented in `src-tauri/src/model*.rs` and surfaced in the STT Models page.

---

## 5. Architecture

### 5.1 Local app ✅

Tauri v2 desktop application.

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Tauri 2.x, rusqlite (bundled SQLite) |
| Frontend | React 19, Next.js 15 (App Router, static export) |
| UI | HeroUI v3, Tailwind CSS v4 |
| AI (local) | `transcribe-rs` (STT), `edge-tts` (TTS), Ollama client (LLM) |
| LAN service | axum HTTP server (`web_service`) exposing STT/TTS/dataset |

Navigation is a single-page shell with a sidebar. Current tabs:

- **Root:** Dictation ✅, Read a Book ✅, Settings ✅
- **Tools group:** Dictation Datasets ✅, STT Models ✅, Edge TTS ✅, LLM Chat ✅, Web Service ✅

Backend modules (`src-tauri/src/`): `model`, `model_download`, `model_list`, `settings`,
`dataset`, `dictation`, `book`, `tools`, `edge_tts`, `llm`, `llm_model_list`, `auth`, `audio`,
`web_service`.

### 5.2 Online website ❌ (partial foundations 🟡)

The forward-looking social/motivation layer:

- **User accounts** 🟡 — OAuth2 login is implemented (`auth_login`, `auth_get_user`,
  `auth_logout`), giving a user identity to build on.
- **User network / friends** ❌
- **Comments system** ❌
- **Dataset & word-set sharing** ❌ (a P2P file-sharing design exists in `.qoder/plans/` but is
  not wired into the current command set).

The online layer is what unlocks cross-device progress, social motivation, and community-shared
datasets.

---

## 6. Data Model & Sharing

The **dataset** is the universal content container. Full spec: [`dataset.md`](./dataset.md).

Summary of a dataset directory:

```
<dataset>/
├── info.json          # metadata: uuid, name, description, version, structure
├── data.sqlite3       # listen_media, listen_subtitle, listen_subtitle_cue
├── media/             # audio/video files
├── subtitle/          # VTT files (one per media)
├── waveform/          # JSON waveforms (generated in-app via Symphonia)
├── transcript/        # (optional) transcript text
└── book.txt           # (optional) full text for alignment
```

**Two databases:**

1. **Dataset DB** (`<dataset>/data.sqlite3`) — media, subtitles, cues for that dataset.
2. **App DB** (`data_dir()/fms-app/app.sqlite3`) — cross-dataset state such as dictation progress.

**Dataset pipeline** ✅ (triggered from the Datasets page):

1. **Import** → read `info.json`, register the dataset.
2. **Generate subtitles** → STT transcription → VTT.
3. **Generate waveforms** → Symphonia peak detection → JSON.
4. **Generate database** → parse VTT → cues into `data.sqlite3`.
5. *(optional)* **Transcripts / Book / Align** → write transcripts, split a book into sentences
   (built-in Rust or bundled `split_book.py` with NLTK), align cues to subtitles
   (`write_transcripts.py`; cue alignment is built-in Rust).

**Sharing** ❌ — datasets (and future word sets) are designed to be shareable units. Planned
mechanisms: LAN distribution via `web_service`, P2P transfer, and/or an online catalog tied to
user accounts.

---

## 7. Workflows

### 7.1 Dictation ✅
`audio → STT → subtitle/cues → dictation (type or speak; speaking uses STT to compare).`

### 7.2 Comprehension ❌
`audio → STT → transcript → LLM generates Q&A → user listens and answers → graded.`

### 7.3 Read a book ✅ / 🟡
`digitalize book section by section → read → save sentences/words → TTS audio per sentence.`
`Future: AI analyses the text (difficulty, expressions, glosses).`

### 7.4 Word → external card site ❌
`explain word with AI → save in context → hand off to an embedded external card/SRS site (loaded by URL).`
`Possible idea (no plan): package words into shareable word datasets with review milestones.`

---

## 8. Implementation Status Snapshot

| Area | Feature | Status |
|------|---------|--------|
| Listening | Dictation + waveform + cue editor | ✅ |
| Listening | Speaking practice via voice input | ✅ |
| Listening | AI comprehension questions | ❌ |
| Reading | Read a book (chapters/sentences/words + TTS) | ✅ |
| Reading | Web/news clipping + notes | 🟡 |
| Reading | AI text analysis | ❌ |
| Vocabulary | Save word in context | ✅ (in Read-a-Book) |
| Vocabulary | Embedded external card/SRS site (by URL) | ❌ |
| Vocabulary | AI word explanation | ❌ |
| Data | Dataset pipeline (subtitles/waveform/DB/align) | ✅ |
| Data | Dataset sharing (LAN / P2P / online) | 🟡 LAN only |
| AI | Bundled local STT/TTS | ✅ |
| AI | Ollama / external LLM | ✅ |
| AI | LAN compute offloading | 🟡 (server side only) |
| Motivation | Local reminders + gamification (local-first) | ❌ |
| Motivation | Social / rewards (possible idea, no plan) | ❌ |
| Platform | Local desktop app | ✅ |
| Platform | OAuth2 login | ✅ |
| Platform | Online website / user network / comments | ❌ |

---

## 9. Roadmap

### Phase 1 — Deepen the core loop (local, near-term)
- **Comprehension questions**: LLM prompt pipeline + Q&A storage + quiz UI on top of existing
  datasets and `llm_chat`.
- **Web/news clipping**: import a URL or pasted text into a reading surface with note-taking.

### Phase 2 — Vocabulary via embedded external SRS
- Embed an external card/SRS website by URL inside the app (no native flashcard pages).
- **AI word explanation** wired into the word drawer and the embedded flow.
- Hand-off: push words saved in Read-a-Book into the external service.

### Phase 3 — Motivation layer (local-first)
- Desktop reminders / notifications and a visible daily streak, persisted locally.
- Gamification: XP, levels, achievements, progress tied to dataset sections.
- No dependency on the online backend.

### Later ideas — no committed plan
Kept as possibilities, not scheduled:
- **Online & social**: public website/backend, friends, user network, comments, cloud sync.
- **Sharing**: online dataset catalog and/or P2P transfer (a libp2p design exists in
  `.qoder/plans/`).
- **Rewards / "profits"** as a motivation mechanic.
- **Word datasets**: shareable vocabulary sets with milestone/review sections.
- **Distributed AI**: LAN compute-node discovery/offloading so a spare PC runs heavy STT/TTS/LLM
  for lighter clients.

---

## 10. Decisions & Open Ideas

### Decided
1. **Vocabulary → embed an external card/SRS site by URL.** No native flashcard system; the app
   captures and explains words and delegates review to an embedded external service.
2. **Motivation → local-first.** Reminders and gamification run entirely on-device and ship without
   the online backend.

### Possible ideas — no plan yet
- **Dataset / word-set sharing transport** — LAN, P2P (libp2p), or an online catalog. No approach
  chosen.
- **"Profits" / rewards as motivation** — meaning still undefined (real money, in-app currency, or
  unlocking content); product/legal implications to be worked out later.
- **Word datasets** — shareable vocabulary sets with milestone/review sections.
- **Friends / user network / comments** — deferred until the online layer is pursued.

### Still to decide
- **Content ingestion scope** — how much effort goes into web/news clipping vs. textbooks and
  audiobooks (authentic web content raises copyright and parsing complexity).
- **Target platforms** — desktop (Tauri) is the current focus; are mobile/web clients in scope for
  reminders/streaks, which are most effective on a phone?
