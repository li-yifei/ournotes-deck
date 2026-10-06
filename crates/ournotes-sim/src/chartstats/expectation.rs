//! Nominal expectations and outward arithmetic for chart measurements.

use std::sync::Arc;

use serde::{Serialize, Serializer};

use super::{
    CHECK_POWER, KIND_TYPES, Kind, Live, MAX_GEKISOU_FEVERS, POWER, RangeInfo, Rng, UNIT_VALUE, check_deck,
    check_skills, kind_factor,
};
use crate::{
    Error,
    live::{
        certified::F64Interval,
        full::{self, LuckDpCertifiedResult, LuckRangeMoments, LuckScoreExpectation, Performer, RealBounds},
    },
    master::Master,
    num::FxHashMap,
    replay::RankConfirmation,
};

/// The interval's center and half-width, in that order.
pub type Estimate = [f64; 2];

/// Decimal centers and radii retain the original enclosure. Point scores use millipoints, counts use
/// five fractional digits, and per-power weights use twelve. The radius includes the center's rounding.
fn rounded(value: Estimate, digits: i32) -> Estimate {
    let unit = 10f64.powi(digits);
    let center = (value[0] * unit).round() / unit;
    if !center.is_finite() || value[1] == 0.0 && center == value[0] {
        return value;
    }
    let source = interval(value);
    let needed = (center - source.lower()).abs().max((source.upper() - center).abs());
    let count = (needed * unit).ceil();
    if !count.is_finite() {
        return value;
    }
    let radius = count / unit;
    [center, if radius >= needed { radius } else { (count + 1.0) / unit }]
}

pub(super) fn serialize_points<S: Serializer>(value: &Estimate, serializer: S) -> Result<S::Ok, S::Error> {
    rounded(*value, 3).serialize(serializer)
}

pub(super) fn serialize_counts<S: Serializer>(value: &Estimate, serializer: S) -> Result<S::Ok, S::Error> {
    rounded(*value, 5).serialize(serializer)
}

fn weight_row(values: &[Estimate]) -> Vec<Estimate> {
    values.iter().map(|&value| rounded(value, 12)).collect()
}

pub(super) fn serialize_weights<S: Serializer>(values: &[Vec<Estimate>], serializer: S) -> Result<S::Ok, S::Error> {
    values.iter().map(|row| weight_row(row)).collect::<Vec<_>>().serialize(serializer)
}

pub(super) fn serialize_optional_weights<S: Serializer>(
    values: &Option<Vec<Estimate>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    values.as_ref().map(|row| weight_row(row)).serialize(serializer)
}

pub(super) fn serialize_optional_matrix<S: Serializer>(
    values: &Option<Vec<Vec<Estimate>>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    values.as_ref().map(|matrix| matrix.iter().map(|row| weight_row(row)).collect::<Vec<_>>()).serialize(serializer)
}

#[allow(clippy::type_complexity)]
fn serialize_range_weights<S: Serializer>(
    values: &Option<Vec<Option<Vec<Vec<Estimate>>>>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    values
        .as_ref()
        .map(|kinds| {
            kinds
                .iter()
                .map(|kind| kind.as_ref().map(|matrix| matrix.iter().map(|row| weight_row(row)).collect::<Vec<_>>()))
                .collect::<Vec<_>>()
        })
        .serialize(serializer)
}

fn serialize_lot_results<S: Serializer>(values: &[Estimate; 4], serializer: S) -> Result<S::Ok, S::Error> {
    values.map(|value| rounded(value, 5)).serialize(serializer)
}

pub(super) fn estimate(value: F64Interval) -> Estimate {
    let center = value.lower() + (value.upper() - value.lower()) / 2.0;
    let radius = (center - value.lower()).max(value.upper() - center);
    [center, if radius == 0.0 { 0.0 } else { radius.next_up() }]
}

pub(super) fn interval(value: Estimate) -> F64Interval {
    if value[1] == 0.0 {
        F64Interval::point(value[0]).expect("finite measurement")
    } else {
        F64Interval::new((value[0] - value[1]).next_down(), (value[0] + value[1]).next_up())
            .expect("finite measurement")
    }
}

pub(super) fn real(value: RealBounds) -> F64Interval {
    F64Interval::new(value.lower, value.upper).expect("ordered expectation")
}

pub(super) fn delta(with: RealBounds, without: RealBounds) -> F64Interval {
    real(with).subtract(real(without))
}

