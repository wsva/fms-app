//! CTC forced alignment and Goodness-Of-Pronunciation (GOP).
//!
//! The acoustic model emits, per 20 ms frame, a log-probability for every phone
//! in its vocabulary. Free decoding collapses those to one phone string; that
//! string is *not* what pronunciation is measured against, because a
//! well-trained model happily produces the right phones from a wrong
//! realisation (the prior repairs the error — the same reason free-decode text
//! cannot score pronunciation).
//!
//! GOP asks a different question: given that the speaker was *supposed* to say
//! phone `p` at this moment, how much probability mass did the model put on
//! `p` versus on its own best guess? That needs the reference phone sequence
//! pinned onto the frames, which is a CTC forced alignment — hence this module.
//!
//! Everything here is pure and driven by a `[frames][phones]` matrix, so the
//! whole numerical core is unit-testable without a model file.

/// Per-frame log-probabilities plus the derived quantities a GOP pass needs
/// repeatedly. Built once per attempt.
pub(crate) struct LogPost {
    frames: Vec<Vec<f32>>,
    /// `max_q log p(q)` per frame — the denominator of every GOP.
    frame_max: Vec<f32>,
    blank: usize,
}

impl LogPost {
    /// `frames[t][v]` is the log-probability of phone `v` at frame `t`.
    /// Values need not be normalized; only differences within a frame matter.
    pub(crate) fn new(frames: Vec<Vec<f32>>, blank: usize) -> Self {
        let frame_max = frames
            .iter()
            .map(|f| f.iter().copied().fold(f32::NEG_INFINITY, f32::max))
            .collect();
        Self { frames, frame_max, blank }
    }

    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub(crate) fn blank(&self) -> usize {
        self.blank
    }

    /// The log-probabilities of one frame, in vocabulary id order.
    pub(crate) fn row(&self, frame: usize) -> &[f32] {
        self.frames.get(frame).map(|f| f.as_slice()).unwrap_or(&[])
    }

    fn at(&self, frame: usize, phone: usize) -> f32 {
        self.frames
            .get(frame)
            .and_then(|f| f.get(phone))
            .copied()
            .unwrap_or(f32::NEG_INFINITY)
    }

    fn max_at(&self, frame: usize) -> f32 {
        self.frame_max.get(frame).copied().unwrap_or(f32::NEG_INFINITY)
    }
}

/// The frames one reference phone was pinned to: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PhoneSpan {
    pub phone: usize,
    pub start: usize,
    pub end: usize,
}

