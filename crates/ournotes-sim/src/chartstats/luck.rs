//! The LUCK table of a chart: the lottery probabilities ([`full::luck_rush_samples`]) at every judged note time of a
//! deck without luck chain skills, and of the decks with one luck chain skill at one level, formation variant and
//! performance position.
//!
//! The lottery-related skills come from the master ([`full::luck_skills`]): every luck chain skill is an entry, and
//! every lottery-dependent score-up shape has a probe, a skill with a score-up of that shape, so the probabilities
//! also say when a score-up of each shape runs. Each table deck holds the probes on the positions the entry's skill
//! leaves free (in batches when there are more shapes than free positions; probes feed no lottery, so every batch
//! samples the same rush). A Gekisou support skill plays only beside a Gekisou skill, so its holder takes the neutral
//! Gekisou skill: the first one with no lottery-related row. A holder's band, character, card type and tag match the
//! formation targets of its skill exactly when its key is matched ([`full::luck_holder`]).

use serde::Serialize;

#[cfg(feature = "search-diagnostics")]
use super::Live;
use crate::error::Error;
use crate::live::full::{
    self, GekisouSetup, LiveNote, LiveParams, LivePlay, LuckShapeProbe, LuckSkillKey, LuckSkills, LuckSource, Performer,
};
#[cfg(feature = "search-diagnostics")]
use crate::live::seeds::seed_candidate;
use crate::master::Master;

/// The model line of the document.
#[cfg(feature = "search-diagnostics")]
pub const MODEL: &str = "experimental single-skill samples, not a composable team expectation: per chart with a luck range, the probabilities [r, s_0, rs_0, s_1, rs_1, ...] at every \
    judged note time as steps over `runs` lives of the LUCK-only replay (lottery, Gekisou skills and conditions \
    native; live and support skills dropped): r that the rush runs, s_j that a score-up of lottery-dependent shape j \
    runs, rs_j that both do; base for a deck without luck chain skills, entries for a deck with one (skill, level, \
    formation match, position); a deck's note score-up is u (1 + r (l - 1)) + sum_j v_j (s_j + rs_j (l - 1)), u the \
    score-up without the lottery-dependent ones, v_j the deck's score-up of shape j, l the full rush luck factor";
/// Default number of lives a table entry samples.
pub const LUCK_RUNS: usize = 4096;
/// Performers of a table deck.
const DECK: usize = 5;

/// What the LUCK table measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LuckOptions {
    /// Lives per entry (base seeds [`crate::live::seeds::seed_candidate`] `0..runs`).
    pub runs: usize,
    /// Every performance position of each skill; else position 0 only.
    pub all_positions: bool,
}

impl Default for LuckOptions {
    fn default() -> LuckOptions {
        LuckOptions { runs: LUCK_RUNS, all_positions: false }
    }
}

/// The lottery probabilities of a deck: steps `(time ms, [r, s_0, rs_0, ...])`.
pub type LuckSteps = Vec<(i32, Vec<f32>)>;

/// One entry: the deck with `skill` at performance position `position`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckEntry {
    pub skill: LuckSkillKey,
    pub position: usize,
    pub steps: LuckSteps,
}

/// The LUCK table of a chart.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckTable {
    pub runs: usize,
    /// The lottery-dependent score-up shapes and their probes, in column order.
    pub shapes: Vec<LuckShapeProbe>,
    /// The neutral Gekisou skill `(id, level)` of Gekisou support skill holders.
    pub neutral: Option<(i64, i64)>,
    /// The deck with no luck chain skill.
    pub base: LuckSteps,
    pub entries: Vec<LuckEntry>,
}

