//! Leader plus unordered nonleader compositions, then a unique-Snap branch-and-bound over the pairings of each
//! composition. A team's layout does not matter for its value: the composition is visited once, in one layout, and
//! the leaf evaluates the canonical layout.
use super::{Engine, Error, PhysicalDeck, slot};
use crate::{
    domain::CandidateDomain,
    search::joint::{JointBounds, SLOTS},
};
#[path = "composition/classes.rs"]
mod classes;

pub(super) fn solve(
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    let candidates = bounds.member_order(domain);
    let mut p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
    e.rec.clock.lap(slot::COMPOSITION);
    let pool = e.pool;
    let legal = |leader: usize| {
        domain.leader().is_none_or(|m| m == leader)
            && !domain
                .required()
                .iter()
                .any(|&m| m != leader && pool.members[m].character_id == pool.members[leader].character_id)
    };
    // The bound of every leader's subtree, then their suffix maxima: the bound of the leaders still open.
    let mut open = vec![None; candidates.len() + 1];
    for (index, &leader) in candidates.iter().enumerate().rev() {
        open[index] = open[index + 1];
        if legal(leader) {
            p.members[2] = leader;
            let cap = members_upper(&p, 1, domain, bounds, orders, e)?;
            open[index] = Some(open[index].map_or(cap, |u: i128| u.max(cap)));
        }
    }
    for (index, &leader) in candidates.iter().enumerate() {
        e.note_open(open[index]);
        if e.expired() {
            unexplored_members(0, index, &mut p, &candidates, domain, bounds, orders, e)?;
            return Ok(false);
        }
        if !legal(leader) {
            continue;
        }
        p.members[2] = leader;
        e.rec.frontier.set(0, index, candidates.len());
        if !members(1, 0, &mut p, &candidates, domain, bounds, orders, e)? {
            unexplored_members(0, index + 1, &mut p, &candidates, domain, bounds, orders, e)?;
            return Ok(false);
        }
    }
    Ok(true)
}

/// The bound of every team whose members of `SLOTS[..depth]` are those of `p` (Snaps free): the composition bound,
/// tightened by the bound modules.
fn members_upper(
    p: &PhysicalDeck,
    depth: usize,
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &Engine<'_, '_>,
) -> Result<i128, Error> {
    let (cap, _) = bounds.composition_expected_upper(e.pool, domain, p, depth, 0, true, orders)?;
    Ok(bounds.modules().iter().filter_map(|m| m.node_upper(e.pool, p, depth, false, orders)).fold(cap, i128::min))
}

/// The public (member, Snap) IDs of a team's canonical layout with the Snaps of `SLOTS[..depth]` assigned and None
/// elsewhere: the least identity any completion of the Snap assignment can have.
fn least_identity(p: &PhysicalDeck, depth: usize, e: &Engine<'_, '_>) -> ([i64; 5], [Option<i64>; 5]) {
    let mut assigned = *p;
    for &slot in &SLOTS[depth..] {
        assigned.snaps[slot] = None;
    }
    let c = super::uniform::canonical(e.pool, &assigned);
    (c.members.map(|m| e.pool.members[m].id), c.snaps.map(|s| s.map(|s| e.pool.snaps[s].id)))
}

fn inferior(cap: i128, power: i64, p: Option<(&PhysicalDeck, usize)>, e: &Engine<'_, '_>) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if crate::parallel::inferior(cap, power) {
        return true;
    }
    let Some((threshold, kth_power)) = e.safe_cutoff() else { return false };
    if cap != threshold || e.metric.secondary_priority().is_some() {
        return cap < threshold;
    }
    // The interval cutoff proves power ties only. It does not identify an exact K-th team whose public-ID tie
    // can close this subtree.
    if e.certified.is_some() {
        return power < i64::from(kth_power);
    }
    if power != i64::from(kth_power) {
        return power < i64::from(kth_power);
    }
    let kth = e.top.last().expect("exact cutoff has K incumbents");
    if let Some((p, depth)) = p {
        // None is legal in every unassigned slot and is the smallest public Snap key.
        let (members, snaps) = least_identity(p, depth, e);
        if members != kth.members {
            return members > kth.members;
        }
        return snaps > kth.snaps;
    }
    false
}

