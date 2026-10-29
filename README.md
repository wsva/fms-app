<div align="center">
  <img src="public/logo.png" alt="FmS logo" width="96" />
  <h1>FmS — fms-app</h1>
  <p><strong>Fremdsprachen machen Spaß!</strong> — Foreign languages are fun.</p>
  <p>
    A local language lab: turn the material you already own into <strong>dictation, reading,
    speaking and vocabulary practice</strong> — on your machine, offline, no account, no per-minute
    bill.
  </p>
  <p>
    Built first for the German learner heading towards Goethe or TestDaF.
  </p>
  <p>
    <a href="https://github.com/wsva/fms-app/releases/latest"><img src="https://img.shields.io/github/v/release/wsva/fms-app?label=release" alt="Latest release" /></a>
    <a href="https://github.com/wsva/fms-app/actions/workflows/build.yml"><img src="https://img.shields.io/github/actions/workflow/status/wsva/fms-app/build.yml?label=build" alt="Build status" /></a>
    <img src="https://img.shields.io/badge/Tauri-2.x-orange" alt="Tauri 2" />
    <img src="https://img.shields.io/badge/React-19-blue" alt="React 19" />
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License MIT" /></a>
  </p>
</div>

---

<table>
  <tr>
    <td><img src="docs/img/dictation.png" alt="Dictation: a cue card above the waveform, the words you missed lit up inside it, with the play and mic controls at its edge" /></td>
    <td><img src="docs/img/book.png" alt="Books: a chapter with the understanding panel open beside the text" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Dictation — type or speak what you hear</sub></td>
    <td align="center"><sub>Books — read, with an understanding panel</sub></td>
  </tr>
  <tr>
    <td><img src="docs/img/read_aloud.png" alt="Read Aloud: a text with its standard audio clip and the record control" /></td>
    <td><img src="docs/img/wiki.png" alt="Wiki: Markdown documents with a table of contents and full-text search" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Read Aloud — a text and its reference clip</sub></td>
    <td align="center"><sub>Wiki — Markdown notes, searchable and synced</sub></td>
  </tr>
</table>

## What is this?

FmS is a **local-first language lab**. You bring the material you already own — a textbook's MP3s,
an audiobook, a podcast, a lecture recording, a chapter of prose — and it becomes practice. Five
modes share one dataset layer, and all five are core: **dictation** (type or speak what you hear),
**books** (read, look up, understand), **read aloud** (speak and be scored), **cards** (review what
you kept) and **wiki** (the notes that tie it together). No upload, no per-minute bill, no account:
the models, the datasets and your progress live on the disk you already have.

**One library, several machines.** A desktop holds the authoritative copy and serves it; your phone
and a second PC follow it, pulling the datasets they need and sending their edits back. Practising on
any of them keeps the same dictation history, review schedule and XP — so the phone in your pocket is
the same library, not a copy you have to remember to update.

The [user manual](docs/manual/index.md) covers each mode in depth — start at
[Getting started](docs/manual/getting_started.md), then read the page for the mode you care about.

**Dictation is where it started.** Import the files, a local speech-to-text model cuts them into
cue-level subtitles, and you train by producing each cue yourself — typed, or spoken into the cue's
microphone and transcribed by the same model — revealing the reference when you are done.

**Between sessions it is a toolkit.** The same app recognises text in a screenshot, splits a raw
text into sentences, reads a PDF, speaks a sentence in a natural voice and keeps a searchable
wiki — the everyday jobs that earn it a permanent place on your machine.

