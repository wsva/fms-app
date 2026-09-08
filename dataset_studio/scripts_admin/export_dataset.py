"""
Export dataset from PostgreSQL to SQLite3 format for the Tauri desktop app.
"""

import os
import re
import sqlite3
import psycopg
import json
from datetime import datetime, timezone
from pathlib import Path
from dotenv import load_dotenv

# Load credentials from .env
load_dotenv(Path(__file__).resolve().parent / ".env")

DB_CONFIG = {
    "dbname": os.environ["PROD_DB_NAME"],
    "user": os.environ["PROD_DB_USER"],
    "password": os.environ["PROD_DB_PASSWORD"],
    "host": os.environ["PROD_DB_HOST"],
    "port": os.environ["PROD_DB_PORT"],
}

DATASET_UUID = "67f077edcf584df88eea53696cde60f3"

# ── SQLite schema (must match src-tauri/src/dataset.rs) ─────────────────────

SQLITE_SCHEMA = """
CREATE TABLE IF NOT EXISTS listen_media (
    uuid       TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL,
    title      TEXT NOT NULL,
    source     TEXT NOT NULL,
    note       TEXT NOT NULL DEFAULT '',
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
"""


def sanitize_filename(name: str) -> str:
    """Remove or replace characters that are invalid in file names."""
    name = re.sub(r'[<>:"/\\|?*]', "_", name)
    name = name.strip()
    return name[:200] if name else "untitled"


def ts_to_str(dt) -> str:
    """Convert a PostgreSQL datetime to an ISO-ish string for SQLite."""
    if dt is None:
        return "datetime('now')"
    return dt.isoformat()


