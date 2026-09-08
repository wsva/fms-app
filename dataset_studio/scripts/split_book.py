#!/usr/bin/env python3
"""
Stage 4a: Split book.txt into sentences and write to book_sentences.txt.

Reads book.txt, splits into sentences using NLTK (with regex fallback),
and writes one sentence per line to book_sentences.txt. This serves as
a cache for the alignment step.

Usage:
    python scripts/split_book.py <dataset_path>
"""

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lib.dataset import load_info

try:
    import nltk
    nltk.download("punkt", quiet=True)
    from nltk.tokenize import sent_tokenize
    HAS_NLTK = True
except ImportError:
    HAS_NLTK = False


def split_sentences_fallback(text):
    """Simple sentence splitting without NLTK."""
    sentences = []
    for line in text.split("\n"):
        line = line.strip()
        if not line:
            continue
        parts = re.split(r'(?<=[.!?])\s+(?=[A-Z])', line)
        for part in parts:
            part = part.strip()
            if part:
                sentences.append(part)
    return sentences


def split_sentences(text):
    if HAS_NLTK:
        sentences = []
        for line in text.split("\n"):
            line = line.strip()
            if not line:
                continue
            sentences.extend(sent_tokenize(line))
        return sentences
    else:
        return split_sentences_fallback(text)


def main():
    parser = argparse.ArgumentParser(description="Split book.txt into sentences.")
    parser.add_argument("dataset_path", type=Path, help="Path to the dataset directory")
    args = parser.parse_args()

    dataset_dir = args.dataset_path.resolve()
    if not dataset_dir.exists():
        print(f"Error: dataset not found: {dataset_dir}")
        sys.exit(1)

    info = load_info(dataset_dir)
    book_path = dataset_dir / "book.txt"
    output_path = dataset_dir / "book_sentences.txt"

    if not book_path.exists():
        print("Error: book.txt not found in dataset directory.")
        sys.exit(1)

    # Read and split
    book_text = book_path.read_text(encoding="utf-8")
    if not book_text.strip():
        print("Error: book.txt is empty.")
        sys.exit(1)

    # Split by paragraphs first, then sentences within each
    paragraphs = [p.strip() for p in book_text.split("\n\n") if p.strip()]
    sentences = []
    for para in paragraphs:
        for line in para.split("\n"):
            line = line.strip()
            if line:
                sentences.extend(split_sentences(line))

    if not sentences:
        print("Error: no sentences found in book.txt.")
        sys.exit(1)

    # Write one sentence per line
    with open(output_path, "w", encoding="utf-8") as f:
        for sentence in sentences:
            f.write(sentence + "\n")

    print(f"Split {len(sentences)} sentences from book.txt")
    print(f"Written to: {output_path.name}")


if __name__ == "__main__":
    main()
