// ---------------------------------------------------------------------------
// Seed workflow definition(s) as raw YAML text.
//
// The Workflow page is YAML-driven: this string is the *source of truth* the
// editor loads, `parseDefinition` turns it into `StepDef[]`, and the graph is
// laid out from that. It mirrors the authoritative `workflow.yaml` schema the
// Rust engine parses (`src-tauri/src/workflow/core`), with one UI-only extra:
// an optional `title` per step (the engine ignores unknown fields). `when`
// keeps the engine's `${steps.x.outputs.y}` form; `parseDefinition` maps the
// known disk-probe guards onto `DatasetFacts` keys for the client projection.
//
// There is no persistence in this cut — editing is session-local, so this seed
// is embedded rather than fetched (static export can only serve `public/`).
// ---------------------------------------------------------------------------

/** The Dataset Dictation pipeline, in the same shape as `workflow.yaml`. */
export const DICTATION_YAML = `# Dictation dataset processing workflow.
# Each step's \`action\` names an MCP tool / Tauri command. Edges come from
# \`depends_on\`. \`title\` is a UI-only short label (the engine ignores it).
name: process_dictation_dataset
version: 1

steps:
  - id: ensure_model
    title: Ensure STT model
    action: model_status
    description: >-
      Check model_status; download + load the preferred STT model if needed.
    params:
      then: ["model_download", "model_load"]
    outputs:
      - model_version
    retry: { attempts: 2, backoff: "30s" }

  - id: create_dataset
    title: Create dataset
    action: dataset_create
    description: Create the empty dictation dataset (or import an existing one).
    params:
      name: "my_dictation"
    outputs:
      - dataset_uuid

  - id: import_media
    title: Import media
    action: dataset_import_media
    description: Copy audio/video files from a source directory into the dataset.
    depends_on:
      - create_dataset
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"
    params:
      source_dir: "C:/path/to/audio"   # EDIT: your media folder
      link: false                      # true = symlink instead of copy
    outputs:
      - media_count

  - id: generate_subtitles
    title: Generate subtitles
    action: dataset_generate_subtitles
    description: STT-transcribe every media file into VTT subtitles (long-running).
    depends_on:
      - ensure_model
      - import_media
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"
    retry: { attempts: 2, backoff: "1m" }

  - id: generate_waveforms
    title: Generate waveforms
    action: dataset_generate_waveforms
    description: Generate waveform JSON via Symphonia peak detection.
    depends_on:
      - import_media
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"

  - id: generate_database
    title: Build cue database
    action: dataset_generate_database
    description: Parse VTT subtitles into listen_media / listen_subtitle / cue tables.
    depends_on:
      - generate_subtitles
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"

  - id: detect_reference
    title: Detect reference
    action: dataset_list_files
    description: List dataset files to see whether book.txt and/or transcript/ exist.
    depends_on:
      - generate_database
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"
    outputs:
      - has_book
      - has_transcript

  - id: align_cues
    title: Align cues (book)
    action: dataset_align_cues
    description: Align cue text against book.txt with multi-pass anchor DP matching.
    depends_on:
      - detect_reference
    when: "\${steps.detect_reference.outputs.has_book}"
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"

  - id: align_cues_transcript
    title: Align cues (transcript)
    action: dataset_align_cues_transcript
    description: Align cue text against per-subtitle files in transcript/ instead.
    depends_on:
      - detect_reference
    when: "\${steps.detect_reference.outputs.has_transcript}"
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"

  - id: adjust_cue_times
    title: Adjust cue times
    action: dataset_adjust_cue_time
    description: Snap cue boundaries to silence detected in the audio.
    depends_on:
      - generate_database
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"
    params:
      mode: "new"
      strategy: "noise_floor"          # or: dual_bound | peak_relative | otsu

  - id: validate
    title: Validate
    action: dataset_get_summary
    description: Summarize counts and spot-check cues per media; ordered, non-empty.
    depends_on:
      - generate_waveforms
      - align_cues
      - align_cues_transcript
      - adjust_cue_times
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"
    on_failure: stop

  - id: mark_ready
    title: Mark ready
    action: dataset_info_update
    description: Record final counts and note the dataset is processed and practice-ready.
    depends_on:
      - validate
    inputs:
      dataset_uuid: "\${steps.create_dataset.outputs.dataset_uuid}"
    params:
      description: "Processed: subtitles + waveforms + cue DB generated and validated."
`;
