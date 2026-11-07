# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""
fms-app STT catalog generator.

Produces `src-tauri/src/models/catalog_stt.json`, which `catalog_stt.rs` parses
and `include_str!`s, so the app ships a complete model list with zero network
access and nobody hand-types a download URL, a hash, or a score again.

Three sources, in the same split Handy uses (see its `scripts/gen_catalog.py`):

  1. Handy's generated `catalog.json`  -> artifact facts for GGUF rows:
     revision-pinned filenames, sizes, sha256, and the `transcribe_cpp`
     capability block read off the HF model cards. Handy's file is itself
     generated from those cards plus a GGUF header probe, so consuming it keeps
     the cards as the single upstream truth instead of a second copy of them.
  2. the CDN itself -> `Content-Length` for every legacy URL, so `size_bytes` is
     measured rather than typed (`--no-head` skips this and uses the table).
  3. the tables at the bottom of this file -> *our* editorial layer: which models
     exist, their ids, names, descriptions, languages, recommended set and rank.
     Nothing here is derivable; everything above is.

Scores are never typed. Handy stores them already derived from measured rtf/WER
with one constant each (speed = 100*(1-e^(-rtf/8)), accuracy = 100*e^(-wer/15));
this script only rescales 0-100 into the 0..1 our UI bars use, and records the
provenance in `scores_from`. A row with no measured sibling keeps `null` scores
rather than an invented number.

Run (repo root, no dependencies needed):
    python scripts/gen_catalog.py
    python scripts/gen_catalog.py --handy-catalog ../Handy/src-tauri/src/catalog/catalog.json
