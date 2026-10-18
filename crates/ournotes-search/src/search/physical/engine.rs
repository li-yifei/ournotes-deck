//! The search engine: the proposals considered so far, the top list, the budget and the telemetry.

use super::*;

pub(super) struct Engine<'a, 'm> {
    /// How the payoffs of a team's performance orders combine into the team's value.
    pub(super) aggregation: Aggregation,
    pub(super) pool: &'a Pool<'m>,
    pub(super) request: &'a SearchRequest,
    pub(super) metric: &'a Metric,
    pub(super) event_input: Option<&'a EventPayoffInput>,
    pub(super) simulation: &'a SimulationInput,
    pub(super) limits: &'a Limits,
    pub(super) budget: crate::search::budget::SearchBudget,
    pub(super) stop: Option<ExitReason>,
    pub(super) tel: Telemetry,
    pub(super) rec: Recorder,
    /// Correlated and resource node bounds are worth their cost for the current search part.
    pub(super) correlated: bool,
    /// Per depth, the recent checks and prunes of the correlated node bound (see `joint_search::correlated_due`).
    pub(super) correlated_recent: [[u64; 2]; 5],
    pub(super) resource: bool,
    pub(super) bound_scratch: crate::search::snaps::JointScratch,
    pub(super) bonus_scratch: crate::search::joint::BonusScratch,
    /// Per depth of the joint traversal, the order-step bound parts of the node there (see `joint::OrderSteps`).
    pub(super) order_steps: [crate::search::joint::OrderSteps; 6],
    pub(super) top: Vec<Entry>,
    pub(super) certified: Option<CertifiedState<'m>>,
    /// The master's lottery-related skills when the decks play lottery-free ([`certified_engine::LotteryMode::Free`]).
    pub(super) lottery_free: Option<std::sync::Arc<ournotes_sim::live::full::LuckSkills>>,
    /// The same skills for the rotation splice of lottery-free orders when the decks play as they are.
    pub(super) splice_skills: Option<std::sync::Arc<ournotes_sim::live::full::LuckSkills>>,
    pub(super) seen: HashSet<PhysicalDeck>,
    pub(super) fifo: VecDeque<PhysicalDeck>,
    pub(super) team_scores: team_scores::TeamScores,
    pub(super) programs: program_cache::ProgramCache,
    pub(super) song: Option<&'a ournotes_sim::cards::SongView>,
    pub(super) event: bool,
    pub(super) skip: Option<&'a ournotes_sim::live::skip::SkipEvaluator>,
    /// Played lives: decks are teams in their canonical layout (`uniform::canonical`).
    pub(super) live: bool,
    /// The performance orders in [`uniform::all_orders`] order, and the positions of the slots in each.
    pub(super) orders: Vec<[usize; 5]>,
    pub(super) positions: Vec<[usize; 5]>,
    /// Decks the warm start evaluated; the traversal treats them as considered (see `warm.rs`).
    pub(super) seeded: HashSet<PhysicalDeck>,
    /// Visit order of the next root loop of `joint_rec`; None keeps the static choice order.
    pub(super) root_order: Option<warm::RootOrder>,
    /// Whole-domain context of the warm start and of polishing new best decks (joint searches only).
    pub(super) warm: Option<warm::Warm<'a>>,
    /// Optional progress reports (`progress.rs`).
    pub(super) progress: Option<progress::Reporter<'a>>,
    /// What the last `consider` evaluated (None: it evaluated nothing).
    pub(super) offered: Option<Offered>,
}

/// An evaluated deck: its mean payoff (numerator, denominator; None when a certified interval does not settle it),
/// power, and highest order score (exact evaluations).
#[derive(Clone, Copy, Debug)]
pub(super) struct Offered {
    pub(super) payoff: Option<(i128, u128)>,
    pub(super) power: i32,
    pub(super) score: Option<i32>,
}

