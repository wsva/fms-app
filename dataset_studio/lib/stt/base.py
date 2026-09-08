"""
Abstract base class for STT engines and shared data types.
"""

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class Segment:
    """A timed text segment from transcription."""
    start: float  # seconds
    end: float    # seconds
    text: str


@dataclass
class TranscriptionResult:
    """Full transcription output."""
    text: str
    segments: list[Segment] = field(default_factory=list)


class STTEngine(ABC):
    """Abstract base class for speech-to-text engines."""

    @abstractmethod
    def transcribe(self, audio_path: Path) -> TranscriptionResult:
        """
        Transcribe an audio file.

        Args:
            audio_path: Path to the audio file (any format supported by librosa).

        Returns:
            TranscriptionResult with full text and timed segments.
        """
        ...

    @abstractmethod
    def name(self) -> str:
        """Return a human-readable name for this engine."""
        ...