"""

import argparse
import datetime
import json
import pathlib
import re
import sys
import urllib.request

CATALOG_VERSION = 1
MIRRORS = ["https://blob.handy.computer"]

DEFAULT_HANDY = "learning/Handy/src-tauri/src/catalog/catalog.json"
DEFAULT_OUT = "src-tauri/src/models/catalog_stt.json"

# ─────────────────────────────────────────────────────────────────────────────
# 3. Curation — ours. slug/id -> editorial UI copy + recommended placement.
#
# `ONNX` / `BIN` are the artifacts already shipped by the hand-written Rust
# table this script replaces; their `url`s are the ones users have already
# downloaded, so they are load-bearing identifiers and must not change. The
# `id`s are load-bearing for the same reason: they are the directory name on
# disk, the key in `index.json` and the value persisted by `model_set_default`,
# so a renamed id orphans somebody's downloaded model and silently drops their
# default. A test in catalog_stt.rs pins the legacy id set.
#
# `GGUF_PILOTS` is the Phase 2 addition: two desktop-only rows beside the
# ONNX/.bin rows they imitate, unranked and not recommended until the probe and
# a real comparison say otherwise.
#
# Every row carries `base_model` — the identity two formats of the same weights
# share, so the UI can present one model with two runtimes while the rows stay
# separate (different files, different ids, different runtimes; never merged,
# never re-pointed, because `model_set_default` persists the id).
# ─────────────────────────────────────────────────────────────────────────────

ONNX = [
    dict(
        id="parakeet-v2", name="Parakeet V2", engine="Parakeet",
        base_model="nvidia/parakeet-tdt-0.6b-v2", architecture="parakeet",
        description="English only. The best model for English speakers",
        url="https://blob.handy.computer/parakeet-v2-int8.tar.gz",
        hf_repo_url="https://huggingface.co/nvidia/parakeet-ctc-0.6b-en",
        sha256="ac9b9429984dd565b25097337a887bb7f0f8ac393573661c651f0e7d31563991",
        fallback_size_mb=451, languages=["en"],
        scores_from="handy-computer/parakeet-tdt-0.6b-v2-gguf",
    ),
    dict(
        id="parakeet-v3", name="Parakeet V3", engine="Parakeet",
        base_model="nvidia/parakeet-tdt-0.6b-v3", architecture="parakeet",
        description="Fast and accurate. Supports 25 European languages",
        url="https://blob.handy.computer/parakeet-v3-int8.tar.gz",
        hf_repo_url="https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
        sha256="43d37191602727524a7d8c6da0eef11c4ba24320f5b4730f1a2497befc2efa77",
        fallback_size_mb=456,
        languages=["bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu",
                   "it", "lv", "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk"],
        scores_from="handy-computer/parakeet-tdt-0.6b-v3-gguf",
        recommended=True, recommended_rank=2,
    ),
    dict(
        id="moonshine-base", name="Moonshine Base", engine="Moonshine",
        base_model="usefulsensors/moonshine-base", architecture="moonshine",
        description="Very fast, English only. Handles accents well",
        url="https://blob.handy.computer/moonshine-base.tar.gz",
        hf_repo_url="https://huggingface.co/usefulsensors/moonshine-base",
        sha256="04bf6ab012cfceebd4ac7cf88c1b31d027bbdd3cd704649b692e2e935236b7e8",
        fallback_size_mb=55, languages=["en"],
        scores_from="handy-computer/moonshine-base-gguf",
    ),
    dict(
        id="moonshine-tiny", name="Moonshine Tiny", engine="MoonshineStreaming",
        base_model="usefulsensors/moonshine-tiny-streaming-en", architecture="moonshine_streaming",
        description="Ultra-fast, English only",
        url="https://blob.handy.computer/moonshine-tiny-streaming-en.tar.gz",
        hf_repo_url="https://huggingface.co/usefulsensors/moonshine-tiny",
        sha256="465addcfca9e86117415677dfdc98b21edc53537210333a3ecdb58509a80abaf",
        fallback_size_mb=31, languages=["en"],
        scores_from="handy-computer/moonshine-streaming-tiny-gguf",
    ),
    dict(
        id="moonshine-small", name="Moonshine Small", engine="MoonshineStreaming",
        base_model="usefulsensors/moonshine-small-streaming-en", architecture="moonshine_streaming",
        description="Fast, English only. Good balance of speed and accuracy",
        url="https://blob.handy.computer/moonshine-small-streaming-en.tar.gz",
        hf_repo_url="https://huggingface.co/usefulsensors/moonshine-small",
        sha256="dbb3e1c1832bd88a4ac712f7449a136cc2c9a18c5fe33a12ed1b7cb1cfe9cdd5",
        fallback_size_mb=99, languages=["en"],
        scores_from="handy-computer/moonshine-streaming-small-gguf",
    ),
    dict(
        id="moonshine-medium", name="Moonshine Medium", engine="MoonshineStreaming",
        base_model="usefulsensors/moonshine-medium-streaming-en", architecture="moonshine_streaming",
        description="English only. High quality",
        url="https://blob.handy.computer/moonshine-medium-streaming-en.tar.gz",
        hf_repo_url="https://huggingface.co/usefulsensors/moonshine-medium",
        sha256="07a66f3bff1c77e75a2f637e5a263928a08baae3c29c4c053fc968a9a9373d13",
        fallback_size_mb=192, languages=["en"],
        scores_from="handy-computer/moonshine-streaming-medium-gguf",
    ),
    dict(
        id="sense-voice", name="SenseVoice", engine="SenseVoice",
        base_model="FunAudioLLM/SenseVoiceSmall", architecture="sensevoice",
        description="Very fast. Chinese, English, Japanese, Korean, Cantonese",
        url="https://blob.handy.computer/sense-voice-int8.tar.gz",
        hf_repo_url="https://huggingface.co/FunAudioLLM/SenseVoiceSmall",
        sha256="171d611fe5d353a50bbb741b6f3ef42559b1565685684e9aa888ef563ba3e8a4",
        fallback_size_mb=152, languages=["zh", "en", "ja", "ko", "yue"],
        scores_from="handy-computer/SenseVoiceSmall-gguf",
    ),
    dict(
        id="gigaam-v3", name="GigaAM v3", engine="GigaAM",
        base_model="ai-sage/GigaAM-v3", architecture="gigaam",
        description="Russian speech recognition. Fast and accurate",
        url="https://blob.handy.computer/giga-am-v3-int8.tar.gz",
        hf_repo_url="https://huggingface.co/sberbank-ai/GigaAM",
        sha256="d872462268430db140b69b72e0fc4b787b194c1dbe51b58de39444d55b6da45b",
        fallback_size_mb=151, languages=["ru"],
        scores_from="handy-computer/gigaam-v3-ctc-gguf",
    ),
    dict(
        id="canary-flash", name="Canary 180M Flash", engine="Canary",
        base_model="nvidia/canary-180m-flash", architecture="canary",
        description="Very fast. English, German, Spanish, French. Supports translation",
        url="https://blob.handy.computer/canary-180m-flash.tar.gz",
        hf_repo_url="https://huggingface.co/nvidia/canary-180m-flash",
        sha256="6d9cfca6118b296e196eaedc1c8fa9788305a7b0f1feafdb6dc91932ab6e53f7",
        fallback_size_mb=146, languages=["en", "de", "es", "fr"],
        scores_from="handy-computer/canary-180m-flash-gguf",
    ),
    dict(
        id="canary-1b", name="Canary 1B v2", engine="Canary",
        base_model="nvidia/canary-1b-v2", architecture="canary",
        description="Accurate multilingual. 25 European languages. Supports translation",
        url="https://blob.handy.computer/canary-1b-v2.tar.gz",
        hf_repo_url="https://huggingface.co/nvidia/canary-1b-v2",
        sha256="02305b2a25f9cf3e7deaffa7f94df00efa44f442cd55c101c2cb9c000f904666",
        fallback_size_mb=691,
        languages=["bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu",
                   "it", "lv", "lt", "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk"],
        scores_from="handy-computer/canary-1b-v2-gguf",
    ),
    dict(
        id="cohere", name="Cohere", engine="Cohere",
        base_model="CohereForAI/canary-labse", architecture="cohere",
        description="A large, slower, but very accurate multilingual model",
        url="https://blob.handy.computer/cohere-int8.tar.gz",
        hf_repo_url="https://huggingface.co/CohereForAI/canary-labse",
        sha256="ea2257d52434f3644574f187dcdcf666e302cd11b92866116ab8e14cd9c887f0",
        fallback_size_mb=1708,
        languages=["en", "fr", "de", "it", "es", "pt", "el", "nl", "pl", "zh", "ja", "ko", "vi", "ar"],
        scores_from="handy-computer/cohere-transcribe-03-2026-gguf",
    ),
]

# Legacy whisper.cpp GGML `.bin` rows, loaded through transcribe-cpp.
# `capabilities_from` copies the timestamp granularity of the same weights in
# their GGUF container: granularity is a property of the architecture, not of
# the file format, and a `.bin` has no header to probe.
BIN = [
    dict(
        id="whisper-small", name="Whisper Small", engine="TranscribeCpp",
        base_model="openai/whisper-small", architecture="whisper",
        description="Fast and fairly accurate",
        url="https://blob.handy.computer/ggml-small.bin",
        hf_repo_url="https://huggingface.co/ggerganov/whisper.cpp",
        fallback_size_mb=465, recommended=True, recommended_rank=3,
        capabilities_from="handy-computer/whisper-small-gguf", scores_from="handy-computer/whisper-small-gguf",
    ),
    dict(
        id="whisper-medium", name="Whisper Medium", engine="TranscribeCpp",
        base_model="openai/whisper-medium", architecture="whisper",
        description="Good accuracy, medium speed",
        url="https://blob.handy.computer/whisper-medium-q4_1.bin",
        hf_repo_url="https://huggingface.co/ggerganov/whisper.cpp",
        fallback_size_mb=469,
        capabilities_from="handy-computer/whisper-medium-gguf", scores_from="handy-computer/whisper-medium-gguf",
    ),
    dict(
        id="whisper-turbo", name="Whisper Turbo", engine="TranscribeCpp",
        base_model="openai/whisper-large-v3-turbo", architecture="whisper",
        description="Balanced accuracy and speed",
        url="https://blob.handy.computer/ggml-large-v3-turbo.bin",
        hf_repo_url="https://huggingface.co/ggerganov/whisper.cpp",
        fallback_size_mb=1549,
        capabilities_from="handy-computer/whisper-large-v3-turbo-gguf",
        scores_from="handy-computer/whisper-large-v3-turbo-gguf",
    ),
    dict(
        id="whisper-large", name="Whisper Large", engine="TranscribeCpp",
        base_model="openai/whisper-large-v3", architecture="whisper",
        description="Good accuracy, but slow",
        url="https://blob.handy.computer/ggml-large-v3-q5_0.bin",
        hf_repo_url="https://huggingface.co/ggerganov/whisper.cpp",
        fallback_size_mb=1031,
        capabilities_from="handy-computer/whisper-large-v3-gguf", scores_from="handy-computer/whisper-large-v3-gguf",
    ),
    dict(
        id="breeze-asr", name="Breeze ASR", engine="TranscribeCpp",
        base_model="MediaTek-Research/Breeze-ASR-25", architecture="whisper",
        description="Optimized for Taiwanese Mandarin with code-switching support",
        url="https://blob.handy.computer/breeze-asr-q5_k.bin",
        hf_repo_url="https://huggingface.co/MediaTek-Research/breeze-asr",
        fallback_size_mb=1030, languages=["zh", "en"],
        capabilities_from="handy-computer/Breeze-ASR-25-gguf", scores_from="handy-computer/Breeze-ASR-25-gguf",
    ),
]

# ─────────────────────────────────────────────────────────────────────────────
# GGUF pilots: same weights as an ONNX/.bin row, desktop-only runtime, so the
# two can be compared on the same audio. Unranked and not recommended — landing
# a row here is not the same as promoting it.
#
# `quant` picks which of the repo's `files[]` we ship as our default artifact;
# Handy's own `default_quant` is used when this says `None`.
# ─────────────────────────────────────────────────────────────────────────────
GGUF_PILOTS = [
    dict(id="parakeet-v3-gguf", handy_id="handy-computer/parakeet-tdt-0.6b-v3-gguf",
         name="Parakeet V3 (GGUF)", base_model="nvidia/parakeet-tdt-0.6b-v3",
         description="Desktop pilot: the same weights as Parakeet V3, loaded through transcribe-cpp",
         quant=None),
    dict(id="whisper-small-gguf", handy_id="handy-computer/whisper-small-gguf",
         name="Whisper Small (GGUF)", base_model="openai/whisper-small",
         description="Desktop pilot: the same weights as Whisper Small in a GGUF container",
         quant=None),
]

# Whisper speaks ~99 languages; the list is curated data (a `.bin` carries no
# language metadata), shared by every GGML whisper row.
WHISPER_LANGS = re.split(r",\s*", """
en, zh, de, es, ru, ko, fr, ja, pt, tr, pl, ca, nl, ar, sv, it, id, hi, fi, vi,
he, uk, el, ms, cs, ro, da, hu, ta, no, th, ur, hr, bg, lt, la, mi, ml, cy, sk,
te, fa, lv, bn, sr, az, sl, kn, et, mk, br, eu, is, hy, ne, mn, bs, kk, sq, sw,
gl, mr, pa, si, km, sn, yo, so, af, oc, ka, be, tg, sd, gu, am, yi, lo, uz, fo,
ht, ps, tk, nn, mt, sa, lb, my, bo, tl, mg, as, tt, haw, ln, ha, ba, jw, su
""".replace("\n", " ").replace(" ", ""))

# ONNX families carry no GGUF header, so nothing can be probed pre-download and
# transcribe-rs reports only `supports_timestamps` without granularity. These are
# the honest baked values; the runtime is still the authority (see 1c in the plan).
# `max_chunk_secs: None` means "no window limit we know of" — not "no limit".
ONNX_CAPS = {
    "Parakeet": dict(timestamps="token", streaming=False, translate=False,
                     lang_detect=False, max_chunk_secs=240.0),
    "Moonshine": dict(timestamps=None, streaming=False, translate=False, lang_detect=False, max_chunk_secs=None),
    "MoonshineStreaming": dict(timestamps=None, streaming=True, translate=False, lang_detect=False, max_chunk_secs=None),
    "SenseVoice": dict(timestamps=None, streaming=False, translate=False, lang_detect=False, max_chunk_secs=None),
    "GigaAM": dict(timestamps=None, streaming=False, translate=False, lang_detect=False, max_chunk_secs=None),
    "Canary": dict(timestamps=None, streaming=False, translate=True, lang_detect=False, max_chunk_secs=None),
    "Cohere": dict(timestamps=None, streaming=False, translate=False, lang_detect=False, max_chunk_secs=None),
}

# Languages transcribe-cpp can load, mirrored from its architecture registry
# (`src/transcribe-arch.cpp` in transcribe-cpp-sys). A test in catalog_stt.rs
# fails if a row names an architecture absent here, which is how a typo in the
# curation table becomes a compile-time fact instead of a runtime surprise.
KNOWN_ARCHES = [
    "whisper", "parakeet", "qwen3_asr", "voxtral", "voxtral_realtime", "cohere",
    "cohere_asr", "canary", "canary_qwen", "moonshine", "moonshine_streaming",
    "sensevoice", "gigaam", "granite", "granite_speech", "granite_nar",
    "granite_speech_nar", "funasr_nano", "medasr", "moss", "sortformer",
]


def head_size(url: str):
    """Measured Content-Length for a URL, or None when it cannot be asked."""
    req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": "fms-app-gen_catalog"})
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            length = resp.headers.get("Content-Length")
            return int(length) if length else None
    except Exception as e:  # offline, 403, no CL — the caller falls back
        print("  warn: HEAD %s -> %s" % (url, e), file=sys.stderr)
        return None


def rescale(score):
    """Handy stores 0-100 derived scores; our bars use 0..1."""
    return None if score is None else round(score / 100.0, 2)


def caps_from_handy(entry):
    """Capability block straight off the card-derived Handy entry."""
    c = entry.get("capabilities", {})
    return dict(
        timestamps=c.get("timestamps"),
        streaming=bool(c.get("streaming")),
        translate=bool(c.get("translate")),
        lang_detect=bool(c.get("lang_detect")),
        # max_audio_ms is not in the cards; the load-time probe reports it.
        max_chunk_secs=None,
    )


def mirror_url(repo_id, revision, filename):
    return "%s/%s/%s/%s" % (MIRRORS[0].rstrip("/"), repo_id, revision, filename)


def pick_file(entry, quant):
    files = entry.get("files", [])
    want = quant or entry.get("default_quant")
    for f in files:
        if f.get("quant") == want:
            return f
    return files[0] if files else None


def build(onnx_rows, bin_rows, pilots, handy_by_id, use_head):
    models = []

    for row in onnx_rows:
        caps = dict(ONNX_CAPS[row["engine"]])
        sibling = handy_by_id.get(row.get("scores_from"))
        # Only the scores cross the container boundary: accuracy and speed belong
        # to the weights and the task. Timestamp granularity belongs to the
        # implementation, so transcribe-cpp emitting token rows for the GGUF of a
        # model says nothing about what the ONNX graph exposes. That stays curated.
        size_measured = head_size(row["url"]) if use_head else None
        models.append({
            "id": row["id"], "name": row["name"], "description": row["description"],
            "engine": row["engine"], "base_model": row["base_model"],
            "architecture": row["architecture"], "container": "onnx-directory",
            "repo_id": None, "revision": None, "filename": None, "quant": None,
            "blob_url": row["url"], "hf_repo_url": row["hf_repo_url"],
            "sha256": row.get("sha256"), "is_directory": True,
            "size_bytes": size_measured or row["fallback_size_mb"] * 1_000_000,
            "size_source": "head" if size_measured else "table",
            "languages": row.get("languages") or (list(sibling.get("languages")) if sibling else []),
            "capabilities": caps,
            "accuracy_score": rescale(sibling.get("accuracy_score")) if sibling else None,
            "speed_score": rescale(sibling.get("speed_score")) if sibling else None,
            "scores_from": row.get("scores_from"),
            "recommended": bool(row.get("recommended")),
            "recommended_rank": row.get("recommended_rank"),
        })

    for row in bin_rows:
        caps_src = handy_by_id.get(row.get("capabilities_from"))
        size_measured = head_size(row["url"]) if use_head else None
        models.append({
            "id": row["id"], "name": row["name"], "description": row["description"],
            "engine": row["engine"], "base_model": row["base_model"],
            "architecture": row["architecture"], "container": "ggml-bin",
            "repo_id": None, "revision": None, "filename": None, "quant": None,
            "blob_url": row["url"], "hf_repo_url": row["hf_repo_url"],
            "sha256": row.get("sha256"), "is_directory": False,
            "size_bytes": size_measured or row["fallback_size_mb"] * 1_000_000,
            "size_source": "head" if size_measured else "table",
            # Language set comes from the same weights in their GGUF container
            # where that sibling exists; the typed list is only the fallback.
            "languages": row.get("languages") or (list(caps_src.get("languages")) if caps_src and caps_src.get("languages") else WHISPER_LANGS),
            "capabilities": caps_from_handy(caps_src) if caps_src else dict(
                timestamps=None, streaming=False, translate=False, lang_detect=False, max_chunk_secs=None),
            "accuracy_score": rescale(caps_src.get("accuracy_score")) if caps_src and row.get("scores_from") else None,
            "speed_score": rescale(caps_src.get("speed_score")) if caps_src and row.get("scores_from") else None,
            "scores_from": row.get("scores_from"),
            "recommended": bool(row.get("recommended")),
            "recommended_rank": row.get("recommended_rank"),
        })

    for p in pilots:
        entry = handy_by_id.get(p["handy_id"])
        if not entry:
            sys.exit("pilot %s: %s is not in the Handy catalog" % (p["id"], p["handy_id"]))
        f = pick_file(entry, p.get("quant"))
        if not f:
            sys.exit("pilot %s: %s lists no files" % (p["id"], p["handy_id"]))
        revision = entry.get("revision")
        if not revision:
            sys.exit("pilot %s: %s has no pinned revision" % (p["id"], p["handy_id"]))
        # A mirror is an untrusted bit-pipe: without a hash we never offer it.
        if not f.get("sha256"):
            sys.exit("pilot %s: %s/%s has no sha256" % (p["id"], p["handy_id"], f["filename"]))
        models.append({
            "id": p["id"], "name": p["name"], "description": p["description"],
            "engine": "TranscribeCpp", "base_model": p["base_model"],
            "architecture": entry.get("architecture"), "container": "gguf",
            "repo_id": p["handy_id"], "revision": revision,
            "filename": f["filename"], "quant": f.get("quant"),
            "blob_url": mirror_url(p["handy_id"], revision, f["filename"]),
            "hf_repo_url": "https://huggingface.co/%s" % p["handy_id"],
            "sha256": f["sha256"], "is_directory": False,
            "size_bytes": f.get("size_bytes"),
            "size_source": "handy_catalog",
            "languages": list(entry.get("languages", [])),
            "capabilities": caps_from_handy(entry),
            "accuracy_score": rescale(entry.get("accuracy_score")),
            "speed_score": rescale(entry.get("speed_score")),
            "scores_from": p["handy_id"],
            "recommended": False, "recommended_rank": None,
        })

    return models


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[1] if __doc__ else "")
    ap.add_argument("--handy-catalog", default=DEFAULT_HANDY)
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--no-head", action="store_true",
                    help="skip CDN Content-Length probes; use the table's fallback sizes")
    args = ap.parse_args()

    handy_path = pathlib.Path(args.handy_catalog)
    if not handy_path.exists():
        sys.exit("Handy catalog not found at %s — pass --handy-catalog" % handy_path)
    handy = json.loads(handy_path.read_text(encoding="utf-8"))
    handy_by_id = {m["id"]: m for m in handy["models"]}

    for table, label in ((ONNX, "ONNX"), (BIN, "BIN")):
        for row in table:
            arch = row.get("architecture")
            if arch not in KNOWN_ARCHES:
                sys.exit("%s row %s: architecture %r is not loadable by transcribe-cpp's registry"
                         % (label, row["id"], arch))

    models = build(ONNX, BIN, GGUF_PILOTS, handy_by_id, use_head=not args.no_head)

    # Row order is not cosmetic: it is the Models page order, and the first
    # loadable row is the built-in fallback `ModelState::new` selects when the
    # user has no preference — `whisper-small` in the hand-written table this
    # replaces. Sort by container so that keeps picking the same model, and so
    # the desktop-only GGUF pilots land last, after everything they imitate.
    # Python's sort is stable, so the curated order inside each group survives.
    group = {"ggml-bin": 0, "onnx-directory": 1, "gguf": 2}
    models.sort(key=lambda m: group[m["container"]])

    # A curation table naming a Handy repo that is not in the catalog is a typo,
    # not an empty field: warn loudly, since the row silently loses its scores.
    for row in ONNX + BIN:
        for key in ("scores_from", "capabilities_from"):
            ref = row.get(key)
            if ref and ref not in handy_by_id:
                print("  warn: %s.%s -> %s is not in the Handy catalog" % (row["id"], key, ref),
                      file=sys.stderr)

    ids = [m["id"] for m in models]
    if len(ids) != len(set(ids)):
        sys.exit("duplicate model ids: %s" % sorted(i for i in set(ids) if ids.count(i) > 1))

    out = {
        "catalog_version": CATALOG_VERSION,
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "sources": {
            "handy_catalog": str(handy_path).replace("\\", "/"),
            "handy_catalog_version": handy.get("catalog_version"),
            "handy_generated_at": handy.get("generated_at"),
        },
        "mirrors": MIRRORS,
        "known_architectures": KNOWN_ARCHES,
        "models": models,
    }
    path = pathlib.Path(args.out)
    path.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print("wrote %s: %d models (%d onnx, %d bin, %d gguf)" % (
        path, len(models), len(ONNX), len(BIN), len(GGUF_PILOTS)))


if __name__ == "__main__":
    main()
