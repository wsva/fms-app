//! The agent-mail ledger's state machine — §4.1 of
//! `docs/design/agent/mailbox.md`, transcribed once.
//!
//! The design document states the machine as one transition table (T1–T16).
//! That table is [`TRANSITIONS`] here, and this module is the only place a
//! move is spelled out: the SQL in `agent_mail` asks [`check`] before it
//! writes, and the tests at the bottom enumerate every state against every
//! actor so a row cannot be added to the document and forgotten in the code.
//!
//! Nothing here reads a clock, opens a database or knows about Tauri. The
//! caller passes the facts it has already read (`expires_at > now`,
//! `claim_id = ?`, capacity free) and gets back the move it is allowed to
//! make. That is what makes the machine total and testable without a ledger.
//!
//! Two consequences worth stating, because they are the ones the round-4
//! review found contradicting each other in prose:
//!
//! - [`TRANSITIONS`] order carries **no** meaning. The ledger names the
//!   transition it intends to perform, so "close the reconcile before the cap"
//!   is [`CLAIM_PATH`]'s ordering and nothing else's.
//! - `Actor` is *who is accountable*, not *what called*. `Clock` moves rows
//!   only forward and never touches the attempt counter.

use MailState::*;

/// `agent_mail.status`. Four open, three terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailState {
    Queued,
    Claimed,
    Delivering,
    WaitingUser,
    Delivered,
    Cancelled,
    Failed,
}

impl MailState {
    /// The four states a row can still leave. `handed_at` is not one of them.
    pub fn is_open(self) -> bool {
        matches!(self, Queued | Claimed | Delivering | WaitingUser)
    }

    pub fn is_terminal(self) -> bool {
        !self.is_open()
    }

    /// The value §4 stores in `agent_mail.status`. The wire form and the
    /// column are the same string on purpose: the dock, the SQL and the
    /// journal all quote one vocabulary.
    pub fn as_str(self) -> &'static str {
        match self {
            Queued => "queued",
            Claimed => "claimed",
            Delivering => "delivering",
            WaitingUser => "waiting_user",
            Delivered => "delivered",
            Cancelled => "cancelled",
            Failed => "failed",
        }
    }

    /// The inverse of [`as_str`](Self::as_str). `None` is a row written by
    /// something that did not agree with this enum, which the ledger treats
    /// as a read error rather than guessing a state.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Queued,
            "claimed" => Claimed,
            "delivering" => Delivering,
            "waiting_user" => WaitingUser,
            "delivered" => Delivered,
            "cancelled" => Cancelled,
            "failed" => Failed,
            _ => return None,
        })
    }
}

/// Who is accountable for a move — the table's `Actor` column.
///
/// `Holder` is the agent currently named by the row's `claim_id`; `Agent` is
/// any agent at all (the claim path). A holder acting on a row it no longer
/// holds is refused by the `claim_matches` guard, not by a different actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// The app itself: row creation, supersede, subject disappearance.
    App,
    /// Any agent, at the claim path.
    Agent,
    /// The agent whose `claim_id` the row currently names.
    Holder,
    /// The person in front of the dock.
    User,
    /// `expires_at` passing. Never an act, and never a rewind.
    Clock,
}

/// The conditions a move requires, read from the row (and from `now`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// The statement's `WHERE` clause is the guard (a CAS on a named column).
    Always,
    /// `claim_id` names the caller.
    ClaimMatches,
    /// `claim_id` names the caller and the payload passed shape/content checks.
    ClaimAndPayload,
    /// `claim_id` names the caller and `parent_uuid` names this request.
    ClaimAndParent,
    /// The row's `resource` has room. A resource with no capacity row is
    /// uncapped, so it always has room.
    CapacityFree,
    /// `expires_at <= now` and the row's `resource` has room.
    LapsedAndFree,
    /// Free — `queued`, or `expires_at <= now` — and at the cap.
    FreeAndAtCap,
    /// `expires_at <= now`, and the artifact at `target_uuid` already carries
    /// this row's `mail_uuid`: the write landed and the row did not close.
    LapsedAndArtifact,
    /// An answer arrived before the `waiting_user` deadline.
    BeforeDeadline,
    /// The deadline passed.
    DeadlinePassed,
    /// The book, dataset or run behind the request no longer resolves.
    SubjectGone,
}

