//! Family-keyed dataset ids: the grammar `<type-slug>_<family-key>[_<disc>]`,
//! the one place that mints and parses it, and the ≤5-probe family lookup.
//!
//! A dataset's `uuid` is an opaque slug (see [`super::info`]) whose only rule is
//! global uniqueness. That opacity is what lets an id carry a *relation*: several
//! datasets built from the same book share a family key, so the dictation set of
//! `book_a1b2c3d4e5f6` is `dictation_a1b2c3d4e5f6` and its card deck
//! `card_a1b2c3d4e5f6` — computable from either side with no scan, no schema
//! field and nothing to keep in sync when one of them moves. It generalizes the
//! `cards_of_book_<uuid>` convention, which also hardcoded the *source* type in
//! the prefix and so could not describe a deck derived from a dictation set.
//!
//! Design rules, each with a reason:
//!
//! * **The prefix is advisory, never authoritative.** Nothing here may be used to
//!   decide what a dataset *is*: the directory slug and `info.type` remain the
//!   routing authority ([`super::find_dataset_dir_typed`] warns and skips on
//!   mismatch). A dataset that was re-typed in place therefore keeps a stale
//!   prefix that is cosmetic, and [`resolve_family`] reports the type it was
//!   *found* under rather than the one the id claims.
//! * **A family is a relation, not an entity.** There is no registry file, no
//!   app-DB table and no `info.json` field for it: a family exists exactly when
//!   some dataset's id parses to it. So there is nothing to administer, and no
//!   second source of truth to fall out of step with the ids.
//! * **Legacy ids never parse.** Random uuids, the website-era slugs
//!   (`de_a1_by_system`), the reserved `dictation-favorites` and the retired
//!   `cards_of_book_…` all yield `None` and behave as a family of one, which is
//!   why adopting this scheme needs no migration: an old dataset is simply
//!   unaffiliated until something re-keys it.
//! * **Parsing is strict so it stays safe.** The key is lowercase hex with no
//!   `_`, which is what makes `read_aloud_…` unambiguous despite the slug itself
//!   containing an underscore, and what keeps an ordinary slug-shaped id from
//!   being read as a family.
//! * **1:1 per (family, type)** is the default and the only thing
//!   [`mint_into`] produces. The third segment is reserved for the day a book
//!   needs two decks; nothing mints one yet, so [`resolve_family`] does not look
//!   for one either.

use std::path::PathBuf;

use uuid::Uuid;

use crate::settings::SettingsState;

use super::{find_dataset_dir_typed, DatasetType};

/// How many hex chars a freshly minted family key carries (48 bits).
///
/// Twelve, not the 32 of a full uuid v4, because the key is paid five times
/// over: it sits inside *every* member's id, and those ids appear in
/// `/api/v1/datasets/{uuid}/…`, in a follower's snapshot path
/// `<datasets>/<type>/<uuid>/`, in `dataset_sync_state`, in `xp_ledger`, in log
/// lines and in MCP parameters an agent has to echo back. `dictation_<key>` is
/// 22 chars instead of 42 for no loss of safety — the collision probability for
/// `n` families is about `n²/2^49`, i.e. ~10⁻⁹ at a thousand families and ~10⁻⁷
/// at ten thousand, orders of magnitude past what one learner's library reaches.
///
/// The failure mode to keep in mind is that a key collision is *not* caught by
/// [`super::assert_uuid_free`]: two unrelated works sharing a key would silently
/// merge into one family, and the repair is re-minting one of them.
///
/// The grammar still accepts up to [`KEY_MAX`] chars, so a full uuid remains a
/// legal key — which is what a migration script reusing an existing uuid as the
/// key produces.
pub(crate) const FAMILY_KEY_LEN: usize = 12;

/// Shortest key [`is_valid_key`] accepts. Below this a key is more likely a
/// fragment of a hand-written id than a family.
const KEY_MIN: usize = 8;

/// Longest key accepted: a dashless uuid v4, so an existing id can be adopted as
/// a family key without truncation.
const KEY_MAX: usize = 32;

/// A fresh family key: the head of a uuid v4, hex only (see [`FAMILY_KEY_LEN`]).
pub(crate) fn new_family_key() -> String {
    Uuid::new_v4().simple().to_string()[..FAMILY_KEY_LEN].to_string()
}

/// True for a well-formed family key: 8–32 lowercase hex chars, no `_`.
///
/// The charset is the load-bearing part — it is what makes the parse in
/// [`split`] unambiguous, since the slug `read_aloud` contains the separator.
pub(crate) fn is_valid_key(key: &str) -> bool {
    (KEY_MIN..=KEY_MAX).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() && (b'a'..=b'f').contains(&b))
}

/// The id of `ty`'s dataset in `family`. Pure string work — no lookup, which is
/// the whole point of the scheme.
pub(crate) fn mint(ty: DatasetType, family: &str) -> String {
    format!("{}_{}", ty.as_str(), family)
}

