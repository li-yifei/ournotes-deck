//! Warm start and visit order of the joint member/Snap search.
//!
//! Nothing here removes a deck from the search. The warm start proposes legal decks of the searched domain and
//! evaluates them with `Engine::consider_with`, the leaf evaluation of the traversal: the same simulator, performance
//! orders, Top-K order with its power and public-ID ties, cutoff tables and budget. Decks are teams: a proposal is
//! compared and remembered in its canonical layout (`uniform::canonical`). It only fills the Top-K earlier and
//! raises its cutoff. The traversal still visits or proves away every deck; a deck the warm start evaluated is a
//! cache hit when the traversal reaches it, which is exact because the Top-K only improves: a deck left outside
//! the Top-K is preceded by K decks, and every later Top-K entry precedes the earlier K-th.
//!
//! The visit order changes only the order of the root children (leader pairs) and of the conversion parts of a
//! Gekisou score search; every root child and every part is still searched. Root children are visited in
//! descending order of the bound their depth-1 node checks, so once one is strictly inferior to the cutoff, so is
//! every later one, and the loop stops where each of them would have been pruned at depth 1. The choice order
//! below the root is the static `JointBounds::choices`, on which the suffix-maximum tail bound relies.
use super::{Engine, Error, PhysicalDeck, slot, uniform};
use crate::{
    clock::Instant,
    domain::CandidateDomain,
    search::joint::{JointBounds, SLOTS},
};
use std::collections::HashSet;

/// Greedy dives, each from one of the best leader pairs by root bound (distinct leader members).
const DIVES: usize = 3;
/// Leaf-bound evaluations of the local search over all dives.
const SURROGATE_CHECKS: u64 = 4_000;
/// Best distinct decks by leaf bound that are evaluated exactly.
const POOL: usize = 16;
/// Exact evaluations of the neighbourhood of the best deck, per polishing round.
const POLISH: usize = 8;
/// Polishing rounds after a new best deck found by the traversal, and at the end of the warm start; a round runs
/// only when the previous one improved the best deck.
const POLISH_ROUNDS: usize = 4;
const SEED_POLISH_ROUNDS: usize = 8;
/// Pair moves per slot: the first legal (member, Snap) pairs of the static choice order.
const PAIR_MOVES: usize = 64;

/// Whether the conversion parts and the root children keep their static order (the validation ablation
/// `STATIC_ORDER`).
pub(super) fn static_order() -> bool {
    crate::search::snaps::ablated(crate::search::ablate::STATIC_ORDER)
}

/// Whole-domain context of the warm start and of polishing: the searched domain with its whole-domain bounds, which
/// hold for every deck of the domain (a conversion part's or a restricted PT domain's bounds do not).
pub(super) struct Warm<'a> {
    domain: &'a CandidateDomain,
    bounds: &'a JointBounds,
    orders: Vec<([usize; 5], u128)>,
}

impl<'a> Warm<'a> {
    /// None under the validation ablation `NO_WARM_START` (no warm start, no polishing).
    pub(super) fn new(
        domain: &'a CandidateDomain,
        bounds: &'a JointBounds,
        orders: &[([usize; 5], u128)],
    ) -> Option<Self> {
        let off = crate::search::snaps::ablated(crate::search::ablate::NO_WARM_START);
        (!off).then(|| Self { domain, bounds, orders: orders.to_vec() })
    }
}

fn snap_of(domain: &CandidateDomain, choice: usize) -> Option<usize> {
    (choice > 0).then(|| domain.snaps()[choice - 1])
}

/// Root children of one traversal in descending order of their depth-1 node bound, with that bound.
pub(super) struct RootOrder {
    pub(super) children: Vec<(usize, usize)>,
    pub(super) caps: Vec<(i128, i64)>,
}

