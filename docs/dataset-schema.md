# Dataset Schema Design (v2)

## Overview

This document describes the redesigned dataset schema with efficient subtitle versioning.

### Design Principles

1. **Immutable history**: Data is never modified — only new versions are created
2. **Space-efficient versioning**: Unchanged cues are not duplicated
3. **Multiple tracks**: One media can have multiple subtitle tracks (different STT models, languages, manual corrections)
4. **Full traceability**: Every change is tracked with metadata
5. **Clean queries**: Current version is simple (`version_superseded IS NULL`)

---

## Filesystem Structure

```
<dataset>/
├── info.json              # Dataset metadata (uuid, name, description, version)
├── data.sqlite3           # SQLite database (all structured data)
├── media/                 # Audio/video files (organized by subfolder)
│   ├── A1.1/
│   │   ├── Lektion_6.mp3
│   │   └── Lektion_7.mp3
├── waveform/              # Cached waveform JSON (optional, regenerable)
│   ├── A1.1/
│   │   ├── Lektion_6.json
├── book.txt               # Full text for alignment (optional)
├── book_sentences.txt     # Sentence-split text (optional, generated)
└── transcript/            # Per-media transcripts (optional)
    ├── A1.1/
    │   ├── Lektion_6.txt
```

**Note:** VTT files are no longer stored on disk — all subtitle data lives in the database with full versioning.

---

## Database Schema

### listen_media

Media files (audio/video) in the dataset.

```sql
CREATE TABLE IF NOT EXISTS listen_media (
    uuid        TEXT PRIMARY KEY,
    source      TEXT NOT NULL,           -- relative path: "A1.1/Lektion_6.mp3"
    duration_ms INTEGER,                 -- duration in milliseconds (nullable)
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### listen_subtitle

Subtitle tracks. One media can have multiple tracks (different STT models, languages, manual corrections).

```sql
CREATE TABLE IF NOT EXISTS listen_subtitle (
    uuid        TEXT PRIMARY KEY,
    media_uuid  TEXT NOT NULL,
    name        TEXT NOT NULL,           -- "STT Parakeet v1", "Manual correction", "German"
    track_type  TEXT,                    -- "stt", "manual", "aligned", "adjusted"
    model_uuid  TEXT,                    -- STT model used (if applicable)
    version     INTEGER NOT NULL DEFAULT 1,  -- current version number
    is_active   INTEGER NOT NULL DEFAULT 1,  -- active track for display/dictation
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    note        TEXT,
    FOREIGN KEY (media_uuid) REFERENCES listen_media(uuid)
);

CREATE INDEX IF NOT EXISTS idx_subtitle_media ON listen_subtitle(media_uuid);
CREATE INDEX IF NOT EXISTS idx_subtitle_active ON listen_subtitle(media_uuid, is_active);
```

### listen_subtitle_cue

Cues with efficient versioning using the "effective range" pattern.

- `version_created`: version when this cue was created or last modified
- `version_superseded`: version when this cue was replaced (NULL = current)

```sql
CREATE TABLE IF NOT EXISTS listen_subtitle_cue (
    uuid              TEXT PRIMARY KEY,
    subtitle_uuid     TEXT NOT NULL,
    order_num         INTEGER NOT NULL,
    start_ms          INTEGER NOT NULL,
    end_ms            INTEGER NOT NULL,
    content           TEXT NOT NULL,
    reference         TEXT,                    -- aligned text from book/transcript
    confidence        REAL,                    -- STT confidence score (0.0-1.0)
    version_created   INTEGER NOT NULL,        -- version when cue was created/modified
    version_superseded INTEGER,                -- version when replaced (NULL = current)
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (subtitle_uuid) REFERENCES listen_subtitle(uuid)
);

CREATE INDEX IF NOT EXISTS idx_cue_subtitle ON listen_subtitle_cue(subtitle_uuid);
CREATE INDEX IF NOT EXISTS idx_cue_current ON listen_subtitle_cue(subtitle_uuid, version_superseded);
CREATE INDEX IF NOT EXISTS idx_cue_version ON listen_subtitle_cue(subtitle_uuid, version_created);
```

### listen_subtitle_version

Version metadata — tracks what changed in each version.

```sql
CREATE TABLE IF NOT EXISTS listen_subtitle_version (
    uuid           TEXT PRIMARY KEY,
    subtitle_uuid  TEXT NOT NULL,
    version        INTEGER NOT NULL,
    change_type    TEXT,                       -- "initial", "stt_regenerate", "alignment", "time_adjust", "manual_edit"
    description    TEXT,                       -- human-readable description
    cues_added     INTEGER NOT NULL DEFAULT 0,
    cues_modified  INTEGER NOT NULL DEFAULT 0,
    cues_deleted   INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    created_by     TEXT,                       -- "system", "user", "goose"
    FOREIGN KEY (subtitle_uuid) REFERENCES listen_subtitle(uuid)
);

