#!/usr/bin/env python3
"""
Stage 4b: Align subtitle cues with reference text from transcripts.

For each subtitle, finds its corresponding transcript file (transcript/{stem}.txt),
splits into sentences, and uses dynamic programming to match each cue to its
corresponding reference sentence. Writes the matched reference text directly
to listen_subtitle_cue.reference.

Usage:
    python scripts/align_cues_transcript.py <dataset_path>
"""

import argparse
import sqlite3
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lib.dataset import load_info, touch_info
from lib.alignment import Cue, Ref, multi_pass_align
from lib.text import split_sentences


def load_transcript_sentences(transcript_path: Path) -> list[Ref]:
    """Read a transcript file and split into sentence-level Ref objects."""
    text = transcript_path.read_text(encoding="utf-8")

    paragraphs = [p.strip() for p in text.split("\n\n") if p.strip()]
    sentences = []
    for para in paragraphs:
        for line in para.split("\n"):
            line = line.strip()
            if line:
                sentences.extend(split_sentences(line))

    return [
        Ref(uuid=str(i), order_num=i, content=s)
        for i, s in enumerate(sentences)
    ]


def main():
    parser = argparse.ArgumentParser(description="Align subtitle cues with transcript references.")
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

    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row

    # Get all subtitles with their media info
    rows = conn.execute("""
        SELECT s.uuid AS subtitle_uuid, s.media_uuid, m.title
        FROM listen_subtitle s
        JOIN listen_media m ON s.media_uuid = m.uuid
    """).fetchall()

    if not rows:
        print("No subtitles found in database. Generate subtitles first.")
        conn.close()
        sys.exit(1)

    total_matches = 0
    skipped = 0

    for row in rows:
        subtitle_uuid = row["subtitle_uuid"]
        media_uuid = row["media_uuid"]
        media_title = row["title"]

        # Find corresponding transcript file
        transcript_path = transcript_dir / f"{media_title}.txt"
        if not transcript_path.exists():
            print(f"  Subtitle {subtitle_uuid[:8]}... : no transcript found for '{media_title}'")
            skipped += 1
            continue

        # Load transcript sentences
        refs = load_transcript_sentences(transcript_path)
        if not refs:
            print(f"  Subtitle {subtitle_uuid[:8]}... : empty transcript '{transcript_path.name}'")
            skipped += 1
            continue

        # Load cues
        cue_rows = conn.execute(
            "SELECT uuid, order_num, content FROM listen_subtitle_cue "
            "WHERE subtitle_uuid = ? ORDER BY order_num",
            (subtitle_uuid,),
        ).fetchall()
        cues = [Cue(uuid=r["uuid"], order_num=r["order_num"], content=r["content"]) for r in cue_rows]

        if not cues:
            continue

        # Run multi-pass anchor-based alignment
        matches = multi_pass_align(cues, refs)

        # Write matched reference text directly to listen_subtitle_cue.reference
        anchors = sum(1 for m in matches if m.match_type == "anchor")
        inferred = sum(1 for m in matches if m.match_type == "inferred")
        for m in matches:
            conn.execute(
                "UPDATE listen_subtitle_cue SET reference = ? WHERE uuid = ?",
                (m.ref.content, m.cue.uuid),
            )

        total_matches += len(matches)
        print(f"  Subtitle {subtitle_uuid[:8]}... : {len(cues)} cues, {len(refs)} refs, {len(matches)} matches ({anchors} anchors, {inferred} inferred)")

    conn.commit()
    conn.close()

    # Update timestamp
    touch_info(dataset_dir)

    print(f"\nDone: {total_matches} total matches across {len(rows) - skipped} subtitle(s)")
    if skipped:
        print(f"  Skipped {skipped} subtitle(s) without matching transcripts")
    print("Reference text written to listen_subtitle_cue.reference")


if __name__ == "__main__":
    main()