impl RootOrder {
    /// The leader pairs the root loop would visit (same filters), sorted by the bound their depth-1 node checks
    /// first; a stable sort keeps the static order among equal bounds.
    pub(super) fn new(
        domain: &CandidateDomain,
        bounds: &JointBounds,
        orders: &[([usize; 5], u128)],
        e: &Engine<'_, '_>,
    ) -> Result<Self, Error> {
        let mut p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
        let mut rows = Vec::new();
        for &(m, choice) in &bounds.choices {
            let character = e.pool.members[m].character_id;
            if domain.leader().is_some_and(|l| l != m)
                || domain.required().iter().any(|&r| r != m && e.pool.members[r].character_id == character)
                || !bounds.allows(SLOTS[0], choice)
            {
                continue;
            }
            p.members[SLOTS[0]] = m;
            p.snaps[SLOTS[0]] = snap_of(domain, choice);
            rows.push(((m, choice), super::node_upper(e.pool, domain, bounds, &p, 1, orders)?));
        }
        rows.sort_by_key(|a| std::cmp::Reverse(a.1));
        let (children, caps) = rows.into_iter().unzip();
        Ok(Self { children, caps })
    }

    pub(super) fn best(&self) -> Option<(i128, i64)> {
        self.caps.first().copied()
    }
}

/// The traversal of `joint_regimes` holding `d`: (part, slot rules) for no converting Snap (0, 0), one converting Snap
/// `converting[c]` in `SLOTS[k]` (1 + c, k), or two or more whose first two `SLOTS` positions are i < j
/// (1 + converting.len(), j(j-1)/2 + i), the order in which `joint_regimes` builds them.
pub(super) fn traversal_of(d: &PhysicalDeck, converting: &[usize]) -> (usize, usize) {
    let held: Vec<usize> = (0..5).filter(|&k| d.snaps[SLOTS[k]].is_some_and(|s| converting.contains(&s))).collect();
    match held[..] {
        [] => (0, 0),
        [k] => {
            let snap = d.snaps[SLOTS[k]].expect("converting Snap");
            (1 + converting.iter().position(|&c| c == snap).expect("converting Snap"), k)
        }
        [i, j, ..] => (1 + converting.len(), j * (j - 1) / 2 + i),
    }
}

/// Whether (payoff cap, power cap) is strictly inferior to a full Top-K's cutoff (the traversal's prune rule).
pub(super) fn inferior(cap: (i128, i64), e: &Engine<'_, '_>) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if crate::parallel::inferior(cap.0, cap.1) {
        return true;
    }
    let Some((threshold, power)) = e.safe_cutoff() else { return false };
    cap.0 < threshold || (cap.0 == threshold && cap.1 < i64::from(power))
}

/// Leaf bound of a full deck, the local search objective: the position-mean cheap relaxation, an upper bound of the
/// team's payoff and of its power.
fn surrogate(w: &Warm<'_>, p: &PhysicalDeck, e: &mut Engine<'_, '_>) -> Result<(i128, i64), Error> {
    e.tel.incumbents.warm_start.leaf_bound_checks += 1;
    w.bounds.expected_upper(e.pool, w.domain, p, 5, &w.orders)
}

/// Legal teams one move away: another member in one slot (Snap kept), another Snap or None in one slot (member
/// kept), one of the first `PAIR_MOVES` other pairs of the static choice order in one slot, or the leader exchanging
/// its member/Snap pair with another slot's (other layouts of a team are the same team). Required members stay and
/// a fixed leader stays in slot 2.
fn neighbours(w: &Warm<'_>, d: &PhysicalDeck, e: &Engine<'_, '_>) -> Vec<PhysicalDeck> {
    let (pool, domain) = (e.pool, w.domain);
    let fixed_leader = domain.leader().is_some();
    let mut out = Vec::new();
    for slot in 0..5 {
        let replaceable = !((slot == SLOTS[0] && fixed_leader) || domain.required().contains(&d.members[slot]));
        if replaceable {
            let mut pairs = 0;
            for &(m, choice) in &w.bounds.choices {
                if pairs == PAIR_MOVES {
                    break;
                }
                let character = pool.members[m].character_id;
                let s = snap_of(domain, choice);
                if m == d.members[slot]
                    || s == d.snaps[slot]
                    || (0..5).any(|t| t != slot && pool.members[d.members[t]].character_id == character)
                    || (s.is_some() && (0..5).any(|t| t != slot && d.snaps[t] == s))
                {
                    continue;
                }
                let mut n = *d;
                n.members[slot] = m;
                n.snaps[slot] = s;
                out.push(n);
                pairs += 1;
            }
        }
        if replaceable {
            for &m in domain.members() {
                let character = pool.members[m].character_id;
                if (0..5).any(|s| s != slot && pool.members[d.members[s]].character_id == character)
                    || m == d.members[slot]
                {
                    continue;
                }
                let mut n = *d;
                n.members[slot] = m;
                out.push(n);
            }
        }
        for choice in 0..=domain.snaps().len() {
            let s = snap_of(domain, choice);
            if s == d.snaps[slot] || (s.is_some() && (0..5).any(|t| t != slot && d.snaps[t] == s)) {
                continue;
            }
            let mut n = *d;
            n.snaps[slot] = s;
            out.push(n);
        }
    }
    if !fixed_leader {
        for other in uniform::NONLEADER {
            let mut n = *d;
            n.members.swap(SLOTS[0], other);
            n.snaps.swap(SLOTS[0], other);
            out.push(n);
        }
    }
    out.iter_mut().for_each(|n| *n = uniform::canonical(pool, n));
    out
}

