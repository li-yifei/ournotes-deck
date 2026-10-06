//! Single-shape Gekisou aptitude under independent nominal lottery and skill probabilities.
//!
//! A member skill acts alone on one performer. A support skill is paired with an effect-free member skill
//! of its mission. Member-target conditions are measured both ways. The expected increments compare these
//! formations with the chart's no-skill expectation; ordinary plain score-up weights supply the cross terms.

use serde::Serialize;

use super::expectation::{Evaluator, delta, estimate, interval, real, scaled};
use super::{
    Estimate, ExpectationCheck, ExpectedStats, KIND_SKILL_BASE, Kind, Live, MAX_GEKISOU_FEVERS, POWER, RangeInfo, Rng,
    UNIT_VALUE, check_deck, kind_factor,
};
use crate::live::score::get_frame;
use crate::{
    Error,
    live::{
        certified::F64Interval,
        full::{LiveModel, Performer},
    },
    master::{GekisouSkillEffectRow, Master, SkillRow},
};

const CONDITION_MEMBER_TARGET: i64 = 5000;
const HOST_SKILL_BASE: i64 = -1000;
const APT_CHECK_SALT: u64 = 0x6170_745f_6368_6563;
const APT_RANK_SALT: u64 = 0x6170_745f_7261_6e6b;
pub const HOST: &str = "each shape alone on one performer, other positions empty; support skills paired with an effect-free member Gekisou skill of the same mission; member-target conditions measured both ways";
pub const MODEL: &str = "every Gekisou skill shape of the chart's missions measured alone under independent nominal lottery and skill probabilities; expected increments with minus without as [center, outward interval half-width] on the best and Perfect plays; tail = score - sum(rangeScore + rankBonus); plain-kind weights and rangeWeights are expected cross terms; each variant checked against a full expectation of a random plain deck at fixed ranks and checkPower within the flooring bound; indicators use deterministic judgement counters and expected lottery points; partial Just rates interpolate the no-live-skill increments, while plain-skill cross weights use the best play";

/// A skill condition of a shape's effect.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    #[serde(rename = "type")]
    pub condition_type: i64,
    pub values: Vec<i64>,
    pub positive: bool,
    /// `None` for a member target condition (5000): the band condition, measured both ways.
    pub target_ids: Option<Vec<i64>>,
}

/// The cumulative condition of a shape's effect.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cumulative {
    #[serde(rename = "type")]
    pub cumulative_type: i64,
    pub values: Vec<i64>,
    pub target_ids: Vec<i64>,
    pub max_cumulative_count: i64,
}

/// An effect row of a shape; condition groups as their condition sets, each a list of conditions.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub effect_type: i64,
    pub trigger_type: i64,
    pub activation_time_second: f32,
    pub effect_value: i64,
    pub max_effect_value: i64,
    pub effect_limit_count: i64,
    pub effect_execute_limit_count: i64,
    pub skill_target_ids: Vec<i64>,
    pub trigger: Vec<Vec<Condition>>,
    pub condition: Vec<Vec<Condition>>,
    pub release: Vec<Vec<Condition>>,
    pub reset: Vec<Vec<Condition>>,
    pub cumulative: Option<Cumulative>,
}

/// A skill and level of a shape.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeSkill {
    pub id: i64,
    pub level: i64,
    /// The targets of its member target conditions (5000), ascending; `None` without one.
    pub member_target_ids: Option<Vec<i64>>,
    /// The bands of those targets, ascending; `None` without a member target condition.
    pub band_ids: Option<Vec<i64>>,
}

/// A Gekisou skill shape.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    pub id: usize,
    /// `"member"` (a member card's Gekisou skill) or `"support"` (a snap's Gekisou support skill).
    pub source: &'static str,
    pub mission: i64,
    /// Whether an effect has a member target condition (5000).
    pub band_condition: bool,
    pub effects: Vec<Effect>,
    pub skills: Vec<ShapeSkill>,
}

/// The aptitude measurement contract and shape catalog.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AptitudeHeader {
    pub plain_kind: Option<usize>,
    pub host: &'static str,
    pub law: &'static str,
    pub shapes: Vec<Shape>,
}