/// The first Gekisou skill `(id, level)` with no lottery-related row: the Gekisou skill of the Gekisou support skill
/// holders of a table deck.
pub fn luck_neutral(master: &Master, skills: &LuckSkills) -> Option<(i64, i64)> {
    let related = |id: i64, level: i64| {
        skills.chain.iter().any(|k| k.source == LuckSource::Gekisou && k.id == id && k.level == level)
            || master
                .gekisou_skill_effects
                .iter()
                .any(|r| r.skill_id == id && r.level == level && skills.rows.contains_key(&(LuckSource::Gekisou, r.id)))
    };
    let mut keys: Vec<_> = master.gekisou_skill_effects.iter().map(|r| (r.skill_id, r.level)).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter().find(|&(id, level)| !related(id, level))
}

/// A holder of skill keys, holding them: at most one Gekisou skill (else the neutral one) and at most two Gekisou
/// support skills, with the attributes of every matched key; an error when the keys' formation matches conflict.
fn holder(master: &Master, keys: &[LuckSkillKey], neutral: Option<(i64, i64)>) -> Result<Performer, Error> {
    let mut p = Performer::default();
    for &key in keys {
        let h = full::luck_holder(master, key)?;
        if h.band_id != 0 {
            p.band_id = h.band_id;
        }
        if h.character_id != 0 {
            p.character_id = h.character_id;
        }
        if h.card_type != 0 {
            p.card_type = h.card_type;
        }
        p.tag_ids.extend(h.tag_ids);
        match key.source {
            LuckSource::Gekisou if p.gekisou_skill.is_none() => p.gekisou_skill = Some((key.id, key.level)),
            LuckSource::Gekisou => return Err(Error::Input("a LUCK table holder with two Gekisou skills".into())),
            LuckSource::GekisouSupport => p.gekisou_support_skills.push((key.id, key.level)),
        }
    }
    if p.gekisou_support_skills.len() > 2 {
        return Err(Error::Input("a LUCK table holder with more than two Gekisou support skills".into()));
    }
    if p.gekisou_skill.is_none() {
        p.gekisou_skill =
            Some(neutral.ok_or_else(|| Error::Master("no Gekisou skill without lottery-related rows".into()))?);
    }
    for &key in keys {
        if full::luck_skill_key(master, key.source, key.id, key.level, &p)? != key {
            return Err(Error::Input(format!("LUCK table keys with conflicting formation matches: {key:?}")));
        }
    }
    Ok(p)
}

/// The value of a step function at `t` (`None` before its first step).
fn at(steps: &LuckSteps, t: i32) -> Option<&[f32]> {
    let i = steps.partition_point(|s| s.0 <= t);
    i.checked_sub(1).map(|i| steps[i].1.as_slice())
}

struct LuckProbeBatch {
    deck: Vec<Performer>,
    probes: Vec<Option<usize>>,
}

/// Shared synthetic decks for nominal and certified DP. Every shape gets exactly one designated holder.
fn luck_probe_batches(
    master: &Master,
    skills: &LuckSkills,
    neutral: Option<(i64, i64)>,
    entries: &[(LuckSkillKey, usize)],
) -> Result<Vec<LuckProbeBatch>, Error> {
    let mut held: [Vec<LuckSkillKey>; DECK] = Default::default();
    for &(key, k) in entries {
        held.get_mut(k).ok_or_else(|| Error::Input(format!("a LUCK DP position is below {DECK}")))?.push(key);
    }
    let free: Vec<_> = (0..DECK).filter(|&k| held[k].is_empty()).collect();
    if free.is_empty() && !skills.shapes.is_empty() {
        return Err(Error::Input("a LUCK DP probe deck needs a free position".into()));
    }
    let mut deck = vec![Performer::default(); DECK];
    for (k, keys) in held.iter().enumerate() {
        if !keys.is_empty() {
            deck[k] = holder(master, keys, neutral)?;
        }
    }
    let mut batches = Vec::new();
    let mut first = 0;
    loop {
        let mut deck = deck.clone();
        let mut probes = vec![None; skills.shapes.len()];
        for (j, &k) in (first..skills.shapes.len()).zip(&free) {
            deck[k] = holder(master, &[skills.shapes[j].probe], neutral)?;
            probes[j] = Some(k);
        }
        batches.push(LuckProbeBatch { deck, probes });
        first += free.len();
        if first >= skills.shapes.len() {
            break;
        }
    }
    Ok(batches)
}

