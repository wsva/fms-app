# Datasets & the Processing Pipeline

Everything you practice in FmS comes from a **dataset**. This page explains what a dataset is, how FmS finds them, how you prepare ("process") one for practice, and how datasets are shared through the online Market.

## What a dataset is

A dataset is a folder on your computer. The simplest one just holds audio files; FmS then generates the helper files that make practice possible. A fully processed dictation dataset looks like this:

```
<dataset>/
├── info.json          # metadata: name, type, language, description, sharing
├── data.sqlite3       # the generated database (media, subtitles, cues)
├── media/             # your audio/video files (mp3, wav, m4a, ogg, flac, mp4, …)
├── subtitle/          # one .vtt subtitle per media file (generated)
├── waveform/          # one .json waveform per media file (generated)
├── transcript/        # optional plain-text transcripts
└── book.txt           # optional full reference text for alignment
```

The `info.json` file is what tells FmS the folder is a dataset and what kind it is. You normally don't edit it by hand — FmS writes and updates it for you.

## The five dataset types

| Type | Used by | Contains |
|------|---------|----------|
| **Dictation** | Dictation | audio + subtitles + cues |
| **Cards** | Cards | flashcards and tags |
| **Book** | Books | chapters, sentences, and per‑sentence audio |
| **Read Aloud** | Read Aloud | texts, their standard audio, and your recorded attempts |
| **Wiki** | Wiki | Markdown documents |

Whenever datasets are listed — in a practice page's picker, in Datasets Sync, or in the Market — they are grouped into tabs by type, and you work with the tab relevant to what you're doing.

## How FmS finds datasets

FmS scans your workspace's **datasets directory** (set in [Settings](workspaces_accounts_settings.md)) for folders containing an `info.json`. Practice pages also let you manage additional **locations** — extra folders to look in — through a shared locations manager, so your material doesn't all have to live in one place. A dataset's **status** (whether its subtitles, waveform, and database have been generated) is shown wherever the dataset is listed, so you always know what still needs processing.

## Processing a dataset

A dataset that only has audio needs three generated pieces before you can practice. On the desktop app these run per dataset:

1. **Generate subtitles** — a speech‑to‑text (STT) model transcribes each media file into timed cues and writes a `.vtt` subtitle. You need at least one STT model downloaded first — see [Models](models.md).
2. **Generate waveforms** — FmS computes each file's audio waveform for the dictation display. This runs entirely in the app; no external tool is required.
3. **Generate the database** — the subtitles and cues are parsed into the dataset's `data.sqlite3`, which is what the practice pages read.

For **Book** datasets there's an extra step: FmS splits the full text into sentences with the SaT sentence‑segmentation model, then aligns the audio cues to those sentences so reading and listening line up.

## The Workflow page (**Desktop**)

**Tools → Workflow** is a guided, step‑by‑step way to run and track dataset processing. Rather than firing one button at a time, a **workflow** is a small pipeline of steps (for example: import → generate subtitles → generate waveforms → build database → align cues → audit) that you advance step by step.

- Each run is **persistent and resumable** — if you stop, you can pick up where you left off.
- Steps report a **status** (pending, running, done, failed) so you can see the whole picture at a glance.
- An **audit / verify** stage checks the finished dataset (are cues present, are timings sane, does every media file have a subtitle?) and the results can be **adopted** into the run, giving you confidence the dataset is ready.
- Built‑in templates cover the common pipeline, so you rarely define steps yourself.

Use the quick per‑dataset buttons for routine work, and the Workflow page when you want control, visibility, or an auditable result.

## Importing datasets

- **From disk (desktop)** — copy a dataset folder into your datasets directory (or add its location) and it appears in the pickers. If it only has audio, process it as above.
- **From the Market** — download a ready dataset someone shared (see below).
- **From another device** — sync it down from your Hub; see [Multiple Devices](multiple_devices.md).

## The online Market

The **Market** is the catalogue of datasets people have shared on the FmS website. You reach it two ways:

- On the **website**, the Tools → **Datasets Market** page lists everything shared or public, which you can browse by type and tag. See [The FmS Website](website.md).
- In the **app**, the **Datasets Sync** page's Pull tab shows the Market when you're connected to the cloud origin, so you can subscribe and download shared datasets straight onto your device.

Downloading a dataset from the Market installs a read‑only copy under your download folder, ready to practice. **Publishing** your own datasets to share them (making them visible and downloadable by others) is a Hub‑side action, described in [Multiple Devices](multiple_devices.md) and [The FmS Website](website.md).

## Next steps

Practice with a processed dataset in [Dictation](dictation.md), [Books](books.md), [Read Aloud](read_aloud.md), [Cards](cards.md), or [Wiki](wiki.md).