if __name__ == "__main__":
    print("Connecting to PostgreSQL")
    pg_conn = psycopg.connect(**DB_CONFIG)
    pg_cur = pg_conn.cursor()

    pg_cur.execute(
        "SELECT uuid, user_id, tag, description, parent_uuid "
        "FROM dataset_tag WHERE uuid = %s",
        (DATASET_UUID,),
    )
    dataset_rows = pg_cur.fetchall()
    if len(dataset_rows) < 1:
        print("  no dataset found")
        os._exit(1)

    dataset_uuid = str(dataset_rows[0][0])
    user_id = str(dataset_rows[0][1])
    dataset_name = str(dataset_rows[0][2])
    dataset_description = str(dataset_rows[0][3])
    dataset_parent_uuid = str(dataset_rows[0][4])
    print(f"  dataset_name:   {dataset_name}")

    # ── Prepare output directory ─────────────────────────────────────────────

    output_dir = os.path.join("dataset_exports", dataset_name.lower())
    os.makedirs(output_dir, exist_ok=True)
    media_dir = os.path.join(output_dir, "media")
    transcript_dir = os.path.join(output_dir, "transcript")
    os.makedirs(media_dir, exist_ok=True)
    os.makedirs(transcript_dir, exist_ok=True)

    db_path = os.path.join(output_dir, "data.sqlite3")
    if os.path.exists(db_path):
        os.remove(db_path)

    sqlite_conn = sqlite3.connect(db_path)
    sqlite_cur = sqlite_conn.cursor()
    sqlite_cur.executescript(SQLITE_SCHEMA)
    sqlite_conn.commit()
    print(f"Created SQLite database: {db_path}")

    # ── 1. Export listen_media ───────────────────────────────────────────────

    pg_cur.execute(
        "SELECT lm.uuid, lm.user_id, lm.title, lm.source, lm.note, lm.created_at, lm.updated_at "
        "FROM listen_media lm, listen_media_tag lmt "
        "WHERE lm.uuid = lmt.media_uuid and lmt.tag_uuid = %s ORDER BY title",
        (dataset_uuid,),
    )
    media_rows = pg_cur.fetchall()
    media_title_map = {row[0]: row[2] for row in media_rows}
    for row in media_rows:
        media_uuid, uid, title, source, note, created_at, updated_at = row
        # get the file name from a path
        file_name = Path(source).name
        print(f"  media: {file_name}")
        sqlite_cur.execute(
            "INSERT INTO listen_media (uuid, user_id, title, source, note, created_at, updated_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                media_uuid,
                uid,
                title,
                file_name,
                note or "",
                ts_to_str(created_at),
                ts_to_str(updated_at),
            ),
        )

        # export subtitle metadata
        pg_cur.execute(
            "SELECT uuid, user_id, media_uuid, language, subtitle, format, created_at, updated_at "
            "FROM listen_subtitle WHERE media_uuid = %s ORDER BY created_at",
            (media_uuid,),
        )
        subtitle_rows = pg_cur.fetchall()
        for row in subtitle_rows:
            (
                subtitle_uuid,
                uid,
                media_uuid,
                language,
                subtitle_text,
                fmt,
                created_at,
                updated_at,
            ) = row
            # Map PG columns → SQLite columns
            name = f"default ({uid})"
            note = ""
            sqlite_cur.execute(
                "INSERT INTO listen_subtitle (uuid, user_id, media_uuid, name, note, created_at, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?, ?)",
                (
                    subtitle_uuid,
                    uid,
                    media_uuid,
                    name,
                    note,
                    ts_to_str(created_at),
                    ts_to_str(updated_at),
                ),
            )

            # export cues
            pg_cur.execute(
                "SELECT uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference "
                "FROM listen_subtitle_cue WHERE subtitle_uuid = %s ORDER BY order_num",
                (subtitle_uuid,),
            )
            cue_rows = pg_cur.fetchall()
            for row in cue_rows:
                uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference = (
                    row
                )
                sqlite_cur.execute(
                    "INSERT INTO listen_subtitle_cue (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?)",
                    (
                        uuid,
                        subtitle_uuid,
                        order_num,
                        start_ms,
                        end_ms,
                        content,
                        reference,
                    ),
                )
            print(f"  listen_subtitle_cue:   {len(cue_rows)} rows")
        print(f"  subtitle_rows:   {len(subtitle_rows)} rows")

        # export transcripts
        pg_cur.execute(
            "SELECT uuid, user_id, media_uuid, transcript, created_at, updated_at "
            "FROM listen_transcript WHERE media_uuid = %s ORDER BY created_at",
            (media_uuid,),
        )
        transcript_rows = pg_cur.fetchall()
        for row in transcript_rows:
            uuid, uid, media_uuid, transcript_text, created_at, updated_at = row
            # Write transcript text to file
            if transcript_text:
                media_title = media_title_map.get(media_uuid, media_uuid)
                safe_name = sanitize_filename(media_title)
                txt_path = os.path.join(transcript_dir, f"{safe_name}.txt")
                with open(txt_path, "w", encoding="utf-8") as f:
                    f.write(transcript_text)
        print(f"  listen_transcript:     {len(transcript_rows)} rows")

    sqlite_conn.commit()
    print(f"  listen_media:          {len(media_rows)} rows")

    # ── Write info.json ──────────────────────────────────────────────────────

    info = {
        "name": dataset_name,
        "uuid": dataset_uuid,
        "description": dataset_description,
        "parent_uuid": dataset_parent_uuid if dataset_parent_uuid != "None" else "",
        "version": 1,
        "structure": "dictation-v1",
        "updated": datetime.now(timezone.utc).isoformat(),
    }
    info_path = os.path.join(output_dir, "info.json")
    with open(info_path, "w", encoding="utf-8") as f:
        json.dump(info, f, indent=2, ensure_ascii=False)
    print(f"  info.json written")

    # ── Cleanup ──────────────────────────────────────────────────────────────

    pg_cur.close()
    pg_conn.close()
    sqlite_conn.close()

    print(f"\nDone! Exported to: {os.path.abspath(output_dir)}")
