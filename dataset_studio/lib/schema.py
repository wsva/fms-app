"""
SQLite schema creation matching the fms-app Tauri backend.
"""

SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS listen_media (
    uuid       TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL,
    title      TEXT NOT NULL,
    source     TEXT NOT NULL,
    note       TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS listen_transcript (
    uuid       TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL,
    media_uuid TEXT NOT NULL,
    transcript TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS listen_subtitle (
    uuid       TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL,
    media_uuid TEXT NOT NULL,
    name       TEXT NOT NULL,
    note       TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS listen_subtitle_cue (
    uuid          TEXT PRIMARY KEY,
    subtitle_uuid TEXT NOT NULL,
    order_num     INTEGER NOT NULL,
    start_ms      INTEGER NOT NULL,
    end_ms        INTEGER NOT NULL,
    content       TEXT NOT NULL,
    reference     TEXT
);

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
"""

DEFAULT_USER_ID = "default"


def create_schema(conn) -> None:
    """Create all tables in the given sqlite3 connection."""
    conn.executescript(SCHEMA_SQL)
