"""
Text normalization and sentence splitting utilities.
"""

import re


def normalize(text: str) -> str:
    """Lowercase, collapse whitespace, strip."""
    text = text.lower()
    text = re.sub(r"\s+", " ", text)
    return text.strip()


# Sentence-ending punctuation (Latin + CJK)
_SENTENCE_ENDS = set(".!?。！？…")


def is_sentence_end(c: str) -> bool:
    return c in _SENTENCE_ENDS


def split_sentences(text: str) -> list[str]:
    """
    Split text into sentences.

    Uses NLTK if available, otherwise falls back to a simple regex splitter.
    NLTK handles abbreviations (Mr., Dr.) and edge cases much better.
    """
    try:
        import nltk
        try:
            nltk.data.find("tokenizers/punkt_tab")
        except LookupError:
            nltk.download("punkt_tab", quiet=True)
        from nltk.tokenize import sent_tokenize
        sentences = sent_tokenize(text)
        return [s.strip() for s in sentences if s.strip()]
    except ImportError:
        return _regex_split_sentences(text)


def _regex_split_sentences(text: str) -> list[str]:
    """Fallback sentence splitter using regex."""
    # Split on sentence-ending punctuation followed by space
    parts = re.split(r'(?<=[.!?。！？…])\s+', text)
    return [p.strip() for p in parts if p.strip()]
