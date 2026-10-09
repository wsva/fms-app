# The Compelete Structure of a Dataset
`````
<directory_on_file_system>
├── info.json
├── data.sqlite3
├── media
│   ├── audio_1.mp3
│   └── audio_2.mp3
├── transcript (optional)
│   ├── audio_1.txt
│   └── audio_2.txt
├── subtitle
│   ├── audio_1.vtt
│   └── audio_2.vtt
├── waveform
│   ├── audio_1.json
│   └── audio_2.json
├── book.txt (optional)
└── README.md (optional)
`````

# Steps to generate Dataset
`````
1, Generate subtitle in VTT format using STT mode
2, Generate waveform files in-app via Symphonia peak detection (pure Rust; no external tool)
3, Create data.sqlite3
4, Split subtitle into lines and write into database
5, Split book into sentences (built-in Rust, or bundled split_book.py with NLTK) -> book_sentences.txt, then align cues to subtitles (built-in Rust)
6, Generate or update info.json
`````

# format of info.json

One shape describes every dataset type, so this file is no longer dictation-specific. The authoritative contract is [design/dataset-info.md](./design/dataset-info.md), with [design/info.schema.json](./design/info.schema.json) as its machine-readable form: the fields, who reads each one, and the old-to-new mapping all live there and are not repeated below. This is only what such a file looks like now:

`````
{
  "spec": 2,
  "uuid": "b76a0a93-20e0-4d1a-bd16-ab6f18197952",
  "type": "dictation",
  "format": "dictation-v2",
  "name": "test1",
  "description": "a short description of this dataset",
  "language": "de",
  "created_at": "2026-10-01T18:18:18Z",
  "updated_at": "2026-10-01T18:18:18Z",
  "sharing": {
    "visibility": "private",
    "owner_id": "",
    "subscribers": []
  }
}
`````

`version` and `parent_uuid` are dropped because nothing read them, and `structure` split into `type` plus `format`. `updated` is now `updated_at`, and `title` is now `name`.

Two things worth knowing before writing one by hand:

- A file in the old shape does not parse at all. `type`, `format` and `name` are required with no defaults, so a pre-unification `info.json` makes `read_info_opt` return `None` and the folder is skipped by discovery rather than half-read. There is no compatibility reader and no automatic migration.
- Stamps are UTC with second precision and a `Z` suffix. Files written before that rule took hold may carry a `+00:00` stamp with sub-second digits; `touch_info` rewrites `updated_at` in the canonical form on the next mutation but never touches `created_at`, so an old `created_at` is expected to persist. It is still parsed as an instant, so it stays comparable.

# tables in database
`````
/*
 * title: name of media file, can be simply filename
 * source: path of url to media file, used to generate the full url to access it
 */
model listen_media {
  uuid       String   @id @db.VarChar(100)
  user_id    String   @db.VarChar(100)
  title      String
  source     String
  note       String
  created_at DateTime @default(now()) @db.Timestamptz(6)
  updated_at DateTime @default(now()) @db.Timestamptz(6)
}

/*
 * name: model, date, corrected or not
 */
model listen_subtitle {
  uuid       String   @id @db.VarChar(100)
  user_id    String   @db.VarChar(100)
  media_uuid String   @db.VarChar(100)
  name       String
  note       String
  created_at DateTime @default(now()) @db.Timestamptz(6)
  updated_at DateTime @default(now()) @db.Timestamptz(6)
}

/*
 * reference: similiar text in transcript or book
 */
model listen_subtitle_cue {
  uuid          String   @id @db.VarChar(100)
  subtitle_uuid String   @db.VarChar(100)
  order_num     Int
  start_ms      Int
  end_ms        Int
  content       String
  reference     String?
}

/*
 * save the progress of dictation
 */
model listen_dictation {
  uuid          String   @id @db.VarChar(100)
  user_id       String   @db.VarChar(100)
  media_uuid    String   @db.VarChar(100)
  subtitle_uuid String   @db.VarChar(100)
  status        String   @db.VarChar(20)
  completed     String
  created_at    DateTime @default(now()) @db.Timestamptz(6)
  updated_at    DateTime @default(now()) @db.Timestamptz(6)

  @@unique([user_id, media_uuid, subtitle_uuid])
}
`````


CREATE TABLE IF NOT EXISTS listen_dictation (
    uuid          TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    media_uuid    TEXT NOT NULL,
    subtitle_uuid TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT '',
    completed     TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, media_uuid, subtitle_uuid)
);