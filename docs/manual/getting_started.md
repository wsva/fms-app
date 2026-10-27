# Getting Started

This page takes you from a fresh install to your first practice session.

## Install

**Desktop (Windows / Linux)** — download the installer for your platform from the FmS website or its GitHub releases page and run it. Launching FmS opens the main window with a sidebar on the left.

**Android** — install the FmS APK. The Android app is a lighter "thin client": it is best used to practice and to sync from a computer, and pairs with a desktop **Hub** (see [Multiple Devices](multiple_devices.md)).

**Website** — open the FmS site in a browser. The landing page introduces the product; sign in to reach the app pages. See [The FmS Website](website.md).

## The main window

The desktop app is a single window with a **sidebar** that holds every feature:

- **Practice modes at the top** — Dictation, Books, Read Aloud, Cards, Wiki.
- **Tools** (a collapsible group) — Workspaces, Workflow, Devices & Hub, Datasets Sync, Cross‑Device Chat, Simple Words, Edge TTS, LLM Chat, OCR, Split Text, PDF Reader, Models, and Logs.
- **Settings** at the bottom.
- Your **account and XP** at the very top: a Login button when you're signed out, and your avatar, level, and XP once signed in.

You can drag the sidebar's right edge to resize it, or double‑click an empty part of the sidebar to collapse it to icons only. On phones the sidebar becomes a bottom navigation bar with the most common modes.

## Point FmS at your folders

Before importing anything, tell FmS where your material lives. Open **Settings**:

- **Datasets directory** — the folder that holds your dataset folders. FmS discovers datasets here.
- **Books directory** and **Recordings directory** — used by Books and Read Aloud respectively.
- **Model directory** — where speech‑to‑text and other model files are downloaded (see [Models](models.md)).

These live in your **workspace**, so each workspace can keep its own folders. More on workspaces in [Workspaces, Accounts & Settings](workspaces_accounts_settings.md).

## Sign in (optional but recommended)

Click the **Login** button at the top of the sidebar and enter your account credentials. Signing in:

- links your **XP and progress** to your account,
- lets the app act as a **Follower** of an online **Hub** so you can download shared datasets,
- enables the **Cross‑Device Chat** and multi‑device sync features.

You can practice fully offline without signing in; you just won't get the hosted/synced benefits. The website account and the app share the same credentials.

## Add your first dataset

You have three options:

1. **Import from disk (desktop)** — place a dataset folder inside your datasets directory, then open it from the practice page's dataset picker. FmS reads each folder's `info.json` to recognise it.
2. **Download from the Market** — browse datasets shared by others and pull a copy onto this device. Covered in [Datasets](datasets.md) and [Website](website.md).
3. **Create from your own audio/text** — drop audio files into a dataset folder and let FmS generate the subtitles, waveforms, and database for you.

## Process the dataset (desktop)

A freshly imported dataset with only audio needs to be prepared before you can practice:

1. **Generate subtitles** — a speech‑to‑text model transcribes each audio file into timed cues.
2. **Generate waveforms** — FmS computes the audio waveform for the dictation display.
3. **Generate the database** — subtitles and cues are written into the dataset's `data.sqlite3`.

You can run these from the dataset's controls on the practice page, or drive them step‑by‑step (and audit the result) from **Tools → Workflow**. Details in [Datasets & the Processing Pipeline](datasets.md).

## Start practicing

Open **Dictation**, pick your dataset at the top, choose a media file, and start typing what you hear. Each cue you complete correctly earns XP. Explore the other modes — [Books](books.md), [Read Aloud](read_aloud.md), [Cards](cards.md) — whenever you like.

## Next steps

- Understand and manage datasets: [Datasets & the Processing Pipeline](datasets.md).
- Set up your phone and computer together: [Using FmS on Multiple Devices](multiple_devices.md).
- Tune the app (theme, zoom, models, sync): [Workspaces, Accounts & Settings](workspaces_accounts_settings.md).