pub fn aptitude_header(master: &Master, kinds: &[Kind]) -> AptitudeHeader {
    AptitudeHeader {
        plain_kind: plain_kind(kinds),
        host: HOST,
        law: "independent nominal lottery and skill probabilities",
        shapes: shapes(master),
    }
}

/// The factors of a range that shape the increments, from the chart's no-skill play.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeFactors {
    /// Judged notes in the range's score frames (after the Start frame to the End frame).
    pub judged_notes: i32,
    /// Of them, judged Just.
    pub just_notes: i32,
    /// Of them, judged Perfect in a Just-count range (a judgement type without a Just row); 0 elsewhere.
    pub perfect_notes: i32,
    /// Notes judged after the End frame to the Complete frame (the tail).
    pub tail_notes: i32,
    /// Notes judged before the Start frame (the combo entering the range).
    pub combo_at_start: i32,
    /// Expected number of lotteries drawn without skills.
    #[serde(serialize_with = "super::expectation::serialize_counts")]
    pub lotteries: [f64; 2],
}

/// Range increments, each [center, outward interval half-width].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeDelta {
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub range_score: [f64; 2],
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub rank_bonus: [f64; 2],
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub rank_bonus_perfect: [f64; 2],
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub range_score_perfect: [f64; 2],
    pub max_combo: [f64; 2],
    pub just_count: [f64; 2],
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub luck_points: [f64; 2],
}

/// A shape's expected increments, each as [center, outward interval half-width].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    pub shape: usize,
    pub band_match: Option<bool>,
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub score: Estimate,
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub score_perfect: Estimate,
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub tail: Estimate,
    #[serde(serialize_with = "super::expectation::serialize_points")]
    pub tail_perfect: Estimate,
    pub converted: Estimate,
    pub ranges: Vec<RangeDelta>,
    #[serde(serialize_with = "super::expectation::serialize_optional_weights")]
    pub weights: Option<Vec<Estimate>>,
    #[serde(serialize_with = "super::expectation::serialize_optional_matrix")]
    pub range_weights: Option<Vec<Vec<Estimate>>>,
    pub check: ExpectationCheck,
}

/// A chart's aptitude for isolated Gekisou skill shapes.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartAptitude {
    pub factors: Vec<RangeFactors>,
    pub variants: Vec<Variant>,
}

/// The plain kind: effect type 2000 on the whole deck for 5 s without targets, conditions or limits (the page's
/// `plainKind`).
pub fn plain_kind(kinds: &[Kind]) -> Option<usize> {
    kinds
        .iter()
        .find(|k| {
            k.effect_type == 2000
                && k.skill_target_ids.is_empty()
                && k.skill_condition_group == 0
                && k.skill_release_condition_group == 0
                && k.effect_limit_count == 0
                && k.effect_execute_limit_count == 0
                && k.duration_ms == 5000
        })
        .map(|k| k.id)
}

/// A condition group as its condition sets (by id), each a list of conditions.
fn group(master: &Master, g: i64) -> Vec<Vec<Condition>> {
    if g == 0 {
        return Vec::new();
    }
    let mut sets: Vec<_> = master.skill_condition_sets.iter().filter(|s| s.group == g).collect();
    sets.sort_by_key(|s| s.id);
    sets.iter()
        .map(|s| {
            s.condition_ids
                .iter()
                .filter_map(|&c| master.skill_condition(c))
                .map(|c| Condition {
                    condition_type: c.condition_type,
                    values: c.condition_values.clone(),
                    positive: c.is_positive,
                    target_ids: (c.condition_type != CONDITION_MEMBER_TARGET).then(|| c.condition_target_ids.clone()),
                })
                .collect()
        })
        .collect()
}

