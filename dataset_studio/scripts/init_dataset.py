#!/usr/bin/env python3
"""
Stage 0: Initialize a new dataset directory structure.

Creates the directory skeleton and writes a fresh info.json with a new UUID.

Usage:
    python scripts/init_dataset.py <dataset_path> [--name NAME] [--description DESC]
"""

import argparse
import sys
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from lib.dataset import create_info, save_info


SUBDIRS = ["media", "subtitle", "waveform", "transcript"]


def main():
    parser = argparse.ArgumentParser(description="Initialize a new dataset directory.")
    parser.add_argument("path", type=Path, help="Path for the new dataset directory")
    parser.add_argument("--name", type=str, default=None, help="Dataset name (defaults to directory name)")
    parser.add_argument("--description", type=str, default="", help="Short description")
    args = parser.parse_args()

    dataset_dir = args.path.resolve()
    name = args.name or dataset_dir.name

    if dataset_dir.exists():
        print(f"Error: directory already exists: {dataset_dir}")
        sys.exit(1)

    # Create directory structure
    dataset_dir.mkdir(parents=True)
    for subdir in SUBDIRS:
        (dataset_dir / subdir).mkdir()

    # Write info.json
    info = create_info(name=name, description=args.description)
    save_info(dataset_dir, info)

    print(f"Created dataset: {dataset_dir}")
    print(f"  UUID: {info['uuid']}")
    print(f"  Name: {info['name']}")


if __name__ == "__main__":
    main()
