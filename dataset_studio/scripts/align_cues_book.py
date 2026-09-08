#!/usr/bin/env python3
"""
Stage 4b: Align subtitle cues with reference text from book.txt.

Reads sentences from book_sentences.txt (cached) or book.txt, then uses dynamic
programming to match each subtitle cue to its corresponding reference sentence.
Writes the matched reference text directly to listen_subtitle_cue.reference.

Usage:
    python scripts/align_cues_book.py <dataset_path>
"""

import argparse
import sqlite3
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lib.dataset import load_info, touch_info
from lib.alignment import Cue, Ref, multi_pass_align
from lib.text import split_sentences


def load_sentences_from_cache(cache_path: Path) -> list[Ref]:
    """Read pre-split sentences from book_sentences.txt cache."""
    sentences = []
    with open(cache_path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                sentences.append(line)
    return [
        Ref(uuid=str(i), order_num=i, content=s)
        for i, s in enumerate(sentences)
    ]


def load_book_sentences(book_path: Path) -> list[Ref]:
    """Read book.txt and split into sentence-level Ref objects."""
    book_text = book_path.read_text(encoding="utf-8")

    paragraphs = [p.strip() for p in book_text.split("\n\n") if p.strip()]
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
    parser = argparse.ArgumentParser(description="Align subtitle cues with book.txt references.")
    parser.add_argument("dataset_path", type=Path, help="Path to the dataset directory")
    args = parser.parse_args()

    dataset_dir = args.dataset_path.resolve()
    if not dataset_dir.exists():
        print(f"Error: dataset not found: {dataset_dir}")
        sys.exit(1)

    info = load_info(dataset_dir)
    db_path = dataset_dir / "data.sqlite3"
    book_path = dataset_dir / "book.txt"
    cache_path = dataset_dir / "book_sentences.txt"

    if not db_path.exists():
        print("Error: data.sqlite3 not found. Run build_database.py first.")
        sys.exit(1)

    if not book_path.exists():
        print("Error: book.txt not found in dataset directory.")
        sys.exit(1)

    # Load reference sentences: use cache if available, otherwise split book.txt
    if cache_path.exists():
        refs = load_sentences_from_cache(cache_path)
        print(f"Loaded {len(refs)} sentences from book_sentences.txt (cache)")
    else:
        refs = load_book_sentences(book_path)
        print(f"Loaded {len(refs)} sentences from book.txt (split on-the-fly)")

    if not refs:
        print("Error: no sentences found.")
        sys.exit(1)

    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row

    # Discover all subtitle UUIDs
    rows = conn.execute(
        "SELECT DISTINCT subtitle_uuid FROM listen_subtitle_cue"
    ).fetchall()
    subtitle_uuids = [r["subtitle_uuid"] for r in rows]

    if not subtitle_uuids:
        print("No subtitles found in database. Generate subtitles first.")
        conn.close()
        sys.exit(1)

    total_matches = 0

    for subtitle_uuid in subtitle_uuids:
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
        print(f"  Subtitle {subtitle_uuid[:8]}... : {len(cues)} cues, {len(matches)} matches ({anchors} anchors, {inferred} inferred)")

    conn.commit()
    conn.close()

    # Update timestamp
    touch_info(dataset_dir)

    print(f"\nDone: {total_matches} total matches across {len(subtitle_uuids)} subtitle(s)")
    print("Reference text written to listen_subtitle_cue.reference")


if __name__ == "__main__":
    main()