impl Guard {
    /// `from` is needed because "free" is a property of the state as much as
    /// of the clock: a `queued` row holds no lease to have lapsed.
    fn holds(self, from: MailState, f: &Facts) -> bool {
        match self {
            Guard::Always => true,
            Guard::ClaimMatches => f.claim_matches,
            Guard::ClaimAndPayload => f.claim_matches && f.payload_valid,
            Guard::ClaimAndParent => f.claim_matches && f.parent_matches,
            Guard::CapacityFree => f.capacity_free,
            Guard::LapsedAndFree => !f.lease_live && f.capacity_free,
            Guard::FreeAndAtCap => (from == Queued || !f.lease_live) && f.attempts_at_cap,
            Guard::LapsedAndArtifact => !f.lease_live && f.artifact_landed,
            Guard::BeforeDeadline => f.before_deadline,
            Guard::DeadlinePassed => !f.before_deadline,
            Guard::SubjectGone => !f.subject_resolves,
        }
    }
}

/// What a move does to `attempts`. Only taking work charges it; only the three
/// refunds give it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptDelta {
    /// 0.
    Same,
    /// +1 — the claim charged it.
    Charge,
    /// −1 — a clarification or a takeover is not a failure.
    Refund,
    /// 0, or −1 when the caller released with `unfit`.
    RefundIfUnfit,
}

impl AttemptDelta {
    pub fn apply(self, f: &Facts) -> i32 {
        match self {
            AttemptDelta::Same => 0,
            AttemptDelta::Charge => 1,
            AttemptDelta::Refund => -1,
            AttemptDelta::RefundIfUnfit => {
                if f.release_unfit {
                    -1
                } else {
                    0
                }
            }
        }
    }
}

/// The facts a guard reads. Every field is something the ledger has already
/// read; the machine never fetches anything itself.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    /// `expires_at > now`. A `queued` row has no lease, so the caller sets
    /// this to `false` for one.
    pub lease_live: bool,
    /// The caller's `claim_id` equals the row's.
    pub claim_matches: bool,
    /// The row's `resource` has room under the per-resource capacity.
    pub capacity_free: bool,
    /// `attempts >= cap`.
    pub attempts_at_cap: bool,
    /// The proposed delivery passed shape, content-hash and scope checks.
    pub payload_valid: bool,
    /// The question's `parent_uuid` names the row it was asked about.
    pub parent_matches: bool,
    /// The artifact at `target_uuid` carries this row's `mail_uuid`.
    pub artifact_landed: bool,
    /// Before the `waiting_user` deadline.
    pub before_deadline: bool,
    /// The subject behind the request still resolves.
    pub subject_resolves: bool,
    /// The holder released with `reason: 'unfit'`.
    pub release_unfit: bool,
}

impl Default for Facts {
    /// Everything closed: no live lease, no matching claim, no capacity, at
    /// the cap, nothing landed. Tests start here and turn on what they mean.
    fn default() -> Self {
        Self {
            lease_live: false,
            claim_matches: false,
            capacity_free: false,
            attempts_at_cap: false,
            payload_valid: false,
            parent_matches: false,
            artifact_landed: false,
            before_deadline: false,
            subject_resolves: false,
            release_unfit: false,
        }
    }
}

/// One row of §4.1's table.
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    /// `T1`..`T16`, the id every other section cites.
    pub id: &'static str,
    /// The states this row can be performed from.
    pub from: &'static [MailState],
    pub actor: Actor,
    pub guard: Guard,
    pub to: MailState,
    pub delta: AttemptDelta,
}

impl Transition {
    fn allows(self, from: MailState, f: &Facts) -> bool {
        self.from.contains(&from) && self.guard.holds(from, f)
    }
}