impl Engine<'_, '_> {
    /// Formal deterministic objectives also retain the leader and four unordered member/Snap pairs.
    /// The explicit v1 session cursor and fixed deterministic evaluation keep their physical-deck contracts.
    pub(super) fn team_identity(&self) -> bool {
        self.team_identity_in(self.tel.environment.traversal)
    }
    /// [`Engine::team_identity`] of a search that ran `traversal`.
    fn team_identity_in(&self, traversal: Traversal) -> bool {
        self.live
            || (self.skip.is_some()
                && matches!(
                    self.metric,
                    Metric::ClientEventPoints { .. }
                        | Metric::ClientChallengePoints { .. }
                        | Metric::RankedEventItems { .. }
                        | Metric::Combined { .. }
                ))
            || (!matches!(traversal, Traversal::Session | Traversal::Fixed)
                && crate::search::team_power::applies(&self.request.objective, self.metric))
    }

    pub(super) fn expired(&mut self) -> bool {
        if self.stop.is_some() {
            return true;
        }
        if self.budget.deadline().is_none() && self.progress.is_none() && !crate::cancel::requested() {
            return false;
        }
        // One clock read serves the deadline and the progress reports.
        self.expired_at(crate::search::budget::now())
    }
    /// `expired` with a clock value the caller has just read; a progress report is due at the same checks.
    pub(super) fn expired_at(&mut self, now: Instant) -> bool {
        if self.stop.is_some() {
            return true;
        }
        if self.budget.expired_at(now) {
            self.stop = Some(ExitReason::TimeLimit);
            return true;
        }
        self.progress_at(now);
        false
    }
    pub(super) fn remember(&mut self, p: PhysicalDeck) {
        if self.limits.cache_entries == 0 {
            return;
        }
        let cache = &mut self.tel.caches.candidates;
        if self.seen.len() >= self.limits.cache_entries
            && let Some(old) = self.fifo.pop_front()
        {
            self.seen.remove(&old);
            cache.evictions += 1;
        }
        self.seen.insert(p);
        self.fifo.push_back(p);
        cache.peak_entries = cache.peak_entries.max(self.seen.len());
    }
    pub(super) fn payoff(
        &self,
        p: &PhysicalDeck,
        score: i32,
        power: i32,
        final_life: Option<i32>,
    ) -> Result<i128, Error> {
        payoff_of(self.pool, self.request, self.metric, self.event_input, p, score, power, final_life)
    }
    /// The expectation numerator of each term of a combined metric over the evaluation's score distribution.
    fn term_payoffs(
        &self,
        p: &PhysicalDeck,
        evaluation: &FiniteEvaluation,
        power: i32,
    ) -> Result<Option<Vec<Option<i128>>>, Error> {
        let Some(terms) = self.metric.terms() else {
            return Ok(None);
        };
        // Maximum aggregation values a team by its best order alone: each term pays what that order scores.
        let masses: Vec<(i32, u128)> = if self.aggregation == Aggregation::Maximum && self.live {
            evaluation
                .outcomes
                .iter()
                .reduce(|best, o| if o.terminal_payoff > best.terminal_payoff { o } else { best })
                .map(|o| vec![(o.final_score, 1)])
                .unwrap_or_default()
        } else {
            evaluation.score_mass.iter().map(|(&score, &mass)| (score, mass)).collect()
        };
        terms
            .map(|term| {
                if matches!(term.metric, Metric::ScoreAndLifeAtLeast { .. }) {
                    return Ok(None);
                }
                masses
                    .iter()
                    .try_fold(0i128, |sum, &(score, mass)| {
                        let payoff =
                            payoff_of(self.pool, self.request, &term.metric, self.event_input, p, score, power, None)?;
                        let mass = i128::try_from(mass).map_err(|_| arithmetic())?;
                        payoff.checked_mul(mass).and_then(|value| sum.checked_add(value)).ok_or_else(arithmetic)
                    })
                    .map(Some)
            })
            .collect::<Result<Vec<_>, Error>>()
            .map(Some)
    }

    pub(super) fn consider(&mut self, physical: PhysicalDeck) -> Result<bool, Error> {
        self.consider_with(physical, None)
    }

    /// Evaluate a candidate deck (a team in its canonical layout for played lives) and offer it to the Top-K. `cut`
    /// supplies the bounds whose per-order caps stop evaluating a team that cannot reach the Top-K. Ok(false) when
    /// the budget or the candidate limit ran out.
    pub(super) fn consider_with(
        &mut self,
        physical: PhysicalDeck,
        cut: Option<(&crate::search::joint::JointBounds, &crate::domain::CandidateDomain)>,
    ) -> Result<bool, Error> {
        self.consider_within(physical, cut, None)
    }