pub(super) fn scaled(value: F64Interval, divisor: f64) -> Result<F64Interval, Error> {
    value.divide(F64Interval::point(divisor)?)
}

/// A range's expected scores and deterministic judgement counters.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedRange {
    #[serde(serialize_with = "serialize_points")]
    pub range_score: Estimate,
    #[serde(serialize_with = "serialize_points")]
    pub rank_bonus: Estimate,
    #[serde(serialize_with = "serialize_points")]
    pub rank_bonus_perfect: Estimate,
    #[serde(serialize_with = "serialize_points")]
    pub range_score_perfect: Estimate,
    pub max_combo: i32,
    pub just_count: i32,
    #[serde(serialize_with = "serialize_points")]
    pub luck_points: Estimate,
    #[serde(serialize_with = "serialize_lot_results")]
    pub lot_results: [Estimate; 4],
}

/// A random score-only deck checked against its full nominal expectation.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectationCheck {
    pub deck: Vec<Option<(usize, i64)>>,
    pub ranks: Option<Vec<i32>>,
    #[serde(serialize_with = "serialize_points")]
    pub expected: Estimate,
    #[serde(serialize_with = "serialize_points")]
    pub predicted: Estimate,
    pub bound: f64,
}

/// Gekisou measurements under independent nominal lottery and skill probabilities.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedStats {
    #[serde(serialize_with = "serialize_points")]
    pub score: Estimate,
    #[serde(serialize_with = "serialize_points")]
    pub score_perfect: Estimate,
    pub ranges: Vec<ExpectedRange>,
    #[serde(serialize_with = "serialize_weights")]
    pub weights: Vec<Vec<Estimate>>,
    #[serde(serialize_with = "serialize_range_weights")]
    pub range_weights: Option<Vec<Option<Vec<Vec<Estimate>>>>>,
    pub check: ExpectationCheck,
    pub rank_check: Option<ExpectationCheck>,
}

/// A chart fixes the master, note schedule and range setup. Measurement masters add only ordinary score
/// rows (2000, 2002, 2004, 2005), which write neither life nor judgements nor lottery state. The full score
/// recorder proves each such row's deterministic schedule before using the shared curve. The key retains
/// the complete Gekisou formation, play choice, power and rank arrivals; changes to these record a new curve.
pub(super) struct Evaluator<'a, 'm> {
    live: &'a Live<'m>,
    master: &'a Master,
    skills: full::LuckSkills,
    curves: FxHashMap<String, Arc<LuckDpCertifiedResult>>,
}

