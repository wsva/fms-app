"""
Adjust subtitle cue timestamps using waveform energy analysis.

Reads cues from listen_subtitle_cue, loads the corresponding audiowaveform
JSON, detects silence regions via energy thresholding, then snaps each cue's
start/end boundaries to the nearest silence points.

Creates a new listen_subtitle row (with note "adjusted using waveform") and
copies all cues into it with new UUIDs and adjusted timestamps.

Usage:
    python adjust_cue_time.py <db_path> <subtitle_uuid> <waveform_dir>
"""

import json
import sqlite3
import sys
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


# ============================================================
# Configuration
# ============================================================

# RMS energy threshold = noise_floor * this multiplier.
# Higher = less sensitive (fewer silence regions detected).
SILENCE_THRESHOLD_FACTOR = 4.0

# Minimum silence duration in ms.  Gaps shorter than this are ignored
# (they are likely pauses within words, not sentence boundaries).
MIN_SILENCE_MS = 300

# Maximum distance (ms) to snap a boundary.  Prevents jumping to a
# completely wrong segment when no nearby silence exists.
MAX_SNAP_MS = 500

# When a silence region adjacent to a cue boundary is at least this
# long (ms), expand the boundary deeper into the silence by EXPAND_MS.
# This gives each cue more breathing room when there is enough silence.
EXPAND_THRESHOLD_MS = 500
EXPAND_MS = 500


# ============================================================
# Models
# ============================================================

@dataclass
class Cue:
    uuid: str
    order_num: int
    start_ms: int
    end_ms: int
    content: str
    reference: str | None


# ============================================================
# Waveform loading
# ============================================================

def load_waveform(waveform_dir: Path, media_source: str) -> tuple[list[int], float] | None:
    """
    Load audiowaveform JSON for a media file.

    Returns (data_array, ms_per_pixel) or None if file not found.
    """
    stem = Path(media_source).stem
    wf_path = waveform_dir / f"{stem}.json"
    if not wf_path.exists():
        return None

    with open(wf_path, encoding="utf-8") as f:
        obj = json.load(f)

    data: list[int] = obj.get("data", [])
    sample_rate: int = obj.get("sample_rate", 16000)
    samples_per_pixel: int = obj.get("samples_per_pixel", 1)

    if samples_per_pixel <= 0 or sample_rate <= 0:
        return None

    ms_per_pixel = (samples_per_pixel / sample_rate) * 1000.0
    return data, ms_per_pixel


# ============================================================
# Energy envelope & silence detection
# ============================================================

def compute_energy(envelope: list[int]) -> list[float]:
    """
    Compute peak-to-peak amplitude per pixel from min/max envelope pairs.

    The audiowaveform data array contains alternating (min, max) values
    per pixel column as 8-bit unsigned integers (0-255, 128 = silence).
    We compute amplitude as max-min for each pair.
    """
    n_pairs = len(envelope) // 2
    energy = []
    for i in range(n_pairs):
        lo = envelope[2 * i]
        hi = envelope[2 * i + 1]
        # Amplitude = peak-to-peak range (unsigned 8-bit)
        amp = hi - lo if hi >= lo else lo - hi
        energy.append(float(amp))
    return energy


