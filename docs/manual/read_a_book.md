# Read a Book

Read a Book works with a **Book** dataset — a longer text (a graded reader, a chapter book, a collection of passages) that FmS has split into **chapters** and **sentences**, optionally with an audio recording for each sentence.

## Opening a book

Open **Read a Book** from the sidebar. You'll see your **Books library** — the list of Book datasets available in your locations. Pick one to open it. The toolbar breadcrumb tracks your depth (Books → the book → the chapter), and **Refresh** re‑reads whichever level you're on. Use the overflow menu to open the **locations** manager and add book folders.

The book's left area can use a **collapsible sidebar** for navigating chapters, so you can jump between sections while reading.

## Reading view

The main **Reading view** shows the current chapter as a sequence of sentences. From the toolbar you can:

- **Navigate chapters** — move to the previous/next chapter, or open the chapter list.
- **Adjust text layout** — switch alignment/justification to suit the text and your screen.
- **Play audio** — listen to a sentence's recording when the book has sentence‑level audio, so reading and listening reinforce each other.

Reading through sentences correctly earns XP, with a bonus for finishing a chapter.

## The sentence drawer

Tapping a sentence opens the **sentence drawer** — a panel for working closely with that sentence. Here you can:

- See and play the sentence's audio.
- Break the sentence into **words** and look them up.
- Add vocabulary you want to remember to your **cards** (linking what you read to spaced repetition — see [Cards](cards.md)).

The drawer is where reading turns into active study rather than passive scrolling.

## Word generation

Book datasets support **word generation**: FmS can automatically build vocabulary entries (cards) from the text, using a language model and settings you control. The **word generation settings** let you choose how words are selected and produced for a book. (On the website, some generation settings are noted as not yet available in the browser; the desktop app is the full‑featured place for this.)

## Understanding mode

Difficult books — philosophy, dense non‑fiction, older prose — reward reading *with* comprehension tools. On a chapter page the toolbar has an **Understanding** toggle. Turning it on splits the reading pane: the text stays on the left, and an **understanding panel** opens on the right, focused on one paragraph. On a phone the same panel slides in from the right as a drawer instead of splitting the screen. Toggle it off again to return to plain reading.

The paragraph the panel follows is the one you tap (or the small **brain** icon on a paragraph). On desktop, scrolling also moves the focus to whichever paragraph is nearest the top, and the focused paragraph gets a subtle ring. A drag handle between the two panes resizes the split; your chosen width is remembered.

The panel has tabs:

- **Explain** — ask the language model to explain the paragraph.
- **Simplify** — a plain‑language rewrite of the same paragraph.
- **Argument** — a structured claim / premise / objection map, for following how an argument is built.
- **Quiz** — comprehension questions: multiple‑choice (pick an answer, then reveal the correct one and why) and open Socratic questions (reveal a model answer and grade yourself). Finishing a quiz awards **XP**, and any question you missed can be **promoted to a card** in a card dataset you pick — closing the loop from reading to spaced repetition (see [Cards](cards.md)).
- **Notes** — write your own note about the paragraph, then **Assess** it to get the model's feedback on your understanding.
- **Chapter** — chapter‑scoped tools: a **Digest** (for argumentative texts this comes back as a premise outline rather than a prose summary) and the book **Glossary**.

The **Glossary** collects recurring concepts and terms (for example *Dasein*, *a priori*) with concise definitions. Use **Extract** to have the model pull terms from the focused paragraph or the whole chapter, or add and edit entries yourself. A definition you have edited by hand is never overwritten by a later extraction.

Two details worth knowing:

- **Listen** in the panel header synthesises the focused paragraph with Edge TTS and plays it, so you can hear the passage as you study it.
- Generated results are marked **outdated** (a wand badge) when you later edit the paragraph's text, because the explanation no longer matches the source. Click **Regenerate** to refresh it. Your own notes are never marked outdated.

Deleting a paragraph (or just its break) removes that paragraph's audio and its generated understanding artifacts along with it.

## Manage and Advanced views

At the book level (not while reading a single chapter) the toolbar offers two extra modes:

- **Manage Chapters** — organise the book's chapters: review, edit, or restructure them.
- **Advanced** — deeper tools for the book, including re‑splitting the text into sentences and aligning cues to audio (the pipeline step that matches each sentence to its place in the recording). If a book's sentence breaks or timings look wrong, this is where you fix them.

## Getting a book ready

A Book dataset is built from a full text (a `book.txt`) plus, optionally, audio. FmS splits it into sentences with its built‑in segmentation model and aligns those to the audio during processing. Details in [Datasets & the Processing Pipeline](datasets.md).

## Related

- Practice what you read by ear: [Dictation](dictation.md).
- Turn new words into flashcards: [Cards](cards.md).
