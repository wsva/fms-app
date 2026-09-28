"""
Stage 4a (Python engine): split book.txt into sentences and cache them.

Uses NLTK's Punkt sentence tokenizer for high-quality sentence boundaries. NLTK
is required; there is no fallback splitter, so the script fails with a clear
message when nltk is missing. The book's language is detected once (using NLTK's
stopwords corpus) and used to load the matching pretrained Punkt model;
unsupported or undetected languages fall back to English. Per-language
abbreviations (see ``EXTRA_ABBREVS``) are merged into the tokenizer so it does
not split a sentence right after tokens like the German ``z.B.``, ``bzw.``,
``ca.`` or ``Dr.`` Paragraph
(``\n\n``) and line (``\n``)
structure is preserved, then each line is split into sentences. Writes one
sentence per line to the output path (book_sentences.txt).

This is the higher-quality alternative to the built-in Rust splitter. It requires
the user to install Python and nltk themselves (e.g. in the ``dataset_studio``
Miniconda env used by fms-app).

Usage:
    python split_book.py <book_path> <output_path>
"""

import re
import sys

try:
    import nltk
    from nltk.corpus import stopwords
    from nltk.data import load as nltk_load
    from nltk.tokenize.punkt import PunktParameters, PunktSentenceTokenizer
except ImportError:
    print(
        "Error: nltk is required but not installed.\n"
        "  Install it in the dataset_studio conda env, e.g.:\n"
        "    conda activate dataset_studio\n"
        "    pip install nltk\n"
        "  Then fetch the Punkt data:  python -m nltk.downloader punkt punkt_tab",
        file=sys.stderr,
    )
    sys.exit(1)

# Ensure the Punkt tokenizer data and the stopwords corpus (used for language
# detection below) are available. Best-effort only: if a download fails (offline,
# proxy, malformed index, ...) we proceed with whatever is already cached locally.
for _pkg in ("punkt", "punkt_tab", "stopwords"):
    try:
        nltk.download(_pkg, quiet=True)
    except Exception:
        pass

# Punkt model names (valid `language` values for sent_tokenize) that also have a
# stopwords list; this is the candidate set used for language detection.
PUNKT_LANGUAGES = (
    "czech",
    "danish",
    "dutch",
    "english",
    "estonian",
    "finnish",
    "french",
    "german",
    "greek",
    "italian",
    "norwegian",
    "polish",
    "portuguese",
    "slovene",
    "spanish",
    "swedish",
    "turkish",
)

# Extra abbreviations per Punkt language, merged into the tokenizer's known
# abbreviations (``abbrev_types``) so a sentence is NOT split right after them.
# Follow NLTK's convention: lowercase, internal periods kept, NO trailing period
# ("z.B." -> "z.b", "bzw." -> "bzw", "ca." -> "ca"). Add more languages here as
# needed; a language with no entry just uses its pretrained model unchanged.
EXTRA_ABBREVS = {
    "german": {
        "z.b", "bzw", "u.a", "u.ä", "d.h", "s.o", "s.u", "v.a", "o.a", "ca",
        "dr", "prof", "hr", "fr", "ggf", "usw", "sog", "bzgl", "inkl", "exkl",
        "mio", "mrd", "tel", "nr", "jg", "abb", "bd", "kap", "vers", "std",
        "min", "sek", "evtl", "n.chr", "v.chr", "etc",
        # Single-letter initials, so the SPACED form ("z. B.", "u. a.", "d. h.",
        # "s. o.", "n. Chr.", ...) isn't split before the following capital:
        # NLTK tokenizes "z. B." as "z." + "B.", so the bare letter must itself be
        # a known abbreviation type. Harmless for real breaks (a German sentence
        # essentially never ends on one of these bare letters).
        "a", "c", "d", "e", "g", "i", "m", "n", "o", "s", "u", "v", "z",
    },
}


def detect_language(text):
    """Return the Punkt model name whose stopword profile best matches *text*.

    NLTK has no built-in language detector, so we use a pure-NLTK heuristic: score
    every candidate language by the fraction of the sample's words that are
    stopwords in it, and pick the highest. Falls back to ``english`` on any error
    (missing corpus, featureless text, ...).
    """
    words = re.findall(r"[^\W\d_]+", text[:20000].lower())
    if not words:
        return "english"
    try:
        available = set(stopwords.fileids())
    except Exception:
        return "english"
    total = len(words)
    best_lang, best_score = "english", 0.0
    for lang in PUNKT_LANGUAGES:
        if lang not in available:
            continue
        try:
            sw = set(stopwords.words(lang))
        except Exception:
            continue
        score = sum(1 for w in words if w in sw) / total
        if score > best_score:
            best_lang, best_score = lang, score
    return best_lang


def _load_pretrained_tokenizer(language):
    """Load the pretrained Punkt model for *language*, tolerating both the modern
    ``punkt_tab`` (NLTK >= 3.8.2) and legacy ``punkt`` data layouts. Returns
    ``None`` when neither is installed, so the caller can fall back gracefully."""
    for resource in (
        f"tokenizers/punkt_tab/{language}.pickle",
        f"tokenizers/punkt/{language}.pickle",
    ):
        try:
            return nltk_load(resource)
        except Exception:
            continue
    return None


def make_tokenizer(language):
    """Build a sentence tokenizer for *language*.

    Prefers the pretrained Punkt model (best boundary quality) and merges our
    extra abbreviations into its ``abbrev_types``. If the model data isn't
    installed (e.g. offline and never downloaded), falls back to an untrained
    tokenizer seeded with the same abbreviations, which still handles the common
    ``<word>. <Capital>`` sentence boundary.
    """
    extra = EXTRA_ABBREVS.get(language, set())
    tokenizer = _load_pretrained_tokenizer(language)
    if tokenizer is None:
        params = PunktParameters()
        params.abbrev_types.update(extra)
        return PunktSentenceTokenizer(params)
    tokenizer._params.abbrev_types.update(extra)
    return tokenizer


def split_sentences(text, tokenizer):
    sentences = []
    for line in text.split("\n"):
        line = line.strip()
        if not line:
            continue
        sentences.extend(tokenizer.tokenize(line))
    return sentences


def main():
    if len(sys.argv) < 3:
        print("Usage: python split_book.py <book_path> <output_path>", file=sys.stderr)
        sys.exit(1)

    book_path = sys.argv[1]
    output_path = sys.argv[2]

    try:
        with open(book_path, "r", encoding="utf-8") as f:
            text = f.read()
    except OSError as e:
        print(f"Error: failed to read {book_path}: {e}", file=sys.stderr)
        sys.exit(1)

    if not text.strip():
        print("Error: book.txt is empty.", file=sys.stderr)
        sys.exit(1)

    # Detect the book's language once, then build a tokenizer (pretrained Punkt
    # model + our extra abbreviations) reused for every line.
    language = detect_language(text)
    tokenizer = make_tokenizer(language)

    # Split by paragraphs first, then lines, then sentences within each line.
    sentences = []
    for para in text.split("\n\n"):
        para = para.strip()
        if not para:
            continue
        for line in para.split("\n"):
            line = line.strip()
            if line:
                sentences.extend(split_sentences(line, tokenizer))

    if not sentences:
        print("Error: no sentences found in book.txt.", file=sys.stderr)
        sys.exit(1)

    with open(output_path, "w", encoding="utf-8") as f:
        for sentence in sentences:
            f.write(sentence + "\n")

    print(f"Split {len(sentences)} sentences from book.txt (Python/NLTK, language={language})")
    print("Written to: book_sentences.txt")


if __name__ == "__main__":
    main()
