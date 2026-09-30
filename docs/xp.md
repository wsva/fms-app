# XP System — MVP Design

## Overview

XP (Experience Points) represents permanent learning progress. Users earn XP through meaningful learning activities. XP cannot be spent, traded, or lost.

## Core Principles

- **Non-spendable**: XP cannot be used to purchase anything
- **Non-transferable**: Users cannot buy, sell, or gift XP
- **Permanent**: Earned XP is never removed
- **Non-purchasable**: Real money cannot buy XP

## MVP XP Rewards

### Dictation XP

| Action | XP | Rationale |
|--------|---:|-----------|
| Complete a cue dictation | 1 | Baseline reward for effort |
| Complete a subtitle (all cues done) | +2 bonus | Encourage finishing units |
| Complete a media file (all subtitles) | +5 bonus | Bigger milestone |

**Notes:**
- XP is awarded when the user successfully completes a dictation attempt
- A "completion" means the user has typed the correct text (or close enough to pass)
- Bonuses are awarded once per subtitle/media (not repeatable)
- Repeating the same cue does not earn additional XP

### Reading XP

| Action | XP | Rationale |
|--------|---:|-----------|
| Type a sentence correctly | 1 | Baseline reward |
| Complete a chapter (all sentences) | +5 bonus | Encourage finishing sections |

**Notes:**
- A sentence is "complete" when the user has typed it correctly
- Chapter bonus is awarded once per chapter

### Future XP Sources (Not in MVP)

These are planned for later versions:

- **Session XP**: Bonus for sustained practice (5/10/20 minute sessions)
- **Retention XP**: Bonus for remembering material after 1/7/30 days
- **Achievement XP**: One-time bonuses for milestones (First Steps, Word Collector, etc.)
- **Accuracy bonuses**: Extra XP for first-attempt correctness

## Level Progression

Use a quadratic XP curve:

```
XP_required(L) = 100 × (L - 1)²
Level = floor(sqrt(LifetimeXP / 100)) + 1
```

| Level | Total XP Required |
|------:|------------------:|
| 1 | 0 |
| 2 | 100 |
| 3 | 400 |
| 4 | 900 |
| 5 | 1,600 |
| 10 | 8,100 |
| 20 | 36,100 |
| 50 | 240,100 |

## Data Model

### App-level SQLite Tables

```sql
-- User XP summary (cached values)
CREATE TABLE xp_user (
    user_id TEXT PRIMARY KEY,
    lifetime_xp INTEGER NOT NULL DEFAULT 0,
    level INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);

-- XP transaction log (append-only)
CREATE TABLE xp_ledger (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    amount INTEGER NOT NULL,
    source TEXT NOT NULL,           -- 'dictation_cue', 'dictation_subtitle', 'dictation_media', 'reading_sentence', 'reading_chapter'
    reference_id TEXT,              -- cue_id, subtitle_id, media_id, sentence_id, chapter_id
    dataset_uuid TEXT,              -- which dataset earned this XP
    created_at TEXT NOT NULL
);

-- Track what has already earned XP (prevent duplicates)
CREATE TABLE xp_earned (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    source TEXT NOT NULL,
    reference_id TEXT NOT NULL,     -- the specific item that earned XP
    dataset_uuid TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(user_id, source, reference_id, dataset_uuid)
);
```

### XP Sources

| Source | Description | Reference ID |
|--------|-------------|--------------|
| `dictation_cue` | Completed a single cue dictation | cue_id |
| `dictation_subtitle` | Completed all cues in a subtitle | subtitle_id |
| `dictation_media` | Completed all subtitles for a media file | media_id |
| `reading_sentence` | Typed a sentence correctly | sentence_id |
| `reading_chapter` | Completed all sentences in a chapter | chapter_id |

## Implementation Plan

### Phase 1: Backend (Rust)

1. Create XP tables in app-level SQLite
2. Add Tauri commands:
   - `xp_get_user` — get current XP and level
   - `xp_award` — award XP for an action (internal use)
   - `xp_get_history` — get recent XP transactions
3. Hook into existing completion events:
   - Dictation cue save → award 1 XP
   - Dictation subtitle completion → award +2 XP
   - Dictation media completion → award +5 XP
   - Reading sentence save → award 1 XP
   - Reading chapter completion → award +5 XP

### Phase 2: Frontend (React)

1. Display XP in sidebar (next to user avatar)
2. Show level badge
3. Add XP earned animation/toast after completing something
4. Optional: XP history page in settings

### Phase 3: Future Enhancements

- Session tracking (time-based XP)
- Retention bonuses (spaced repetition)
- Achievement system
- Accuracy bonuses
- XP leaderboard (if multi-user)

## Prevention of XP Farming

1. **Unique constraint**: Each (user_id, source, reference_id, dataset_uuid) can only earn XP once
2. **Server-side validation**: XP is awarded by the backend, not the client
3. **No duplicate awards**: Repeating the same cue/subtitle does not earn more XP
4. **Audit trail**: All XP transactions are logged in xp_ledger