CREATE INDEX IF NOT EXISTS idx_version_subtitle ON listen_subtitle_version(subtitle_uuid);
CREATE UNIQUE INDEX IF NOT EXISTS idx_version_unique ON listen_subtitle_version(subtitle_uuid, version);
```

### listen_waveform

Cached waveform data for visualization (optional, can be regenerated).

```sql
CREATE TABLE IF NOT EXISTS listen_waveform (
    uuid          TEXT PRIMARY KEY,
    media_uuid    TEXT NOT NULL,
    peaks_data    TEXT,                        -- JSON array of peak values
    sample_rate   INTEGER,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (media_uuid) REFERENCES listen_media(uuid)
);

CREATE INDEX IF NOT EXISTS idx_waveform_media ON listen_waveform(media_uuid);
```

### listen_dictation (App-level DB)

Dictation progress — stored in app-level database, not dataset database.

```sql
CREATE TABLE IF NOT EXISTS listen_dictation (
    uuid            TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    media_uuid      TEXT NOT NULL,
    subtitle_uuid   TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT '',
    completed       TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, media_uuid, subtitle_uuid)
);
```

---

## Query Patterns

### Get current cues (active version)

```sql
SELECT * FROM listen_subtitle_cue 
WHERE subtitle_uuid = ? AND version_superseded IS NULL
ORDER BY order_num
```

### Get cues at a specific version

```sql
SELECT * FROM listen_subtitle_cue 
WHERE subtitle_uuid = ? 
  AND version_created <= ? 
  AND (version_superseded IS NULL OR version_superseded > ?)
ORDER BY order_num
```

### Get version history

```sql
SELECT * FROM listen_subtitle_version 
WHERE subtitle_uuid = ? 
ORDER BY version DESC
```

### Diff between versions

```sql
-- Cues added in version N
SELECT * FROM listen_subtitle_cue 
WHERE subtitle_uuid = ? AND version_created = ?

-- Cues modified in version N (compare with previous)
SELECT 
    c1.order_num,
    c1.content AS new_content,
    c2.content AS old_content,
    c1.start_ms AS new_start,
    c2.start_ms AS old_start,
    c1.end_ms AS new_end,
    c2.end_ms AS old_end
FROM listen_subtitle_cue c1
JOIN listen_subtitle_cue c2 
    ON c2.subtitle_uuid = c1.subtitle_uuid 
    AND c2.order_num = c1.order_num 
    AND c2.version_superseded = ?
WHERE c1.subtitle_uuid = ? AND c1.version_created = ?

-- Cues deleted in version N
SELECT * FROM listen_subtitle_cue 
WHERE subtitle_uuid = ? AND version_superseded = ?
```

### Get active subtitle for a media

```sql
SELECT * FROM listen_subtitle 
WHERE media_uuid = ? AND is_active = 1
```

### Count cues per version

```sql
SELECT 
    version_created,
    COUNT(*) AS cue_count
