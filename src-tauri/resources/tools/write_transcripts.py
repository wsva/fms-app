"""
Write transcript files to the listen_transcript table in the dataset's SQLite database.

Matches transcript files (transcript/{stem}.txt) to existing listen_media entries by title.

Usage:
    python write_transcripts.py <db_path> <transcript_dir>
"""

import os
import sqlite3
import sys
import uuid


def main():
    if len(sys.argv) != 3:
        print("Usage: python write_transcripts.py <db_path> <transcript_dir>")
        sys.exit(1)

    db_path = sys.argv[1]
    transcript_dir = sys.argv[2]

    if not os.path.exists(db_path):
        print(f"Database not found: {db_path}")
        sys.exit(1)

    if not os.path.isdir(transcript_dir):
        print(f"Transcript directory not found: {transcript_dir}")
        sys.exit(1)

    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()

    # Get all media entries
    cursor.execute("SELECT uuid, title FROM listen_media")
    media_rows = cursor.fetchall()

    written = 0
    for media_uuid, title in media_rows:
        transcript_path = os.path.join(transcript_dir, f"{title}.txt")
        if os.path.exists(transcript_path):
            with open(transcript_path, "r", encoding="utf-8") as f:
                text = f.read()

            transcript_uuid = uuid.uuid4().hex
            now = "datetime('now')"
            cursor.execute(
                """INSERT OR REPLACE INTO listen_transcript
                   (uuid, user_id, media_uuid, transcript, created_at, updated_at)
                   VALUES (?, 'default', ?, ?, datetime('now'), datetime('now'))""",
                (transcript_uuid, media_uuid, text),
            )
            written += 1

    conn.commit()
    conn.close()
    print(f"Wrote {written} transcript(s) to listen_transcript.")


if __name__ == "__main__":
    main()
