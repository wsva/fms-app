"""
Parse a book.txt file into sentence-level chunks and write them
to the listen_subtitle_reference table in the dataset's SQLite database.

Usage:
    python split_book.py <db_path> <book_path>
"""

import sqlite3
import sys
import uuid
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
        # Split on sentence-ending punctuation followed by space + uppercase
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

    db_path = sys.argv[1]
    book_path = sys.argv[2]

    # Read book text
    with open(book_path, "r", encoding="utf-8") as f:
        text = f.read()

    # Split into sentences
    sentences = split_sentences(text)
    chunk_uuid = uuid.uuid4().hex

    print(f"chunk_uuid:{chunk_uuid}")
    print(f"sentences:{len(sentences)}")

    # Connect to SQLite database
    conn = sqlite3.connect(db_path)
    cur = conn.cursor()

    # Clear any existing rows for this chunk
    cur.execute("DELETE FROM listen_subtitle_reference WHERE chunk_uuid = ?", (chunk_uuid,))

    # Insert sentences
    for order_num, sentence in enumerate(sentences, start=1):
        sentence = sentence.strip()
        if not sentence:
            continue
        row_uuid = uuid.uuid4().hex
        cur.execute(
            "INSERT INTO listen_subtitle_reference (uuid, chunk_uuid, order_num, content) VALUES (?, ?, ?, ?)",
            (row_uuid, chunk_uuid, order_num, sentence),
        )

    conn.commit()
    conn.close()

    print("done")


if __name__ == "__main__":
    main()
