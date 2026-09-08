#!/usr/bin/env python3
"""
Stage 3: Build the SQLite database from subtitles, media, and transcripts.

Parses VTT files and creates data.sqlite3 with the full schema.

Usage:
    python scripts/build_database.py <dataset_path>
"""

import argparse
import sqlite3
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lib.dataset import load_info, list_audio_files, touch_info, generate_uuid, now_iso
from lib.schema import create_schema, DEFAULT_USER_ID
from lib.vtt import parse_vtt


def main():
    parser = argparse.ArgumentParser(description="Build SQLite database for a dataset.")
    parser.add_argument("dataset_path", type=Path, help="Path to the dataset directory")
    args = parser.parse_args()

    dataset_dir = args.dataset_path.resolve()
    if not dataset_dir.exists():
        print(f"Error: dataset not found: {dataset_dir}")
        sys.exit(1)

    info = load_info(dataset_dir)
    media_dir = dataset_dir / "media"
    subtitle_dir = dataset_dir / "subtitle"
    transcript_dir = dataset_dir / "transcript"
    db_path = dataset_dir / "data.sqlite3"

    audio_files = list_audio_files(media_dir)
    if not audio_files:
        print("No audio files found in media/")
        sys.exit(1)

    # Remove existing database
    if db_path.exists():
        db_path.unlink()
        print("Removed existing database")

    conn = sqlite3.connect(str(db_path))
    create_schema(conn)

    timestamp = now_iso()
    total = len(audio_files)

    for i, audio_path in enumerate(audio_files, 1):
        stem = audio_path.stem
        media_uuid = generate_uuid()

        print(f"  [{i}/{total}] Processing: {audio_path.name}")

        # Insert listen_media
        conn.execute(
            "INSERT INTO listen_media (uuid, user_id, title, source, created_at, updated_at) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            (media_uuid, DEFAULT_USER_ID, stem, audio_path.name, timestamp, timestamp),
        )

        # Insert transcript if exists
        transcript_path = transcript_dir / f"{stem}.txt"
        if transcript_path.exists():
            transcript_text = transcript_path.read_text(encoding="utf-8")
            transcript_uuid = generate_uuid()
            conn.execute(
                "INSERT INTO listen_transcript (uuid, user_id, media_uuid, transcript, created_at, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (transcript_uuid, DEFAULT_USER_ID, media_uuid, transcript_text, timestamp, timestamp),
            )
            print(f"    + transcript")

        # Parse VTT and insert subtitle + cues
        vtt_path = subtitle_dir / f"{stem}.vtt"
        if vtt_path.exists():
            cues = parse_vtt(vtt_path)
            subtitle_uuid = generate_uuid()

            conn.execute(
                "INSERT INTO listen_subtitle (uuid, user_id, media_uuid, name, created_at, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (subtitle_uuid, DEFAULT_USER_ID, media_uuid, stem, timestamp, timestamp),
            )

            for order, cue in enumerate(cues):
                cue_uuid = generate_uuid()
                conn.execute(
                    "INSERT INTO listen_subtitle_cue "
                    "(uuid, subtitle_uuid, order_num, start_ms, end_ms, content) "
                    "VALUES (?, ?, ?, ?, ?, ?)",
                    (cue_uuid, subtitle_uuid, order, cue.start_ms, cue.end_ms, cue.content),
                )

            print(f"    + subtitle: {len(cues)} cues")
        else:
            print(f"    ! no subtitle VTT found")

    conn.commit()
    conn.close()

    # Update timestamp
    touch_info(dataset_dir)
    print(f"\nDone: database created at {db_path.name}")


if __name__ == "__main__":
    main()
