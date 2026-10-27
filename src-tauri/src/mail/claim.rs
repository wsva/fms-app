//! The claim path of §4.1: a read phase, one short `BEGIN IMMEDIATE`, and the
//! classification that joins them.
//!
//! The split is not stylistic. Every app-DB writer — todos, XP, `mcp_audit`,
//! this path — queues behind one write lock, and the reconcile needs a *second*
//! database, which can open a file or wait out another process's write for up
//! to the 30 s busy timeout. So the reconcile read happens before the lock is
//! taken, and the transaction re-tests every condition it relies on: a row that
//! changed between the phases merely makes one statement change 0 rows.
//!
//! Nothing here opens a dataset. The one cross-database read arrives as a
//! closure returning [`Reconciled`], which is what keeps the ledger runnable
//! against `:memory:`.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use uuid::Uuid;

use super::store::{self, Mail, Refusal};
use super::{
    APP_TO_AGENT, CAPACITY, CLAIMED, DELIVERING, HOLD_CAP_LEASES, KIND_REQUEST, LEASE_MS, QUEUED,
    WAITING_USER, WRITE_LEASE_MS,
};

// ============================================================
// The reconcile, as a value
// ============================================================

/// What the artifact read found at a lapsed row's `target_uuid`. Three
/// outcomes, because "I could not look" must not be confused with "nothing is
/// there": the first closes the row, the second makes it claimable, the third
/// fails it (`subject_gone`).
pub enum Reconciled {
    /// The artifact already carries this row's `mail_uuid`: the write landed
    /// and the row did not close. T6.
    Landed,
    /// No artifact yet: the row is claimable (T4).
    Absent,
    /// The dataset, book or run no longer resolves (T16).
    SubjectGone,
}

// ============================================================
// The request and its answer
// ============================================================

/// `mail_claim`'s arguments, in the shape the SQL needs rather than the shape
/// the tool surface has.
pub struct ClaimRequest<'a> {
    /// Claim this row specifically.
    pub mail_uuid: Option<&'a str>,
    /// Or let the server choose the oldest free item.
    pub next: bool,
    /// Attach to a claim the caller already holds, instead of competing for a
    /// new one (§5: "`claim_id` attaches to an existing claim").
    pub claim_id: Option<&'a str>,
    /// Minted per attempt by the caller when it can; the server mints one when
    /// it cannot, and returns it either way so a retry can search by it.
    pub nonce: Option<&'a str>,
    /// What this caller declined already this turn.
    pub skip: &'a [String],
    /// A filter over what the caller is offered — never a declaration of what
    /// the work needs; the row's own `resource` decides capacity.
    pub want: Option<&'a str>,
    /// Sanitised `clientInfo.name`, display only.
    pub claimed_by: &'a str,
}

impl<'a> ClaimRequest<'a> {
    pub fn new(claimed_by: &'a str) -> Self {
        Self {
            mail_uuid: None,
            next: false,
            claim_id: None,
            nonce: None,
            skip: &[],
            want: None,
            claimed_by,
        }
    }

    pub fn item(mut self, uuid: &'a str) -> Self {
        self.mail_uuid = Some(uuid);
        self
    }

    pub fn next_item(mut self) -> Self {
        self.next = true;
        self
    }

    pub fn attach(mut self, uuid: &'a str, claim_id: &'a str) -> Self {
        self.mail_uuid = Some(uuid);
        self.claim_id = Some(claim_id);
        self
    }

    pub fn nonce(mut self, nonce: &'a str) -> Self {
        self.nonce = Some(nonce);
        self
    }

    pub fn skip(mut self, skip: &'a [String]) -> Self {
        self.skip = skip;
        self
    }

    pub fn want(mut self, resource: Option<&'a str>) -> Self {
        self.want = resource;
        self
    }
}

/// A lease the caller now holds.
#[derive(Debug, Clone, Serialize)]
pub struct Claimed {
    pub mail: Mail,
    /// `claimed` (a fresh or reclaimed lease), `attached` (an existing one,
    /// extended) or `resumed` (a retry that found its lease by nonce).
    pub how: &'static str,
    /// The nonce this lease is keyed on: keep it, and a retry can find the
    /// lease without knowing the uuid.
    pub nonce: Option<String>,
    pub expires_at: Option<i64>,
    /// How long the lease may still be extended, as an absolute time. Past it
    /// the only moves are deliver, fail or release.
    pub extendable_until: Option<i64>,
}

impl Claimed {
    fn of(mail: Mail, how: &'static str) -> Self {
        let extendable_until = mail
            .claimed_at
            .map(|c| c + HOLD_CAP_LEASES * LEASE_MS);
        Self {
            nonce: mail.claim_nonce.clone(),
            expires_at: mail.expires_at,
            mail,
            how,
            extendable_until,
        }
    }
}

// ============================================================
// The path
// ============================================================