/// Nominal independent-draw DP of the same probe decks as [`luck_table_steps`]. State counts are the maximum
/// over probe batches; transitions are summed. Unknown lottery mechanisms are refused by the DP compiler.
#[allow(clippy::too_many_arguments)]
pub fn luck_table_dp(
    master: &Master,
    skills: &LuckSkills,
    neutral: Option<(i64, i64)>,
    notes: &[LiveNote],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    entries: &[(LuckSkillKey, usize)],
) -> Result<full::LuckDpResult, Error> {
    let n_shapes = skills.shapes.len();
    let mut batches = Vec::new();
    let (mut peak_states, mut transitions) = (0usize, 0u64);
    for LuckProbeBatch { deck, probes } in luck_probe_batches(master, skills, neutral, entries)? {
        let batch = full::luck_rush_dp(master, skills, notes, params, setup, play, delta_times, &deck, Some(&probes))?;
        peak_states = peak_states.max(batch.peak_states);
        transitions = transitions
            .checked_add(batch.transitions)
            .ok_or_else(|| Error::Capacity("LUCK DP transition count overflow".into()))?;
        batches.push((probes, batch.steps));
    }
    let mut times: Vec<_> = batches.iter().flat_map(|(_, steps)| steps).map(|s| s.0).collect();
    times.sort_unstable();
    times.dedup();
    let mut steps: LuckSteps = Vec::new();
    for t in times {
        let mut p = vec![0f32; 1 + 2 * n_shapes];
        for (b, (probes, batch)) in batches.iter().enumerate() {
            let Some(v) = at(batch, t) else { continue };
            if b == 0 {
                p[0] = v[0];
            } else if v[0] != p[0] {
                return Err(Error::Unsupported(format!("LUCK DP: a probe changes the rush at {t} ms")));
            }
            for (j, member) in probes.iter().enumerate() {
                if member.is_some() {
                    p[1 + 2 * j] = v[1 + 2 * j];
                    p[2 + 2 * j] = v[2 + 2 * j];
                }
            }
        }
        if steps.last().is_none_or(|last| last.1 != p) {
            steps.push((t, p));
        }
    }
    Ok(full::LuckDpResult { steps, peak_states, transitions })
}

/// Join batches only after the DP compiler has established that every held score probe uses the same
/// supported direct 7021 predicate. Equality includes all four directly accumulated joint buckets, not
/// just Rush marginals. Keep one curve verbatim: averaging or intersecting batch bounds is unnecessary.
fn merge_certified_batch(
    combined: &mut Option<full::LuckDpCertifiedResult>,
    batch: full::LuckDpCertifiedResult,
) -> Result<(), Error> {
    let Some(result) = combined else {
        *combined = Some(batch);
        return Ok(());
    };
    if result.steps != batch.steps {
        return Err(Error::Unsupported("LUCK certified DP: probe batches have different joint curves".into()));
    }
    if result.probes.len() != batch.probes.len() {
        return Err(Error::Input("LUCK certified DP: probe batch widths differ".into()));
    }
    for (held, additional) in result.probes.iter_mut().zip(batch.probes) {
        if *held && additional {
            return Err(Error::Input("LUCK certified DP: a shape is probed twice".into()));
        }
        *held |= additional;
    }
    result.peak_states = result.peak_states.max(batch.peak_states);
    result.transitions = result
        .transitions
        .checked_add(batch.transitions)
        .ok_or_else(|| Error::Capacity("LUCK certified DP transition count overflow".into()))?;
    Ok(())
}