fn effect(master: &Master, r: &GekisouSkillEffectRow) -> Effect {
    let cumulative = if r.skill_cumulative_condition_id != 0 {
        master.cumulative_condition(r.skill_cumulative_condition_id).map(|c| Cumulative {
            cumulative_type: c.condition_type,
            values: c.condition_values.clone(),
            target_ids: c.condition_target_ids.clone(),
            max_cumulative_count: c.max_cumulative_count,
        })
    } else {
        None
    };
    Effect {
        effect_type: r.skill_effect_type,
        trigger_type: r.skill_trigger_type,
        activation_time_second: r.activation_time_second,
        effect_value: r.effect_value,
        max_effect_value: r.max_effect_value,
        effect_limit_count: r.effect_limit_count,
        effect_execute_limit_count: r.effect_execute_limit_count,
        skill_target_ids: r.skill_target_ids.clone(),
        trigger: group(master, r.skill_trigger_condition_group),
        condition: group(master, r.skill_condition_group),
        release: group(master, r.skill_release_condition_group),
        reset: group(master, r.effect_execute_limit_reset_condition_group),
        cumulative,
    }
}

/// The groups of an effect row.
fn groups(r: &GekisouSkillEffectRow) -> [i64; 4] {
    [
        r.skill_trigger_condition_group,
        r.skill_condition_group,
        r.skill_release_condition_group,
        r.effect_execute_limit_reset_condition_group,
    ]
}

/// The member target conditions (5000) of effect rows: `(positive, target ids)`.
fn member_conditions(master: &Master, rows: &[&GekisouSkillEffectRow]) -> Vec<(bool, Vec<i64>)> {
    let mut out = Vec::new();
    for r in rows {
        for g in groups(r) {
            if g == 0 {
                continue;
            }
            for s in master.skill_condition_sets.iter().filter(|s| s.group == g) {
                for c in s.condition_ids.iter().filter_map(|&c| master.skill_condition(c)) {
                    if c.condition_type == CONDITION_MEMBER_TARGET {
                        out.push((c.is_positive, c.condition_target_ids.clone()));
                    }
                }
            }
        }
    }
    out
}

/// The effect rows of a skill at a level, by row id.
fn skill_rows<'m>(master: &'m Master, source: &str, id: i64, level: i64) -> Vec<&'m GekisouSkillEffectRow> {
    let table = if source == "member" { &master.gekisou_skill_effects } else { &master.gekisou_support_skill_effects };
    let mut rows: Vec<&GekisouSkillEffectRow> = table.iter().filter(|r| r.skill_id == id && r.level == level).collect();
    rows.sort_by_key(|r| r.id);
    rows
}

