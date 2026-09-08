#!/usr/bin/env python3
"""
Stage 2b: Generate waveform JSON files using audiowaveform.

Requires audiowaveform to be installed on the system PATH.
https://github.com/bbc/audiowaveform

Usage:
    python scripts/generate_waveforms.py <dataset_path> [--pixels-per-second N]
"""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lib.dataset import load_info, list_audio_files


def find_audiowaveform() -> str:
    """Find audiowaveform binary on system PATH."""
    path = shutil.which("audiowaveform")
    if path:
        return path
    print("Error: audiowaveform not found on system PATH.")
    print("Install: https://github.com/bbc/audiowaveform#installation")
    sys.exit(1)


def main():
    parser = argparse.ArgumentParser(description="Generate waveform JSON files.")
    parser.add_argument("dataset_path", type=Path, help="Path to the dataset directory")
    parser.add_argument("--pixels-per-second", type=int, default=100,
                        help="Resolution of waveform (default: 100)")
    args = parser.parse_args()

    dataset_dir = args.dataset_path.resolve()
    if not dataset_dir.exists():
        print(f"Error: dataset not found: {dataset_dir}")
        sys.exit(1)

    info = load_info(dataset_dir)
    media_dir = dataset_dir / "media"
    waveform_dir = dataset_dir / "waveform"

    audio_files = list_audio_files(media_dir)
    if not audio_files:
        print("No audio files found in media/")
        sys.exit(1)

    audiowaveform = find_audiowaveform()
    waveform_dir.mkdir(exist_ok=True)

    total = len(audio_files)
    generated = 0
    skipped = 0

    for i, audio_path in enumerate(audio_files, 1):
        stem = audio_path.stem
        output_path = waveform_dir / f"{stem}.json"

        if output_path.exists():
            print(f"  [{i}/{total}] Skip (exists): {audio_path.name}")
            skipped += 1
            continue

        print(f"  [{i}/{total}] Generating: {audio_path.name} ... ", end="", flush=True)

        result = subprocess.run(
            [
                audiowaveform,
                "-i", str(audio_path),
                "-o", str(output_path),
                "--pixels-per-second", str(args.pixels_per_second),
                "--bits", "8",
            ],
            capture_output=True,
            text=True,
        )

        if result.returncode == 0:
            print("OK")
            generated += 1
        else:
            print("FAIL")
            print(f"    {result.stderr.strip()}")
            output_path.unlink(missing_ok=True)

    print(f"\nDone: {generated} generated, {skipped} skipped")


if __name__ == "__main__":
    main()
