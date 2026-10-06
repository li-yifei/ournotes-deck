//! All-path enclosures for the native solo score calculator.
//!
//! The recorder supplies a proved deterministic command/query schedule. Possible direct-7021 probe filings and
//! Rush filings are added to it, and [`replay`] encloses the native binary32 factor state of every path through
//! that schedule. A note links its chart-time joint masses once the native frame has filed its lottery-dependent
//! commands. Earlier queries keep support bounds. Combo histories retain every possible prior execution,
//! replacing their hull only on a proved mandatory replay. This is not an exact expectation or search completion.

use super::*;
use crate::live::certified::{F32Interval, F64Interval, I32Interval, ProbabilityMass, rank_mean_bounds};
use crate::live::score::get_luck_factor_percent;
use serde::Serialize;

#[cfg(feature = "search-diagnostics")]
mod profile;
mod replay;
#[cfg(feature = "search-diagnostics")]
pub use profile::{LuckScoreProfile, take_luck_score_profile};

const FIELDS: usize = 6;

pub(super) use replay::ProbeRow;
use replay::{Classes, Replay};

#[derive(Clone, Debug)]
pub(super) enum BoundsEvent {
    Note {
        frame: usize,
        index: usize,
        note: NoteCommand,
    },
    Factor {
        frame: usize,
        command: FactorCommand,
    },
    /// A possible Rush filing in this frame.
    Potential {
        frame: usize,
    },
    /// A possible probe filing at this frame time: every probe row may switch on or off here.
    Probe {
        frame: usize,
        time_ms: i32,
    },
    Query {
        time_ms: i32,
        to: i32,
    },
    /// Exact deterministic combo inputs at a query; emitted only when this note's values change.
    Combo {
        frame: usize,
        index: usize,
        ordinary: f32,
        gekisou: f32,
    },
    /// All lottery-dependent commands at chart times <= this completed native frame have been filed.
    ProbabilityReady(i32),
    Rank {
        range: usize,
        time_ms: i32,
        percent: i64,
        start: Option<usize>,
        end: Option<usize>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct BoundsTrace {
    pub events: Vec<BoundsEvent>,
    pub queries: usize,
    pub frames: usize,
    pub probes: Vec<ProbeRow>,
    pub combo: ComboObserver,
    pub has_luck: bool,
}

/// The filed notes a score query must observe again: every note whose combo inputs may have changed since the
/// previous query. A note outside these frames still has the factors it was last observed with, so the full
/// rescan would emit nothing for it.
#[derive(Clone, Debug, Default)]
pub(super) struct ComboObserver {
    /// Last observed (ordinary, Gekisou) combo factor bits of each filed note, by frame and index.
    seen: Vec<Vec<Option<(u32, u32)>>>,
    /// Every note of the first `consistent` frames was observed at the inputs of the previous query.
    consistent: usize,
    /// Combo judgements recorded before the previous query.
    judgements: usize,
    /// Gekisou combo ranges and versions at the previous query; None before the first query.
    windows: Option<Vec<(i32, i32, u64)>>,
    current: Vec<(i32, i32, u64)>,
    /// Frames that received a note since the previous query.
    filed: Vec<usize>,
    stale: Vec<(usize, usize)>,
}

impl ComboObserver {
    pub(super) fn filed(&mut self, frame: usize) {
        self.filed.push(frame);
    }

    /// Ascending disjoint half-open intervals of the first `frames` frames to observe at this query. `frame_of` is
    /// the monotone frame a note at a chart time is filed in. Return the intervals through [`Self::end`].
    pub(super) fn begin(
        &mut self,
        frames: usize,
        combo: &super::combo::ComboCounter,
        gekisou: Option<&dyn crate::live::score::GekisouComboInfo>,
        frame_of: impl Fn(i32) -> usize,
    ) -> Vec<(usize, usize)> {
        let known = self.consistent.min(frames);
        let mut stale = std::mem::take(&mut self.stale);
        stale.clear();
        stale.push((known, frames));
        // A judgement at t changes the combo read only at times after t.
        let times = combo.judgement_times();
        match times.get(self.judgements..) {
            Some(new) => {
                if let Some(&first) = new.iter().min() {
                    stale.push((frame_of(first), known));
                }
            }
            None => stale.push((0, known)),
        }
        self.judgements = times.len();
        self.current.clear();
        if let Some(gekisou) = gekisou {
            gekisou.combo_windows(&mut self.current);
        }
        match &mut self.windows {
            Some(previous) if previous.len() == self.current.len() => {
                for (old, new) in previous.iter().zip(&self.current) {
                    if old != new {
                        stale.push((frame_of(old.0.min(new.0)), frame_of(old.1.max(new.1)) + 1));
                    }
                }
                std::mem::swap(previous, &mut self.current);
            }
            windows => {
                stale.push((0, known));
                *windows = Some(self.current.clone());
            }
        }
        stale.extend(self.filed.drain(..).map(|frame| (frame, frame + 1)));
        for interval in &mut stale {
            interval.1 = interval.1.min(frames);
        }
        stale.retain(|interval| interval.0 < interval.1);
        stale.sort_unstable();
        let mut merged = 0;
        for i in 0..stale.len() {
            if merged > 0 && stale[i].0 <= stale[merged - 1].1 {
                stale[merged - 1].1 = stale[merged - 1].1.max(stale[i].1);
            } else {
                stale[merged] = stale[i];
                merged += 1;
            }
        }
        stale.truncate(merged);
        self.consistent = frames;
        stale
    }

    pub(super) fn end(&mut self, stale: Vec<(usize, usize)>) {
        self.stale = stale;
    }

    #[cfg(test)]
    pub(super) fn seen(&self, frame: usize, index: usize) -> Option<(u32, u32)> {
        self.seen.get(frame).and_then(|row| row.get(index)).copied().flatten()
    }

    /// Records an observation; true when it differs from the note's previous one.
    pub(super) fn changed(&mut self, frame: usize, index: usize, bits: (u32, u32)) -> bool {
        if self.seen.len() <= frame {
            self.seen.resize_with(frame + 1, Vec::new);
        }
        let row = &mut self.seen[frame];
        if row.len() <= index {
            row.resize(index + 1, None);
        }
        let changed = row[index] != Some(bits);
        row[index] = Some(bits);
        changed
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct RealBounds {
    pub lower: f64,
    pub upper: f64,
}

impl From<F64Interval> for RealBounds {
    fn from(value: F64Interval) -> Self {
        Self { lower: value.lower(), upper: value.upper() }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct IntegerBounds {
    pub lower: i32,
    pub upper: i32,
}

impl From<I32Interval> for IntegerBounds {
    fn from(value: I32Interval) -> Self {
        Self { lower: value.lower(), upper: value.upper() }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckNoteBounds {
    pub note_id: i32,
    pub time_ms: i32,
    pub life: i32,
    /// Conditional integer supports for the factors visible on the note's last execution: 00/01/10/11 (probe
    /// bit 0, Rush bit 1); None when no path reaches the bucket's probe class.
    pub buckets: [Option<IntegerBounds>; 4],
    pub combo: RealBounds,
    /// Present once all lottery-dependent commands at this note's chart time have been filed.
    pub probability: Option<[RealBounds; 4]>,
    pub mean: Option<RealBounds>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckScoreQueryBounds {
    pub time_ms: i32,
    pub score_frame: i32,
    /// Probability-linked notes and the support bounds of notes whose lottery filing is still pending.
    pub mean: RealBounds,
    pub support: IntegerBounds,
    /// [combo, note, Just, Perfect, Great, Good]: the widest enclosure of a measured note's factor over all paths.
    pub factor_width: [f64; FIELDS],
    pub note_count: usize,
    /// Detailed cells are retained only for actual range snapshot queries; other queries keep aggregates.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<LuckNoteBounds>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckRangeScoreBounds {
    pub range: usize,
    pub start_query: Option<usize>,
    pub end_query: usize,
    pub percent: i64,
    pub mean: RealBounds,
    pub support: IntegerBounds,
    pub bonus_mean: RealBounds,
    pub bonus_support: IntegerBounds,
    pub luck_points_mean: Option<RealBounds>,
    /// Expected Miss, Hit, Super Hit and Critical counts.
    pub lot_results_mean: Option<[RealBounds; 4]>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckScoreBounds {
    pub model: &'static str,
    pub queries: Vec<LuckScoreQueryBounds>,
    pub ranges: Vec<LuckRangeScoreBounds>,
    pub final_mean: RealBounds,
    pub final_note_mean: RealBounds,
    pub final_rank_mean: RealBounds,
    pub final_support: IntegerBounds,
    /// Constant across the admitted lottery paths; established by `check_recorder`'s life dependency closure.
    pub exact_final_life: Option<i32>,
    pub final_notes: Vec<LuckNoteBounds>,
    pub query_limit: u64,
    pub actual_queries: usize,
    pub probability_peak_states: usize,
    pub probability_transitions: u64,
}

/// Compact production result. A singleton *native integer support* proves a constant score; a narrow
/// floating expectation interval alone never becomes an exact rational or a completion certificate.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckScoreSummary {
    pub final_mean: RealBounds,
    pub final_support: IntegerBounds,
    pub exact_constant_score: Option<i32>,
    /// Some only when every admitted path has this same native terminal life.
    pub exact_final_life: Option<i32>,
    pub probability_peak_states: usize,
    pub probability_transitions: u64,
}

fn refuse(why: &str) -> Error {
    Error::Unsupported(format!("native LUCK score bounds: {why}"))
}

pub(super) fn deterministic(checker: &Checker) -> bool {
    match checker {
        Checker::Fixed(_)
        | Checker::LifeAtLeast(_)
        | Checker::LifeAtMost(_)
        | Checker::LifeGreater(_)
        | Checker::LifeLess(_)
        | Checker::LifeChanged { .. }
        | Checker::LifePercent { .. }
        | Checker::LifeDelta { .. }
        | Checker::LiveComboMultiple { .. }
        | Checker::LiveComboAtLeast(_)
        | Checker::ElapsedTime { .. }
        | Checker::SameMemberLiveSkill(_)
        | Checker::NoteJudgementMatch { .. }
        | Checker::NoteJudgementCount { .. }
        | Checker::ComboAtLeast { .. }
        | Checker::GkInterval { .. }
        | Checker::JustAtLeast { .. }
        | Checker::GkJustEdge { .. }
        | Checker::SnapGekisouStart { .. }
        | Checker::GkLiveStart(_)
        | Checker::GkReachRank { .. }
        | Checker::RangeStart { .. }
        | Checker::RangePlaying { .. }
        | Checker::RangeComplete => true,
        Checker::And { items, .. } | Checker::Or(items) => items.iter().all(deterministic),
        Checker::Not(inner) => deterministic(inner),
        _ => false,
    }
}

/// Every current cumulative primitive reads only the fixed formation, frame clock, deterministic life and
/// converted-judgement/controller counts. Keep this exhaustive over variants: a future RNG/score-dependent
/// cumulative needs its own proof instead of inheriting this certificate.
pub(super) fn deterministic_cumulative(cumulative: &conditions::Cumulative) -> bool {
    match cumulative {
        conditions::Cumulative::Fixed(_)
        | conditions::Cumulative::ElapsedTime { .. }
        | conditions::Cumulative::ComboPerN { .. }
        | conditions::Cumulative::JudgementPerN { .. }
        | conditions::Cumulative::LifePerN { .. } => true,
    }
}

fn fixed_predicate(checker: &Checker) -> bool {
    match checker {
        Checker::Fixed(_) => true,
        Checker::And { items, .. } | Checker::Or(items) => items.iter().all(fixed_predicate),
        Checker::Not(inner) => fixed_predicate(inner),
        _ => false,
    }
}

/// Retained ordinary effects change only score or deterministic life under a constant maximum. With no stochastic conditions on
/// those rows, note damage, frozen life, judgements, combos, gates and command values are independent of
/// lottery outcomes. The reduced DP separately compiles the lottery rows against that same life timeline.
/// Induction over native frame/phase order closes life, conversion limits, live-skill duration, ordinary combo,
/// Gekisou combo/Just counts, cumulative counters and their predicates. Their writers below read only these same
/// deterministic domains; no predicate reads score or a lottery result. Rank predicates read either solo rank 1
/// or the fixed external arrival timeline. Range states use fever events and the original binary32 stopwatches.
/// The separately admitted 11000..11005 chain and direct-7021 score rows write none of those domains. Extension
/// 15000 changes the native live-skill lifecycle in this same deterministic closure; no updater is bypassed.
/// Therefore every lottery path has the recorder's identical converted judgements, life, combo inputs, ordinary
/// command filings and effect values. The DP life recorder retains these writers and consumes the same converted
/// results. A sampled agreement is not the authority for exact_final_life or the ordinary score command history.
fn check_recorder(model: &LiveModel, skills: &LuckSkills) -> Result<(), Error> {
    // AddCommand invalidates all four native cache fields, so extra reads preserve the command-log
    // semantics at a constant cap. UpdateLifeMax deliberately does not invalidate: a read before a
    // max change can retain an old-cap recovery. A single reference path cannot certify that domain
    // until its reader/cache history is included in the stochastic state. Current JP has no 3000 rows.
    if model.rows.iter().any(|row| row.effect_type == 3000) {
        return Err(refuse("dynamic life maximum requires a complete reader/cache-history certificate"));
    }
    let related: FxHashSet<_> = model.luck_score_rows(skills).iter().map(|row| row.row).collect();
    let check = |row: &EffectRow, cumulative: Option<&conditions::Cumulative>, checkers: &[&Option<Checker>]| {
        if !matches!(row.effect_type, 2000..=2005 | 3000..=3004 | 4004 | 12000 | 12002..=12004 | 12006
            | 13000 | 13002..=13005 | 15000)
            || cumulative.is_some_and(|c| !deterministic_cumulative(c))
            || checkers.iter().any(|checker| checker.as_ref().is_some_and(|c| !deterministic(c)))
        {
            return Err(refuse(&format!("ordinary row {} has no proved deterministic score/life schedule", row.id)));
        }
        Ok(())
    };
    for skill in &model.live {
        for effect in &skill.effects {
            check(&model.rows[effect.row], effect.cumulative.as_ref(), &[&effect.condition, &effect.release])?;
        }
    }
    // The probe rows switch together: one trigger, one gate, fixed conditions (the replay's two classes).
    let mut probe_gate = None;
    for skill in &model.cond {
        for (effect_index, effect) in skill.updater.effects().iter().enumerate() {
            if related.contains(&effect.row) {
                if *probe_gate.get_or_insert(skill.updater.gate_mission()) != skill.updater.gate_mission() {
                    return Err(refuse("score probes need one common Gekisou gate"));
                }
                if !matches!(effect.trigger, Some(Checker::LuckRushPlaying(_)))
                    || effect.trigger_type != SUSTAINED
                    || effect.act != 0.0
                    || effect.cumulative.is_some()
                    || effect.execute_limit != 0
                    || effect.reset.is_some()
                    || effect.condition.as_ref().is_some_and(|checker| !fixed_predicate(checker))
                    || skill
                        .updater
                        .updaters
                        .iter()
                        .any(|updater| updater.effect == effect_index && updater.release.is_some())
                {
                    return Err(refuse(
                        "score probes need a common untimed direct 7021 predicate with fixed conditions",
                    ));
                }
                continue;
            }
            if is_luck_chain(model.rows[effect.row].effect_type) {
                continue;
            }
            check(
                &model.rows[effect.row],
                effect.cumulative.as_ref(),
                &[&effect.trigger, &effect.condition, &effect.reset],
            )?;
            for updater in skill.updater.updaters.iter().filter(|updater| updater.effect == effect_index) {
                if updater.release.as_ref().is_some_and(|checker| !deterministic(checker)) {
                    return Err(refuse("an ordinary release reads an unproved state"));
                }
            }
        }
    }
    Ok(())
}

fn f32_real(value: F32Interval) -> F64Interval {
    F64Interval::new(f64::from(value.lower()), f64::from(value.upper())).expect("ordered finite native support")
}

fn checked_support(lower: i64, upper: i64) -> Result<I32Interval, Error> {
    let lo = i32::try_from(lower).map_err(|_| refuse("score support may wrap i32; refine paths"))?;
    let hi = i32::try_from(upper).map_err(|_| refuse("score support may wrap i32; refine paths"))?;
    I32Interval::new(lo, hi)
}

/// A query is the native note sum plus integer multiples of already filed rank bonuses.
/// Keep those bonuses as shared random variables: subtracting two snapshots must cancel a
/// bonus present with the same coefficient, even when its expectation/support is still wide.
#[derive(Clone)]
struct QueryParts {
    note_mean: F64Interval,
    note_support: I32Interval,
    fixed_coefficients: Vec<u8>,
    /// The last score frame the query sums.
    to: i32,
    /// The first score frame the query executed: the notes of the frames before it kept their native values since
    /// the previous query.
    executed_from: i32,
    /// For a rank snapshot, each summed note's score frame, support and mean.
    notes: Option<Vec<(i32, I32Interval, F64Interval)>>,
}

/// The note sum and filed bonuses snapshot `end` gained since snapshot `start`, given by their query indices into
/// the parts of every query. The frames that no query after `start` up to `end` executed again hold the same notes
/// with the same native values in both snapshots, which cancel exactly: only the notes of the later frames are
/// enclosed, on each side. Without the notes of both snapshots both whole note sums are enclosed separately.
fn snapshot_difference(
    start: Option<usize>,
    end: usize,
    queries: &[QueryParts],
    fixed: &[(i32, u8, F64Interval, I32Interval)],
) -> Result<(F64Interval, I32Interval), Error> {
    let (start_index, start, end_index, end) = (start, start.map(|index| &queries[index]), end, &queries[end]);
    // The last frame every query after `start` up to `end` left as `start` summed it.
    let kept = start_index.zip(start).filter(|&(index, _)| index <= end_index).map(|(index, start)| {
        let executed = queries[index + 1..=end_index].iter().map(|q| q.executed_from).min().unwrap_or(i32::MAX);
        start.to.min(executed.saturating_sub(1))
    });
    let later = |notes: &[(i32, I32Interval, F64Interval)], kept: i32| {
        notes.iter().filter(|&&(frame, _, _)| frame > kept).fold(
            (F64Interval::ZERO, 0i64, 0i64),
            |(mean, lower, upper), &(_, support, note_mean)| {
                (mean.add(note_mean), lower + i64::from(support.lower()), upper + i64::from(support.upper()))
            },
        )
    };
    let (mut mean, mut lower, mut upper) = match (kept, start.and_then(|v| v.notes.as_deref()), end.notes.as_deref()) {
        (Some(kept), Some(a), Some(b)) => {
            let (a_mean, a_lower, a_upper) = later(a, kept);
            let (b_mean, b_lower, b_upper) = later(b, kept);
            (b_mean.subtract(a_mean), b_lower - a_upper, b_upper - a_lower)
        }
        _ => {
            let a_support = start.map_or(I32Interval::point(0), |v| v.note_support);
            (
                end.note_mean.subtract(start.map_or(F64Interval::ZERO, |v| v.note_mean)),
                i64::from(end.note_support.lower()) - i64::from(a_support.upper()),
                i64::from(end.note_support.upper()) - i64::from(a_support.lower()),
            )
        }
    };
    for (index, &(_, _, bonus_mean, bonus_support)) in fixed.iter().enumerate() {
        let a = start.and_then(|v| v.fixed_coefficients.get(index)).copied().unwrap_or(0);
        let b = end.fixed_coefficients.get(index).copied().unwrap_or(0);
        let coefficient = i64::from(b) - i64::from(a);
        if coefficient == 0 {
            continue;
        }
        mean = mean.add(bonus_mean.scale_integer(i128::from(coefficient)));
        let x = coefficient * i64::from(bonus_support.lower());
        let y = coefficient * i64::from(bonus_support.upper());
        lower += x.min(y);
        upper += x.max(y);
    }
    Ok((mean, checked_support(lower, upper)?))
}

fn link_note_probability(
    note: &mut LuckNoteBounds,
    curve: &LuckDpCertifiedResult,
) -> Result<(F64Interval, I32Interval), Error> {
    let index = curve.steps.partition_point(|(time, _)| *time <= note.time_ms);
    let mass = index
        .checked_sub(1)
        .map_or([ProbabilityMass::ONE, ProbabilityMass::ZERO, ProbabilityMass::ZERO, ProbabilityMass::ZERO], |index| {
            curve.steps[index].1
        });
    let mut mean = F64Interval::ZERO;
    let (mut lower, mut upper) = (i32::MAX, i32::MIN);
    for (bucket, probability) in note.buckets.iter().zip(mass) {
        let possible = probability.interval().upper() > 0.0;
        // No native path reaches this probe class at the note, so its true mass is zero.
        let Some(bucket) = bucket else {
            if probability.interval().lower() > 0.0 {
                return Err(refuse("a lottery state has no path through the native factor schedule"));
            }
            continue;
        };
        mean = mean.add(probability.interval().multiply(I32Interval::new(bucket.lower, bucket.upper)?.as_real()));
        if possible {
            lower = lower.min(bucket.lower);
            upper = upper.max(bucket.upper);
        }
    }
    let support = I32Interval::new(lower, upper)?;
    mean = mean.intersect(support.as_real()).ok_or_else(|| refuse("note probability mean misses its support"))?;
    note.probability = Some(mass.map(|probability| probability.interval().into()));
    note.mean = Some(mean.into());
    Ok((mean, support))
}

fn note_bounds(
    calc: &LiveScoreCalculator,
    note: &NoteCommand,
    executed: &Classes,
    combo: F32Interval,
    gekisou: F32Interval,
    rush_percent: i32,
) -> Result<(LuckNoteBounds, I32Interval), Error> {
    let point = F32Interval::point;
    let note_percent =
        *calc.note_factor_percent.get(&note.note_type).ok_or_else(|| refuse("unknown score note type"))?;
    let judge_percent =
        *calc.judgement_score_factor_percent.get(&note.score_type).ok_or_else(|| refuse("unknown score judgement"))?;
    let t = point(calc.score_adjustment_factor)?
        .multiply(point(calc.state.band_total_power as f32)?)?
        .multiply(point(calc.music_difficulty_factor)?)?;
    let b = point(note_percent as f32 / 100f32)?.multiply(t)?.multiply(point(judge_percent as f32 / 100f32)?)?;
    let mut buckets = [None; 4];
    let mut combo_hull = None::<F32Interval>;
    let mut lower = i32::MAX;
    let mut upper = i32::MIN;
    for (bucket, out) in buckets.iter_mut().enumerate() {
        let Some(fields) = executed[bucket & 1] else { continue };
        let combo_factor = gekisou.multiply(fields[0].add(combo)?)?;
        if !combo_factor.lower().is_finite() || !combo_factor.upper().is_finite() {
            return Err(refuse("nonfinite combo-factor support"));
        }
        combo_hull = Some(combo_hull.map_or(combo_factor, |h| h.hull(combo_factor)));
        let judge = match note.score_type {
            1 => fields[2],
            2 => fields[3],
            3 => fields[4],
            4 => fields[5],
            _ => point(0.0)?,
        };
        let up = fields[1].add(judge)?;
        let luck = get_luck_factor_percent(if bucket & 2 == 0 { 0 } else { rush_percent }) as f32 / 100f32;
        let x = b
            .multiply(combo_factor)?
            .multiply(up)?
            .multiply(point(luck)?)?
            .divide(point(calc.converted_note_count as f32)?)?;
        let life = if note.life > 0 { 1.0 } else { calc.life_onus_factor };
        let integer = point(calc.assist_factor)?
            .multiply(point(life)?.multiply(x.native_floor_as_float().multiply(point(calc.event_bonus_factor)?)?)?)?
            .native_floor();
        lower = lower.min(integer.lower());
        upper = upper.max(integer.upper());
        *out = Some(integer.into());
    }
    let combo = combo_hull.ok_or_else(|| refuse("a filed note has no native execution"))?;
    Ok((
        LuckNoteBounds {
            note_id: note.note_id,
            time_ms: note.time_ms,
            life: note.life,
            buckets,
            combo: f32_real(combo).into(),
            probability: None,
            mean: None,
        },
        I32Interval::new(lower, upper)?,
    ))
}

/// The widest field enclosure of a note's last execution over its classes.
fn factor_widths(executed: &Classes, widths: &mut [f64; FIELDS]) {
    for fields in executed.iter().flatten() {
        for (width, field) in widths.iter_mut().zip(fields) {
            *width = width.max(f64::from(field.upper()) - f64::from(field.lower()));
        }
    }
}

/// Run a diagnostic certificate of native score expectations under the independent nominal lottery law.
/// Preconditions are checked on the complete physical deck: deterministic ordinary score/life rows,
/// the reduced DP's admitted lottery rows and untimed direct-7021 score rows; a fully finished solo schedule.
/// Widths remain explicit. This function never constructs an ExactExpectation or a search completion state.
#[allow(clippy::too_many_arguments)]
pub fn luck_score_bounds(
    master: &Master,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
) -> Result<LuckScoreBounds, Error> {
    luck_score_bounds_with_ranking(master, deck, notes, events, params, setup, play, delta_times, None)
}

/// The same native arithmetic enclosure using an explicitly declared external rank-confirmation timeline.
/// External ranks use the controller's actual frame score snapshots, without solo timestamp rewinds.
#[allow(clippy::too_many_arguments)]
pub fn luck_score_bounds_with_ranking(
    master: &Master,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    ranking: Option<&[crate::replay::RankConfirmation]>,
) -> Result<LuckScoreBounds, Error> {
    let skills = luck_skills(master)?;
    luck_score_bounds_internal(
        master,
        &skills,
        deck,
        notes,
        events,
        params,
        setup,
        play,
        delta_times,
        ranking,
        true,
        None,
        false,
        None,
    )
}

/// The same arithmetic and native snapshot proof without retaining diagnostic query/note reports. `skills` is
/// [`luck_skills`] of `master`.
#[allow(clippy::too_many_arguments)]
pub fn luck_score_summary_with_ranking(
    master: &Master,
    skills: &LuckSkills,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    ranking: Option<&[crate::replay::RankConfirmation]>,
) -> Result<LuckScoreSummary, Error> {
    luck_score_summary_with_curves(master, skills, deck, notes, events, params, setup, play, delta_times, ranking, None)
}

/// [`luck_score_summary_with_ranking`] whose lottery curve comes from, and enters, `curves` when given.
#[allow(clippy::too_many_arguments)]
pub fn luck_score_summary_with_curves(
    master: &Master,
    skills: &LuckSkills,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    ranking: Option<&[crate::replay::RankConfirmation]>,
    curves: Option<&mut LuckDpCache>,
) -> Result<LuckScoreSummary, Error> {
    let bounds = luck_score_bounds_internal(
        master,
        skills,
        deck,
        notes,
        events,
        params,
        setup,
        play,
        delta_times,
        ranking,
        false,
        curves,
        false,
        None,
    )?;
    Ok(LuckScoreSummary {
        final_mean: bounds.final_mean,
        final_support: bounds.final_support,
        exact_constant_score: (bounds.final_support.lower == bounds.final_support.upper)
            .then_some(bounds.final_support.lower),
        exact_final_life: bounds.exact_final_life,
        probability_peak_states: bounds.probability_peak_states,
        probability_transitions: bounds.probability_transitions,
    })
}

/// The expected final score and range scores of a solo play under the independent nominal lottery law.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckScoreExpectation {
    pub final_mean: RealBounds,
    /// A singleton proves a constant score.
    pub final_support: IntegerBounds,
    /// One per Gekisou range, in range order: its score and rank bonus.
    pub ranges: Vec<LuckRangeScoreBounds>,
}

/// [`luck_score_summary_with_ranking`] of a solo play with the range enclosures. `skills` is [`luck_skills`] of
/// `master`.
#[allow(clippy::too_many_arguments)]
pub fn luck_score_expectation(
    master: &Master,
    skills: &LuckSkills,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
) -> Result<LuckScoreExpectation, Error> {
    luck_score_expectation_with_curves(
        master,
        skills,
        deck,
        notes,
        events,
        params,
        setup,
        play,
        delta_times,
        None,
        None,
    )
}

/// Range expectations under an explicit rank-arrival timeline, sharing certified lottery curves when supplied.
#[allow(clippy::too_many_arguments)]
pub fn luck_score_expectation_with_curves(
    master: &Master,
    skills: &LuckSkills,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    ranking: Option<&[crate::replay::RankConfirmation]>,
    curves: Option<&mut LuckDpCache>,
) -> Result<LuckScoreExpectation, Error> {
    let mut bounds = luck_score_bounds_internal(
        master,
        skills,
        deck,
        notes,
        events,
        params,
        setup,
        play,
        delta_times,
        ranking,
        false,
        curves,
        false,
        None,
    )?;
    bounds.ranges.sort_by_key(|r| r.range);
    if bounds.ranges.len() != setup.fevers.len() || bounds.ranges.iter().enumerate().any(|(i, r)| r.range != i) {
        return Err(refuse("a Gekisou range has no rank enclosure"));
    }
    Ok(LuckScoreExpectation {
        final_mean: bounds.final_mean,
        final_support: bounds.final_support,
        ranges: bounds.ranges,
    })
}

/// Chart measurements vary ordinary score-only rows while retaining this curve's lottery dependencies.
/// The caller keeps the formation, play, power and rank timeline identical to the curve's inputs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn luck_score_expectation_for_chart(
    master: &Master,
    skills: &LuckSkills,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    ranking: Option<&[crate::replay::RankConfirmation]>,
    probability: std::sync::Arc<LuckDpCertifiedResult>,
) -> Result<LuckScoreExpectation, Error> {
    let mut bounds = luck_score_bounds_internal(
        master,
        skills,
        deck,
        notes,
        events,
        params,
        setup,
        play,
        delta_times,
        ranking,
        false,
        None,
        true,
        Some(probability),
    )?;
    bounds.ranges.sort_by_key(|r| r.range);
    if bounds.ranges.len() != setup.fevers.len() || bounds.ranges.iter().enumerate().any(|(i, r)| r.range != i) {
        return Err(refuse("a Gekisou range has no rank enclosure"));
    }
    Ok(LuckScoreExpectation {
        final_mean: bounds.final_mean,
        final_support: bounds.final_support,
        ranges: bounds.ranges,
    })
}

/// Prepares a Gekisou live without a LUCK range for a run that draws no random value. A deck that reads no lottery
/// or probability is left as it is; any other deck becomes a LUCK weighted live without probabilities
/// ([`LiveModel::set_luck_weights`] with no steps). Without a LUCK range the controller consumes no lottery
/// (`luck_score_bounds_internal`): a probability predicate gates only luck chain effects, which write the unread luck
/// machine, and lottery-dependent score-ups, which need a running rush. The recorder conditions admit no other reader
/// of these states. The prepared live therefore has the native score, judgements, life and rank arrivals of every
/// seed. Call it before the first frame.
pub fn prepare_lottery_free(model: &mut LiveModel, skills: &LuckSkills) -> Result<(), Error> {
    let Some(gk) = model.gk.as_ref() else { return Ok(()) };
    if gk.ctrl.ranges.iter().any(|range| range.mission == gekisou::M_LUCK) {
        return Err(refuse("a LUCK range draws lotteries"));
    }
    if !model.reads_lottery() {
        return Ok(());
    }
    check_recorder(model, skills)?;
    model.set_luck_weights(skills, Vec::new())
}

#[allow(clippy::too_many_arguments)]
fn luck_score_bounds_internal(
    master: &Master,
    skills: &LuckSkills,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    ranking: Option<&[crate::replay::RankConfirmation]>,
    details: bool,
    curves: Option<&mut LuckDpCache>,
    counterfactual_solo: bool,
    probability: Option<std::sync::Arc<LuckDpCertifiedResult>>,
) -> Result<LuckScoreBounds, Error> {
    #[cfg(feature = "search-diagnostics")]
    let phase_start = std::time::Instant::now();
    let mut model = if let Some(ranking) = ranking {
        let mut model = if counterfactual_solo {
            LiveModel::new_gekisou_ranked(master, deck, notes, events, params, setup)?
        } else {
            LiveModel::new_gekisou_external(master, deck, notes, events, params, setup)?
        };
        model.set_rank_confirmation_timeline(ranking)?;
        model
    } else {
        LiveModel::new_gekisou(master, deck, notes, events, params, setup)?
    };
    check_recorder(&model, skills)?;
    #[cfg(feature = "search-diagnostics")]
    let model_setup_ms = phase_start.elapsed().as_secs_f64() * 1e3;
    #[cfg(feature = "search-diagnostics")]
    let phase_start = std::time::Instant::now();
    let has_luck = setup.missions.iter().take(setup.fevers.len()).any(|&mission| mission == gekisou::M_LUCK);
    // Without a LUCK range, Controller::judge/update_luck and pending_lots never consume a lottery. Chain
    // probability predicates may still draw SKILL values, but their writes are confined to the unused luck
    // machine/gauge/points. check_recorder excludes any other stochastic writer and direct-7021 probes remain
    // false. Consequently these draws cannot reach score, judgements, life or fixed rank arrivals; this is a
    // deterministic score law even when native.draws() > 0. It is not a fixed-seed approximation.
    let collect_moments = details || counterfactual_solo;
    let probability = if let Some(probability) = probability {
        probability
    } else if !has_luck {
        std::sync::Arc::new(LuckDpCertifiedResult {
            steps: Vec::new(),
            probes: vec![false; skills.shapes.len()],
            range_moments: if collect_moments {
                vec![super::luck_dp::LuckRangeMoments::default(); setup.fevers.len()]
            } else {
                Vec::new()
            },
            peak_states: 1,
            transitions: 0,
        })
    } else if let Some(curves) = curves {
        if collect_moments {
            curves.certified_with_moments(
                master,
                skills,
                notes,
                events,
                params,
                setup,
                play,
                delta_times,
                deck,
                None,
                ranking,
            )?
        } else {
            curves.certified(master, skills, notes, events, params, setup, play, delta_times, deck, None, ranking)?
        }
    } else {
        std::sync::Arc::new(luck_dp::certified_mode(
            master,
            skills,
            notes,
            events,
            params,
            setup,
            play,
            delta_times,
            deck,
            None,
            ranking,
            collect_moments,
        )?)
    };
    #[cfg(feature = "search-diagnostics")]
    let curve_dp_ms = phase_start.elapsed().as_secs_f64() * 1e3;
    let probes: Vec<_> = model
        .luck_score_rows(skills)
        .into_iter()
        .filter(|row| row.may_hold)
        .map(|row| ProbeRow { owner: row.owner, value: row.value })
        .collect();
    if probes.iter().any(|row| !row.value.is_finite() || row.value <= i32::MIN as f32 / 100000f32) {
        return Err(refuse("a direct score command cannot be safely paired with its signed inverse"));
    }
    let calc = model.score.calc.clone();
    if calc.converted_note_count <= 0
        || ![
            calc.score_adjustment_factor,
            calc.music_difficulty_factor,
            calc.life_onus_factor,
            calc.event_bonus_factor,
            calc.assist_factor,
        ]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(refuse("nonfinite score constants or nonpositive note count"));
    }
    let rush_percent = i32::try_from(setting(master, "gekisou_luck_rush_score_bonus_percent")?)
        .map_err(|_| refuse("Rush percent exceeds i32"))?;
    if 100i32.checked_add(rush_percent).is_none() {
        return Err(refuse("Rush factor may wrap"));
    }
    model.set_luck_weights(skills, Vec::new())?;
    model.score.begin_bounds(
        probes,
        setup.missions.iter().take(setup.fevers.len()).any(|&mission| mission == gekisou::M_LUCK),
    );
    #[cfg(feature = "search-diagnostics")]
    let phase_start = std::time::Instant::now();
    model.run_timed(play, delta_times)?;
    #[cfg(feature = "search-diagnostics")]
    let recorder_run_ms = phase_start.elapsed().as_secs_f64() * 1e3;
    #[cfg(feature = "search-diagnostics")]
    let phase_start = std::time::Instant::now();
    if model.random.draws() != 0 {
        return Err(refuse("the supposedly deterministic recorder consumed random draws"));
    }
    if model.gk.as_ref().is_none_or(|g| g.ctrl.states.iter().any(|state| state.state != gekisou::S_FINISH)) {
        return Err(refuse("terminal query precedes a range FINISH"));
    }
    let trace = model.score.bounds_trace.take().expect("bounds recorder enabled");
    let query_limit = (play.frames.len() as u64)
        .checked_mul(2)
        .and_then(|value| {
            value.checked_add(if ranking.is_none() || counterfactual_solo { 2 * setup.fevers.len() as u64 } else { 0 })
        })
        .ok_or_else(|| Error::Capacity("score query count overflow".into()))?;
    if trace.queries as u64 > query_limit {
        return Err(refuse("unaccounted native calculate entry point"));
    }
    let mut replay = Replay::new(trace.frames, &trace.probes);
    let (mut prev, mut added) = (-1i32, None::<i32>);
    let mut mandatory_added = None::<i32>;
    let mut probability_ready = i32::MIN;
    // (frame, index in the frame, note, index in `replay.notes`)
    let mut filed = Vec::<(usize, usize, NoteCommand, usize)>::new();
    // (frame, position in `filed`) by ascending frame: a query's combo window is a contiguous run.
    let mut by_frame = Vec::<(usize, usize)>::new();
    let mut current_combos = FxHashMap::<(usize, usize), (F32Interval, F32Interval)>::default();
    let mut retained_combos = FxHashMap::<(usize, usize), (F32Interval, F32Interval)>::default();
    let mut queries = Vec::<LuckScoreQueryBounds>::new();
    let detailed_queries: FxHashSet<usize> = trace
        .events
        .iter()
        .flat_map(|event| match event {
            BoundsEvent::Rank { start, end, .. } => [*start, *end],
            _ => [None, None],
        })
        .flatten()
        .collect();
    let mut query_means = Vec::<F64Interval>::new();
    let mut query_supports = Vec::<I32Interval>::new();
    let mut query_parts = Vec::<QueryParts>::new();
    let mut ranges = Vec::new();
    // Pending fixed scores add immediately even outside the score-frame array. Later undo/execute only
    // changes their in-prefix contribution; the filing offset is permanent.
    let mut fixed = Vec::<(i32, u8, F64Interval, I32Interval)>::new();
    let mut pending = None;
    for event in &trace.events {
        match event {
            BoundsEvent::Note { frame, index, note } => {
                let at = replay.file_note(*frame, note.time_ms, note.note_id)?;
                by_frame.insert(by_frame.partition_point(|&(f, _)| f <= *frame), (*frame, filed.len()));
                filed.push((*frame, *index, *note, at));
                added = Some(added.map_or(*frame as i32, |old| old.min(*frame as i32)));
                mandatory_added = Some(mandatory_added.map_or(*frame as i32, |old| old.min(*frame as i32)));
            }
            BoundsEvent::Factor { frame, command } => {
                if command.band_total_power != 0 {
                    return Err(refuse("recorder produced an unproved power command"));
                }
                replay.file_command(*frame, command)?;
                added = Some(added.map_or(*frame as i32, |old| old.min(*frame as i32)));
                if command.luck == 0 {
                    mandatory_added = Some(mandatory_added.map_or(*frame as i32, |old| old.min(*frame as i32)));
                }
            }
            BoundsEvent::Potential { frame } => {
                replay.potential(*frame);
                added = Some(added.map_or(*frame as i32, |old| old.min(*frame as i32)));
            }
            BoundsEvent::Probe { frame, time_ms } => {
                replay.probe(*frame, *time_ms)?;
                added = Some(added.map_or(*frame as i32, |old| old.min(*frame as i32)));
            }
            BoundsEvent::Combo { frame, index, ordinary, gekisou } => {
                current_combos
                    .insert((*frame, *index), (F32Interval::point(*ordinary)?, F32Interval::point(*gekisou)?));
            }
            BoundsEvent::ProbabilityReady(time) => {
                probability_ready = probability_ready.max(*time);
            }
            BoundsEvent::Rank { range, time_ms, percent, start, end } => {
                let Some(end) = *end else {
                    return Err(refuse("rank end query was not recorded"));
                };
                // Network FEVER_START may snapshot the initial zero before any score calculation.
                // Both snapshots refer to the very same filed bonus values. Cancel their integer
                // coefficients before enclosing the remaining difference; never assume independence.
                let (mean, support) = snapshot_difference(*start, end, &query_parts, &fixed)?;
                let x = i128::from(support.lower()) * i128::from(*percent) / 100;
                let y = i128::from(support.upper()) * i128::from(*percent) / 100;
                let bonus_support = I32Interval::new(
                    i32::try_from(x.min(y)).map_err(|_| refuse("rank bonus wraps"))?,
                    i32::try_from(x.max(y)).map_err(|_| refuse("rank bonus wraps"))?,
                )?;
                let bonus = if support.lower() == support.upper() {
                    bonus_support.as_real()
                } else {
                    rank_mean_bounds(mean, support, *percent)?
                };
                pending = Some((get_frame(*time_ms), bonus, bonus_support));
                ranges.push(LuckRangeScoreBounds {
                    range: *range,
                    start_query: *start,
                    end_query: end,
                    percent: *percent,
                    mean: mean.into(),
                    support: support.into(),
                    bonus_mean: bonus.into(),
                    bonus_support: bonus_support.into(),
                    luck_points_mean: probability.range_moments.get(*range).map(|m| m.luck_points.into()),
                    lot_results_mean: probability.range_moments.get(*range).map(|m| m.lot_results.map(Into::into)),
                });
            }
            BoundsEvent::Query { time_ms, to } => {
                // The admitted controller/ordinary predicates never read score. Only native rank
                // snapshots and the terminal result observe an integer score; those query IDs are
                // all retained. Other queries still execute every factor diff/undo count and combo
                // history update below. Their intermediate integer sums may wrap modulo 2^32, but
                // do not change a later sum or any control-flow input: native addition/subtraction
                // are modular, and each actually observed snapshot is separately proved in i32.
                // Thus production need not re-enclose every note thousands of unused times.
                let measure =
                    details || detailed_queries.contains(&query_means.len()) || query_means.len() + 1 == trace.queries;
                replay.query(*to)?;
                let u = added.map_or(*to, |frame| (*to).min(frame - 1));
                let start = if u < prev { u + 1 } else { prev + 1 };
                let mandatory_u = mandatory_added.map_or(*to, |frame| (*to).min(frame - 1));
                let mandatory_start = if mandatory_u < prev { mandatory_u + 1 } else { prev + 1 };
                let mut factor_width = [0.0; FIELDS];
                let mut measured = Vec::new();
                let retain_notes = details && detailed_queries.contains(&query_means.len());
                let mut note_count = 0;
                let (mut lo, mut hi) = (0i64, 0i64);
                let mut mean = F64Interval::ZERO;
                let mut rank_notes = detailed_queries.contains(&query_means.len()).then(Vec::new);
                // A note first enters a query at or after `start` (its filing lowers `added`, and an unqueried
                // note lies after `prev`), so every note's observation is checked in this window once.
                let first = by_frame.partition_point(|&(f, _)| (f as i32) < start);
                for &(frame, at) in by_frame[first..].iter().take_while(|&&(f, _)| f as i32 <= *to) {
                    let key = (frame, filed[at].1);
                    let current =
                        *current_combos.get(&key).ok_or_else(|| refuse("note combo observation is missing"))?;
                    if frame as i32 >= mandatory_start {
                        retained_combos.insert(key, current);
                    } else {
                        retained_combos
                            .entry(key)
                            .and_modify(|old| {
                                old.0 = old.0.hull(current.0);
                                old.1 = old.1.hull(current.1);
                            })
                            .or_insert(current);
                    }
                }
                // Measured sums keep the filing order of their outward-rounded additions.
                let measured_notes: &[_] = if measure { &filed } else { &[] };
                for &(frame, index, ref note, at) in
                    measured_notes.iter().filter(|(frame, _, _, _)| *frame as i32 <= *to)
                {
                    let key = (frame, index);
                    let (combo, gekisou_combo) =
                        *retained_combos.get(&key).ok_or_else(|| refuse("note has no possible execution"))?;
                    let executed = &replay.notes[at].executed;
                    factor_widths(executed, &mut factor_width);
                    let (mut bounds, mut support) =
                        note_bounds(&calc, note, executed, combo, gekisou_combo, rush_percent)?;
                    let note_mean = if note.time_ms <= probability_ready {
                        let (mean, linked_support) = link_note_probability(&mut bounds, &probability)?;
                        support = linked_support;
                        mean
                    } else {
                        support.as_real()
                    };
                    mean = mean.add(note_mean);
                    lo += i64::from(support.lower());
                    hi += i64::from(support.upper());
                    if let Some(notes) = &mut rank_notes {
                        notes.push((frame as i32, support, note_mean));
                    }
                    note_count += 1;
                    if retain_notes {
                        measured.push(bounds);
                    }
                }
                let note_support = checked_support(lo, hi)?;
                let note_mean = mean;
                if let Some((frame, bonus, support)) = pending.take() {
                    fixed.push((frame, u8::from(frame > *to), bonus, support));
                }
                let mut fixed_coefficients = Vec::with_capacity(fixed.len());
                for &(frame, offset, bonus, support) in &fixed {
                    let coefficient = i64::from(offset) + i64::from(frame <= *to);
                    fixed_coefficients.push(coefficient as u8);
                    mean = mean.add(bonus.scale_integer(i128::from(coefficient)));
                    lo += coefficient * i64::from(support.lower());
                    hi += coefficient * i64::from(support.upper());
                }
                let support = if measure { checked_support(lo, hi)? } else { I32Interval::point(0) };
                query_means.push(mean);
                query_supports.push(support);
                query_parts.push(QueryParts {
                    note_mean,
                    note_support,
                    fixed_coefficients,
                    to: *to,
                    executed_from: start,
                    notes: rank_notes,
                });
                if details {
                    queries.push(LuckScoreQueryBounds {
                        time_ms: *time_ms,
                        score_frame: *to,
                        mean: mean.into(),
                        support: support.into(),
                        factor_width,
                        note_count,
                        notes: measured,
                    });
                }
                prev = *to;
                added = None;
                mandatory_added = None;
            }
        }
    }
    if pending.is_some() || query_means.is_empty() {
        return Err(refuse("terminal query has not filed every rank bonus"));
    }
    let mut final_mean = F64Interval::ZERO;
    let mut final_notes = Vec::new();
    for &(frame, index, ref note, at) in filed.iter().filter(|(frame, _, _, _)| *frame as i32 <= prev) {
        let (combo, gekisou_combo) =
            *retained_combos.get(&(frame, index)).ok_or_else(|| refuse("final note combo is missing"))?;
        let (mut bounds, _) = note_bounds(&calc, note, &replay.notes[at].executed, combo, gekisou_combo, rush_percent)?;
        if note.time_ms > probability_ready {
            return Err(refuse("final note has pending lottery commands"));
        }
        let (mean, _) = link_note_probability(&mut bounds, &probability)?;
        final_mean = final_mean.add(mean);
        if details {
            final_notes.push(bounds);
        }
    }
    let final_note_mean = final_mean;
    let mut final_rank_mean = F64Interval::ZERO;
    for &(frame, offset, bonus, _) in &fixed {
        let coefficient = i128::from(offset) + i128::from(frame <= prev);
        final_rank_mean = final_rank_mean.add(bonus.scale_integer(coefficient));
    }
    final_mean = final_mean.add(final_rank_mean);
    let final_support =
        if has_luck { *query_supports.last().expect("terminal query") } else { I32Interval::point(model.score()) };
    if !has_luck {
        final_mean = F64Interval::integer(model.score() as i128);
    }
    #[cfg(feature = "search-diagnostics")]
    profile::record(LuckScoreProfile {
        evaluations: 1,
        model_setup_ms,
        curve_dp_ms,
        recorder_run_ms,
        bound_replay_ms: phase_start.elapsed().as_secs_f64() * 1e3,
    });
    Ok(LuckScoreBounds {
        model: "independent nominal draws; all-path native arithmetic enclosure, not an exact expectation or search completion; note probabilities link after their native lottery commands are filed",
        final_support: final_support.into(),
        exact_final_life: Some(model.current_life()),
        final_mean: final_mean.into(),
        final_note_mean: final_note_mean.into(),
        final_rank_mean: final_rank_mean.into(),
        final_notes,
        queries,
        ranges,
        query_limit,
        actual_queries: trace.queries,
        probability_peak_states: probability.peak_states,
        probability_transitions: probability.transitions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snapshot_difference_cancels_only_shared_fixed_bonus_coefficients() {
        let fixed = [
            (1, 1, F64Interval::new(-1000.0, 2000.0).unwrap(), I32Interval::new(-1000, 2000).unwrap()),
            (2, 0, F64Interval::new(10.0, 20.0).unwrap(), I32Interval::new(10, 20).unwrap()),
        ];
        let part = |sum: i32| QueryParts {
            note_mean: F64Interval::integer(i128::from(sum)),
            note_support: I32Interval::point(sum),
            fixed_coefficients: vec![2, 1],
            to: 0,
            executed_from: 0,
            notes: None,
        };
        let mut queries = vec![part(100), part(130)];
        let (mean, support) = snapshot_difference(Some(0), 1, &queries, &fixed).unwrap();
        assert_eq!(support, I32Interval::point(30));
        assert!(mean.contains(30.0) && mean.upper() - mean.lower() < 1e-10);
        // A rewind removes the second bonus from the prefix, while the first bonus's permanent
        // filing offset and in-prefix copy are present on both sides. Only the latter cancels.
        queries[1].fixed_coefficients[1] = 0;
        let (mean, support) = snapshot_difference(Some(0), 1, &queries, &fixed).unwrap();
        assert_eq!(support, I32Interval::new(10, 20).unwrap());
        assert!(mean.contains(10.0) && mean.contains(20.0));
        // Initial network zero has no symbols, so neither a filing offset nor a prefix may vanish.
        let (_, support) = snapshot_difference(None, 1, &queries, &fixed).unwrap();
        assert_eq!(support, I32Interval::new(-1870, 4130).unwrap());
    }

    #[test]
    fn snapshot_difference_cancels_notes_that_no_later_query_executed_again() {
        // Two notes in frames 1 and 2 whose lottery leaves each between 100 and 200 points, summed by the start
        // snapshot at frame 2, and one note in frame 4 between 30 and 60 points.
        let early = |frame| (frame, I32Interval::new(100, 200).unwrap(), F64Interval::new(100.0, 200.0).unwrap());
        let late = (4, I32Interval::new(30, 60).unwrap(), F64Interval::new(30.0, 60.0).unwrap());
        let part = |to: i32, executed_from: i32, notes: Vec<(i32, I32Interval, F64Interval)>, rank: bool| {
            let (lower, upper) = notes.iter().fold((0, 0), |(l, u), n| (l + n.1.lower(), u + n.1.upper()));
            QueryParts {
                note_mean: notes.iter().fold(F64Interval::ZERO, |m, n| m.add(n.2)),
                note_support: I32Interval::new(lower, upper).unwrap(),
                fixed_coefficients: Vec::new(),
                to,
                executed_from,
                notes: rank.then_some(notes),
            }
        };
        let mut queries = vec![
            part(2, 0, vec![early(1), early(2)], true),
            part(3, 3, vec![early(1), early(2)], false),
            part(4, 4, vec![early(1), early(2), late], true),
        ];
        let (mean, support) = snapshot_difference(Some(0), 2, &queries, &[]).unwrap();
        assert_eq!(support, I32Interval::new(30, 60).unwrap());
        assert_eq!((mean.lower(), mean.upper()), (30.0, 60.0));
        // A query between them executed frame 2 again: its note may have changed, so it is enclosed on each side.
        queries[1].executed_from = 2;
        let (_, support) = snapshot_difference(Some(0), 2, &queries, &[]).unwrap();
        assert_eq!(support, I32Interval::new(130 - 200, 260 - 100).unwrap());
        // From frame 0 on, every note is enclosed on each side.
        queries[1].executed_from = 0;
        let (_, support) = snapshot_difference(Some(0), 2, &queries, &[]).unwrap();
        assert_eq!(support, I32Interval::new(230 - 400, 460 - 200).unwrap());
        // Without the start snapshot's notes, so are the whole sums.
        queries[1].executed_from = 3;
        queries[0].notes = None;
        let (_, support) = snapshot_difference(Some(0), 2, &queries, &[]).unwrap();
        assert_eq!(support, I32Interval::new(230 - 400, 460 - 200).unwrap());
    }

    #[test]
    fn recorder_refuses_hidden_random_checks() {
        let condition = Checker::And {
            items: vec![Checker::Fixed(false), Checker::Probability(0.25)],
            resettable: vec![false, false],
        };
        assert!(!deterministic(&condition));
        assert!(!deterministic(&Checker::LuckRushPlaying(false)));
        assert!(deterministic(&Checker::LifeAtLeast(Some(700))));
    }

    fn fixture() -> (Master, Vec<LiveNote>, LiveParams, GekisouSetup, LivePlay, Vec<f32>) {
        let lots: Vec<_> =
            (0..5).map(|kind| json!({"_id":kind+1,"_chanceLotType":kind,"_lotResult":3,"_weight":1})).collect();
        let tables = json!({
            "MasterLiveSettings":[
                {"_id":1,"_key":"note_score_adjustment_factor","_value":"3"},
                {"_id":2,"_key":"note_score_life_onus_factor","_value":"0.5"},
                {"_id":3,"_key":"life_base","_value":"1000"},
                {"_id":4,"_key":"life_denger","_value":"300"},
                {"_id":5,"_key":"gekisou_luck_gauge_max","_value":"140"},
                {"_id":6,"_key":"gekisou_luck_gauge_max_rush","_value":"70"},
                {"_id":7,"_key":"gekisou_luck_rush_score_bonus_percent","_value":"10"}],
            "MasterLiveNoteParameter":[{"_id":1,"_noteOperateType":1,"_scorePercent":100}],
            "MasterLiveJudgementParameter":[{"_id":1,"_noteSimulateJudgement":5,"_scorePercent":100,"_damage":0}],
            "MasterLiveJudgementTiming":[{"_id":1,"_noteJudgementType":1,"_noteSimulateJudgement":5,"_afterMs":0}],
            "MasterLiveGekisouLuckBasePoint":[{"_id":1,"_noteCategory":0,"_noteSimulateJudgement":5,"_weight":1,"_basePoint":60}],
            "MasterLiveGekisouLuckBonusLot":lots
        });
        let texts: Vec<_> = tables
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, rows)| (name.clone(), json!({"_allData":rows}).to_string()))
            .collect();
        let master =
            Master::from_json_tables(|name| texts.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()))
                .unwrap();
        let notes: Vec<_> = [100, 110, 120, 200, 260, 300, 310, 400, 500, 800]
            .into_iter()
            .enumerate()
            .map(|(i, time_ms)| LiveNote { note_id: i as i32, note_operate_type: 1, judgement_type: 1, time_ms })
            .collect();
        let params = LiveParams {
            skill_target_music_type: 0,
            total_power: 1000,
            music_level: 20,
            converted_note_count: notes.len() as i32,
            music_length_ms: 2000,
            score_music_length_ms: None,
            assist_factor: 1.0,
        };
        let setup = GekisouSetup { fevers: vec![(100, 500)], missions: vec![2, 2, 2] };
        let mut frames: Vec<_> = (0..=20).map(|i| PlayFrame { time_ms: i * 100, judged: Vec::new() }).collect();
        for note in &notes {
            frames.iter_mut().find(|frame| frame.time_ms >= note.time_ms).unwrap().judged.push(JudgedNote {
                note_id: note.note_id,
                judgement: 5,
                judgement_time_ms: note.time_ms,
            });
        }
        let delta = vec![0.1; frames.len()];
        (master, notes, params, setup, LivePlay { frames, base_seed: 0 }, delta)
    }

    #[test]
    fn completed_native_score_and_real_range_queries_are_enclosed() {
        let (master, notes, params, setup, play, delta) = fixture();
        let bounds = luck_score_bounds(&master, &[], &notes, &[], params, &setup, &play, &delta).unwrap();
        let mut native = LiveModel::new_gekisou(&master, &[], &notes, &[], params, &setup).unwrap();
        let score = native.run_timed(&play, &delta).unwrap();
        assert!(bounds.final_mean.lower <= f64::from(score) && f64::from(score) <= bounds.final_mean.upper);
        assert_eq!(bounds.actual_queries as u64, 2 * play.frames.len() as u64 + 2);
        assert_eq!(bounds.ranges.len(), 1);
        let range = &bounds.ranges[0];
        assert_eq!(bounds.queries[range.start_query.unwrap()].time_ms, setup.fevers[0].0);
        assert_eq!(bounds.queries[range.end_query].time_ms, setup.fevers[0].1);
        assert_eq!(bounds.final_notes.len(), notes.len());
        assert_eq!(bounds.queries.iter().filter(|query| !query.notes.is_empty()).count(), 2);
        assert!(range.mean.upper - range.mean.lower < 1e-8);
        assert!(bounds.queries[range.end_query].notes.iter().all(|note| note.probability.is_some()));
    }

    #[test]
    fn probabilistic_start_guarantees_match_the_full_nominal_law() {
        for (minimum, percent) in [(2, 5), (2, 60), (3, 5), (3, 60)] {
            let (mut master, notes, params, setup, play, delta) = fixture();
            master.gekisou_luck_bonus_lots = (0..5)
                .flat_map(|kind| {
                    (0..4).map(move |result| crate::master::LuckBonusLotRow {
                        id: kind * 4 + result + 1,
                        chance_lot_type: kind,
                        lot_result: result,
                        weight: 1,
                    })
                })
                .collect();
            for (id, kind, values) in [(1, 7010, vec![]), (2, 4011, vec![percent]), (3, 7013, vec![])] {
                master.skill_conditions.push(
                    serde_json::from_value(json!({
                        "_id": id, "_conditionType": kind, "_conditionValues": values,
                        "_conditionTargetIDs": [], "_isPositive": true,
                    }))
                    .unwrap(),
                );
                master.skill_condition_sets.push(
                    serde_json::from_value(json!({
                        "_id": id, "_group": id, "_conditionIds": [id],
                    }))
                    .unwrap(),
                );
            }
            master.skill_effect_settings.push(
                serde_json::from_value(json!({
                    "_id": 1, "_skillEffectType": 11005, "_phase": 1,
                }))
                .unwrap(),
            );
            let skill = crate::master::SkillRow { id: 1, gekisou_mission_type: 2, ..Default::default() };
            master.gekisou_skills.push(skill.clone());
            master.gekisou_support_skills.push(skill);
            master.gekisou_support_skill_effects.push(
                serde_json::from_value(json!({
                    "_id": 1, "_gekisouSupportSkillID": 1, "_level": 1,
                    "_skillTriggerType": 1, "_skillTriggerConditionGroup": 1,
                    "_skillConditionGroup": 2, "_skillReleaseConditionGroup": 3,
                    "_skillEffectType": 11005, "_effectValue": minimum, "_effectLimitCount": 1,
                }))
                .unwrap(),
            );
            master.reindex().unwrap();
            let deck = [Performer {
                gekisou_skill: Some((1, 1)),
                gekisou_mission_type: 2,
                gekisou_support_skills: vec![(1, 1)],
                ..Default::default()
            }];
            let skills = luck_skills(&master).unwrap();
            let dp =
                luck_score_expectation(&master, &skills, &deck, &notes, &[], params, &setup, &play, &delta).unwrap();
            let mut expected = 0.0;
            for threshold in [0, 1001] {
                let mut branch_master = master.clone();
                let condition = branch_master.skill_conditions.iter_mut().find(|row| row.id == 2).unwrap();
                condition.condition_type = 2001;
                condition.condition_values = vec![threshold];
                branch_master.reindex().unwrap();
                let attempt = super::super::luck_exact::luck_exact_law_with_ranking(
                    &branch_master,
                    &deck,
                    &notes,
                    &[],
                    params,
                    &setup,
                    &play,
                    &delta,
                    None,
                    &mut super::super::luck_exact::LuckExactBudget::default(),
                    || false,
                )
                .unwrap();
                let law = attempt.law.expect("the finite nominal tree completes");
                let probability = f64::from(percent as f32 / 100.0);
                expected += (if threshold == 0 { probability } else { 1.0 - probability })
                    * law
                        .atoms()
                        .iter()
                        .map(|atom| f64::from(atom.score) * atom.mass.numerator as f64 / atom.mass.denominator as f64)
                        .sum::<f64>();
            }
            assert!(
                dp.final_mean.lower <= expected && expected <= dp.final_mean.upper,
                "minimum={minimum}, percent={percent}, nominal={expected}, dp={:?}",
                dp.final_mean
            );
        }
    }

    #[test]
    fn later_native_ranges_cancel_old_random_bonuses_and_keep_all_seed_scores() {
        let (mut master, _, mut params, mut setup, _, _) = fixture();
        master.gekisou_luck_bonus_lots = (0..5)
            .flat_map(|kind| {
                (0..4).map(move |result| crate::master::LuckBonusLotRow {
                    id: kind * 4 + result + 1,
                    chance_lot_type: kind,
                    lot_result: result,
                    weight: 1,
                })
            })
            .collect();
        master.gekisou_ranking_score_bonuses = (1..=3)
            .map(|count| crate::master::GekisouRankingBonusRow {
                id: count,
                mission_pattern: gekisou::mission_pattern(2, 2, 2),
                rank: 1,
                count,
                score_bonus_percent: 250,
            })
            .collect();
        // Leave the native completion stopwatch enough frames to finish before the next range.
        setup.fevers = vec![(100, 500), (3600, 4100), (7200, 7700)];
        let notes: Vec<_> = (1..=18)
            .map(|i| LiveNote {
                note_id: i,
                note_operate_type: 1,
                judgement_type: 1,
                time_ms: i * 100
                    + if i <= 6 {
                        0
                    } else if i <= 12 {
                        3000
                    } else {
                        6000
                    },
            })
            .collect();
        params.converted_note_count = notes.len() as i32;
        params.music_length_ms = 11000;
        let frames: Vec<_> = (0..=110)
            .map(|i| PlayFrame {
                time_ms: i * 100,
                judged: notes
                    .iter()
                    .filter(|note| note.time_ms == i * 100)
                    .map(|note| JudgedNote { note_id: note.note_id, judgement: 5, judgement_time_ms: note.time_ms })
                    .collect(),
            })
            .collect();
        let delta = vec![0.1; frames.len()];
        let play = LivePlay { frames, base_seed: 0 };
        let bounds = luck_score_bounds(&master, &[], &notes, &[], params, &setup, &play, &delta).unwrap();
        assert_eq!(bounds.ranges.len(), 3);
        let mut strict = false;
        for range in bounds.ranges.iter().skip(1) {
            let start = &bounds.queries[range.start_query.unwrap()];
            let end = &bounds.queries[range.end_query];
            let naive_width = (end.mean.upper - end.mean.lower) + (start.mean.upper - start.mean.lower);
            let width = range.mean.upper - range.mean.lower;
            assert!(width <= naive_width + 1e-8);
            strict |= width + 0.1 < naive_width;
        }
        assert!(strict, "the fixture must expose a shared uncertain old bonus");
        let mut scores = std::collections::BTreeSet::new();
        for seed in -16..48 {
            let mut native = LiveModel::new_gekisou(&master, &[], &notes, &[], params, &setup).unwrap();
            native.set_seed(seed);
            for (frame, &dt) in play.frames.iter().zip(&delta) {
                native.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
            }
            assert!(bounds.final_support.lower <= native.score() && native.score() <= bounds.final_support.upper);
            scores.insert(native.score());
        }
        assert!(scores.len() > 1);
    }

    /// Three ranges with Rank bonuses of 250%: three notes at each of five or six times in the first two ranges,
    /// whose Rush may or may not run, and a single note in the third. `fine` plays 10 ms frames and spreads each
    /// three notes over 20 ms (one more note lies just before the second range), so that later filings execute
    /// score frames of earlier snapshots again.
    fn lottery_wide_ranges(fine: bool) -> (Master, Vec<LiveNote>, LiveParams, GekisouSetup, LivePlay, Vec<f32>) {
        let (mut master, _, mut params, mut setup, _, _) = fixture();
        master.gekisou_luck_bonus_lots = (0..5)
            .flat_map(|kind| {
                (0..4).map(move |result| crate::master::LuckBonusLotRow {
                    id: kind * 4 + result + 1,
                    chance_lot_type: kind,
                    lot_result: result,
                    weight: 1,
                })
            })
            .collect();
        master.gekisou_ranking_score_bonuses = (1..=3)
            .map(|count| crate::master::GekisouRankingBonusRow {
                id: count,
                mission_pattern: gekisou::mission_pattern(2, 2, 2),
                rank: 1,
                count,
                score_bonus_percent: 250,
            })
            .collect();
        setup.fevers = vec![(100, 500), (3600, 4100), (7200, 7700)];
        let spread = if fine { [0, 10, 20] } else { [0; 3] };
        let mut times: Vec<i32> = (1..=5).chain(36..=41).flat_map(|t| spread.map(|d| t * 100 + d)).collect();
        if fine {
            times.push(3590);
        }
        times.push(7300);
        times.sort_unstable();
        let notes: Vec<_> = times
            .iter()
            .enumerate()
            .map(|(i, &time_ms)| LiveNote { note_id: i as i32 + 1, note_operate_type: 1, judgement_type: 1, time_ms })
            .collect();
        params.converted_note_count = notes.len() as i32;
        params.music_length_ms = 11000;
        let step = if fine { 10 } else { 100 };
        let frames: Vec<_> = (0..=11000 / step)
            .map(|i| PlayFrame {
                time_ms: i * step,
                judged: notes
                    .iter()
                    .filter(|note| note.time_ms == i * step)
                    .map(|note| JudgedNote { note_id: note.note_id, judgement: 5, judgement_time_ms: note.time_ms })
                    .collect(),
            })
            .collect();
        let delta = vec![step as f32 / 1000.0; frames.len()];
        (master, notes, params, setup, LivePlay { frames, base_seed: 0 }, delta)
    }

    #[test]
    fn range_gains_after_lottery_wide_earlier_ranges_stay_enclosed_and_nonnegative() {
        let (master, notes, params, setup, play, delta) = lottery_wide_ranges(false);
        let bounds = luck_score_bounds(&master, &[], &notes, &[], params, &setup, &play, &delta).unwrap();
        assert_eq!(bounds.ranges.len(), 3);
        for range in &bounds.ranges {
            assert!(range.support.lower >= 0, "{range:?}");
        }
        let mut gains = std::collections::BTreeSet::new();
        for seed in -16..48 {
            let mut native = LiveModel::new_gekisou(&master, &[], &notes, &[], params, &setup).unwrap();
            native.set_seed(seed);
            for (frame, &dt) in play.frames.iter().zip(&delta) {
                native.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
            }
            let gk = native.gk.as_ref().unwrap();
            for range in &bounds.ranges {
                let gain = gk.ctrl.range_score(range.range);
                assert!(range.support.lower <= gain && gain <= range.support.upper, "{range:?} {gain}");
                gains.insert((range.range, gain));
            }
            assert!(bounds.final_support.lower <= native.score() && native.score() <= bounds.final_support.upper);
        }
        assert!(gains.len() > bounds.ranges.len(), "the lottery must change some range gain");
    }

    #[test]
    fn network_range_gains_over_executed_again_frames_stay_enclosed_and_nonnegative() {
        let (master, notes, params, setup, play, delta) = lottery_wide_ranges(true);
        let confirmations: Vec<_> =
            (0..3).map(|range| crate::replay::RankConfirmation { frame: 0, range, rank: 1, percent: 250 }).collect();
        let bounds = luck_score_bounds_with_ranking(
            &master,
            &[],
            &notes,
            &[],
            params,
            &setup,
            &play,
            &delta,
            Some(&confirmations),
        )
        .unwrap();
        assert_eq!(bounds.ranges.len(), 3);
        for range in &bounds.ranges {
            assert!(range.support.lower >= 0, "{range:?}");
        }
        let mut gains = std::collections::BTreeSet::new();
        for seed in -16..48 {
            let mut native = LiveModel::new_gekisou_external(&master, &[], &notes, &[], params, &setup).unwrap();
            native.set_rank_confirmation_timeline(&confirmations).unwrap();
            native.set_seed(seed);
            for (frame, &dt) in play.frames.iter().zip(&delta) {
                native.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
            }
            let gk = native.gk.as_ref().unwrap();
            for range in &bounds.ranges {
                let gain = gk.ctrl.range_score(range.range);
                assert!(range.support.lower <= gain && gain <= range.support.upper, "{range:?} {gain}");
                gains.insert((range.range, gain));
            }
            assert!(bounds.final_support.lower <= native.score() && native.score() <= bounds.final_support.upper);
        }
        assert!(gains.len() > bounds.ranges.len(), "the lottery must change some range gain");
    }

    #[test]
    fn compact_summary_preserves_the_same_proof_without_diagnostic_rows() {
        let (master, notes, params, setup, play, delta) = fixture();
        let full = luck_score_bounds(&master, &[], &notes, &[], params, &setup, &play, &delta).unwrap();
        let skills = luck_skills(&master).unwrap();
        let compact = luck_score_bounds_internal(
            &master,
            &skills,
            &[],
            &notes,
            &[],
            params,
            &setup,
            &play,
            &delta,
            None,
            false,
            None,
            false,
            None,
        )
        .unwrap();
        assert!(compact.queries.is_empty() && compact.final_notes.is_empty());
        assert_eq!(compact.final_mean.lower, full.final_mean.lower);
        assert_eq!(compact.final_mean.upper, full.final_mean.upper);
        let summary = luck_score_summary_with_ranking(
            &master,
            &luck_skills(&master).unwrap(),
            &[],
            &notes,
            &[],
            params,
            &setup,
            &play,
            &delta,
            None,
        )
        .unwrap();
        assert_eq!(summary.exact_constant_score, Some(full.final_support.lower));
        assert_eq!(summary.final_support.lower, summary.final_support.upper);
        assert_eq!(summary.exact_final_life, full.exact_final_life);
    }

    #[test]
    fn dynamic_max_does_not_inherit_a_constant_life_or_score_certificate() {
        let (mut master, notes, params, setup, play, delta) = fixture();
        master.live_skill_effects.push(crate::master::LiveSkillEffectRow {
            id: 901,
            live_skill_id: 901,
            level: 1,
            skill_effect_type: 3000,
            effect_value: 100,
            ..Default::default()
        });
        master.reindex().unwrap();
        let deck = [Performer { live_skill: Some((901, 1)), ..Default::default() }];
        let events = [(0, 150)];
        let mut native = LiveModel::new_gekisou(&master, &deck, &notes, &events, params, &setup).unwrap();
        native.run_timed(&play, &delta).unwrap();
        let certificate = luck_score_summary_with_ranking(
            &master,
            &luck_skills(&master).unwrap(),
            &deck,
            &notes,
            &events,
            params,
            &setup,
            &play,
            &delta,
            None,
        );
        assert!(matches!(certificate, Err(Error::Unsupported(message)) if message.contains("reader/cache")));
        // The native effect stays implemented; only the unjustified single-path certificate is refused.
        assert!(native.life.max_life() > 1000);
    }

    #[test]
    fn exact_life_certificate_covers_misses_recovery_and_random_luck_paths() {
        let (mut master, notes, params, setup, mut play, delta) = fixture();
        master.judgement_parameters.push(crate::master::JudgementParameterRow {
            id: 2,
            note_simulate_judgement: 1,
            score_percent: 0,
            damage: 350,
        });
        master.gekisou_luck_base_points.push(crate::master::LuckBasePointRow {
            id: 2,
            note_simulate_judgement: 1,
            weight: 1,
            base_point: 0,
            ..Default::default()
        });
        master.gekisou_luck_bonus_lots = (0..5)
            .flat_map(|kind| {
                (0..4).map(move |result| crate::master::LuckBonusLotRow {
                    id: kind * 4 + result + 1,
                    chance_lot_type: kind,
                    lot_result: result,
                    weight: 1,
                })
            })
            .collect();
        master.skill_conditions.push(crate::master::SkillConditionRow {
            id: 1,
            condition_type: 4010,
            condition_values: vec![],
            is_positive: true,
            condition_target_ids: vec![],
        });
        master.skill_condition_sets.push(crate::master::SkillConditionSetRow {
            id: 1,
            group: 1,
            condition_ids: vec![1],
        });
        master.support_skill_effects.push(crate::master::SupportSkillEffectRow {
            id: 1,
            support_skill_id: 1,
            level: 1,
            skill_trigger_type: ONE_SHOT,
            skill_trigger_condition_group: 1,
            skill_effect_type: 3001,
            effect_value: 125,
            ..Default::default()
        });
        master.reindex().unwrap();
        play.frames
            .iter_mut()
            .flat_map(|frame| &mut frame.judged)
            .find(|note| note.note_id == notes[0].note_id)
            .unwrap()
            .judgement = 1;
        let deck = [Performer { support_skills: vec![(1, 1)], ..Default::default() }];
        let events = [(0, 300)];
        let summary = luck_score_summary_with_ranking(
            &master,
            &luck_skills(&master).unwrap(),
            &deck,
            &notes,
            &events,
            params,
            &setup,
            &play,
            &delta,
            None,
        )
        .unwrap();
        assert_eq!(summary.exact_final_life, Some(775));
        let mut common_life = None;
        let mut scores = std::collections::BTreeSet::new();
        for seed in -16..48 {
            let mut native = LiveModel::new_gekisou(&master, &deck, &notes, &events, params, &setup).unwrap();
            native.set_seed(seed);
            let mut life = Vec::new();
            for (frame, &dt) in play.frames.iter().zip(&delta) {
                native.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
                life.push(native.current_life());
            }
            assert!(native.draws() > 0);
            assert_eq!(Some(native.current_life()), summary.exact_final_life);
            assert!(life.windows(2).any(|v| v[1] < v[0]), "the Miss must reduce life");
            assert!(life.windows(2).any(|v| v[1] > v[0]), "the support event must recover life");
            if let Some(reference) = &common_life {
                assert_eq!(&life, reference, "seed {seed}: the whole life trajectory is shared");
            } else {
                common_life = Some(life);
            }
            scores.insert(native.score());
        }
        assert!(scores.len() > 1, "the fixture must exercise distinct lottery-dependent scores");
        // This proof does not admit a recovery whose trigger reads Rush, even if one sampled path ends full.
        master.skill_conditions[0].condition_type = 7021;
        master.reindex().unwrap();
        assert!(
            luck_skills(&master)
                .and_then(|skills| luck_score_summary_with_ranking(
                    &master, &skills, &deck, &notes, &events, params, &setup, &play, &delta, None,
                ))
                .is_err()
        );
    }

    #[test]
    fn lottery_free_live_without_luck_range_plays_every_native_seed() {
        let (mut master, notes, params, mut setup, play, delta) = fixture();
        setup.missions = vec![1, 3, 1];
        master.skill_conditions.extend([
            crate::master::SkillConditionRow {
                id: 1,
                condition_type: 4011,
                condition_values: vec![50],
                is_positive: true,
                condition_target_ids: vec![],
            },
            crate::master::SkillConditionRow {
                id: 2,
                condition_type: 4010,
                condition_values: vec![],
                is_positive: true,
                condition_target_ids: vec![],
            },
        ]);
        master.skill_condition_sets.extend([
            crate::master::SkillConditionSetRow { id: 1, group: 1, condition_ids: vec![1] },
            crate::master::SkillConditionSetRow { id: 2, group: 2, condition_ids: vec![2] },
        ]);
        master.skill_effect_settings.push(crate::master::SkillEffectSettingRow {
            id: 1,
            skill_effect_type: 11003,
            phase: 1,
        });
        master.gekisou_skills.push(crate::master::SkillRow { id: 1, gekisou_mission_type: 1, ..Default::default() });
        // A probability-gated lottery chain: it feeds only the luck machine of a LUCK range.
        master.gekisou_skill_effects.push(crate::master::GekisouSkillEffectRow {
            id: 1,
            skill_id: 1,
            level: 1,
            skill_trigger_type: ONE_SHOT,
            skill_trigger_condition_group: 1,
            skill_effect_type: 11003,
            effect_value: 1,
            ..Default::default()
        });
        // A deterministic score-up on the skill event keeps the score nontrivial.
        master.support_skill_effects.push(crate::master::SupportSkillEffectRow {
            id: 1,
            support_skill_id: 1,
            level: 1,
            skill_trigger_type: ONE_SHOT,
            skill_trigger_condition_group: 2,
            skill_effect_type: 2000,
            activation_time_second: 1.0,
            effect_value: 30_000,
            ..Default::default()
        });
        master.reindex().unwrap();
        let deck = [Performer {
            gekisou_skill: Some((1, 1)),
            gekisou_mission_type: 1,
            support_skills: vec![(1, 1)],
            ..Default::default()
        }];
        let events = [(0, 300)];
        let skills = luck_skills(&master).unwrap();
        let mut free = LiveModel::new_gekisou(&master, &deck, &notes, &events, params, &setup).unwrap();
        assert!(free.reads_lottery());
        prepare_lottery_free(&mut free, &skills).unwrap();
        let score = free.run_timed(&play, &delta).unwrap();
        assert_eq!(free.draws(), 0);
        let summary = luck_score_summary_with_ranking(
            &master, &skills, &deck, &notes, &events, params, &setup, &play, &delta, None,
        )
        .unwrap();
        assert_eq!(summary.exact_constant_score, Some(score));
        assert_eq!(summary.exact_final_life, Some(free.current_life()));
        let plain =
            LiveModel::new_gekisou(&master, &[], &notes, &events, params, &setup).unwrap().run_timed(&play, &delta);
        assert!(score > plain.unwrap(), "the score-up must count");
        for seed in -16..48 {
            let mut native = LiveModel::new_gekisou(&master, &deck, &notes, &events, params, &setup).unwrap();
            native.set_seed(seed);
            for (frame, &dt) in play.frames.iter().zip(&delta) {
                native.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
            }
            assert!(native.draws() > 0, "seed {seed}: the probability must draw natively");
            assert_eq!(native.score(), score, "seed {seed}");
            assert_eq!(native.current_life(), free.current_life(), "seed {seed}");
        }
        // A deck reading no probability plays as it is; a LUCK range draws lotteries.
        let mut quiet = LiveModel::new_gekisou(&master, &[], &notes, &events, params, &setup).unwrap();
        prepare_lottery_free(&mut quiet, &skills).unwrap();
        assert!(quiet.score.calc.luck_weight.is_none());
        setup.missions = vec![2, 3, 1];
        let mut luck = LiveModel::new_gekisou(&master, &deck, &notes, &events, params, &setup).unwrap();
        assert!(prepare_lottery_free(&mut luck, &skills).is_err());
    }

    #[test]
    fn fixed_scores_outside_the_frame_array_remain_in_later_range_snapshots() {
        let (mut master, _, mut params, mut setup, _, _) = fixture();
        master.gekisou_ranking_score_bonuses = (1..=2)
            .map(|count| crate::master::GekisouRankingBonusRow {
                id: count,
                mission_pattern: 1,
                rank: 1,
                count,
                score_bonus_percent: 50,
            })
            .collect();
        params.music_level = 5;
        params.converted_note_count = 2;
        params.music_length_ms = 5000;
        params.score_music_length_ms = Some(1);
        setup.fevers = vec![(100, 3000), (500, 3500)];
        setup.missions = vec![1, 1, 1];
        let notes: Vec<_> = [200, 1000]
            .into_iter()
            .enumerate()
            .map(|(id, time_ms)| LiveNote { note_id: id as i32, note_operate_type: 1, judgement_type: 1, time_ms })
            .collect();
        let frames = (0..=50)
            .map(|frame| PlayFrame {
                time_ms: frame * 100,
                judged: notes
                    .iter()
                    .filter(|note| note.time_ms == frame * 100)
                    .map(|note| JudgedNote { note_id: note.note_id, judgement: 5, judgement_time_ms: note.time_ms })
                    .collect(),
            })
            .collect();
        let play = LivePlay { frames, base_seed: 0 };
        let delta = vec![0.1; play.frames.len()];
        let mut native = LiveModel::new_gekisou(&master, &[], &notes, &[], params, &setup).unwrap();
        assert_eq!(native.run_timed(&play, &delta).unwrap(), 5250);
        let bounds = luck_score_bounds(&master, &[], &notes, &[], params, &setup, &play, &delta).unwrap();
        assert!(bounds.final_mean.lower <= 5250.0 && bounds.final_mean.upper >= 5250.0);
        assert!(bounds.final_mean.upper < 5300.0);
        let second = &bounds.ranges[1];
        let start = &bounds.queries[second.start_query.unwrap()];
        assert!(start.mean.lower <= 3000.0 && start.mean.upper >= 3000.0);
        assert!(second.bonus_mean.lower <= 750.0 && second.bonus_mean.upper >= 750.0);
    }

    #[test]
    fn deterministic_extension_cumulative_combo_and_conversion_keep_native_trace_enclosed() {
        let (mut master, notes, params, setup, play, delta) = fixture();
        for (id, kind) in [(101, 4010), (102, 7010), (103, 7013)] {
            master.skill_conditions.push(crate::master::SkillConditionRow {
                id,
                condition_type: kind,
                condition_values: vec![],
                is_positive: true,
                condition_target_ids: vec![],
            });
            master.skill_condition_sets.push(crate::master::SkillConditionSetRow {
                id,
                group: id,
                condition_ids: vec![id],
            });
        }
        master
            .skill_targets
            .push(serde_json::from_value(json!({"_id":101,"_skillTargetType":4,"_judgement":5})).unwrap());
        master.cumulative_conditions.push(
            serde_json::from_value(json!({"_id":101,"_skillCumulativeConditionType":1000,
            "_conditionValues":[1],"_conditionTargetIDs":[101],"_maxCumulativeCount":99}))
            .unwrap(),
        );
        for (id, ty) in [(101, 2000), (102, 2002), (103, 15000), (104, 12006), (105, 2001)] {
            master
                .skill_effect_settings
                .push(serde_json::from_value(json!({"_id":id,"_skillEffectType":ty,"_phase":2})).unwrap());
        }
        master.live_skill_effects.push(
            serde_json::from_value(json!({"_id":101,"_liveSkillID":101,"_level":1,
            "_skillEffectType":2000,"_effectValue":2500,"_activationTimeSecond":0.35}))
            .unwrap(),
        );
        master.live_skill_effects.push(
            serde_json::from_value(json!({"_id":102,"_liveSkillID":101,"_level":1,
            "_skillEffectType":2002,"_effectValue":1200,"_activationTimeSecond":0.35}))
            .unwrap(),
        );
        master.support_skill_effects.push(
            serde_json::from_value(json!({"_id":103,"_supportSkillID":101,"_level":1,
            "_skillTriggerType":1,"_skillTriggerConditionGroup":101,"_skillEffectType":15000,"_effectValue":400}))
            .unwrap(),
        );
        master.support_skill_effects.push(
            serde_json::from_value(json!({"_id":104,"_supportSkillID":102,"_level":1,
            "_skillTriggerType":1,"_skillTriggerConditionGroup":101,"_skillEffectType":12006,"_effectValue":4,
            "_effectLimitCount":2,"_skillTargetIDs":[101],"_activationTimeSecond":0.5}))
            .unwrap(),
        );
        master.gekisou_skills.push(serde_json::from_value(json!({"_id":101,"_gekisouMissionType":2})).unwrap());
        master.gekisou_skill_effects.push(
            serde_json::from_value(json!({"_id":105,"_gekisouSkillID":101,"_level":1,
            "_skillTriggerType":1,"_skillTriggerConditionGroup":102,"_skillReleaseConditionGroup":103,
            "_skillEffectType":2001,"_effectValue":100,"_maxEffectValue":5000,"_skillCumulativeConditionID":101}))
            .unwrap(),
        );
        master.judgement_parameters.push(
            serde_json::from_value(json!({"_id":4,"_noteSimulateJudgement":4,
            "_scorePercent":80,"_damage":30}))
            .unwrap(),
        );
        master.gekisou_luck_base_points.push(
            serde_json::from_value(json!({"_id":4,"_noteCategory":0,"_noteSimulateJudgement":4,
            "_weight":1,"_basePoint":60}))
            .unwrap(),
        );
        master.reindex().unwrap();
        let deck = [Performer {
            live_skill: Some((101, 1)),
            support_skills: vec![(101, 1), (102, 1)],
            gekisou_skill: Some((101, 1)),
            gekisou_mission_type: 2,
            ..Default::default()
        }];
        let events = [(0, 80)];
        let bounds = luck_score_bounds(&master, &deck, &notes, &events, params, &setup, &play, &delta).unwrap();
        let mut reference = None;
        for seed in -16..32 {
            let mut native = LiveModel::new_gekisou(&master, &deck, &notes, &events, params, &setup).unwrap();
            native.set_seed(seed);
            let mut trace = Vec::new();
            for (frame, &dt) in play.frames.iter().zip(&delta) {
                native.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
                trace.push((
                    native.current_life(),
                    native.frame_judgements().to_vec(),
                    native.gekisou_ranges().iter().map(|range| (range.combo, range.just_count)).collect::<Vec<_>>(),
                ));
            }
            assert!(bounds.final_support.lower <= native.score() && native.score() <= bounds.final_support.upper);
            assert_eq!(Some(native.current_life()), bounds.exact_final_life);
            assert!(
                trace.iter().flat_map(|(_, judgements, _)| judgements).any(|(_, j, _)| *j == 4),
                "conversion must execute"
            );
            if let Some(reference) = &reference {
                assert_eq!(&trace, reference);
            } else {
                reference = Some(trace);
            }
        }
        let condition = master.skill_conditions.iter_mut().find(|c| c.id == 101).unwrap();
        condition.condition_type = 4011;
        condition.condition_values = vec![50];
        master.reindex().unwrap();
        assert!(
            luck_score_bounds(&master, &deck, &notes, &events, params, &setup, &play, &delta).is_err(),
            "random conversion/extension cannot inherit the deterministic closure proof"
        );
    }

    #[test]
    fn deterministic_combo_queries_replace_the_global_table_hull() {
        let (mut master, notes, params, mut setup, play, delta) = fixture();
        setup.missions = vec![1, 1, 1];
        master.combo_score_bonuses = vec![
            crate::master::ComboScoreBonusRow {
                id: 1,
                combo_bonus_type: 0,
                required_combo_count: 1,
                bonus_factor: 0.3,
            },
            crate::master::ComboScoreBonusRow {
                id: 2,
                combo_bonus_type: 1,
                required_combo_count: 2,
                bonus_factor: 0.4,
            },
        ];
        let bounds = luck_score_bounds(&master, &[], &notes, &[], params, &setup, &play, &delta).unwrap();
        let mut native = LiveModel::new_gekisou(&master, &[], &notes, &[], params, &setup).unwrap();
        let score = native.run_timed(&play, &delta).unwrap();
        assert!(bounds.final_mean.lower <= f64::from(score) && bounds.final_mean.upper >= f64::from(score));
        assert!(bounds.final_mean.upper - bounds.final_mean.lower < 1e-8);
        assert!(bounds.final_notes.iter().all(|note| note.combo.lower == note.combo.upper));
    }

    #[test]
    fn external_rank_uses_frame_snapshots_and_initial_zero_without_solo_queries() {
        let (master, notes, params, mut setup, play, delta) = fixture();
        setup.fevers[0].0 = 0;
        let confirmations = [crate::replay::RankConfirmation { frame: 0, range: 0, rank: 1, percent: 50 }];
        let bounds = luck_score_bounds_with_ranking(
            &master,
            &[],
            &notes,
            &[],
            params,
            &setup,
            &play,
            &delta,
            Some(&confirmations),
        )
        .unwrap();
        let mut native = LiveModel::new_gekisou_external(&master, &[], &notes, &[], params, &setup).unwrap();
        native.set_rank_confirmation_timeline(&confirmations).unwrap();
        let score = native.run_timed(&play, &delta).unwrap();
        assert_eq!(bounds.actual_queries, 2 * play.frames.len());
        assert_eq!(bounds.ranges.len(), 1);
        assert_eq!(bounds.ranges[0].start_query, None);
        assert!(bounds.final_mean.lower <= f64::from(score) && bounds.final_mean.upper >= f64::from(score));
    }

    #[test]
    fn terminal_probability_link_requires_a_finished_native_schedule() {
        let (master, notes, params, setup, mut play, mut delta) = fixture();
        play.frames.truncate(6);
        delta.truncate(6);
        assert!(matches!(
            luck_score_bounds(&master, &[], &notes, &[], params, &setup, &play, &delta),
            Err(Error::Unsupported(_))
        ));
    }
}