/// The Gekisou skill shapes of a master: member cards' Gekisou skills at their highest level and snaps' first Gekisou
/// support skills at the level of their highest rank, by skill id, grouped by source, mission and effects.
pub fn shapes(master: &Master) -> Vec<Shape> {
    let mut pairs: Vec<(&'static str, i64, i64)> = Vec::new();
    for c in &master.member_cards {
        let id = c.gekisou_skill_id;
        let level = master.gekisou_skill_effects.iter().filter(|r| id != 0 && r.skill_id == id).map(|r| r.level).max();
        if let Some(level) = level
            && !pairs.contains(&("member", id, level))
        {
            pairs.push(("member", id, level));
        }
    }
    for s in &master.support_cards {
        let id = s.gekisou_support_skill_id_01;
        let level = master
            .support_card_ranks
            .iter()
            .filter(|r| r.group == s.rank_group)
            .max_by_key(|r| r.rank)
            .map_or(0, |r| r.gekisou_support_skill_01_level);
        if id != 0 && level > 0 && !pairs.contains(&("support", id, level)) {
            pairs.push(("support", id, level));
        }
    }
    pairs.sort_by_key(|&(source, id, level)| (source != "member", id, level));
    let mut out: Vec<Shape> = Vec::new();
    for (source, id, level) in pairs {
        let row: Option<&SkillRow> =
            if source == "member" { master.gekisou_skill(id) } else { master.gekisou_support_skill(id) };
        let Some(row) = row else { continue };
        let rows = skill_rows(master, source, id, level);
        if rows.is_empty() {
            continue;
        }
        let effects: Vec<Effect> = rows.iter().map(|r| effect(master, r)).collect();
        let conditions = member_conditions(master, &rows);
        let band_condition = !conditions.is_empty();
        let mut targets: Vec<i64> = conditions.iter().flat_map(|c| c.1.iter().copied()).collect();
        targets.sort_unstable();
        targets.dedup();
        let mut bands: Vec<i64> =
            targets.iter().filter_map(|&t| master.skill_target(t)).map(|t| t.band_id).filter(|&b| b > 0).collect();
        bands.sort_unstable();
        bands.dedup();
        let skill = ShapeSkill {
            id,
            level,
            member_target_ids: band_condition.then(|| targets.clone()),
            band_ids: band_condition.then_some(bands),
        };
        let mission = row.gekisou_mission_type;
        match out.iter_mut().find(|s| s.source == source && s.mission == mission && s.effects == effects) {
            Some(s) => s.skills.push(skill),
            None => {
                out.push(Shape { id: out.len(), source, mission, band_condition, effects, skills: vec![skill] });
            }
        }
    }
    out
}

/// A master with the synthetic host Gekisou skills (missions 1 to 4, no effects).
fn with_hosts(master: &Master) -> Result<Master, Error> {
    let mut m = master.clone();
    for mission in 1..=4 {
        m.gekisou_skills.push(SkillRow {
            id: HOST_SKILL_BASE - mission,
            skill_categories: Vec::new(),
            gekisou_mission_type: mission,
        });
    }
    m.reindex()?;
    Ok(m)
}

/// Whether a performer is a target of member target conditions.
fn is_target(master: &Master, conditions: &[(bool, Vec<i64>)], p: &Performer) -> bool {
    conditions
        .iter()
        .any(|(_, targets)| targets.iter().filter_map(|&t| master.skill_target(t)).any(|t| p.matches_skill_target(t)))
}

/// The performer of a variant.
fn performer(master: &Master, shape: &Shape, band: Option<bool>) -> Result<Performer, Error> {
    let s = &shape.skills[0];
    let mut p = Performer { gekisou_mission_type: shape.mission, ..Default::default() };
    if shape.source == "member" {
        p.gekisou_skill = Some((s.id, s.level));
    } else {
        p.gekisou_skill = Some((HOST_SKILL_BASE - shape.mission, 1));
        p.gekisou_support_skills = vec![(s.id, s.level)];
    }
    if let Some(want) = band {
        let rows = skill_rows(master, shape.source, s.id, s.level);
        let conditions = member_conditions(master, &rows);
        if want {
            let t =
                conditions.iter().flat_map(|c| c.1.iter()).find_map(|&t| master.skill_target(t)).ok_or_else(|| {
                    Error::Master(format!("shape {}: a member target condition without targets", shape.id))
                })?;
            p.band_id = t.band_id;
            p.character_id = t.character_id;
            p.card_type = t.card_type;
            if t.tag_id > 0 {
                p.tag_ids = vec![t.tag_id];
            }
            p.live_skill_categories = t.live_skill_categories.clone();
            p.gekisou_skill_categories = t.gekisou_skill_categories.clone();
        }
        if is_target(master, &conditions, &p) != want {
            return Err(Error::Game(format!("shape {}: no performer with band condition {want}", shape.id)));
        }
    }
    Ok(p)
}

/// The range factors of a chart's play.
fn factors(live: &Live<'_>, infos: &[RangeInfo], base: &ExpectedStats) -> Result<Vec<RangeFactors>, Error> {
    let g = live.gekisou.as_ref().ok_or_else(|| Error::Input("aptitude without Gekisou".into()))?;
    let mut lm = LiveModel::new_gekisou(live.master, &[], live.notes, &[], live.params, &g.setup)?;
    let frames = lm.record_range_frames(&live.play, &g.dt)?;
    if frames.len() != infos.len() {
        return Err(Error::Game("the play has other ranges".into()));
    }
    let time: std::collections::HashMap<i32, i32> = live.notes.iter().map(|n| (n.note_id, n.time_ms)).collect();
    let mut out = Vec::with_capacity(infos.len());
    for (j, (info, rf)) in infos.iter().zip(&frames).enumerate() {
        let (fs, fe) = (get_frame(info.start_ms), get_frame(info.end_ms));
        let (mut judged, mut just, mut perfect, mut tail, mut before) = (0, 0, 0, 0, 0);
        for (i, f) in live.play.frames.iter().enumerate() {
            for n in &f.judged {
                let t = *time.get(&n.note_id).ok_or_else(|| Error::Input(format!("unknown note {}", n.note_id)))?;
                let tf = get_frame(t);
                if fs < tf && tf <= fe {
                    judged += 1;
                    if n.judgement == 6 {
                        just += 1;
                    } else if n.judgement == 5 && info.mission == 3 {
                        perfect += 1;
                    }
                }
                if rf.end < i && i <= rf.complete {
                    tail += 1;
                }
                if i < rf.start {
                    before += 1;
                }
            }
        }
        let lotteries =
            estimate(base.ranges[j].lot_results.iter().fold(F64Interval::ZERO, |sum, &x| sum.add(interval(x))));
        out.push(RangeFactors {
            judged_notes: judged,
            just_notes: just,
            perfect_notes: perfect,
            tail_notes: tail,
            combo_at_start: before,
            lotteries,
        });
    }
    Ok(out)
}

pub(super) struct Inputs<'a> {
    pub base: &'a ExpectedStats,
    pub linear: bool,
    pub score_id: i64,
    pub judged: i32,
}

