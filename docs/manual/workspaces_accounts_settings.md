# Workspaces, Accounts & Settings

This page covers who you are when you use FmS (accounts and workspaces) and how to shape the app to your taste (Settings).

## Workspaces

A **workspace** is a separate, self‑contained learning area with its own datasets, folders, sync role, and progress. Think of it as a profile.

- **Why use more than one?** Different learners who share a computer, different languages kept fully apart, or a "personal" vs. "class" area can each live in its own workspace. On a single machine one workspace can be a **Hub** while another is a **Follower**.
- **Open Workspaces** from **Tools → Workspaces** to create, rename, and switch between them.
- Each workspace card shows how it relates to the account you're signed in as — badges like **Same user**, **Other user**, or **Unclaimed** help you see which account owns it.
- A workspace can be marked for **auto‑login**, so FmS returns to it (and its account) on launch.
- **Open folder** (**Desktop** only) reveals that workspace's directory in your file manager — handy for backing it up or clearing space. On Android the folder is app‑private, so the button is not offered there.

The folders FmS reads are **per workspace** (dataset locations, books, recordings), so switching workspaces switches material. FmS keeps those folders under the workspace directory itself; they are not edited from Settings — a dataset type gains another folder through its own page's **Add dataset location** button.

## Accounts & signing in

- The **Login** button at the top of the sidebar signs you into your FmS account. On the desktop and Android the sign‑in happens **inside the app** — you enter your email (or nickname) and password and FmS verifies them against the website's sign‑in service; there's no browser round trip.
- The identities you've signed in with on a device are remembered as quick‑pick chips, so you don't retype them after each logout.
- Signing in connects your **XP and progress** to your account and unlocks Hub downloads, sync, and cross‑device chat.
- **Log out** from the same spot (your avatar in the sidebar).

The same account works on the website, so your identity and hosted data are shared between app and site. You can create an account on the website first, then use it here.

## Settings

Open **Settings** at the bottom of the sidebar. It is organised into sections; picking one shows just that section, and **All** stacks them.

### Theme

Four themes: **Light** (default), **Dark**, **Solarized**, and **Gruvbox**. Your choice is remembered on this device. The theme applies across the whole app.

### LLM Settings

Which language model backend the app talks to. These apply to the app as a whole, not per workspace:

- **Ollama URL** — the address of a local Ollama server, used by the features that call a language model (OCR clean‑up, Books word explanations, read‑aloud normalization). See [AI Chat, Agents & Voice](ai_chat_agents.md).
- **LLM provider, API key, and model** — cloud model settings for those same features, if you don't use a local Ollama. The **Test connection** button next to them reports whether the provider answers and which models it offers.

Speech‑to‑text models live on the [Models](models.md) page, the Hub this device syncs with and its token on [Multiple Devices](multiple_devices.md), and the agent dock on [AI Chat, Agents & Voice](ai_chat_agents.md).

### Microphone

Where FmS records audio (system‑wide voice input and Read Aloud), you can choose which **input device** to use and test it here. Pick your preferred microphone and confirm it captures sound before you rely on it for recording or dictation.

## Page zoom & sidebar

The app supports a global page zoom (remembered per device) if text is too small or too large on your screen. The sidebar itself can be dragged wider/narrower or collapsed to icons — see [Getting Started](getting_started.md).

## Next steps

- Manage your material: [Datasets & the Processing Pipeline](datasets.md).
- Multi‑device setup: [Using FmS on Multiple Devices](multiple_devices.md).
