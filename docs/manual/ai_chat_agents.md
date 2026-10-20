# AI Chat, Agents & Voice

Beyond the practice modes, FmS bundles several AI and voice capabilities. They're grouped here because they share the same underlying language‑model and speech plumbing.

## LLM Chat

**Tools → LLM Chat** is a chat interface to a large language model, useful for asking about grammar, usage, translations, or anything language‑related while you study.

- It talks to a local **Ollama** server by default. Point it at your Ollama URL in Settings, choose a model, and start chatting. You can list, pull, and manage Ollama models from here.
- If you'd rather use a hosted model, set the **LLM provider, API key, and model** in Settings instead.
- The dock/agent (below) can sit alongside your work so you can ask questions without leaving the page you're on.

## The Agent Dock (Goose) — **Desktop**

The **Agent Dock** connects FmS to an AI agent (Goose) over its ACP bridge. Because every part of FmS is exposed through the built‑in MCP server, the agent can do real work for you: import and process datasets, generate subtitles, run card reviews, manage sync — the same actions you'd do by hand.

- Configure the connection in Settings → Global (the **Agent / Goose** section, desktop only).
- Use the dock to give the agent a task and watch it operate on your data.

This is optional — you never need the agent to use FmS — but it's what makes repetitive batch work (processing twenty datasets, cleaning up cues) something you can delegate.

## Edge Text‑to‑Speech

**Tools → Edge TTS** turns written text into spoken audio using Microsoft Edge's online voices — handy for hearing a passage, generating pronunciation examples, or producing audio you can save.

- Choose a **language** and **voice** (English, Deutsch, Français, Español, Italiano, and more).
- Type the text, then **preview** it or **synthesize** and **save** the clip.
- Adjust **rate**, **volume**, and **pitch** to shape the delivery.

## Cross‑Device Chat

**Tools → Cross‑Device Chat** is a private conversation between **your own** devices — send a text or a file from your computer to your phone, for example.

- All messages and attachments live on your **Hub**; followers read them from there rather than keeping separate copies.
- Files are downloaded on demand when you open them, so they don't take space until you need them.
- If the Hub is offline, sends fail and the pending count shows on the Devices & Hub page until it's reachable again.

Details on roles and pairing are in [Using FmS on Multiple Devices](multiple_devices.md).

## Voice input across the app

FmS supports recording audio from your microphone in more than one place — Read Aloud takes, dictation‑related capture, and system‑wide voice input into text fields. Your preferred **microphone device** is chosen and tested in Settings → Microphone (see [Workspaces, Accounts & Voice settings](workspaces_accounts_settings.md)).

## Related

- The speech‑to‑text side of AI: [Models](models.md).
- How chat and generation feed reading vocabulary: [Read a Book](read_a_book.md).