/// §4.1's table, in id order. **Order is documentation, not precedence** —
/// the ledger names the move it wants ([`check`]) or walks an explicit path
/// ([`CLAIM_PATH`]), so two rows can share a from-state without one shadowing
/// the other.
pub const TRANSITIONS: &[Transition] = &[
    // T1 — the insert. `target_uuid` and both keys are derived here, never
    // chosen by a caller.
    Transition {
        id: "T1",
        from: &[],
        actor: Actor::App,
        guard: Guard::Always,
        to: Queued,
        delta: AttemptDelta::Same,
    },
    // T2 — supersede: the user asked for the same subject again with different
    // parameters while nobody had taken the row.
    Transition {
        id: "T2",
        from: &[Queued],
        actor: Actor::App,
        guard: Guard::Always,
        to: Cancelled,
        delta: AttemptDelta::Same,
    },
    // T3 — the ordinary claim. A `queued` row is claimable whatever
    // `expires_at` holds, because release nulls it; only capacity can refuse.
    Transition {
        id: "T3",
        from: &[Queued],
        actor: Actor::Agent,
        guard: Guard::CapacityFree,
        to: Claimed,
        delta: AttemptDelta::Charge,
    },
    // T4 — reclaim. A lapsed `delivering` arrives here as `claimed`, because
    // the reconcile (T6) closed it first if the write had landed.
    Transition {
        id: "T4",
        from: &[Claimed, Delivering],
        actor: Actor::Agent,
        guard: Guard::LapsedAndFree,
        to: Claimed,
        delta: AttemptDelta::Charge,
    },
    // T5 — enter the two-phase commit. The lease shrinks to the write lease.
    Transition {
        id: "T5",
        from: &[Claimed],
        actor: Actor::Holder,
        guard: Guard::ClaimAndPayload,
        to: Delivering,
        delta: AttemptDelta::Same,
    },
    // T6 — the reconcile: the write landed and the row did not close.
    Transition {
        id: "T6",
        from: &[Claimed, Delivering],
        actor: Actor::Agent,
        guard: Guard::LapsedAndArtifact,
        to: Delivered,
        delta: AttemptDelta::Same,
    },
    // T7 — the close. Can change 0 rows; that is §5.1's `written_unconfirmed`.
    Transition {
        id: "T7",
        from: &[Delivering],
        actor: Actor::Holder,
        guard: Guard::ClaimMatches,
        to: Delivered,
        delta: AttemptDelta::Same,
    },
    // T8 — a reported write error rolls back to `claimed`, not to `queued`:
    // the holder keeps its lease and may retry.
    Transition {
        id: "T8",
        from: &[Delivering],
        actor: Actor::Holder,
        guard: Guard::ClaimMatches,
        to: Claimed,
        delta: AttemptDelta::Same,
    },
    // T9 — ask the user. A clarification is not a failure, so the attempt the
    // claim charged is given back.
    Transition {
        id: "T9",
        from: &[Claimed, Delivering],
        actor: Actor::Holder,
        guard: Guard::ClaimAndParent,
        to: WaitingUser,
        delta: AttemptDelta::Refund,
    },
    // T10 — the answer. `read_at` is set on the question in the same
    // statement, so a paid debt cannot re-enter the Inbox unread.
    Transition {
        id: "T10",
        from: &[WaitingUser],
        actor: Actor::User,
        guard: Guard::BeforeDeadline,
        to: Queued,
        delta: AttemptDelta::Same,
    },
    // T11 — the answer deadline passed.
    Transition {
        id: "T11",
        from: &[WaitingUser],
        actor: Actor::Clock,
        guard: Guard::DeadlinePassed,
        to: Failed,
        delta: AttemptDelta::Same,
    },
    // T12 — the cap. Runs on any read as well as on a claim, so a third
    // attempt that crashed still reaches the Inbox as `failed`.
    Transition {
        id: "T12",
        from: &[Queued, Claimed, Delivering],
        actor: Actor::Clock,
        guard: Guard::FreeAndAtCap,
        to: Failed,
        delta: AttemptDelta::Same,
    },
    // T13 — the holder hands it back.
    Transition {
        id: "T13",
        from: &[Claimed],
        actor: Actor::Holder,
        guard: Guard::ClaimMatches,
        to: Queued,
        delta: AttemptDelta::RefundIfUnfit,
    },
    // T14 — `Take over`: the user ends a lease that has **not** lapsed, on the
    // `claim_id` the dock displayed.
    Transition {
        id: "T14",
        from: &[Claimed],
        actor: Actor::User,
        guard: Guard::Always,
        to: Queued,
        delta: AttemptDelta::Refund,
    },
    // T15 — Cancel. No `delivering` in the from-column: two minutes is too
    // short to arbitrate, so the user has no transition out of a write.
    Transition {
        id: "T15",
        from: &[Queued, Claimed, WaitingUser],
        actor: Actor::User,
        guard: Guard::Always,
        to: Cancelled,
        delta: AttemptDelta::Same,
    },
    // T16 — the subject behind the request stopped resolving.
    Transition {
        id: "T16",
        from: &[Queued, Claimed, Delivering, WaitingUser],
        actor: Actor::App,
        guard: Guard::SubjectGone,
        to: Failed,
        delta: AttemptDelta::Same,
    },
    // T17 — `mail_fail`: the holder ends its own item. The diagram has always
    // asserted this move; the table did not have a row for it, so the attempt
    // cap and the holder's explicit exit were conflated under T12. They are
    // not the same move: T12 is the clock ending work nobody holds, this is
    // the holder saying it cannot be done, and only this one carries the
    // caller's reason. It is available from `delivering` on purpose — a
    // holder that has discovered the write cannot succeed should be able to
    // say so rather than sit out the write lease.
    Transition {
        id: "T17",
        from: &[Claimed, Delivering],
        actor: Actor::Holder,
        guard: Guard::ClaimMatches,
        to: Failed,
        delta: AttemptDelta::Same,
    },
];

