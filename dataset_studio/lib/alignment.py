"""
Multi-pass anchor-based alignment engine for matching subtitle cues to reference text.

Algorithm:
1. Pass 1: Find exact matches using sliding window of combined cues
2. Pass 2: Threshold descent [90%, 80%, 70%, 60%] to find more anchors
3. Pass 3: Fill gaps between anchors using DP alignment on subsets
4. Pass 4: Iterate until convergence
"""

from dataclasses import dataclass, field
from difflib import SequenceMatcher

from lib.text import normalize


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

GAP_PENALTY = 30
MIN_MATCH_SCORE = 60
MIN_ANCHOR_LENGTH = 20  # Minimum chars for reliable anchor
MAX_WINDOW_SIZE = 4  # Max cues to combine for anchor discovery
ANCHOR_THRESHOLDS = [100, 90, 80, 70, 60]  # Threshold descent sequence


# ---------------------------------------------------------------------------
# Models
# ---------------------------------------------------------------------------

@dataclass
class Cue:
    uuid: str
    order_num: int
    content: str


@dataclass
class Ref:
    uuid: str
    order_num: int
    content: str


@dataclass
class Match:
    cue: Cue
    ref: Ref
    score: float
    match_type: str  # "anchor" or "inferred"


@dataclass
class AnchorGroup:
    """A group of cues matched to a group of refs as anchors."""
    cues: list[Cue] = field(default_factory=list)
    refs: list[Ref] = field(default_factory=list)
    score: float = 0.0


# ---------------------------------------------------------------------------
# Similarity
# ---------------------------------------------------------------------------

def similarity_score(text_a: str, text_b: str) -> float:
    """Return similarity score 0–100 between two text strings."""
    text_a = normalize(text_a)
    text_b = normalize(text_b)

    if not text_a or not text_b:
        return 0.0

    if len(text_a) > 20 and text_a == text_b:
        return 100.0

    if text_a in text_b or text_b in text_a:
        return 90.0

    ratio = SequenceMatcher(None, text_a, text_b).ratio()
    return ratio * 80.0


# ---------------------------------------------------------------------------
# Sliding window anchor discovery
# ---------------------------------------------------------------------------

def find_anchors(
    cues: list[Cue],
    refs: list[Ref],
    threshold: float = 100.0,
    max_window: int = MAX_WINDOW_SIZE,
    min_length: int = MIN_ANCHOR_LENGTH,
) -> list[AnchorGroup]:
    """
    Find anchor matches using sliding window of combined cues.

    For each window size [1, 2, 3, 4]:
        For each consecutive group of cues:
            Combine text, check if it matches any ref(s) above threshold
    """
    anchors = []
    used_cue_indices = set()
    used_ref_indices = set()

    for window_size in range(1, max_window + 1):
        for i in range(len(cues) - window_size + 1):
            # Skip if any cue in window is already used
            if any((i + k) in used_cue_indices for k in range(window_size)):
                continue

            cue_group = cues[i:i + window_size]
            combined_text = " ".join(c.content for c in cue_group)

            # Skip if too short for reliable matching
            if len(normalize(combined_text)) < min_length:
                continue

            # Try matching against single refs and ref groups
            for ref_window in range(1, max_window + 1):
                for j in range(len(refs) - ref_window + 1):
                    # Skip if any ref in window is already used
                    if any((j + k) in used_ref_indices for k in range(ref_window)):
                        continue

                    ref_group = refs[j:j + ref_window]
                    ref_combined = " ".join(r.content for r in ref_group)

                    score = similarity_score(combined_text, ref_combined)

                    if score >= threshold:
                        anchors.append(AnchorGroup(
                            cues=cue_group.copy(),
                            refs=ref_group.copy(),
                            score=score,
                        ))
                        # Mark as used
                        for k in range(window_size):
                            used_cue_indices.add(i + k)
                        for k in range(ref_window):
                            used_ref_indices.add(j + k)

    return anchors


# ---------------------------------------------------------------------------
# Gap filling with DP alignment
# ---------------------------------------------------------------------------

def dp_align(cues: list[Cue], refs: list[Ref]) -> list[Match]:
    """
    Align cues to refs using dynamic programming.
    Returns list of Match objects with match_type="inferred".
    """
    if not cues or not refs:
        return []

    n = len(cues)
    m = len(refs)

    dp = [[0] * (m + 1) for _ in range(n + 1)]
    back = [[None] * (m + 1) for _ in range(n + 1)]

    for i in range(1, n + 1):
        dp[i][0] = dp[i - 1][0] - GAP_PENALTY
        back[i][0] = ("UP",)

    for j in range(1, m + 1):
        dp[0][j] = dp[0][j - 1] - GAP_PENALTY
        back[0][j] = ("LEFT",)

    for i in range(1, n + 1):
        cue = cues[i - 1]
        for j in range(1, m + 1):
            ref = refs[j - 1]
            match_score = similarity_score(cue.content, ref.content)
            diag = dp[i - 1][j - 1] + match_score
            up = dp[i - 1][j] - GAP_PENALTY
            left = dp[i][j - 1] - GAP_PENALTY
            best = max(diag, up, left)
            dp[i][j] = best
            if best == diag:
                back[i][j] = ("MATCH", match_score)
            elif best == up:
                back[i][j] = ("UP",)
            else:
                back[i][j] = ("LEFT",)

    # Backtrack
    matches = []
    i, j = n, m
    while i > 0 and j > 0:
        step = back[i][j]
        if step[0] == "MATCH":
            score = step[1]
            if score >= MIN_MATCH_SCORE:
                matches.append(Match(
                    cue=cues[i - 1],
                    ref=refs[j - 1],
                    score=score,
                    match_type="inferred",
                ))
            i -= 1
            j -= 1
        elif step[0] == "UP":
            i -= 1
        else:
            j -= 1

    matches.reverse()
    return matches