/// A brand-new family of one.
pub(crate) fn mint_new(ty: DatasetType) -> String {
    mint(ty, &new_family_key())
}

/// Parse an id into the type its prefix claims and its family key, or `None` when
/// it does not follow the grammar (every id minted before this scheme).
///
/// A discriminator (`card_<key>_2`) is dropped: the family is the part before it.
/// The longest matching slug wins, so a slug that is a prefix of another cannot
/// steal the parse.
pub(crate) fn split(uuid: &str) -> Option<(DatasetType, &str)> {
    let mut best: Option<(usize, DatasetType, &str)> = None;
    for ty in DatasetType::ALL {
        let Some(rest) = uuid.strip_prefix(ty.as_str()).and_then(|r| r.strip_prefix('_')) else {
            continue;
        };
        let key = rest.split('_').next().unwrap_or("");
        if !is_valid_key(key) {
            continue;
        }
        let len = ty.as_str().len();
        if best.map_or(true, |(best_len, ..)| len > best_len) {
            best = Some((len, ty, key));
        }
    }
    best.map(|(_, ty, key)| (ty, key))
}

/// The family an id belongs to, or `None` for a legacy/unaffiliated dataset.
pub(crate) fn family_of(uuid: &str) -> Option<&str> {
    split(uuid).map(|(_, key)| key)
}

/// The 8-char disambiguator the directory-name fallbacks append when a folder
/// name is taken.
///
/// Replaces the `&uuid[..8]` slices in [`super::book`] and
/// [`super::read_aloud`], which on a prefixed id would cut `"read_alo"` out of
/// `read_aloud_…` and `"dictatio"` out of `dictation_…` — the same suffix for
/// every dataset of that type, i.e. no disambiguation at all. Taking the *key's*
/// head keeps it per-family, and `chars().take` cannot panic on a short id.
pub(crate) fn short(uuid: &str) -> String {
    family_of(uuid).unwrap_or(uuid).chars().take(8).collect()
}

/// The datasets that carry `family`, as `(type it was found under, uuid, dir)`.
///
/// Probes the five candidate ids through [`find_dataset_dir_typed`] rather than
/// scanning every root: at most five lookups, cross-platform, and it works for a
/// family whose members live in different linked directories. An invalid key
/// resolves to nothing rather than erroring, so a listing can call this on every
/// id it has.
///
/// A discriminated sibling (`card_<key>_2`) is *not* found here — nothing mints
/// one yet, and finding it would mean enumerating suffixes that do not exist.
pub(crate) fn resolve_family(
    settings: &SettingsState,
    family: &str,
) -> Vec<(DatasetType, String, PathBuf)> {
    if !is_valid_key(family) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for ty in DatasetType::ALL {
        let id = mint(ty, family);
        // The directory decides the type, not the prefix: `found` is what the
        // dataset was discovered as, which is the authoritative answer.
        if let Ok((path, found)) = find_dataset_dir_typed(settings, &id) {
            out.push((found, id, path));
        }
    }
    out
}

/// Refuse a family key nothing uses — the typo guard on mint-into-family.
///
/// Without it, `card_aaaab` (one char short of `card_aaaaaa`) would silently
/// start a *second* family that looks affiliated in the UI but is not.
pub(crate) fn family_resolves(settings: &SettingsState, family: &str) -> Result<(), String> {
    if !is_valid_key(family) {
        return Err(format!(
            "'{}' is not a family key: expected {}-{} lowercase hex characters (an id looks like '{}')",
            family,
            KEY_MIN,
            KEY_MAX,
            mint(DatasetType::Book, "a1b2c3d4e5f6")
        ));
    }
    if resolve_family(settings, family).is_empty() {
        let tried: Vec<String> = DatasetType::ALL.iter().map(|t| mint(*t, family)).collect();
        return Err(format!(
            "No dataset belongs to family '{}' — tried {}. Copy the key from an existing \
             dataset's id (the part after '<type>_'), or omit `family` to start a new family.",
            family,
            tried.join(", ")
        ));
    }
    Ok(())
}