/// Why a named transition may not be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Illegal {
    /// No row has that id — a typo in the ledger, not a user-facing refusal.
    Unknown(&'static str),
    /// The row is not in a state this move starts from.
    WrongState {
        id: &'static str,
        from: MailState,
        allowed: &'static [MailState],
    },
    /// The row is in the right state, but the guard did not hold. The ledger
    /// turns this into the specific refusal (`late_refused`, `claim_mismatch`,
    /// `retry_after`, …).
    GuardFailed { id: &'static str, guard: Guard },
}

/// Look a transition up by the id another section cites.
pub fn get(id: &str) -> Option<&'static Transition> {
    TRANSITIONS.iter().find(|t| t.id == id)
}

/// The ledger's question before it writes: *may I perform `id` on this row?*
///
/// Naming the move rather than scanning is deliberate — several rows share a
/// from-state (T14 and T15 from `claimed`; T4 and T6 from `delivering`), and a
/// scan would have to pick a winner by table order, which is the kind of
/// hidden precedence this module exists to remove.
pub fn check(id: &str, from: MailState, f: &Facts) -> Result<&'static Transition, Illegal> {
    let t = get(id).ok_or(Illegal::Unknown("transition id"))?;
    if t.allows(from, f) {
        return Ok(t);
    }
    // Refused: name the piece that refused, because "illegal" is not an action
    // an agent can take.
    if !t.from.contains(&from) {
        return Err(Illegal::WrongState {
            id: t.id,
            from,
            allowed: t.from,
        });
    }
    Err(Illegal::GuardFailed {
        id: t.id,
        guard: t.guard,
    })
}

/// The promotions a claim runs before it takes anything, in order: close the
/// reconcile, then apply the cap, then take. Read tools run only the
/// ledger-only subset — [`promotion`] — because T6 needs the dataset and a
/// read tool must never write across databases.
pub const CLAIM_PATH: &[&str] = &["T6", "T12"];

/// The take is T3 or T4 by the row's state: a `queued` row, or a lapsed
/// `claimed`/`delivering` one.
pub fn take_id(from: MailState) -> Option<&'static str> {
    match from {
        Queued => Some("T3"),
        Claimed | Delivering => Some("T4"),
        WaitingUser | Delivered | Cancelled | Failed => None,
    }
}