/// Claim one item, the next item, or extend a lease the caller already holds.
///
/// `reconcile` is called at most once, outside any transaction, with the row
/// whose lease has lapsed: the caller looks for an artifact carrying this
/// row's `mail_uuid` at its `target_uuid` and says whether it landed. Taking
/// the whole row rather than a uuid is what lets the caller resolve the
/// subject (`ref_kind` / `ref_id`) as well as the target.
pub fn claim<F>(
    conn: &mut Connection,
    now: i64,
    req: &ClaimRequest,
    reconcile: F,
) -> Result<Claimed, Refusal>
where
    F: Fn(&Mail) -> Reconciled,
{
    // ---- Phase 0a: the nonce retry -------------------------------------
    // Keyed on the nonce ALONE: a `next` caller that lost the response never
    // learned the uuid, so a lookup by uuid could not find its lease.
    if let Some(nonce) = req.nonce {
        if let Some((uuid, claim_id, expires_at)) = nonce_lease(conn, nonce)? {
            let live = expires_at.map(|e| e > now).unwrap_or(false);
            let mail = store::read_one(conn, &uuid)
                .map_err(Refusal::storage)?
                .ok_or_else(|| Refusal::new("unknown", "the lease this nonce names is gone").at(&uuid))?;
            if live {
                return Ok(Claimed::of(mail, "resumed"));
            }
            // A lapsed lease is re-extended rather than handed back dead, so a
            // retry never returns a claim that cannot deliver.
            let touched = extend(conn, &uuid, claim_id.as_deref().unwrap_or(""), now)
                .map_err(Refusal::storage)?;
            if touched == 0 {
                return Err(Refusal::new(
                    "hold_expired",
                    "this lease is past its hold cap (three leases from the claim) or has been \
                     displaced. Deliver, fail or release; claiming again will not extend it.",
                )
                .at(&uuid));
            }
            let mail = store::read_one(conn, &uuid)
                .map_err(Refusal::storage)?
                .ok_or_else(|| Refusal::new("unknown", "the lease this nonce names is gone").at(&uuid))?;
            return Ok(Claimed::of(mail, "resumed"));
        }
    }

    // ---- Phase 0b: attach ------------------------------------------------
    // A carrier hands its claim to the agent it spawns; touch-on-use is the
    // same statement, which is why there is no heartbeat verb.
    if let Some(claim_id) = req.claim_id {
        let uuid = req.mail_uuid.ok_or_else(|| {
            Refusal::new(
                "invalid_request",
                "attach needs the mail_uuid whose claim_id you hold; to take new work call \
                 mail_claim {next: true}.",
            )
        })?;
        let touched = extend(conn, uuid, claim_id, now).map_err(Refusal::storage)?;
        if touched == 1 {
            let mail = store::read_one(conn, uuid)
                .map_err(Refusal::storage)?
                .ok_or_else(|| Refusal::new("unknown", "no mail with that uuid").at(uuid))?;
            return Ok(Claimed::of(mail, "attached"));
        }
        return Err(attach_refusal(conn, uuid, claim_id, now)?);
    }

    // ---- Phase 0c: the candidate ----------------------------------------
    let candidate = match req.mail_uuid {
        Some(uuid) => uuid.to_string(),
        None if req.next => match select_next(conn, now, req.want, req.skip)
            .map_err(Refusal::storage)?
        {
            Some(uuid) => uuid,
            None => return Err(empty_refusal(conn, now)?),
        },
        None => {
            return Err(Refusal::new(
                "invalid_request",
                "name a mail_uuid, or pass next: true to take the oldest free item.",
            ))
        }
    };

    // ---- Phase 0d: the one cross-database read, outside any lock ---------
    let mut landed = false;
    let mut gone = false;
    if let Some(row) = store::read_one(conn, &candidate).map_err(Refusal::storage)? {
        let lapsed = row.expires_at.map(|e| e <= now).unwrap_or(true);
        if matches!(row.status.as_str(), CLAIMED | DELIVERING) && lapsed {
            match reconcile(&row) {
                Reconciled::Landed => landed = true,
                Reconciled::SubjectGone => gone = true,
                Reconciled::Absent => {}
            }
        }
    }

    // ---- Phase 1: one short write transaction ---------------------------
    let claim_id = Uuid::new_v4().to_string();
    let nonce = req
        .nonce
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(Refusal::storage)?;

    // Step 1 — close what phase 0 proved delivered (T6). Conditional on the
    // claim that was read, so a row that changed meanwhile writes nothing.
    if landed {
        let closed = tx
            .execute(
                "UPDATE agent_mail \
                    SET status = 'delivered', delivered_at = ?2, result_ref = target_uuid, \
                        expires_at = NULL, updated_at = ?2 \
                  WHERE uuid = ?1 AND status IN ('claimed', 'delivering') \
                    AND expires_at IS NOT NULL AND expires_at <= ?2",
                params![candidate, now],
            )
            .map_err(Refusal::storage)?;
        if closed == 1 {
            tx.commit().map_err(Refusal::storage)?;
            return Err(Refusal::new(
                "reconciled",
                "a previous holder's write had already landed, so this item is delivered and \
                 needs nothing from you. Claim again for the next one.",
            )
            .at(&candidate));
        }
    }

    // Step 2 — the cap, behind the close: a third attempt that did write was
    // just delivered, not failed. Ledger-only, so the shared promotion runs.
    store::promote(&tx, now).map_err(Refusal::storage)?;

    // T16 — the subject no longer resolves, so this row is not work.
    if gone {
        store::subject_gone(&tx, &candidate, now).map_err(Refusal::storage)?;
        tx.commit().map_err(Refusal::storage)?;
        return Err(Refusal::new(
            "subject_gone",
            "the book, dataset or run behind this request no longer resolves, so it cannot be \
             generated. It has been failed and the user can see why.",
        )
        .at(&candidate));
    }

    // Step 3 — take it, or reclaim it once lapsed (T3/T4).
    let taken = tx
        .execute(&take_sql(), params![nonce, candidate, req.claimed_by, claim_id, now, LEASE_MS])
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ffi::ErrorCode::ConstraintViolation =>
            {
                // The nonce index is partial on live leases and unique on
                // purpose: a session-wide nonce would silently return a
                // different item's lease. Say so instead.
                Refusal::new(
                    "nonce_in_use",
                    "that nonce already names a live lease. Mint a fresh one per attempt, and \
                     retry with the nonce of the attempt you are resuming.",
                )
            }
            other => Refusal::storage(other),
        })?;

    if taken == 0 {
        // Nothing was written, so nothing needs committing.
        drop(tx);
        return Err(classify(conn, &candidate, now)?);
    }

    let mail = store::read_one(&tx, &candidate)
        .map_err(Refusal::storage)?
        .ok_or_else(|| Refusal::new("storage", "the row vanished inside its own claim"))?;
    tx.commit().map_err(Refusal::storage)?;
    Ok(Claimed::of(mail, "claimed"))
}

