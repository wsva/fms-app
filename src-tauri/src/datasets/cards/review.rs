//! Per-user card review state, stored in the **app** DB (sync-v2.md §17).
//!
//! A rating is an *event*, not a mutation: one row per review, carrying the SM-2
//! values it produced. That shape is what makes replay safe. The old design
//! journaled `{card_uuid, quality}` — an instruction to *compute* — so both hosts
//! replayed it by calling `card_test_submit` again, and SM-2 folds onto whatever
//! state that node already held. Applying one review twice compounded its
//! interval (1→3→8), doubled its familiarity jump and penalised the ease factor
//! twice; two nodes with different histories reached different schedules from the
//! same journal, which the drift check then tried to "heal" with a snapshot that
//! overwrote the follower's own practice state. Storing the outcome turns replay
//! into `INSERT OR IGNORE` and convergence into set equality (INV-23).
//!
//! Three consequences worth knowing before editing this file:
//! * Rows are keyed by `user_id`, so two people practising one dataset no longer
//!   fight over the single shared `card_review` row the old table forced.
//! * Nothing here lives in the dataset directory, so practising a read-only
//!   shared dataset needs no overlay and never dirties a published hash (INV-9) —
//!   and a rating no longer writes `card.familiarity`/`card.updated_at`, which
//!   used to ship practice state to the website as a *content* edit.
//! * The dataset DB's legacy `card_review` table is read once to seed events and
//!   never written again. It stays in the schema so an older build still finds
//!   its own state after a downgrade.

use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use uuid::Uuid;

use super::CardReview;
use crate::datasets::{now_row_stamp, row_stamp_at};
use crate::settings::SettingsState;

/// The acting user's newest review, joined onto a card row aliased `c`. Expects
/// `?1` to be the acting `user_id` — every query using it binds that first.
///
/// A `LEFT JOIN`, because "this user has never rated it" is a state the queue
/// has to see (it is what makes a card `fresh` for *me* even when the dataset shipped
/// someone else's reviews).
pub(crate) const LATEST_JOIN: &str =
    "LEFT JOIN app.card_review_latest cr ON cr.card_uuid = c.uuid AND cr.user_id = ?1";

/// Familiarity as the acting user's, falling back to the value the dataset shipped.
/// Imports and online exports legitimately carry `card.familiarity`, so it stays
/// the seed; after the first rating of my own it stops mattering.
pub(crate) const FAMILIARITY: &str = "COALESCE(cr.familiarity, c.familiarity)";

/// A card I have never rated. `cr.uuid` is the event uuid, so it is NULL exactly
/// when no event exists for (me, card).
pub(crate) const UNRATED: &str = "cr.uuid IS NULL";

/// The three SM-2 inputs a new rating folds onto.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sm2State {
    pub repetitions: i32,
    pub interval_days: i32,
    pub ease_factor: i32,
}

/// One review event: a rating plus the state it produced.
#[derive(Clone, Debug)]
pub(crate) struct ReviewEvent {
    pub uuid: String,
    pub user_id: String,
    pub dataset_uuid: String,
    pub card_uuid: String,
    /// `None` for a row seeded from the legacy table, which never recorded its
    /// rating — the *state* is what migration carries forward, not the input.
    pub quality: Option<i32>,
    pub reviewed_at: String,
    pub familiarity: i32,
    pub interval_days: i32,
    pub ease_factor: i32,
    pub repetitions: i32,
    pub next_review_at: Option<String>,
}

/// A card DB with the app DB attached, plus the identity whose reviews the
/// queries see. Dropping this drops the attachment.
pub(crate) struct ReviewCtx {
    pub conn: Connection,
    pub me: String,
}