/// Outward probability bounds for the same synthetic decks as [`luck_table_dp`]. The four joint buckets
/// are [neither, score only, Rush only, Rush and score], shared by the supported direct 7021 score probes.
/// Every batch must compile to that predicate and return the identical direct joint curve. This certifies
/// only lottery probabilities under independent nominal draws, not whole-score paths or finite-seed means.
#[allow(clippy::too_many_arguments)]
pub fn luck_table_dp_certified(
    master: &Master,
    skills: &LuckSkills,
    neutral: Option<(i64, i64)>,
    notes: &[LiveNote],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    entries: &[(LuckSkillKey, usize)],
) -> Result<full::LuckDpCertifiedResult, Error> {
    let mut combined = None;
    for LuckProbeBatch { deck, probes } in luck_probe_batches(master, skills, neutral, entries)? {
        let batch = full::luck_rush_dp_certified(
            master,
            skills,
            notes,
            params,
            setup,
            play,
            delta_times,
            &deck,
            Some(&probes),
        )?;
        if !batch.probes.iter().copied().eq(probes.iter().map(Option::is_some)) {
            return Err(Error::Input("LUCK certified DP: a probe batch did not retain its designated holders".into()));
        }
        merge_certified_batch(&mut combined, batch)?;
    }
    let result = combined.expect("at least the base probe deck");
    if result.probes.iter().any(|held| !held) {
        return Err(Error::Input("LUCK certified DP: a score shape has no verified probe".into()));
    }
    Ok(result)
}

/// The lottery probabilities of a table deck over one live per base seed of `seeds`: five performers, the skills of
/// `entries` at their positions (one holder per position, [`holder`]) and the probes of `skills` on the other
/// positions, in batches when there are more shapes than free positions. `neutral` is [`luck_neutral`].
#[allow(clippy::too_many_arguments)]
pub fn luck_table_steps(
    master: &Master,
    skills: &LuckSkills,
    neutral: Option<(i64, i64)>,
    notes: &[LiveNote],
    params: LiveParams,
    setup: &GekisouSetup,
    play: &LivePlay,
    delta_times: &[f32],
    entries: &[(LuckSkillKey, usize)],
    seeds: &[i32],
) -> Result<LuckSteps, Error> {
    let n_shapes = skills.shapes.len();
    let width = 1 + 2 * n_shapes;
    let mut held: [Vec<LuckSkillKey>; DECK] = Default::default();
    for &(key, k) in entries {
        held.get_mut(k).ok_or_else(|| Error::Input(format!("a LUCK table position is below {DECK}")))?.push(key);
    }
    let free: Vec<usize> = (0..DECK).filter(|&k| held[k].is_empty()).collect();
    if free.is_empty() && n_shapes > 0 {
        return Err(Error::Input("a LUCK table deck needs a free position for the probes".into()));
    }
    let mut batches: Vec<LuckSteps> = Vec::new();
    let mut first = 0;
    loop {
        let mut deck = vec![Performer::default(); DECK];
        for (k, keys) in held.iter().enumerate() {
            if !keys.is_empty() {
                deck[k] = holder(master, keys, neutral)?;
            }
        }
        let mut probes = vec![None; n_shapes];
        for (j, &k) in (first..n_shapes).zip(&free) {
            deck[k] = holder(master, &[skills.shapes[j].probe], neutral)?;
            probes[j] = Some(k);
        }
        batches.push(full::luck_rush_samples(
            master,
            skills,
            notes,
            params,
            setup,
            play,
            delta_times,
            &deck,
            Some(&probes),
            seeds,
        )?);
        first += free.len();
        if first >= n_shapes {
            break;
        }
    }
    if batches.len() == 1 {
        return Ok(batches.pop().unwrap_or_default());
    }
    let mut times: Vec<i32> = batches.iter().flatten().map(|s| s.0).collect();
    times.sort_unstable();
    times.dedup();
    let mut out: LuckSteps = Vec::new();
    for t in times {
        let mut p = vec![0f32; width];
        for (b, steps) in batches.iter().enumerate() {
            let Some(v) = at(steps, t) else { continue };
            if b == 0 {
                p[0] = v[0];
            } else if v[0] != p[0] {
                return Err(Error::Unsupported(format!("LUCK table: a probe changes the rush at {t} ms")));
            }
            let lo = b * free.len();
            for j in lo..(lo + free.len()).min(n_shapes) {
                p[1 + 2 * j] = v[1 + 2 * j];
                p[2 + 2 * j] = v[2 + 2 * j];
            }
        }
        if out.last().is_none_or(|last| last.1 != p) {
            out.push((t, p));
        }
    }
    Ok(out)
}