/// T13 — the holder hands it back. `unfit` refunds the attempt the claim
/// charged, because declining work you cannot do is not a failed attempt.
pub fn release(
    conn: &Connection,
    now: i64,
    uuid: &str,
    claim_id: &str,
    reason: Option<&str>,
) -> Result<Mail, Refusal> {
    let changed = conn
        .execute(
            "UPDATE agent_mail \
                SET status = 'queued', \
                    attempts = CASE WHEN ?3 = 'unfit' THEN MAX(attempts - 1, 0) ELSE attempts END, \
                    claim_id = NULL, claim_nonce = NULL, expires_at = NULL, reason = NULL, \
                    updated_at = ?4 \
              WHERE uuid = ?1 AND claim_id = ?2 AND status = 'claimed'",
            params![uuid, claim_id, reason.unwrap_or(""), now],
        )
        .map_err(Refusal::storage)?;

    if changed == 0 {
        let row = store::read_one(conn, uuid).map_err(Refusal::storage)?;
        let refusal = match row {
            None => Refusal::new("unknown", "no mail with that uuid").at(uuid),
            Some(row) if row.status == DELIVERING => Refusal::new(
                "in_flight",
                "a write is in flight on this item, so it cannot be released. It settles in at \
                 most two minutes; deliver, fail, or let it lapse.",
            )
            .at(uuid),
            Some(row) => Refusal::new(
                "claim_mismatch",
                "your claim_id no longer names this row: it lapsed, was taken over, or the work \
                 is already done. Re-read it with mail_list before acting.",
            )
            .at(uuid)
            .by(row.claimed_by.as_deref())
            .successor(row.superseded_by.as_deref()),
        };
        return Err(refusal);
    }

    store::read_one(conn, uuid)
        .map_err(Refusal::storage)?
        .ok_or_else(|| Refusal::new("storage", "the released row vanished").at(uuid))
}

/// T17 — the holder ends it. The attempt was charged at claim, so this is the
/// explicit exit rather than a counter; nothing else about the row changes,
/// because the record of who held it is what the user reads afterwards.
pub fn fail(
    conn: &Connection,
    now: i64,
    uuid: &str,
    claim_id: &str,
    reason: &str,
) -> Result<Mail, Refusal> {
    let changed = conn
        .execute(
            "UPDATE agent_mail \
                SET status = 'failed', reason = ?3, expires_at = NULL, updated_at = ?4 \
              WHERE uuid = ?1 AND claim_id = ?2 AND status IN ('claimed', 'delivering')",
            params![uuid, claim_id, store::clip_reason(reason), now],
        )
        .map_err(Refusal::storage)?;

    if changed == 0 {
        let row = store::read_one(conn, uuid).map_err(Refusal::storage)?;
        return Err(match row {
            None => Refusal::new("unknown", "no mail with that uuid").at(uuid),
            Some(row) => Refusal::new(
                "claim_mismatch",
                "your claim_id no longer names this row. If it was delivered meanwhile, nothing \
                 is left to do; otherwise re-read it with mail_list.",
            )
            .at(uuid)
            .by(row.claimed_by.as_deref())
            .successor(row.superseded_by.as_deref()),
        });
    }

    store::read_one(conn, uuid)
        .map_err(Refusal::storage)?
        .ok_or_else(|| Refusal::new("storage", "the failed row vanished").at(uuid))
}

// ============================================================
// Statements
// ============================================================

