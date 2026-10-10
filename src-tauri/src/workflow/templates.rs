//! Built-in workflow templates shipped with the app.
//!
//! Each template is a plain `workflow.yaml` definition, baked into the binary
//! at compile time via [`include_str!`] — no runtime resource resolution, no
//! extraction step, identical on desktop and Android. This replaces the former
//! split source of truth between `docs/ai/workflow/dataset_dictation.yaml`
//! (documentation only, never loaded by any code) and the frontend's
//! `seed.ts` (a hand-maintained duplicate that silently drifted from it).
//!
//! The [`super`] module (`workflow/mod.rs`) owns the actual [`core::Definition`]
//! parsing on top of these strings, so the templates themselves stay pure data.

/// The Dataset Dictation pipeline — import a raw folder, generate subtitles /
/// waveforms, align and adjust cue times. See the file for its own stage docs.
pub(crate) const DICTATION_YAML: &str = include_str!("templates/dataset_dictation.yaml");

/// Every built-in template, as `(id, yaml)` pairs. `id` is the stable key the
/// Workflow page and the `workflow_builtin_templates` MCP tool use to pick one;
/// the definition's own `name`/`version` come from parsing the YAML itself.
pub(crate) fn all() -> Vec<(&'static str, &'static str)> {
    vec![("dictation", DICTATION_YAML)]
}
