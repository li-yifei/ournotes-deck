//! A deck-local lottery curve under independent nominal lottery/skill probabilities.
//!
//! The fast entry approximates probabilities numerically; the certified entry encloses them. Neither
//! computes whole-score paths. Binary32 gauge factors and native range/
//! effect timing are recorded by the reduced native interpreter; only the small lottery state is propagated.
//! The seeded System.Random streams are not independent draws over the finite seed space. No claim of exact
//! averaging over all seeds is made. The diagnostic entry uses binary64 accumulation and binary32 output;
//! the certified entry propagates outward binary64 bounds from original integer weights and binary32 chances.
//!
//! The supported mechanics are range-start/playing gauge speed, probabilistic start gauge additions, one-draw
//! start guarantees, a once-per-range Miss gauge addition, and Critical bonus points (which do not feed back).
//! Supported score probes are untimed sustained direct 7021 score-ups. Other shapes fail explicitly.

use super::gekisou::{LotteryMachine, LuckScore, M_LUCK, S_COMPLETE, S_END, S_FINISH, S_PLAYING, S_START};
use super::*;
use crate::live::certified::{F64Interval, ProbabilityMass};
use crate::num::{FxHashMap, floor_to_i32};

/// A complete nominal lottery curve and the size of its sparse computation.
#[derive(Clone, Debug)]
pub struct LuckDpResult {
    pub steps: Vec<(i32, Vec<f32>)>,
    pub peak_states: usize,
    pub transitions: u64,
}

/// Outward probability bounds under independent nominal lotteries. This certifies lottery-state
/// probabilities only; a whole-score certificate also needs the game's binary32/floor score pipeline.
#[derive(Clone, Debug)]
pub struct LuckDpCertifiedResult {
    /// Joint probabilities in the order [neither, score only, Rush only, Rush and score]. Every bucket is
    /// accumulated directly from mutually exclusive DP states, never by subtracting rounded marginals.
    pub steps: Vec<(i32, [ProbabilityMass; 4])>,
    /// All supported score probes use the same 7021 predicate; this identifies the shapes with a holder.
    pub probes: Vec<bool>,
    /// Optional additive indicators, one per range when requested; empty for score-only curves.
    pub range_moments: Vec<LuckRangeMoments>,
    pub peak_states: usize,
    pub transitions: u64,
}

/// Expectations of additive lottery indicators under the independent nominal law.
#[derive(Clone, Copy, Debug)]
pub struct LuckRangeMoments {
    pub luck_points: F64Interval,
    /// Expected result counts in Miss, Hit, Super Hit, Critical order.
    pub lot_results: [F64Interval; 4],
}

impl Default for LuckRangeMoments {
    fn default() -> Self {
        Self { luck_points: F64Interval::ZERO, lot_results: [F64Interval::ZERO; 4] }
    }
}

struct DpResult<W> {
    steps: Vec<(i32, W)>,
    probes: Vec<bool>,
    range_moments: Vec<LuckRangeMoments>,
    peak_states: usize,
    transitions: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct Chain {
    gauge: i32,
    lots: i32,
    next: i8,
    rush: u8,
    maximum: i64,
}

impl Chain {
    fn of(score: &LuckScore) -> Self {
        Self {
            gauge: score.gauge,
            lots: score.lot_count,
            next: score.next as i8,
            // From the fourth Critical onwards the table is always CHANCE_LOW. No retained mechanism reads
            // the exact count; saturation avoids retaining an unbounded, irrelevant history.
            rush: score.rush_combo.min(4) as u8,
            maximum: score.gauge_max,
        }
    }

    fn score(self, template: &LuckScore) -> LuckScore {
        let mut score = template.clone();
        score.gauge = self.gauge;
        score.lot_count = self.lots;
        score.next = i64::from(self.next);
        score.rush_combo = i32::from(self.rush);
        score.gauge_max = self.maximum;
        score
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct State {
    /// At most one Luck range can be active in the supported scheduling domain. Finished ranges are
    /// reset before the next range starts, and future ranges still have their deterministic templates.
    /// Keeping only the active chain therefore forgets no information used by a future transition.
    chain: Chain,
    /// All supported guarantees last one draw and share their release. A draw consumes every enabled one,
    /// so their maximum is a sufficient state (not an independent guarantee for each holder).
    minimum: i8,
    miss_used: bool,
    previous_miss: bool,
    frame_lot: bool,
    frame_miss: bool,
    previous_critical: bool,
    frame_critical: bool,
    rush: bool,
    /// Chart-time rush while scanning this frame's note times. A FINISH disable at frame time must not
    /// remove the rush from earlier chart times in that same frame.
    query_rush: bool,
    score: bool,
    score_before: bool,
}

impl std::hash::Hash for State {
    fn hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        // Preserve every Eq field but feed the hasher three words instead of separately hashing each
        // byte-sized flag. The signed fields keep all their original bits; this imposes no gauge or lot cap.
        hasher.write_i64(self.chain.maximum);
        hasher.write_u64(u64::from(self.chain.gauge as u32) | (u64::from(self.chain.lots as u32) << 32));
        hasher.write_u32(
            u32::from(self.chain.next as u8)
                | (u32::from(self.chain.rush) << 8)
                | (u32::from(self.minimum as u8) << 16)
                | ((self.miss_used as u32) << 24)
                | ((self.previous_miss as u32) << 25)
                | ((self.frame_lot as u32) << 26)
                | ((self.frame_miss as u32) << 27)
                | ((self.rush as u32) << 28)
                | ((self.query_rush as u32) << 29)
                | ((self.score as u32) << 30)
                | ((self.score_before as u32) << 31),
        );
        if self.previous_critical || self.frame_critical {
            hasher.write_u8(u8::from(self.previous_critical) | (u8::from(self.frame_critical) << 1));
        }
    }
}

type Distribution<M = f64> = FxHashMap<State, M>;

/// One transition graph, with either fast nominal masses or outward-certified masses. In particular,
/// certified arithmetic never turns a small positive upper bound into a pruned branch.
trait Mass: Copy {
    type Weights: PartialEq;
    const ZERO: Self;
    const ONE: Self;

    fn from_f32(value: f32) -> Option<Self>;
    fn multiply(self, other: Self) -> Self;
    fn merge(self, other: Self) -> Self;
    fn complement(self) -> Self;
    fn possible(self) -> bool;
    /// The value's bit pattern; equal patterns mean equal values.
    fn bits(self) -> [u64; 2];
    fn interval(self) -> F64Interval;
    fn bonus(machine: &LotteryMachine, kind: usize, buff: i32, minimum: i8) -> Result<Vec<(Self, i64)>, Error>;
    fn base(machine: &LotteryMachine, note_type: i32, judgement: i32) -> Result<Vec<(Self, i64)>, Error>;
    fn weights(dist: &Distribution<Self>, probes: &[bool], at_frame: bool) -> Self::Weights;
}

impl Mass for f64 {
    type Weights = Vec<f32>;
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;

    fn from_f32(value: f32) -> Option<Self> {
        Some(f64::from(value))
    }

    fn multiply(self, other: Self) -> Self {
        self * other
    }

    fn merge(self, other: Self) -> Self {
        self + other
    }

    fn complement(self) -> Self {
        1.0 - self
    }

    fn possible(self) -> bool {
        self > 0.0
    }

    fn bits(self) -> [u64; 2] {
        [self.to_bits(), 0]
    }

    fn interval(self) -> F64Interval {
        F64Interval::point(self).expect("finite nominal mass")
    }

    fn bonus(machine: &LotteryMachine, kind: usize, buff: i32, minimum: i8) -> Result<Vec<(Self, i64)>, Error> {
        machine.bonus_probabilities(kind, buff, i64::from(minimum))
    }

    fn base(machine: &LotteryMachine, note_type: i32, judgement: i32) -> Result<Vec<(Self, i64)>, Error> {
        machine.base_point_probabilities(note_type, judgement)
    }

    fn weights(dist: &Distribution<Self>, probes: &[bool], at_frame: bool) -> Self::Weights {
        let mut values = [0f64; 3];
        for (state, &probability) in dist {
            let rush = if at_frame { state.rush } else { state.query_rush };
            let score = if at_frame { state.score } else { state.score_before };
            if rush {
                values[0] += probability;
            }
            if score {
                values[1] += probability;
                if rush {
                    values[2] += probability;
                }
            }
        }
        let values = values.map(|value| value.clamp(0.0, 1.0) as f32);
        let mut weights = Vec::with_capacity(1 + 2 * probes.len());
        weights.push(values[0]);
        for &enabled in probes {
            weights.extend_from_slice(if enabled { &values[1..] } else { &[0.0, 0.0] });
        }
        weights
    }
}

fn certified_weights(weights: Vec<(u64, u64, i64)>) -> Result<Vec<(ProbabilityMass, i64)>, Error> {
    let mut out: Vec<(ProbabilityMass, i64)> = Vec::new();
    for (weight, total, result) in weights {
        let probability = ProbabilityMass::from_ratio(weight, total)?;
        if let Some(existing) = out.iter_mut().find(|item| item.1 == result) {
            existing.0 = existing.0.merge_disjoint(probability);
        } else {
            out.push((probability, result));
        }
    }
    Ok(out)
}

impl Mass for ProbabilityMass {
    type Weights = [ProbabilityMass; 4];
    const ZERO: Self = ProbabilityMass::ZERO;
    const ONE: Self = ProbabilityMass::ONE;

    fn from_f32(value: f32) -> Option<Self> {
        ProbabilityMass::from_f32(value).ok()
    }

    fn multiply(self, other: Self) -> Self {
        ProbabilityMass::multiply(self, other)
    }

    fn merge(self, other: Self) -> Self {
        self.merge_disjoint(other)
    }

    fn complement(self) -> Self {
        ProbabilityMass::complement(self)
    }

    fn possible(self) -> bool {
        self.interval().upper() > 0.0
    }

    fn bits(self) -> [u64; 2] {
        [self.interval().lower().to_bits(), self.interval().upper().to_bits()]
    }

    fn interval(self) -> F64Interval {
        ProbabilityMass::interval(self)
    }

    fn bonus(machine: &LotteryMachine, kind: usize, buff: i32, minimum: i8) -> Result<Vec<(Self, i64)>, Error> {
        certified_weights(machine.bonus_probability_weights(kind, buff, i64::from(minimum))?)
    }

    fn base(machine: &LotteryMachine, note_type: i32, judgement: i32) -> Result<Vec<(Self, i64)>, Error> {
        certified_weights(machine.base_point_probability_weights(note_type, judgement)?)
    }

    fn weights(dist: &Distribution<Self>, _probes: &[bool], at_frame: bool) -> Self::Weights {
        let mut joint = [Self::ZERO; 4];
        for (state, &probability) in dist {
            let rush = if at_frame { state.rush } else { state.query_rush };
            let score = if at_frame { state.score } else { state.score_before };
            let index = 2 * usize::from(rush) + usize::from(score);
            joint[index] = joint[index].merge_disjoint(probability);
        }
        joint
    }
}

/// The native nominal table coalesces identical results and rejects values outside 0..=3. Store its
/// existing order, including the original probability values, without allocating on each state draw.
#[derive(Clone, Copy)]
struct Draws<M: Mass> {
    outcomes: [(M, i64); 4],
    len: usize,
}

impl<M: Mass> Draws<M> {
    fn one(result: i64) -> Self {
        Self { outcomes: [(M::ONE, result), (M::ZERO, 0), (M::ZERO, 0), (M::ZERO, 0)], len: 1 }
    }

    fn of(outcomes: &[(M, i64)]) -> Result<Self, Error> {
        if outcomes.len() > 4 {
            return Err(Error::Capacity("LUCK DP exceeds four distinct lottery results".into()));
        }
        let mut draws = Self { outcomes: [(M::ZERO, 0); 4], len: outcomes.len() };
        draws.outcomes[..draws.len].copy_from_slice(outcomes);
        Ok(draws)
    }

