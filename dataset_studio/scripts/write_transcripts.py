#!/usr/bin/env python3
"""
Stage 4c: Write transcript files into the database.

Reads transcript/*.txt files and inserts them into the listen_transcript table,
linked to the corresponding media entry by filename stem.

Usage:
    python scripts/write_transcripts.py <dataset_path>
"""

import argparse
import sqlite3
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lib.dataset import load_info, touch_info, generate_uuid, now_iso
from lib.schema import DEFAULT_USER_ID


def main():
    parser = argparse.ArgumentParser(description="Import transcript files into database.")
    parser.add_argument("dataset_path", type=Path, help="Path to the dataset directory")
    args = parser.parse_args()

    dataset_dir = args.dataset_path.resolve()
    if not dataset_dir.exists():
        print(f"Error: dataset not found: {dataset_dir}")
        sys.exit(1)

    info = load_info(dataset_dir)
    db_path = dataset_dir / "data.sqlite3"
    transcript_dir = dataset_dir / "transcript"

    if not db_path.exists():
        print("Error: data.sqlite3 not found. Run build_database.py first.")
        sys.exit(1)

    if not transcript_dir.exists():
        print("Error: transcript/ directory not found.")
        sys.exit(1)

    # Find transcript files
    transcript_files = sorted(
        p for p in transcript_dir.iterdir()
        if p.is_file() and p.suffix == ".txt"
    )

    if not transcript_files:
        print("No transcript files found in transcript/")
        sys.exit(1)

    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    timestamp = now_iso()

    written = 0
    skipped = 0

    for tf in transcript_files:
        stem = tf.stem
        text = tf.read_text(encoding="utf-8").strip()

        if not text:
            print(f"  Skip (empty): {tf.name}")
            skipped += 1
            continue

        # Find matching media entry by title (stem)
        media_row = conn.execute(
            "SELECT uuid FROM listen_media WHERE title = ?",
            (stem,),
        ).fetchone()

        if not media_row:
            print(f"  Skip (no matching media): {tf.name}")
            skipped += 1
            continue

        media_uuid = media_row["uuid"]

        # Check if transcript already exists
        existing = conn.execute(
            "SELECT uuid FROM listen_transcript WHERE media_uuid = ?",
            (media_uuid,),
        ).fetchone()

        if existing:
            # Update existing
            conn.execute(
                "UPDATE listen_transcript SET transcript = ?, updated_at = ? WHERE uuid = ?",
                (text, timestamp, existing["uuid"]),
            )
            print(f"  Updated: {tf.name}")
        else:
            # Insert new
            transcript_uuid = generate_uuid()
            conn.execute(
                "INSERT INTO listen_transcript (uuid, user_id, media_uuid, transcript, created_at, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (transcript_uuid, DEFAULT_USER_ID, media_uuid, text, timestamp, timestamp),
            )
            print(f"  Inserted: {tf.name}")

        written += 1

    conn.commit()
    conn.close()

    # Update timestamp
    touch_info(dataset_dir)

    print(f"\nDone: {written} written, {skipped} skipped")


if __name__ == "__main__":
    main()
