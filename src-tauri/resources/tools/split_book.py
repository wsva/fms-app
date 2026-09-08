"""
Validate that book.txt exists and can be parsed into sentences.

Usage:
    python split_book.py <db_path> <book_path>
"""

import sys
import re

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
    if len(sys.argv) < 3:
        print("Usage: python split_book.py <db_path> <book_path>")
        sys.exit(1)

    book_path = sys.argv[2]

    # Read book text
    with open(book_path, "r", encoding="utf-8") as f:
        text = f.read()

    if not text.strip():
        print("Error: book.txt is empty.")
        sys.exit(1)

    # Split into sentences for counting
    sentences = split_sentences(text)
    print(f"sentences:{len(sentences)}")
    print("Validation passed.")


if __name__ == "__main__":
    main()