pub(super) fn chart_aptitude(
    live: &Live<'_>,
    kinds: &[Kind],
    infos: &[RangeInfo],
    shapes: &[Shape],
    inp: &Inputs<'_>,
) -> Result<ChartAptitude, Error> {
    let master = with_hosts(live.master)?;
    let measure = with_hosts(live.measure)?;
    let mut eval = Evaluator::new(live, &master)?;
    let plain = plain_kind(kinds);
    let divisor = f64::from(POWER) * plain.map_or(1.0, |p| kind_factor(kinds[p].effect_type, UNIT_VALUE));
    let none = vec![None; live.positions];
    let mut variants = Vec::new();
    for shape in shapes.iter().filter(|s| s.mission == 4 || infos.iter().any(|r| r.mission == s.mission)) {
        let bands = if shape.band_condition { vec![Some(true), Some(false)] } else { vec![None] };
        for band in bands {
            let p = performer(&master, shape, band)?;
            let mut formation = vec![Performer::default(); live.positions.max(1)];
            formation[0] = p;
            let best = eval.run(&master, &formation, &none, POWER, false, None)?;
            let perfect = eval.run(&master, &formation, &none, POWER, true, None)?;
            let score = real(best.final_mean).subtract(interval(inp.base.score));
            let score_perfect = real(perfect.final_mean).subtract(interval(inp.base.score_perfect));
            let (_, counters, converted) = live.run_counted(&live.play, &master, &formation, &none, POWER, 0, None)?;
            let mut ranges = Vec::with_capacity(infos.len());
            let mut tail = score;
            let mut tail_perfect = score_perfect;
            for (((r, rp), base), counter) in
                best.ranges.iter().zip(&perfect.ranges).zip(&inp.base.ranges).zip(counters)
            {
                let range_score = real(r.mean).subtract(interval(base.range_score));
                let rank_bonus = real(r.bonus_mean).subtract(interval(base.rank_bonus));
                let range_score_perfect = real(rp.mean).subtract(interval(base.range_score_perfect));
                let rank_bonus_perfect = real(rp.bonus_mean).subtract(interval(base.rank_bonus_perfect));
                tail = tail.subtract(range_score).subtract(rank_bonus);
                tail_perfect = tail_perfect.subtract(range_score_perfect).subtract(rank_bonus_perfect);
                ranges.push(RangeDelta {
                    range_score: estimate(range_score),
                    rank_bonus: estimate(rank_bonus),
                    rank_bonus_perfect: estimate(rank_bonus_perfect),
                    range_score_perfect: estimate(range_score_perfect),
                    max_combo: [f64::from(counter.max_combo - base.max_combo), 0.0],
                    just_count: [f64::from(counter.just_count - base.just_count), 0.0],
                    luck_points: estimate(
                        real(r.luck_points_mean.expect("range indicators requested"))
                            .subtract(interval(base.luck_points)),
                    ),
                });
            }
            let mut weights = plain.map(|_| vec![[0.0; 2]; live.positions]);
            let mut range_weights =
                (plain.is_some() && inp.linear).then(|| vec![vec![[0.0; 2]; infos.len()]; live.positions]);
            if let Some(plain) = plain {
                for k in 0..live.positions {
                    let mut live_skills = none.clone();
                    live_skills[k] = Some(KIND_SKILL_BASE - plain as i64);
                    let cross = eval.run(&measure, &formation, &live_skills, POWER, false, None)?;
                    weights.as_mut().unwrap()[k] = estimate(
                        scaled(delta(cross.final_mean, best.final_mean), divisor)?
                            .subtract(interval(inp.base.weights[plain][k])),
                    );
                    if let Some(rw) = &mut range_weights {
                        let base_rw = inp
                            .base
                            .range_weights
                            .as_ref()
                            .and_then(|r| r[plain].as_ref())
                            .expect("plain range weights");
                        for (i, (r, b)) in cross.ranges.iter().zip(&best.ranges).enumerate() {
                            rw[k][i] =
                                estimate(scaled(delta(r.mean, b.mean), divisor)?.subtract(interval(base_rw[k][i])));
                        }
                    }
                }
            }
            let salt = inp.score_id as u64 ^ ((shape.id as u64) << 32) ^ (band.map_or(0, |b| 1 + b as u64) << 48);
            let mut rng = Rng(APT_CHECK_SALT ^ salt);
            let mut rank_rng = Rng(APT_RANK_SALT ^ salt);
            let ranks: Vec<_> =
                infos.iter().map(|_| if inp.linear { 1 + rank_rng.below(super::RANKS) as i32 } else { 1 }).collect();
            let usable: Vec<_> = plain.into_iter().collect();
            let (deck, rows) = check_deck(kinds, &usable, live.positions, &mut rng);
            let check_master = Live::master_with(&master, &rows);
            let mut baseline = interval(inp.base.score);
            let mut gain = score;
            let mut shifts = vec![0.0; infos.len()];
            if inp.linear {
                gain = tail;
                for (i, ((base, info), &rank)) in inp.base.ranges.iter().zip(infos).zip(&ranks).enumerate() {
                    let pr = info.percent(rank)? as f64 / 100.0;
                    shifts[i] = (info.percent(rank)? - info.percent(1)?) as f64 / 100.0;
                    baseline = baseline
                        .subtract(interval(base.rank_bonus))
                        .add(interval(base.range_score).multiply(F64Interval::point(pr)?));
                    gain = gain.add(interval(ranges[i].range_score).multiply(F64Interval::point(1.0 + pr)?));
                }
            }
            let check = eval.check(
                kinds,
                &check_master,
                &formation,
                &deck,
                Some(&ranks),
                infos,
                scaled(baseline.add(gain), f64::from(POWER))?,
                |_, k| {
                    let plain = plain.expect("the check deck only uses the plain kind");
                    let mut w = interval(inp.base.weights[plain][k]).add(interval(weights.as_ref().unwrap()[k]));
                    if let Some(rw) = &range_weights {
                        let base_rw = inp.base.range_weights.as_ref().unwrap()[plain].as_ref().unwrap();
                        for (i, &d) in shifts.iter().enumerate() {
                            w = w.add(
                                interval(base_rw[k][i])
                                    .add(interval(rw[k][i]))
                                    .multiply(F64Interval::point(d).unwrap()),
                            );
                        }
                    }
                    w
                },
                f64::from(inp.judged) + MAX_GEKISOU_FEVERS as f64 + 2.0 * infos.len() as f64,
                2.0 * infos.len() as f64,
            )?;
            variants.push(Variant {
                shape: shape.id,
                band_match: band,
                score: estimate(score),
                score_perfect: estimate(score_perfect),
                tail: estimate(tail),
                tail_perfect: estimate(tail_perfect),
                converted: [converted as f64, 0.0],
                ranges,
                weights,
                range_weights,
                check,
            });
        }
    }
    Ok(ChartAptitude { factors: factors(live, infos, inp.base)?, variants })
}
