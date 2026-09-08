"""
Align subtitle cues with reference text from book.txt using dynamic programming.
Reads book.txt directly, writes matched text to listen_subtitle_cue.reference.

Usage:
    python align_cue.py <db_path> <subtitle_uuid>
"""

import re
import sqlite3
import sys
from dataclasses import dataclass
from difflib import SequenceMatcher
from pathlib import Path

try:
    import nltk
    nltk.download("punkt", quiet=True)
    from nltk.tokenize import sent_tokenize
    HAS_NLTK = True
except ImportError:
    HAS_NLTK = False


# ============================================================
# Configuration
# ============================================================

GAP_PENALTY = 30
MIN_MATCH_SCORE = 60
MAX_MERGE_SIZE = 3


# ============================================================
# Models
# ============================================================

@dataclass
class Cue:
    uuid: str
    order_num: int
    content: str


@dataclass
class Ref:
    uuid: str
    order_num: int
    content: str


# ============================================================
# Text utilities
# ============================================================

def normalize(text: str) -> str:
    text = text.lower()
    text = re.sub(r"\s+", " ", text)
    return text.strip()


def similarity_score(cue_text: str, ref_text: str) -> float:
    """0~100"""
    cue_text = normalize(cue_text)
    ref_text = normalize(ref_text)

    if len(cue_text) > 20 and cue_text == ref_text:
        return 100

    if cue_text in ref_text or ref_text in cue_text:
        return 90

    ratio = SequenceMatcher(None, cue_text, ref_text).ratio()
    return ratio * 80


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


# ============================================================
# Candidate generation
# ============================================================

def build_cue_candidates(cues):
    """
    Generate:
        cue[i]
        cue[i]+cue[i+1]
        cue[i]+cue[i+1]+cue[i+2]
    """
    candidates = []
    n = len(cues)
    for i in range(n):
        for length in range(1, MAX_MERGE_SIZE + 1):
            if i + length > n:
                break

            group = cues[i:i + length]

            candidates.append({
                "start": i,
                "end": i + length - 1,
                "uuids": [c.uuid for c in group],
                "content": " ".join(c.content for c in group)
            })

    return candidates


# ============================================================
# Dynamic programming alignment
# ============================================================

def align(cues, refs):
    n = len(cues)
    m = len(refs)

    dp = [[0] * (m + 1) for _ in range(n + 1)]
    back = [[None] * (m + 1) for _ in range(n + 1)]

    for i in range(1, n + 1):
        dp[i][0] = dp[i - 1][0] - GAP_PENALTY
        back[i][0] = ("UP",)

    for j in range(1, m + 1):
        dp[0][j] = dp[0][j - 1] - GAP_PENALTY
        back[0][j] = ("LEFT",)

    for i in range(1, n + 1):
        cue = cues[i - 1]

        for j in range(1, m + 1):
            ref = refs[j - 1]

            match_score = similarity_score(cue.content, ref.content)

            diag = dp[i - 1][j - 1] + match_score
            up = dp[i - 1][j] - GAP_PENALTY
            left = dp[i][j - 1] - GAP_PENALTY

            best = max(diag, up, left)

            dp[i][j] = best

            if best == diag:
                back[i][j] = ("MATCH", match_score)
            elif best == up:
                back[i][j] = ("UP",)
            else:
                back[i][j] = ("LEFT",)

    matches = []

    i = n
    j = m

    while i > 0 and j > 0:
        step = back[i][j]

        if step[0] == "MATCH":
            score = step[1]
            if score >= MIN_MATCH_SCORE:
                matches.append((cues[i - 1], refs[j - 1], score))

            i -= 1
            j -= 1
        elif step[0] == "UP":
            i -= 1
        else:
            j -= 1

    matches.reverse()
    return matches


# ============================================================
# Database
# ============================================================

def load_cues(conn, subtitle_uuid):
    cur = conn.cursor()
    cur.execute("""
        SELECT uuid, order_num, content
        FROM listen_subtitle_cue
        WHERE subtitle_uuid=?
        ORDER BY order_num
    """, (subtitle_uuid,))
    return [Cue(*row) for row in cur.fetchall()]


def load_refs_from_book(book_path):
    """Read book.txt and split into sentence-level Ref objects."""
    text = Path(book_path).read_text(encoding="utf-8")
    sentences = split_sentences(text)
    return [Ref(uuid=str(i), order_num=i, content=s) for i, s in enumerate(sentences)]


def update_cue_references(conn, matches):
    """Write matched reference text to listen_subtitle_cue.reference."""
    cur = conn.cursor()
    for cue, ref, score in matches:
        cur.execute("""
            UPDATE listen_subtitle_cue
            SET reference=?
            WHERE uuid=?
        """, (ref.content, cue.uuid))
    conn.commit()


# ============================================================
# Main
# ============================================================

def process_subtitle(db_path, subtitle_uuid, book_path):
    conn = sqlite3.connect(db_path)
    try:
        cues = load_cues(conn, subtitle_uuid)
        if not cues:
            print(f"matched:0 (no cues found)")
            return

        refs = load_refs_from_book(book_path)
        if not refs:
            print(f"matched:0 (no sentences in book.txt)")
            return

        matches = align(cues, refs)
        update_cue_references(conn, matches)
        print(f"matched:{len(matches)}")
    finally:
        conn.close()


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Usage: python align_cue.py <db_path> <subtitle_uuid>")
        sys.exit(1)

    db_path = sys.argv[1]
    subtitle_uuid = sys.argv[2]

    # Derive book.txt path from db_path (same directory)
    book_path = Path(db_path).parent / "book.txt"
    if not book_path.exists():
        print(f"Error: book.txt not found at {book_path}")
        sys.exit(1)

    process_subtitle(db_path, subtitle_uuid, str(book_path))
