//! Every model's measurement of one attempt.
//!
//! The same recording scored by two speech models does not give two versions of
//! one number: it gives two different numbers, often tens of points apart, and
//! the learner cannot tell from `92` that another model would have said `61`.
//! A measurement is therefore recorded *per model* rather than overwriting the
//! last one, and the attempt's final score is the best of them.
//!
//! What is stored is one row per model with its three headline numbers — not a
//! whole assessment per model. The stored card (`metrics_json`, see
//! [`super::assess`]) stays a single picture: the best measurement's, the one
//! the score and the XP are earned from. Keeping five of those would multiply
//! the largest field of a journalled row by five for detail nobody compares.
//!
//! ## The one rule that makes the list meaningful
//!
//! A score is only comparable to another score measured against the same
//! reference, so the list carries the digest of the reference it was measured
//! against ([`super::standard_audio::text_digest`]). When the text's effective
//! reference changes, the list is not re-scaled and not quietly kept — it is
//! *restarted* by the next measurement, exactly as a standard clip refuses to
//! score against an older text. Numbers that cannot be compared must not be
//! ranked against each other.

use serde::{Deserialize, Serialize};

/// How many models one attempt keeps measurements for. Chosen by the row
/// budget, not by taste: an entry is ~110 bytes, so twelve stay well inside
/// the ~2 KB an attempt's `metrics_json` is already allowed (D12), and the list
/// is sorted best-first — a thirteenth model costs the *worst* entry its place,
/// never the best.
pub(crate) const MAX_MEASUREMENTS: usize = 12;

/// The model name of a measurement taken before a model was ever recorded.
/// Older rows have a score and no idea which engine produced it; seeding that
/// number as an entry is what stops a later, worse measurement from silently
/// replacing the best score the learner already earned.
pub(crate) const UNRECORDED_MODEL: &str = "unrecorded";

/// One model's measurement of one attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Measurement {
    /// The STT model whose transcript this was measured from. Its catalog id,
    /// which is also what [`Measurements::record`] keys the list on: measuring
    /// the same model again replaces its row instead of adding a second one.
    pub model: String,
    /// Accuracy 0-100 — the number the final score is chosen on.
    pub score: f64,
    /// Correct words per minute, when something placed the take in time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wcpm: Option<f64>,
    /// Pronunciation 0-100, when the phoneme pass ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronunciation: Option<f64>,
    /// `"hub"` when another device ran the model. The same fact `metrics_json`
    /// carries as `provenance`, repeated here so a breakdown can be read
    /// without opening the stored card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// When this was measured, in the same canonical stamp every row uses.
    pub measured_at: String,
}

/// The stored list: every measurement of one attempt, and the reference all of
/// them were scored against.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Measurements {
    /// [`super::standard_audio::text_digest`] of the effective reference. Empty
    /// on a row written before the digest existed, which counts as
    /// incomparable: the next measurement restarts the list.
    #[serde(default)]
    pub reference: String,
    /// Best score first, at most [`MAX_MEASUREMENTS`] entries.
    #[serde(default)]
    pub entries: Vec<Measurement>,
}