/// The best distinct decks by leaf bound seen by the local search.
struct Shortlist {
    rows: Vec<((i128, i64), PhysicalDeck)>,
    seen: HashSet<PhysicalDeck>,
}
impl Shortlist {
    fn offer(&mut self, value: (i128, i64), d: PhysicalDeck) {
        if !self.seen.insert(d) {
            return;
        }
        let at = self.rows.partition_point(|r| r.0 >= value);
        if at < POOL {
            self.rows.insert(at, (value, d));
            self.rows.truncate(POOL);
        }
    }
}

/// Greedy dive from a leader pair: each further slot (in `SLOTS` order) takes the legal pair with the highest
/// depth-(d+1) node bound; once the missing required members fill the remaining slots, only they are legal.
fn dive(w: &Warm<'_>, leader: (usize, usize), e: &mut Engine<'_, '_>) -> Result<Option<PhysicalDeck>, Error> {
    let (pool, domain, bounds) = (e.pool, w.domain, w.bounds);
    let mut p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
    p.members[SLOTS[0]] = leader.0;
    p.snaps[SLOTS[0]] = snap_of(domain, leader.1);
    for depth in 1..5 {
        let slot = SLOTS[depth];
        let missing: Vec<usize> =
            domain.required().iter().copied().filter(|r| !SLOTS[..depth].iter().any(|&s| p.members[s] == *r)).collect();
        if missing.len() > 5 - depth {
            return Ok(None);
        }
        let mut best: Option<((i128, i64), usize, usize)> = None;
        for &(m, choice) in &bounds.choices {
            let character = pool.members[m].character_id;
            let snap = snap_of(domain, choice);
            if SLOTS[..depth].iter().any(|&s| pool.members[p.members[s]].character_id == character)
                || domain.required().iter().any(|&r| r != m && pool.members[r].character_id == character)
                || (missing.len() == 5 - depth && !missing.contains(&m))
                || (snap.is_some() && SLOTS[..depth].iter().any(|&s| p.snaps[s] == snap))
            {
                continue;
            }
            p.members[slot] = m;
            p.snaps[slot] = snap;
            let value = bounds.expected_upper(pool, domain, &p, depth + 1, &w.orders)?;
            if best.is_none_or(|b| value > b.0) {
                best = Some((value, m, choice));
            }
        }
        let Some((_, m, choice)) = best else { return Ok(None) };
        p.members[slot] = m;
        p.snaps[slot] = snap_of(domain, choice);
    }
    Ok(Some(p))
}

/// Exactly evaluates `d` unless it was evaluated before or the traversal would prune it by its leaf bound `value`;
/// Ok(false) when the budget ran out.
fn evaluate(
    w: &Warm<'_>,
    value: (i128, i64),
    d: PhysicalDeck,
    polish: bool,
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    let d = uniform::canonical(e.pool, &d);
    if e.seeded.contains(&d) || e.top.iter().any(|t| t.physical == d) || inferior(value, e) {
        return Ok(true);
    }
    assert!(w.domain.check_fixed(e.pool, &d).is_ok(), "warm start proposed an illegal deck");
    if polish {
        e.tel.incumbents.warm_start.polish_evaluations += 1;
    } else {
        e.tel.incumbents.warm_start.evaluations += 1;
    }
    if !e.consider_with(d, Some((w.bounds, w.domain)))? {
        return Ok(false);
    }
    e.seeded.insert(d);
    Ok(true)
}

