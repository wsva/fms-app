# AI Chat, Agents & Voice

Beyond the practice modes, FmS bundles several AI and voice capabilities. They're grouped here because they share the same underlying language‑model and speech plumbing.

## LLM Chat

**Tools → LLM Chat** is a chat interface to a large language model, useful for asking about grammar, usage, translations, or anything language‑related while you study.

- It talks to a local **Ollama** server by default. Point it at your Ollama URL in Settings, choose a model, and start chatting. You can list, pull, and manage Ollama models from here.
- If you'd rather use a hosted model, set the **LLM provider, API key, and model** in Settings instead.
- The dock/agent (below) can sit alongside your work so you can ask questions without leaving the page you're on.

## The Agent Dock and the Mailbox — **Desktop**

FmS does not start an agent; one you already use attaches to the app's built‑in MCP server, and the Dock shows the mailbox it works from. Because every part of FmS is exposed there, the agent can do real work for you: import and process datasets, generate subtitles, run card reviews, manage sync — the same actions you'd do by hand.

- **Tools → Agent** is where the connection is set up, and where you decide how much an attached agent may do.
- The Dock (bot button, or `Ctrl`+`Space`) carries the **Mailbox**: leave a request in FmS, and an attached agent claims it and files the result back where you asked for it.

Setting it up — including the one prompt that makes an agent watch the mailbox for you — is [Attaching an Agent](agents.md).

This is optional — you never need the agent to use FmS — but it's what makes repetitive batch work (processing twenty datasets, cleaning up cues) something you can delegate.

### Example: build a Read Aloud dataset from an ebook

The agent reaches the same tools you do, so a prompt says *what* to make and *how much* of it and leaves the steps to it. This one ran end to end on a 2 MB EPUB — 75 passages, one probe clip, and nothing generated in bulk until the user answered:

```
Use the FmS MCP at http://127.0.0.1:35711/mcp — read /agent.md first, it has a build-read-aloud-from-ebook recipe. Create a read_aloud dataset named kaenguru_offenbarung, language de, from C:\Users\yanan\data\book\Die_Känguru_Offenbarung.epub. Split by the book's TOC (one chapter per text); where a chapter runs longer than a page, split it at the EPUB's own page breaks. Skip TOC entries that are only a heading or an image. Texts only for now — one voice-probe clip, then ask before generating the rest.
```

- **The endpoint and `/agent.md` first.** The manual is where the recipes live, so naming it saves the agent from discovering a couple of hundred tools one at a time — and you from writing the steps yourself. `35711` is the port a desktop starts its service on, and the manual it serves names the port that actually got bound, so nothing downstream has to be guessed. An agent that cannot register that many tools up front will fetch the manual and call the endpoint over HTTP instead, which works, and naming the endpoint is what makes it work.
- **"Split by the TOC … at the EPUB's own page breaks"** is the part only the agent can do. FmS has no EPUB reader, and its own splitter only breaks long *plain prose* into passages, so how an ebook becomes text is the agent's judgement — here, 11 chapters with one text each, the long ones cut at the book's page markers so no passage begins mid‑sentence.
- **"Skip TOC entries that are only a heading or an image"** heads off the usual ebook failure: a title page or a photo gallery becomes a passage with no prose in it, and the dataset then reports it as an empty text.
- **"Texts only for now — one voice‑probe clip, then ask"** puts the expensive half behind your approval. The clips are the long part (hours of speech), so the agent makes one and stops: you listen, and if the voice is wrong you say so before the rest exists — changing a preference never re‑voices a clip that already does.

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

FmS supports recording audio from your microphone in more than one place — Read Aloud attempts, dictation‑related capture, and system‑wide voice input into text fields. Your preferred **microphone device** is chosen and tested in Settings → Microphone (see [Workspaces, Accounts & Voice settings](workspaces_accounts_settings.md)).

## Related

- The speech‑to‑text side of AI: [Models](models.md).
- How chat and generation feed reading vocabulary: [Books](books.md).
