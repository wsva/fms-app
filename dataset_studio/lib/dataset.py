"""
Dataset discovery, info.json read/write, and utility functions.
"""

import json
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from config import STRUCTURE_VERSION


def generate_uuid() -> str:
    return str(uuid.uuid4())


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def create_info(name: str, description: str = "") -> dict:
    """Create a new info.json dict with fresh UUID and timestamp."""
    return {
        "name": name,
        "uuid": generate_uuid(),
        "description": description,
        "parent_uuid": "",
        "version": 1,
        "structure": STRUCTURE_VERSION,
        "updated": now_iso(),
    }


def load_info(dataset_dir: Path) -> dict:
    """Load and return info.json from a dataset directory."""
    info_path = dataset_dir / "info.json"
    if not info_path.exists():
        raise FileNotFoundError(f"No info.json found in {dataset_dir}")
    with open(info_path, "r", encoding="utf-8") as f:
        return json.load(f)


def save_info(dataset_dir: Path, info: dict) -> None:
    """Write info dict to info.json in the dataset directory."""
    info["updated"] = now_iso()
    info_path = dataset_dir / "info.json"
    with open(info_path, "w", encoding="utf-8") as f:
        json.dump(info, f, indent=2, ensure_ascii=False)


def touch_info(dataset_dir: Path) -> None:
    """Update the timestamp in an existing info.json."""
    info = load_info(dataset_dir)
    save_info(dataset_dir, info)


def list_audio_files(media_dir: Path) -> list[Path]:
    """Return sorted list of audio/video files in a media directory."""
    from config import AUDIO_EXTENSIONS
    if not media_dir.exists():
        return []
    files = [
        p for p in sorted(media_dir.iterdir())
        if p.is_file() and p.suffix.lower() in AUDIO_EXTENSIONS
    ]
    return files


def find_dataset_by_uuid(base_dir: Path, target_uuid: str) -> Optional[Path]:
    """Search subdirectories of base_dir for a dataset with the given UUID."""
    if not base_dir.exists():
        return None
    for entry in sorted(base_dir.iterdir()):
        if not entry.is_dir():
            continue
        info_path = entry / "info.json"
        if not info_path.exists():
            continue
        try:
            info = load_info(entry)
            if info.get("uuid") == target_uuid:
                return entry
        except (json.JSONDecodeError, KeyError):
            continue
    return None
