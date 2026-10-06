//! Native proposal ordering for LUCK charts. Every proposal still uses the full scorer.
use super::{Engine, PhysicalDeck, uniform};
use crate::domain::CandidateDomain;
use ournotes_sim::{
    Error,
    live::full::{LuckSource, luck_skill_key},
};
use std::collections::HashSet;

fn pilot_orders() -> Vec<[usize; 5]> {
    let mut out = Vec::with_capacity(10);
    for mut base in [[0, 1, 2, 3, 4], [4, 3, 2, 1, 0]] {
        for _ in 0..5 {
            out.push(base);
            base.rotate_left(1);
        }
    }
    out
}

/// Pilot values only schedule proposals; they never enter the certified frontier or remove a domain member.
#[derive(Default)]
pub(super) struct Screen {
    best: Vec<f64>,
}
impl Screen {
    pub(super) fn admit(&mut self, p: &PhysicalDeck, priority: bool, e: &mut Engine<'_, '_>) -> Result<bool, Error> {
        if e.expired() {
            return Ok(false);
        }
        let mut input = super::expectation::context(e.pool, p, &e.request.objective)?;
        if let Some(v) = e.simulation.music_length_ms {
            input.params.music_length_ms = v;
        }
        if let Some(v) = e.simulation.score_music_length_ms {
            input.params.score_music_length_ms = Some(v);
        }
        crate::search::certified_search::canonicalize_performers(&mut input);
        let skills = e.certified_luck_skills()?;
        // Two cyclic sets give each performer two appearances at every position.
        let pilot = pilot_orders();
        let state = e.certified.as_mut().expect("LUCK");
        let caches = if state.parallel_curves.is_empty() {
            std::slice::from_mut(&mut state.luck_curves)
        } else {
            &mut state.parallel_curves
        };
        let result = crate::native_jobs::map(caches, &pilot, crate::parallel::cancellation_check(), |cache, order| {
            crate::search::certified_search::luck_order(e.pool.master, &skills, &input, *order, Some(cache))
        })?;
        let Some(result) = result else {
            return Ok(false);
        };
        e.tel.incumbents.warm_start.pilot_orders += result.len() as u64;
        let mean = result.iter().map(|r| (r.mean.lower() + r.mean.upper()) * 0.5).sum::<f64>() / result.len() as f64;
        if !priority && self.best.len() == 5 && mean < self.best.iter().sum::<f64>() / 5.0 * 0.98 {
            e.tel.incumbents.warm_start.deferred_proposals += 1;
            return Ok(false);
        }
        self.best.push(mean);
        self.best.sort_by(|a, b| b.total_cmp(a));
        self.best.truncate(5);
        Ok(true)
    }
}

pub(super) fn seed(domain: &CandidateDomain, e: &mut Engine<'_, '_>, screen: &mut Screen) -> Result<(), Error> {
    let pool = e.pool;
    let luck = |m: usize| pool.members[m].gekisou_mission_type == Some(2);
    let strength = |m: usize| {
        let p = pool.members[m].power;
        p.performance as i128 + p.technique as i128 + p.visual as i128
    };
    let mut members = domain.members().to_vec();
    members.sort_by_key(|&m| (std::cmp::Reverse(strength(m)), pool.members[m].id));
    let distinct = members
        .iter()
        .copied()
        .filter(|&m| luck(m))
        .filter(|&m| {
            !domain.required().iter().any(|&r| !luck(r) && pool.members[r].character_id == pool.members[m].character_id)
        })
        .map(|m| pool.members[m].character_id)
        .collect::<HashSet<_>>()
        .len()
        .min(5 - domain.required().iter().filter(|&&m| !luck(m)).count());
    if distinct == 0 {
        return Ok(());
    }
    // Rank support pairings using their mission and holder's formation predicates.
    let mut pairings = vec![Vec::new(); pool.members.len()];
    for &m in &members {
        let holder = crate::search::snaps::performer(&pool.members[m], None)?;
        for &s in domain.snaps() {
            let mut matched = 0;
            if holder.gekisou_skill.is_some() {
                for (id, level) in pool.snaps[s].gekisou_support_skills()? {
                    if pool.master.gekisou_support_skill(id).is_some_and(|r| r.gekisou_mission_type == 2)
                        && luck_skill_key(pool.master, LuckSource::GekisouSupport, id, level, &holder)?.matched
                            != Some(false)
                    {
                        matched += 1;
                    }
                }
            }
            let p = pool.snaps[s].power_bonus_percent;
            pairings[m].push((s, matched, p.performance as i128 + p.technique as i128 + p.visual as i128));
        }
        pairings[m].sort_by_key(|&(s, matched, power)| {
            (std::cmp::Reverse(matched), std::cmp::Reverse(power), pool.snaps[s].id)
        });
    }
    let mut seen = HashSet::new();
    // Maximal LUCK coverage comes first; then replace one and two slots with strong generalists.
    for quota in (distinct.saturating_sub(2).max(1)..=distinct).rev() {
        if e.expired() {
            break;
        }
        let mut proposals = Vec::new();
        for &anchor in &members {
            let mut chosen = domain.required().to_vec();
            let add = |chosen: &mut Vec<usize>, m: usize| {
                if chosen.len() < 5
                    && !chosen.iter().any(|&v| pool.members[v].character_id == pool.members[m].character_id)
                {
                    chosen.push(m);
                }
            };
            if (quota == distinct) != luck(anchor) {
                continue;
            }
            add(&mut chosen, anchor);
            for &m in &members {
                if chosen.iter().filter(|&&v| luck(v)).count() >= quota {
                    break;
                }
                if luck(m) {
                    add(&mut chosen, m);
                }
            }
            for &m in &members {
                if !luck(m) {
                    add(&mut chosen, m);
                }
            }
            if chosen.len() != 5 {
                continue;
            }
            let count = chosen.iter().filter(|&&m| luck(m)).count();
            if count != quota {
                continue;
            }
            for leader in 0..5 {
                if domain.leader().is_some_and(|m| chosen[leader] != m) {
                    continue;
                }
                let mut p = PhysicalDeck { members: chosen.clone().try_into().expect("five"), snaps: [None; 5] };
                p.members.swap(2, leader);
                let mut used = HashSet::new();
                for slot in [2, 0, 1, 3, 4] {
                    if let Some(&(s, _, _)) = pairings[p.members[slot]].iter().find(|(s, _, _)| !used.contains(s)) {
                        p.snaps[slot] = Some(s);
                        used.insert(s);
                    }
                }
                let p = uniform::canonical(pool, &p);
                domain.check_fixed(pool, &p)?;
                if seen.insert(p) {
                    let power = pool.deck_power(&p.as_deck(), e.song, e.event)?.power();
                    proposals.push((power, p));
                }
            }
        }
        proposals.sort_by_key(|(power, _)| std::cmp::Reverse(*power));
        for (_, p) in proposals.into_iter().take(8) {
            if !screen.admit(&p, quota == distinct, e)? {
                continue;
            }
            e.tel.incumbents.warm_start.evaluations += 1;
            if !e.consider(p)? {
                return Ok(());
            }
            e.seeded.insert(p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pilot_balances_every_performer_position() {
        let orders = super::pilot_orders();
        assert_eq!(orders.iter().collect::<std::collections::HashSet<_>>().len(), 10);
        for slot in 0..5 {
            for performer in 0..5 {
                assert_eq!(orders.iter().filter(|o| o[slot] == performer).count(), 2);
            }
        }
    }
}