/// The transform the table entries compose in: `f_a(x) = ((1 - x)^-a - 1) / a` (`a = 0`: `-ln(1 - x)`), increasing
/// and convex, so increments that push a probability towards 1 compose sub-additively.
fn compose_f(a: f64, x: f64) -> f64 {
    let x = x.min(1.0 - 1e-9);
    if a == 0.0 { -(1.0 - x).ln() } else { ((1.0 - x).powf(-a) - 1.0) / a }
}

fn compose_inv(a: f64, y: f64) -> f64 {
    if a == 0.0 {
        1.0 - (-y).exp()
    } else {
        let v = 1.0 + a * y;
        if v > 0.0 { 1.0 - v.powf(-1.0 / a) } else { 0.0 }
    }
}

/// The lottery probabilities of a deck from a table: each probability `x` at a time composes the base `b` and the
/// deck's entries `e_i` as `f_a^-1(f_a(b) + sum_i (f_a(e_i) - f_a(b)))` ([`compose_f`]), clamped to `[0, 1]`, as
/// steps over the union of the step times.
pub fn luck_compose(base: &LuckSteps, entries: &[&LuckSteps], a: f64) -> LuckSteps {
    let mut times: Vec<i32> = base.iter().chain(entries.iter().copied().flatten()).map(|s| s.0).collect();
    times.sort_unstable();
    times.dedup();
    let mut out: LuckSteps = Vec::new();
    for t in times {
        let Some(b) = at(base, t) else { continue };
        let mut p = Vec::with_capacity(b.len());
        for (j, &bj) in b.iter().enumerate() {
            let fb = compose_f(a, f64::from(bj));
            let mut y = fb;
            for e in entries {
                if let Some(v) = at(e, t) {
                    y += compose_f(a, f64::from(v[j])) - fb;
                }
            }
            p.push(compose_inv(a, y).clamp(0.0, 1.0) as f32);
        }
        if out.last().is_none_or(|last| last.1 != p) {
            out.push((t, p));
        }
    }
    out
}

/// A diagnostic luck table of one chart by sampling; the chart-stats document does not include it.
#[cfg(feature = "search-diagnostics")]
pub fn diagnostic_luck_table(
    master: &Master,
    chart: &crate::data::DataChart,
    options: &LuckOptions,
) -> Result<Option<LuckTable>, Error> {
    use crate::live::model::{JudgementStream, JustRule};
    use crate::live::score::LiveScoreSettings;
    let settings = LiveScoreSettings::from_master(master)?;
    let c = chart.chart(&settings)?;
    if chart.fevers.len() > super::MAX_GEKISOU_FEVERS {
        return Ok(None);
    }
    let (music_id, _) = super::song_of_score(master, chart.score_id)?;
    let resolved = crate::scenario::Scenario::Free(music_id).resolve(master)?;
    let setup = resolved.gekisou_setup(&chart.fevers);
    let rule = JustRule::new(master, &setup)?;
    let stream = JudgementStream::theoretical_best_gekisou(&c, &chart.judgement_types, &rule)?;
    let play = stream.to_live_play()?;
    let notes: Vec<_> = c
        .notes
        .iter()
        .zip(&chart.judgement_types)
        .map(|(note, &judgement_type)| LiveNote {
            note_id: note.id,
            time_ms: note.time_ms,
            note_operate_type: note.note_type,
            judgement_type,
        })
        .collect();
    let events: Vec<_> = c.skill_events.iter().map(|e| (e.index, e.time_ms)).collect();
    let level = master
        .live_music_score(chart.score_id)
        .ok_or_else(|| Error::Input(format!("chart {}: missing score row", chart.score_id)))?
        .music_score_level as i32;
    let live = Live {
        master,
        measure: master,
        notes: &notes,
        events: &events,
        params: LiveParams {
            skill_target_music_type: resolved.skill_target_music_type,
            total_power: super::POWER,
            music_level: level,
            converted_note_count: c.converted_note_count,
            music_length_ms: c.last_timing_note_ms.wrapping_add(1000),
            score_music_length_ms: None,
            assist_factor: 1.0,
        },
        gekisou: Some(super::Gekisou { setup, dt: stream.delta_times()?, perfect: play.clone() }),
        play,
        positions: 5,
    };
    live.luck_table(options)
}