impl PhoneSpan {
    pub(crate) fn frames(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// Standard CTC forward/backward lattice, restricted to the best path.
///
/// States alternate blank and target phone: `2L + 1` states for `L` phones, and
/// from a phone state the lattice may skip the following blank only when the
/// next phone differs (repeated phones must be separated, or the aligner would
/// merge them into one).
pub(crate) fn forced_align(post: &LogPost, target: &[usize]) -> Option<Vec<PhoneSpan>> {
    if target.is_empty() || post.is_empty() {
        return None;
    }
    let frames = post.len();
    if target.len() > frames {
        // More phones than frames cannot be aligned; the caller reports
        // "too_short" rather than fabricating spans.
        return None;
    }
    let states = 2 * target.len() + 1;
    let blank = post.blank();

    let state_phone = |s: usize| -> Option<usize> {
        if s % 2 == 1 {
            target.get(s / 2).copied()
        } else {
            None
        }
    };

    let mut score = vec![f32::NEG_INFINITY; states];
    let mut back = vec![usize::MAX; frames * states];

    // t = 0: may sit on the leading blank or on the first phone.
    for s in 0..states {
        let emit = match state_phone(s) {
            Some(p) => post.at(0, p),
            None => post.at(0, blank),
        };
        score[s] = if s <= 1 { emit } else { f32::NEG_INFINITY };
    }

    for t in 1..frames {
        let mut next = vec![f32::NEG_INFINITY; states];
        for s in 0..states {
            let emit = match state_phone(s) {
                Some(p) => post.at(t, p),
                None => post.at(t, blank),
            };
            let mut best = score[s];
            let mut from = s;
            if s >= 1 && score[s - 1] > best {
                best = score[s - 1];
                from = s - 1;
            }
            // Skip the in-between blank: allowed only between differing phones.
            if s >= 2 {
                let differs = match (state_phone(s), state_phone(s - 2)) {
                    (Some(a), Some(b)) => a != b,
                    _ => false,
                };
                if differs && score[s - 2] > best {
                    best = score[s - 2];
                    from = s - 2;
                }
            }
            if best > f32::NEG_INFINITY {
                next[s] = best + emit;
                back[t * states + s] = from;
            }
        }
        score = next;
    }

    // End on the trailing blank or the last phone.
    let end_state = if score[states - 1] >= score[states - 2] {
        states - 1
    } else {
        states - 2
    };
    if score[end_state] == f32::NEG_INFINITY {
        return None;
    }

    // Trace back, collecting one frame range per phone state visited.
    let mut spans: Vec<PhoneSpan> = Vec::with_capacity(target.len());
    let mut state = end_state;
    for t in (0..frames).rev() {
        if let Some(phone) = state_phone(state) {
            match spans.last_mut() {
                Some(span) if span.phone == phone && span.start == t + 1 => span.start = t,
                _ => spans.push(PhoneSpan { phone, start: t, end: t + 1 }),
            }
        }
        if t > 0 {
            state = back[t * states + state];
            if state == usize::MAX {
                return None;
            }
        }
    }
    spans.reverse();

    // A phone the aligner never visited cannot be scored. That happens when the
    // recording is far too short for the reference; report it instead of inventing
    // a span (which would silently score a word the speaker never reached).
    if spans.len() != target.len() {
        return None;
    }
    Some(spans)
}

/// Mean GOP over one span: `mean_t[ log p(target) − log max_q p(q) ]`.
///
/// Always ≤ 0 and 0 for a perfectly confident correct phone, which is what
/// makes the exponential mapping in [`pronunciation_score`] monotone.
pub(crate) fn gop(post: &LogPost, span: &PhoneSpan) -> f32 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for t in span.start..span.end.min(post.len()) {
        let target = post.at(t, span.phone);
        let best = post.max_at(t);
        if !target.is_finite() || !best.is_finite() {
            continue;
        }
        sum += target - best;
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

/// Map a GOP into (0, 1] as `exp(gop / tau)`, clamped.
///
/// Monotone in `gop` and bounded, so a single readable number comes out without
/// any fitting. `tau` is the only knob: larger values flatten the curve.
/// Replacing `tau` with a calibrated mapping is future work (it needs teacher
/// labels this project does not have).
pub(crate) fn pronunciation_score(gop: f32, tau: f32) -> f64 {
    if !gop.is_finite() {
        return 0.0;
    }
    let tau = if tau.abs() < 1e-6 { 1.0 } else { tau };
    (gop / tau).exp().clamp(0.0, 1.0) as f64
}

/// Below this a word is reported as weak (highlighted in the diff).
pub(crate) const WEAK_WORD_SCORE: f64 = 0.4;

/// Score the phones belonging to one reference word.
///
/// Duration-weighted: a long vowel the model was unsure about matters more than
/// a 20 ms plosive closure. Words with no phones (or whose phones were never
/// aligned) score `None` rather than 0 — "not measured" is not "wrong".
pub(crate) fn word_score(post: &LogPost, spans: &[PhoneSpan], tau: f32) -> Option<f64> {
    let mut weighted = 0.0;
    let mut total_frames = 0usize;
    for span in spans {
        let frames = span.frames();
        if frames == 0 {
            continue;
        }
        weighted += pronunciation_score(gop(post, span), tau) * frames as f64;
        total_frames += frames;
    }
    if total_frames == 0 {
        None
    } else {
        Some(weighted / total_frames as f64)
    }
}

/// Roll per-word scores up into one attempt-level number.
///
/// Returns `(score, weak_ratio)`. Words worth 3 characters or fewer carry half
/// weight: "a" and "of" are too short for a phone-level judgement to mean
/// anything, and letting them dominate would make the aggregate noise.
pub(crate) fn rollup(words: &[(usize, f64)]) -> Option<(f64, f64)> {
    if words.is_empty() {
        return None;
    }
    let mut weighted = 0.0;
    let mut weight_total = 0.0;
    let mut weak = 0usize;
    for &(ref_len, score) in words {
        let w = if ref_len <= 2 { 0.5 } else { 1.0 };
        weighted += score * w;
        weight_total += w;
        if score < WEAK_WORD_SCORE {
            weak += 1;
        }
    }
    if weight_total <= 0.0 {
        return None;
    }
    Some((weighted / weight_total, weak as f64 / words.len() as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLANK: usize = 0;

    /// Build one frame where `dominant` is 0.0 and everything else is -10.0.
    fn frame(dominant: usize, vocab: usize) -> Vec<f32> {
        (0..vocab)
            .map(|p| if p == dominant { 0.0 } else { -10.0 })
            .collect()
    }

    #[test]
    fn aligns_two_phones_to_their_frames() {
        // blank=0, phone A=1, phone B=2.
        let frames = vec![
            frame(BLANK, 3),
            frame(1, 3),
            frame(2, 3),
            frame(2, 3),
            frame(BLANK, 3),
        ];
        let post = LogPost::new(frames, BLANK);
        let spans = forced_align(&post, &[1, 2]).expect("alignable");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0], PhoneSpan { phone: 1, start: 1, end: 2 });
        assert_eq!(spans[1], PhoneSpan { phone: 2, start: 2, end: 4 });
    }

    #[test]
    fn a_repeated_phone_keeps_two_spans() {
        // Without the "skip only between differing phones" rule these merge.
        let frames = vec![
            frame(BLANK, 3),
            frame(1, 3),
            frame(BLANK, 3),
            frame(1, 3),
            frame(BLANK, 3),
        ];
        let post = LogPost::new(frames, BLANK);
        let spans = forced_align(&post, &[1, 1]).expect("alignable");
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (1, 2));
        assert_eq!((spans[1].start, spans[1].end), (3, 4));
    }

    #[test]
    fn a_mispronounced_phone_is_aligned_anyway_and_scores_low() {
        // Frames 2-3 actually sound like phone 3, but the reference says 2:
        // forced alignment must not "repair" the error by choosing phone 3.
        let frames = vec![
            frame(BLANK, 4),
            frame(1, 4),
            frame(3, 4),
            frame(3, 4),
            frame(BLANK, 4),
        ];
        let post = LogPost::new(frames, BLANK);
        let spans = forced_align(&post, &[1, 2]).expect("alignable");
        assert_eq!(spans[1].phone, 2);
        let mispronounced = gop(&post, &spans[1]);
        assert!((mispronounced - -10.0).abs() < 1e-4, "gop: {}", mispronounced);
        assert!(pronunciation_score(mispronounced, 2.5) < 0.05);
    }

    #[test]
    fn a_correct_phone_scores_one() {
        let frames = vec![frame(1, 3), frame(1, 3)];
        let post = LogPost::new(frames, BLANK);
        let spans = forced_align(&post, &[1]).unwrap();
        assert!((gop(&post, &spans[0])).abs() < 1e-6);
        assert!((pronunciation_score(0.0, 2.5) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn score_is_monotone_and_bounded() {
        // Start above the maximum so the first (gop = 0 → 1.0) comparison is
        // meaningful.
        let mut prev = f64::INFINITY;
        for step in 0..=20 {
            let gop = -(step as f32);
            let s = pronunciation_score(gop, 2.5);
            assert!((0.0..=1.0).contains(&s), "score out of range: {}", s);
            assert!(s <= prev, "must be non-increasing in gop");
            prev = s;
        }
        assert_eq!(pronunciation_score(f32::NEG_INFINITY, 2.5), 0.0);
    }

    #[test]
    fn more_phones_than_frames_or_empty_target_is_none() {
        let post = LogPost::new(vec![frame(BLANK, 3)], BLANK);
        assert!(forced_align(&post, &[1, 2]).is_none());
        assert!(forced_align(&post, &[]).is_none());
        let empty = LogPost::new(Vec::new(), BLANK);
        assert!(forced_align(&empty, &[1]).is_none());
    }

    #[test]
    fn word_score_is_duration_weighted() {
        // A long confident phone and a short hopeless one: the average must sit
        // near the confident one, not halfway.
        let mut frames = vec![frame(1, 3); 9];
        frames.push(frame(2, 3)); // wrong phone for the 10th frame
        let post = LogPost::new(frames, BLANK);
        let spans = forced_align(&post, &[1, 2]).unwrap();
        let score = word_score(&post, &spans, 2.5).unwrap();
        assert!(score > 0.5, "duration weighting should favour the long phone: {score}");
    }

    #[test]
    fn rollup_ignores_nothing_and_reports_weak_ratio() {
        let (score, weak) = rollup(&[(7, 0.9), (8, 0.9), (7, 0.1)]).unwrap();
        assert!(score > 0.5 && score < 0.75, "score: {score}");
        assert!((weak - 1.0 / 3.0).abs() < 1e-9);
        assert!(rollup(&[]).is_none());
    }

    #[test]
    fn short_words_carry_half_weight() {
        let (score, _) = rollup(&[(1, 0.0), (8, 1.0)]).unwrap();
        // 0.5 * 0 + 1.0 * 1.0 over 1.5 total weight.
        assert!((score - 2.0 / 3.0).abs() < 1e-9, "score: {score}");
    }
}
