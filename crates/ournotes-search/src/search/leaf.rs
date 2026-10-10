//! Leaf evaluation of a played-live team: its exact payoff numerator over the 120 performance orders, or a proof that
//! it cannot enter the Top-K.
//!
//! Contract of [`Engine::evaluate_orders`]: given a team in its canonical layout, its power and optionally the joint
//! bounds, return either the exact evaluation (one simulation per performance order, every outcome kept) or `Pruned`
//! only when the team provably ranks after the current K-th (payoff below it, or equal with a smaller power), or
//! `Stopped` when the budget ran out. Per-order caps (`JointBounds::order_cheap_caps`, `tighten_order_caps`) and the
//! cutoff tables are the upper bounds it may use.
//!
//! The orders play as one tree ([`OrderedLive::simulate_orders_bounded`]): the frames before a member acts are the
//! same in every order that agrees on the members that acted, so they play once. A node's bound is the sum, over its
//! orders, of the order's cap and of its cutoff table at the node's settled prefix: until a member of an open
//! position acts, the node's live is the live of each of its orders, so each table applies to the shared prefix.
use super::*;
use ournotes_sim::live::full::{LiveModel, OrdersOutcome, Settled};
use ournotes_sim::live::random::LiveRandom;
use std::ops::ControlFlow;

/// Frames between two bound checks of the node being played.
const CUTOFF_EVERY: usize = 30;

/// How the evaluation of a team's performance orders ended.
pub(super) enum Leaf {
    Evaluated(FiniteEvaluation),
    Certified(
        crate::search::certified_search::CertifiedEvaluation,
        Vec<u8>,
        crate::search::certified_search::PayoffMap,
    ),
    /// Provably below the K-th: not a Top-K deck.
    Pruned,
    /// The budget ran out.
    Stopped,
}

/// Saturating sum of payoff caps.
fn cap_sum(caps: &[i128]) -> i128 {
    caps.iter().fold(0i128, |a, &c| a.saturating_add(c))
}