    /// [`Engine::consider_with`] for a deck whose power is at most `power_upper`, when known: its caps at that power
    /// may drop it before its power is computed ([`Engine::below_at_power_bound`]). The canonical layout keeps the
    /// leader and the member and Snap of every slot, so it keeps the power too.
    pub(super) fn consider_within(
        &mut self,
        physical: PhysicalDeck,
        cut: Option<(&crate::search::joint::JointBounds, &crate::domain::CandidateDomain)>,
        power_upper: Option<i64>,
    ) -> Result<bool, Error> {
        self.tel.leaves.proposed += 1;
        self.offered = None;
        if self.expired() {
            return Ok(false);
        }
        let physical = if self.team_identity() { uniform::canonical(self.pool, &physical) } else { physical };
        self.tel.caches.candidates.lookups += 1;
        if self.seen.contains(&physical)
            || self.seeded.contains(&physical)
            || self.top.iter().any(|e| e.physical == physical)
            || self.certified.as_ref().is_some_and(|s| s.contains(&physical))
        {
            self.tel.caches.candidates.hits += 1;
            return Ok(true);
        }
        if self.limits.max_candidates.is_some_and(|n| self.tel.leaves.visited >= n) {
            self.stop = Some(ExitReason::CandidateLimit);
            return Ok(false);
        }
        self.tel.leaves.visited += 1;
        if let (Some(upper), Some(cut)) = (power_upper, cut)
            && self.below_at_power_bound(&physical, upper, cut)
        {
            self.remember(physical);
            return Ok(true);
        }
        let power = self.pool.deck_power(&physical.as_deck(), self.song, self.event)?.power();
        let evaluation = if self.live {
            // A certified frontier reduces the orders by its own reduction ([`CertifiedState::reduction`]).
            let leaf = if self.aggregation == Aggregation::Maximum && self.certified.is_none() {
                self.evaluate_maximum(&physical, power, cut)?
            } else {
                self.evaluate_orders(&physical, power, cut)?
            };
            match leaf {
                Leaf::Evaluated(evaluation) => evaluation,
                Leaf::Certified(evaluation, program, payoff) => {
                    self.offer_certified(physical, power, *evaluation, program, payoff)?;
                    return Ok(true);
                }
                Leaf::Pruned => {
                    self.remember(physical);
                    return Ok(true);
                }
                Leaf::Stopped => {
                    self.tel.leaves.partial += 1;
                    return Ok(false);
                }
            }
        } else {
            let score = match &self.skip {
                Some(skip) => skip.score(power).0,
                None => power,
            };
            let payoff = self.payoff(&physical, score, power, None)?;
            expectation::aggregate(vec![SeedOutcome {
                root_seed: 0,
                weight: 1,
                performance_order: [0, 1, 2, 3, 4],
                final_score: score,
                terminal_payoff: payoff,
            }])?
        };
        self.tel.leaves.evaluated += 1;
        self.remember(physical);
        self.offered = Some(Offered {
            payoff: Some((evaluation.expected_payoff.numerator, evaluation.expected_payoff.denominator)),
            power,
            score: evaluation.score_mass.last_key_value().map(|(&score, _)| score),
        });
        let mut entry = Entry {
            terms: None,
            physical,
            members: physical.members.map(|i| self.pool.members[i].id),
            snaps: physical.snaps.map(|i| i.map(|i| self.pool.snaps[i].id)),
            power,
            evaluation,
        };
        let pos = self.top.iter().position(|e| compare(&entry, e) == Ordering::Less).unwrap_or(self.top.len());
        // A strictly higher best payoff (not a power or ID tie-break) starts a polish round below.
        let better = self
            .top
            .first()
            .is_none_or(|best| entry.evaluation.expected_payoff.numerator > best.evaluation.expected_payoff.numerator);
        if pos < self.request.k {
            entry.terms = self.term_payoffs(&entry.physical, &entry.evaluation, entry.power)?;
            self.top.insert(pos, entry);
            self.top.truncate(self.request.k);
            let (best, kth, filled) = self.standing();
            self.rec.incumbent(&mut self.tel, best, kth, filled);
            if better {
                self.polish()?;
            }
            self.report_progress();
        }
        self.tel.leaves.peak_retained = self.tel.leaves.peak_retained.max(self.top.len());
        Ok(true)
    }

