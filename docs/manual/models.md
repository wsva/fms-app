# Models

Several FmS features rely on AI models that run **on your own machine** — speech‑to‑text for generating subtitles and scoring read‑aloud, a sentence‑segmentation model for splitting books, and optional models for OCR and text‑to‑speech. **Tools → Models** is where you download and manage them.

## Model lifecycle

Every model on this page shows a status badge:

- **Not downloaded** — listed but not yet on your device.
- **Downloading** — a download is in progress, with progress shown.
- **Downloaded** — the files are on disk, idle.
- **Loaded / Running** — the model is resident in memory, ready to use right now.

You load a model when you need it and it can be released afterward to free memory. Your **model directory** (set in Settings) is where the files are stored; see [Workspaces, Accounts & Settings](workspaces_accounts_settings.md).

## Speech‑to‑text (STT) models

STT models turn audio into text. They power:

- **Generating subtitles** for Dictation and Book datasets,
- **Scoring** your Read Aloud recordings.

Models are largely from the Parakeet family and run locally through an ONNX runtime, so transcription works offline. On the Models page you can:

- **Download** a model version.
- **Select / load** which version is active.
- **Set default** so the app uses it automatically.
- **Unload / delete** to reclaim disk and memory.

The **Model unload timeout** in Settings controls how long a model stays loaded after use (immediately, after N minutes, or never). Keep it loaded if you process many files in a row; unload sooner if memory is tight.

## Download sources

Models come from a catalogue of hosts. When downloading a HuggingFace‑catalog model you can choose the source:

- **Hugging Face** — the default.
- **HF Mirror** — a mirror, useful where the main host is slow or blocked.
- **ModelScope** — an alternative host.

Pick whichever reaches the files fastest for your region.

## Text‑split (sentence segmentation) model

Book datasets need the **SaT** ("Segment any Text") model to split a full text into sentences. It appears on the Models page alongside STT models and is loaded on first use. You can download and delete it here just like an STT model. **Note:** text‑split models are managed only on the **desktop** app — the Android thin client doesn't include them.

## OCR and text‑to‑speech models

The same catalogue also carries **OCR** models (used by the OCR tool for recognising text in images) and **TTS** models. If those tools tell you a model is missing, download it from here. See [Tools](tools.md) for OCR.

## Availability on each platform

- **Desktop** — full model management for all types, all downloadable from the hosts above.
- **Android** — can download and run STT models locally to generate subtitles and score recordings on the device.
- **Website** — server‑side; you don't manage models on the site.

## Related

- Using STT to prepare a dataset: [Datasets & the Processing Pipeline](datasets.md).
- The language model behind chat and word generation: [AI Chat, Agents & Voice](ai_chat_agents.md).