/// The forward promotions that need nothing but the ledger, so any read may
/// run them: the answer deadline (T11) and the cap (T12).
pub fn promotion(from: MailState, f: &Facts) -> Option<&'static Transition> {
    TRANSITIONS
        .iter()
        .filter(|t| t.actor == Actor::Clock && t.from.contains(&from))
        .find(|t| t.guard.holds(from, f))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: &[MailState] = &[
        Queued,
        Claimed,
        Delivering,
        WaitingUser,
        Delivered,
        Cancelled,
        Failed,
    ];
    const ACTORS: &[Actor] = &[
        Actor::App,
        Actor::Agent,
        Actor::Holder,
        Actor::User,
        Actor::Clock,
    ];

    /// Facts that satisfy one guard and nothing else — except where a guard
    /// deliberately ignores a column (T4 does not test `artifact_landed`
    /// because T6 owns that question, and ordering in the *ledger* decides).
    fn facts_for(guard: Guard) -> Facts {
        let mut f = Facts::default();
        match guard {
            Guard::Always => {}
            Guard::ClaimMatches => f.claim_matches = true,
            Guard::ClaimAndPayload => {
                f.claim_matches = true;
                f.payload_valid = true;
            }
            Guard::ClaimAndParent => {
                f.claim_matches = true;
                f.parent_matches = true;
            }
            Guard::LapsedAndFree => {
                f.capacity_free = true;
            }
            Guard::CapacityFree => {
                f.capacity_free = true;
            }
            Guard::FreeAndAtCap => {
                f.capacity_free = true;
                f.attempts_at_cap = true;
            }
            Guard::LapsedAndArtifact => {
                f.artifact_landed = true;
            }
            Guard::BeforeDeadline => f.before_deadline = true,
            Guard::DeadlinePassed => {}
            Guard::SubjectGone => {}
        }
        f
    }

    #[test]
    fn ids_are_unique_and_ordered() {
        let mut seen = Vec::new();
        for t in TRANSITIONS {
            assert!(!seen.contains(&t.id), "duplicate id {}", t.id);
            seen.push(t.id);
        }
        for (i, t) in TRANSITIONS.iter().enumerate() {
            assert_eq!(t.id, format!("T{}", i + 1), "table must stay in id order");
        }
    }

    /// Every row is reachable from every state it names, and from those states
    /// only. This is the direction that catches the round-4 bug: a row whose
    /// `from` list omits a state the prose says it covers (T4 and `delivering`).
    #[test]
    fn each_row_is_exactly_reachable() {
        for t in TRANSITIONS {
            let f = facts_for(t.guard);
            for state in STATES {
                let got = check(t.id, *state, &f);
                if t.from.contains(state) {
                    let got = got.unwrap_or_else(|e| {
                        panic!("{} should apply from {:?}: {:?}", t.id, state, e)
                    });
                    assert_eq!(got.to, t.to);
                } else {
                    assert_eq!(
                        got.err(),
                        Some(Illegal::WrongState {
                            id: t.id,
                            from: *state,
                            allowed: t.from,
                        }),
                        "{} must not apply from {:?}",
                        t.id,
                        state
                    );
                }
            }
        }
    }

    /// The property test proper: for every (state, actor) pair, the answer is
    /// the listed move or a refusal. A pair with no row is refused under *any*
    /// facts, so "a bystander cannot answer" and "a clock cannot rewind" are
    /// assertions rather than claims.
    #[test]
    fn every_actor_from_every_state() {
        // Two fact sets that between them flip every guard: all true, all false.
        let mut all_on = Facts {
            subject_resolves: true,
            before_deadline: true,
            ..Facts::default()
        };
        all_on.lease_live = false;
        all_on.claim_matches = true;
        all_on.capacity_free = true;
        all_on.attempts_at_cap = true;
        all_on.payload_valid = true;
        all_on.parent_matches = true;
        all_on.artifact_landed = true;
        let all_off = Facts {
            lease_live: true,
            subject_resolves: true,
            before_deadline: false,
            ..Facts::default()
        };

        for state in STATES {
            for actor in ACTORS {
                let listed: Vec<&Transition> = TRANSITIONS
                    .iter()
                    .filter(|t| t.actor == *actor && t.from.contains(state))
                    .collect();
                if listed.is_empty() {
                    assert!(
                        promotion(*state, &all_on).is_none() || *actor != Actor::Clock,
                        "{:?} + {:?}: no row, yet a promotion fires",
                        state,
                        actor
                    );
                    for t in TRANSITIONS {
                        assert!(
                            !t.allows(*state, &all_on) || t.actor != *actor,
                            "{:?} + {:?}: phantom row {}",
                            state,
                            actor,
                            t.id
                        );
                        assert!(!t.allows(*state, &all_off) || t.actor != *actor);
                    }
                } else {
                    // At least one listed row must be performable once its
                    // guard is satisfied.
                    let reachable = listed
                        .iter()
                        .any(|t| check(t.id, *state, &facts_for(t.guard)).is_ok());
                    assert!(reachable, "{:?} + {:?}: no listed row is performable", state, actor);
                }
            }
        }
    }

    /// Terminal means what it says: nothing leaves it, whoever asks and
    /// whatever the facts.
    #[test]
    fn terminal_states_are_absorbing() {
        for state in STATES.iter().filter(|s| s.is_terminal()) {
            for actor in ACTORS {
                for t in TRANSITIONS {
                    assert!(
                        !t.from.contains(state),
                        "{} lists terminal {:?}",
                        t.id,
                        state
                    );
                    assert!(!t.allows(*state, &Facts::default()) || t.actor != *actor);
                }
                assert!(promotion(*state, &Facts::default()).is_none());
            }
        }
    }

    /// A hold is not a debt and a hold is not an answer: nobody but the user
    /// moves a `waiting_user` row forward by answering it, and `delivering` has
    /// no user row at all.
    #[test]
    fn waiting_user_and_delivering_are_not_bystander_territory() {
        for t in TRANSITIONS {
            if t.from.contains(&WaitingUser) {
                assert!(
                    matches!(t.actor, Actor::User | Actor::Clock | Actor::App),
                    "{}: {:?} may act on waiting_user",
                    t.id,
                    t.actor
                );
            }
            if t.from.contains(&Delivering) {
                assert_ne!(t.actor, Actor::User, "{}: the user has no delivering row", t.id);
            }
        }
    }

    /// The clock only ever ends a row and never touches `attempts`.
    #[test]
    fn the_clock_never_rewinds() {
        for t in TRANSITIONS.iter().filter(|t| t.actor == Actor::Clock) {
            assert_eq!(t.delta, AttemptDelta::Same, "{}: the clock changed attempts", t.id);
            assert!(
                matches!(t.to, Failed | Delivered),
                "{}: the clock moved a row to {:?}",
                t.id,
                t.to
            );
        }
    }

    /// Taking work is the only charge; the only refunds are the three the
    /// document names — a question (T9), a takeover (T14), and `unfit` (T13).
    #[test]
    fn attempts_are_charged_once_and_refunded_thrice() {
        let charged: Vec<&str> = TRANSITIONS
            .iter()
            .filter(|t| t.delta == AttemptDelta::Charge)
            .map(|t| t.id)
            .collect();
        assert_eq!(charged, vec!["T3", "T4"]);

        let refunded: Vec<&str> = TRANSITIONS
            .iter()
            .filter(|t| t.delta == AttemptDelta::Refund)
            .map(|t| t.id)
            .collect();
        assert_eq!(refunded, vec!["T9", "T14"]);

        let if_unfit: Vec<&str> = TRANSITIONS
            .iter()
            .filter(|t| t.delta == AttemptDelta::RefundIfUnfit)
            .map(|t| t.id)
            .collect();
        assert_eq!(if_unfit, vec!["T13"]);

        // A claim charges even when it re-takes a lapsed row: retries cost.
        assert_eq!(AttemptDelta::Charge.apply(&Facts::default()), 1);
        assert_eq!(AttemptDelta::RefundIfUnfit.apply(&Facts::default()), 0);
        let unfit = Facts {
            release_unfit: true,
            ..Facts::default()
        };
        assert_eq!(AttemptDelta::RefundIfUnfit.apply(&unfit), -1);
    }

    /// Close, then cap, then take — the order §4.1 makes load-bearing. Without
    /// the close first, a third attempt that *did* write is failed instead of
    /// delivered.
    #[test]
    fn the_claim_path_closes_before_it_caps() {
        assert_eq!(CLAIM_PATH, &["T6", "T12"]);
        assert_eq!(get(CLAIM_PATH[0]).unwrap().to, Delivered);
        assert_eq!(get(CLAIM_PATH[1]).unwrap().to, Failed);
        assert_eq!(take_id(Queued), Some("T3"));
        assert_eq!(take_id(Claimed), Some("T4"));
        assert_eq!(take_id(Delivering), Some("T4"));
        for state in [WaitingUser, Delivered, Cancelled, Failed] {
            assert_eq!(take_id(state), None, "{:?} must not be takeable", state);
        }
    }

    /// A read tool may run the promotions and nothing else: `promotion` offers
    /// only ledger-only rows, never the reconcile that needs the dataset.
    #[test]
    fn reads_promote_only_in_ledger() {
        for t in TRANSITIONS.iter().filter(|t| t.actor == Actor::Clock) {
            for state in t.from {
                let f = facts_for(t.guard);
                assert_eq!(promotion(*state, &f).map(|p| p.id), Some(t.id));
            }
        }
        // A lapsed claim under the cap is untouched: it is still work.
        let under_cap = Facts {
            capacity_free: true,
            ..Facts::default()
        };
        assert!(promotion(Claimed, &under_cap).is_none());
        assert!(promotion(Delivering, &under_cap).is_none());
    }

    /// The one move a caller cannot get by scanning: the reconcile and the cap
    /// are indistinguishable by (state, actor) from a take, which is why the
    /// ledger names the id instead of asking "what applies here?".
    #[test]
    fn a_named_move_is_not_a_scan() {
        let lapsed_and_landed = Facts {
            artifact_landed: true,
            capacity_free: true,
            ..Facts::default()
        };
        assert!(check("T6", Delivering, &lapsed_and_landed).is_ok());
        assert!(check("T4", Delivering, &lapsed_and_landed).is_ok());
        assert!(check("T7", Delivering, &lapsed_and_landed).is_err());
        assert_eq!(
            check("T99", Queued, &Facts::default()).err(),
            Some(Illegal::Unknown("transition id"))
        );
    }
}
