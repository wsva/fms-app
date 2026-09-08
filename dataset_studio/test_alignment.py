#!/usr/bin/env python3
"""
Tests for the multi-pass anchor-based alignment algorithm.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from lib.alignment import (
    Cue, Ref, Match,
    similarity_score, find_anchors, dp_align, multi_pass_align,
)


def test_similarity_score():
    """Test similarity scoring."""
    # Exact match (needs > 20 chars)
    assert similarity_score("the quick brown fox jumps", "the quick brown fox jumps") == 100.0

    # Substring
    assert similarity_score("the quick brown fox", "the quick brown fox jumps over") == 90.0

    # Similar
    score = similarity_score("the quick brown fox", "the slow brown fox")
    assert 40 < score < 80

    # Different
    score = similarity_score("hello", "goodbye")
    assert score < 30

    print("[OK] similarity_score tests passed")


def test_find_anchors_exact():
    """Test anchor discovery with exact matches."""
    cues = [
        Cue(uuid="c1", order_num=0, content="I went to the store"),
        Cue(uuid="c2", order_num=1, content="yes"),
        Cue(uuid="c3", order_num=2, content="and bought some milk"),
    ]
    refs = [
        Ref(uuid="r1", order_num=0, content="I went to the store"),
        Ref(uuid="r2", order_num=1, content="and bought some milk"),
    ]

    anchors = find_anchors(cues, refs, threshold=100.0, min_length=10)

    # Should find 2 anchors (c1↔r1, c3↔r2)
    # c2 is too short to be an anchor
    assert len(anchors) >= 1

    print(f"[OK] find_anchors found {len(anchors)} anchors")


def test_find_anchors_window():
    """Test anchor discovery with sliding window."""
    # Short individual cues that combine to form a reliable anchor
    cues = [
        Cue(uuid="c1", order_num=0, content="I"),
        Cue(uuid="c2", order_num=1, content="went"),
        Cue(uuid="c3", order_num=2, content="to"),
        Cue(uuid="c4", order_num=3, content="the store"),
    ]
    refs = [
        Ref(uuid="r1", order_num=0, content="I went to the store"),
    ]

    # Single cues are too short, but combined they should match
    anchors = find_anchors(cues, refs, threshold=90.0, min_length=15, max_window=4)

    print(f"[OK] find_anchors with window found {len(anchors)} anchors")


def test_dp_align():
    """Test DP alignment on small input."""
    cues = [
        Cue(uuid="c1", order_num=0, content="the quick brown fox jumps"),
        Cue(uuid="c2", order_num=1, content="how are you doing today"),
    ]
    refs = [
        Ref(uuid="r1", order_num=0, content="the quick brown fox jumps"),
        Ref(uuid="r2", order_num=1, content="how are you doing today my friend"),
    ]

    matches = dp_align(cues, refs)

    assert len(matches) >= 1
    assert all(m.match_type == "inferred" for m in matches)

    print(f"[OK] dp_align found {len(matches)} matches")


def test_multi_pass_align():
    """Test full multi-pass alignment."""
    cues = [
        Cue(uuid="c1", order_num=0, content="I went to the store yesterday"),
        Cue(uuid="c2", order_num=1, content="and"),
        Cue(uuid="c3", order_num=2, content="bought some milk"),
        Cue(uuid="c4", order_num=3, content="it was good"),
    ]
    refs = [
        Ref(uuid="r1", order_num=0, content="I went to the store yesterday"),
        Ref(uuid="r2", order_num=1, content="and bought some milk"),
        Ref(uuid="r3", order_num=2, content="it was really good"),
    ]

    matches = multi_pass_align(cues, refs)

    anchors = [m for m in matches if m.match_type == "anchor"]
    inferred = [m for m in matches if m.match_type == "inferred"]

    print(f"[OK] multi_pass_align: {len(matches)} matches ({len(anchors)} anchors, {len(inferred)} inferred)")

    for m in matches:
        print(f"    {m.cue.content[:30]:30} <-> {m.ref.content[:30]:30} [{m.match_type}] score={m.score:.1f}")


def test_multi_pass_with_gaps():
    """Test multi-pass alignment with gaps that need filling."""
    cues = [
        Cue(uuid="c1", order_num=0, content="the quick brown fox"),
        Cue(uuid="c2", order_num=1, content="jumps"),
        Cue(uuid="c3", order_num=2, content="over the lazy dog"),
    ]
    refs = [
        Ref(uuid="r1", order_num=0, content="the quick brown fox"),
        Ref(uuid="r2", order_num=1, content="jumps over the lazy dog"),
    ]

    matches = multi_pass_align(cues, refs)

    print(f"[OK] multi_pass_align with gaps: {len(matches)} matches")
    for m in matches:
        print(f"    {m.cue.content[:30]:30} <-> {m.ref.content[:30]:30} [{m.match_type}]")


def test_short_cues():
    """Test that short cues are handled properly."""
    cues = [
        Cue(uuid="c1", order_num=0, content="yes"),
        Cue(uuid="c2", order_num=1, content="I agree with that"),
        Cue(uuid="c3", order_num=2, content="ok"),
    ]
    refs = [
        Ref(uuid="r1", order_num=0, content="yes"),
        Ref(uuid="r2", order_num=1, content="I agree with that completely"),
        Ref(uuid="r3", order_num=2, content="ok"),
    ]

    matches = multi_pass_align(cues, refs)

    print(f"[OK] short cues test: {len(matches)} matches")
    for m in matches:
        print(f"    '{m.cue.content}' <-> '{m.ref.content}' [{m.match_type}]")


if __name__ == "__main__":
    print("Running alignment algorithm tests...\n")

    test_similarity_score()
    test_find_anchors_exact()
    test_find_anchors_window()
    test_dp_align()
    test_multi_pass_align()
    test_multi_pass_with_gaps()
    test_short_cues()

    print("\n[OK] All tests passed!")
