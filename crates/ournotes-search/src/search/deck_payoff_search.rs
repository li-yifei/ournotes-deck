//! Canonical leader/member-Snap team Top-K by the deck payoff bound of `deck_payoff.rs`.
//!
//! Members branch in `SLOTS` order with ascending nonleader IDs, so each canonical member layout is visited once, and
//! a complete layout offers its exact frontier of Snap bindings. The ranking (payoff, then power, then canonical IDs)
//! needs no evaluation: the search keeps it alone and evaluates only the decks it ranks first, in rank order. A
//! deck-determined payoff must be reproduced exactly. A bounded payoff (score steps) may fall short: such a deck is
//! ranked again by what it paid, and the ranking repeats until its first K decks are all evaluated. The joint
//! traversal takes over after `ROUNDS` rankings, or when a certified evaluation falls short.
use super::*;
use crate::domain::CandidateDomain;
use crate::search::deck_payoff::DeckPayoffBounds;
use crate::search::team_power::SLOTS;
use std::collections::HashMap;

type Key = ([i64; 5], [Option<i64>; 5]);

/// Rankings of a bounded payoff before the joint traversal takes over.
const ROUNDS: usize = 8;

/// A ranked team: payoff numerator over the engine's order mass (its bound until evaluated), power, canonical key.
struct Ranked {
    payoff: i128,
    power: i64,
    key: Key,
    deck: PhysicalDeck,
}

impl Ranked {
    fn ahead_of(&self, other: &Ranked) -> bool {
        (self.payoff, self.power) > (other.payoff, other.power)
            || ((self.payoff, self.power) == (other.payoff, other.power) && self.key < other.key)
    }
}

/// Evaluated decks that paid less than their bound, by member layout, with their payoff numerators.
type Below = HashMap<[usize; 5], Vec<(PhysicalDeck, i128)>>;

/// Ok(false) when the joint traversal is to take over (only with `joint_follows`).
pub(super) fn solve(
    domain: &CandidateDomain,
    bounds: &DeckPayoffBounds,
    e: &mut Engine<'_, '_>,
    joint_follows: bool,
) -> Result<bool, Error> {
    let scale = if e.live { ORDERS as i128 } else { 1 };
    let mut p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
    let root = bounds.upper(e.pool, domain, &p, 0)?.map(|(payoff, _)| payoff * scale);
    e.rec.frontier.clear();
    (e.rec.tracked, e.rec.bounded) = (true, true);
    e.note_open(root);
    let mut below = Below::new();
    let mut evaluated = HashSet::new();
    for round in 1..=ROUNDS {
        let setup = e.tel.environment.bounds.deck_payoff.get_or_insert_with(Default::default);
        setup.rounds = round;
        let mut top = Vec::new();
        visit(0, &mut p, domain, bounds, &below, scale, &mut top, e)?;
        if e.stop.is_some() {
            return stopped(e, root);
        }
        let mut fell = false;
        for row in top {
            if !evaluated.insert(row.deck) {
                continue;
            }
            if !e.consider(row.deck)? {
                return stopped(e, root);
            }
            let Some(offered) = e.offered.take().or_else(|| e.recorded(&row.deck)) else {
                // This known deck is outside Top-K, but its optimistic rank may still precede an unseen winner.
                // No retained exact payoff means a bounded ranking cannot close on this row. The full joint
                // traversal retains the incumbents and proves every remaining candidate against them.
                if bounds.bounded_only() {
                    if joint_follows {
                        return hand_over(e);
                    }
                    return Err(Error::Domain("unsettled deck payoff without a joint traversal".into()));
                }
                // A deck-determined payoff is exact: K evaluated decks rank ahead of this exact ranking entry.
                below.entry(row.deck.members).or_default().push((row.deck, i128::MIN));
                continue;
            };
            let mismatch = || {
                Error::Domain(format!(
                    "deck payoff bound predicted payoff {}/{scale} and power {} for {:?}, the evaluation differs",
                    row.payoff, row.power, row.key
                ))
            };
            if i64::from(offered.power) != row.power {
                return Err(mismatch());
            }
            if let (Some(cap), Some(score)) = (bounds.score_cap(row.power), offered.score)
                && i128::from(score) > cap
            {
                return Err(Error::Domain(format!("score {score} above the score cap {cap} of {:?}", row.key)));
            }
            // Mean payoff numerator/denominator (in lowest terms) against the bound numerator over `scale`, and the
            // payoff numerator over `scale` when it is one.
            let paid = offered.payoff.and_then(|(numerator, denominator)| {
                let denominator = i128::try_from(denominator).ok()?;
                let scaled = numerator.checked_mul(scale)?;
                let ranked = (scaled % denominator == 0).then(|| scaled / denominator);
                Some((scaled, row.payoff.checked_mul(denominator)?, ranked))
            });
            match paid {
                Some((paid, bound, _)) if paid == bound => {}
                Some((paid, bound, Some(ranked))) if paid < bound && bounds.bounded_only() && e.certified.is_none() => {
                    below.entry(row.deck.members).or_default().push((row.deck, ranked));
                    e.tel.environment.bounds.deck_payoff.get_or_insert_with(Default::default).shortfalls += 1;
                    fell = true;
                }
                Some((paid, bound, _)) if paid > bound => return Err(mismatch()),
                // A certified payoff below its bound, or not settled: the joint traversal ranks it.
                _ if bounds.bounded_only() && joint_follows => return hand_over(e),
                _ => return Err(mismatch()),
            }
        }
        if !fell {
            return Ok(true);
        }
    }
    if joint_follows {
        hand_over(e)
    } else {
        Err(Error::Domain("bounded deck payoff without a joint traversal".into()))
    }
}