def estimate_noise_floor(energy: list[float]) -> float:
    """Estimate background noise level from the quietest 10% of frames."""
    if not energy:
        return 1.0
    sorted_e = sorted(energy)
    n = max(1, len(sorted_e) // 10)
    avg = sum(sorted_e[:n]) / n
    return max(avg, 1.0)


def find_silence_regions(
    energy: list[float],
    ms_per_pixel: float,
) -> list[tuple[int, int]]:
    """
    Detect silence regions in the energy envelope.

    Returns list of (start_ms, end_ms) for each silence region.
    A silence region is a contiguous run of frames where energy is
    below the threshold AND the run is at least MIN_SILENCE_MS long.
    """
    if not energy:
        return []

    noise_floor = estimate_noise_floor(energy)
    threshold = noise_floor * SILENCE_THRESHOLD_FACTOR

    # Mark each frame as silent or not
    is_silent = [e < threshold for e in energy]

    # Find contiguous silent runs
    regions: list[tuple[int, int]] = []  # (start_pixel, end_pixel)
    run_start: int | None = None

    for i, silent in enumerate(is_silent):
        if silent and run_start is None:
            run_start = i
        elif not silent and run_start is not None:
            duration_ms = (i - run_start) * ms_per_pixel
            if duration_ms >= MIN_SILENCE_MS:
                regions.append((run_start, i - 1))
            run_start = None

    # Handle trailing silence
    if run_start is not None:
        duration_ms = (len(is_silent) - run_start) * ms_per_pixel
        if duration_ms >= MIN_SILENCE_MS:
            regions.append((run_start, len(is_silent) - 1))

    # Convert pixel indices to milliseconds
    return [
        (int(s * ms_per_pixel), int(e * ms_per_pixel))
        for s, e in regions
    ]


# ============================================================
# Boundary snapping
# ============================================================

def snap_start(original_ms: int, silences: list[tuple[int, int]]) -> int:
    """
    Snap a cue start to the beginning of the nearest silence before it.

    Finds a silence region whose end <= original_ms (entirely before the
    cue).  Snaps to s_start — the far edge of that silence gap.
    """
    best = original_ms
    best_dist = MAX_SNAP_MS

    for s_start, s_end in silences:
        if s_end <= original_ms:
            dist = original_ms - s_start
            if dist <= best_dist:
                best = s_start
                best_dist = dist

    return max(0, best)


def snap_end(original_ms: int, silences: list[tuple[int, int]]) -> int:
    """
    Snap a cue end to the end of the nearest silence after it.

    Finds a silence region whose start >= original_ms (entirely after the
    cue).  Snaps to s_end — the far edge of that silence gap.
    """
    best = original_ms
    best_dist = MAX_SNAP_MS

    for s_start, s_end in silences:
        if s_start >= original_ms:
            dist = s_end - original_ms
            if dist <= best_dist:
                best = s_end
                best_dist = dist

    return best


def expand_start(snapped_ms: int, silences: list[tuple[int, int]]) -> int:
    """
    After snapping to the outer edge of a silence region, try to move
    start even earlier into the *previous* silence region (the one before
    the gap we already snapped into).  If that silence region is at least
    EXPAND_THRESHOLD_MS long, move start back by EXPAND_MS (but not past
    its own start).

    This handles cases where multiple silence regions are close together
    and the cue can benefit from additional leading room.
    """
    # Find the silence whose end is closest to (and <=) snapped_ms,
    # then look at the silence before it.
    prev_silence: tuple[int, int] | None = None
    best_gap = MAX_SNAP_MS

    for s_start, s_end in silences:
        if s_end <= snapped_ms:
            gap = snapped_ms - s_end
            if gap <= best_gap:
                prev_silence = (s_start, s_end)
                best_gap = gap

    # prev_silence is the gap we snapped into.  Now find the one before it.
    if prev_silence is not None:
        target: tuple[int, int] | None = None
        best_gap2 = MAX_SNAP_MS
        for s_start, s_end in silences:
            if s_end <= prev_silence[0]:  # ends before prev silence starts
                gap = prev_silence[0] - s_end
                if gap <= best_gap2:
                    target = (s_start, s_end)
                    best_gap2 = gap

        if target is not None:
            s_start, s_end = target
            silence_len = s_end - s_start
            if silence_len >= EXPAND_THRESHOLD_MS:
                return max(s_start, snapped_ms - EXPAND_MS)

    return snapped_ms


def expand_end(snapped_ms: int, silences: list[tuple[int, int]]) -> int:
    """
    After snapping to the outer edge of a silence region, try to move
    end even later into the *next* silence region (the one after the gap
    we already snapped into).  If that silence region is at least
    EXPAND_THRESHOLD_MS long, move end forward by EXPAND_MS (but not past
    its own end).

    This handles cases where multiple silence regions are close together
    and the cue can benefit from additional trailing room.
    """
    # Find the silence whose start is closest to (and >=) snapped_ms,
    # then look at the silence after it.
    next_silence: tuple[int, int] | None = None
    best_gap = MAX_SNAP_MS

    for s_start, s_end in silences:
        if s_start >= snapped_ms:
            gap = s_start - snapped_ms
            if gap <= best_gap:
                next_silence = (s_start, s_end)
                best_gap = gap

    # next_silence is the gap we snapped into.  Now find the one after it.
    if next_silence is not None:
        target: tuple[int, int] | None = None
        best_gap2 = MAX_SNAP_MS
        for s_start, s_end in silences:
            if s_start >= next_silence[1]:  # starts after next silence ends
                gap = s_start - next_silence[1]
                if gap <= best_gap2:
                    target = (s_start, s_end)
                    best_gap2 = gap

        if target is not None:
            s_start, s_end = target
            silence_len = s_end - s_start
            if silence_len >= EXPAND_THRESHOLD_MS:
                return min(s_end, snapped_ms + EXPAND_MS)

    return snapped_ms


def adjust_cues(
    cues: list[Cue],
    silences: list[tuple[int, int]],
) -> list[tuple[Cue, int, int]]:
    """
    Adjust all cue boundaries.  Returns list of (cue, new_start, new_end).
    Ensures adjusted cues don't overlap.
    """
    adjusted: list[tuple[Cue, int, int]] = []

    for cue in cues:
        # Step 1: snap to nearest silence boundary
        new_start = snap_start(cue.start_ms, silences)
        new_end = snap_end(cue.end_ms, silences)

        # Step 2: expand outward when enough silence is available
        new_start = expand_start(new_start, silences)
        new_end = expand_end(new_end, silences)

        # Ensure cue remains at least 50ms after all adjustments
        if new_end - new_start < 50:
            new_start = cue.start_ms
            new_end = cue.end_ms

        # Prevent overlap with previous cue
        if adjusted:
            _, _, prev_end = adjusted[-1]
            if new_start < prev_end:
                new_start = prev_end

        # Ensure start < end again after overlap fix
        if new_start >= new_end:
            new_start = cue.start_ms
            new_end = cue.end_ms

        # Final overlap check — if still overlapping, warn
        if adjusted:
            _, _, prev_end = adjusted[-1]
            if new_start < prev_end:
                print(
                    f"  Warning: cue #{cue.order_num} still overlaps after "
                    f"adjustment ({new_start}ms < prev end {prev_end}ms)"
                )

        adjusted.append((cue, new_start, new_end))

    return adjusted


# ============================================================
# Database operations
# ============================================================

def load_subtitle(conn: sqlite3.Connection, subtitle_uuid: str):
    """Load subtitle metadata. Returns (uuid, media_uuid, name, note) or None."""
    cur = conn.cursor()
    cur.execute(
        "SELECT uuid, media_uuid, name, note FROM listen_subtitle WHERE uuid=?",
        (subtitle_uuid,),
    )
    return cur.fetchone()


def load_media_source(conn: sqlite3.Connection, media_uuid: str) -> str | None:
    """Get the source (file path) for a media entry."""
    cur = conn.cursor()
    cur.execute(
        "SELECT source FROM listen_media WHERE uuid=?",
        (media_uuid,),
    )
    row = cur.fetchone()
    return row[0] if row else None


def load_cues(conn: sqlite3.Connection, subtitle_uuid: str) -> list[Cue]:
    """Load all cues for a subtitle, ordered by order_num."""
    cur = conn.cursor()
    cur.execute(
        """
        SELECT uuid, order_num, start_ms, end_ms, content, reference
        FROM listen_subtitle_cue
        WHERE subtitle_uuid=?
        ORDER BY order_num
        """,
        (subtitle_uuid,),
    )
    return [Cue(*row) for row in cur.fetchall()]


def create_adjusted_subtitle(
    conn: sqlite3.Connection,
    original_subtitle: tuple,
) -> str:
    """
    Create a new listen_subtitle row based on the original, with a note
    indicating waveform adjustment.  Returns the new subtitle UUID.
    """
    _, user_id, media_uuid, name, _note, _created, _updated = original_subtitle
    new_uuid = str(uuid.uuid4())
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"

    conn.execute(
        """
        INSERT INTO listen_subtitle
            (uuid, user_id, media_uuid, name, note, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        (
            new_uuid,
            user_id,
            media_uuid,
            name,
            "adjusted using waveform",
            now,
            now,
        ),
    )
    return new_uuid


def insert_adjusted_cues(
    conn: sqlite3.Connection,
    new_subtitle_uuid: str,
    adjusted: list[tuple[Cue, int, int]],
) -> int:
    """
    Insert adjusted cues into listen_subtitle_cue with new UUIDs.
    Returns the number of cues inserted.
    """
    rows = []
    for cue, new_start, new_end in adjusted:
        cue_uuid = str(uuid.uuid4())
        rows.append((
            cue_uuid,
            new_subtitle_uuid,
            cue.order_num,
            new_start,
            new_end,
            cue.content,
            cue.reference,
        ))

    conn.executemany(
        """
        INSERT INTO listen_subtitle_cue
            (uuid, subtitle_uuid, order_num, start_ms, end_ms, content, reference)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        rows,
    )
    return len(rows)


# ============================================================
# Main
# ============================================================

def process(db_path: str, subtitle_uuid: str, waveform_dir: str) -> None:
    db_path_obj = Path(db_path)
    waveform_dir_obj = Path(waveform_dir)

    if not db_path_obj.exists():
        print(f"Error: database not found: {db_path}")
        return
    if not waveform_dir_obj.exists():
        print(f"Error: waveform directory not found: {waveform_dir}")
        return

    conn = sqlite3.connect(db_path)
    try:
        # 1. Load subtitle and media info
        subtitle = load_subtitle(conn, subtitle_uuid)
        if subtitle is None:
            print(f"Error: subtitle not found: {subtitle_uuid}")
            return

        media_uuid = subtitle[1]
        media_source = load_media_source(conn, media_uuid)
        if media_source is None:
            print(f"Error: media not found for uuid: {media_uuid}")
            return

        # 2. Load waveform
        wf = load_waveform(waveform_dir_obj, media_source)
        if wf is None:
            print(f"Error: waveform JSON not found for: {media_source}")
            return
        data, ms_per_pixel = wf

        # 3. Compute energy and find silence regions
        energy = compute_energy(data)
        silences = find_silence_regions(energy, ms_per_pixel)
        total_ms = len(energy) * ms_per_pixel
        silence_total = sum(e - s for s, e in silences)
        print(
            f"Waveform: {len(energy)} frames, "
            f"{total_ms:.0f}ms total, "
            f"{len(silences)} silence regions ({silence_total:.0f}ms)"
        )

        # 4. Load cues
        cues = load_cues(conn, subtitle_uuid)
        if not cues:
            print("No cues found for this subtitle.")
            return

        # 5. Adjust cue boundaries
        adjusted = adjust_cues(cues, silences)

        # 6. Create new subtitle and insert adjusted cues
        new_uuid = create_adjusted_subtitle(conn, subtitle)
        count = insert_adjusted_cues(conn, new_uuid, adjusted)
        conn.commit()

        # 7. Report
        shifted = sum(
            1 for cue, ns, ne in adjusted
            if ns != cue.start_ms or ne != cue.end_ms
        )
        print(f"Created subtitle {new_uuid[:8]}... with {count} cues ({shifted} adjusted)")

    finally:
        conn.close()


if __name__ == "__main__":
    if len(sys.argv) < 4:
        print("Usage: python adjust_cue_time.py <db_path> <subtitle_uuid> <waveform_dir>")
        sys.exit(1)

    process(sys.argv[1], sys.argv[2], sys.argv[3])
