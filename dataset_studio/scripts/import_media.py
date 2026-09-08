#!/usr/bin/env python3
"""
Stage 1: Import media files into a dataset.

Copies audio/video files from a source directory into the dataset's media/ folder.

Usage:
    python scripts/import_media.py <dataset_path> <source_dir>
"""

import argparse
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from config import AUDIO_EXTENSIONS
from lib.dataset import load_info, touch_info


def main():
    parser = argparse.ArgumentParser(description="Import media files into a dataset.")
    parser.add_argument("dataset_path", type=Path, help="Path to the dataset directory")
    parser.add_argument("source_dir", type=Path, help="Directory containing audio files")
    parser.add_argument("--link", action="store_true", help="Create symlinks instead of copying")
    args = parser.parse_args()

    dataset_dir = args.dataset_path.resolve()
    source_dir = args.source_dir.resolve()
    media_dir = dataset_dir / "media"

    if not dataset_dir.exists():
        print(f"Error: dataset directory not found: {dataset_dir}")
        sys.exit(1)

    if not source_dir.exists():
        print(f"Error: source directory not found: {source_dir}")
        sys.exit(1)

    # Load info to verify it's a valid dataset
    info = load_info(dataset_dir)

    # Find audio files in source
    audio_files = sorted(
        p for p in source_dir.iterdir()
        if p.is_file() and p.suffix.lower() in AUDIO_EXTENSIONS
    )

    if not audio_files:
        print(f"No audio files found in {source_dir}")
        sys.exit(1)

    media_dir.mkdir(exist_ok=True)
    copied = 0
    skipped = 0

    for src_file in audio_files:
        dst_file = media_dir / src_file.name
        if dst_file.exists():
            print(f"  Skip (exists): {src_file.name}")
            skipped += 1
            continue

        if args.link:
            dst_file.symlink_to(src_file)
            print(f"  Linked: {src_file.name}")
        else:
            shutil.copy2(src_file, dst_file)
            print(f"  Copied: {src_file.name}")
        copied += 1

    # Update timestamp
    touch_info(dataset_dir)

    print(f"\nDone: {copied} imported, {skipped} skipped")
    print(f"Dataset: {info['name']} ({info['uuid'][:8]}...)")


if __name__ == "__main__":
    main()