/// Polishing rounds from the current best deck: its neighbours in descending leaf-bound order, the first `POLISH`
/// of them evaluated exactly; another round only if the best deck changed. Ok(false) when the budget ran out.
fn polish_rounds(w: &Warm<'_>, rounds: usize, e: &mut Engine<'_, '_>) -> Result<bool, Error> {
    // Overlapping certified candidates have no exact best deck to polish from.
    if e.certified.is_some() {
        return Ok(true);
    }
    for _ in 0..rounds {
        let Some(best) = e.top.first().map(|t| t.physical) else { return Ok(true) };
        e.tel.incumbents.warm_start.polish_rounds += 1;
        let mut ranked = Vec::new();
        for n in neighbours(w, &best, e) {
            if !e.seeded.contains(&n) && !e.top.iter().any(|t| t.physical == n) {
                ranked.push((surrogate(w, &n, e)?, n));
            }
        }
        ranked.sort_by_key(|a| std::cmp::Reverse(a.0));
        for (value, d) in ranked.into_iter().take(POLISH) {
            if !evaluate(w, value, d, true, e)? {
                return Ok(false);
            }
        }
        if e.top.first().map(|t| t.physical) == Some(best) {
            break;
        }
    }
    Ok(true)
}

impl Engine<'_, '_> {
    /// After a strictly better best payoff: polishing rounds around the new best deck. Taking the context out keeps
    /// the evaluations inside from starting a nested polish.
    pub(super) fn polish(&mut self) -> Result<(), Error> {
        let Some(w) = self.warm.take() else { return Ok(()) };
        let started = Instant::now();
        let (_, resume) = self.rec.clock.lap(slot::WARM);
        let result = polish_rounds(&w, POLISH_ROUNDS, self);
        self.rec.clock.lap(resume);
        self.tel.incumbents.warm_start.polish_ms += started.elapsed().as_secs_f64() * 1000.0;
        self.warm = Some(w);
        result.map(|_| ())
    }
}

/// Warm start on the whole domain with its whole-domain bounds: greedy dives from the best leader pairs, a
/// best-improvement local search on the leaf bound, exact evaluation of the dive decks and of the best decks the
/// local search saw, then polishing rounds around the best deck. Certified seeding starts new proposals during
/// the first quarter of the remaining time for unbounded score targets. Bounded
/// targets use the complete request deadline for their finite proposal shortlist.
/// Certified requests allow POOL proposals for a domain with one empty Snap binding,
/// POOL for bounded score targets, and min(K, DIVES) otherwise.
/// The complete-domain traversal handles subsequent candidates.
pub(super) fn seed(e: &mut Engine<'_, '_>) -> Result<(), Error> {
    #[cfg(not(target_arch = "wasm32"))]
    if crate::parallel::has_cutoff() {
        return Ok(());
    }
    let Some(w) = e.warm.take() else { return Ok(()) };
    e.rec.begin(&mut e.tel, "seed", None);
    let (_, resume) = e.rec.clock.lap(slot::WARM);
    let seed_deadline =
        e.certified.as_ref().filter(|_| !bounded_target(e.metric)).and_then(|_| e.budget.deadline()).map(|deadline| {
            let now = crate::search::budget::now();
            now + deadline.saturating_duration_since(now) / 4
        });
    let result = seed_inner(&w, e, seed_deadline);
    e.rec.clock.lap(resume);
    e.rec.end(&mut e.tel);
    // This telemetry field is an exact incumbent value, not the interval route's conservative cutoff.
    e.tel.incumbents.warm_start.kth =
        e.safe_cutoff().filter(|_| e.certified.is_none()).map(|(threshold, _)| threshold.to_string());
    e.warm = Some(w);
    result
}

fn bounded_target(metric: &crate::types::Metric) -> bool {
    matches!(
        metric,
        crate::types::Metric::CappedScore { .. }
            | crate::types::Metric::ScoreAtLeast { .. }
            | crate::types::Metric::ScoreAndLifeAtLeast { .. }
    )
}