FROM listen_subtitle_cue
WHERE subtitle_uuid = ? AND version_superseded IS NULL
GROUP BY version_created
```

---

## Versioning Workflow

### Stage 1: Initial STT (version 1)

```
subtitle: version=1, name="STT Parakeet"
cues: [
  {order:1, text:"Hallo", start:0, end:1000, version_created:1, version_superseded:NULL},
  {order:2, text:"Wie gehts", start:1500, end:3000, version_created:1, version_superseded:NULL},
  {order:3, text:"Gut", start:3500, end:4500, version_created:1, version_superseded:NULL}
]
version_log: {version:1, change_type:"initial", description:"Initial STT transcription"}
```

### Stage 2: Re-run STT with better model (version 2)

```
subtitle: version=2
cues: [
  -- unchanged (no new row needed)
  {order:1, text:"Hallo", start:0, end:1000, version_created:1, version_superseded:NULL},
  
  -- modified (old row marked, new row added)
  {order:2, text:"Wie geht es", start:1500, end:3200, version_created:2, version_superseded:NULL},
  {order:2, text:"Wie gehts", start:1500, end:3000, version_created:1, version_superseded:2},
  
  -- unchanged
  {order:3, text:"Gut", start:3500, end:4500, version_created:1, version_superseded:NULL},
  
  -- new cue
  {order:4, text:"Danke", start:5000, end:6000, version_created:2, version_superseded:NULL}
]
version_log: {version:2, change_type:"stt_regenerate", cues_added:1, cues_modified:1, description:"Re-transcribe with Parakeet v2"}
```

### Stage 3: Adjust timestamps (version 3)

```
subtitle: version=3
cues: [
  -- unchanged
  {order:1, text:"Hallo", start:0, end:1000, version_created:1, version_superseded:NULL},
  
  -- adjusted (new timestamps)
  {order:2, text:"Wie geht es", start:1450, end:3250, version_created:3, version_superseded:NULL},
  {order:2, text:"Wie geht es", start:1500, end:3200, version_created:2, version_superseded:3},
  
  -- adjusted
  {order:3, text:"Gut", start:3450, end:4550, version_created:3, version_superseded:NULL},
  {order:3, text:"Gut", start:3500, end:4500, version_created:1, version_superseded:3},
  
  -- unchanged
  {order:4, text:"Danke", start:5000, end:6000, version_created:2, version_superseded:NULL}
]
version_log: {version:3, change_type:"time_adjust", cues_modified:2, description:"Adjust timestamps via silence detection"}
```

---

## API Design

### Version Management

#### Create new version

```rust
/// Create a new version for a subtitle, returning the new version number.
/// 
/// This function:
/// 1. Increments the subtitle's version counter
/// 2. Creates a version log entry
/// 3. Returns the new version number for use in cue updates
pub async fn subtitle_create_version(
    subtitle_uuid: &str,
    change_type: &str,
    description: &str,
    created_by: &str,
) -> Result<i32, String>
```

#### Finalize version

```rust
/// Finalize a version by updating the version log with change statistics.
/// 
/// Call this after all cue modifications are complete.
pub async fn subtitle_finalize_version(
    subtitle_uuid: &str,
    version: i32,
    cues_added: i32,
    cues_modified: i32,
    cues_deleted: i32,
) -> Result<(), String>
```

#### Query version history

```rust
/// Get the version history for a subtitle.
pub async fn subtitle_get_versions(
    subtitle_uuid: &str,
) -> Result<Vec<SubtitleVersion>, String>
```

#### Rollback to version

```rust
/// Rollback a subtitle to a previous version.
/// 
/// This creates a NEW version that restores the cues from the target version.
/// It does NOT delete any history.
pub async fn subtitle_rollback_to_version(
    subtitle_uuid: &str,
    target_version: i32,
) -> Result<i32, String>  // returns new version number
```

### Cue Management

#### Get current cues

```rust
/// Get all current cues for a subtitle (version_superseded IS NULL).
pub async fn subtitle_get_current_cues(
    subtitle_uuid: &str,
) -> Result<Vec<SubtitleCue>, String>
```

#### Get cues at version

```rust
/// Get cues as they existed at a specific version.
pub async fn subtitle_get_cues_at_version(
    subtitle_uuid: &str,
    version: i32,
) -> Result<Vec<SubtitleCue>, String>
```

#### Update cue (creates new version row)

```rust
/// Update a cue, marking the old row as superseded and inserting a new row.
/// 
/// This is the core function for efficient versioning:
/// - Old cue row: set version_superseded = new_version
/// - New cue row: insert with version_created = new_version
pub async fn cue_update(
    cue_uuid: &str,
    new_start_ms: i32,
    new_end_ms: i32,
    new_content: &str,
    new_version: i32,
) -> Result<String, String>  // returns new cue UUID
```

---

## Key Benefits

| Aspect | Benefit |
|--------|---------|
| **Space efficiency** | Only changed cues create new rows |
| **Full history** | Reconstruct any version at any time |
| **Traceability** | Version metadata tracks what changed and why |
| **Multiple tracks** | Compare STT models, languages, manual edits |
| **Clean queries** | Current version: `version_superseded IS NULL` |
| **Rollback** | Easy to revert to previous version |
| **Collaboration** | Track who made changes (user, system, Goose) |

---

## Migration Notes

This is a clean-slate design. Existing datasets should be exported and re-imported in the new format. The old VTT-based workflow is replaced with database-native versioning.