# ---------------------------------------------------------------------------
# Multi-pass alignment
# ---------------------------------------------------------------------------

def multi_pass_align(
    cues: list[Cue],
    refs: list[Ref],
    thresholds: list[float] = None,
    max_window: int = MAX_WINDOW_SIZE,
    min_length: int = MIN_ANCHOR_LENGTH,
) -> list[Match]:
    """
    Multi-pass anchor-based alignment.

    1. Find exact matches (100%) using sliding window
    2. Threshold descent [90%, 80%, 70%, 60%] for more anchors
    3. Fill gaps between anchors using DP alignment
    4. Iterate until convergence
    """
    if thresholds is None:
        thresholds = ANCHOR_THRESHOLDS

    all_matches: list[Match] = []
    matched_cue_uuids = set()
    matched_ref_uuids = set()

    # Pass 1 & 2: Find anchors at each threshold level
    for threshold in thresholds:
        # Get unmatched cues and refs
        unmatched_cues = [c for c in cues if c.uuid not in matched_cue_uuids]
        unmatched_refs = [r for r in refs if r.uuid not in matched_ref_uuids]

        if not unmatched_cues or not unmatched_refs:
            break

        # Find anchors at this threshold
        new_anchors = find_anchors(
            unmatched_cues,
            unmatched_refs,
            threshold=threshold,
            max_window=max_window,
            min_length=min_length,
        )

        # Convert anchors to matches
        for anchor in new_anchors:
            # For simplicity, create 1-to-1 matches within the anchor group
            # This is a simplification; real multi-to-multi mapping is more complex
            for cue, ref in zip(anchor.cues, anchor.refs):
                if cue.uuid not in matched_cue_uuids and ref.uuid not in matched_ref_uuids:
                    all_matches.append(Match(
                        cue=cue,
                        ref=ref,
                        score=anchor.score,
                        match_type="anchor",
                    ))
                    matched_cue_uuids.add(cue.uuid)
                    matched_ref_uuids.add(ref.uuid)

    # Pass 3: Fill gaps between anchors
    # Sort anchors by position to identify gaps
    anchor_matches = sorted(
        [m for m in all_matches if m.match_type == "anchor"],
        key=lambda m: m.cue.order_num
    )

    # Find gaps and fill them
    prev_cue_idx = -1
    prev_ref_idx = -1

    for anchor in anchor_matches:
        # Get cues and refs between previous anchor and this one
        gap_cues = [
            c for c in cues
            if prev_cue_idx < c.order_num < anchor.cue.order_num
            and c.uuid not in matched_cue_uuids
        ]
        gap_refs = [
            r for r in refs
            if prev_ref_idx < r.order_num < anchor.ref.order_num
            and r.uuid not in matched_ref_uuids
        ]

        if gap_cues and gap_refs:
            # DP alignment on this gap
            gap_matches = dp_align(gap_cues, gap_refs)
            all_matches.extend(gap_matches)
            for m in gap_matches:
                matched_cue_uuids.add(m.cue.uuid)
                matched_ref_uuids.add(m.ref.uuid)

        prev_cue_idx = anchor.cue.order_num
        prev_ref_idx = anchor.ref.order_num

    # Final gap: after last anchor to end
    remaining_cues = [c for c in cues if c.uuid not in matched_cue_uuids]
    remaining_refs = [r for r in refs if r.uuid not in matched_ref_uuids]

    if remaining_cues and remaining_refs:
        final_matches = dp_align(remaining_cues, remaining_refs)
        all_matches.extend(final_matches)

    return all_matches


# ---------------------------------------------------------------------------
# Legacy API (for backward compatibility)
# ---------------------------------------------------------------------------

def align(cues: list[Cue], refs: list[Ref]) -> list[tuple[Cue, Ref, float]]:
    """
    Legacy API: Align cues to refs using dynamic programming.
    Returns list of (cue, ref, score) tuples.

    For new code, prefer multi_pass_align() which returns Match objects.
    """
    matches = dp_align(cues, refs)
    return [(m.cue, m.ref, m.score) for m in matches]