#[cfg(feature = "search-diagnostics")]
impl Live<'_> {
    /// The LUCK table of the chart; `None` without a luck range.
    pub(super) fn luck_table(&self, options: &LuckOptions) -> Result<Option<LuckTable>, Error> {
        let Some(g) = &self.gekisou else { return Ok(None) };
        if !g.setup.missions.iter().take(g.setup.fevers.len()).any(|&m| m == super::MISSION_LUCK) {
            return Ok(None);
        }
        if options.runs == 0 {
            return Err(Error::Input("a LUCK table needs at least one run".into()));
        }
        let master = self.master;
        let skills = full::luck_skills(master)?;
        let neutral = luck_neutral(master, &skills);
        let seeds: Vec<i32> = (0..options.runs as u64).map(seed_candidate).collect();
        let sample = |entries: &[(LuckSkillKey, usize)]| {
            luck_table_steps(
                master,
                &skills,
                neutral,
                self.notes,
                self.params,
                &g.setup,
                &self.play,
                &g.dt,
                entries,
                &seeds,
            )
        };
        let base = sample(&[])?;
        let positions = if options.all_positions { DECK } else { 1 };
        let mut entries = Vec::new();
        for &skill in &skills.chain {
            for position in 0..positions {
                entries.push(LuckEntry { skill, position, steps: sample(&[(skill, position)])? });
            }
        }
        Ok(Some(LuckTable { runs: options.runs, shapes: skills.shapes, neutral, base, entries }))
    }
}

#[cfg(test)]
mod certified_tests {
    use super::*;
    use crate::live::certified::ProbabilityMass;

    fn batch(probes: Vec<bool>, bucket: usize, peak_states: usize, transitions: u64) -> full::LuckDpCertifiedResult {
        let mut joint = [ProbabilityMass::ZERO; 4];
        joint[bucket] = ProbabilityMass::ONE;
        full::LuckDpCertifiedResult {
            steps: vec![(100, joint)],
            probes,
            range_moments: Vec::new(),
            peak_states,
            transitions,
        }
    }

    #[test]
    fn certified_batches_keep_one_identical_joint_curve_and_all_probes() {
        let first = batch(vec![true, false], 3, 5, 7);
        let expected = first.steps.clone();
        let mut combined = Some(first);
        merge_certified_batch(&mut combined, batch(vec![false, true], 3, 3, 11)).unwrap();
        let result = combined.unwrap();
        assert_eq!(result.steps, expected);
        assert_eq!(result.probes, vec![true, true]);
        assert_eq!(result.peak_states, 5);
        assert_eq!(result.transitions, 18);
    }

    #[test]
    fn certified_batches_reject_different_joint_curves_even_with_the_same_rush() {
        let mut combined = Some(batch(vec![true, false], 2, 1, 1));
        let error = merge_certified_batch(&mut combined, batch(vec![false, true], 3, 1, 1)).unwrap_err();
        assert!(matches!(error, Error::Unsupported(_)));
    }

    #[test]
    fn certified_batches_reject_duplicate_probes_and_transition_overflow() {
        let mut duplicate = Some(batch(vec![true], 0, 1, 1));
        assert!(matches!(merge_certified_batch(&mut duplicate, batch(vec![true], 0, 1, 1)), Err(Error::Input(_))));
        let mut overflow = Some(batch(vec![true, false], 0, 1, u64::MAX));
        assert!(matches!(
            merge_certified_batch(&mut overflow, batch(vec![false, true], 0, 1, 1)),
            Err(Error::Capacity(_))
        ));
    }
}