    fn iter(&self) -> impl Iterator<Item = (M, i64)> + '_ {
        self.outcomes[..self.len].iter().copied()
    }
}

#[derive(Clone, Copy, Debug)]
enum Action<M> {
    StartGauge { value: i64, chance: M },
    StartMinimum { result: i8, chance: M },
    MissGauge { value: i64 },
    CriticalPoints { value: i32, chance: M },
    StartPoints { value: i32, chance: M },
}

struct Plan<M> {
    actions: Vec<(i64, Action<M>, Option<Checker>)>,
    probes: Vec<bool>,
}

impl<M> Default for Plan<M> {
    fn default() -> Self {
        Self { actions: Vec::new(), probes: Vec::new() }
    }
}

fn unsupported(row: &EffectRow, why: &str) -> Error {
    Error::Unsupported(format!("LUCK DP effect row {} (type {}): {why}", row.id, row.effect_type))
}

/// These predicates contain no mutable checkers. Their nominal probability is sufficient because every
/// accepted start row is checked only once per range. Formation was already resolved by the native factory.
/// Life comparisons read the complete native recorder at this frame and skill phase, never initial life.
fn chance<M: Mass>(checker: &Checker, life: i32) -> Option<M> {
    let fixed = |value| Some(if value { M::ONE } else { M::ZERO });
    match checker {
        Checker::Fixed(value) => fixed(*value),
        Checker::Probability(value) if value.is_finite() => M::from_f32(value.clamp(0.0, 1.0)),
        Checker::LifeGreater(Some(value)) => fixed(i64::from(life) > *value),
        Checker::LifeAtLeast(Some(value)) => fixed(i64::from(life) >= *value),
        Checker::LifeLess(Some(value)) => fixed(i64::from(life) < *value),
        Checker::LifeAtMost(Some(value)) => fixed(i64::from(life) <= *value),
        Checker::Not(inner) => chance(inner, life).map(M::complement),
        Checker::And { items, .. } => {
            items.iter().try_fold(M::ONE, |p, item| chance(item, life).map(|q| p.multiply(q)))
        }
        Checker::Or(items) => items
            .iter()
            .try_fold(M::ONE, |p, item| chance::<M>(item, life).map(|q| p.multiply(q.complement())))
            .map(M::complement),
        _ => None,
    }
}

fn fixed_condition(checker: &Checker) -> Option<bool> {
    match checker {
        Checker::Fixed(value) => Some(*value),
        Checker::Not(inner) => fixed_condition(inner).map(|value| !value),
        Checker::And { items, .. } => {
            items.iter().try_fold(true, |value, item| fixed_condition(item).map(|next| value && next))
        }
        Checker::Or(items) => {
            items.iter().try_fold(false, |value, item| fixed_condition(item).map(|next| value || next))
        }
        _ => None,
    }
}

fn has_probability(checker: &Checker) -> bool {
    match checker {
        Checker::Probability(_) => true,
        Checker::Not(inner) => has_probability(inner),
        Checker::And { items, .. } | Checker::Or(items) => items.iter().any(has_probability),
        _ => false,
    }
}

fn complete(checker: Option<&Checker>) -> bool {
    matches!(checker, Some(Checker::RangeComplete))
}

fn luck_missions(missions: &[i64]) -> bool {
    // The verified JP factory resolves target 56 (skillTargetType 5, mission 2) to [2]. Empty
    // missions are the native unrestricted variant, equivalent in this non-overlapping Luck domain.
    missions.iter().all(|&mission| mission == M_LUCK)
}

fn reads_life(checker: &Checker) -> bool {
    checker.any(&|c| {
        matches!(c, Checker::LifeGreater(_) | Checker::LifeAtLeast(_) | Checker::LifeLess(_) | Checker::LifeAtMost(_))
    })
}

/// These appliers are the dependency closure of life under a fixed judgement stream: life commands,
/// judgement conversion (which changes note damage), and extensions of live effects. Score/rank/lottery
/// appliers do not mutate any of them. All retained checkers must be independent of lottery outcomes.
fn life_dependency(effect: i64) -> bool {
    // Combo/Just writers also belong to the closure: deterministic life/conversion predicates and cumulative
    // counters can read them. Dropping them would change the life recorder's native trace.
    matches!(effect, 3000..=3004 | 4004 | 12000 | 12002..=12004 | 12006 | 13000 | 13002..=13005 | 15000)
}

fn deterministic_life_checker(checker: &Checker) -> bool {
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
        | Checker::GkReachRank { .. }
        | Checker::RangeStart { .. }
        | Checker::RangePlaying { .. }
        | Checker::RangeComplete
        | Checker::SnapGekisouStart { .. }
        | Checker::GkLiveStart(_) => true,
        Checker::And { items, .. } | Checker::Or(items) => items.iter().all(deterministic_life_checker),
        Checker::Not(inner) => deterministic_life_checker(inner),
        _ => false,
    }
}

/// Whether a held effect can convert the declared judgements before the LUCK controller consumes them.
/// Even ordinary live/Snap conversions make a Rush curve dependent on the deck and skill-event timeline.
#[doc(hidden)]
pub fn luck_has_judgement_conversion(master: &Master, deck: &[Performer]) -> bool {
    let converts = |effect| matches!(effect, 12006 | 13005);
    deck.iter().any(|p| {
        p.live_skill.is_some_and(|(id, lv)| {
            master
                .live_skill_effects
                .iter()
                .any(|r| r.live_skill_id == id && r.level == lv && converts(r.skill_effect_type))
        }) || p.support_skills.iter().any(|&(id, lv)| {
            master
                .support_skill_effects
                .iter()
                .any(|r| r.support_skill_id == id && r.level == lv && converts(r.skill_effect_type))
        }) || p.gekisou_skill.is_some_and(|(id, lv)| {
            master
                .gekisou_skill_effects
                .iter()
                .any(|r| r.skill_id == id && r.level == lv && converts(r.skill_effect_type))
        }) || (p.gekisou_skill.is_some()
            && p.gekisou_support_skills.iter().any(|&(id, lv)| {
                master
                    .gekisou_support_skill_effects
                    .iter()
                    .any(|r| r.skill_id == id && r.level == lv && converts(r.skill_effect_type))
            }))
    })
}

#[allow(clippy::too_many_arguments)]
fn life_recorder(
    master: &Master,
    deck: &[Performer],
    notes: &[LiveNote],
    events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    ranking: Option<&[crate::replay::RankConfirmation]>,
) -> Result<LiveModel, Error> {
    let mut relevant = master.clone();
    // A live skill's parent/pool lifecycle depends on all its effects. Keep complete groups whenever a
    // group can change life or a judgement, even when another effect of the group only changes score.
    let live_groups: FxHashSet<_> = master
        .live_skill_effects
        .iter()
        .filter(|r| life_dependency(r.skill_effect_type))
        .map(|r| (r.live_skill_id, r.level))
        .collect();
    relevant.live_skill_effects.retain(|r| live_groups.contains(&(r.live_skill_id, r.level)));
    relevant.support_skill_effects.retain(|r| life_dependency(r.skill_effect_type));
    relevant.gekisou_skill_effects.retain(|r| life_dependency(r.skill_effect_type));
    relevant.gekisou_support_skill_effects.retain(|r| life_dependency(r.skill_effect_type));
    let mut model =
        LiveModel::build(&relevant, deck, notes, events, params, Some(setup), ranking.is_some(), None, None)?;
    if model.rows.iter().any(|row| row.effect_type == 3000) {
        return Err(Error::Unsupported(
            "LUCK life DP: dynamic life maximum needs the complete native reader/cache history".into(),
        ));
    }
    if let Some(ranking) = ranking {
        model.set_rank_confirmation_timeline(ranking)?;
    }
    let check = |row: &EffectRow,
                 cumulative: Option<&conditions::Cumulative>,
                 checkers: &[&Option<Checker>]|
     -> Result<(), Error> {
        if cumulative.is_some_and(|c| !super::luck_score_bounds::deterministic_cumulative(c))
            || checkers.iter().any(|c| c.as_ref().is_some_and(|c| !deterministic_life_checker(c)))
        {
            return Err(unsupported(row, "life/judgement dependency is not deterministic under the declared play"));
        }
        Ok(())
    };
    for skill in &model.live {
        for effect in &skill.effects {
            check(&model.rows[effect.row], effect.cumulative.as_ref(), &[&effect.condition, &effect.release])?;
        }
    }
    for skill in &model.cond {
        for effect in skill.updater.effects() {
            check(
                &model.rows[effect.row],
                effect.cumulative.as_ref(),
                &[&effect.trigger, &effect.condition, &effect.reset],
            )?;
        }
        for updater in &skill.updater.updaters {
            check(&model.rows[skill.updater.effect(updater.effect).row], None, &[&updater.release])?;
        }
    }
    model.gk.as_mut().expect("Gekisou life recorder").ctrl.luck_weighted = true;
    model.phase_life = Some([0; 2]);
    Ok(model)
}

fn consume_bound(frames: usize, mut judgements: impl Iterator<Item = usize>) -> Result<(), Error> {
    let total = judgements.try_fold(frames, |sum, count| sum.checked_add(count));
    if total.is_none_or(|total| total > i32::MAX as usize) {
        return Err(Error::Capacity("LUCK DP cannot prove native rush_combo avoids signed wrapping".into()));
    }
    Ok(())
}

fn compile<M: Mass>(
    master: &Master,
    model: &mut LiveModel,
    skills: &LuckSkills,
    probes: Option<&[Option<usize>]>,
    collect_moments: bool,
) -> Result<Plan<M>, Error> {
    let mut plan = Plan::default();
    for condition in &model.cond {
        for (effect_index, effect) in condition.updater.effects().iter().enumerate() {
            let row = &model.rows[effect.row];
            let fail = |why| unsupported(row, why);
            if !matches!(effect.phase, 1 | 2) {
                return Err(fail("effect phase is outside the native two phases"));
            }
            let release = condition
                .updater
                .updaters
                .iter()
                .find(|updater| updater.effect == effect_index)
                .and_then(|updater| updater.release.as_ref());
            let source = if condition.skill_type == SKILL_TYPE_GEKISOU {
                LuckSource::Gekisou
            } else {
                LuckSource::GekisouSupport
            };
            let native = source
                .rows(master)
                .iter()
                .find(|native| native.id == row.id)
                .ok_or_else(|| Error::Master(format!("LUCK DP cannot find effect row {}", row.id)))?;
            let mission = match source {
                LuckSource::Gekisou => master.gekisou_skill(native.skill_id).map(|skill| skill.gekisou_mission_type),
                LuckSource::GekisouSupport => {
                    master.gekisou_support_skill(native.skill_id).map(|skill| skill.gekisou_mission_type)
                }
            };
            if mission != Some(M_LUCK) {
                return Err(fail("only Luck mission mechanisms and score probes are supported"));
            }
            if effect.cumulative.is_some() {
                return Err(fail("a cumulative lottery mechanism"));
            }
            let probability = match effect.condition.as_ref() {
                Some(checker) => chance::<f64>(checker, 0)
                    .ok_or_else(|| fail("a condition outside deterministic-life/independent-probability predicates"))?,
                None => 1.0,
            };
            let mass = match effect.condition.as_ref() {
                Some(checker) => chance::<M>(checker, 0)
                    .ok_or_else(|| fail("a condition outside deterministic-life/independent-probability predicates"))?,
                None => M::ONE,
            };
            let start = matches!(effect.trigger.as_ref(), Some(Checker::RangeStart { missions, .. }) if luck_missions(missions));
            let playing = matches!(effect.trigger.as_ref(), Some(Checker::RangePlaying { missions, .. }) if luck_missions(missions));
            let miss = matches!(effect.trigger.as_ref(), Some(Checker::LuckLotResult { target: 0, .. }));
            let critical = matches!(effect.trigger.as_ref(), Some(Checker::LuckLotResult { target: 3, .. }));
            let rush = matches!(effect.trigger.as_ref(), Some(Checker::LuckRushPlaying(_)));
            if row.effect_type == 11002 && !collect_moments {
                continue;
            }
            match row.effect_type {
                11001 => {
                    let timed = start && effect.trigger_type == ONE_SHOT && effect.act > 0.0 && release.is_none();
                    let sustained = playing
                        && effect.trigger_type == SUSTAINED
                        && effect.act == 0.0
                        && (release.is_none() || complete(release));
                    if !(timed || sustained)
                        || !matches!(probability, 0.0 | 1.0)
                        || effect.condition.as_ref().is_some_and(has_probability)
                        || effect.condition.as_ref().is_some_and(|checker| fixed_condition(checker).is_none())
                        || effect.execute_limit != 0
                        || effect.reset.is_some()
                    {
                        return Err(fail("an unknown gauge-speed shape"));
                    }
                    // The native recorder installs and ends these deterministic binary32 commands.
                }
                11003 if start => {
                    if effect.trigger_type != ONE_SHOT
                        || effect.act != 0.0
                        || !complete(release)
                        || effect.execute_limit != 0
                        || effect.reset.is_some()
                    {
                        return Err(fail("an unknown range-start gauge shape"));
                    }
                    plan.actions.push((
                        effect.phase,
                        Action::StartGauge { value: row.effect_value, chance: mass },
                        effect.condition.clone(),
                    ));
                }
                11005 if start => {
                    if effect.trigger_type != ONE_SHOT
                        || effect.act != 0.0
                        || !complete(release)
                        || row.effect_limit_count != 1
                        || effect.execute_limit != 0
                        || effect.reset.is_some()
                        || !(2..=4).contains(&row.effect_value)
                    {
                        return Err(fail("only one-draw range-start guarantees are supported"));
                    }
                    plan.actions.push((
                        effect.phase,
                        Action::StartMinimum { result: (row.effect_value - 1) as i8, chance: mass },
                        effect.condition.clone(),
                    ));
                }
                11003 if miss => {
                    if effect.trigger_type != ONE_SHOT
                        || effect.act != 0.0
                        || !complete(release)
                        || effect.execute_limit != 1
                        || !complete(effect.reset.as_ref())
                        || !matches!(probability, 0.0 | 1.0)
                        || effect.condition.as_ref().is_some_and(|checker| fixed_condition(checker).is_none())
                    {
                        return Err(fail("an unknown once-per-range Miss gauge shape"));
                    }
                    if probability == 1.0 {
                        plan.actions.push((effect.phase, Action::MissGauge { value: row.effect_value }, None));
                    }
                }
                11002 if start => {
                    if effect.trigger_type != ONE_SHOT
                        || effect.act != 0.0
                        || effect.execute_limit != 0
                        || effect.reset.is_some()
                        || !(release.is_none() || complete(release))
                    {
                        return Err(fail("an unknown range-start bonus-point shape"));
                    }
                    plan.actions.push((
                        effect.phase,
                        Action::StartPoints { value: row.effect_value as i32, chance: mass },
                        effect.condition.clone(),
                    ));
                }
                11002 if critical => {
                    if effect.trigger_type != ONE_SHOT
                        || effect.act != 0.0
                        || release.is_some()
                        || effect.execute_limit != 0
                        || effect.reset.is_some()
                        || !matches!(probability, 0.0 | 1.0)
                        || effect.condition.as_ref().is_some_and(|checker| fixed_condition(checker).is_none())
                    {
                        return Err(fail("an unknown Critical bonus-point shape"));
                    }
                    plan.actions.push((
                        effect.phase,
                        Action::CriticalPoints { value: row.effect_value as i32, chance: mass },
                        effect.condition.clone(),
                    ));
                }
                2000 | 2005 if rush => {
                    if effect.trigger_type != SUSTAINED
                        || effect.act != 0.0
                        || release.is_some()
                        || effect.execute_limit != 0
                        || effect.reset.is_some()
                        || !matches!(probability, 0.0 | 1.0)
                        || effect.condition.as_ref().is_some_and(|checker| fixed_condition(checker).is_none())
                    {
                        return Err(fail("only untimed sustained direct Rush score probes are supported"));
                    }
                }
                _ => return Err(fail("an unknown lottery or score-probe shape")),
            }
        }
    }
    // Stable phase ordering matches the interpreter: all updater checks, then appliers in list/row order,
    // separately for phase 1 and phase 2. Rows within one condition skill are already sorted by effect id.
    plan.actions.sort_by_key(|action| action.0);
    let rows = model.luck_score_rows(skills);
    for shape in 0..skills.shapes.len() {
        let member = match probes {
            Some(probes) => probes[shape],
            None => rows.iter().find(|row| row.shape == shape && row.may_hold).map(|row| row.member),
        };
        if let Some(member) = member
            && !rows.iter().any(|row| row.shape == shape && row.member == member && row.may_hold)
        {
            return Err(Error::Input(format!("LUCK DP: position {member} holds no score-up of shape {shape}")));
        }
        plan.probes.push(member.is_some());
    }
    Ok(plan)
}

