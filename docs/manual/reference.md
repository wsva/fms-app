# Reference

Quick look‑ups: keyboard shortcuts, a glossary of FmS terms, common troubleshooting, and a note on the app's agent‑friendly (MCP) design.

## Keyboard shortcuts

These apply to the **Dictation** page on desktop (they need a hardware keyboard; the phone offers the same actions as on‑screen buttons). Use `Cmd` in place of `Ctrl` on a Mac.

| Action | Shortcut |
|--------|----------|
| Play / pause the current cue audio | `Ctrl+S` (or type two spaces at the end of the answer) |
| Clear the answer input | `Ctrl+D` |
| Voice input (dictate by speaking) | `Ctrl+C` |
| Previous / next cue | `Ctrl+↑` / `Ctrl+↓` |
| Show / hide the content & reference tips | `Ctrl+←` / `Ctrl+→` (or double‑click the cue) |
| Zoom the page in / out / reset | `Ctrl` `+` / `Ctrl` `-` / `Ctrl+0` |

## Glossary

| Term | Meaning |
|------|---------|
| **Dataset** | A folder of learning material (audio, text, cards, documents) with an `info.json`. The unit everything revolves around. |
| **Dataset type** | One of Dictation, Cards, Book, Read Aloud, Wiki — decides which practice mode uses it. |
| **Workspace** | A self‑contained profile with its own folders, role, and progress. |
| **Hub** | The authoritative copy of your datasets, served to your other devices. |
| **Follower** | A device that downloads datasets from a Hub and syncs its edits back up. |
| **Cluster ID** | A short code identifying one Hub so followers talk to the right machine. |
| **Cue** | A small time‑coded segment of audio (usually a phrase) you dictate or read. |
| **Subtitle** | The full set of cues for one media file. |
| **Waveform** | The visual shape of an audio file, generated for the dictation display. |
| **STT** | Speech‑to‑text — the model that turns audio into text. |
| **Location** | An extra folder (besides your datasets directory) FmS scans for datasets. |
| **Market** | The catalogue of datasets shared on the website. |
| **XP / Level** | Permanent practice points and the level they earn you. |
| **MCP** | The built‑in server that lets AI agents drive FmS. |

## Troubleshooting

- **A dataset doesn't appear in the picker.** Check that its folder is inside your datasets directory (or an added location) and has a valid `info.json`. Use the toolbar **Refresh** to re‑scan.
- **"Generate subtitles" fails or says no model.** Download an STT model first — [Models](models.md).
- **Processing is slow or memory‑heavy.** Lower the **Model unload timeout** in Settings so models free memory sooner.
- **Read Aloud records nothing.** Select and test your microphone in Settings → Microphone.
- **Sync problems (Hub unreachable, not paired, resync due).** See the troubleshooting section of [Using FmS on Multiple Devices](multiple_devices.md).
- **Something else misbehaves.** Open **Tools → Logs** and look for the error — [Tools](tools.md).
- **Text too small or too large.** Use page zoom (`Ctrl` `+` / `-`, `Ctrl+0` to reset) or your OS display scaling.

## Agent‑friendly design (MCP)

FmS is built so an AI agent can do anything you can do in the interface. Every feature — dataset discovery and processing, dictation, cards, wiki editing, models, sync — is exposed through a built‑in **MCP server** with structured, JSON responses.

What this means for you:

- You can point an agent (like the Goose **Agent Dock**, or any MCP client) at FmS and ask it to batch‑process datasets, clean up cues, or run sync rounds.
- The design follows a few rules that also make the app safer for you: destructive actions back things up rather than asking for a confirmation flag, tools return structured results, and status/discovery tools let an agent check state before acting.

You never have to touch this to enjoy FmS — but it's why repetitive work can be handed off. See [AI Chat, Agents & Voice](ai_chat_agents.md).

## Back to the manual

Full topic list on the [contents page](index.md).