/// The root cap covers every unvisited branch and every ranked team not yet evaluated.
fn stopped(e: &mut Engine<'_, '_>, root: Option<i128>) -> Result<bool, Error> {
    let started = e.bound_start();
    e.fold_unexplored(root, started);
    Ok(true)
}

fn hand_over(e: &mut Engine<'_, '_>) -> Result<bool, Error> {
    e.tel.environment.bounds.deck_payoff.get_or_insert_with(Default::default).handed_over = true;
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn visit(
    depth: usize,
    p: &mut PhysicalDeck,
    domain: &CandidateDomain,
    bounds: &DeckPayoffBounds,
    below: &Below,
    scale: i128,
    top: &mut Vec<Ranked>,
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    e.tel.nodes += 1;
    if e.expired() {
        return Ok(false);
    }
    let upper = bounds.upper(e.pool, domain, p, depth)?;
    let k = e.request.k;
    let count = e.tel.joint.modules.entry("deckPayoff").or_default();
    count.checks += 1;
    let Some((payoff, power)) = upper else {
        count.pruned += 1;
        return Ok(true);
    };
    let payoff = payoff * scale;
    #[cfg(not(target_arch = "wasm32"))]
    if crate::parallel::inferior(payoff, power) {
        count.pruned += 1;
        return Ok(true);
    }
    if top.len() == k {
        let kth = &top[k - 1];
        // Equal payoff and power keep every completion whose canonical key may still come first.
        if (payoff, power) < (kth.payoff, kth.power)
            || ((payoff, power) == (kth.payoff, kth.power)
                && bounds.power.least_key(e.pool, domain, p, depth).is_some_and(|key| key > kth.key))
        {
            count.pruned += 1;
            return Ok(true);
        }
    }
    if depth == 5 {
        let bare = PhysicalDeck { members: p.members, snaps: [None; 5] };
        let base = e.pool.deck_power(&bare.as_deck(), e.song, e.event)?.power();
        // Decks that paid less only move down: the layout's first K stay within its first K plus their count.
        let paid = below.get(&p.members).map_or(&[][..], Vec::as_slice);
        for (bound, power, deck) in bounds.frontier(domain, p, i64::from(base), k + paid.len())? {
            let payoff = paid.iter().find(|(d, _)| *d == deck).map_or(bound * scale, |&(_, numerator)| numerator);
            let key = (deck.members.map(|m| e.pool.members[m].id), deck.snaps.map(|s| s.map(|s| e.pool.snaps[s].id)));
            let row = Ranked { payoff, power, key, deck };
            let at = top.partition_point(|t| t.ahead_of(&row));
            if at < k {
                top.insert(at, row);
                top.truncate(k);
            }
        }
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
        if !visit(depth + 1, p, domain, bounds, below, scale, top, e)? {
            return Ok(false);
        }
    }
    Ok(true)
}