/// The stop leaves the member choices `candidates[from..]` at `depth` unexplored (leaders at depth 0): bound each
/// by its child's composition bound, the bound the traversal itself checks there.
#[allow(clippy::too_many_arguments)]
fn unexplored_members(
    depth: usize,
    from: usize,
    p: &mut PhysicalDeck,
    candidates: &[usize],
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<(), Error> {
    if e.stop.is_none() {
        return Ok(());
    }
    let started = e.bound_start();
    let mut upper: Option<i128> = None;
    for &m in &candidates[from.min(candidates.len())..] {
        let character = e.pool.members[m].character_id;
        if (depth == 0 && domain.leader().is_some_and(|l| l != m))
            || SLOTS[..depth].iter().any(|&s| e.pool.members[p.members[s]].character_id == character)
            || domain.required().iter().any(|&r| r != m && e.pool.members[r].character_id == character)
        {
            continue;
        }
        p.members[SLOTS[depth]] = m;
        let cap = members_upper(p, depth + 1, domain, bounds, orders, e)?;
        upper = Some(upper.map_or(cap, |u| u.max(cap)));
    }
    e.fold_unexplored(upper, started);
    Ok(())
}

/// The stop leaves this composition node's whole subtree unexplored.
fn unexplored_node(
    depth: usize,
    p: &PhysicalDeck,
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<(), Error> {
    if e.stop.is_none() {
        return Ok(());
    }
    let started = e.bound_start();
    let cap = members_upper(p, depth, domain, bounds, orders, e)?;
    e.fold_unexplored(Some(cap), started);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn members(
    depth: usize,
    start: usize,
    p: &mut PhysicalDeck,
    candidates: &[usize],
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    e.tel.nodes += 1;
    e.tel.composition.member_nodes[depth] += 1;
    if e.expired() {
        unexplored_node(depth, p, domain, bounds, orders, e)?;
        return Ok(false);
    }
    let missing: Vec<_> =
        domain.required().iter().filter(|m| !SLOTS[..depth].iter().any(|&s| p.members[s] == **m)).collect();
    if missing.len() > 5 - depth || missing.iter().any(|m| !candidates[start..].contains(m)) {
        return Ok(true);
    }
    if e.has_pruning_cutoff() {
        e.tel.composition.composition.checks += 1;
        let (cap, power) = bounds.composition_expected_upper(e.pool, domain, p, depth, 0, true, orders)?;
        if inferior(cap, power, None, e) {
            e.tel.composition.composition.pruned += 1;
            return Ok(true);
        }
        for module in bounds.modules() {
            let Some(cap) = module.node_upper(e.pool, p, depth, false, orders) else { continue };
            e.tel.composition.modules.entry(module.name()).or_default().checks += 1;
            if inferior(cap, power, None, e) {
                e.tel.composition.modules.entry(module.name()).or_default().pruned += 1;
                return Ok(true);
            }
        }
    }
    if depth == 5 {
        e.tel.composition.compositions += 1;
        let mut seeds = std::collections::HashSet::new();
        let more = (!bounds.uses_class_search() || seed_team(p, &mut seeds, domain, bounds, orders, e)?)
            && team(p, &seeds, domain, bounds, orders, e)?;
        if !more {
            unexplored_node(depth, p, domain, bounds, orders, e)?;
        }
        return Ok(more);
    }
    for (index, &m) in candidates.iter().enumerate().skip(start) {
        let character = e.pool.members[m].character_id;
        if SLOTS[..depth].iter().any(|&s| e.pool.members[p.members[s]].character_id == character)
            || domain.required().iter().any(|&r| r != m && e.pool.members[r].character_id == character)
        {
            continue;
        }
        p.members[SLOTS[depth]] = m;
        e.rec.frontier.set(depth, index - start, candidates.len() - start);
        if !members(depth + 1, index + 1, p, candidates, domain, bounds, orders, e)? {
            unexplored_members(depth, index + 1, p, candidates, domain, bounds, orders, e)?;
            return Ok(false);
        }
    }
    Ok(true)
}

/// Class schedules only: proposals evaluated before the composition's class search.
fn seed_team(
    p: &mut PhysicalDeck,
    seen: &mut std::collections::HashSet<PhysicalDeck>,
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    if e.expired() {
        return Ok(false);
    }
    p.snaps = [None; 5];
    let (cap, power) = bounds.composition_expected_upper(e.pool, domain, p, 5, 0, false, orders)?;
    if inferior(cap, power, Some((p, 0)), e) {
        return Ok(true);
    }
    let (_, first) = bounds.layout_power(domain, p, 0);
    for proposal in std::iter::once(first).chain(bounds.weighted_layout_seeds(domain, p, orders)) {
        if seen.contains(&proposal) {
            continue;
        }
        e.tel.composition.seeds.preseed += 1;
        if !e.consider(proposal)? {
            return Ok(false);
        }
        seen.insert(proposal);
    }
    Ok(true)
}

/// The Snap pairings of one composition: the best-power pairing first, then (PT) the power frontier, then the
/// branch-and-bound over the remaining pairings.
fn team(
    p: &mut PhysicalDeck,
    seeds: &std::collections::HashSet<PhysicalDeck>,
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    if e.expired() {
        return Ok(false);
    }
    p.snaps = [None; 5];
    let mut evaluated: std::collections::HashSet<_> =
        seeds.iter().filter(|d| d.members == p.members).map(|d| d.snaps).collect();
    e.tel.composition.team.checks += 1;
    let (cap, power) = bounds.composition_expected_upper(e.pool, domain, p, 5, 0, false, orders)?;
    if inferior(cap, power, Some((p, 0)), e) {
        e.tel.composition.team.pruned += 1;
        return Ok(true);
    }
    // Every proposal is a leaf of this composition: its per-order caps drop it when it cannot enter the Top-K.
    let (_, proposal) = bounds.layout_power(domain, p, 0);
    if !evaluated.contains(&proposal.snaps) {
        e.tel.composition.seeds.team += 1;
        if !e.consider_with(proposal, Some((bounds, domain)))? {
            return Ok(false);
        }
        evaluated.insert(proposal.snaps);
    }
    if domain.snaps().is_empty() {
        e.tel.composition.power_frontier_closed += 1;
        return Ok(true);
    }
    if bounds.uses_class_search() {
        for proposal in bounds.weighted_layout_seeds(domain, p, orders) {
            if evaluated.contains(&proposal.snaps) {
                continue;
            }
            e.tel.composition.seeds.weighted += 1;
            if !e.consider(proposal)? {
                return Ok(false);
            }
            evaluated.insert(proposal.snaps);
        }
        return classes::solve(p, &evaluated, domain, bounds, orders, e);
    }
    // Pay for the Top-K assignment DP only when the best-power seed actually
    // attains the composition's primary cap. Skipping it merely retains full DFS.
    let canonical = super::uniform::canonical(e.pool, &proposal);
    let cap_attained = e.certified.is_none()
        && e.top.iter().any(|entry| entry.physical == canonical && entry.evaluation.expected_payoff.numerator == cap);
    if e.metric.secondary_priority().is_none() && bounds.has_terminal_payoff_cap() && cap_attained {
        let proposals = bounds.layout_power_frontier(e.pool, domain, p, e.request.k);
        let exhausted = proposals.len() < e.request.k;
        let last = proposals.last().copied();
        for proposal in proposals {
            if evaluated.contains(&proposal.snaps) {
                continue;
            }
            e.tel.composition.seeds.power_frontier += 1;
            if !e.consider_with(proposal, Some((bounds, domain)))? {
                return Ok(false);
            }
            evaluated.insert(proposal.snaps);
        }
        if exhausted {
            e.tel.composition.power_frontier_closed += 1;
            return Ok(true);
        }
        if let Some(last) = last {
            let power = i64::from(e.pool.deck_power(&last.as_deck(), e.song, e.event)?.power());
            let equal = e.certified.is_none()
                && e.safe_cutoff()
                    .is_some_and(|(threshold, kth_power)| cap == threshold && power == i64::from(kth_power))
                && e.top.last().is_some_and(|kth| least_identity(&last, 5, e) == (kth.members, kth.snaps));
            // All unexamined bindings rank strictly after the last power-ranked
            // proposal. Equal cutoff identity was already evaluated above.
            if equal || inferior(cap, power, Some((&last, 5)), e) {
                e.tel.composition.power_frontier_closed += 1;
                return Ok(true);
            }
        }
    }
    let choices = std::array::from_fn(|slot| bounds.slot_choices(domain, p, slot, orders));
    snaps(0, p, &choices, &evaluated, domain, bounds, orders, e)
}

#[allow(clippy::too_many_arguments)]
fn snaps(
    depth: usize,
    p: &mut PhysicalDeck,
    choices: &[Vec<usize>; 5],
    evaluated: &std::collections::HashSet<[Option<usize>; 5]>,
    domain: &CandidateDomain,
    bounds: &JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    e.tel.nodes += 1;
    e.tel.composition.snap_nodes += 1;
    if e.expired() {
        return Ok(false);
    }
    if depth == 5 && evaluated.contains(&p.snaps) {
        return Ok(true);
    }
    if e.has_pruning_cutoff() {
        e.tel.composition.team.checks += 1;
        let (cap, power) = bounds.composition_expected_upper(e.pool, domain, p, 5, depth, false, orders)?;
        if inferior(cap, power, Some((p, depth)), e) {
            e.tel.composition.team.pruned += 1;
            return Ok(true);
        }
    }
    if depth == 5 {
        // The leaf sums the team's per-order caps (cheap, raw, fine) before simulating.
        return e.consider_with(*p, Some((bounds, domain)));
    }
    let slot = SLOTS[depth];
    for &choice in &choices[slot] {
        let snap = if choice == 0 { None } else { Some(domain.snaps()[choice - 1]) };
        if snap.is_some() && SLOTS[..depth].iter().any(|&s| p.snaps[s] == snap) {
            continue;
        }
        p.snaps[slot] = snap;
        if !snaps(depth + 1, p, choices, evaluated, domain, bounds, orders, e)? {
            return Ok(false);
        }
    }
    Ok(true)
}
