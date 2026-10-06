//! Exact canonical team Top-K for native power and monotone Skip utilities.
use super::*;
use crate::{
    domain::CandidateDomain,
    search::team_power::{SLOTS, TeamPowerBounds},
};

pub(super) fn solve(domain: &CandidateDomain, bounds: &TeamPowerBounds, e: &mut Engine<'_, '_>) -> Result<(), Error> {
    debug_assert!(!e.live && e.certified.is_none());
    let mut p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
    let Some(root_power) = bounds.upper(e.pool, domain, &p, 0) else {
        return Ok(());
    };
    let root = bounds.primary_upper(root_power, e.metric)?;
    e.rec.frontier.clear();
    (e.rec.tracked, e.rec.bounded) = (true, true);
    e.note_open(Some(root));
    let pool = e.pool;
    for proposal in bounds.seeds(pool, domain, || e.expired()) {
        if !e.consider(proposal)? {
            break;
        }
    }
    if !e.expired() {
        visit(0, &mut p, domain, bounds, e)?;
    }
    if e.stop.is_some() {
        let started = e.bound_start();
        e.fold_unexplored(Some(root), started);
    }
    Ok(())
}

fn visit(
    depth: usize,
    p: &mut PhysicalDeck,
    domain: &CandidateDomain,
    bounds: &TeamPowerBounds,
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    e.tel.nodes += 1;
    if e.expired() {
        return Ok(false);
    }
    let upper = bounds.upper(e.pool, domain, p, depth);
    let count = e.tel.joint.modules.entry("teamPower").or_default();
    count.checks += 1;
    let Some(power) = upper else {
        count.pruned += 1;
        return Ok(true);
    };
    #[cfg(not(target_arch = "wasm32"))]
    if crate::parallel::inferior(bounds.primary_upper(power, e.metric)?, power) {
        count.pruned += 1;
        return Ok(true);
    }
    if e.top.len() == e.request.k {
        let kth = e.top.last().expect("full exact Top-K");
        // Monotone primary then power has the same order as power alone. Equal
        // power must still retain every smaller canonical member/Snap identity.
        if power < i64::from(kth.power)
            || (power == i64::from(kth.power)
                && bounds.least_key(e.pool, domain, p, depth).is_some_and(|key| key > (kth.members, kth.snaps)))
        {
            count.pruned += 1;
            return Ok(true);
        }
    }
    if depth == 5 {
        let proposals = bounds.frontier(domain, p, e.request.k);
        if e.expired() {
            return Ok(false);
        }
        for proposal in proposals {
            if !e.consider(proposal)? {
                return Ok(false);
            }
        }
        // Every omitted binding has K distinct better bindings of THIS member
        // layout and leader. Other leaders and member compositions remain open.
        e.tel.composition.power_frontier_closed += 1;
        return Ok(true);
    }
    let slot = SLOTS[depth];
    for (index, &member) in bounds.members.iter().enumerate() {
        if index % 32 == 31 && e.expired() {
            return Ok(false);
        }
        if slot == 2 && domain.leader().is_some_and(|m| m != member) {
            continue;
        }
        if depth > 1 && e.pool.members[member].id <= e.pool.members[p.members[SLOTS[depth - 1]]].id {
            continue;
        }
        let c = e.pool.members[member].character_id;
        if SLOTS[..depth].iter().any(|&s| e.pool.members[p.members[s]].character_id == c)
            || domain.required().iter().any(|&m| m != member && e.pool.members[m].character_id == c)
        {
            continue;
        }
        p.members[slot] = member;
        e.rec.frontier.set(depth, index, bounds.members.len());
        if !visit(depth + 1, p, domain, bounds, e)? {
            return Ok(false);
        }
    }
    Ok(true)
}
