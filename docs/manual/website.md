# The FmS Website

The FmS website is the browser home of the product. It has three jobs: introduce FmS and hand out downloads, host your **account** and shared data, and provide a set of app pages you can use directly in the browser — including the online **Dataset Market** where learning material is shared between people.

## The landing page

Visiting the site root shows a public marketing landing page — what FmS is, its features, and download links for the **desktop (Windows/Linux)** and **Android** apps (the latest release is shown with its version). It's available in multiple languages via a language toggle. If you're signed in, the call‑to‑action takes you into the app; otherwise it invites you to sign in.

## Your account

- **Sign in / register / reset password** — standard account flows. The desktop and Android apps log in with these same credentials, so one account covers app and site.
- Your name and identity appear in the app's sidebar and travel with your **XP and progress**.
- The website is also the natural **cloud Hub**: your datasets can be hosted here so any of your devices can sync against it. See [Using FmS on Multiple Devices](multiple_devices.md).

## Using the app in the browser

Once signed in, the site presents the same navigation model as the desktop app — practice modes at the top, and grouped items beneath:

- **Dictation**, **Read a Book**, **Read Aloud**, **Cards**, **Wiki** — the practice modes, working in the browser against the datasets on your account. See the linked app pages for how each mode works: [Dictation](dictation.md), [Read a Book](read_a_book.md), [Read Aloud](read_aloud.md), [Cards](cards.md), [Wiki](wiki.md).
- **Settings** — your site settings.
- **Blog** — write and read learning blogs: your own posts, posts by others, and adding new entries.
- **Tools** — a small set of hosted utilities:
  - **Plan** — set study plans and due reminders.
  - **Learning Stats** — a view of your XP, progress, and practice history.
  - **PDF Reader** — read PDFs in the browser.
  - **Datasets Market** — browse datasets shared by others (see below).
  - **My Datasets** — manage your own datasets' visibility, tags, and deletion.

The mobile website adapts to small screens, and the sidebar collapses into a slide‑in menu on phones.

## The Dataset Market

The **Market** is the community side of FmS — a catalogue of datasets people have chosen to share.

- **Browse** datasets by type (Dictation, Cards, Book, Read Aloud, Wiki) and by tag.
- **Subscribe** to keep an eye on a dataset, and **download** it into your account so the app can pull it onto your devices.
- **Publish your own**: from **My Datasets** you set a dataset's visibility (private, shared, or public). Shared and public datasets appear to others in the Market; private ones stay yours.

Downloading from the Market gets the dataset onto your account; the app then adopts it as a read‑only copy you can practice with. The relationship between the website Market and the app's Datasets Sync Pull tab is explained in [Datasets & the Processing Pipeline](datasets.md).

## Settings on the website

The site's Settings page gathers your account‑level preferences, grouped into sections:

- **General** — basic preferences.
- **Motto** — a personal motto shown around the app.
- **Local service** — connection settings for a local FmS/desktop service.
- **Default Card Tags** — tags applied automatically when creating cards.
- **API Keys** — create and manage keys for programmatic access.
- **API Reference** — documentation for the website's REST API (the `/api/v1` endpoints the app and other tools call for sync, datasets, and the Market).

Some deep‑linking still works (for example opening straight on the API reference).

## App vs. website — what's different

| | Website | Desktop app | Android app |
|---|---------|-------------|-------------|
| Practice modes | Yes | Yes | Yes |
| Account, blog, market | Yes | via sync | via sync |
| Plan, Learning Stats | Yes | — | — |
| Run STT/OCR models locally | No | Yes | STT only |
| Import from disk / process pipeline | No | Yes | No |
| Heavy tools (Workflow, OCR, Edge TTS, Logs) | No | Yes | No |

In short: the **website** is for your account, sharing, and light browser practice; the **desktop app** is where content gets processed and models run; the **Android app** is a practice‑and‑sync companion. Each complements the others.

## Related

- [Getting Started](getting_started.md) and [Datasets & the Processing Pipeline](datasets.md).
- [Using FmS on Multiple Devices](multiple_devices.md) for Hub/sync details.
