# Wiki

The Wiki is FmS's system for **Markdown document collections**. A Wiki dataset is a folder of `.md` files — notes, grammar references, bilingual texts, instructions, anything you want to read, write, search, and keep in sync. Unlike the audio‑based types, a wiki's content *is* the documents themselves, which makes it especially portable between devices.

## Opening a wiki

Open **Wiki** from the sidebar. A **collapsible sidebar** on the left lists your wiki datasets and the documents inside the current one, so you can switch datasets or jump between pages. Use it (and the locations controls) to add wiki folders.

When you open a document, the viewer shows its rendered Markdown plus a **table of contents** derived from the document's headings, so long references are easy to navigate.

## Reading

Rendered pages support headings, lists, links, tables, and code — the usual Markdown set. Internal links connect pages within a wiki, which is how a manual like this one holds together. The table of contents and section navigation update as you move through a document.

## Writing

You can create and edit documents directly:

- **New document** — add a page to the current wiki.
- **Edit** — the built‑in Markdown editor opens with the raw text; changes save back to the file. A live preview mirrors what readers will see. The editor grows to fit your content.
- **Delete** — remove a document you no longer want.

Because editing works on plain Markdown, anything you can write in Markdown you can put in a wiki.

## Search

Each device builds its own full‑text index over a wiki copy, so **search** finds text inside documents quickly and works even on a freshly downloaded wiki — the index is generated locally on first search and is never transferred.

## Wikis and sync

A wiki is ideal for multi‑device use because every page you save is a syncable change:

- Editing a page on a **Follower** queues the change and sends it to the **Hub** the next time you sync. Pages changed on the Hub come back down the same way.
- Within one file the later edit wins, but a change you made locally that hasn't synced yet is never silently overwritten — your edit goes up first.
- You can **read** a Hub's wiki from a follower without downloading it: the Wiki toolbar's **Hub** button switches the browser and search to a read‑only view of the Hub's wikis; **Local** takes you back. **Download** pulls the whole wiki onto this device, which is what makes it editable here and subscribes it to future syncs.

Full details on roles and syncing are in [Using FmS on Multiple Devices](multiple_devices.md).

## This manual is a wiki

The pages you're reading are themselves stored as a wiki dataset (a folder of Markdown files under `docs/manual`). That's a good example of using the Wiki feature for living, shareable documentation.

## Related

- Dataset types at a glance: [Datasets & the Processing Pipeline](datasets.md).
- Syncing documents across devices: [Using FmS on Multiple Devices](multiple_devices.md).