struct Dp<'a, M: Mass> {
    templates: Vec<LuckScore>,
    active_range: Option<usize>,
    machine: &'a LotteryMachine,
    draws: FxHashMap<(usize, i32, i8), Draws<M>>,
    dist: Distribution<M>,
    spare: Distribution<M>,
    peak: usize,
    transitions: u64,
    range_moments: Vec<LuckRangeMoments>,
    track_critical: bool,
}

impl<'a, M: Mass> Dp<'a, M> {
    fn new(templates: Vec<LuckScore>, machine: &'a LotteryMachine, collect_moments: bool) -> Self {
        let mut dist = Distribution::<M>::default();
        dist.insert(State::default(), M::ONE);
        Self {
            range_moments: if collect_moments {
                vec![LuckRangeMoments::default(); templates.len()]
            } else {
                Vec::new()
            },
            track_critical: false,
            templates,
            active_range: None,
            machine,
            draws: FxHashMap::default(),
            dist,
            spare: Distribution::default(),
            peak: 1,
            transitions: 0,
        }
    }

    fn push(&mut self, out: &mut Distribution<M>, state: State, probability: M) -> Result<(), Error> {
        if probability.possible() {
            let existing = out.entry(state).or_insert(M::ZERO);
            *existing = existing.merge(probability);
            self.transitions = self
                .transitions
                .checked_add(1)
                .ok_or_else(|| Error::Capacity("LUCK DP transitions overflow".into()))?;
        }
        Ok(())
    }

    fn replace(&mut self, out: Distribution<M>) -> Result<(), Error> {
        self.peak = self.peak.max(out.len());
        if out.len() > 2_000_000 {
            return Err(Error::Capacity("LUCK DP exceeds two million states".into()));
        }
        self.dist = out;
        Ok(())
    }

    fn map(&mut self, mut transform: impl FnMut(State) -> Result<State, Error>) -> Result<(), Error> {
        let mut out = std::mem::take(&mut self.spare);
        let mut previous = std::mem::take(&mut self.dist);
        for (state, probability) in previous.drain() {
            self.push(&mut out, transform(state)?, probability)?;
        }
        self.spare = previous;
        self.replace(out)
    }

    fn action(&mut self, action: Action<M>, target: usize) -> Result<(), Error> {
        debug_assert_eq!(self.active_range, Some(target));
        if let Action::CriticalPoints { value, chance } | Action::StartPoints { value, chance } = action {
            let mass = self
                .dist
                .iter()
                .filter(|(state, _)| matches!(action, Action::StartPoints { .. }) || state.previous_critical)
                .fold(M::ZERO, |sum, (_, &probability)| sum.merge(probability));
            let points = mass.multiply(chance).interval().multiply(F64Interval::integer(i128::from(value)));
            self.range_moments[target].luck_points = self.range_moments[target].luck_points.add(points);
            return Ok(());
        }
        let mut out = std::mem::take(&mut self.spare);
        let mut previous = std::mem::take(&mut self.dist);
        for (state, probability) in previous.drain() {
            // All accepted Miss rows share the same once/reset trigger. Leave their used flag untouched
            // until every row has run so that multiple eligible rows each apply once in native order.
            if matches!(action, Action::MissGauge { .. }) && (!state.previous_miss || state.miss_used) {
                let existing = out.entry(state).or_insert(M::ZERO);
                *existing = existing.merge(probability);
                continue;
            }
            let mut next = state;
            let chance = match action {
                Action::StartMinimum { result, chance } => {
                    next.minimum = next.minimum.max(result);
                    chance
                }
                Action::StartGauge { value, .. } | Action::MissGauge { value } => {
                    let mut score = state.chain.score(&self.templates[target]);
                    let gauge = (score.gauge_max as i128 * value as i128) as i32;
                    score.add_gauge(floor_to_i32(gauge as f32 / 10000f32))?;
                    next.chain = Chain::of(&score);
                    if let Action::StartGauge { chance, .. } = action { chance } else { M::ONE }
                }
                Action::CriticalPoints { .. } | Action::StartPoints { .. } => unreachable!(),
            };
            self.push(&mut out, next, probability.multiply(chance))?;
            let complement = chance.complement();
            if complement.possible() {
                self.push(&mut out, state, probability.multiply(complement))?;
            }
        }
        self.spare = previous;
        self.replace(out)
    }

    fn draw(&mut self, kind: usize, buff: i32, minimum: i8) -> Result<Draws<M>, Error> {
        let key = (kind, buff, minimum);
        if let Some(draws) = self.draws.get(&key) {
            return Ok(*draws);
        }
        let draws = Draws::of(&M::bonus(self.machine, kind, buff, minimum)?)?;
        self.draws.insert(key, draws);
        Ok(draws)
    }

    fn consume(
        &mut self,
        mut state: State,
        range: usize,
        buff: i32,
        p: M,
        out: &mut Distribution<M>,
    ) -> Result<(), Error> {
        state.chain.lots -= 1;
        let first = if state.chain.next == -1 {
            let draws = self.draw(0, buff, state.minimum)?;
            state.minimum = 0;
            draws
        } else {
            Draws::one(i64::from(state.chain.next))
        };
        for (pn, result) in first.iter() {
            let mut next = state;
            let mut score = next.chain.score(&self.templates[range]);
            if score.rush_combo == 0 || result != 3 {
                next.rush = result == 3;
                next.query_rush = next.rush;
            }
            score.add_score(result)?;
            if let Some(moments) = self.range_moments.get_mut(range) {
                let mass = p.multiply(pn).interval();

                moments.lot_results[result as usize] = moments.lot_results[result as usize].add(mass);
                let points = if result == 0 { 0 } else { super::gekisou::BONUS_POINT_BY_RESULT[result as usize - 1] };
                moments.luck_points = moments.luck_points.add(mass.multiply(F64Interval::integer(i128::from(points))));
            }
            next.chain = Chain::of(&score);
            next.frame_lot = true;
            next.frame_miss |= result == 0;
            next.frame_critical |= self.track_critical && result == 3;
            let draws = self.draw(score.current_lot_type(), buff, next.minimum)?;
            next.minimum = 0;
            for (pr, result) in draws.iter() {
                let mut after = next;
                after.chain.next = result as i8;
                self.push(out, after, p.multiply(pn).multiply(pr))?;
            }
        }
        Ok(())
    }

    fn note(
        &mut self,
        range: usize,
        note_type: i32,
        judgement: i32,
        buff: i32,
        speed: f32,
        consumes: bool,
    ) -> Result<(), Error> {
        if matches!(judgement, 0 | 7) {
            return Ok(());
        }
        debug_assert_eq!(self.active_range, Some(range));
        let base: Vec<_> = M::base(self.machine, note_type, judgement)?
            .into_iter()
            .map(|(probability, point)| (probability, floor_to_i32((speed + 1f32) * point as f32)))
            .collect();
        let mut out = std::mem::take(&mut self.spare);
        let mut previous = std::mem::take(&mut self.dist);
        for (state, probability) in previous.drain() {
            for &(pb, gauge) in &base {
                let mut next = state;
                let mut score = next.chain.score(&self.templates[range]);
                score.add_gauge(gauge)?;
                next.chain = Chain::of(&score);
                if consumes && score.lot_count > 0 {
                    self.consume(next, range, buff, probability.multiply(pb), &mut out)?;
                } else {
                    self.push(&mut out, next, probability.multiply(pb))?;
                }
            }
        }
        self.spare = previous;
        self.replace(out)
    }

    fn pending(&mut self, range: usize, buff: i32) -> Result<(), Error> {
        debug_assert_eq!(self.active_range, Some(range));
        let mut out = std::mem::take(&mut self.spare);
        let mut previous = std::mem::take(&mut self.dist);
        for (state, probability) in previous.drain() {
            if state.chain.lots > 0 && !state.frame_lot {
                self.consume(state, range, buff, probability, &mut out)?;
            } else {
                self.push(&mut out, state, probability)?;
            }
        }
        self.spare = previous;
        self.replace(out)
    }

    fn weights(&self, probes: &[bool], at_frame: bool) -> M::Weights {
        M::weights(&self.dist, probes, at_frame)
    }
}

/// Run a native weighted frame's BEFORE/skills phase. Weighted mode draws no lottery, but records deterministic
/// gauge-speed commands with the exact updater lifecycle and binary32 filing order.
fn record_before(model: &mut LiveModel, time: i32, delta: f32) -> Result<(), Error> {
    model.frame_time = time;
    let gk = model.gk.as_mut().expect("DP requires Gekisou");
    gk.fever.update(time, &mut gk.fever_updates);
    let mut handle = Handle { sc: &mut model.scorectl, score: &mut model.score };
    let mut env = Env { random: &mut model.random, handle: &mut handle };
    gk.ctrl.before_update(delta, time, &gk.fever_updates, 0, &mut env)?;
    let input = FrameInput { time_ms: time, music_length_ms: model.music_length_ms, is_live_finished: false };
    for condition in &mut model.cond {
        condition.updater.begin_frame();
    }
    // The live's per-frame buffers: no model frame is open while the recorder runs its phases.
    let mut listed = std::mem::take(&mut model.scratch.listed);
    let mut updated = std::mem::take(&mut model.scratch.updated);
    for phase in PHASES {
        let gk =
            model.gk.as_ref().map(|g| GkView { ctrl: &g.ctrl, prev_lots: &g.prev_lots, prev_lot_ms: g.prev_lot_ms });
        let mut context = CheckCtx {
            life: &mut model.life,
            random: &mut model.random,
            frame_time: time,
            current_combo: 0,
            judged: &[],
            events: &[],
            gk,
            prev_confirmed_rank: None,
        };
        listed.clear();
        for (updater, condition) in model.cond.iter_mut().enumerate() {
            condition.updater.update_into(phase, input, &mut context, &mut updated)?;
            for &u in &updated {
                if condition.updater.updaters[u].state.state != STAY {
                    listed.push(Listed::Cond { updater, u });
                }
            }
        }
        for &item in &listed {
            model.apply(item)?;
        }
    }
    (model.scratch.listed, model.scratch.updated) = (listed, updated);
    Ok(())
}

fn record_after(model: &mut LiveModel, time: i32, judged: &[gekisou::GkNote]) -> Result<(), Error> {
    let gk = model.gk.as_mut().expect("Gekisou checked");
    let mut handle = Handle { sc: &mut model.scorectl, score: &mut model.score };
    let mut env = Env { random: &mut model.random, handle: &mut handle };
    gk.ctrl.update(time, judged, &gk.fever_updates, 0, &mut env)
}

/// Compute the same `(note chart time, [rush, score_0, rush_and_score_0, ...])` curve as `luck_rush_samples`.
/// This prototype requires the theoretical-play scheduling contract: each note is judged in the first declared
/// frame at or after its chart time, with ascending chart times inside a frame. Cross-frame early/late judgements
/// and overlapping Luck ranges are explicitly unsupported rather than approximated.
#[allow(clippy::too_many_arguments)]
pub fn luck_rush_dp(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
) -> Result<LuckDpResult, Error> {
    luck_rush_dp_with_events(master, skills, notes, &[], params, setup, play, delta_times, deck, probes)
}

/// Nominal diagnostic curve including the complete deterministic life/converted-judgement schedule.
#[allow(clippy::too_many_arguments)]
pub fn luck_rush_dp_with_events(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
) -> Result<LuckDpResult, Error> {
    luck_rush_dp_with_ranking(master, skills, notes, skill_events, params, setup, play, delta_times, deck, probes, None)
}

/// Nominal diagnostic curve under the caller's explicit external rank-arrival scenario.
#[allow(clippy::too_many_arguments)]
pub fn luck_rush_dp_with_ranking(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
    ranking: Option<&[crate::replay::RankConfirmation]>,
) -> Result<LuckDpResult, Error> {
    let result = run::<f64>(
        master,
        skills,
        notes,
        skill_events,
        params,
        setup,
        play,
        delta_times,
        deck,
        probes,
        ranking,
        false,
    )?;
    Ok(LuckDpResult { steps: result.steps, peak_states: result.peak_states, transitions: result.transitions })
}

/// Run the same supported transition graph with outward probability arithmetic, emitting the four
/// joint Rush/7021 states directly. Probability sources are original integer lottery weights and exact
/// binary32 skill chances. This does not certify the subsequent whole-score calculation.
#[allow(clippy::too_many_arguments)]
pub fn luck_rush_dp_certified(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
) -> Result<LuckDpCertifiedResult, Error> {
    luck_rush_dp_certified_with_events(master, skills, notes, &[], params, setup, play, delta_times, deck, probes)
}