impl Engine<'_, '_> {
    /// The payoff numerator of a team over the performance orders: each order simulated once, the sum over all of
    /// them. With a full Top-K and bounds, the team's per-order caps (cheap, then raw and fine) are summed first; the
    /// orders then run in descending cap order, each with a cutoff table when the fine bound is compiled. The team
    /// is dropped as soon as the exact payoffs of the orders run plus the caps of the others fall below the K-th (an
    /// equal total with a smaller power too, which keeps every power/ID tie).
    pub(super) fn evaluate_orders(
        &mut self,
        physical: &PhysicalDeck,
        power: i32,
        cut: Option<(&crate::search::joint::JointBounds, &crate::domain::CandidateDomain)>,
    ) -> Result<Leaf, Error> {
        if self.certified.is_some() {
            // The caps prove a team below the K-th whether or not its scores are cached.
            if let (Some((bounds, domain)), Some((threshold, kth_power))) = (cut, self.safe_cutoff()) {
                let below = |total: i128| total < threshold || (total == threshold && power < kth_power);
                let mut caps = bounds.order_cheap_caps(domain, physical, i64::from(power), &self.positions);
                if below(cap_sum(&caps)) {
                    self.tel.leaves.cheap_pruned += 1;
                    return Ok(Leaf::Pruned);
                }
                if bounds.has_fine() {
                    let (_, resume) = self.rec.clock.lap(slot::FINE);
                    let pruned = bounds.tighten_order_caps_until(
                        domain,
                        physical,
                        i64::from(power),
                        &self.positions,
                        &mut caps,
                        &mut self.bound_scratch,
                        below,
                        &mut self.tel.leaves.fine_orders,
                    );
                    self.rec.clock.lap(resume);
                    if pruned {
                        self.tel.leaves.fine_pruned += 1;
                        return Ok(Leaf::Pruned);
                    }
                }
            }
            let mut input = expectation::context(self.pool, physical, &self.request.objective)?;
            if let Some(v) = self.simulation.music_length_ms {
                input.params.music_length_ms = v;
            }
            if let Some(v) = self.simulation.score_music_length_ms {
                input.params.score_music_length_ms = Some(v);
            }
            self.admit_certified_refinement(input.notes.len(), input.play.frames.len());
            let program = crate::search::certified_search::canonicalize_performers(&mut input);
            let master = self.pool.master;
            let score = if let Some(score) = self.cached_certified_score(&program, power) {
                score
            } else {
                self.tel.leaves.started += 1;
                let skills = self.certified_luck_skills()?;
                let mut curves = std::mem::take(&mut self.certified.as_mut().expect("certified request").luck_curves);
                let (_, resume) = self.rec.clock.lap(slot::SIMULATION);
                #[cfg(not(target_arch = "wasm32"))]
                let parallel = if crate::parallel::simulation_workers() > 1 {
                    Some(crate::search::certified_search::evaluate_luck_context_parallel(
                        master,
                        &skills,
                        &input,
                        &crate::search::certified_search::PayoffMap::Score,
                        &mut self.certified.as_mut().expect("certified request").parallel_curves,
                    ))
                } else {
                    None
                };
                #[cfg(target_arch = "wasm32")]
                let parallel = None;
                let result = parallel.unwrap_or_else(|| {
                    crate::search::certified_search::evaluate_luck_context(
                        master,
                        &skills,
                        &input,
                        &crate::search::certified_search::PayoffMap::Score,
                        Some(&mut curves),
                        || self.expired(),
                    )
                });
                self.rec.clock.lap(resume);
                let stats = curves.stats();
                #[cfg(not(target_arch = "wasm32"))]
                let stats = self.certified.as_ref().expect("certified request").parallel_curves.iter().fold(
                    stats,
                    |mut sum, cache| {
                        let s = cache.stats();
                        sum.lookups += s.lookups;
                        sum.hits += s.hits;
                        sum.evictions += s.evictions;
                        sum.peak_entries += s.peak_entries;
                        sum.peak_key_bytes += s.peak_key_bytes;
                        sum.record_ms += s.record_ms;
                        sum.propagate_ms += s.propagate_ms;
                        sum
                    },
                );
                self.tel.caches.luck_curves.record(stats);
                self.certified.as_mut().expect("certified request").luck_curves = curves;
                let Some(score) = result? else {
                    self.expired();
                    return Ok(Leaf::Stopped);
                };
                self.tel.leaves.simulations += ORDERS as u64;
                self.cache_certified_score(program.clone(), power, &score);
                score
            };
            let support = (
                score.orders.iter().map(|o| o.support.0).min().expect("120 orders"),
                score.orders.iter().map(|o| o.support.1).max().expect("120 orders"),
            );
            let map = crate::search::certified_payoff::payoff_map(
                self.pool,
                self.request,
                self.metric,
                self.event_input,
                physical,
                power,
                support,
            )?;
            let evaluation = crate::search::certified_search::aggregate_orders(score.orders, &map)?;
            return Ok(Leaf::Certified(evaluation, program, map));
        }
        if let Some(threshold) = crate::search::snaps::census() {
            return self.census_leaf(physical, power, cut, threshold);
        }
        if let Some(scores) = self.team_scores.get(physical, power, &mut self.tel.caches.team_scores) {
            let outcomes = self
                .orders
                .iter()
                .zip(scores)
                .map(|(&performance_order, (final_score, final_life))| {
                    Ok(SeedOutcome {
                        root_seed: 0,
                        weight: 1,
                        performance_order,
                        final_score,
                        terminal_payoff: self.payoff(physical, final_score, power, Some(final_life))?,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            return Ok(Leaf::Evaluated(expectation::aggregate(outcomes)?));
        }
        let mut input = expectation::context(self.pool, physical, &self.request.objective)?;
        if let Some(v) = self.simulation.music_length_ms {
            input.params.music_length_ms = v;
        }
        if let Some(v) = self.simulation.score_music_length_ms {
            input.params.score_music_length_ms = Some(v);
        }
        input.lottery_free = self.lottery_free.clone();
        let performers = input.performers.clone();
        let cached = self.programs.get_partial(physical.members, &performers, power, &mut self.tel.caches.programs);
        self.tel.caches.program_evaluation_ms = self.programs.evaluation_ms();
        let mut outcomes: Vec<Option<SeedOutcome>> = vec![None; ORDERS];
        let mut final_lives = [0; ORDERS];
        let mut cached_sum = 0i128;
        let mut cached_count = 0usize;
        if let Some(cached) = cached {
            for (i, score) in cached.into_iter().enumerate() {
                if let Some((final_score, final_life)) = score {
                    let payoff = self.payoff(physical, final_score, power, Some(final_life))?;
                    cached_sum += payoff;
                    cached_count += 1;
                    final_lives[i] = final_life;
                    outcomes[i] = Some(SeedOutcome {
                        root_seed: 0,
                        weight: 1,
                        performance_order: self.orders[i],
                        final_score,
                        terminal_payoff: payoff,
                    });
                }
            }
            self.tel.caches.program_orders_reused += cached_count as u64;
        }
        if cached_count == ORDERS {
            let evaluation = expectation::aggregate(outcomes.into_iter().map(Option::unwrap).collect())?;
            self.team_scores.insert(physical, power, &evaluation, &final_lives, &mut self.tel.caches.team_scores);
            return Ok(Leaf::Evaluated(evaluation));
        }
        let kth = self.safe_cutoff();
        let below = |total: i128| {
            kth.is_some_and(|(threshold, kth_power)| total < threshold || (total == threshold && power < kth_power))
        };
        let bounds = cut.filter(|_| kth.is_some());
        let caps = match bounds {
            Some((b, domain)) => {
                let mut caps = b.order_cheap_caps(domain, physical, i64::from(power), &self.positions);
                if below(cap_sum(&caps)) {
                    self.tel.leaves.cheap_pruned += 1;
                    return Ok(Leaf::Pruned);
                }
                if b.has_fine() {
                    let (_, resume) = self.rec.clock.lap(slot::FINE);
                    let pruned = b.tighten_order_caps_until(
                        domain,
                        physical,
                        i64::from(power),
                        &self.positions,
                        &mut caps,
                        &mut self.bound_scratch,
                        below,
                        &mut self.tel.leaves.fine_orders,
                    );
                    self.rec.clock.lap(resume);
                    if pruned {
                        self.tel.leaves.fine_pruned += 1;
                        return Ok(Leaf::Pruned);
                    }
                }
                Some(caps)
            }
            None => None,
        };
        // The cap of each order: its own cap, else the metric's (None: unbounded).
        let caps = caps.or_else(|| self.metric.upper().map(|c| vec![c; ORDERS]));
        if caps.as_ref().is_some_and(|caps| {
            below(
                caps.iter().enumerate().fold(cached_sum, |sum, (i, cap)| {
                    if outcomes[i].is_some() { sum } else { sum.saturating_add(*cap) }
                }),
            )
        }) {
            self.tel.leaves.order_bound_pruned += 1;
            return Ok(Leaf::Pruned);
        }
        let fine = bounds.filter(|(b, _)| b.has_fine());
        let (pool, request, metric, event_input) = (self.pool, self.request, self.metric, self.event_input);
        let master = self.pool.master;
        let performance_orders = self.orders.clone();
        let missing: Vec<usize> = (0..ORDERS).filter(|&i| outcomes[i].is_none()).collect();
        let missing_orders: Vec<[usize; 5]> = missing.iter().map(|&i| performance_orders[i]).collect();
        let orders: Vec<Vec<usize>> = missing_orders.iter().map(|o| o.to_vec()).collect();
        let capture_budget =
            self.programs.capture_budget_for(physical.members, &performers, &mut self.tel.caches.program_admissions);
        self.tel.caches.program_bytes = self.programs.allocated_bytes();
        self.tel.caches.program_recordings += u64::from(capture_budget > 0);
        let live = input.into_ordered();
        self.tel.leaves.started += 1;
        let recording_started = (capture_budget > 0).then(Instant::now);
        let (_, resume) = self.rec.clock.lap(slot::SIMULATION);
        let mut grouped: Option<(OrdersOutcome, Option<Vec<ournotes_sim::live::full::RecordedOrder>>)> = None;
        #[cfg(not(target_arch = "wasm32"))]
        if crate::parallel::simulation_workers() > 1 {
            // Native: the orders play as prefix groups on several threads. The per-order caps still
            // bound the team; a shared remaining-sum check replaces the per-node cutoff tables.
            let workers = crate::parallel::simulation_workers();
            let local_caps: Option<Vec<i128>> = caps.as_ref().map(|caps| missing.iter().map(|&i| caps[i]).collect());
            let stop = match (local_caps.as_deref(), kth) {
                (Some(caps), Some((threshold, kth_power))) => {
                    // `below(total)` holds exactly when `total < stop_below`
                    let stop_below = (if power < kth_power { threshold.saturating_add(1) } else { threshold })
                        .saturating_sub(cached_sum);
                    Some((stop_below, caps))
                }
                _ => None,
            };
            let cancelled = crate::parallel::cancellation_check();
            let payoff = |_: usize, model: &LiveModel| -> Result<i128, Error> {
                if model.draws() != 0 {
                    return Err(Error::Unsupported(
                        "a skill or mission of this team draws a lottery; the uniform member-order target covers \
                         lottery-free lives only"
                            .into(),
                    ));
                }
                payoff_of(
                    pool,
                    request,
                    metric,
                    event_input,
                    physical,
                    model.score(),
                    power,
                    Some(model.current_life()),
                )
            };
            let (outcome, results, programs) = live.simulate_orders_grouped(
                master,
                &orders,
                LiveRandom::new(0),
                capture_budget,
                workers,
                crate::parallel::order_group_depth(workers),
                stop,
                CUTOFF_EVERY,
                &cancelled,
                &payoff,
            )?;
            for r in results.into_iter().flatten() {
                let i = missing[r.index];
                final_lives[i] = r.final_life;
                outcomes[i] = Some(SeedOutcome {
                    root_seed: 0,
                    weight: 1,
                    performance_order: performance_orders[i],
                    final_score: r.score,
                    terminal_payoff: r.payoff,
                });
            }
            if matches!(outcome, OrdersOutcome::Interrupted(_)) {
                self.expired();
            }
            grouped = Some((outcome, programs));
        }
        let (outcome, programs) = if let Some(grouped) = grouped {
            grouped
        } else {
            let mut visit = |local: usize, model: &LiveModel| -> Result<i128, Error> {
                let i = missing[local];
                if model.draws() != 0 {
                    return Err(Error::Unsupported(
                        "a skill or mission of this team draws a lottery; the uniform member-order target covers \
                     lottery-free lives only"
                            .into(),
                    ));
                }
                let final_score = model.score();
                final_lives[i] = model.current_life();
                let payoff = payoff_of(
                    pool,
                    request,
                    metric,
                    event_input,
                    physical,
                    final_score,
                    power,
                    Some(model.current_life()),
                )?;
                outcomes[i] = Some(SeedOutcome {
                    root_seed: 0,
                    weight: 1,
                    performance_order: performance_orders[i],
                    final_score,
                    terminal_payoff: payoff,
                });
                Ok(payoff)
            };
            match (caps, kth) {
                (Some(caps), Some((threshold, kth_power))) => {
                    // `below(total)` holds exactly when `total < stop_below`
                    let stop_below = (if power < kth_power { threshold.saturating_add(1) } else { threshold })
                        .saturating_sub(cached_sum);
                    let mut tables: Vec<Option<Option<_>>> = (0..ORDERS).map(|_| None).collect();
                    let upper = |ids: &[usize], model: &LiveModel, s: Settled| {
                        if self.expired() {
                            return Ok(ControlFlow::Break(()));
                        }
                        let Some((b, domain)) = fine.filter(|_| model.frames_played() > 0) else {
                            return Ok(ControlFlow::Continue(
                                ids.iter().fold(0i128, |a, &local| a.saturating_add(caps[missing[local]])),
                            ));
                        };
                        let (_, resume) = self.rec.clock.lap(slot::CUTOFF_TABLE);
                        let mut sum = 0i128;
                        for &local in ids {
                            let i = missing[local];
                            let table = tables[i].get_or_insert_with(|| {
                                let t = b.cutoff_table(
                                    domain,
                                    physical,
                                    i64::from(power),
                                    &self.positions[i],
                                    &mut self.bound_scratch,
                                );
                                self.tel.leaves.cutoff.tables += u64::from(t.is_some());
                                self.tel.leaves.cutoff.unavailable += u64::from(t.is_none());
                                t
                            });
                            let cap =
                                table.as_ref().and_then(|t| t.payoff_cap(b, s)).map_or(caps[i], |c| c.min(caps[i]));
                            sum = sum.saturating_add(cap);
                        }
                        self.rec.clock.lap(resume);
                        Ok(ControlFlow::Continue(sum))
                    };
                    live.simulate_orders_bounded_recorded_partial(
                        master,
                        &orders,
                        LiveRandom::new(0),
                        capture_budget,
                        stop_below,
                        CUTOFF_EVERY,
                        upper,
                        &mut visit,
                    )?
                }
                _ => {
                    let (shared, programs) =
                        live.simulate_orders_recorded(master, &orders, LiveRandom::new(0), capture_budget, |i, m| {
                            visit(i, m).map(|_| ())
                        })?;
                    (OrdersOutcome::Complete(shared), programs)
                }
            }
        };
        if let Some(started) = recording_started {
            self.tel.caches.program_recording_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        if let Some(programs) = programs {
            self.programs.insert_partial(
                physical.members,
                &performers,
                &missing_orders,
                programs,
                &mut self.tel.caches.programs,
            );
            self.tel.caches.program_bytes = self.programs.allocated_bytes();
            (self.tel.caches.program_recorded_nodes, self.tel.caches.program_recorded_bytes) =
                self.programs.recorded_work();
        }
        let (OrdersOutcome::Complete(shared) | OrdersOutcome::Stopped(shared) | OrdersOutcome::Interrupted(shared)) =
            outcome;
        self.tel.leaves.order_tree.add(&shared);
        self.tel.leaves.simulations += (outcomes.iter().filter(|o| o.is_some()).count() - cached_count) as u64;
        match outcome {
            OrdersOutcome::Complete(_) => {
                self.rec.clock.lap(resume);
                let evaluation =
                    expectation::aggregate(outcomes.into_iter().map(|o| o.expect("every order evaluated")).collect())?;
                self.team_scores.insert(physical, power, &evaluation, &final_lives, &mut self.tel.caches.team_scores);
                Ok(Leaf::Evaluated(evaluation))
            }
            OrdersOutcome::Stopped(s) => {
                self.rec.clock.lap_as(slot::STOPPED, resume);
                telemetry::record_stop(&mut self.tel.leaves.cutoff, s.frames as usize, s.separate_frames as usize);
                self.tel.leaves.order_bound_pruned += 1;
                Ok(Leaf::Pruned)
            }
            OrdersOutcome::Interrupted(_) => {
                self.rec.clock.lap(resume);
                Ok(Leaf::Stopped)
            }
        }
    }

    /// Census leaf (`snaps::census`): the team's cheap, then raw and fine caps against the fixed threshold; a team
    /// they leave at or above it is counted, not simulated.
    fn census_leaf(
        &mut self,
        physical: &PhysicalDeck,
        power: i32,
        cut: Option<(&crate::search::joint::JointBounds, &crate::domain::CandidateDomain)>,
        threshold: i128,
    ) -> Result<Leaf, Error> {
        let below = |total: i128| total < threshold;
        let census = self.tel.leaves.census.get_or_insert_with(|| telemetry::Census::new(threshold));
        let Some((b, domain)) = cut else {
            census.unbounded += 1;
            return Ok(Leaf::Pruned);
        };
        let mut caps = b.order_cheap_caps(domain, physical, i64::from(power), &self.positions);
        let cheap = cap_sum(&caps);
        if below(cheap) {
            self.tel.leaves.cheap_pruned += 1;
            return Ok(Leaf::Pruned);
        }
        if b.has_fine() {
            let (_, resume) = self.rec.clock.lap(slot::FINE);
            let pruned = b.tighten_order_caps_until(
                domain,
                physical,
                i64::from(power),
                &self.positions,
                &mut caps,
                &mut self.bound_scratch,
                below,
                &mut self.tel.leaves.fine_orders,
            );
            self.rec.clock.lap(resume);
            if pruned {
                self.tel.leaves.fine_pruned += 1;
                return Ok(Leaf::Pruned);
            }
        }
        let fine = cap_sum(&caps);
        let team = telemetry::CensusTeam {
            members: physical.members.map(|i| self.pool.members[i].id),
            snaps: physical.snaps.map(|i| i.map(|i| self.pool.snaps[i].id)),
            power,
            cheap: cheap.to_string(),
            fine: fine.to_string(),
        };
        let census = self.tel.leaves.census.as_mut().expect("census started");
        census.record(physical.members, team, cheap, fine);
        Ok(Leaf::Pruned)
    }
}
