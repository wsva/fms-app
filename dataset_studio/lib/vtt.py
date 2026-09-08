"""
VTT (WebVTT) parser and writer.

Compatible with the VTT output produced by the fms-app Tauri backend.
"""

import re
from dataclasses import dataclass


@dataclass
class VttCue:
    start_ms: int
    end_ms: int
    content: str


def parse_vtt_timestamp(s: str) -> int:
    """Parse a VTT timestamp like '00:01:23.456' into milliseconds."""
    s = s.strip().split()[0]  # ignore position info after space
    parts = s.split(":")
    if len(parts) != 3:
        raise ValueError(f"Invalid VTT timestamp: {s}")
    hours = int(parts[0])
    minutes = int(parts[1])
    sec_parts = parts[2].split(".")
    seconds = int(sec_parts[0])
    millis = int(sec_parts[1].ljust(3, "0")[:3]) if len(sec_parts) == 2 else 0
    return hours * 3_600_000 + minutes * 60_000 + seconds * 1_000 + millis


def format_vtt_time(ms: int) -> str:
    """Format milliseconds as HH:MM:SS.mmm for VTT."""
    total_ms = max(0, ms)
    h = total_ms // 3_600_000
    m = (total_ms % 3_600_000) // 60_000
    s = (total_ms % 60_000) // 1_000
    ms_rem = total_ms % 1_000
    return f"{h:02}:{m:02}:{s:02}.{ms_rem:03}"


def parse_vtt(path) -> list[VttCue]:
    """Parse a VTT file into a list of VttCue objects."""
    from pathlib import Path as P
    content = P(path).read_text(encoding="utf-8")
    return parse_vtt_string(content)


def parse_vtt_string(content: str) -> list[VttCue]:
    """Parse VTT content string into a list of VttCue objects."""
    cues = []
    lines = content.split("\n")
    i = 0

    # Skip WEBVTT header
    while i < len(lines):
        line = lines[i].strip()
        if line.startswith("WEBVTT") or line == "":
            i += 1
            continue
        break

    while i < len(lines):
        # Skip blank lines
        while i < len(lines) and lines[i].strip() == "":
            i += 1
        if i >= len(lines):
            break

        # Look for timing line
        line = lines[i].strip()
        if "-->" not in line:
            i += 1
            continue

        parts = line.split("-->")
        if len(parts) != 2:
            i += 1
            continue

        try:
            start_ms = parse_vtt_timestamp(parts[0])
            end_ms = parse_vtt_timestamp(parts[1])
        except ValueError:
            i += 1
            continue

        i += 1
        text_lines = []
        while i < len(lines) and lines[i].strip() != "":
            text_lines.append(lines[i])
            i += 1

        text = "\n".join(text_lines)
        if text.strip():
            cues.append(VttCue(start_ms=start_ms, end_ms=end_ms, content=text))

    return cues


def write_vtt(path, cues: list[VttCue]) -> None:
    """Write a list of VttCue objects to a VTT file."""
    from pathlib import Path as P
    lines = ["WEBVTT", ""]
    for cue in cues:
        lines.append(f"{format_vtt_time(cue.start_ms)} --> {format_vtt_time(cue.end_ms)}")
        lines.append(cue.content)
        lines.append("")
    P(path).write_text("\n".join(lines), encoding="utf-8")