/// Create the event table, the newest-per-card view and the migration marker.
/// Idempotent, and safe to call on any connection to the app DB.
fn ensure_review_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS card_review_event (
            uuid           TEXT PRIMARY KEY,
            user_id        TEXT NOT NULL,
            dataset_uuid   TEXT NOT NULL,
            card_uuid      TEXT NOT NULL,
            quality        INTEGER,
            reviewed_at    TEXT NOT NULL,
            familiarity    INTEGER NOT NULL DEFAULT 0,
            interval_days  INTEGER NOT NULL DEFAULT 0,
            ease_factor    INTEGER NOT NULL DEFAULT 250,
            repetitions    INTEGER NOT NULL DEFAULT 0,
            next_review_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_cre_owner_card
            ON card_review_event(user_id, dataset_uuid, card_uuid, reviewed_at);

        /* The newest event per (user, dataset, card) — the user's current schedule.
           SQLite's bare-column rule takes the other columns from the row that
           achieves the MAX, and the max is over `reviewed_at || '#' || uuid`
           rather than `reviewed_at` alone on purpose: two ratings stamped in the
           same second (offline edits merged later) would otherwise pick a winner
           per node, and the whole point of this table is that every node agrees.
           Stamps are one fixed-width UTC shape (`datasets::now_row_stamp`), so
           text order here is instant order. */
        CREATE VIEW IF NOT EXISTS card_review_latest AS
            SELECT user_id, dataset_uuid, card_uuid, quality, reviewed_at,
                   familiarity, interval_days, ease_factor, repetitions,
                   next_review_at, uuid,
                   MAX(reviewed_at || '#' || uuid) AS latest_key
            FROM card_review_event
            GROUP BY user_id, dataset_uuid, card_uuid;

        /* One row per (dataset, user) already folded out of the legacy table, so
           the seed costs one indexed lookup per open instead of a scan. */
        CREATE TABLE IF NOT EXISTS card_review_migrated (
            dataset_uuid TEXT NOT NULL,
            user_id      TEXT NOT NULL,
            migrated_at  TEXT NOT NULL,
            PRIMARY KEY (dataset_uuid, user_id)
        );",
    )
    .map_err(|e| e.to_string())
}

/// Open a dataset for practice: its own DB for content, the app DB attached for my
/// review state, both under one connection so the queue query stays in SQL.
///
/// Takes the **read** resolved directory on purpose — reviewing is not editing, so
/// it must not fork a read-only base into an overlay just to record a rating.
pub(crate) fn open(
    settings: &SettingsState,
    dataset_dir: &Path,
    dataset_uuid: &str,
) -> Result<ReviewCtx, String> {
    let me = crate::auth::workspace_identity(settings);
    let app_path = crate::datasets::dictation::app_db_path(settings);
    // Open it once for real: this creates the file and its tables, so the ATTACH
    // below has something to bind to.
    {
        let app_conn = crate::datasets::dictation::open_app_db(settings)?;
        ensure_review_schema(&app_conn)?;
    }

    let conn = super::open_card_db(dataset_dir)?;
    // Two connections to one file from here on. A review write is short, but give
    // the other writer a chance to finish rather than surface SQLITE_BUSY as a
    // sync or practice failure.
    let _ = conn.busy_timeout(Duration::from_secs(5));
    conn.execute("ATTACH DATABASE ?1 AS app", params![app_path.to_string_lossy().as_ref()])
        .map_err(|e| format!("attaching app DB '{}': {e}", app_path.display()))?;

    let ctx = ReviewCtx { conn, me };
    migrate_legacy(&ctx.conn, &ctx.me, dataset_uuid)?;
    Ok(ctx)
}

