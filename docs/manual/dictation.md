# Dictation

Dictation is FmS's core exercise: you listen to a short clip of real audio and type exactly what you hear. The app checks your answer against the transcript, keeps your place, and rewards progress with XP.

## Opening a dataset and a clip

Open **Dictation** from the sidebar. At the top you pick:

- **Dataset** — a Dictation‑type dataset from your locations.
- **Media file** — one recording inside that dataset.
- **Subtitle version** — if a file has more than one transcript, choose which to practice against.

The toolbar's **breadcrumb** shows where you are (dataset → media) and lets you jump back. The toolbar **Refresh** re‑reads the current list, and the overflow menu opens the **locations** manager to add or browse dataset folders.

## The practice screen

Once a media file is loaded you see:

- A **media player** with the audio and its **waveform** — the waveform highlights the current cue's span so you can see where you are in the audio.
- A list of **cues** — the audio is broken into small segments (usually a phrase or sentence each). One cue is active at a time.

## Answering a cue

For the active cue, type what you hear into the answer field, then check it. When your text matches the reference closely enough, the cue is marked complete and you move on. Completing cues earns XP; finishing a whole subtitle or media file adds a bonus.

The cue field switches between a compact and a larger editor as you work (activating a cue brings focus to its input), so you can type short answers or expand to edit longer ones.

## Dictation modes and tools

- **Focus mode** — hides everything but the current cue so you can concentrate on listening and typing.
- **Tip toggle** — shows or hides helpful hints while you dictate.
- **Cue editor** — lets you adjust cues themselves: edit their text and their **start/end times**, useful when the automatic segmentation or timing needs fixing.
- **Play / replay** — play the current cue (or the whole file); click on the waveform or subtitle to jump to a spot.
- **Previous / next** — move between cues; keyboard arrows and shortcuts help you fly through a file (see [Reference](reference.md)).

## Editing the media list

A **media edit mode** toggle reveals the media list's editable fields (source path and note) so you can fix a mis‑linked file or annotate entries.

## Favourites

When a cue is especially worth keeping — a tricky phrase, a favourite line — mark it. Favourited cues are collected into a special **Favourites** dataset, and the audio clips you favourite are kept there so you can revisit them. On the website this is the "Favorite Cue Audio Clips" collection; in the app it behaves the same way.

## Working with cues that aren't right

Auto‑generated subtitles are good but not perfect. Use the cue editor to retime or reword cues, and if a whole file's segmentation is poor you can re‑align cues on the desktop — see [Datasets & the Processing Pipeline](datasets.md) and the Tools pages for the alignment workflow.

## On phones and the website

The dictation experience is available on desktop, Android, and the website. On a phone the layout collapses to fit the small screen (with a "cues only" view you can toggle), and everything you complete syncs back to your Hub if you use multi‑device sync.

## Related

- Preparing the audio and subtitles: [Datasets & the Processing Pipeline](datasets.md).
- Choosing a speech‑to‑text model: [Models](models.md).
- Turning dictation cues into flashcards: [Cards](cards.md).
