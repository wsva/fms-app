"""
Audio loading and preprocessing utilities shared across STT engines.
"""

import numpy as np
from pathlib import Path


def load_audio(audio_path: Path, sample_rate: int = 16000) -> np.ndarray:
    """
    Load an audio file and resample to the target sample rate (mono).

    Supports all formats handled by librosa/soundfile:
    mp3, wav, flac, ogg, m4a, mp4, webm, etc.

    Returns:
        numpy array of float32 samples in [-1, 1] at the target sample rate.
    """
    import librosa
    audio, _ = librosa.load(str(audio_path), sr=sample_rate, mono=True)
    return audio.astype(np.float32)