/// Fold this dataset's legacy `card_review` rows into events for the acting user,
/// once per (dataset, user).
///
/// Seeding is what keeps an upgrading user's schedule. It is also, for a dataset
/// downloaded from someone else, a way to inherit *their* schedule — which is the
/// same leak that publishing should strip (sync-v2.md §17.5). The seed is
/// idempotent because the legacy review row's uuid is stable per card, so the
/// `NOT EXISTS` on it makes a second pass a no-op.
fn migrate_legacy(conn: &Connection, me: &str, dataset_uuid: &str) -> Result<(), String> {
    let seeded: bool = conn
        .query_row(
            "SELECT 1 FROM app.card_review_migrated WHERE dataset_uuid = ?1 AND user_id = ?2",
            params![dataset_uuid, me],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if seeded {
        return Ok(());
    }
    let now = now_row_stamp();
    conn.execute(
        "INSERT INTO app.card_review_event
            (uuid, user_id, dataset_uuid, card_uuid, quality, reviewed_at,
             familiarity, interval_days, ease_factor, repetitions, next_review_at)
         SELECT cr.uuid, ?1, ?2, cr.card_uuid, NULL,
                COALESCE(cr.last_review_at, ?3),
                cr.familiarity, cr.interval_days, cr.ease_factor, cr.repetitions,
                cr.next_review_at
         FROM card_review cr
         WHERE NOT EXISTS (
             SELECT 1 FROM app.card_review_event e WHERE e.uuid = cr.uuid)",
        params![me, dataset_uuid, now],
    )
    .map_err(|e| format!("seeding review events: {e}"))?;
    conn.execute(
        "INSERT OR REPLACE INTO app.card_review_migrated (dataset_uuid, user_id, migrated_at) VALUES (?1, ?2, ?3)",
        params![dataset_uuid, me, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The state a rating folds onto: my newest event, or SM-2's starting point.
pub(crate) fn previous_state(conn: &Connection, me: &str, card_uuid: &str) -> Result<Sm2State, String> {
    Ok(latest(conn, me, card_uuid)?
        .map(|r| Sm2State {
            repetitions: r.repetitions,
            interval_days: r.interval_days,
            ease_factor: r.ease_factor,
        })
        .unwrap_or(Sm2State {
            repetitions: 0,
            interval_days: 0,
            // 2.5 in SM-2's own units; the table stores it x100.
            ease_factor: 250,
        }))
}

/// My current schedule for one card, or `None` if I have never rated it.
pub(crate) fn latest(conn: &Connection, me: &str, card_uuid: &str) -> Result<Option<CardReview>, String> {
    let row = conn.query_row(
        "SELECT uuid, card_uuid, familiarity, interval_days, ease_factor, repetitions, \
                reviewed_at, next_review_at \
         FROM app.card_review_latest WHERE user_id = ?1 AND card_uuid = ?2",
        params![me, card_uuid],
        |row| {
            Ok(CardReview {
                uuid: row.get(0)?,
                card_uuid: row.get(1)?,
                familiarity: row.get(2)?,
                interval_days: row.get(3)?,
                ease_factor: row.get(4)?,
                repetitions: row.get(5)?,
                last_review_at: row.get(6)?,
                next_review_at: row.get(7)?,
            })
        },
    );
    match row {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Record one rating as an event. Nothing outside the app DB is written.
///
/// The event uuid is minted by the device that *took* the rating and never
/// changes, which is what lets every other node absorb it with `INSERT OR
/// IGNORE` — including the device that authored it, when the authority echoes it
/// back in the same round.
pub(crate) fn append_event(conn: &Connection, ev: &ReviewEvent) -> Result<bool, String> {
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO app.card_review_event
                (uuid, user_id, dataset_uuid, card_uuid, quality, reviewed_at,
                 familiarity, interval_days, ease_factor, repetitions, next_review_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                ev.uuid,
                ev.user_id,
                ev.dataset_uuid,
                ev.card_uuid,
                ev.quality,
                ev.reviewed_at,
                ev.familiarity,
                ev.interval_days,
                ev.ease_factor,
                ev.repetitions,
                ev.next_review_at,
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

/// Take a rating: fold SM-2 onto this user's newest state and store the result.
/// Shared by the local command and the legacy-replay fallback so the arithmetic
/// lives in exactly one place.
pub(crate) fn record_rating(
    conn: &Connection,
    me: &str,
    dataset_uuid: &str,
    card_uuid: &str,
    quality: i32,
) -> Result<ReviewEvent, String> {
    let prev = previous_state(conn, me, card_uuid)?;
    let (interval_days, ease_factor, repetitions, familiarity) =
        super::sm2_next(quality, prev.repetitions, prev.interval_days, prev.ease_factor);
    let reviewed_at = now_row_stamp();
    let next_review_at = row_stamp_at(Utc::now() + chrono::Duration::days(interval_days as i64));
    let ev = ReviewEvent {
        uuid: Uuid::new_v4().to_string(),
        user_id: me.to_string(),
        dataset_uuid: dataset_uuid.to_string(),
        card_uuid: card_uuid.to_string(),
        quality: Some(quality),
        reviewed_at,
        familiarity,
        interval_days,
        ease_factor,
        repetitions,
        next_review_at: Some(next_review_at),
    };
    append_event(conn, &ev)?;
    Ok(ev)
}

/// The journal payload for one event.
///
/// A **superset** of the old `{card_uuid, quality}` on purpose: a hub that
/// predates this shape replays `card_review` by calling `card_test_submit` with
/// those two fields, so keeping them means an un-upgraded authority still
/// converges on its own (older) semantics instead of rejecting the change.
pub(crate) fn payload(ev: &ReviewEvent) -> Value {
    json!({
        "event_uuid": ev.uuid,
        "user_id": ev.user_id,
        "dataset_uuid": ev.dataset_uuid,
        "card_uuid": ev.card_uuid,
        "quality": ev.quality,
        "reviewed_at": ev.reviewed_at,
        "familiarity": ev.familiarity,
        "interval_days": ev.interval_days,
        "ease_factor": ev.ease_factor,
        "repetitions": ev.repetitions,
        "next_review_at": ev.next_review_at,
    })
}

/// Hub-side read behind the practice-history backfill (`GET /api/v1/app/state`,
/// sync-v2.md §17.6 / INV-21): page one identity's review events oldest-first so
/// a device that subscribes after the journal was pruned can still recover its
/// schedules. Paginated by `rowid` (at most `limit` rows after cursor `after`,
/// `0` = start); the next cursor is `Some` only when the page was full, exactly
/// like [`crate::datasets::dictation::dictation_rows_for_user`]. Page-stable
/// because an event is insert-once — its rowid never moves. Rows carry the
/// [`payload`] shape so the follower feeds them straight to [`apply_remote`].
// Hub-side read: only `sync::rest` calls it, so the mobile build has no caller.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub(crate) fn rows_for_user(
    settings: &SettingsState,
    user_id: &str,
    after: i64,
    limit: i64,
) -> Result<(Vec<Value>, Option<i64>), String> {
    let conn = crate::datasets::dictation::open_app_db(settings)?;
    ensure_review_schema(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT rowid, uuid, user_id, dataset_uuid, card_uuid, quality, reviewed_at, \
             familiarity, interval_days, ease_factor, repetitions, next_review_at \
             FROM card_review_event WHERE user_id = ?1 AND rowid > ?2 ORDER BY rowid ASC LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id, after, limit], |row| {
            let rowid = row.get::<_, i64>(0)?;
            let value = json!({
                "event_uuid": row.get::<_, String>(1)?,
                "user_id": row.get::<_, String>(2)?,
                "dataset_uuid": row.get::<_, String>(3)?,
                "card_uuid": row.get::<_, String>(4)?,
                "quality": row.get::<_, Option<i32>>(5)?,
                "reviewed_at": row.get::<_, String>(6)?,
                "familiarity": row.get::<_, i32>(7)?,
                "interval_days": row.get::<_, i32>(8)?,
                "ease_factor": row.get::<_, i32>(9)?,
                "repetitions": row.get::<_, i32>(10)?,
                "next_review_at": row.get::<_, Option<String>>(11)?,
            });
            Ok((rowid, value))
        })
        .map_err(|e| e.to_string())?;
    let mut out: Vec<Value> = Vec::new();
    let mut last_id = after;
    for r in rows.filter_map(|r| r.ok()) {
        last_id = r.0;
        out.push(r.1);
    }
    let next = if out.len() as i64 >= limit { Some(last_id) } else { None };
    Ok((out, next))
}

/// Replay one journaled `card_review` entry: store the event it describes, or —
/// for a payload that predates this shape — fold the rating it carries.
///
/// `dataset_uuid` may be empty: reviews journal under [`APP_SCOPE`], whose pull
/// arms pass no dataset, so the dataset is read out of the payload instead (as `xp`
/// does). A legacy change logged under the *dataset's* uuid arrives with it.
///
/// Returns `false` when the event was already present, which is the normal
/// outcome for the author receiving its own echo and *not* an error. The caller
/// owns the [`crate::sync::change_log::ApplyGuard`] / journal-append decision,
/// exactly as the other replay arms do.
pub(crate) fn apply_remote(
    settings: &SettingsState,
    dataset_uuid: &str,
    payload: &Value,
    write_identity: &str,
) -> Result<bool, String> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let event_uuid = get("event_uuid");
    let card_uuid = get("card_uuid");
    if card_uuid.is_empty() {
        return Err("card_review change carries no card_uuid".to_string());
    }
    // Shadowed as an owned String so a payload-carried dataset and an
    // argument-carried dataset follow the same path below.
    let dataset_uuid = if dataset_uuid.is_empty() { get("dataset_uuid") } else { dataset_uuid.to_string() };
    if dataset_uuid.is_empty() {
        return Err("card_review change carries no dataset_uuid".to_string());
    }

    let dir = super::find_card_dataset_dir(settings, &dataset_uuid)?;
    let ctx = open(settings, &dir, &dataset_uuid)?;

    // A legacy payload has no event uuid and no outcome: it is an instruction to
    // compute. Fold it against *this* node's history and store the result as a
    // local event — one rating applied once, which is the best available reading
    // of a change whose author's numbers we were never sent.
    if event_uuid.is_empty() {
        let quality = payload.get("quality").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        log::warn!(
            "[card_review] legacy change for card '{card_uuid}' (no event_uuid): folding quality {quality} locally"
        );
        let ev = record_rating(&ctx.conn, write_identity, &dataset_uuid, &card_uuid, quality)?;
        crate::sync::change_log::commit_change_as(
            settings,
            "card_review",
            &dataset_uuid,
            // `self::` because the parameter `payload` shadows this fn's name.
            &self::payload(&ev),
            write_identity,
        )?;
        return Ok(true);
    }

    let ev = ReviewEvent {
        uuid: event_uuid,
        user_id: if write_identity.is_empty() { get("user_id") } else { write_identity.to_string() },
        dataset_uuid: dataset_uuid.clone(),
        card_uuid,
        quality: payload.get("quality").and_then(|v| v.as_i64()).map(|q| q as i32),
        reviewed_at: canonical_or_now(payload.get("reviewed_at").and_then(|v| v.as_str())),
        familiarity: payload_i32(payload, "familiarity", 0),
        interval_days: payload_i32(payload, "interval_days", 0),
        ease_factor: payload_i32(payload, "ease_factor", 250),
        repetitions: payload_i32(payload, "repetitions", 0),
        next_review_at: payload
            .get("next_review_at")
            .and_then(|v| v.as_str())
            .map(|s| crate::datasets::canonical_stamp(s)),
    };
    let inserted = append_event(&ctx.conn, &ev)?;
    if inserted {
        // Log it onward so other followers receive the same event uuid and reach
        // the same state. Suppressed by the pull-side ApplyGuard, so this is how a
        // hub fans out; a follower re-logging its own echo cannot happen.
        crate::sync::change_log::commit_change_as(
            settings,
            "card_review",
            &dataset_uuid,
            &self::payload(&ev),
            &ev.user_id,
        )?;
    }
    Ok(inserted)
}

fn payload_i32(payload: &Value, key: &str, default: i32) -> i32 {
    payload.get(key).and_then(|v| v.as_i64()).map(|n| n as i32).unwrap_or(default)
}

/// Keep a stored stamp in the one shape this table's ordering depends on.
fn canonical_or_now(value: Option<&str>) -> String {
    match value {
        Some(raw) if !raw.is_empty() => crate::datasets::canonical_stamp(raw),
        _ => now_row_stamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_settings(tag: &str) -> (SettingsState, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fms-review-test-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = SettingsState::new();
        settings.set_workspace_dir(Some(dir.clone()));
        (settings, dir)
    }

    fn app_conn(settings: &SettingsState) -> Connection {
        let conn = crate::datasets::dictation::open_app_db(settings).unwrap();
        ensure_review_schema(&conn).unwrap();
        conn
    }

    fn ev(uuid: &str, user: &str, day: &str) -> ReviewEvent {
        ReviewEvent {
            uuid: uuid.to_string(),
            user_id: user.to_string(),
            dataset_uuid: "ds".to_string(),
            card_uuid: format!("card-{uuid}"),
            quality: Some(4),
            reviewed_at: format!("2026-01-{day}T00:00:00Z"),
            familiarity: 5,
            interval_days: 3,
            ease_factor: 250,
            repetitions: 1,
            next_review_at: Some(format!("2026-01-{day}T00:00:00Z")),
        }
    }

    /// Seed one event straight into the app DB (as an author or a replay would).
    fn seed_event(conn: &Connection, e: &ReviewEvent) {
        conn.execute(
            "INSERT INTO card_review_event \
             (uuid, user_id, dataset_uuid, card_uuid, quality, reviewed_at, \
              familiarity, interval_days, ease_factor, repetitions, next_review_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                e.uuid, e.user_id, e.dataset_uuid, e.card_uuid, e.quality, e.reviewed_at,
                e.familiarity, e.interval_days, e.ease_factor, e.repetitions, e.next_review_at
            ],
        )
        .unwrap();
    }

    // -- the hub-side state read behind the practice backfill (§17.6 / INV-21) ----

    #[test]
    fn rows_for_user_pages_only_my_events() {
        let (settings, _dir) = temp_settings("rows");
        {
            let conn = app_conn(&settings);
            for e in [ev("a", "local", "01"), ev("b", "local", "02"), ev("c", "local", "03"), ev("z", "other", "04")] {
                seed_event(&conn, &e);
            }
        }
        // A page larger than the set is the whole owner set, and `next` is None.
        let (all, next) = rows_for_user(&settings, "local", 0, 10).unwrap();
        assert_eq!(all.len(), 3, "only my events, never another user's");
        assert!(next.is_none(), "a short page means the history is exhausted");
        let ids: Vec<&str> = all.iter().map(|r| r["event_uuid"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["a", "b", "c"], "ordered by rowid (insertion order)");

        // A page exactly the size of a chunk reports a next cursor and continues.
        let (p1, n1) = rows_for_user(&settings, "local", 0, 2).unwrap();
        assert_eq!(p1.len(), 2);
        let c1 = n1.expect("a full page carries a next cursor");
        let (p2, n2) = rows_for_user(&settings, "local", c1, 2).unwrap();
        assert_eq!(p2[0]["event_uuid"].as_str(), Some("c"));
        assert!(n2.is_none());
    }

    #[test]
    fn rows_for_user_rows_carry_the_apply_remote_payload_shape() {
        let (settings, _dir) = temp_settings("shape");
        let src = ev("a", "local", "01");
        {
            let conn = app_conn(&settings);
            seed_event(&conn, &src);
        }
        let (rows, _) = rows_for_user(&settings, "local", 0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        // The page row must be exactly what `apply_remote` consumes: the same field
        // set `payload()` emits for a locally-authored event. Proving byte-identity
        // here is the round-trip guarantee, without needing a full dataset on disk.
        assert_eq!(rows[0], payload(&src), "a page row is identical to the journal payload");
    }
}