/// Mint the id for a new dataset of `ty`, either into an existing `family` or
/// into a fresh one.
///
/// This is the single funnel every creator goes through, so the three rules live
/// here once instead of being re-derived per type:
///
/// 1. the key is well formed and at least one dataset already uses it;
/// 2. the family has no member of this type yet — one dataset per type per
///    family, so two decks of one book need the reserved discriminator rather
///    than a second `card_<key>`;
/// 3. the caller still runs [`super::assert_uuid_free`] on the result, which is
///    what rejects the reserved Favorites id and any cross-type collision.
pub(crate) fn mint_into(
    settings: &SettingsState,
    ty: DatasetType,
    family: Option<&str>,
) -> Result<String, String> {
    let Some(family) = family.map(str::trim).filter(|f| !f.is_empty()) else {
        return Ok(mint_new(ty));
    };
    family_resolves(settings, family)?;
    if let Some((_, uuid, path)) = resolve_family(settings, family)
        .into_iter()
        .find(|(found, ..)| *found == ty)
    {
        return Err(format!(
            "Family '{}' already has a {} dataset ('{}' at '{}') — one dataset per type per \
             family. Open that one instead, or give the new dataset a discriminator id \
             ('{}') if it really is a second {} of the same work.",
            family,
            ty.as_str(),
            uuid,
            path.display(),
            format!("{}_2", mint(ty, family)),
            ty.as_str()
        ));
    }
    Ok(mint(ty, family))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of exactly the minted length, reused so the expectations below are
    /// readable.
    const KEY: &str = "a1b2c3d4e5f6";

    #[test]
    fn minted_keys_are_well_formed() {
        let key = new_family_key();
        assert_eq!(key.len(), FAMILY_KEY_LEN, "{key}");
        assert!(is_valid_key(&key), "{key}");
        assert!(key.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    /// Every type round-trips, including `read_aloud`, whose slug contains the
    /// separator — the case the hex-only key charset exists for.
    #[test]
    fn every_type_round_trips() {
        for ty in DatasetType::ALL {
            let id = mint(ty, KEY);
            assert_eq!(id, format!("{}_{}", ty.as_str(), KEY));
            assert_eq!(split(&id), Some((ty, KEY)), "{id}");
            assert_eq!(family_of(&id), Some(KEY));
            // The sibling's id is pure string work — the reason the scheme exists.
            assert_eq!(mint(DatasetType::Card, family_of(&id).unwrap()), mint(DatasetType::Card, KEY));
        }
    }

    /// Ids that predate the scheme must read as unaffiliated, not as a family
    /// with a nonsense key — including the two that *look* like a prefix.
    #[test]
    fn legacy_and_reserved_ids_do_not_parse() {
        for id in [
            "a3a033d207c84b9a88d97452a9c3f7b1", // a dashless uuid v4
            "b76a0a93-20e0-4d1a-bd16-ab6f18197952", // a dashed one
            "de_a1_by_system",                  // website-era slug
            "dictation-favorites",              // the reserved Favorites id
            "cards_of_book_a3a033d207c84b9a88d97452a9c3f7b1", // the retired convention
            "card_",                            // prefix with no key
            "",
        ] {
            assert_eq!(split(id), None, "{id} should not parse");
            assert_eq!(family_of(id), None, "{id}");
        }
    }

    /// `cards_of_book_…` in particular must not read as a `card` family: the
    /// slug is followed by `s`, not the separator.
    #[test]
    fn the_retired_convention_is_not_a_card_family() {
        assert_eq!(family_of("cards_of_book_a1b2c3d4e5f6"), None);
    }

    /// The reserved third segment is the escape hatch for a second deck of one
    /// book: it is part of the id but not part of the family.
    #[test]
    fn a_discriminator_is_dropped_from_the_key() {
        assert_eq!(split(&format!("card_{KEY}_2")), Some((DatasetType::Card, KEY)));
        assert_eq!(family_of(&format!("card_{KEY}_v2")), Some(KEY));
    }

    /// The key charset and bounds are what keep an arbitrary slug from being
    /// read as a family.
    #[test]
    fn key_shape_is_enforced() {
        assert!(is_valid_key(KEY));
        assert!(is_valid_key("abcdef01"), "8 chars is the floor");
        assert!(is_valid_key(&"0".repeat(KEY_MAX)), "a full uuid v4 is legal");
        assert!(!is_valid_key("abcdef0"), "7 chars");
        assert!(!is_valid_key(&"0".repeat(KEY_MAX + 1)), "33 chars");
        assert!(!is_valid_key("ABCDEF012345"), "uppercase");
        assert!(!is_valid_key("a1b2c3d4e5g6"), "not hex");
        assert!(!is_valid_key("a1b2c3d4e5f"), "odd length is fine, '_' is not");
        assert!(!is_valid_key("a1b2_3d4e5f6"), "the separator cannot appear in a key");
        assert_eq!(split("card_ABCDEF012345"), None);
    }

    /// The longest slug must win, so a future slug that prefixes another cannot
    /// steal the parse.
    #[test]
    fn the_longest_slug_wins() {
        // `read_aloud` (10) beats a hypothetical `read`-shaped prefix: the id
        // parses as the type whose slug is actually followed by '_'.
        assert_eq!(split(&format!("read_aloud_{KEY}")), Some((DatasetType::Read, KEY)));
    }

    /// Directory-name fallbacks append this; on a prefixed id it must come from
    /// the key, not from the slug (which would be the same for every dataset of
    /// that type).
    #[test]
    fn short_takes_the_key_head() {
        assert_eq!(short(&format!("read_aloud_{KEY}")), "a1b2c3d4");
        assert_eq!(short(&format!("dictation_{KEY}")), "a1b2c3d4");
        // A legacy id falls back to its own head, and a short one cannot panic.
        assert_eq!(short("a3a033d207c84b9a88d97452a9c3f7b1"), "a3a033d2");
        assert_eq!(short("abc"), "abc");
    }
}