/// Certified curve including native skill events, life commands and the two-phase life checks.
#[allow(clippy::too_many_arguments)]
pub fn luck_rush_dp_certified_with_events(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
) -> Result<LuckDpCertifiedResult, Error> {
    luck_rush_dp_certified_with_ranking(
        master,
        skills,
        notes,
        skill_events,
        params,
        setup,
        play,
        delta_times,
        deck,
        probes,
        None,
    )
}

/// Certified probabilities under an explicit external rank-arrival scenario. Retained lottery effects
/// read no rank/score counters; the deterministic life recorder uses the same native external lifecycle.
#[allow(clippy::too_many_arguments)]
pub fn luck_rush_dp_certified_with_ranking(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
    ranking: Option<&[crate::replay::RankConfirmation]>,
) -> Result<LuckDpCertifiedResult, Error> {
    certified_mode(master, skills, notes, skill_events, params, setup, play, delta_times, deck, probes, ranking, false)
}

/// Certified curve with expected additive range points and consumed result counts.
#[allow(clippy::too_many_arguments)]
pub fn luck_rush_dp_certified_with_moments(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
    ranking: Option<&[crate::replay::RankConfirmation]>,
) -> Result<LuckDpCertifiedResult, Error> {
    certified_mode(master, skills, notes, skill_events, params, setup, play, delta_times, deck, probes, ranking, true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn certified_mode(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
    ranking: Option<&[crate::replay::RankConfirmation]>,
    collect_moments: bool,
) -> Result<LuckDpCertifiedResult, Error> {
    let result = run::<ProbabilityMass>(
        master,
        skills,
        notes,
        skill_events,
        params,
        setup,
        play,
        delta_times,
        deck,
        probes,
        ranking,
        collect_moments,
    )?;
    Ok(LuckDpCertifiedResult {
        steps: result.steps,
        probes: result.probes,
        range_moments: result.range_moments,
        peak_states: result.peak_states,
        transitions: result.transitions,
    })
}

#[allow(clippy::too_many_arguments)]
fn run<M: Mass>(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
    ranking: Option<&[crate::replay::RankConfirmation]>,
    collect_moments: bool,
) -> Result<DpResult<M::Weights>, Error> {
    propagate(&record::<M>(
        master,
        skills,
        notes,
        skill_events,
        params,
        setup,
        play,
        delta_times,
        deck,
        probes,
        ranking,
        collect_moments,
    )?)
}

/// Everything the lottery-state propagation reads, recorded from the reduced native interpreter and the optional
/// deterministic life recorder. [`propagate`] is a function of this value alone: equal transcripts give the same
/// curve, bit for bit.
struct Transcript<M> {
    collect_moments: bool,
    templates: Vec<LuckScore>,
    machine: LotteryMachine,
    /// One flag per range: whether it is a Luck range.
    luck: Vec<bool>,
    probes: Vec<bool>,
    /// Whether the plan has a Miss gauge row. Their used flags are committed after every gated frame.
    miss_rows: bool,
    frames: Vec<Frame>,
    notes: Vec<Judged>,
    hits: Vec<Hit>,
    actions: Vec<Action<M>>,
    /// `(range, lot buff at the frame time)` of every playing Luck range, in range order.
    pending: Vec<(usize, i32)>,
    /// A recording failure after the last recorded frame. It is reported once the recorded frames propagate, in
    /// the order the interleaved computation meets the two kinds of failure.
    failure: Option<Error>,
}

/// One frame, or `repeat` consecutive quiet frames (no note, no Luck range transition) with equal records.
#[derive(Clone, Copy, Debug)]
struct Frame {
    time_ms: i32,
    repeat: u32,
    /// The Luck range that starts in this frame.
    start: Option<usize>,
    complete: bool,
    finish: bool,
    gate: bool,
    /// Whether the current playing range is a Luck range.
    current_luck: bool,
    target: i32,
    /// Exclusive ends of the frame's entries in the note, action and pending lists.
    notes: usize,
    actions: usize,
    pending: usize,
}

impl Frame {
    fn quiet(&self, notes_from: usize) -> bool {
        self.notes == notes_from && self.start.is_none() && !self.complete && !self.finish
    }
}

/// A judged note in chart-time order, with its judgement after any deterministic conversion.
#[derive(Clone, Copy, Debug)]
struct Judged {
    time_ms: i32,
    note_type: i32,
    judgement: i32,
    /// Exclusive end of the note's entries in the hit list.
    hits: usize,
}

/// A Luck range containing a note, with the lot buff and gauge speed filed at the note time.
#[derive(Clone, Copy, Debug)]
struct Hit {
    range: usize,
    buff: i32,
    speed: f32,
    consumes: bool,
}

fn push_i32(out: &mut Vec<u64>, value: i32) {
    out.push(u64::from(value as u32));
}

fn action_words<M: Mass>(action: &Action<M>) -> [u64; 4] {
    match *action {
        Action::StartGauge { value, chance } => {
            let [lower, upper] = chance.bits();
            [0, value as u64, lower, upper]
        }
        Action::StartMinimum { result, chance } => {
            let [lower, upper] = chance.bits();
            [1, u64::from(result as u8), lower, upper]
        }
        Action::MissGauge { value } => [2, value as u64, 0, 0],
        Action::CriticalPoints { value, chance } => {
            let [lower, upper] = chance.bits();
            [3, u64::from(value as u32), lower, upper]
        }
        Action::StartPoints { value, chance } => {
            let [lower, upper] = chance.bits();
            [4, u64::from(value as u32), lower, upper]
        }
    }
}

fn same_actions<M: Mass>(a: &[Action<M>], b: &[Action<M>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| action_words(a) == action_words(b))
}

impl<M: Mass> Transcript<M> {
    /// Append a recorded frame, merged into the previous frame when both are quiet with equal records. The merged
    /// frame's later times are never read: a quiet frame files no weights.
    fn push_frame(&mut self, frame: Frame) {
        let n = self.frames.len();
        let notes_from = self.frames.last().map_or(0, |f| f.notes);
        if n > 0 && frame.quiet(notes_from) {
            let from = |i: usize| {
                if i == 0 {
                    (0, 0, 0)
                } else {
                    (self.frames[i - 1].notes, self.frames[i - 1].actions, self.frames[i - 1].pending)
                }
            };
            let previous = self.frames[n - 1];
            let (previous_notes, previous_actions, previous_pending) = from(n - 1);
            if previous.quiet(previous_notes)
                && previous.repeat < u32::MAX
                && (previous.gate, previous.current_luck, previous.target)
                    == (frame.gate, frame.current_luck, frame.target)
                && same_actions(
                    &self.actions[previous_actions..previous.actions],
                    &self.actions[previous.actions..frame.actions],
                )
                && self.pending[previous_pending..previous.pending] == self.pending[previous.pending..frame.pending]
            {
                self.actions.truncate(previous.actions);
                self.pending.truncate(previous.pending);
                self.frames[n - 1].repeat += 1;
                return;
            }
        }
        self.frames.push(frame);
    }

    /// Every field as words (binary32 and binary64 values by bit pattern, every list after its length), or None
    /// for a failed recording. Equal words mean equal transcripts.
    fn key(&self) -> Option<Vec<u64>> {
        let Self {
            collect_moments,
            templates,
            machine,
            luck,
            probes,
            miss_rows,
            frames,
            notes,
            hits,
            actions,
            pending,
            failure,
        } = self;
        if failure.is_some() {
            return None;
        }
        let mut out = Vec::with_capacity(64 + 4 * frames.len() + 4 * notes.len() + 4 * hits.len() + 4 * actions.len());
        if *collect_moments {
            out.push(0x6d6f6d656e747331);
        }
        out.push(templates.len() as u64);
        templates.iter().for_each(|template| template.push_words(&mut out));
        machine.push_words(&mut out);
        out.push(luck.len() as u64);
        out.extend(luck.iter().map(|&flag| u64::from(flag)));
        out.push(probes.len() as u64);
        out.extend(probes.iter().map(|&flag| u64::from(flag)));
        out.push(u64::from(*miss_rows));
        out.push(frames.len() as u64);
        for frame in frames {
            let Frame { time_ms, repeat, start, complete, finish, gate, current_luck, target, notes, actions, pending } =
                *frame;
            push_i32(&mut out, time_ms);
            out.push(u64::from(repeat));
            out.push(start.map_or(0, |range| range as u64 + 1));
            out.push(
                u64::from(complete) | u64::from(finish) << 1 | u64::from(gate) << 2 | u64::from(current_luck) << 3,
            );
            push_i32(&mut out, target);
            out.extend([notes as u64, actions as u64, pending as u64]);
        }
        out.push(notes.len() as u64);
        for &Judged { time_ms, note_type, judgement, hits } in notes {
            push_i32(&mut out, time_ms);
            push_i32(&mut out, note_type);
            push_i32(&mut out, judgement);
            out.push(hits as u64);
        }
        out.push(hits.len() as u64);
        for &Hit { range, buff, speed, consumes } in hits {
            out.push(range as u64);
            push_i32(&mut out, buff);
            out.push(u64::from(speed.to_bits()));
            out.push(u64::from(consumes));
        }
        out.push(actions.len() as u64);
        actions.iter().for_each(|action| out.extend(action_words(action)));
        out.push(pending.len() as u64);
        for &(range, buff) in pending {
            out.push(range as u64);
            push_i32(&mut out, buff);
        }
        Some(out)
    }
}

/// Diagnostic totals of the recording pass of the calling thread (diagnostic builds only; zero elsewhere).
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckRecordProfile {
    pub calls: u64,
    /// Recordings whose reduced model kept no condition skill, and recordings that ran the life recorder.
    pub without_skills: u64,
    pub with_life: u64,
    pub total_ms: f64,
    pub life_setup_ms: f64,
    pub life_frames_ms: f64,
    pub before_ms: f64,
    pub after_ms: f64,
}

#[cfg(feature = "search-diagnostics")]
thread_local! {
    static RECORD_PROFILE: std::cell::RefCell<LuckRecordProfile> = std::cell::RefCell::new(LuckRecordProfile::default());
}

#[cfg(feature = "search-diagnostics")]
fn timed<T>(slot: fn(&mut LuckRecordProfile) -> &mut f64, run: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let out = run();
    let ms = started.elapsed().as_secs_f64() * 1e3;
    RECORD_PROFILE.with(|profile| *slot(&mut profile.borrow_mut()) += ms);
    out
}

#[cfg(feature = "search-diagnostics")]
fn count(slot: fn(&mut LuckRecordProfile) -> &mut u64) {
    RECORD_PROFILE.with(|profile| *slot(&mut profile.borrow_mut()) += 1);
}

#[cfg(not(feature = "search-diagnostics"))]
#[inline(always)]
fn count(_: fn(&mut LuckRecordProfile) -> &mut u64) {}

#[cfg(not(feature = "search-diagnostics"))]
#[inline(always)]
fn timed<T>(_: fn(&mut LuckRecordProfile) -> &mut f64, run: impl FnOnce() -> T) -> T {
    run()
}

/// Return and reset the calling thread's recording totals. These are measurements only.
pub fn take_luck_record_profile() -> LuckRecordProfile {
    #[cfg(feature = "search-diagnostics")]
    {
        RECORD_PROFILE.with(|profile| std::mem::take(&mut *profile.borrow_mut()))
    }
    #[cfg(not(feature = "search-diagnostics"))]
    {
        LuckRecordProfile::default()
    }
}

/// Run the reduced native interpreter (and the deterministic life recorder when a retained row reads life or a
/// judgement can convert) and record the propagation's inputs. Failures before the first frame return at once; a
/// failure inside the frame loop ends the transcript (see [`Transcript::failure`]).
#[allow(clippy::too_many_arguments)]
fn record<M: Mass>(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
    ranking: Option<&[crate::replay::RankConfirmation]>,
    collect_moments: bool,
) -> Result<Transcript<M>, Error> {
    timed(
        |p| &mut p.total_ms,
        || {
            record_frames(
                master,
                skills,
                notes,
                skill_events,
                params,
                setup,
                play,
                delta_times,
                deck,
                probes,
                ranking,
                collect_moments,
            )
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn record_frames<M: Mass>(
    master: &Master,
    skills: &LuckSkills,
    notes: &[LiveNote],
    skill_events: &[(i32, i32)],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    deck: &[Performer],
    probes: Option<&[Option<usize>]>,
    ranking: Option<&[crate::replay::RankConfirmation]>,
    collect_moments: bool,
) -> Result<Transcript<M>, Error> {
    // Initial/precomputed draws choose `next` without adding score. Each consumed result calls add_score
    // once; non-overlapping Luck ranges consume at most once per note plus one pending result per frame.
    // Thus saturation at four Criticals cannot hide a later native i32 wrap back to zero.
    consume_bound(play.frames.len(), play.frames.iter().map(|frame| frame.judged.len()))?;
    if delta_times.len() != play.frames.len() {
        return Err(Error::Input("LUCK DP needs one delta time per frame".into()));
    }
    if probes.is_some_and(|probes| probes.len() != skills.shapes.len()) {
        return Err(Error::Input(format!("LUCK DP needs one probe per shape ({})", skills.shapes.len())));
    }
    for performer in deck {
        if luck::luck_signature(master, performer)?.is_none() {
            return Err(Error::Unsupported(
                "LUCK DP: an ordinary lottery effect or cumulative lottery condition".into(),
            ));
        }
    }
    let reduced: Vec<_> = deck
        .iter()
        .cloned()
        .map(|mut performer| {
            performer.live_skill = None;
            performer.support_skills.clear();
            performer
        })
        .collect();
    let mut model =
        LiveModel::build(master, &reduced, notes, &[], params, Some(setup), ranking.is_some(), None, Some(skills))?;
    if let Some(ranking) = ranking {
        model.set_rank_confirmation_timeline(ranking)?;
    }
    let plan = compile::<M>(master, &mut model, skills, probes, collect_moments)?;
    count(|p| &mut p.calls);
    if model.cond.is_empty() {
        count(|p| &mut p.without_skills);
    }
    let mut life = if plan.actions.iter().any(|(_, _, checker)| checker.as_ref().is_some_and(reads_life))
        || luck_has_judgement_conversion(master, deck)
    {
        count(|p| &mut p.with_life);
        Some(timed(
            |p| &mut p.life_setup_ms,
            || life_recorder(master, deck, notes, skill_events, params, setup, ranking),
        )?)
    } else {
        None
    };
    let gk = model.gk.as_mut().expect("Gekisou setup supplied");
    let ranges: Vec<_> = gk.ctrl.ranges.iter().map(|range| (range.start_ms, range.end_ms, range.mission)).collect();
    let templates = gk.ctrl.states.iter().map(|state| state.luck.clone()).collect();
    let machine = gk.ctrl.machine.clone();
    gk.ctrl.luck_weighted = true;
    let note_map: FxHashMap<_, _> = notes.iter().map(|note| (note.note_id, note)).collect();
    let Plan { actions, probes: probe_flags } = plan;
    let mut transcript = Transcript {
        collect_moments,
        templates,
        machine,
        luck: ranges.iter().map(|range| range.2 == M_LUCK).collect(),
        probes: probe_flags,
        miss_rows: actions.iter().any(|(_, action, _)| matches!(action, Action::MissGauge { .. })),
        frames: Vec::new(),
        notes: Vec::new(),
        hits: Vec::new(),
        actions: Vec::new(),
        pending: Vec::new(),
        failure: None,
    };
    let mut previous_frame = i32::MIN;
    for (frame, &delta) in play.frames.iter().zip(delta_times) {
        let recorded = record_frame(
            &mut model,
            life.as_mut(),
            &actions,
            &ranges,
            &note_map,
            frame,
            delta,
            previous_frame,
            &mut transcript,
        );
        if let Err(error) = recorded {
            transcript.failure = Some(error);
            break;
        }
        previous_frame = frame.time_ms;
    }
    Ok(transcript)
}

/// Record one frame. Every check that precedes the frame's propagation fails before the frame is appended; the
/// native AFTER phase runs once it is appended, as in the interleaved computation.
#[allow(clippy::too_many_arguments)]
fn record_frame<M: Mass>(
    model: &mut LiveModel,
    life: Option<&mut LiveModel>,
    actions: &[(i64, Action<M>, Option<Checker>)],
    ranges: &[(i32, i32, i64)],
    note_map: &FxHashMap<i32, &LiveNote>,
    frame: &PlayFrame,
    delta: f32,
    previous_frame: i32,
    out: &mut Transcript<M>,
) -> Result<(), Error> {
    if frame.time_ms <= previous_frame || !delta.is_finite() || delta < 0.0 {
        return Err(Error::Input("LUCK DP needs increasing frames and finite nonnegative deltas".into()));
    }
    let mut judged = Vec::with_capacity(frame.judged.len());
    let mut previous_note = previous_frame;
    for judgement in &frame.judged {
        let note = note_map
            .get(&judgement.note_id)
            .ok_or_else(|| Error::Input(format!("unknown note {}", judgement.note_id)))?;
        if note.time_ms <= previous_frame
            || note.time_ms > frame.time_ms
            || note.time_ms < previous_note
            || judgement.judgement_time_ms != note.time_ms
        {
            return Err(Error::Unsupported(
                "LUCK DP requires notes in their first chart-time frame, in chart-time order".into(),
            ));
        }
        if luck::luck_judgement_class(judgement.judgement).is_none() {
            return Err(Error::Unsupported(format!("LUCK DP judgement {}", judgement.judgement)));
        }
        previous_note = note.time_ms;
        judged.push((note.note_id, note.note_operate_type, note.time_ms, judgement.judgement));
    }
    let phase_life = if let Some(life) = life {
        timed(|p| &mut p.life_frames_ms, || life.frame_timed(frame.time_ms, &frame.judged, delta))?;
        if life.draws() != 0 {
            return Err(Error::Unsupported("LUCK life recorder consumed a random value".into()));
        }
        for (judged, &(_, converted, _)) in judged.iter_mut().zip(life.frame_judgements()) {
            judged.3 = converted;
        }
        life.phase_life.expect("life recorder phase trace")
    } else {
        [model.life.current_life; 2]
    };
    timed(|p| &mut p.before_ms, || record_before(model, frame.time_ms, delta))?;
    let controller = &model.gk.as_ref().expect("Gekisou checked").ctrl;
    let states = |range: usize| controller.states[range].state;
    let updates = &controller.state_updates;
    let current = controller.current_playing_index;
    let target = controller.dp_playing_range_index();
    let current_luck = current >= 0 && ranges[current as usize].2 == M_LUCK;
    let gate = updates.iter().any(|&i| ranges[i].2 == M_LUCK) || current_luck;
    let (mut active, mut active_luck) = (0usize, false);
    for (range, state) in controller.states.iter().enumerate() {
        if (S_START..S_FINISH).contains(&state.state) {
            active += 1;
            active_luck |= ranges[range].2 == M_LUCK;
        }
    }
    if active > 1 && active_luck {
        return Err(Error::Unsupported("LUCK DP: a Luck range overlaps another active range".into()));
    }
    let start =
        updates.iter().find_map(|&range| (ranges[range].2 == M_LUCK && states(range) == S_START).then_some(range));
    let complete = updates.iter().any(|&i| ranges[i].2 == M_LUCK && states(i) == S_COMPLETE);
    let finish = updates.iter().any(|&i| ranges[i].2 == M_LUCK && states(i) == S_FINISH);
    if finish && updates.iter().any(|&i| states(i) == S_START) {
        // The old rush is closed at frame time, while a new range's note can file a command at
        // an EARLIER chart time. That needs overlapping historical span state, outside this prototype.
        return Err(Error::Unsupported("LUCK DP: another range starts in a Luck finish frame".into()));
    }
    for &(_, note_type, note_time, judgement) in &judged {
        for (range, &(begin, end, mission)) in ranges.iter().enumerate() {
            if mission == M_LUCK && begin <= note_time && note_time <= end {
                let (buff, speed) = controller.dp_factors_at(note_time);
                out.hits.push(Hit { range, buff, speed, consumes: states(range) <= S_END });
            }
        }
        out.notes.push(Judged { time_ms: note_time, note_type, judgement, hits: out.hits.len() });
    }
    if gate && target >= 0 {
        for (phase, action, condition) in actions {
            let mass = condition
                .as_ref()
                .map_or(Some(M::ONE), |checker| chance::<M>(checker, phase_life[(*phase - 1) as usize]))
                .expect("compiled start condition");
            match *action {
                Action::StartGauge { value, .. } if start.is_some() => {
                    out.actions.push(Action::StartGauge { value, chance: mass })
                }
                Action::StartMinimum { result, .. } if start.is_some() => {
                    out.actions.push(Action::StartMinimum { result, chance: mass })
                }
                Action::MissGauge { value } => out.actions.push(Action::MissGauge { value }),
                Action::CriticalPoints { value, .. } => {
                    out.actions.push(Action::CriticalPoints { value, chance: mass })
                }
                Action::StartPoints { value, .. } if start.is_some() => {
                    out.actions.push(Action::StartPoints { value, chance: mass })
                }
                _ => {}
            }
        }
    }
    for (range, state) in controller.states.iter().enumerate() {
        if ranges[range].2 == M_LUCK && state.state == S_PLAYING {
            out.pending.push((range, controller.dp_factors_at(frame.time_ms).0));
        }
    }
    let recorded = Frame {
        time_ms: frame.time_ms,
        repeat: 1,
        start,
        complete,
        finish,
        gate,
        current_luck,
        target,
        notes: out.notes.len(),
        actions: out.actions.len(),
        pending: out.pending.len(),
    };
    out.push_frame(recorded);
    timed(|p| &mut p.after_ms, || record_after(model, frame.time_ms, &judged))
}

/// Propagate the lottery-state distribution through a transcript.
fn propagate<M: Mass>(transcript: &Transcript<M>) -> Result<DpResult<M::Weights>, Error> {
    let t = transcript;
    let mut dp = Dp::<M>::new(t.templates.clone(), &t.machine, t.collect_moments);
    dp.track_critical = t.actions.iter().any(|a| matches!(a, Action::CriticalPoints { .. }));
    // Each note hit and each pending frame consumes at most one result. Critical point commands fire at
    // most once per recorded action. These bounds prove that additive indicators cannot wrap native i32.
    if t.collect_moments {
        let draws = t.hits.len() as i128 + t.frames.iter().map(|f| i128::from(f.repeat)).sum::<i128>();
        let mut lower = 0i128;
        let mut upper = 10 * draws;
        let mut from = 0;
        for frame in &t.frames {
            for action in &t.actions[from..frame.actions] {
                if let Action::CriticalPoints { value, chance } | Action::StartPoints { value, chance } = *action
                    && chance.possible()
                {
                    let value = i128::from(value) * i128::from(frame.repeat);
                    lower += value.min(0);
                    upper += value.max(0);
                }
            }
            from = frame.actions;
        }
        if t.templates.iter().any(|s| {
            i128::from(s.total_bonus_point) + lower < i128::from(i32::MIN)
                || i128::from(s.total_bonus_point) + upper > i128::from(i32::MAX)
        }) {
            return Err(Error::Unsupported("LUCK DP: additive lottery points may wrap".into()));
        }
    }
    let mut steps: Vec<(i32, M::Weights)> = Vec::new();
    let mut previous_lot = false;
    let mut queued = vec![false; t.luck.len()];
    let (mut notes_from, mut actions_from, mut pending_from) = (0usize, 0usize, 0usize);
    for frame in &t.frames {
        let hits_from = notes_from.checked_sub(1).map_or(0, |i| t.notes[i].hits);
        let notes = &t.notes[notes_from..frame.notes];
        let actions = &t.actions[actions_from..frame.actions];
        let pending = &t.pending[pending_from..frame.pending];
        (notes_from, actions_from, pending_from) = (frame.notes, frame.actions, frame.pending);
        let (finish, complete, gate, current_luck) = (frame.finish, frame.complete, frame.gate, frame.current_luck);
        for _ in 0..frame.repeat {
            if notes.is_empty()
                && frame.start.is_none()
                && !complete
                && !finish
                && !previous_lot
                && !pending.iter().any(|&(range, _)| queued[range])
            {
                // No lottery or dependent skill state can change here. A consumed lot forces the FOLLOWING
                // frame through the DP for 7021 and previous-frame 7000.
                continue;
            }
            if let Some(range) = frame.start {
                debug_assert!(dp.active_range.is_none());
                dp.active_range = Some(range);
            }
            let starting_chain = frame.start.map(|range| Chain::of(&dp.templates[range]));
            dp.map(|mut state| {
                if let Some(chain) = starting_chain {
                    state.chain = chain;
                }
                state.query_rush = state.rush;
                state.score_before = state.score;
                state.frame_lot = false;
                state.frame_miss = false;
                state.frame_critical = false;
                if finish {
                    state.rush = false;
                }
                if gate {
                    if current_luck {
                        state.score = state.chain.rush != 0;
                    }
                    if complete || finish {
                        state.score = false;
                    }
                }
                if complete {
                    state.minimum = 0;
                    state.miss_used = false;
                }
                Ok(state)
            })?;
            if gate && frame.target >= 0 {
                for &action in actions {
                    dp.action(action, frame.target as usize)?;
                }
                if t.miss_rows {
                    dp.map(|mut state| {
                        state.miss_used |= state.previous_miss;
                        Ok(state)
                    })?;
                }
            }
            let mut hit = hits_from;
            let mut i = 0;
            while i < notes.len() {
                let time = notes[i].time_ms;
                let mut end = i + 1;
                while end < notes.len() && notes[end].time_ms == time {
                    end += 1;
                }
                for note in &notes[i..end] {
                    for h in &t.hits[hit..note.hits] {
                        dp.note(h.range, note.note_type, note.judgement, h.buff, h.speed, h.consumes)?;
                    }
                    hit = note.hits;
                }
                if time < frame.time_ms {
                    let values = dp.weights(&t.probes, false);
                    if steps.last().is_none_or(|last| last.1 != values) {
                        steps.push((time, values));
                    }
                }
                i = end;
            }
            for &(range, buff) in pending {
                dp.pending(range, buff)?;
            }
            if notes.last().is_some_and(|note| note.time_ms == frame.time_ms) {
                let values = dp.weights(&t.probes, true);
                if steps.last().is_none_or(|last| last.1 != values) {
                    steps.push((frame.time_ms, values));
                }
            }
            previous_lot = dp.dist.keys().any(|state| state.frame_lot);
            dp.map(|mut state| {
                state.previous_miss = state.frame_miss;
                state.previous_critical = state.frame_critical;
                state.query_rush = state.rush;
                state.score_before = state.score;
                state.frame_miss = false;
                state.frame_critical = false;
                state.frame_lot = false;
                if finish {
                    state.chain = Chain::default();
                }
                Ok(state)
            })?;
            if finish {
                dp.active_range = None;
            }
            queued.fill(false);
            if let Some(range) = dp.active_range {
                queued[range] = dp.dist.keys().any(|state| state.chain.lots > 0);
            }
        }
    }
    if let Some(failure) = &t.failure {
        return Err(failure.clone());
    }
    Ok(DpResult {
        steps,
        probes: t.probes.clone(),
        range_moments: dp.range_moments,
        peak_states: dp.peak,
        transitions: dp.transitions,
    })
}

/// Certified curves reused within one request, across performance orders and decks.
///
/// A curve is keyed by the complete transcript its propagation reads: the reduced native recording (range
/// templates, lottery tables, probes, every frame's range transitions, judged notes with their lot buffs and
/// binary32 gauge speeds, start-row chances and pending lots) encoded word for word. A hit compares the whole key,
/// so it returns exactly the curve [`luck_rush_dp_certified_with_ranking`] computes for the same arguments. The
/// recording itself runs on every call; failed recordings and failed propagations are never stored. Entries are
/// dropped oldest first once the keys exceed the byte capacity.
#[derive(Default)]
pub struct LuckDpCache {
    entries: FxHashMap<std::sync::Arc<[u64]>, std::sync::Arc<LuckDpCertifiedResult>>,
    order: std::collections::VecDeque<std::sync::Arc<[u64]>>,
    words: usize,
    capacity_words: usize,
    stats: LuckDpCacheStats,
}

/// Use of a [`LuckDpCache`]. Timings are filled only in diagnostic builds.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckDpCacheStats {
    pub lookups: u64,
    pub hits: u64,
    pub evictions: u64,
    pub peak_entries: usize,
    /// The largest total key size held, in bytes.
    pub peak_key_bytes: usize,
    pub record_ms: f64,
    pub propagate_ms: f64,
}

impl LuckDpCache {
    /// A cache holding keys of at most `capacity_bytes` in total; zero stores nothing.
    pub fn new(capacity_bytes: usize) -> Self {
        Self { capacity_words: capacity_bytes / std::mem::size_of::<u64>(), ..Default::default() }
    }

    pub fn stats(&self) -> LuckDpCacheStats {
        self.stats
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// [`luck_rush_dp_certified_with_ranking`] of the same arguments, from the cache when an equal transcript was
    /// propagated before.
    #[allow(clippy::too_many_arguments)]
    pub fn certified(
        &mut self,
        master: &Master,
        skills: &LuckSkills,
        notes: &[LiveNote],
        skill_events: &[(i32, i32)],
        params: LiveParams,
        setup: &GekisouSetup,
        play: &LivePlay,
        delta_times: &[f32],
        deck: &[Performer],
        probes: Option<&[Option<usize>]>,
        ranking: Option<&[crate::replay::RankConfirmation]>,
    ) -> Result<std::sync::Arc<LuckDpCertifiedResult>, Error> {
        self.certified_mode(
            master,
            skills,
            notes,
            skill_events,
            params,
            setup,
            play,
            delta_times,
            deck,
            probes,
            ranking,
            false,
        )
    }

    /// Cached certified curve with additive range indicators.
    #[allow(clippy::too_many_arguments)]
    pub fn certified_with_moments(
        &mut self,
        master: &Master,
        skills: &LuckSkills,
        notes: &[LiveNote],
        skill_events: &[(i32, i32)],
        params: LiveParams,
        setup: &GekisouSetup,
        play: &LivePlay,
        delta_times: &[f32],
        deck: &[Performer],
        probes: Option<&[Option<usize>]>,
        ranking: Option<&[crate::replay::RankConfirmation]>,
    ) -> Result<std::sync::Arc<LuckDpCertifiedResult>, Error> {
        self.certified_mode(
            master,
            skills,
            notes,
            skill_events,
            params,
            setup,
            play,
            delta_times,
            deck,
            probes,
            ranking,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn certified_mode(
        &mut self,
        master: &Master,
        skills: &LuckSkills,
        notes: &[LiveNote],
        skill_events: &[(i32, i32)],
        params: LiveParams,
        setup: &GekisouSetup,
        play: &LivePlay,
        delta_times: &[f32],
        deck: &[Performer],
        probes: Option<&[Option<usize>]>,
        ranking: Option<&[crate::replay::RankConfirmation]>,
        collect_moments: bool,
    ) -> Result<std::sync::Arc<LuckDpCertifiedResult>, Error> {
        #[cfg(feature = "search-diagnostics")]
        let started = std::time::Instant::now();
        let transcript = record::<ProbabilityMass>(
            master,
            skills,
            notes,
            skill_events,
            params,
            setup,
            play,
            delta_times,
            deck,
            probes,
            ranking,
            collect_moments,
        );
        let key = transcript.as_ref().ok().and_then(|transcript| transcript.key());
        #[cfg(feature = "search-diagnostics")]
        {
            self.stats.record_ms += started.elapsed().as_secs_f64() * 1e3;
        }
        let transcript = transcript?;
        if let Some(key) = &key {
            self.stats.lookups += 1;
            if let Some(found) = self.entries.get(&key[..]) {
                self.stats.hits += 1;
                return Ok(found.clone());
            }
        }
        #[cfg(feature = "search-diagnostics")]
        let started = std::time::Instant::now();
        let result = propagate(&transcript);
        #[cfg(feature = "search-diagnostics")]
        {
            self.stats.propagate_ms += started.elapsed().as_secs_f64() * 1e3;
        }
        let result = result?;
        let result = std::sync::Arc::new(LuckDpCertifiedResult {
            steps: result.steps,
            probes: result.probes,
            range_moments: result.range_moments,
            peak_states: result.peak_states,
            transitions: result.transitions,
        });
        if let Some(key) = key {
            self.insert(key, result.clone());
        }
        Ok(result)
    }

    fn insert(&mut self, key: Vec<u64>, value: std::sync::Arc<LuckDpCertifiedResult>) {
        if key.len() > self.capacity_words {
            return;
        }
        while self.words + key.len() > self.capacity_words {
            let oldest = self.order.pop_front().expect("held keys account for the held words");
            self.words -= oldest.len();
            self.entries.remove(&oldest[..]);
            self.stats.evictions += 1;
        }
        let key: std::sync::Arc<[u64]> = key.into();
        self.words += key.len();
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
        self.stats.peak_entries = self.stats.peak_entries.max(self.entries.len());
        self.stats.peak_key_bytes = self.stats.peak_key_bytes.max(self.words * std::mem::size_of::<u64>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::certified::F64Interval;
    use serde_json::{Value, json};

    fn assert_certified_encloses_nominal(certified: &LuckDpCertifiedResult, nominal: &LuckDpResult) {
        for &(time, joint) in &certified.steps {
            let total = joint.iter().fold(F64Interval::ZERO, |sum, mass| sum.add(mass.interval()));
            assert!(total.contains(1.0), "t={time} total={total:?}");
            let values = &nominal.steps[nominal.steps.partition_point(|step| step.0 <= time) - 1].1;
            let rush = joint[2].merge_disjoint(joint[3]).interval();
            assert!((rush.lower() as f32..=rush.upper() as f32).contains(&values[0]));
            for (shape, &enabled) in certified.probes.iter().enumerate() {
                let score = if enabled { joint[1].merge_disjoint(joint[3]) } else { ProbabilityMass::ZERO }.interval();
                let both = if enabled { joint[3] } else { ProbabilityMass::ZERO }.interval();
                assert!((score.lower() as f32..=score.upper() as f32).contains(&values[1 + 2 * shape]));
                assert!((both.lower() as f32..=both.upper() as f32).contains(&values[2 + 2 * shape]));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        id: i64,
        key: &str,
        skill: i64,
        effect: i64,
        value: i64,
        trigger: i64,
        condition: i64,
        release: i64,
    ) -> Value {
        json!({"_id":id,key:skill,"_level":1,"_skillTriggerType":1,
            "_skillTriggerConditionGroup":trigger,"_skillConditionGroup":condition,"_skillReleaseConditionGroup":release,
            "_skillTargetIDs":[],"_skillEffectType":effect,"_activationTimeSecond":0.0,"_effectValue":value,
            "_maxEffectValue":0,"_effectLimitCount":1,"_skillCumulativeConditionID":0,
            "_effectExecuteLimitCount":0,"_effectExecuteLimitResetConditionGroup":0})
    }

    #[test]
    fn ordinary_and_gekisou_conversions_prevent_a_chart_only_luck_curve() {
        let tables = json!({
            "MasterLiveSkillEffect":[row(1,"_liveSkillID",41,12006,10000,0,0,0)],
            "MasterSupportSkillEffect":[row(2,"_supportSkillID",42,13005,10000,0,0,0)],
            "MasterGekisouSkillEffect":[row(3,"_gekisouSkillID",43,12006,10000,0,0,0)],
            "MasterGekisouSupportSkillEffect":[row(4,"_gekisouSupportSkillID",44,12006,10000,0,0,0)]
        });
        let texts: Vec<_> = tables
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, rows)| (name.clone(), json!({"_allData":rows}).to_string()))
            .collect();
        let master =
            Master::from_json_tables(|name| texts.iter().find(|(key, _)| key == name).map(|(_, text)| text.as_str()))
                .unwrap();
        for performer in [
            Performer { live_skill: Some((41, 1)), ..Default::default() },
            Performer { support_skills: vec![(42, 1)], ..Default::default() },
            Performer { gekisou_skill: Some((43, 1)), ..Default::default() },
            Performer { gekisou_skill: Some((1, 1)), gekisou_support_skills: vec![(44, 1)], ..Default::default() },
        ] {
            assert!(luck_has_judgement_conversion(&master, &[performer]));
        }
        assert!(!luck_has_judgement_conversion(&master, &[Performer::default()]));
        assert!(!luck_has_judgement_conversion(
            &master,
            &[Performer { live_skill: Some((41, 2)), ..Default::default() }]
        ));
        // Gekisou Snap effects have no controller to run on when their member has no Gekisou skill.
        assert!(!luck_has_judgement_conversion(
            &master,
            &[Performer { gekisou_support_skills: vec![(44, 1)], ..Default::default() }]
        ));
    }

    fn fixture(result: i64, base: i64) -> (Master, Vec<LiveNote>, LiveParams, GekisouSetup, LivePlay, Vec<f32>) {
        let mut speed = row(1, "_gekisouSkillID", 1, 11001, 20000, 7010, 0, 0);
        speed["_activationTimeSecond"] = json!(0.2);
        let mut sustained = row(3, "_gekisouSkillID", 3, 11001, 20000, 7020, 0, 7013);
        sustained["_skillTriggerType"] = json!(2);
        let mut score = row(31, "_gekisouSupportSkillID", 31, 2000, 10000, 7021, 0, 0);
        score["_skillTriggerType"] = json!(2);
        let mut miss = row(96, "_gekisouSupportSkillID", 96, 11003, 10000, 7000, 0, 7013);
        miss["_effectExecuteLimitCount"] = json!(1);
        miss["_effectExecuteLimitResetConditionGroup"] = json!(7013);
        let lots: Vec<_> =
            (0..5).map(|kind| json!({"_id":kind+1,"_chanceLotType":kind,"_lotResult":result,"_weight":1})).collect();
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
            "MasterLiveGekisouLuckBasePoint":[{"_id":1,"_noteCategory":0,"_noteSimulateJudgement":5,"_weight":1,"_basePoint":base}],
            "MasterLiveGekisouLuckBonusLot":lots,
            "MasterSkillTarget":[{"_id":56,"_skillTargetType":5,"_gekisouMissionType":2}],
            "MasterSkillCondition":[
                {"_id":4011,"_conditionType":4011,"_conditionValues":[100],"_conditionTargetIDs":[],"_isPositive":true},
                {"_id":7000,"_conditionType":7000,"_conditionValues":[0],"_conditionTargetIDs":[],"_isPositive":true},
                {"_id":7010,"_conditionType":7010,"_conditionValues":[],"_conditionTargetIDs":[56],"_isPositive":true},
                {"_id":7013,"_conditionType":7013,"_conditionValues":[],"_conditionTargetIDs":[],"_isPositive":true},
                {"_id":7020,"_conditionType":7020,"_conditionValues":[],"_conditionTargetIDs":[56],"_isPositive":true},
                {"_id":7021,"_conditionType":7021,"_conditionValues":[],"_conditionTargetIDs":[],"_isPositive":true}],
            "MasterSkillConditionSet":[
                {"_id":1,"_group":4011,"_conditionIds":[4011]},
                {"_id":2,"_group":7000,"_conditionIds":[7000]},
                {"_id":3,"_group":7010,"_conditionIds":[7010]},
                {"_id":4,"_group":7013,"_conditionIds":[7013]},
                {"_id":5,"_group":7021,"_conditionIds":[7021]},
                {"_id":6,"_group":7020,"_conditionIds":[7020]}],
            "MasterGekisouSkill":[{"_id":1,"_gekisouMissionType":2},{"_id":2,"_gekisouMissionType":2},{"_id":3,"_gekisouMissionType":2}],
            "MasterGekisouSkillEffect":[speed,sustained,row(2,"_gekisouSkillID",2,11003,10000,7010,4011,7013)],
            "MasterGekisouSupportSkill":[{"_id":31,"_gekisouMissionType":2},{"_id":66,"_gekisouMissionType":2},{"_id":96,"_gekisouMissionType":2}],
            "MasterGekisouSupportSkillEffect":[score,miss,row(66,"_gekisouSupportSkillID",66,11005,4,7010,4011,7013)]
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
    fn deterministic_curves_match_native_with_same_frame_notes_and_probe_delay() {
        for result in [0, 3] {
            let (master, notes, params, setup, play, delta) = fixture(result, 60);
            let skills = luck_skills(&master).unwrap();
            for skill in [1, 2, 3] {
                let deck = [Performer {
                    gekisou_skill: Some((skill, 1)),
                    gekisou_support_skills: vec![(31, 1), (96, 1)],
                    ..Default::default()
                }];
                let dp = luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
                let samples = luck::luck_rush_samples(
                    &master,
                    &skills,
                    &notes,
                    params,
                    &setup,
                    &play,
                    &delta,
                    &deck,
                    None,
                    &[0, 1, 7],
                )
                .unwrap();
                assert_eq!(dp.steps, samples, "result={result} skill={skill}");
                assert!(dp.peak_states >= 1 && dp.transitions > 0);
            }
        }
    }

    #[test]
    fn range_moments_match_complete_lottery_indicators_and_critical_skills() {
        for result in 0..4 {
            let (mut master, notes, params, mut setup, play, delta) = fixture(result, 60);
            master.skill_conditions.push(
                serde_json::from_value(json!({
                    "_id": 7003, "_conditionType": 7000, "_conditionValues": [3],
                    "_conditionTargetIDs": [], "_isPositive": true,
                }))
                .unwrap(),
            );
            master.skill_condition_sets.push(
                serde_json::from_value(json!({
                    "_id": 7, "_group": 7003, "_conditionIds": [7003],
                }))
                .unwrap(),
            );
            master.gekisou_support_skills.push(crate::master::SkillRow {
                id: 77,
                gekisou_mission_type: 2,
                ..Default::default()
            });
            master
                .gekisou_support_skill_effects
                .push(serde_json::from_value(row(77, "_gekisouSupportSkillID", 77, 11002, 7, 7003, 0, 0)).unwrap());
            master.reindex().unwrap();
            setup.fevers = vec![(100, 300), (1100, 1200)];
            let deck = [Performer {
                gekisou_skill: Some((2, 1)),
                gekisou_mission_type: 2,
                gekisou_support_skills: vec![(77, 1)],
                ..Default::default()
            }];
            let skills = luck_skills(&master).unwrap();
            let dp = luck_rush_dp_certified_with_moments(
                &master,
                &skills,
                &notes,
                &[],
                params,
                &setup,
                &play,
                &delta,
                &deck,
                None,
                None,
            )
            .unwrap();
            let compact =
                luck_rush_dp_certified(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
            assert!(compact.range_moments.is_empty());
            let mut plain = master.clone();
            plain.gekisou_support_skill_effects.retain(|row| row.skill_id != 77);
            plain.reindex().unwrap();
            let without = luck_rush_dp_certified(
                &plain,
                &luck_skills(&plain).unwrap(),
                &notes,
                params,
                &setup,
                &play,
                &delta,
                &deck,
                None,
            )
            .unwrap();
            assert_eq!(curve_words(&compact), curve_words(&without));
            assert_eq!((compact.peak_states, compact.transitions), (without.peak_states, without.transitions));
            let mut cache = LuckDpCache::new(1 << 20);
            cache.certified(&master, &skills, &notes, &[], params, &setup, &play, &delta, &deck, None, None).unwrap();
            cache
                .certified(
                    &plain,
                    &luck_skills(&plain).unwrap(),
                    &notes,
                    &[],
                    params,
                    &setup,
                    &play,
                    &delta,
                    &deck,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(cache.stats().hits, 1);
            let indicators = cache
                .certified_with_moments(&master, &skills, &notes, &[], params, &setup, &play, &delta, &deck, None, None)
                .unwrap();
            assert!(!indicators.range_moments.is_empty());
            assert_eq!(cache.len(), 2);
            let mut native = LiveModel::new_gekisou(&master, &deck, &notes, &[], params, &setup).unwrap();
            native.run_timed(&play, &delta).unwrap();
            for (moments, range) in dp.range_moments.iter().zip(native.gekisou_ranges()) {
                assert!(
                    moments.luck_points.contains(f64::from(range.luck_points)),
                    "result={result}, range={range:?}, moments={moments:?}"
                );
                assert!(moments.luck_points.upper() - moments.luck_points.lower() < 1e-8);
                for (count, expected) in moments.lot_results.iter().zip(range.lot_results) {
                    assert!(count.contains(f64::from(expected)));
                    assert!(count.upper() - count.lower() < 1e-8);
                }
            }
        }
    }

    #[test]
    fn one_consumed_lottery_has_exact_nominal_rewards_and_critical_points() {
        let (mut master, _, mut params, mut setup, _, _) = fixture(0, 140);
        master.gekisou_skill_effects.clear();
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
        master.skill_conditions.push(
            serde_json::from_value(json!({
                "_id": 7003, "_conditionType": 7000, "_conditionValues": [3],
                "_conditionTargetIDs": [], "_isPositive": true,
            }))
            .unwrap(),
        );
        master
            .skill_condition_sets
            .push(serde_json::from_value(json!({"_id": 7, "_group": 7003, "_conditionIds": [7003]})).unwrap());
        master.gekisou_support_skills.push(crate::master::SkillRow {
            id: 77,
            gekisou_mission_type: 2,
            ..Default::default()
        });
        master
            .gekisou_support_skill_effects
            .push(serde_json::from_value(row(77, "_gekisouSupportSkillID", 77, 11002, 7, 7003, 0, 0)).unwrap());
        master.reindex().unwrap();
        let notes = [LiveNote { note_id: 1, time_ms: 200, note_operate_type: 1, judgement_type: 1 }];
        params.converted_note_count = 1;
        setup.fevers = vec![(100, 400)];
        let mut frames: Vec<_> = (0..=20).map(|i| PlayFrame { time_ms: i * 100, judged: Vec::new() }).collect();
        frames[2].judged.push(JudgedNote { note_id: 1, judgement: 5, judgement_time_ms: 200 });
        let delta = vec![0.1; frames.len()];
        let play = LivePlay { frames, base_seed: 0 };
        let deck = [Performer {
            gekisou_skill: Some((2, 1)),
            gekisou_mission_type: 2,
            gekisou_support_skills: vec![(77, 1)],
            ..Default::default()
        }];
        let skills = luck_skills(&master).unwrap();
        let dp = luck_rush_dp_certified_with_moments(
            &master,
            &skills,
            &notes,
            &[],
            params,
            &setup,
            &play,
            &delta,
            &deck,
            None,
            None,
        )
        .unwrap();
        let range = &dp.range_moments[0];
        for count in range.lot_results {
            assert!(count.contains(0.25), "{range:?}");
            assert!(count.upper() - count.lower() < 1e-10);
        }
        // One equally weighted outcome awards (0 + 5 + 10 + 10)/4, plus a seven-point
        // one-shot bonus in the next frame on the Critical branch only.
        assert!(range.luck_points.contains(8.0), "{range:?}");
        assert!(range.luck_points.upper() - range.luck_points.lower() < 1e-10);
    }

    #[test]
    fn separate_luck_ranges_reset_the_chain_and_miss_limit_like_native() {
        for result in [0, 3] {
            let (master, mut notes, mut params, mut setup, _, _) = fixture(result, 60);
            setup.fevers.extend([(1500, 1900), (2900, 3300)]);
            params.music_length_ms = 4000;
            for start in [1500, 2900] {
                for offset in [0, 10, 20, 100, 160, 200, 210, 300, 400] {
                    notes.push(LiveNote {
                        note_id: notes.len() as i32,
                        note_operate_type: 1,
                        judgement_type: 1,
                        time_ms: start + offset,
                    });
                }
            }
            params.converted_note_count = notes.len() as i32;
            let mut frames: Vec<_> = (0..=40).map(|i| PlayFrame { time_ms: i * 100, judged: Vec::new() }).collect();
            for note in &notes {
                frames.iter_mut().find(|frame| frame.time_ms >= note.time_ms).unwrap().judged.push(JudgedNote {
                    note_id: note.note_id,
                    judgement: 5,
                    judgement_time_ms: note.time_ms,
                });
            }
            let play = LivePlay { frames, base_seed: 0 };
            let delta = vec![0.1; play.frames.len()];
            let skills = luck_skills(&master).unwrap();
            let deck = [Performer {
                gekisou_skill: Some((2, 1)),
                gekisou_support_skills: vec![(31, 1), (96, 1)],
                ..Default::default()
            }];
            let dp = luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
            let native = luck::luck_rush_samples(
                &master,
                &skills,
                &notes,
                params,
                &setup,
                &play,
                &delta,
                &deck,
                None,
                &[0, 1, 7],
            )
            .unwrap();
            assert_eq!(dp.steps, native, "result={result}");
        }
    }

    #[test]
    fn guarantee_is_consumed_by_the_initial_predraw_not_the_following_result() {
        let (mut master, notes, params, setup, play, delta) = fixture(0, 60);
        for kind in 0..5 {
            master.gekisou_luck_bonus_lots.push(crate::master::LuckBonusLotRow {
                id: 100 + kind,
                chance_lot_type: kind,
                lot_result: 3,
                weight: 1,
            });
        }
        let skills = luck_skills(&master).unwrap();
        let deck = [Performer {
            gekisou_skill: Some((2, 1)),
            gekisou_support_skills: vec![(31, 1), (66, 1)],
            ..Default::default()
        }];
        let dp = luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
        let seeds: Vec<_> = (0..4096).collect();
        let samples =
            luck::luck_rush_samples(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None, &seeds)
                .unwrap();
        for &(time, ref values) in &dp.steps {
            let sample = &samples[samples.partition_point(|step| step.0 <= time) - 1].1;
            for (expected, observed) in values.iter().zip(sample) {
                assert!((expected - observed).abs() < 0.04, "t={time} expected={expected} sample={observed}");
            }
        }
    }

    #[test]
    fn certified_first_draw_encloses_exact_weight_and_start_chance_fractions() {
        let (mut master, notes, params, setup, play, delta) = fixture(0, 60);
        // The initial 60 gauge cannot draw unless the 1/4 start-gauge effect succeeds. The first
        // independent lottery is Critical with probability 2/3, so Rush at t=100 is exactly 1/6.
        master.skill_conditions.iter_mut().find(|row| row.id == 4011).unwrap().condition_values = vec![25];
        for kind in 0..5 {
            master.gekisou_luck_bonus_lots.push(crate::master::LuckBonusLotRow {
                id: 100 + kind,
                chance_lot_type: kind,
                lot_result: 3,
                weight: 2,
            });
        }
        let skills = luck_skills(&master).unwrap();
        let deck =
            [Performer { gekisou_skill: Some((2, 1)), gekisou_support_skills: vec![(31, 1)], ..Default::default() }];
        let certified =
            luck_rush_dp_certified(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
        let nominal = luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
        let joint = certified.steps.iter().find(|step| step.0 == 100).unwrap().1;
        assert!(joint[0].interval().contains(5.0 / 6.0));
        assert_eq!(joint[1], ProbabilityMass::ZERO);
        assert!(joint[2].interval().contains(1.0 / 6.0));
        assert_eq!(joint[3], ProbabilityMass::ZERO);
        assert!(joint[2].interval().upper() - joint[2].interval().lower() < 1e-12);
        assert_certified_encloses_nominal(&certified, &nominal);
    }

    #[test]
    fn certified_deterministic_curves_keep_exact_zero_and_one() {
        for result in [0, 3] {
            let (master, notes, params, setup, play, delta) = fixture(result, 60);
            let skills = luck_skills(&master).unwrap();
            let deck = [Performer {
                gekisou_skill: Some((2, 1)),
                gekisou_support_skills: vec![(31, 1), (96, 1)],
                ..Default::default()
            }];
            let certified =
                luck_rush_dp_certified(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
            let native = luck::luck_rush_samples(
                &master,
                &skills,
                &notes,
                params,
                &setup,
                &play,
                &delta,
                &deck,
                None,
                &[0, 1, 7],
            )
            .unwrap();
            for &(time, joint) in &certified.steps {
                let weights = &native[native.partition_point(|step| step.0 <= time) - 1].1;
                let index = 2 * usize::from(weights[0] == 1.0) + usize::from(weights[1] == 1.0);
                for (bucket, mass) in joint.into_iter().enumerate() {
                    assert_eq!(mass, if bucket == index { ProbabilityMass::ONE } else { ProbabilityMass::ZERO });
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn assert_native_life_curve(
        master: &Master,
        notes: &[LiveNote],
        events: &[(i32, i32)],
        params: LiveParams,
        setup: &GekisouSetup,
        play: &LivePlay,
        delta: &[f32],
        deck: &[Performer],
    ) -> LuckDpResult {
        let skills = luck_skills(master).unwrap();
        let nominal =
            luck_rush_dp_with_events(master, &skills, notes, events, params, setup, play, delta, deck, None).unwrap();
        let certified =
            luck_rush_dp_certified_with_events(master, &skills, notes, events, params, setup, play, delta, deck, None)
                .unwrap();
        assert_certified_encloses_nominal(&certified, &nominal);
        let mut native = LiveModel::new_gekisou(master, deck, notes, events, params, setup).unwrap();
        native.run_timed(play, delta).unwrap();
        let spans = native.rush_command_spans();
        for note in notes {
            let index = nominal.steps.partition_point(|step| step.0 <= note.time_ms);
            let probability = if index == 0 { 0.0 } else { nominal.steps[index - 1].1[0] };
            let expected = spans.iter().any(|&(start, end)| start <= note.time_ms && note.time_ms < end);
            assert_eq!(probability, if expected { 1.0 } else { 0.0 }, "note {} at {}", note.note_id, note.time_ms);
        }
        nominal
    }

    #[test]
    fn range_start_life_conditions_use_native_damage_and_exact_thresholds() {
        for kind in [2000, 2001, 2002, 2003] {
            for positive in [true, false] {
                let (mut master, notes, params, setup, play, delta) = fixture(3, 0);
                let condition = master.skill_conditions.iter_mut().find(|row| row.id == 4011).unwrap();
                condition.condition_type = kind;
                condition.condition_values = vec![700];
                condition.is_positive = positive;
                master.judgement_parameters[0].damage = 300;
                let deck = [Performer { gekisou_skill: Some((2, 1)), ..Default::default() }];
                let curve = assert_native_life_curve(&master, &notes, &[], params, &setup, &play, &delta, &deck);
                let starts = matches!(kind, 2001 | 2003) == positive;
                assert_eq!(curve.steps.iter().any(|step| step.1[0] > 0.0), starts, "type {kind}, positive {positive}");
            }
        }
    }

    #[test]
    fn range_start_life_reads_before_its_phase_and_after_previous_phase_healing() {
        for phase in [1, 2] {
            let (mut master, notes, params, setup, play, delta) = fixture(3, 0);
            let condition = master.skill_conditions.iter_mut().find(|row| row.id == 4011).unwrap();
            condition.condition_type = 2001;
            condition.condition_values = vec![700];
            // Reuse this fixture's unused Miss condition group as a native member-skill event trigger.
            master.skill_conditions.iter_mut().find(|row| row.id == 7000).unwrap().condition_type = 4010;
            master.gekisou_support_skill_effects.retain(|row| row.id != 96);
            master.judgement_parameters[0].damage = 600;
            master
                .skill_effect_settings
                .push(serde_json::from_value(json!({"_id":1,"_skillEffectType":11003,"_phase":phase})).unwrap());
            master
                .skill_effect_settings
                .push(serde_json::from_value(json!({"_id":2,"_skillEffectType":3001,"_phase":1})).unwrap());
            master
                .support_skill_effects
                .push(serde_json::from_value(row(501, "_supportSkillID", 501, 3001, 600, 7000, 0, 0)).unwrap());
            let deck =
                [Performer { gekisou_skill: Some((2, 1)), support_skills: vec![(501, 1)], ..Default::default() }];
            let events = [(0, 100)];
            let mut recorder = life_recorder(&master, &deck, &notes, &events, params, &setup, None).unwrap();
            for (frame, &dt) in play.frames.iter().zip(&delta).take_while(|(frame, _)| frame.time_ms <= 100) {
                recorder.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
            }
            assert_eq!(recorder.phase_life, Some([400, 1000]));
            let curve = assert_native_life_curve(&master, &notes, &events, params, &setup, &play, &delta, &deck);
            assert_eq!(curve.steps.iter().any(|step| step.1[0] > 0.0), phase == 2);
            let without_event = assert_native_life_curve(&master, &notes, &[], params, &setup, &play, &delta, &deck);
            assert!(!without_event.steps.iter().any(|step| step.1[0] > 0.0));
        }
    }

    #[test]
    fn native_consume_count_bound_proves_rush_saturation_cannot_wrap() {
        assert!(consume_bound(10, [20, 30].into_iter()).is_ok());
        assert!(consume_bound(i32::MAX as usize, [0].into_iter()).is_ok());
        assert!(matches!(consume_bound(i32::MAX as usize, [1].into_iter()), Err(Error::Capacity(_))));
        assert!(matches!(consume_bound(usize::MAX, [1].into_iter()), Err(Error::Capacity(_))));
    }

    #[test]
    fn rush_saturation_preserves_table_and_maximum_changes() {
        let mut score = LuckScore::default();
        for _ in 0..8 {
            score.add_score(3).unwrap();
            let reduced = Chain::of(&score).score(&LuckScore::default());
            assert_eq!(score.current_lot_type(), reduced.current_lot_type());
            assert_eq!(score.gauge_max, reduced.gauge_max);
        }
    }

    #[test]
    fn before_frame_probe_is_distinct_from_new_rush() {
        let (master, notes, params, setup, _, _) = fixture(3, 60);
        let model = LiveModel::new_gekisou(&master, &[], &notes, &[], params, &setup).unwrap();
        let machine = &model.gk.as_ref().unwrap().ctrl.machine;
        let mut dp = Dp::<f64>::new(vec![LuckScore::default(); 3], machine, false);
        let mut dist = Distribution::default();
        dist.insert(State { rush: true, query_rush: true, score: true, score_before: false, ..State::default() }, 1.0);
        dp.dist = dist;
        assert_eq!(dp.weights(&[true], false), [1.0, 0.0, 0.0]);
        assert_eq!(dp.weights(&[true], true), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn future_probe_shapes_and_non_theoretical_judgements_are_rejected() {
        let (mut master, notes, params, setup, mut play, delta) = fixture(3, 60);
        let deck =
            [Performer { gekisou_skill: Some((1, 1)), gekisou_support_skills: vec![(31, 1)], ..Default::default() }];
        master.gekisou_skill_effects.iter_mut().find(|row| row.id == 1).unwrap().skill_condition_group = 4011;
        let skills = luck_skills(&master).unwrap();
        assert!(matches!(
            luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None),
            Err(Error::Unsupported(_))
        ));
        master.gekisou_skill_effects.iter_mut().find(|row| row.id == 1).unwrap().skill_condition_group = 0;
        master.skill_conditions.iter_mut().find(|row| row.id == 4011).unwrap().condition_type = 2001;
        master.skill_conditions.iter_mut().find(|row| row.id == 4011).unwrap().condition_values = vec![700];
        master.gekisou_support_skill_effects.iter_mut().find(|row| row.id == 31).unwrap().skill_condition_group = 4011;
        let skills = luck_skills(&master).unwrap();
        assert!(matches!(
            luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None),
            Err(Error::Unsupported(_))
        ));
        master.gekisou_support_skill_effects.iter_mut().find(|row| row.id == 31).unwrap().skill_condition_group = 0;
        let skills = luck_skills(&master).unwrap();
        play.frames.iter_mut().find(|frame| !frame.judged.is_empty()).unwrap().judged[0].judgement_time_ms += 1;
        assert!(matches!(
            luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn overlapping_other_missions_do_not_silently_release_luck_mechanisms() {
        let (master, notes, params, mut setup, play, delta) = fixture(3, 60);
        setup.fevers.push((200, 600));
        setup.missions = vec![2, 1, 2];
        let skills = luck_skills(&master).unwrap();
        assert!(matches!(
            luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &[], None),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn finish_and_new_start_in_one_frame_need_historical_span_state() {
        let (master, mut notes, params, mut setup, mut play, delta) = fixture(3, 60);
        setup.fevers.push((1150, 1550));
        notes.push(LiveNote { note_id: 10, note_operate_type: 1, judgement_type: 1, time_ms: 1150 });
        play.frames.iter_mut().find(|frame| frame.time_ms == 1200).unwrap().judged.push(JudgedNote {
            note_id: 10,
            judgement: 5,
            judgement_time_ms: 1150,
        });
        let skills = luck_skills(&master).unwrap();
        assert!(matches!(
            luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &[], None),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn quiet_frames_preserve_pending_lots_and_next_frame_miss_effects() {
        let (mut master, notes, params, setup, _, _) = fixture(3, 60);
        // The deterministic lottery alternates Critical -> Miss -> Critical. Large additions create
        // queued lots consumed in otherwise empty frames; a Miss then adds more gauge on the NEXT frame.
        master.gekisou_luck_bonus_lots.iter_mut().find(|row| row.chance_lot_type == 4).unwrap().lot_result = 0;
        master.gekisou_skill_effects.iter_mut().find(|row| row.id == 2).unwrap().effect_value = 40_000;
        master.gekisou_support_skill_effects.iter_mut().find(|row| row.id == 96).unwrap().effect_value = 30_000;
        let skills = luck_skills(&master).unwrap();
        let deck = [Performer {
            gekisou_skill: Some((2, 1)),
            gekisou_support_skills: vec![(31, 1), (96, 1)],
            ..Default::default()
        }];
        let mut frames: Vec<_> = (0..=200).map(|i| PlayFrame { time_ms: i * 10, judged: Vec::new() }).collect();
        for note in &notes {
            frames.iter_mut().find(|frame| frame.time_ms == note.time_ms).unwrap().judged.push(JudgedNote {
                note_id: note.note_id,
                judgement: 5,
                judgement_time_ms: note.time_ms,
            });
        }
        let play = LivePlay { frames, base_seed: 0 };
        let delta = vec![0.01; play.frames.len()];
        let dp = luck_rush_dp(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None).unwrap();
        let samples =
            luck::luck_rush_samples(&master, &skills, &notes, params, &setup, &play, &delta, &deck, None, &[0, 1, 7])
                .unwrap();
        assert_eq!(dp.steps, samples);
    }

    fn curve_words(curve: &LuckDpCertifiedResult) -> Vec<u64> {
        let mut out = Vec::new();
        for (time, joint) in &curve.steps {
            out.push(u64::from(*time as u32));
            joint.iter().for_each(|mass| out.extend(mass.bits()));
        }
        out.extend(curve.probes.iter().map(|&probe| u64::from(probe)));
        out.extend([curve.peak_states as u64, curve.transitions]);
        out
    }

    /// A chance-gated start gauge and a 2:1 Critical table: the curve carries many inexact masses.
    fn random_fixture() -> (Master, Vec<LiveNote>, LiveParams, GekisouSetup, LivePlay, Vec<f32>) {
        let (mut master, notes, params, setup, play, delta) = fixture(0, 60);
        master.skill_conditions.iter_mut().find(|row| row.id == 4011).unwrap().condition_values = vec![25];
        for kind in 0..5 {
            master.gekisou_luck_bonus_lots.push(crate::master::LuckBonusLotRow {
                id: 100 + kind,
                chance_lot_type: kind,
                lot_result: 3,
                weight: 2,
            });
        }
        (master, notes, params, setup, play, delta)
    }

    #[test]
    fn curve_cache_reuses_equal_recordings_and_returns_the_computed_curve() {
        let (master, notes, params, setup, play, delta) = random_fixture();
        let skills = luck_skills(&master).unwrap();
        let holder = Performer {
            gekisou_skill: Some((2, 1)),
            gekisou_support_skills: vec![(31, 1), (96, 1)],
            ..Default::default()
        };
        let speed = Performer { gekisou_skill: Some((1, 1)), ..Default::default() };
        let decks = [
            vec![holder.clone(), Performer::default()],
            // The same members in another order: the recording is the same.
            vec![Performer::default(), holder.clone()],
            // A timed gauge-speed holder changes the binary32 speed of the range's first notes.
            vec![holder.clone(), speed.clone()],
            vec![speed, holder],
        ];
        let mut cache = LuckDpCache::new(1 << 24);
        let mut direct = Vec::new();
        for (i, deck) in decks.iter().enumerate() {
            let expected =
                luck_rush_dp_certified(&master, &skills, &notes, params, &setup, &play, &delta, deck, None).unwrap();
            let cached = cache
                .certified(&master, &skills, &notes, &[], params, &setup, &play, &delta, deck, None, None)
                .unwrap();
            assert_eq!(curve_words(&cached), curve_words(&expected), "deck {i}");
            direct.push(curve_words(&expected));
        }
        assert_ne!(direct[0], direct[2]);
        let stats = cache.stats();
        assert_eq!((stats.lookups, stats.hits, cache.len()), (4, 2, 2));
        let again = cache
            .certified(&master, &skills, &notes, &[], params, &setup, &play, &delta, &decks[2], None, None)
            .unwrap();
        assert_eq!(curve_words(&again), direct[2]);
        assert_eq!(cache.stats().hits, 3);
    }

    #[test]
    fn curve_cache_misses_on_any_recorded_input_change() {
        let (master, notes, params, setup, play, delta) = random_fixture();
        let deck = [Performer {
            gekisou_skill: Some((2, 1)),
            gekisou_support_skills: vec![(31, 1), (96, 1)],
            ..Default::default()
        }];
        let mut later = setup.clone();
        later.fevers[0] = (200, 500);
        let mut chance = master.clone();
        chance.skill_conditions.iter_mut().find(|row| row.id == 4011).unwrap().condition_values = vec![50];
        let mut weights = master.clone();
        weights.gekisou_luck_bonus_lots.iter_mut().filter(|row| row.id >= 100).for_each(|row| row.weight = 3);
        let cases: [(&Master, &GekisouSetup); 4] =
            [(&master, &setup), (&master, &later), (&chance, &setup), (&weights, &setup)];
        let mut cache = LuckDpCache::new(1 << 24);
        for (i, &(master, setup)) in cases.iter().enumerate() {
            let skills = luck_skills(master).unwrap();
            let expected =
                luck_rush_dp_certified(master, &skills, &notes, params, setup, &play, &delta, &deck, None).unwrap();
            let cached =
                cache.certified(master, &skills, &notes, &[], params, setup, &play, &delta, &deck, None, None).unwrap();
            assert_eq!(curve_words(&cached), curve_words(&expected), "case {i}");
            assert_eq!(cache.stats().hits, 0, "case {i}");
        }
        assert_eq!(cache.len(), cases.len());
    }

    #[test]
    fn curve_cache_capacity_bounds_the_held_keys() {
        let (master, notes, params, setup, play, delta) = random_fixture();
        let skills = luck_skills(&master).unwrap();
        let decks: Vec<_> = [(2, vec![(31, 1), (96, 1)]), (2, vec![(31, 1)]), (3, vec![(31, 1)])]
            .into_iter()
            .map(|(skill, supports)| {
                [Performer { gekisou_skill: Some((skill, 1)), gekisou_support_skills: supports, ..Default::default() }]
            })
            .collect();
        let mut empty = LuckDpCache::new(0);
        for deck in &decks {
            let expected =
                luck_rush_dp_certified(&master, &skills, &notes, params, &setup, &play, &delta, deck, None).unwrap();
            let cached = empty
                .certified(&master, &skills, &notes, &[], params, &setup, &play, &delta, deck, None, None)
                .unwrap();
            assert_eq!(curve_words(&cached), curve_words(&expected));
        }
        assert!(empty.is_empty() && empty.stats().hits == 0);
        // Room for the first key only: a later key replaces it or, when larger, is not held.
        let mut one = LuckDpCache::new(usize::MAX);
        one.certified(&master, &skills, &notes, &[], params, &setup, &play, &delta, &decks[0], None, None).unwrap();
        let capacity = one.stats().peak_key_bytes;
        assert!(capacity > 0);
        let mut small = LuckDpCache::new(capacity);
        for deck in &decks {
            small.certified(&master, &skills, &notes, &[], params, &setup, &play, &delta, deck, None, None).unwrap();
            assert_eq!(small.len(), 1);
            assert!(small.stats().peak_key_bytes <= capacity);
        }
    }
}
