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
//! `read_aloud` / `card`), one directory each under `templates/`, because a given
//! kind of dataset may grow several workflows over time — `dictation` has two
//! today.
//! The Workflow page renders one tab per category and lists the templates inside
//! it; `builtin_templates` reports each template's `category` and `run_slug` so
//! the page (and an agent) can group them and name their runs identically.
//!
//! The [`super`] module (`workflow/mod.rs`) owns the actual [`core::Definition`]
//! parsing on top of these strings, so the templates themselves stay pure data.

/// The dictation *build* pipeline — import a raw folder, generate subtitles /
/// waveforms, align and adjust cue times. The only category wired for in-app
/// execution so far, and the one a brand-new dataset starts from; a workflow that
/// improves an already-built dataset gets its own file (and `id`) in the same
/// directory.
pub(crate) const DICTATION_YAML: &str = include_str!("templates/dictation/build_dictation_dataset.yaml");

/// The dictation *sync cue times* pipeline — copy timings from the dataset's VTT
/// files into the cue rows it already has, without re-minting anything. The second
/// template in the `dictation` category, and the reason a category's primary run id
/// is not simply "the dictation run": see [`all`]`'s` run slug.
pub(crate) const SYNC_CUE_TIMES_YAML: &str =
    include_str!("templates/dictation/sync_dictation_cue_times.yaml");

/// The Book library pipeline — create a book, author chapters / sentences, attach
/// audio, build vocabulary. View / agent-facing (actions name real `book_*` tools).
pub(crate) const BOOK_YAML: &str = include_str!("templates/book/build_book_library.yaml");
/// The Card dataset pipeline — create a dataset, author cards / tags, build the search
/// index, sync online. View / agent-facing (actions name real `card_*` tools).
pub(crate) const CARD_YAML: &str = include_str!("templates/card/setup_card_dataset.yaml");

/// The read-aloud pipeline — import passages from an extracted book, probe one
/// clip for a human to listen to, then fill in the rest. It is the first
/// read-aloud template and therefore its category's *primary* workflow, so its
/// run id is plain `read_aloud-<uuid>`; every step is gated by
/// `read_aloud_audit`'s checks, which is what lets `workflow_verify` carry it.
pub(crate) const READ_ALOUD_YAML: &str =
    include_str!("templates/read_aloud/build_read_aloud_dataset.yaml");

/// Every built-in template, as `(category, id, run_slug, yaml)` quadruples.
/// `category` is the dataset type the Workflow page tabs on (`dictation` /
/// `book` / `read_aloud` / `card`); `id` is the stable key that picks one within
/// a category (a category may hold several). `run_slug` names the dataset-scoped
/// run this template keeps its progress in: `None` is the category's *primary*
/// workflow, whose run has always been `<category>-<uuid>` and keeps that id so
/// the runs already on disk stay loadable; `Some(slug)` gives
/// `<category>-<slug>-<uuid>`,
/// so two workflows over one dataset are two histories. [`super::run_id_for`]
/// applies the rule, and [`super::builtin_templates`] hands the slug to the page
/// and to agents instead of making each side re-invent it.
pub(crate) fn all() -> Vec<(&'static str, &'static str, Option<&'static str>, &'static str)> {
    vec![
        ("dictation", "build_dictation_dataset", None, DICTATION_YAML),
        ("dictation", "sync_dictation_cue_times", Some("sync_cue_times"), SYNC_CUE_TIMES_YAML),
        ("book", "build_book_library", None, BOOK_YAML),
        ("card", "setup_card_dataset", None, CARD_YAML),
        ("read_aloud", "build_read_aloud_dataset", None, READ_ALOUD_YAML),
    ]
}
