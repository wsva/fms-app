//! Built-in workflow templates shipped with the app.
//!
//! Each template is a plain `workflow.yaml` definition, baked into the binary
//! at compile time via [`include_str!`] — no runtime resource resolution, no
//! extraction step, identical on desktop and Android. This replaces the former
//! split source of truth between `docs/ai/workflow/dataset_dictation.yaml`
//! (documentation only, never loaded by any code) and the frontend's
//! `seed.ts` (a hand-maintained duplicate that silently drifted from it).
//!
//! Templates are grouped by dataset **category** (`dictation` / `book` /
//! `card`), one directory each under `templates/`, because a given kind of
//! dataset may grow several workflows over time. The Workflow page renders one
//! tab per category; `builtin_templates` reports each template's `category` so
//! the page (and an agent) can group them.
//!
//! The [`super`] module (`workflow/mod.rs`) owns the actual [`core::Definition`]
//! parsing on top of these strings, so the templates themselves stay pure data.

/// The Dataset Dictation pipeline — import a raw folder, generate subtitles /
/// waveforms, align and adjust cue times. The only category wired for in-app
/// execution so far.
pub(crate) const DICTATION_YAML: &str = include_str!("templates/dictation/dataset_dictation.yaml");

/// The Book library pipeline — create a book, author chapters / sentences, attach
/// audio, build vocabulary. View / agent-facing (actions name real `book_*` tools).
pub(crate) const BOOK_YAML: &str = include_str!("templates/book/build_book_library.yaml");

/// The Card deck pipeline — create a deck, author cards / tags, build the search
/// index, sync online. View / agent-facing (actions name real `card_*` tools).
pub(crate) const CARD_YAML: &str = include_str!("templates/card/setup_card_deck.yaml");

/// Every built-in template, as `(category, id, yaml)` triples. `category` is the
/// dataset type the Workflow page tabs on (`dictation` / `book` / `card`); `id`
/// is the stable key that picks one within a category (a category may hold
/// several). The definition's own `name`/`version` come from parsing the YAML.
pub(crate) fn all() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("dictation", "process_dictation_dataset", DICTATION_YAML),
        ("book", "build_book_library", BOOK_YAML),
        ("card", "setup_card_deck", CARD_YAML),
    ]
}
