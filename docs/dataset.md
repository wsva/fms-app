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
2, Generate waveform files using audiowaveform (https://github.com/bbc/audiowaveform)
3, Create data.sqlite3
4, Split subtitle into lines and write into database
5, Split transcript or book into chunks and write into database, then align cues (split_book.py, align_cue.py)
6, Generate or update info.json
`````

# format of info.json
`````
{
    "name": "test1",
    "uuid": "b76a0a93-20e0-4d1a-bd16-ab6f18197952",
    "description": "a short description of this dataset",
    "parent_uuid": "",
    "version": 1,
    "structure": "dictation-v1",
    "updated": "YYYY-MM-DDTHH:MM:SSZ"
}
`````

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