/// The per-resource capacity list, built from [`CAPACITY`] so the ledger and
/// the manual cannot disagree about what is capped. A resource with no row
/// here is uncapped, which is how `llm` stays free while `stt` is one.
fn capacity_cte() -> String {
    let values = CAPACITY
        .iter()
        .map(|(res, cap)| format!("('{res}', {cap})"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("WITH capacity(res, cap) AS (VALUES {values})")
}

/// "Does this row's resource have room?" — the decision half of the forecast
/// the `next` selection makes.
fn capacity_clause() -> &'static str {
    " AND (resource IS NULL \
        OR NOT EXISTS (SELECT 1 FROM capacity c WHERE c.res = agent_mail.resource) \
        OR (SELECT COUNT(*) FROM agent_mail m2 \
              WHERE m2.resource = agent_mail.resource \
                AND m2.uuid <> agent_mail.uuid \
                AND m2.status IN ('claimed', 'delivering') \
                AND m2.expires_at > ?5) \
           < (SELECT c.cap FROM capacity c WHERE c.res = agent_mail.resource))"
}

/// §4.1 step 3. `changes() = 1` means you won; 0 means terminal,
/// `waiting_user`, a live claim, or no capacity.
fn take_sql() -> String {
    format!(
        "{cte} \
         UPDATE agent_mail \
            SET status = 'claimed', claimed_by = ?3, claimed_at = ?5, claim_id = ?4, \
                claim_nonce = ?1, expires_at = ?5 + ?6, attempts = attempts + 1, \
                reason = NULL, updated_at = ?5 \
          WHERE uuid = ?2 AND status IN ('queued', 'claimed', 'delivering') \
            AND (status = 'queued' OR (expires_at IS NOT NULL AND expires_at <= ?5)){capacity}",
        cte = capacity_cte(),
        capacity = capacity_clause(),
    )
}

/// §4.1 step 4 — attach and touch-on-use in one statement. The extension is
/// state-appropriate (2 minutes while `delivering`, 10 while `claimed`) and
/// hard-capped at three leases from the claim.
fn extend(conn: &Connection, uuid: &str, claim_id: &str, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE agent_mail \
            SET expires_at = MIN(?3 + CASE status WHEN 'delivering' THEN ?4 ELSE ?5 END, \
                                 claimed_at + ?6 * ?5), \
                updated_at = ?3 \
          WHERE uuid = ?1 AND claim_id = ?2 AND status IN ('claimed', 'delivering') \
            AND ?3 < claimed_at + ?6 * ?5",
        params![uuid, claim_id, now, WRITE_LEASE_MS, LEASE_MS, HOLD_CAP_LEASES],
    )
}

/// §4.1's `next` selection: a plain read, no lock, because two callers
/// selecting the same row is resolved by the take, not prevented here. It
/// filters by direction and kind so a `queued` row of any sort cannot be
/// handed out as work.
fn select_next(
    conn: &Connection,
    now: i64,
    want: Option<&str>,
    skip: &[String],
) -> rusqlite::Result<Option<String>> {
    let mut sql = format!(
        "{cte} SELECT uuid FROM agent_mail \
          WHERE direction = '{APP_TO_AGENT}' AND kind = '{KIND_REQUEST}' \
            AND (status = '{QUEUED}' \
                 OR (status IN ('{CLAIMED}', '{DELIVERING}') AND expires_at <= ?1)) \
            AND (?2 IS NULL OR resource = ?2)",
        cte = capacity_cte(),
    );
    let mut args: Vec<&dyn rusqlite::ToSql> = Vec::new();
    args.push(&now);
    args.push(&want);
    if !skip.is_empty() {
        let list = (0..skip.len())
            .map(|i| format!("?{}", i + 3))
            .collect::<Vec<_>>()
            .join(", ");
        sql.push_str(&format!(" AND uuid NOT IN ({list})"));
        for uuid in skip {
            args.push(uuid);
        }
    }
    sql.push_str(&capacity_clause().replace("?5", "?1"));
    // FIFO, and `rowid` breaks the tie: two requests written in the same
    // millisecond must go out in the order they arrived, not in uuid order.
    sql.push_str(" ORDER BY created_at, rowid LIMIT 1");

    conn.query_row(&sql, args.as_slice(), |r| r.get::<_, String>(0))
        .optional()
}