    /// The evaluation of a deck the Top-K (or the certified frontier) still holds.
    pub(super) fn recorded(&self, p: &PhysicalDeck) -> Option<Offered> {
        if let Some(state) = &self.certified {
            let (payoff, power) = state.evaluated(p)?;
            return Some(Offered { payoff: payoff.map(|x| (x.numerator, x.denominator)), power, score: None });
        }
        self.top.iter().find(|t| t.physical == *p).map(|t| Offered {
            payoff: Some((t.evaluation.expected_payoff.numerator, t.evaluation.expected_payoff.denominator)),
            power: t.power,
            score: t.evaluation.score_mass.last_key_value().map(|(&score, _)| score),
        })
    }

    /// Best and K-th payoff numerators, and the decks held.
    pub(super) fn standing(&self) -> (i128, Option<i128>, usize) {
        let numerator = |e: &Entry| e.evaluation.expected_payoff.numerator;
        let kth = (self.top.len() == self.request.k).then(|| numerator(self.top.last().expect("K decks")));
        (self.top.first().map_or(0, numerator), kth, self.top.len())
    }

    /// The stop leaves this joint node's whole subtree unexplored.
    pub(super) fn unexplored_node(
        &mut self,
        p: &PhysicalDeck,
        depth: usize,
        domain: &crate::domain::CandidateDomain,
        bounds: &crate::search::joint::JointBounds,
        orders: &[([usize; 5], u128)],
    ) -> Result<(), Error> {
        if self.stop.is_none() {
            return Ok(());
        }
        let started = self.bound_start();
        let upper = if depth == 0 {
            joint_root_upper(self.pool, domain, bounds, orders, 0)?
        } else {
            Some(node_upper(self.pool, domain, bounds, p, depth, orders)?.0)
        };
        self.fold_unexplored(upper, started);
        Ok(())
    }