impl<'a, 'm> Evaluator<'a, 'm> {
    pub(super) fn new(live: &'a Live<'m>, master: &'a Master) -> Result<Self, Error> {
        Ok(Self { live, master, skills: full::luck_skills(master)?, curves: FxHashMap::default() })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn run(
        &mut self,
        master: &Master,
        formation: &[Performer],
        live_skills: &[Option<i64>],
        power: i32,
        perfect: bool,
        ranking: Option<&[RankConfirmation]>,
    ) -> Result<LuckScoreExpectation, Error> {
        for id in live_skills.iter().flatten() {
            if master
                .live_skill_effects
                .iter()
                .any(|r| r.live_skill_id == *id && !KIND_TYPES.contains(&r.skill_effect_type))
            {
                return Err(Error::Input("a chart measurement requires score-only live skills".into()));
            }
        }
        let live = self.live;
        let g = live.gekisou.as_ref().ok_or_else(|| Error::Input("expectation without Gekisou".into()))?;
        let play = if perfect { &g.perfect } else { &live.play };
        let params = full::LiveParams { total_power: power, ..live.params };
        let key = format!("{formation:?}/{power}/{perfect}/{ranking:?}");
        let probability = match self.curves.get(&key) {
            Some(value) => value.clone(),
            None => {
                let value = if g.setup.missions.iter().take(g.setup.fevers.len()).any(|&m| m == 2) {
                    full::luck_rush_dp_certified_with_moments(
                        self.master,
                        &self.skills,
                        live.notes,
                        live.events,
                        params,
                        &g.setup,
                        play,
                        &g.dt,
                        formation,
                        None,
                        ranking,
                    )?
                } else {
                    LuckDpCertifiedResult {
                        steps: Vec::new(),
                        probes: vec![false; self.skills.shapes.len()],
                        range_moments: vec![LuckRangeMoments::default(); g.setup.fevers.len()],
                        peak_states: 1,
                        transitions: 0,
                    }
                };
                self.curves.entry(key).or_insert_with(|| Arc::new(value)).clone()
            }
        };
        let deck: Vec<_> = (0..live.positions.max(formation.len()).max(1))
            .map(|k| Performer {
                live_skill: live_skills.get(k).copied().flatten().map(|id| (id, 1)),
                ..formation.get(k).cloned().unwrap_or_default()
            })
            .collect();
        full::luck_score_expectation_for_chart(
            master,
            &self.skills,
            &deck,
            live.notes,
            live.events,
            params,
            &g.setup,
            play,
            &g.dt,
            ranking,
            probability,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn check(
        &mut self,
        kinds: &[Kind],
        master: &Master,
        formation: &[Performer],
        deck: &[Option<(usize, i64)>],
        ranks: Option<&[i32]>,
        infos: &[RangeInfo],
        base: F64Interval,
        weight: impl Fn(usize, usize) -> F64Interval,
        floors: f64,
        slack: f64,
    ) -> Result<ExpectationCheck, Error> {
        let ranking: Option<Vec<_>> = ranks
            .map(|ranks| {
                ranks
                    .iter()
                    .zip(infos)
                    .enumerate()
                    .map(|(range, (&rank, info))| {
                        Ok(RankConfirmation { frame: 0, range, rank, percent: info.percent(rank)? })
                    })
                    .collect::<Result<_, Error>>()
            })
            .transpose()?;
        let expected = self.run(master, formation, &check_skills(deck), CHECK_POWER, false, ranking.as_deref())?;
        let mut per_power = base;
        let mut gain = 0.0;
        for (k, value) in deck.iter().enumerate() {
            if let Some((kind, value)) = *value {
                let factor = kind_factor(kinds[kind].effect_type, value);
                per_power = per_power.add(weight(kind, k).multiply(F64Interval::point(factor)?));
                gain += factor.abs();
            }
        }
        let predicted = per_power.scale_integer(i128::from(CHECK_POWER));
        let scale = f64::from(CHECK_POWER) / f64::from(POWER);
        let bound = floors * (1.0 + scale * (1.0 + 2.0 * gain))
            + slack * scale * gain
            + 4e-6 * predicted.lower().abs().max(predicted.upper().abs());
        let expected = real(expected.final_mean);
        let error = (expected.lower() - predicted.upper()).abs().max((expected.upper() - predicted.lower()).abs());
        if !error.is_finite() || error > bound {
            return Err(Error::Domain(format!(
                "chart expectation check at ranks {ranks:?}: expected {expected:?}, predicted {predicted:?}, difference {error} points (bound {bound})"
            )));
        }
        Ok(ExpectationCheck {
            deck: deck.to_vec(),
            ranks: ranks.map(<[i32]>::to_vec),
            expected: estimate(expected),
            predicted: estimate(predicted),
            bound,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn chart_expectation(
    live: &Live<'_>,
    kinds: &[Kind],
    infos: &[RangeInfo],
    linear: bool,
    reads_rank: &[bool],
    rng: &mut Rng,
    rank_rng: &mut Rng,
    judged: i32,
) -> Result<ExpectedStats, Error> {
    let mut eval = Evaluator::new(live, live.master)?;
    let none = vec![None; live.positions];
    let formation = vec![Performer::default(); live.positions.max(1)];
    let best = eval.run(live.master, &formation, &none, POWER, false, None)?;
    let perfect = eval.run(live.master, &formation, &none, POWER, true, None)?;
    // The score recorder's admission proof closes all judgement/combo writers independently of lotteries.
    let (_, counters, _) = live.run_counted(&live.play, live.master, &formation, &none, POWER, 0, None)?;
    let ranges: Vec<_> = best
        .ranges
        .iter()
        .zip(&perfect.ranges)
        .zip(counters)
        .map(|((r, p), counter)| ExpectedRange {
            range_score: estimate(real(r.mean)),
            rank_bonus: estimate(real(r.bonus_mean)),
            rank_bonus_perfect: estimate(real(p.bonus_mean)),
            range_score_perfect: estimate(real(p.mean)),
            max_combo: counter.max_combo,
            just_count: counter.just_count,
            luck_points: estimate(real(r.luck_points_mean.expect("range indicators requested"))),
            lot_results: r.lot_results_mean.expect("range indicators requested").map(|v| estimate(real(v))),
        })
        .collect();
    let mut weights = vec![vec![[0.0; 2]; live.positions]; kinds.len()];
    let mut range_weights = vec![vec![vec![[0.0; 2]; ranges.len()]; live.positions]; kinds.len()];
    for (kind_index, kind) in kinds.iter().enumerate() {
        let divisor = f64::from(POWER) * kind_factor(kind.effect_type, UNIT_VALUE);
        for k in 0..live.positions {
            let mut live_skills = none.clone();
            live_skills[k] = Some(super::KIND_SKILL_BASE - kind_index as i64);
            let measured = eval.run(live.measure, &formation, &live_skills, POWER, false, None)?;
            weights[kind_index][k] = estimate(scaled(delta(measured.final_mean, best.final_mean), divisor)?);
            for (i, (r, b)) in measured.ranges.iter().zip(&best.ranges).enumerate() {
                range_weights[kind_index][k][i] = estimate(scaled(delta(r.mean, b.mean), divisor)?);
            }
        }
    }
    let range_weights: Option<Vec<_>> =
        linear.then(|| range_weights.into_iter().zip(reads_rank).map(|(w, &reads)| (!reads).then_some(w)).collect());
    let usable: Vec<_> = (0..kinds.len()).collect();
    let (deck, rows) = check_deck(kinds, &usable, live.positions, rng);
    let master = Live::master_with(live.master, &rows);
    let floors = f64::from(judged) + MAX_GEKISOU_FEVERS as f64;
    let check = eval.check(
        kinds,
        &master,
        &formation,
        &deck,
        None,
        infos,
        scaled(real(best.final_mean), f64::from(POWER))?,
        |ki, k| interval(weights[ki][k]),
        floors,
        0.0,
    )?;
    let mut rank_check = None;
    if let Some(rw) = &range_weights
        && !ranges.is_empty()
        && deck.iter().flatten().all(|&(ki, _)| rw[ki].is_some())
    {
        let ranks: Vec<_> = infos.iter().map(|_| 1 + rank_rng.below(super::RANKS) as i32).collect();
        let mut base = real(best.final_mean);
        let shifts: Vec<_> = infos
            .iter()
            .zip(&ranks)
            .map(|(info, &rank)| Ok((info.percent(rank)? - info.percent(1)?) as f64 / 100.0))
            .collect::<Result<_, Error>>()?;
        for ((range, info), &rank) in ranges.iter().zip(infos).zip(&ranks) {
            base = base
                .subtract(interval(range.rank_bonus))
                .add(interval(range.range_score).multiply(F64Interval::point(info.percent(rank)? as f64 / 100.0)?));
        }
        // Each transformed bonus replaces one integer truncation by its real-valued mean product.
        rank_check = Some(eval.check(
            kinds,
            &master,
            &formation,
            &deck,
            Some(&ranks),
            infos,
            scaled(base, f64::from(POWER))?,
            |ki, k| {
                shifts.iter().enumerate().fold(interval(weights[ki][k]), |sum, (i, &d)| {
                    sum.add(
                        interval(rw[ki].as_ref().expect("checked rank weights")[k][i])
                            .multiply(F64Interval::point(d).unwrap()),
                    )
                })
            },
            floors + infos.len() as f64,
            2.0 * infos.len() as f64,
        )?);
    }
    Ok(ExpectedStats {
        score: estimate(real(best.final_mean)),
        score_perfect: estimate(real(perfect.final_mean)),
        ranges,
        weights,
        range_weights,
        check,
        rank_check,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_and_half_width_preserve_outward_endpoints() {
        for (lo, hi) in [(0.0, 0.0), (-1.0, 4.0), (1e6, 1e6 + 0.000001), (-1e-20, 1e-20)] {
            let original = F64Interval::new(lo, hi).unwrap();
            let published = estimate(original);
            let restored = interval(published);
            assert!(restored.lower() <= lo && restored.upper() >= hi);
            assert!(published[1] >= 0.0);
        }
    }

    #[test]
    fn decimal_intervals_enclose_the_original_values_at_every_output_precision() {
        for (lo, hi) in [(-1.23456789, 9.87654321), (0.0, 0.0), (17.0, 17.0), (1e6, 1e6 + 1e-7), (1e-20, 2e-20)] {
            let original = F64Interval::new(lo, hi).unwrap();
            for digits in [3, 5, 12] {
                let published = rounded(estimate(original), digits);
                let restored = interval(published);
                assert!(restored.lower() <= lo && restored.upper() >= hi, "{published:?}, digits={digits}");
                assert!(published[1] >= 0.0);
            }
        }
    }
}