/// One SELECT on the nonce alone. The partial unique index is what makes this
/// unambiguous: at most one live lease can hold a nonce.
fn nonce_lease(
    conn: &Connection,
    nonce: &str,
) -> Result<Option<(String, Option<String>, Option<i64>)>, Refusal> {
    conn.query_row(
        "SELECT uuid, claim_id, expires_at FROM agent_mail \
          WHERE claim_nonce = ?1 AND status IN ('claimed', 'delivering')",
        [nonce],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .map_err(Refusal::storage)
}

// ============================================================
// Classification
// ============================================================

/// Nothing was selected. If that is because the only work in the tray is
/// held, answer with the lease that will free it: `empty` would send the
/// caller home when a retry a few seconds later would have got work, and
/// the holder's name is what tells two agents they are not colliding.
fn empty_refusal(conn: &Connection, now: i64) -> Result<Refusal, Refusal> {
    let held = conn
        .query_row(
            "SELECT claimed_by, expires_at FROM agent_mail \
              WHERE direction = ?1 AND kind = ?2 \
                AND status IN (?3, ?4) \
                AND expires_at IS NOT NULL AND expires_at > ?5 \
              ORDER BY expires_at LIMIT 1",
            params![APP_TO_AGENT, KIND_REQUEST, CLAIMED, DELIVERING, now],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(Refusal::storage)?;

    Ok(match held {
        Some((holder, until)) => Refusal::new(
            "retry_after",
            format!(
                "every item in the tray is held right now ({} holds the earliest lease). Retry \
                 after the retry time, or call mail_wait to be woken when it frees.",
                holder.as_deref().unwrap_or("another agent")
            ),
        )
        .by(holder.as_deref())
        .retry(Some(until)),
        None => Refusal::new(
            "empty",
            "nothing is waiting to be claimed right now. Call mail_wait to be woken by the next \
             request, or ask the user to copy their brief.",
        ),
    })
}

/// Why a take changed 0 rows. The four cases have four different next actions,
/// which is the whole reason this is a function rather than one message.
fn classify(conn: &Connection, uuid: &str, now: i64) -> Result<Refusal, Refusal> {
    let row = store::read_one(conn, uuid).map_err(Refusal::storage)?;
    let Some(row) = row else {
        return Ok(Refusal::new(
            "unknown",
            "no mail with that uuid — it may have been trimmed by the 90-day retention \
             policy. Call mail_list for what is live.",
        )
        .at(uuid));
    };

    // A prompt pasted before a regenerate names the old row; send the caller
    // to the successor rather than into an error it cannot act on (§2.2).
    if let Some(successor) = row.superseded_by.as_deref() {
        return Ok(Refusal::new(
            "superseded",
            "this request was replaced by a newer one for the same subject, so there is nothing \
             to deliver. Claim the uuid this answer names.",
        )
        .at(uuid)
        .successor(Some(successor)));
    }

    match row.status.as_str() {
        WAITING_USER => Ok(Refusal::new(
            "not_claimable",
            "this request is waiting for the user's answer and cannot be claimed by anyone. \
             It returns to the queue as soon as they answer.",
        )
        .at(uuid)
        .by(row.claimed_by.as_deref())),
        CLAIMED | DELIVERING => {
            let live = row.expires_at.map(|e| e > now).unwrap_or(false);
            if live {
                Ok(Refusal::new(
                    "retry_after",
                    format!(
                        "{} already holds this item. Its lease lapses at the retry time; until \
                         then work on something else, or ask the user to take it back.",
                        row.claimed_by.as_deref().unwrap_or("another agent")
                    ),
                )
                .at(uuid)
                .by(row.claimed_by.as_deref())
                .retry(row.expires_at))
            } else {
                Ok(Refusal::new(
                    "retry_after",
                    "the row changed while this call was in flight. Call mail_claim again — the \
                     next attempt will see it as free.",
                )
                .at(uuid)
                .retry(Some(now + LEASE_MS)))
            }
        }
        QUEUED => {
            // A `queued` row with room is always takeable, so this is capacity.
            let soonest = soonest_lease(conn, row.resource.as_deref(), now).unwrap_or(now);
            Ok(Refusal::new(
                "retry_after",
                format!(
                    "the '{}' resource is at capacity{}, so this item is not yours yet. Retry \
                     after the retry time, or claim other work in the meantime.",
                    row.resource.as_deref().unwrap_or("stt"),
                    row.resource
                        .as_deref()
                        .and_then(|r| CAPACITY.iter().find(|(name, _)| *name == r))
                        .map(|(_, cap)| format!(" ({cap} in flight)"))
                        .unwrap_or_default(),
                ),
            )
            .at(uuid)
            .retry(Some(soonest)))
        }
        _ => Ok(Refusal::new(
            "settled",
            format!(
                "this item is already {} ({}). Nothing is left to claim; read it with mail_list.",
                row.status,
                row.reason.as_deref().unwrap_or("no reason recorded")
            ),
        )
        .at(uuid)
        .successor(row.superseded_by.as_deref())),
    }
}

/// Why an attach/touch changed nothing.
fn attach_refusal(
    conn: &Connection,
    uuid: &str,
    claim_id: &str,
    now: i64,
) -> Result<Refusal, Refusal> {
    let _ = now;
    let row = store::read_one(conn, uuid).map_err(Refusal::storage)?;
    Ok(match row {
        None => Refusal::new("unknown", "no mail with that uuid").at(uuid),
        Some(row) if row.claim_id.as_deref() == Some(claim_id) => Refusal::new(
            "hold_expired",
            "this claim is past its hold cap (three leases from the claim), so it can no longer \
             be extended. Deliver, fail or release.",
        )
        .at(uuid),
        Some(row) if row.status == WAITING_USER => Refusal::new(
            "not_claimable",
            "this request is waiting for the user's answer; there is no lease to extend.",
        )
        .at(uuid),
        Some(row) => Refusal::new(
            "claim_mismatch",
            "your claim_id no longer names this row: nobody holds it, or somebody else does. \
             Re-read it with mail_list, then claim it again if it is free.",
        )
        .at(uuid)
        .by(row.claimed_by.as_deref())
        .successor(row.superseded_by.as_deref()),
    })
}

/// The soonest a lease on this resource lapses — the honest `retry_after` for
/// a capacity refusal. `None` when nothing live holds it, which means the
/// caller lost a race rather than a capacity draw.
fn soonest_lease(conn: &Connection, resource: Option<&str>, now: i64) -> Option<i64> {
    conn.query_row(
        "SELECT MIN(expires_at) FROM agent_mail \
          WHERE resource IS ?1 AND status IN ('claimed', 'delivering') AND expires_at > ?2",
        params![resource, now],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::store::{ensure_schema, head, list, request, MailQuery, NewRequest};
    use crate::mail::{
        Folder, OwedBy, ATTEMPT_CAP, DELIVERED, FAILED, ORIGIN_APP, REASON_ATTEMPTS_EXHAUSTED,
        REASON_SUBJECT_GONE,
    };

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn seed(conn: &mut Connection, subject: &str) -> String {
        let req = NewRequest::new("chan", subject, subject).origin(ORIGIN_APP);
        request(conn, 1_000, &req).unwrap().mail.uuid
    }

    /// Nothing here is there.
    fn absent(_: &Mail) -> Reconciled {
        Reconciled::Absent
    }

    #[test]
    fn an_empty_tray_answers_empty() {
        let mut conn = db();
        let err = claim(&mut conn, 1_000, &ClaimRequest::new("goose").next_item(), absent).unwrap_err();
        assert_eq!(err.code, "empty");
        assert!(err.retry_after.is_none());
    }

    #[test]
    fn a_claim_charges_an_attempt_and_takes_a_lease() {
        let mut conn = db();
        let uuid = seed(&mut conn, "s1");
        let got = claim(
            &mut conn,
            2_000,
            &ClaimRequest::new("goose").next_item().nonce("n1"),
            absent,
        )
        .unwrap();
        assert_eq!(got.mail.uuid, uuid);
        assert_eq!(got.mail.status, CLAIMED);
        assert_eq!(got.mail.attempts, 1);
        assert_eq!(got.mail.claimed_by.as_deref(), Some("goose"));
        assert_eq!(got.mail.expires_at, Some(2_000 + LEASE_MS));
        assert_eq!(got.nonce.as_deref(), Some("n1"));
        assert_eq!(got.how, "claimed");
        assert_eq!(got.mail.folder(), Folder::Outbox);
    }

    #[test]
    fn a_live_claim_is_refused_with_retry_after_and_the_holder() {
        let mut conn = db();
        seed(&mut conn, "s1");
        claim(&mut conn, 2_000, &ClaimRequest::new("goose").next_item(), absent).unwrap();
        let err = claim(&mut conn, 2_001, &ClaimRequest::new("codex").next_item(), absent).unwrap_err();
        assert_eq!(err.code, "retry_after");
        assert_eq!(err.holder.as_deref(), Some("goose"));
        assert_eq!(err.retry_after, Some(2_000 + LEASE_MS));
    }

    #[test]
    fn a_named_claim_keeps_the_queue_order() {
        let mut conn = db();
        let first = seed(&mut conn, "s1");
        let second = seed(&mut conn, "s2");
        let got = claim(&mut conn, 2_000, &ClaimRequest::new("a").next_item(), absent).unwrap();
        assert_eq!(got.mail.uuid, first, "the oldest goes first");
        let got = claim(&mut conn, 2_000, &ClaimRequest::new("b").next_item(), absent).unwrap();
        assert_eq!(got.mail.uuid, second);
    }

    #[test]
    fn skip_is_honoured_within_a_turn() {
        let mut conn = db();
        let first = seed(&mut conn, "s1");
        let second = seed(&mut conn, "s2");
        let skip = vec![first.clone()];
        let got = claim(
            &mut conn,
            2_000,
            &ClaimRequest::new("a").next_item().skip(&skip),
            absent,
        )
        .unwrap();
        assert_eq!(got.mail.uuid, second);
    }

    /// A retry that lost the response finds its lease by nonce alone, and a
    /// retry of a *lapsed* lease gets it extended rather than handed back dead.
    #[test]
    fn a_nonce_retry_resumes_the_same_lease() {
        let mut conn = db();
        seed(&mut conn, "s1");
        let first = claim(
            &mut conn,
            2_000,
            &ClaimRequest::new("goose").next_item().nonce("n1"),
            absent,
        )
        .unwrap();

        let retry = claim(
            &mut conn,
            2_500,
            &ClaimRequest::new("goose").next_item().nonce("n1"),
            absent,
        )
        .unwrap();
        assert_eq!(retry.mail.uuid, first.mail.uuid);
        assert_eq!(retry.how, "resumed");
        assert_eq!(retry.mail.attempts, 1, "a retry is not a second attempt");

        // Lapsed: extended, not dead.
        let extended = claim(
            &mut conn,
            2_000 + LEASE_MS + 1,
            &ClaimRequest::new("goose").next_item().nonce("n1"),
            absent,
        )
        .unwrap();
        assert!(extended.mail.expires_at.unwrap() > 2_000 + LEASE_MS);
        assert_eq!(extended.mail.attempts, 1);
    }

    /// Touch-on-use cannot hold an item forever: past three leases from the
    /// claim the only answers are deliver, fail or release.
    #[test]
    fn the_hold_cap_ends_the_extension() {
        let mut conn = db();
        seed(&mut conn, "s1");
        let held = claim(
            &mut conn,
            1_000,
            &ClaimRequest::new("goose").next_item(),
            absent,
        )
        .unwrap();
        let claim_id = held.mail.claim_id.clone().unwrap();
        let uuid = held.mail.uuid.clone();

        // Each touch inside the window extends by one lease.
        for step in 1..HOLD_CAP_LEASES {
            let now = 1_000 + step * LEASE_MS / 2;
            let touched = claim(
                &mut conn,
                now,
                &ClaimRequest::new("goose").attach(&uuid, &claim_id),
                absent,
            )
            .unwrap();
            assert_eq!(touched.how, "attached");
        }

        // Past `claimed_at + 3 leases`, nothing extends.
        let err = claim(
            &mut conn,
            1_000 + HOLD_CAP_LEASES * LEASE_MS + 1,
            &ClaimRequest::new("goose").attach(&uuid, &claim_id),
            absent,
        )
        .unwrap_err();
        assert_eq!(err.code, "hold_expired");
    }

    /// The reconcile is what makes "the write landed but the row did not
    /// close" cost nothing: the next claim closes the row instead of redoing
    /// two minutes of model work.
    #[test]
    fn a_lapsed_row_whose_write_landed_is_closed_not_handed_out() {
        let mut conn = db();
        seed(&mut conn, "s1");
        let held = claim(&mut conn, 1_000, &ClaimRequest::new("goose").next_item(), absent).unwrap();
        let uuid = held.mail.uuid.clone();
        conn.execute(
            "UPDATE agent_mail SET status = 'delivering', expires_at = 2000 WHERE uuid = ?1",
            [&uuid],
        )
        .unwrap();

        let err = claim(&mut conn, 3_000, &ClaimRequest::new("codex").next_item(), |_| {
            Reconciled::Landed
        })
        .unwrap_err();
        assert_eq!(err.code, "reconciled");
        let row = store::read_one(&conn, &uuid).unwrap().unwrap();
        assert_eq!(row.status, DELIVERED);
        assert_eq!(row.delivered_at, Some(3_000));
        assert!(row.expires_at.is_none());
    }

    #[test]
    fn a_lapsed_row_without_an_artifact_is_reclaimed() {
        let mut conn = db();
        seed(&mut conn, "s1");
        let held = claim(&mut conn, 1_000, &ClaimRequest::new("goose").next_item(), absent).unwrap();
        let again = claim(
            &mut conn,
            1_000 + LEASE_MS + 1,
            &ClaimRequest::new("codex").next_item(),
            absent,
        )
        .unwrap();
        assert_eq!(again.mail.uuid, held.mail.uuid);
        assert_eq!(again.mail.attempts, 2, "reclaiming is a new attempt");
        assert_eq!(again.mail.claimed_by.as_deref(), Some("codex"));
        assert_ne!(again.mail.claim_id, held.mail.claim_id, "the old writer is fenced off");
    }

    #[test]
    fn a_gone_subject_fails_the_row() {
        let mut conn = db();
        seed(&mut conn, "s1");
        claim(&mut conn, 1_000, &ClaimRequest::new("goose").next_item(), absent).unwrap();
        conn.execute("UPDATE agent_mail SET expires_at = 2000", []).unwrap();
        let err = claim(&mut conn, 3_000, &ClaimRequest::new("codex").next_item(), |_| {
            Reconciled::SubjectGone
        })
        .unwrap_err();
        assert_eq!(err.code, "subject_gone");
        let rows = list(&conn, 3_000, &MailQuery::page(10, 0)).unwrap();
        assert_eq!(rows[0].status, FAILED);
        assert_eq!(rows[0].reason.as_deref(), Some(REASON_SUBJECT_GONE));
    }

    /// The cap runs behind the reconcile and behind an expiry test: a live
    /// third attempt is left alone, and a dead one reaches the Inbox.
    #[test]
    fn a_dead_third_attempt_is_capped_at_claim_even_when_it_was_skipped() {
        let mut conn = db();
        seed(&mut conn, "s1");
        let third = seed(&mut conn, "s2");
        conn.execute(
            "UPDATE agent_mail SET status = 'claimed', claimed_at = 0, claim_id = 'old', \
                expires_at = 100, attempts = ?2 WHERE uuid = ?1",
            params![third, ATTEMPT_CAP],
        )
        .unwrap();

        let held = claim(&mut conn, 1_000, &ClaimRequest::new("goose").next_item(), absent).unwrap();
        assert_ne!(held.mail.uuid, third, "an exhausted row is not handed out");
        let dead = store::read_one(&conn, &third).unwrap().unwrap();
        assert_eq!(dead.status, FAILED);
        assert_eq!(dead.reason.as_deref(), Some(REASON_ATTEMPTS_EXHAUSTED));
    }

    #[test]
    fn capacity_is_per_resource_and_refused_with_the_soonest_lease() {
        let mut conn = db();
        // One STT item and one LLM item; STT is capped at one.
        let stt = request(
            &mut conn,
            1_000,
            &NewRequest::new("c", "stt-1", "transcribe")
                .resource(Some("stt"))
                .origin(ORIGIN_APP),
        )
        .unwrap()
        .mail
        .uuid;
        let llm = request(
            &mut conn,
            1_000,
            &NewRequest::new("c", "llm-1", "explain")
                .resource(Some("llm"))
                .origin(ORIGIN_APP),
        )
        .unwrap()
        .mail
        .uuid;

        let won = claim(&mut conn, 2_000, &ClaimRequest::new("a").next_item(), absent).unwrap();
        assert_eq!(won.mail.uuid, stt);

        // The second caller is offered the uncapped resource, not the capped one.
        let other = claim(&mut conn, 2_000, &ClaimRequest::new("b").next_item(), absent).unwrap();
        assert_eq!(other.mail.uuid, llm);

        // A named claim on a *free* row of the capped resource is held to the
        // same count, and the refusal names the resource and its soonest lease.
        let second_stt = request(
            &mut conn,
            1_000,
            &NewRequest::new("c", "stt-2", "transcribe")
                .resource(Some("stt"))
                .origin(ORIGIN_APP),
        )
        .unwrap()
        .mail
        .uuid;
        let err = claim(&mut conn, 2_000, &ClaimRequest::new("c").item(&second_stt), absent)
            .unwrap_err();
        assert_eq!(err.code, "retry_after");
        assert_eq!(err.retry_after, Some(2_000 + LEASE_MS));
        assert!(err.message.contains("stt"));

        // Naming the held row itself answers with its holder instead, because
        // that lease is what the caller is waiting on.
        let err = claim(&mut conn, 2_000, &ClaimRequest::new("c").item(&stt), absent).unwrap_err();
        assert_eq!(err.code, "retry_after");
        assert_eq!(err.holder.as_deref(), Some("a"));
        assert_eq!(err.retry_after, Some(2_000 + LEASE_MS));
    }

    #[test]
    fn release_refunds_only_unfit() {
        let mut conn = db();
        seed(&mut conn, "s1");
        let held = claim(&mut conn, 1_000, &ClaimRequest::new("goose").next_item(), absent).unwrap();
        let uuid = held.mail.uuid.clone();
        let claim_id = held.mail.claim_id.clone().unwrap();

        let back = release(&conn, 2_000, &uuid, &claim_id, None).unwrap();
        assert_eq!(back.status, QUEUED);
        assert_eq!(back.attempts, 1, "a plain release keeps the attempt");
        assert!(back.claim_id.is_none() && back.claim_nonce.is_none());
        assert!(back.expires_at.is_none());

        // The nonce is reusable now: the index only covers live leases.
        let again = claim(
            &mut conn,
            3_000,
            &ClaimRequest::new("codex").item(&uuid).nonce("n1"),
            absent,
        )
        .unwrap();
        let claim_id = again.mail.claim_id.clone().unwrap();
        let back = release(&conn, 4_000, &uuid, &claim_id, Some("unfit")).unwrap();
        assert_eq!(back.attempts, 1, "refunded back to one");

        // A stranger cannot release somebody else's work.
        claim(&mut conn, 5_000, &ClaimRequest::new("goose").item(&uuid).nonce("n2"), absent).unwrap();
        let err = release(&conn, 6_000, &uuid, "not-the-claim", None).unwrap_err();
        assert_eq!(err.code, "claim_mismatch");
    }

    #[test]
    fn fail_ends_the_item_and_keeps_the_reason() {
        let mut conn = db();
        seed(&mut conn, "s1");
        let held = claim(&mut conn, 1_000, &ClaimRequest::new("goose").next_item(), absent).unwrap();
        let claim_id = held.mail.claim_id.clone().unwrap();
        let failed = fail(&conn, 2_000, &held.mail.uuid, &claim_id, "the text is cut off").unwrap();
        assert_eq!(failed.status, FAILED);
        assert_eq!(failed.reason.as_deref(), Some("the text is cut off"));
        assert_eq!(failed.owed_by(), OwedBy::You, "a failure is the user's to see");

        let err = fail(&conn, 3_000, &held.mail.uuid, &claim_id, "again").unwrap_err();
        assert_eq!(err.code, "claim_mismatch");
    }

    #[test]
    fn a_superseded_uuid_answers_with_its_successor() {
        let mut conn = db();
        let old = seed(&mut conn, "s1");
        let new = seed(&mut conn, "s1");
        let err = claim(&mut conn, 2_000, &ClaimRequest::new("goose").item(&old), absent).unwrap_err();
        assert_eq!(err.code, "superseded");
        assert_eq!(err.successor.as_deref(), Some(new.as_str()));
    }

    #[test]
    fn a_waiting_request_is_not_claimable() {
        let mut conn = db();
        let uuid = seed(&mut conn, "s1");
        conn.execute(
            "UPDATE agent_mail SET status = 'waiting_user', expires_at = 99999999 WHERE uuid = ?1",
            [&uuid],
        )
        .unwrap();
        let err = claim(&mut conn, 2_000, &ClaimRequest::new("goose").next_item(), absent).unwrap_err();
        assert_eq!(err.code, "empty", "a waiting row is not offered by next");

        let err = claim(&mut conn, 2_000, &ClaimRequest::new("goose").item(&uuid), absent).unwrap_err();
        assert_eq!(err.code, "not_claimable");
    }

    /// The head and the claim path must agree about what is left to do.
    #[test]
    fn head_and_claims_agree() {
        let mut conn = db();
        seed(&mut conn, "s1");
        seed(&mut conn, "s2");
        let starting = head(&conn, 0).unwrap();
        assert_eq!(starting.owed_agent, 2);
        claim(&mut conn, 1_000, &ClaimRequest::new("a").next_item(), absent).unwrap();
        let held = head(&conn, 1_500).unwrap();
        assert_eq!(held.owed_agent, 2);
        assert_eq!(held.held, 1);
        assert_eq!(held.waiting, 1);
        // The lease lapses and the row is still offered, only counted as waiting.
        let lapsed = head(&conn, 1_000 + LEASE_MS + 1).unwrap();
        assert_eq!(lapsed.held, 0);
        assert_eq!(lapsed.waiting, 2);
    }
}