**Who it's for:** the intermediate German learner — roughly B1 to C1 — working towards the
Goethe-Zertifikat or TestDaF, who wants to train on *real* speech (a novel read aloud, a news
podcast, a course's own drills) rather than the dozen canned clips that come with a textbook. The
method is not German-specific: it works for any language with a usable local STT model, and it is
the loop the old FSI and Peace Corps drill courses were built around.

**Why not something else?**
- **Anki** is a scheduler: it reviews cards you have already made and does not transcribe or cue
  audio.
- **LingQ** is reading-first — the text sits in front of you and you tap the words you don't know.
  FmS hides the text and asks you to produce it.
- **Language Reactor** runs on top of Netflix and YouTube, so the material has to live on those
  platforms, and the subtitles are given to you rather than being the thing you write.
- **Speechify** reads text aloud to you; FmS asks you to write down what you hear.

**Audio stays on the device**, and no model call is metered. The few features that do talk to the
network are listed under [Privacy](#privacy), and what you may safely import is covered under
[Bring your own content](#bring-your-own-content).

The app is a **Tauri v2** port of the web version at [wsva/fms](https://github.com/wsva/fms),
and ships for **Windows, Linux and macOS**, plus **Android and iOS** clients that practise a dataset
and sync it with a PC. Every target is built from this repository by
[GitHub Actions](.github/workflows/build.yml) on each `v*` tag.

## Install and first run

Grab a prebuilt installer from the [latest release](https://github.com/wsva/fms-app/releases/latest):

| Platform | Artifacts | Notes |
|----------|-----------|-------|
| Windows  | `.exe` (NSIS) | x64 |
| Linux    | `.AppImage`, `.deb`, `.rpm` | x64 |
| macOS    | `.dmg` | Apple Silicon only, macOS 11+, unsigned |
| Android  | `.apk` | arm64 |
| iOS      | `.ipa` | arm64, unsigned — re-sign it before it will install |

Each asset is named `FmS_<platform>_<arch>_<tag>.<ext>` — `FmS_windows_x64_v0.1.1.exe`,
`FmS_linux_amd64_v0.1.1.deb`, `FmS_macos_arm64_v0.1.1.dmg` — so the release tag is the only version
in the name. Updating, the Apple signing caveats and the per-platform prerequisites are further
below: [Updates](#updates), [Apple builds](#apple-builds), [Platform notes](#platform-notes).

### First five minutes

FmS ships no content. It has to be told where your material is and what to listen with, and a
fresh dataset has to be prepared before there is anything to practise.

1. **Point it at a folder.** *Settings → Datasets directory*; every dataset is a folder inside it.
   The Books, Recordings and Model directories sit on the same page.
2. **Download a speech model.** *Models*. Choose by language before size: the Moonshine models and
   Parakeet V2 are English-only, while **Parakeet V3** (456 MB) covers 25 European languages
   including German and the **Whisper** models cover 90+ in 465 MB to 1.5 GB. The download happens
   once; after that, transcription is offline.
3. **Add a dataset.** Copy a folder into the datasets directory, or import it from the Datasets page.
   It needs an `info.json` — [Datasets](docs/manual/datasets.md) describes the layout, and how to
   build one from nothing but audio.
4. **Prepare it.** *Generate Subtitles* → *Generate Waveforms* → *Generate Database*, one stage at a
   time on the dataset page, or the whole run at once on the **Workflow** page. The subtitle pass is
   the slow one on a long audiobook; *Logs* shows it working.
5. **Practise.** Open the Dictation page and start typing. Everything after that point is optional.

Signing in is optional, and it is not what makes a phone work: a hub and its followers talk over your
LAN or tailnet with a pairing code and no account at all. An account only ties XP to the website and
lets a hub publish its datasets there. [Getting started](docs/manual/getting_started.md) is the
longer version of these five steps.

## Features

**Five practice modes, all of them core** — each with its own dataset type, its own page, and the
same cross-device sync.

- **[Dictation](docs/manual/dictation.md)** — import what you own (`mp3`, `wav`, `m4a`/`m4b`
  audiobooks, `ogg`, `flac`, `mp4`, `webm`), let on-device STT cut it into cues, then answer each one
  by typing it or saying it into the cue's mic — the same local model writes down what you said.
  Canvas waveform, cue cards, per-cue progress, large and focus modes, and a word-level hint that
  lights up the words you missed.
- **[Books](docs/manual/books.md)** — split a text into chapters and sentences, read along with
  per-sentence TTS, look words up in context, and build its understanding layer: explanations,
  simplifications, a chapter digest, an argument map, a glossary, and comprehension quizzes.
- **[Read Aloud](docs/manual/read_aloud.md)** — record a text and have it scored on accuracy, fluency
  and pronunciation against a reference clip you generate with Edge TTS or cut out of your own
  recording by alignment. Feedback comes from a local model or an attached agent.
- **[Cards](docs/manual/cards.md)** — separate flashcard datasets with SM-2 review queues, FTS5
  search, tags and a hint field, for the vocabulary you collect from your own material.
- **[Wiki](docs/manual/wiki.md)** — syncable Markdown trees with full-text search: notes, grammar
  references, transcripts and the links between them, editable on the phone as well as on the PC.

Cards, read-aloud passes and book quizzes all pay into one **XP ledger**.

**Cross device — the same library on every machine**

- **A hub and its followers.** A workspace is either the **hub** — the authoritative copy, serving
  its datasets over the network — or a **follower** that pulls what it wants and pushes edits back.
  Roles belong to the workspace rather than the app, a phone is always a follower, and the service
  starts and stops with the role: there is no server to start by hand, and no port to open.
- **Found, then paired once.** Devices announce themselves on the LAN (with a direct TCP sweep when
  broadcasts are blocked) and over Tailscale. Pairing is one short code, shown on both screens and
  approved on the hub; afterwards sync needs no further approval. Loopback and tailnet peers skip
  pairing entirely, and a hub can revoke a device at any time.
- **Both halves travel.** Media, subtitles, cues, cards, books, read-aloud takes and wiki pages move
  as dataset content; dictation history, review scheduling, attempts and XP move as your own state. A
  conflict keeps the later edit, and a local edit that has not been pushed yet is never overwritten.
- **A wiki you have not downloaded.** On a follower the Wiki page reads and searches the hub's copies
  read-only; downloading one is what makes it editable, and the search index is rebuilt locally
  rather than transferred.
- **Chat between your own devices.** One thread with attachments, stored on the hub, for sending a
  file from the PC to the phone.

See [Multiple devices](docs/manual/multiple_devices.md).

**The layer they share**

- **One pipeline** — import → subtitles (STT) → waveforms → database, plus transcript alignment and
  book splitting: one click on the dataset page, or step by step on the **Workflow** page. See
  [Datasets & the pipeline](docs/manual/datasets.md).
- **Local models** — Parakeet ONNX and Whisper GGML for speech, SaT for sentence splitting, Tesseract
  for OCR — all downloaded and swapped from the Models page. See [Models](docs/manual/models.md).
- **Nothing is lost by accident** — a destructive action moves the dataset or file into the app
  trash, where you can restore it.

**[Everyday tools](docs/manual/tools.md) — the reason it stays installed**

- **OCR** — drop in a screenshot, a photo of a page or a scanned exercise and get editable text: pick
  the languages, and the layout comes back as lines and paragraphs instead of a word dump. Words
  broken across a line break are joined when a downloaded Hunspell dictionary knows the result
  (`Sprach-` + `verarbeitung` → `Sprachverarbeitung`) and kept as printed when it does not; 15
  dictionaries are available, a shape rule covers the rest, and an optional LLM pass cleans up
  obvious errors. Screenshot and clipboard capture are built in.
- **Edge TTS** — speak any text in a natural voice, in every language the app needs, with no API key
  (the text itself goes to Microsoft's Edge endpoint; see [Privacy](#privacy)).
- **Split Text** — run the sentence-segmentation model on any pasted text and get clean sentences
  back.
- **PDF Reader** — read documents and handouts without leaving the app.
- **Simple Words** — mark the words you already know, so word generation and dictation helpers stop
  offering them.
- **Voice input** — press the mic on a cue (or `Ctrl+C` in any text field) and your speech is
  transcribed locally and inserted at the cursor. In dictation that is the answering mode: say the
  cue instead of typing it.
- **[Workspaces](docs/manual/workspaces_accounts_settings.md)** — several learners or several
  projects, with separate settings, progress and datasets on one machine.

**Agent-first by design.** The practice modes and the dataset pipeline are exposed to agents as MCP
tools, alongside everything you can click — see [Built for agents](#built-for-agents).

**LLM features, no chat window.** A language model is called from the features that need one —
chapter digests, word explanations in Books, comprehension quizzes, OCR clean-up, read-aloud text
normalization, and feedback on a take. Point it at a local Ollama, or at a cloud provider through the
goose SDK layer; for open-ended questions use your own model or an attached agent.

## Built for agents

**The bet.** The agent will be the most important software on a personal computer — not a panel
bolted to the side of an app, but the place most people work from — and knowing how to direct one
will be a core ability, as running a search engine or a spreadsheet once was. FmS is built on that
assumption completely rather than cautiously: agent-friendliness is a first-class design constraint
here, weighted the same as being usable by a human, so a decision that serves only one of the two is
a decision not yet made. The rules live in
[`docs/agent_friendly_design.md`](docs/agent_friendly_design.md).

There is a second, more self-interested reason. A language app lives or dies between sessions: the
learner who has to find the right folder, import, wait, nurse a failed stage and cut reference clips
by hand eventually stops opening the app at all. Every one of those jobs can be handed to an agent
already sitting on the same machine. So whatever makes FmS legible to an agent also makes it used —
the two arguments point the same way, which is why a principle this broad is worth holding to.

What that means in practice:

- **One app, two tenants.** Everything you can do in the UI is also an MCP tool — datasets and the
  pipeline, dictation and cues, cards and review, books and their understanding layer, read-aloud
  scoring, wiki, OCR, models, sync, workflows, settings — and the page publishes what the user is
  looking at and which actions are on screen, so an agent can act on the screen in front of the
  human rather than guess at it.
- **The app documents itself.** It serves an agent manual (`/agent.md`, `/agent.json`), its own tool
  index, MCP resources for the domain rules (`fms://status`, `fms://docs/...`) and ready-made prompt
  recipes, so an agent that has never seen FmS can find the right call without being taught.
- **Proof, not trust.** Writes return JSON that proves them — a count, or a server-assigned id — and
  a bulk call returns `saved` / `failed` summing to what was sent. Errors name the next call to make
  instead of leaving an agent to guess.
- **Reversible by default.** Destruction is not gated behind a confirmation parameter: it moves the
  dataset or file into the app trash, where it can be restored. Every tool carries a tier — `read`,
  `write`, `destructive`, `ui-control`, `human-handoff` — and every call is written to a journal
  (`app_audit_get` reads it back), so `mcp_policy = "enforce"` can refuse the last two outright.
- **Humans stay where they matter.** Work that needs a person — record this take, approve this
  subtitle, judge whether this reference clip is right — is not forced through a tool call: an agent
  leaves it as a task in the app, deep-linked to the exact screen, and a workflow that ends in a
  `human` step stays parked until the task is ticked off, then continues. The agent dock shows the
  work while it happens.

**What you get from it.** An import that would be an evening of clicking becomes one conversation; a
pipeline stage that failed can be asked about rather than deciphered; material can be prepared —
imported, split, spoken with TTS, normalized — while you practise something else. Nothing about it
moves your data: the agent reaches the app by running on your machine, or over the pairing you
already use for your phone (see [Privacy](#privacy)).

## Privacy

FmS is **local-first, not local-only**. Transcription, cuing, review scheduling, scoring and OCR all
run on-device; a handful of features need the network, and each one is something you switch on.
There is no telemetry, no analytics, no crash reporting, and no account is needed to use the app.

| What you do | Leaves the device? | What is sent |
|---|---|---|
| Import, STT subtitles, waveforms, cue database, alignment | **No** | — |
| Dictation, cards, review scheduling, search, XP, wiki | **No** | — |
| Voice input, read-aloud scoring, OCR, book splitting | **No** | — |
| LLM features with Ollama on `localhost` (OCR clean-up, word explanations, read-aloud normalization) | **No** | — |
| The same LLM features with a cloud provider (`openai`, `anthropic`, `groq`, `databricks`) | Yes — only if you configure a key | The prompt, and the text it asks about |
| Speaking text aloud: Books, Read Aloud, Understanding panel, TTS page | Yes | The text to be spoken → Microsoft Edge TTS |
| Dataset sync to another of your devices | To that device only | The dataset snapshot and your progress, over your LAN or tailnet |
| Website account, dataset registry, mirror | Yes — opt-in | Login plus the dataset metadata you publish to `lusworkshop.site` |
| Model download | Yes — opt-in | A file request to `huggingface.co` or `blob.handy.computer` |
| Release/update check (Settings → About) | Yes | A request to `api.github.com`; no identifiers beyond the request itself |

**Audio never leaves the device.** Every speech-to-text path is on-device (ONNX and GGML models
downloaded once into your model directory; see the [manual](docs/manual/models.md)). Only *text* can
leave, and only via the rows above that say so.

The embedded MCP/HTTP server (port `35711`) is how an agent talks to the app. Loopback peers and
Tailscale addresses (`100.64.0.0/10`) are trusted without credentials — with a `Host`-header check so
a web page cannot reach it by pointing a public name at `127.0.0.1` — and every other peer must
present a paired device's Ed25519 signature; unpaired peers can read only `/api/v1/status`,
`/api/v1/cluster`, `/api/v1/pair/*` and the agent manual. The socket does listen on all interfaces,
because that is how a paired phone syncs, and a UDP beacon on `35712` advertises the machine on your
LAN so devices can find each other. Whatever you let a connected agent read does go to that agent —
that is the point of attaching one — so attach an agent on your own machine if you want that traffic
to stay there.

API keys and the website login token are stored in this app's own settings and auth files inside
your workspace, are only ever sent to the provider you chose, and are redacted (`***`) when a tool
reads settings back.

### Edge TTS is an unofficial dependency

Speech synthesis uses the endpoint behind Edge's *Read Aloud*. It needs no API key, which is why it
is free and covers every language the app needs — but it is **not a public Microsoft API**: it can
change or stop working without notice, and it is governed by Microsoft's terms, not this project's.
It has broken once already (the older client's `Sec-MS-GEC` token began returning `403`, fixed by a
newer token version in `edge-tts-rust` 0.1.7). Treat speech synthesis as best-effort: synthesized
audio is cached on disk, so anything already spoken keeps playing offline.

**Planned:** a local TTS engine (Piper-class) as a fallback, which would let Books and Read Aloud
run with the network switched off. Until then, TTS is the one feature that needs a connection.

## Bring your own content

FmS ships **no datasets** — no audio, no subtitles, no transcripts. It is a player and a drill tool:
the material you feed it is content you bring, and its licence is yours to respect, the same way a
video player has no opinion about the film you open.

- **Import what you may use.** Buying an audiobook or a textbook gives you a licence to listen and
  read, not to transcribe, redistribute or publish. Removing DRM is illegal in most jurisdictions
  whatever you do with the file afterwards, so don't feed FmS a DRM-stripped file.
- **Practising is not publishing.** Running STT, building cues and drilling on your own machine is
  personal use. *Upload* and then *Publish* on the Datasets Sync page is redistribution: the dataset
  goes to the website, the upload dialog's visibility decides who can pull it (`private` by default,
  `shared` for subscribed accounts, `public` for everyone), and every audio file, subtitle, cue and
  transcript in it has to be licensed for that. *Withdraw* takes a published version down but keeps
  the stored bytes, so it is not an undo for a licensing mistake — check before you publish.
- **Credit where a licence asks for it.** CC BY and CC BY-SA sources need attribution (and
  share-alike on derivatives); public-domain sources don't, though naming the source costs nothing.

### Sources you can legally import

None of these are bundled with the app — they are simply places whose terms allow you to download
the material and practise with it. Check the licence on the individual item, not just the front
page, and check it for the country you are in.

| Source | What it gives you | Terms |
|---|---|---|
| [LibriVox](https://librivox.org) | Volunteer audiobooks in many languages — German, French, Spanish and English classics included | Public domain in the USA; verify the author's status in your own country |
| [Project Gutenberg](https://www.gutenberg.org) | The text to pair with LibriVox audio, for transcript alignment | Public-domain texts under US law; the Gutenberg header is a trademark licence you can strip |
| [FSI & Peace Corps courses](https://www.fsi-language-courses.org/) ([mirror](https://yojik.eu)) | Full courses: textbook PDFs plus hours of drill audio, dozens of languages including German | US government works, public domain |
| [VOA Learning English](https://learningenglish.voanews.com) | Slow, scripted news with transcripts, graded by level | Public domain with credit to VOA — except AP/Reuters/AFP material, which is not |
| [Wikimedia Commons](https://commons.wikimedia.org) | Spoken Wikipedia articles, pronunciation clips | Per file: CC BY, CC BY-SA or public domain |
| [Lingua Libre](https://lingualibre.org) | Native-speaker recordings of words and phrases | CC BY-SA 4.0 |
| [Tatoeba](https://tatoeba.org) | Sentence pairs in 300+ languages, good for card decks | Sentences CC BY 2.0 FR; each recording carries its speaker's own licence |
| [Internet Archive](https://archive.org) | Old radio, government and public-domain recordings, audiobooks | Mixed — filter to public domain or CC before importing |

## Updates

The app updates itself from **Settings → About**: it reports whether a newer release exists, and
*Download & install* fetches it, verifies the signature and restarts into it. Nothing installs on its
own, and nothing is checked at startup — the check runs when you open that panel.

| Platform | In-app update |
|----------|---------------|
| Windows (NSIS `.exe`) | Yes |
| Linux (AppImage) | Yes — only when run from the AppImage; a `.deb`/`.rpm` install shows a link to the release instead |
| macOS, Android, iOS | No — re-download and reinstall |

Releases up to v0.1.13 shipped an `.msi` as well; that build is gone. **If you installed from an
`.msi`, uninstall that copy once** and install the current `.exe` — the two are separate installs,
and only the NSIS one can update itself.

Installers are signed with a minisign key whose public half lives in `tauri.conf.json`, so a release
whose signature does not verify is refused rather than installed. A bad release is pulled by deleting
`latest.json` from it — clients then report no update and install nothing.

## Apple builds

**Apple Silicon only.** The STT and desktop feature sets link ONNX Runtime through `ort-sys`, whose
prebuilt binaries cover `aarch64-apple-darwin` and `aarch64-apple-ios` but not Intel Macs, so there
is no `x86_64` build to download. macOS 11 (Big Sur) is the floor.

**The macOS bundle is unsigned**, so Gatekeeper blocks the first launch of the downloaded app.
Either right-click → *Open*, or clear the quarantine flag once:

```bash
xattr -dr com.apple.quarantine /Applications/FmS.app
```

**The iOS package needs re-signing.** CI holds no Apple Developer certificate, so the `.ipa` is
archived with signing disabled and published as `FmS_ios_arm64_<tag>-unsigned.ipa`. Re-sign it with
a free Apple ID (Sideloadly, AltStore) or a paid one (Xcode), then sideload.

**The phone builds are clients, not PC replacements.** A phone practises a dataset and syncs it with
a PC. What *builds* one — subtitle and waveform generation, transcript alignment, book splitting —
and the PC-hosted extras (embedded HTTP/MCP server, OCR, screenshot capture, the Goose agent bridge)
are desktop-only.

## Platform notes

- **OCR needs Tesseract on macOS and Linux.** The app runs `tesseract` from `PATH`
  (`brew install tesseract` / `apt install tesseract-ocr`); only the usual Windows install
  locations are probed by the app itself.
- **Screen capture on macOS** asks for *Screen Recording* permission on first use and captures the
  primary monitor — the drag-out-a-region overlay is Windows-only.

## License

FmS is released under the [MIT License](LICENSE). You may use, modify, and redistribute it,
including commercially, as long as the copyright notice and permission notice are kept.

The bundled Playwrite fonts in [`public/fonts`](public/fonts) are covered by their own
[SIL Open Font License](public/fonts/PLAYWRITE-OFL.txt). Downloaded models and any content you
import into a dataset keep whatever license their respective owners give them — FmS ships no
third-party audio, only the tools to practice with your own.