impl Measurements {
    /// Read the stored text back. Unparseable text is an empty list rather than
    /// a failed attempt listing — the same rule `stored_metrics` applies to the
    /// assessment card.
    pub(crate) fn parse(raw: Option<&str>) -> Measurements {
        raw.and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// The text to store, or `None` when there is nothing to store — so an
    /// attempt measured by one model and then seen by an older peer still looks
    /// exactly as it did before this list existed.
    pub(crate) fn to_text(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        serde_json::to_string(self).ok()
    }

    /// The measurements that mean something today: none, when the reference they
    /// were scored against is not the one in force. Every score the UI then
    /// shows — the attempt's own, which is re-scored from its transcript on
    /// read — refers to the same text.
    pub(crate) fn comparable(&self, digest: &str) -> &[Measurement] {
        if self.entries.is_empty() || self.reference != digest {
            return &[];
        }
        &self.entries
    }

    /// Add one measurement, replacing the earlier one from the same model, and
    /// report whether the new one is now the best.
    ///
    /// `true` means the attempt's headline picture — transcript, score, stored
    /// card — should become this measurement's, because the final score is
    /// defined as the best one. `false` means the list grew and the picture
    /// stays: measuring with a weaker model adds information, it never removes
    /// a score the learner already earned.
    pub(crate) fn record(&mut self, digest: &str, fresh: Measurement) -> bool {
        let model = fresh.model.clone();
        if self.reference != digest {
            // Different reference: nothing in the old list is comparable, so it
            // is replaced rather than ranked. This is also how a row written
            // before the digest existed (empty) starts its first real list.
            *self = Measurements::default();
            self.reference = digest.to_string();
        }
        match self.entries.iter_mut().find(|m| m.model == fresh.model) {
            Some(slot) => *slot = fresh,
            None => self.entries.push(fresh),
        }
        // Best first; the newest wins a tie, so a re-measure moves to the top.
        self.entries.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.measured_at.cmp(&a.measured_at))
        });
        self.entries.truncate(MAX_MEASUREMENTS);
        // The fresh row is the attempt's picture exactly when it sits first:
        // same model, because its entry was just overwritten with these numbers.
        self.entries.first().is_some_and(|m| m.model == model)
    }

    /// The attempt's final score: the best measurement of it. `None` when there
    /// is no comparable list, in which case the caller's own number stands.
    pub(crate) fn best_score(&self, digest: &str) -> Option<f64> {
        self.comparable(digest).first().map(|m| m.score)
    }

    /// The stored measurement a legacy row implies: its score, no model name.
    /// Seeding it is what keeps "best is final" true across the upgrade instead
    /// of letting the next, possibly worse, measurement take the headline.
    pub(crate) fn seed(&mut self, digest: &str, score: f64, measured_at: &str) {
        if !self.entries.is_empty() {
            return;
        }
        self.record(
            digest,
            Measurement {
                model: UNRECORDED_MODEL.to_string(),
                score,
                wcpm: None,
                pronunciation: None,
                via: None,
                measured_at: measured_at.to_string(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement(model: &str, score: f64, at: &str) -> Measurement {
        Measurement {
            model: model.to_string(),
            score,
            wcpm: Some(120.0),
            pronunciation: None,
            via: None,
            measured_at: at.to_string(),
        }
    }

    #[test]
    fn a_first_measurement_starts_the_list() {
        let mut m = Measurements::default();
        assert!(m.record("ref1", measurement("parakeet", 92.0, "2026-01-01 00:00:00")));
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.reference, "ref1");
    }

    #[test]
    fn a_better_model_becomes_the_headline_and_sits_first() {
        let mut m = Measurements::default();
        m.record("ref1", measurement("whisper", 61.0, "2026-01-01 00:00:00"));
        assert!(
            m.record("ref1", measurement("parakeet", 92.0, "2026-01-02 00:00:00")),
            "the better new measurement is the best one"
        );
        assert_eq!(m.entries[0].model, "parakeet");
        assert_eq!(m.best_score("ref1"), Some(92.0));
    }

    #[test]
    fn a_worse_model_adds_a_row_and_keeps_the_headline() {
        let mut m = Measurements::default();
        m.record("ref1", measurement("parakeet", 92.0, "2026-01-01 00:00:00"));
        assert!(
            !m.record("ref1", measurement("whisper", 61.0, "2026-01-02 00:00:00")),
            "a weaker measurement must not become the attempt's picture"
        );
        assert_eq!(m.entries.len(), 2, "but it is still saved");
        assert_eq!(m.best_score("ref1"), Some(92.0));
    }

    #[test]
    fn measuring_the_same_model_again_replaces_its_row() {
        let mut m = Measurements::default();
        m.record("ref1", measurement("parakeet", 92.0, "2026-01-01 00:00:00"));
        m.record("ref1", measurement("parakeet", 55.0, "2026-01-02 00:00:00"));
        assert_eq!(m.entries.len(), 1, "one row per model, not one per press");
        assert_eq!(m.best_score("ref1"), Some(55.0), "the newest answer for that model");
    }

    #[test]
    fn a_changed_reference_restarts_the_list() {
        let mut m = Measurements::default();
        m.record("ref1", measurement("parakeet", 92.0, "2026-01-01 00:00:00"));
        m.record("ref2", measurement("whisper", 61.0, "2026-01-02 00:00:00"));
        assert_eq!(m.entries.len(), 1, "the old number scores a different text");
        assert_eq!(m.best_score("ref2"), Some(61.0));
        assert!(
            m.best_score("ref1").is_none(),
            "and is not merely hidden — it cannot win a comparison"
        );
    }

    #[test]
    fn a_stale_list_shows_nothing_rather_than_nothing_comparable() {
        let mut m = Measurements::default();
        m.record("ref1", measurement("parakeet", 92.0, "2026-01-01 00:00:00"));
        assert_eq!(m.comparable("ref1").len(), 1);
        assert!(m.comparable("ref2").is_empty());
        assert!(m.comparable("").is_empty());
    }

    #[test]
    fn the_list_keeps_the_best_models_when_it_fills_up() {
        let mut m = Measurements::default();
        for i in 0..(MAX_MEASUREMENTS + 3) {
            m.record("ref1", measurement(&format!("m{i}"), 50.0 + i as f64, "2026-01-01 00:00:00"));
        }
        assert_eq!(m.entries.len(), MAX_MEASUREMENTS);
        assert_eq!(m.entries[0].model, format!("m{}", MAX_MEASUREMENTS + 2));
        // Scores run 50.0..=64.0; the twelve kept are 64.0 down to 53.0.
        assert_eq!(m.entries.last().unwrap().score, 53.0);
        assert_eq!(
            m.entries.last().unwrap().model,
            "m3",
            "the dropped ones are the weakest, never the newest"
        );
    }

    #[test]
    fn a_legacy_score_is_seeded_once_so_the_best_survives_the_upgrade() {
        let mut m = Measurements::default();
        m.seed("ref1", 85.0, "2025-12-01 00:00:00");
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].model, UNRECORDED_MODEL);
        // A worse measurement then keeps the attempt's 85 rather than replacing it.
        assert!(!m.record("ref1", measurement("whisper", 61.0, "2026-01-01 00:00:00")));
        assert_eq!(m.best_score("ref1"), Some(85.0));
        // Seeding an already-populated list is a no-op.
        m.seed("ref1", 10.0, "2026-01-02 00:00:00");
        assert_eq!(m.best_score("ref1"), Some(85.0));
    }

    #[test]
    fn an_empty_list_stores_nothing_and_bad_text_reads_as_empty() {
        assert_eq!(Measurements::default().to_text(), None);
        assert!(Measurements::parse(Some("not json at all")).entries.is_empty());
        assert!(Measurements::parse(None).entries.is_empty());
    }

    #[test]
    fn the_list_round_trips_through_the_stored_text() {
        let mut m = Measurements::default();
        m.record("ref1", measurement("parakeet", 92.0, "2026-01-01 00:00:00"));
        m.record("ref1", measurement("whisper", 61.0, "2026-01-02 00:00:00"));
        let text = m.to_text().unwrap();
        let back = Measurements::parse(Some(&text));
        assert_eq!(back, m);
        // A row a list fits inside the budget the attempt already has.
        assert!(text.len() < 600, "two entries are ~110 bytes each, got {}", text.len());
    }

}
