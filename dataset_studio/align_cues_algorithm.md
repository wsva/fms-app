# Alignment Algorithm

## Overview

A **multi-pass anchor-based alignment** algorithm that first establishes high-confidence anchor points, then fills gaps between anchors using constrained DP alignment.

This approach is:
- **Interpretable** — you can see which matches are "certain" vs "inferred"
- **Robust** — exact matches anchor the alignment, reducing error propagation
- **Efficient** — gaps are smaller subproblems, faster than full DP
- **Handles splits/merges** — if STT splits one sentence into multiple cues, the anchors constrain the search

---

## Input

- **Cues**: Subtitle segments from STT (e.g., "hello world", "how are you")
- **Refs**: Reference sentences from book.txt or transcript (e.g., "Hello, world!", "How are you doing?")

---

## Pass 1: Exact Matches (Anchors) with Sliding Window

```
For window_size in [1, 2, 3, 4]:
    For each consecutive group of window_size unmatched cues:
        combined_text = " ".join(cue[i:i+window_size])
        If len(combined_text) < 20:
            continue  # too short for reliable anchor
        For each consecutive group of refs:
            If similarity(combined_text, ref_combined) == 100%:
                Mark all cues in group as fixed anchor pairs
```

Short cues like "yes" or "ok" are unreliable as anchors. By combining consecutive cues with a sliding window, we get longer text that produces more reliable exact matches. Minimum 20 characters required for anchor reliability.

---

## Pass 2: Threshold Descent with Sliding Window

```
For threshold in [90%, 80%, 70%, 60%]:
    For window_size in [1, 2, 3, 4]:
        For each consecutive group of window_size unmatched cues:
            combined_text = " ".join(cue[i:i+window_size])
            If len(combined_text) < 20:
                continue
            For each consecutive group of unmatched refs:
                If similarity(combined_text, ref_combined) >= threshold:
                    Mark as fixed anchor pair
```

Same sliding window approach as Pass 1, but with decreasing similarity thresholds. Each threshold level adds more anchors.

---

## Pass 3: Gap Filling

```
Identify gaps between consecutive anchors:
    gap = (anchor_i, anchor_j) where j > i+1

For each gap:
    Extract cues between anchor_i and anchor_j
    Extract refs between anchor_i and anchor_j
    Run DP alignment on this subset
    Mark all matches in gap as "inferred" (not anchors)
```

Gaps are filled using DP alignment on the constrained subset. This handles cases where cues and refs don't align 1-to-1 within the gap.

---

## Pass 4: Iterate

```
Repeat Pass 2-3 until:
    - No new matches found, OR
    - All cues matched
```

After gap filling, new anchors may have been established. Repeat the process until convergence.

---

## Final State

| Match Type | Description | Reference |
|------------|-------------|-----------|
| **Anchor** | High-confidence match (≥60% similarity) | `match_type="anchor"` |
| **Inferred** | Gap-filled match (lower confidence) | `match_type="inferred"` |
| **Unmatched** | No suitable ref found | `reference=NULL` |

---

## Similarity Scoring

For each cue↔ref pair, compute a similarity score (0–100):

| Score | Condition |
|-------|-----------|
| 100 pts | Exact match (if text > 20 chars) |
| 90 pts | One text is substring of the other |
| 0–80 pts | SequenceMatcher ratio × 80 |

Text is normalized first (lowercase, collapse whitespace).

---

## DP Alignment (for Gap Filling)

For each gap between anchors, build a DP matrix and align:

| Move | Meaning | Score |
|------|---------|-------|
| **Diagonal** | Match cue[i] with ref[j] | `dp[i-1][j-1] + similarity(cue[i], ref[j])` |
| **Up** | Skip cue[i] (no match) | `dp[i-1][j] - GAP_PENALTY` |
| **Left** | Skip ref[j] (no match) | `dp[i][j-1] - GAP_PENALTY` |

`GAP_PENALTY = 30` — skipping a cue or ref costs 30 points.

Backtrack from `dp[n][m]` to `dp[0][0]`, recording matches where similarity ≥ `MIN_MATCH_SCORE (60)`.

---

## Visual Example

```
Initial:
Cues:  [A]  [B]  [C]  [D]  [E]  [F]
Refs:  [1]  [2]  [3]  [4]  [5]  [6]

Pass 1 (exact, 100%):
Cues:  [A]  [B]  [C]  [D]  [E]  [F]
        ✓              ✓
Refs:  [1]  [2]  [3]  [4]  [5]  [6]
        ✓              ✓
Anchors: A↔1, D↔4

Pass 2 (threshold 90%):
Cues:  [A]  [B]  [C]  [D]  [E]  [F]
        ✓   ✓        ✓   ✓
Refs:  [1]  [2]  [3]  [4]  [5]  [6]
        ✓   ✓        ✓   ✓
Anchors: A↔1, B↔2, D↔4, E↔5

Pass 3 (gap filling):
Gap (B↔2, D↔4): cues=[C], refs=[3] → C↔3 (inferred)
Gap (E↔5, end): cues=[F], refs=[6] → F↔6 (inferred)

Final:
Cues:  [A]  [B]  [C]  [D]  [E]  [F]
        ✓   ✓   ○   ✓   ✓   ○
Refs:  [1]  [2]  [3]  [4]  [5]  [6]
        ✓   ✓   ○   ✓   ✓   ○

✓ = anchor, ○ = inferred
```

---

## Key Parameters

| Parameter | Value | Effect |
|-----------|-------|--------|
| `GAP_PENALTY` | 30 | Higher = stricter alignment, fewer gaps |
| `MIN_MATCH_SCORE` | 60 | Higher = only accept very similar matches |
| `MIN_ANCHOR_LENGTH` | 20 | Minimum chars for reliable anchor |
| `MAX_WINDOW_SIZE` | 4 | Max cues to combine for anchor discovery |
| `ANCHOR_THRESHOLDS` | [100, 90, 80, 70, 60] | Anchor discovery order |

---

## Data Structures

```python
@dataclass
class Match:
    cue: Cue
    ref: Ref
    score: float
    match_type: str  # "anchor" or "inferred"
```

---

## Confidence Scores

Store if needed:
- **Anchor matches**: `match_type="anchor"`, score = actual similarity
- **Inferred matches**: `match_type="inferred"`, score = DP alignment score
- **Unmatched**: `reference=NULL`

---

## Advantages Over Pure DP

1. **Anchors prevent error propagation** — exact matches define fixed points
2. **Interpretable results** — you can see which matches are certain vs inferred
3. **Handles long documents** — gaps are small subproblems, not one giant DP matrix
4. **Robust to STT errors** — if STT splits/merges sentences, anchors constrain the search
