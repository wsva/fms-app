# Dataset Studio — Admin Design Doc

Internal documentation for admin-only operations.

---

## Export from Production

Datasets can be exported from the FMS website by connecting directly to the production database and extracting the relevant rows. This is a separate workflow from the local build pipeline.

### Overview

The production database contains all user-generated data: media entries, subtitles, cues, dictation progress, and references. Exporting a dataset means:

1. Connecting to the production PostgreSQL database
2. Selecting all rows belonging to a specific dataset (by `user_id` or dataset identifier)
3. Packaging the data along with the corresponding media files, subtitles, and waveforms into the standard dataset directory structure
4. Writing an `info.json` to make it compatible with the local fms-app client

### Database Tables

The production schema mirrors the local SQLite schema documented in [DESIGN.md](DESIGN.md), but uses PostgreSQL:

- `listen_media` — source media entries
- `listen_subtitle` — STT-generated subtitles
- `listen_subtitle_cue` — timed cues within subtitles (includes `reference` column for alignment results)
- `listen_dictation` — per-user dictation progress

### Access

Requires production database credentials. Connection details are stored in admin-only configuration, not in this repository.

---

*This file is restricted to project administrators.*
