#!/usr/bin/env python3
"""
Stage 2a: Generate subtitles (VTT files) using a Parakeet STT model.

Transcribes each media file and writes sentence-level VTT subtitles.

Usage:
    python scripts/generate_subtitles.py <dataset_path> [--model MODEL_ID]
"""

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from config import MODEL_DIR, DEFAULT_MODEL
from lib.dataset import load_info, list_audio_files, touch_info
from lib.vtt import VttCue, write_vtt
from lib.stt.base import STTEngine


def get_engine(model_id: str) -> STTEngine:
    """Create the Parakeet STT engine."""
    from lib.stt.parakeet import create_engine
    model_dir = MODEL_DIR / model_id
    if not model_dir.exists():
        print(f"Error: model directory not found: {model_dir}")
        print("Download the model via fms-app first.")
        sys.exit(1)
    return create_engine(model_id, model_dir)


def segments_to_sentence_cues(segments, merge_sentences: bool = True) -> list[VttCue]:
    """
    Convert STT segments to VTT cues.

    If merge_sentences is True, merges word-level segments into sentence-level
    cues by splitting on sentence-ending punctuation.
    """
    from lib.text import is_sentence_end

    if not segments:
        return []

    if not merge_sentences:
        return [
            VttCue(
                start_ms=int(seg.start * 1000),
                end_ms=int(seg.end * 1000),
                content=seg.text,
            )
            for seg in segments
        ]

    # Merge segments into sentences
    cues = []
    current_text = ""
    start_sec = segments[0].start

    for seg in segments:
        if not current_text:
            start_sec = seg.start
        current_text += seg.text

        # Check if this segment ends with sentence-ending punctuation
        stripped = seg.text.rstrip()
        if stripped and is_sentence_end(stripped[-1]):
            cues.append(VttCue(
                start_ms=int(start_sec * 1000),
                end_ms=int(seg.end * 1000),
                content=current_text.strip(),
            ))
            current_text = ""

    # Flush remaining text
    if current_text.strip():
        cues.append(VttCue(
            start_ms=int(start_sec * 1000),
            end_ms=int(segments[-1].end * 1000),
            content=current_text.strip(),
        ))

    return cues


def main():
    parser = argparse.ArgumentParser(description="Generate subtitles for a dataset.")
    parser.add_argument("dataset_path", type=Path, help="Path to the dataset directory")
    parser.add_argument("--model", type=str, default=DEFAULT_MODEL,
                        help=f"Model ID (default: {DEFAULT_MODEL})")
    args = parser.parse_args()

    dataset_dir = args.dataset_path.resolve()
    if not dataset_dir.exists():
        print(f"Error: dataset not found: {dataset_dir}")
        sys.exit(1)

    info = load_info(dataset_dir)
    media_dir = dataset_dir / "media"
    subtitle_dir = dataset_dir / "subtitle"

    audio_files = list_audio_files(media_dir)
    if not audio_files:
        print("No audio files found in media/")
        sys.exit(1)

    # Initialize engine
    print(f"Loading Parakeet engine with model '{args.model}'...")
    engine = get_engine(args.model)
    print(f"Engine: {engine.name()}")

    # Clear old subtitles
    if subtitle_dir.exists():
        for f in subtitle_dir.glob("*.vtt"):
            f.unlink()
    subtitle_dir.mkdir(exist_ok=True)

    # Transcribe each file
    total = len(audio_files)
    for i, audio_path in enumerate(audio_files, 1):
        stem = audio_path.stem
        vtt_path = subtitle_dir / f"{stem}.vtt"

        print(f"\n[{i}/{total}] Transcribing: {audio_path.name}")
        result = engine.transcribe(audio_path)

        # Convert segments to VTT cues
        if result.segments:
            cues = segments_to_sentence_cues(result.segments)
        else:
            # No segments — write full text as single cue
            cues = [VttCue(start_ms=0, end_ms=0, content=result.text)]

        write_vtt(vtt_path, cues)
        print(f"  -> {len(cues)} cues written to {vtt_path.name}")
        if result.text:
            preview = result.text[:80] + ("..." if len(result.text) > 80 else "")
            print(f"  Text: {preview}")

    # Update timestamp
    touch_info(dataset_dir)
    print(f"\nDone: {total} files transcribed")


if __name__ == "__main__":
    main()