    /// The stop leaves the choices of this joint node from `offset` on unexplored; `root` is the bound order the
    /// root loop follows, if any.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn unexplored_choices(
        &mut self,
        p: &PhysicalDeck,
        depth: usize,
        offset: usize,
        tail: Option<(&crate::search::joint::JointBounds, &crate::search::joint::TailState)>,
        root: Option<&warm::RootOrder>,
        domain: &crate::domain::CandidateDomain,
        bounds: &crate::search::joint::JointBounds,
        orders: &[([usize; 5], u128)],
    ) -> Result<(), Error> {
        let choices = root.map_or(bounds.choices.len(), |r| r.children.len());
        if self.stop.is_none() || offset >= choices {
            return Ok(());
        }
        let started = self.bound_start();
        // Each bound covers every child at or after `offset` (the node bound covers all of its children; an ordered
        // root's cover is the largest depth-1 bound from its child on).
        let upper = match (tail, root) {
            (_, Some(r)) => Some(r.covers[offset].0),
            (Some((tail_bounds, state)), None) => Some(tail_bounds.tail_upper(state, offset)?.0),
            (None, None) if depth > 0 => Some(node_upper(self.pool, domain, bounds, p, depth, orders)?.0),
            (None, None) => joint_root_upper(self.pool, domain, bounds, orders, offset)?,
        };
        self.fold_unexplored(upper, started);
        Ok(())
    }

    /// Start bounding what the stop leaves unexplored. Its time goes to `proof.boundMs` and `otherMs`, not to the
    /// search activity the stop interrupted; pass the returned mark to `fold_unexplored`.
    pub(super) fn bound_start(&mut self) -> (Instant, usize) {
        self.rec.clock.lap(slot::OTHER)
    }

    /// The branches still open are bounded by `open` (None: no branch open): offer the larger of it and the best
    /// payoff as the global upper bound. Only a traversal of the whole remaining domain (one search part) offers it.
    pub(super) fn note_open(&mut self, open: Option<i128>) {
        // The interval frontier has no scalar Top-K; `open` alone would omit its retained candidates.
        if self.rec.parts != 1 || self.certified.is_some() {
            return;
        }
        let best = self.top.first().map(|e| e.evaluation.expected_payoff.numerator);
        if let Some(upper) = best.max(open) {
            self.rec.offer_upper(upper);
        }
    }

    pub(super) fn fold_unexplored(&mut self, upper: Option<i128>, (started, resume): (Instant, usize)) {
        if let Some(upper) = upper {
            self.rec.unexplored(upper);
        }
        let (now, _) = self.rec.clock.lap(resume);
        self.tel.proof.bound_ms += now.saturating_duration_since(started).as_secs_f64() * 1000.0;
    }

    /// Close the recording into the wire document.
    /// `standing` is the final Top-K's (best, K-th, decks held), taken before the results leave the engine.
    pub(super) fn finish_telemetry(&mut self, standing: (i128, Option<i128>, usize)) {
        self.rec.end_open(&mut self.tel);
        self.rec.clock.add_to(&mut self.tel.time);
        let complete = self.stop.is_none();
        if self.certified.is_none() {
            // `top` has already moved into the results. Its saved standing, not the now-empty vector, covers the
            // evaluated domain. An empty result supplies no scalar best, including no synthetic zero.
            let best = (standing.2 > 0).then_some(standing.0);
            if complete {
                // Every part has closed: the exact best is the whole-domain optimum, even with several parts or
                // an exhaustive traversal that offered no running bound.
                self.rec.upper = best;
            } else if self.rec.bounded {
                // Unwinding folded all remaining branches, including a pool-wide bound for any later parts.
                // Their maximum with the saved incumbent covers the whole domain; no new bounds are computed here.
                if let Some(upper) = best.max(self.rec.unexplored) {
                    self.rec.offer_upper(upper);
                }
            }
        }
        let mut tel = std::mem::take(&mut self.tel);
        self.close_telemetry(&mut tel, standing, true);
        self.tel = tel;
    }

    /// The counters kept outside the document, the last incumbent and the proof. `stopped` is true when the search
    /// has ended (completed or stopped, with what a stop leaves unexplored bounded); a progress report is neither.
    fn close_telemetry(&self, tel: &mut Telemetry, standing: (i128, Option<i128>, usize), stopped: bool) {
        (tel.caches.bonus_rows, tel.caches.bonus_rows_refused) = self.bonus_scratch.cache_use();
        tel.caches.rush_windows = self.bound_scratch.rush_windows();
        let (best, kth, filled) = standing;
        self.rec.close_timeline(tel, best, kth, filled);
        let complete = stopped && self.stop.is_none();
        let proof = &mut tel.proof;
        proof.complete = complete;
        proof.parts = self.rec.parts;
        proof.parts_done = if complete { self.rec.parts } else { self.rec.parts_done };
        proof.fraction = if complete { Some(1.0) } else { self.rec.progress() };
        if !complete
            && self.rec.tracked
            && let Some((done, total)) = self.rec.frontier.top_level()
        {
            (proof.top_level_done, proof.top_level_total) = (Some(done), Some(total));
        }
        // The proof reads in the payoff of the results: a combined metric's first level.
        let unit = self.metric.primary_unit();
        let (best, kth) = (best.div_euclid(unit), kth.map(|v| v.div_euclid(unit)));
        let unexplored = self.rec.unexplored.map(|v| v.div_euclid(unit));
        if filled > 0 {
            proof.best = Some(best.to_string());
        }
        proof.kth = kth.map(|v| v.to_string());
        tel.memory = telemetry::Memory::now();
        proof.global_upper_bound = self.rec.upper.map(|v| v.div_euclid(unit).to_string());
        if stopped && !complete && self.rec.bounded {
            proof.upper_bound = unexplored.map(|v| v.to_string());
            let gap = |x: i128| match unexplored {
                Some(upper) => telemetry::gap(upper, x),
                None => Some(0.0),
            };
            proof.best_gap = (filled > 0).then(|| gap(best)).flatten();
            proof.kth_gap = kth.and_then(gap);
        }
        if self.certified.is_some() {
            // Scalar incumbent fields are exact numerators, not interval endpoints. The result carries bounds.
            proof.best = None;
            proof.kth = None;
            proof.best_gap = None;
            proof.kth_gap = None;
            if self.stop == Some(ExitReason::RefinementRequired) {
                proof.fraction = None;
            }
        }
    }

    /// Decks of the Top-K the warm start evaluated first.
    pub(super) fn seeded_in_top(&self) -> usize {
        self.top.iter().filter(|t| self.seeded.contains(&t.physical)).count()
    }

    /// The result ended by `exit_reason` with these decks and telemetry, `elapsed` after the search start; `fixed`
    /// marks the evaluation of one requested deck. The final result and the progress reports share it.
    pub(super) fn outcome(
        &self,
        strategy: &Strategy,
        exit_reason: ExitReason,
        fixed: bool,
        results: Vec<RecommendedDeck>,
        telemetry: Telemetry,
        elapsed: Duration,
    ) -> RecommendationOutcome {
        let proven = exit_reason == ExitReason::Exhausted;
        let optimality = if fixed {
            Optimality::NotApplicable
        } else if proven {
            Optimality::Proven
        } else if matches!(strategy, Strategy::Candidate { .. }) {
            Optimality::Heuristic
        } else {
            Optimality::Unproven
        };
        let live = matches!(self.request.objective.inner(), Objective::LiveScore { .. });
        let maximum = self.aggregation == Aggregation::Maximum;
        let lottery = if self.certified.is_some() {
            "certifiedNativeLotteryIntervals"
        } else if self.lottery_free.is_some() {
            "noLuckRange"
        } else {
            "none"
        };
        let probability_law = if live && maximum {
            serde_json::json!({"kind":"maximumOverMemberOrders","orders":ORDERS,
                "orderValue":"terminalPayoffAtExpectedScore","lottery":lottery})
        } else if live {
            serde_json::json!({"kind":"uniformMemberOrder","orders":ORDERS,"lottery":lottery})
        } else {
            serde_json::json!({"kind":"deterministic"})
        };
        RecommendationOutcome {
            format: RESULT_FORMAT,
            completion: if proven {
                Completion::Complete
            } else if exit_reason == ExitReason::RefinementRequired {
                Completion::RefinementRequired
            } else {
                Completion::TimedOut
            },
            optimality,
            exit_reason,
            result_identity: match (fixed, self.team_identity_in(telemetry.environment.traversal)) {
                (true, true) => "fixedTeam",
                (true, false) => "fixedPhysicalDeck",
                (false, true) => "team",
                (false, false) => "physicalDeck",
            },
            metric: self.metric.clone(),
            aggregation: self.aggregation,
            maximum_model: maximum.then(|| MaximumModel::for_execution(live)),
            player_goal: None,
            strategy: strategy.clone(),
            probability_law,
            proof_scope: if live && maximum {
                "conditional on declared master, roster and complete judgement/clock inputs, with the value of a team its best performance order, each order worth the terminal payoff at its expected score over random draws; client counters are not server reward authority"
            } else {
                "conditional on declared master, roster and complete judgement/clock inputs, with the five members performing in a uniformly random order; client counters are not server reward authority"
            },
            resolved_context: serde_json::Value::Null,
            results,
            telemetry,
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        }
    }

    /// A progress report at `now`: the result the search would return if its time limit expired now. The telemetry
    /// so far is closed on a copy (the recording stays open); it has no bound of the unexplored part, which only a
    /// stop computes.
    pub(super) fn report_outcome(
        &self,
        strategy: &Strategy,
        start: Instant,
        now: Instant,
    ) -> Result<RecommendationOutcome, Error> {
        let mut tel = self.tel.clone();
        tel.incumbents.warm_start.final_top_k = self.seeded_in_top();
        self.rec.peek_open(&mut tel, now);
        self.rec.clock.peek_into(now, &mut tel.time);
        self.close_telemetry(&mut tel, self.standing(), false);
        let results = if self.certified.is_some() {
            self.certified_results(false)?.0
        } else {
            self.top
                .iter()
                .cloned()
                .map(|e| e.wire(self.metric, self.aggregation, self.live))
                .collect::<Result<Vec<_>, _>>()?
        };
        let elapsed = now.saturating_duration_since(start);
        Ok(self.outcome(strategy, ExitReason::TimeLimit, false, results, tel, elapsed))
    }
}