fn seed_inner(w: &Warm<'_>, e: &mut Engine<'_, '_>, deadline: Option<Instant>) -> Result<(), Error> {
    let initial_evaluations = e.tel.incumbents.warm_start.evaluations;
    let proposal_limit =
        if w.domain.snaps().is_empty() || bounded_target(e.metric) { POOL } else { e.request.k.min(DIVES) } as u64;
    let certified_seed_complete = |e: &Engine<'_, '_>| {
        e.certified.is_some()
            && e.tel.incumbents.warm_start.evaluations.saturating_sub(initial_evaluations) >= proposal_limit
    };
    let paused = |e: &mut Engine<'_, '_>| {
        let now = crate::search::budget::now();
        e.expired_at(now) || deadline.is_some_and(|deadline| now >= deadline)
    };
    if paused(e) {
        return Ok(());
    }
    let root = RootOrder::new(w.domain, w.bounds, &w.orders, e)?;
    let mut leaders = Vec::new();
    for &(m, choice) in &root.children {
        if leaders.len() == DIVES {
            break;
        }
        if !leaders.iter().any(|&(l, _)| l == m) {
            leaders.push((m, choice));
        }
    }
    let mut shortlist = Shortlist { rows: Vec::new(), seen: HashSet::new() };
    let mut checks = 0u64;
    for leader in leaders {
        #[cfg(not(target_arch = "wasm32"))]
        if crate::parallel::has_cutoff() {
            return Ok(());
        }
        if paused(e) {
            return Ok(());
        }
        let Some(current) = dive(w, leader, e)? else { continue };
        let mut current = uniform::canonical(e.pool, &current);
        let mut value = surrogate(w, &current, e)?;
        #[cfg(test)]
        crate::search::budget::test_clock::stage("seed-dive");
        if paused(e) || !evaluate(w, value, current, false, e)? {
            return Ok(());
        }
        #[cfg(test)]
        crate::search::budget::test_clock::stage("seed-evaluated");
        if certified_seed_complete(e) {
            return Ok(());
        }
        #[cfg(not(target_arch = "wasm32"))]
        if crate::parallel::has_cutoff() {
            return Ok(());
        }
        shortlist.offer(value, current);
        while checks < SURROGATE_CHECKS {
            if paused(e) {
                return Ok(());
            }
            let mut step: Option<((i128, i64), PhysicalDeck)> = None;
            for n in neighbours(w, &current, e) {
                if checks >= SURROGATE_CHECKS {
                    break;
                }
                checks += 1;
                if paused(e) {
                    return Ok(());
                }
                #[cfg(not(target_arch = "wasm32"))]
                if checks.is_multiple_of(64) && crate::parallel::has_cutoff() {
                    return Ok(());
                }
                let v = surrogate(w, &n, e)?;
                shortlist.offer(v, n);
                if v > value && step.is_none_or(|s| v > s.0) {
                    step = Some((v, n));
                }
            }
            let Some((v, n)) = step else { break };
            (value, current) = (v, n);
        }
    }
    for (value, d) in shortlist.rows.clone() {
        if certified_seed_complete(e) {
            return Ok(());
        }
        if paused(e) || !evaluate(w, value, d, false, e)? {
            return Ok(());
        }
    }
    if !paused(e) {
        polish_rounds(w, SEED_POLISH_ROUNDS, e)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::search::budget::{self, test_clock};
    use crate::search::gate_tests::common::{Rng, extend_table, replace_table, roster, set_column, synth_snaps};
    use crate::search::{Completion, Constraints, GekisouObjective, Objective, PlayInput, SearchRequest, SeedSet};
    use crate::types::{Limits, Metric, SimulationInput, Strategy};
    use ournotes_sim::live::model::JudgementStream;
    use ournotes_sim::live::score::LiveScoreSettings;
    use ournotes_sim::live::skip::{Chart, ChartNote};
    use ournotes_sim::pool::Pool;
    use ournotes_sim::scenario::{ContextInput, PowerSnapshotInput, Scenario};
    use serde_json::json;

    #[test]
    fn certified_seed_allocation_yields_to_the_complete_domain() {
        let mut source = synth_snaps(&mut Rng::new(91), 5, 0, &[]);
        set_column(&mut source, "MasterMemberCard", &mut |row| {
            row["_characterID"] = row["_id"].clone();
            row["_leaderSkillID"] = json!(4);
        });
        set_column(&mut source, "MasterLiveMusic", &mut |row| {
            row["_gekisouMission1"] = json!(2);
            row["_gekisouMission2"] = json!(3);
            row["_gekisouMission3"] = json!(1);
        });
        replace_table(&mut source, "MasterLiveSkillEffect", json!([]));
        extend_table(
            &mut source,
            "MasterLiveSettings",
            vec![
                json!({"_id":30,"_key":"gekisou_luck_gauge_max","_value":"40"}),
                json!({"_id":31,"_key":"gekisou_luck_gauge_max_rush","_value":"20"}),
                json!({"_id":32,"_key":"gekisou_luck_rush_score_bonus_percent","_value":"10"}),
            ],
        );
        replace_table(
            &mut source,
            "MasterLiveGekisouLuckBasePoint",
            json!([
                {"_id":1,"_noteCategory":0,"_noteSimulateJudgement":5,"_weight":1,"_basePoint":10}
            ]),
        );
        replace_table(
            &mut source,
            "MasterLiveGekisouLuckBonusLot",
            json!(
                (0..5)
                    .flat_map(|kind| (0..4).map(move |result| json!({
                        "_id":kind*10+result+1,"_chanceLotType":kind,"_lotResult":result,"_weight":1
                    })))
                    .collect::<Vec<_>>()
            ),
        );
        replace_table(
            &mut source,
            "MasterLiveGekisouRankingScoreBonus",
            json!(
                (1..=3)
                    .flat_map(|pattern| {
                        (1..=3).map(move |count| json!({
                "_id":pattern*10+count,"_missionPattern":pattern,"_count":count,"_rank":1,"_scoreBonusPercent":10
            }))
                    })
                    .collect::<Vec<_>>()
            ),
        );
        let master = source.master();
        let owned = roster(&mut Rng::new(92), &master);
        let pool = Pool::new(&master, &owned).unwrap();
        let chart = Chart::from_notes(
            vec![ChartNote { id: 1, time_ms: 100, note_type: 1 }],
            vec![],
            &LiveScoreSettings::from_master(&master).unwrap(),
        )
        .unwrap();
        let stream = JudgementStream::theoretical_best(&chart);
        let context = ContextInput {
            power_snapshot: PowerSnapshotInput { event_ids: vec![], captured_jst_ticks: None },
            result_clock: None,
            event_payoff: None,
        }
        .resolve(&master, Scenario::Mission(10), Some(1004), &[(50, 150)])
        .unwrap();
        let request = SearchRequest {
            objective: Objective::LiveScore {
                score_id: 1004,
                chart,
                play: PlayInput::Stream { stream, judgement_types: vec![1] },
                event: false,
                exclude_snap_skills: false,
                gekisou: Some(GekisouObjective { seeds: SeedSet::List(vec![0]), fevers: vec![(50, 150)] }),
            }
            .in_scenario(context),
            k: 5,
            constraints: Constraints { no_snaps: true, ..Default::default() },
            time_limit: None,
        };
        let limits = Limits { time_limit_ms: Some(3_000), max_candidates: None, cache_entries: 64 };
        for (stage, completed) in [("seed-dive", 0), ("seed-evaluated", 1)] {
            let result = test_clock::with_expiry(stage, 1, || {
                super::super::solve_physical_impl(
                    &pool,
                    &request,
                    &Metric::Score,
                    None,
                    &limits,
                    &Strategy::BranchAndBound,
                    None,
                    &SimulationInput::default(),
                    None,
                    &[],
                    None,
                    budget::now(),
                    None,
                )
            })
            .unwrap();
            assert_eq!(result.completion, Completion::Complete);
            assert_eq!(result.results.len(), 5);
            let seed = result.telemetry.phases.iter().find(|phase| phase.name == "seed").unwrap();
            assert_eq!(seed.candidates, completed);
            assert_eq!(result.telemetry.leaves.partial, 0);
            assert!(result.telemetry.phases.iter().any(|phase| phase.name == "search" && phase.candidates > 0));
        }
    }
}
