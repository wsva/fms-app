"""
Shared configuration for dataset_studio scripts.
"""

import os
import platform
from pathlib import Path


# ---------------------------------------------------------------------------
# Model directory (shared with fms-app desktop client)
# ---------------------------------------------------------------------------

def default_model_dir() -> Path:
    """Return the platform-specific directory where fms-app stores models."""
    system = platform.system()
    if system == "Windows":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local"))
        return base / "fms-app" / "models"
    elif system == "Darwin":
        return Path.home() / "Library" / "Application Support" / "fms-app" / "models"
    else:
        return Path.home() / ".local" / "share" / "fms-app" / "models"


MODEL_DIR = Path(os.environ.get("FMS_MODEL_DIR", default_model_dir()))

DEFAULT_MODEL = "parakeet-v3"

# ---------------------------------------------------------------------------
# Audio file extensions
# ---------------------------------------------------------------------------

AUDIO_EXTENSIONS = {".mp3", ".wav", ".flac", ".ogg", ".m4a", ".m4b", ".aac", ".mp4", ".webm", ".mkv"}

# ---------------------------------------------------------------------------
# Dataset structure version
# ---------------------------------------------------------------------------

STRUCTURE_VERSION = "dictation-v1"
