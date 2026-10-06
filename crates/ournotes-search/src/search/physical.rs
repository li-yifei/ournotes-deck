//! Deck search: deterministic power/skip search and the exact search of played lives under the uniform member-order
//! target (`super::uniform`). Candidate proposals are heuristic; every returned value is evaluated exactly under the
//! declared model. Only exhaustion/proven pruning certifies K.

use super::expectation::{self, FiniteEvaluation, FiniteSeedContext, PhysicalDeck, SeedOutcome};
use super::telemetry::{self, Recorder, Telemetry, Traversal, slot};
use super::uniform::{self, ORDERS};
use super::{Completion, Objective, Pool, SearchRequest};
use crate::clock::Instant;
use crate::handler::{BuiltProblem, build_card_pool, reject_unsupported_lifecycle, validate};
use crate::types::*;
use ournotes_sim::replay::RankConfirmation;
use ournotes_sim::scenario::EventPayoffInput;
use ournotes_sim::{Error, cards::Roster, data::DeckData};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::Duration;

#[path = "composition.rs"]
mod composition;
#[path = "leaf.rs"]
mod leaf;
use leaf::Leaf;
#[path = "certified_engine.rs"]
mod certified_engine;
#[cfg(not(target_arch = "wasm32"))]
#[path = "luck_seed.rs"]
mod luck_seed;
use certified_engine::CertifiedState;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn needs_certified_frontier(built: &BuiltProblem<'_>) -> Result<bool, Error> {
    Ok(certified_engine::lottery_mode(built.pool(), &built.context.request, built.domain())?
        == certified_engine::LotteryMode::Certified)
}
#[path = "deck_payoff_search.rs"]
mod deck_payoff_search;
#[path = "program_cache.rs"]
mod program_cache;
#[path = "progress.rs"]
mod progress;
#[path = "session.rs"]
mod session;
#[path = "team_power_search.rs"]
mod team_power_search;
#[path = "team_scores.rs"]
mod team_scores;
#[path = "warm.rs"]
mod warm;
pub(crate) use progress::ProgressHook;
pub use session::{
    GOAL_SPEC_VERSION, GoalSpec, SESSION_FORMAT, SearchSession, SessionBinding, SessionProgress, SessionStatus,
    StepBudget,
};

pub(crate) fn session_start_clock() -> Instant {
    super::budget::now()
}

#[derive(Clone)]
struct Entry {
    physical: PhysicalDeck,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    power: i32,
    evaluation: FiniteEvaluation,
}
fn compare(a: &Entry, b: &Entry) -> Ordering {
    b.evaluation
        .expected_payoff
        .numerator
        .cmp(&a.evaluation.expected_payoff.numerator)
        .then_with(|| b.power.cmp(&a.power))
        .then_with(|| a.members.cmp(&b.members))
        .then_with(|| a.snaps.cmp(&b.snaps))
}
impl Entry {
    fn wire(self, metric: &Metric) -> Result<RecommendedDeck, Error> {
        if matches!(metric, Metric::Power) {
            return Ok(RecommendedDeck {
                members: self.members,
                snaps: self.snaps,
                power: self.power,
                expected_score: None,
                expected_payoff: Some(self.evaluation.expected_payoff.into()),
                score_interval: None,
                payoff_interval: None,
                rank_certified: None,
                score_summary: None,
                best_order: None,
                order_outcomes: Vec::new(),
            });
        }
        let score_summary = score_summary(&self.evaluation.score_mass, metric.target())?;
        // The first order with the highest payoff, then the highest score (orders in lexicographic order).
        let best_order = (self.evaluation.outcomes.len() == ORDERS)
            .then(|| {
                self.evaluation.outcomes.iter().reduce(|best, o| {
                    if (o.terminal_payoff, o.final_score) > (best.terminal_payoff, best.final_score) { o } else { best }
                })
            })
            .flatten()
            .map(|o| OrderResult {
                performance_order: o.performance_order,
                members: o.performance_order.map(|slot| self.members[slot]),
                score: o.final_score,
                payoff: o.terminal_payoff.to_string(),
            });
        Ok(RecommendedDeck {
            members: self.members,
            snaps: self.snaps,
            power: self.power,
            expected_score: Some(self.evaluation.expected_score.into()),
            expected_payoff: Some(self.evaluation.expected_payoff.into()),
            score_interval: None,
            payoff_interval: None,
            rank_certified: None,
            score_summary: Some(score_summary),
            best_order,
            order_outcomes: if self.evaluation.outcomes.len() == ORDERS {
                self.evaluation
                    .outcomes
                    .iter()
                    .map(|o| (o.performance_order, o.final_score, o.terminal_payoff))
                    .collect()
            } else {
                Vec::new()
            },
        })
    }
}

struct Engine<'a, 'm> {
    pool: &'a Pool<'m>,
    request: &'a SearchRequest,
    metric: &'a Metric,
    event_input: Option<&'a EventPayoffInput>,
    simulation: &'a SimulationInput,
    limits: &'a Limits,
    budget: super::budget::SearchBudget,
    stop: Option<ExitReason>,
    tel: Telemetry,
    rec: Recorder,
    /// Correlated and resource node bounds are worth their cost for the current search part.
    correlated: bool,
    resource: bool,
    bound_scratch: super::snaps::JointScratch,
    bonus_scratch: super::joint::BonusScratch,
    /// Per depth of the joint traversal, the order-step bound parts of the node there (see `joint::OrderSteps`).
    order_steps: [super::joint::OrderSteps; 6],
    top: Vec<Entry>,
    certified: Option<CertifiedState>,
    /// The master's lottery-related skills when the decks play lottery-free ([`certified_engine::LotteryMode::Free`]).
    lottery_free: Option<std::sync::Arc<ournotes_sim::live::full::LuckSkills>>,
    seen: HashSet<PhysicalDeck>,
    fifo: VecDeque<PhysicalDeck>,
    team_scores: team_scores::TeamScores,
    programs: program_cache::ProgramCache,
    song: Option<&'a ournotes_sim::cards::SongView>,
    event: bool,
    skip: Option<&'a ournotes_sim::live::skip::SkipEvaluator>,
    /// Played lives: decks are teams in their canonical layout (`uniform::canonical`).
    live: bool,
    /// The performance orders in [`uniform::all_orders`] order, and the positions of the slots in each.
    orders: Vec<[usize; 5]>,
    positions: Vec<[usize; 5]>,
    /// Decks the warm start evaluated; the traversal treats them as considered (see `warm.rs`).
    seeded: HashSet<PhysicalDeck>,
    /// Visit order of the next root loop of `joint_rec`; None keeps the static choice order.
    root_order: Option<warm::RootOrder>,
    /// Whole-domain context of the warm start and of polishing new best decks (joint searches only).
    warm: Option<warm::Warm<'a>>,
    /// Optional progress reports (`progress.rs`).
    progress: Option<progress::Reporter<'a>>,
    /// What the last `consider` evaluated (None: it evaluated nothing).
    offered: Option<Offered>,
}

/// An evaluated deck: its mean payoff (numerator, denominator; None when a certified interval does not settle it),
/// power, and highest order score (exact evaluations).
#[derive(Clone, Copy, Debug)]
struct Offered {
    payoff: Option<(i128, u128)>,
    power: i32,
    score: Option<i32>,
}
impl Engine<'_, '_> {
    /// Formal deterministic objectives also retain the leader and four unordered member/Snap pairs.
    /// The explicit v1 session cursor and fixed deterministic evaluation keep their physical-deck contracts.
    fn team_identity(&self) -> bool {
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
                        | Metric::ConditionalClientEventItems { .. }
                ))
            || (!matches!(traversal, Traversal::Session | Traversal::Fixed)
                && super::team_power::applies(&self.request.objective, self.metric))
    }

    fn expired(&mut self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if crate::parallel::cancelled() {
            self.stop = Some(ExitReason::TimeLimit);
            return true;
        }
        if self.stop.is_some() {
            return true;
        }
        if self.budget.deadline().is_none() && self.progress.is_none() {
            return false;
        }
        // One clock read serves the deadline and the progress reports.
        self.expired_at(super::budget::now())
    }
    /// `expired` with a clock value the caller has just read; a progress report is due at the same checks.
    fn expired_at(&mut self, now: Instant) -> bool {
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
    fn remember(&mut self, p: PhysicalDeck) {
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
    fn payoff(&self, p: &PhysicalDeck, score: i32, power: i32, final_life: Option<i32>) -> Result<i128, Error> {
        payoff_of(self.pool, self.request, self.metric, self.event_input, p, score, power, final_life)
    }
    fn consider(&mut self, physical: PhysicalDeck) -> Result<bool, Error> {
        self.consider_with(physical, None)
    }

    /// Evaluate a candidate deck (a team in its canonical layout for played lives) and offer it to the Top-K. `cut`
    /// supplies the bounds whose per-order caps stop evaluating a team that cannot reach the Top-K. Ok(false) when
    /// the budget or the candidate limit ran out.
    fn consider_with(
        &mut self,
        physical: PhysicalDeck,
        cut: Option<(&super::joint::JointBounds, &crate::domain::CandidateDomain)>,
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
        #[cfg(not(target_arch = "wasm32"))]
        if !crate::parallel::visit() {
            self.stop = Some(ExitReason::CandidateLimit);
            return Ok(false);
        }
        self.tel.leaves.visited += 1;
        let power = self.pool.deck_power(&physical.as_deck(), self.song, self.event)?.power();
        let evaluation = if self.live {
            match self.evaluate_orders(&physical, power, cut)? {
                Leaf::Evaluated(evaluation) => evaluation,
                Leaf::Certified(evaluation, program, payoff) => {
                    self.offer_certified(physical, power, evaluation, program, payoff)?;
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
        let entry = Entry {
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
            #[cfg(not(target_arch = "wasm32"))]
            if crate::parallel::sharing() {
                crate::parallel::publish(entry.clone().wire(self.metric)?)?;
            }
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
    fn recorded(&self, p: &PhysicalDeck) -> Option<Offered> {
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
    fn standing(&self) -> (i128, Option<i128>, usize) {
        let numerator = |e: &Entry| e.evaluation.expected_payoff.numerator;
        let kth = (self.top.len() == self.request.k).then(|| numerator(self.top.last().expect("K decks")));
        (self.top.first().map_or(0, numerator), kth, self.top.len())
    }

    /// The stop leaves this joint node's whole subtree unexplored.
    fn unexplored_node(
        &mut self,
        p: &PhysicalDeck,
        depth: usize,
        domain: &crate::domain::CandidateDomain,
        bounds: &super::joint::JointBounds,
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
    fn unexplored_choices(
        &mut self,
        p: &PhysicalDeck,
        depth: usize,
        offset: usize,
        tail: Option<(&super::joint::JointBounds, &super::joint::TailState)>,
        root: Option<&warm::RootOrder>,
        domain: &crate::domain::CandidateDomain,
        bounds: &super::joint::JointBounds,
        orders: &[([usize; 5], u128)],
    ) -> Result<(), Error> {
        let choices = root.map_or(bounds.choices.len(), |r| r.children.len());
        if self.stop.is_none() || offset >= choices {
            return Ok(());
        }
        let started = self.bound_start();
        // Each bound covers every child at or after `offset` (the node bound covers all of its children; ordered root
        // children are in descending order of their depth-1 bound).
        let upper = match (tail, root) {
            (_, Some(r)) => Some(r.caps[offset].0),
            (Some((tail_bounds, state)), None) => Some(tail_bounds.tail_upper(state, offset)?.0),
            (None, None) if depth > 0 => Some(node_upper(self.pool, domain, bounds, p, depth, orders)?.0),
            (None, None) => joint_root_upper(self.pool, domain, bounds, orders, offset)?,
        };
        self.fold_unexplored(upper, started);
        Ok(())
    }

    /// Start bounding what the stop leaves unexplored. Its time goes to `proof.boundMs` and `otherMs`, not to the
    /// search activity the stop interrupted; pass the returned mark to `fold_unexplored`.
    fn bound_start(&mut self) -> (Instant, usize) {
        self.rec.clock.lap(slot::OTHER)
    }

    /// The branches still open are bounded by `open` (None: no branch open): offer the larger of it and the best
    /// payoff as the global upper bound. Only a traversal of the whole remaining domain (one search part) offers it.
    fn note_open(&mut self, open: Option<i128>) {
        // The interval frontier has no scalar Top-K; `open` alone would omit its retained candidates.
        if self.rec.parts != 1 || self.certified.is_some() {
            return;
        }
        let best = self.top.first().map(|e| e.evaluation.expected_payoff.numerator);
        if let Some(upper) = best.max(open) {
            self.rec.offer_upper(upper);
        }
    }

    fn fold_unexplored(&mut self, upper: Option<i128>, (started, resume): (Instant, usize)) {
        if let Some(upper) = upper {
            self.rec.unexplored(upper);
        }
        let (now, _) = self.rec.clock.lap(resume);
        self.tel.proof.bound_ms += now.saturating_duration_since(started).as_secs_f64() * 1000.0;
    }

    /// Close the recording into the wire document.
    /// `standing` is the final Top-K's (best, K-th, decks held), taken before the results leave the engine.
    fn finish_telemetry(&mut self, standing: (i128, Option<i128>, usize)) {
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
        if filled > 0 {
            proof.best = Some(best.to_string());
        }
        proof.kth = kth.map(|v| v.to_string());
        tel.memory = telemetry::Memory::now();
        proof.global_upper_bound = self.rec.upper.map(|v| v.to_string());
        if stopped && !complete && self.rec.bounded {
            proof.upper_bound = self.rec.unexplored.map(|v| v.to_string());
            let gap = |x: i128| match self.rec.unexplored {
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
    fn seeded_in_top(&self) -> usize {
        self.top.iter().filter(|t| self.seeded.contains(&t.physical)).count()
    }

    /// The result ended by `exit_reason` with these decks and telemetry, `elapsed` after the search start; `fixed`
    /// marks the evaluation of one requested deck. The final result and the progress reports share it.
    fn outcome(
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
        let probability_law = if live {
            let lottery = if self.certified.is_some() {
                "certifiedNativeLotteryIntervals"
            } else if self.lottery_free.is_some() {
                "noLuckRange"
            } else {
                "none"
            };
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
            player_goal: None,
            strategy: strategy.clone(),
            probability_law,
            proof_scope: "conditional on declared master, roster and complete judgement/clock inputs, with the five members performing in a uniformly random order; client counters are not server reward authority",
            resolved_context: serde_json::Value::Null,
            results,
            telemetry,
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        }
    }

    /// A progress report at `now`: the result the search would return if its time limit expired now. The telemetry
    /// so far is closed on a copy (the recording stays open); it has no bound of the unexplored part, which only a
    /// stop computes.
    fn report_outcome(
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
            self.top.iter().cloned().map(|e| e.wire(self.metric)).collect::<Result<Vec<_>, _>>()?
        };
        let elapsed = now.saturating_duration_since(start);
        Ok(self.outcome(strategy, ExitReason::TimeLimit, false, results, tel, elapsed))
    }
}

/// The best payoff any completion through the depth-0 joint choices from `from` can reach: each choice's depth-1
/// node bound, the bound the traversal itself would check there. None when no legal choice remains.
fn joint_root_upper(
    pool: &Pool,
    domain: &crate::domain::CandidateDomain,
    bounds: &super::joint::JointBounds,
    orders: &[([usize; 5], u128)],
    from: usize,
) -> Result<Option<i128>, Error> {
    use super::joint::SLOTS;
    let mut p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
    let mut upper = None;
    for &(m, choice) in &bounds.choices[from..] {
        let character = pool.members[m].character_id;
        if domain.leader().is_some_and(|l| l != m)
            || domain.required().iter().any(|&r| r != m && pool.members[r].character_id == character)
            || !bounds.allows(SLOTS[0], choice)
        {
            continue;
        }
        p.members[SLOTS[0]] = m;
        p.snaps[SLOTS[0]] = if choice == 0 { None } else { Some(domain.snaps()[choice - 1]) };
        let (cap, _) = node_upper(pool, domain, bounds, &p, 1, orders)?;
        upper = Some(upper.map_or(cap, |u: i128| u.max(cap)));
    }
    Ok(upper)
}

/// The (payoff, power) bound a joint node at `depth > 0` checks first: the cheap bound of the Gekisou combo carrier
/// level its completions can reach, with the envelope keyed by its placed carriers (the pool-wide cheap bound without
/// carrier levels). It covers every completion of the prefix.
fn node_upper(
    pool: &Pool,
    domain: &crate::domain::CandidateDomain,
    bounds: &super::joint::JointBounds,
    p: &PhysicalDeck,
    depth: usize,
    orders: &[([usize; 5], u128)],
) -> Result<(i128, i64), Error> {
    let choices = super::joint::JointBounds::prefix_choices(domain, p, depth);
    let placed = bounds.carriers_placed(p, depth, &choices);
    let keyed = bounds.keyed(p, depth, &choices, 5 - depth, 5 - depth);
    bounds.carrier_level((placed + 5 - depth).min(5)).expected_upper_keyed(
        pool,
        domain,
        p,
        depth,
        orders,
        keyed.as_ref(),
    )
}
/// The payoff of one outcome of a deck under the metric.
#[allow(clippy::too_many_arguments)]
pub(crate) fn payoff_of(
    pool: &Pool,
    request: &SearchRequest,
    metric: &Metric,
    event_input: Option<&EventPayoffInput>,
    p: &PhysicalDeck,
    score: i32,
    power: i32,
    final_life: Option<i32>,
) -> Result<i128, Error> {
    match *metric {
        Metric::Power => Ok(power as i128),
        Metric::Score => Ok(score as i128),
        Metric::ScoreAtLeast { threshold } => Ok(i128::from(score >= threshold)),
        Metric::CappedScore { threshold } => Ok(score.min(threshold) as i128),
        Metric::ScoreAndLifeAtLeast { threshold, min_final_life } => Ok(i128::from(
            score >= threshold
                && final_life.ok_or_else(|| Error::Input("terminal life requires played Live".into()))?
                    >= min_final_life,
        )),
        Metric::ClientEventPoints { event_id } => Ok(request
            .objective
            .context()
            .expect("validated context")
            .preview_event_points(pool, &p.as_deck(), event_input.expect("validated event input"), event_id, score)?
            .points_for(event_id) as i128),
        Metric::ClientChallengePoints { event_id } => Ok(request
            .objective
            .context()
            .expect("validated context")
            .preview_event_points(pool, &p.as_deck(), event_input.expect("validated event input"), event_id, score)?
            .challenge_points_for(event_id) as i128),
        Metric::ConditionalClientEventItems { event_id, resource_type, resource_id } => {
            let items = request.objective.context().expect("validated context").preview_event_items(
                pool,
                &p.as_deck(),
                event_input.expect("validated event input"),
                event_id,
                score,
            )?;
            ournotes_sim::scenario::item_payoff(&items, event_id, resource_type, resource_id)
        }
    }
}

pub(crate) fn arithmetic() -> Error {
    Error::Domain("finite-law checked exact arithmetic overflow".into())
}

/// A score distribution summary preserves integer masses and does not use f64 ranks.
/// Quantile p is the smallest score with cumulative mass >= ceil(p * total mass).
pub fn score_summary(mass: &BTreeMap<i32, u128>, target: Option<i32>) -> Result<ScoreSummary, Error> {
    if mass.is_empty() || mass.values().any(|&w| w == 0) {
        return Err(Error::Input("score summary requires positive nonempty masses".into()));
    }
    let total = mass.values().try_fold(0u128, |a, &w| a.checked_add(w).ok_or_else(arithmetic))?;
    let quantile = |tenth: u128| -> Result<i32, Error> {
        // Dividing first keeps valid u128 totals safe, including u128::MAX.
        let rank = (total / 10)
            .checked_mul(tenth)
            .and_then(|n| n.checked_add(((total % 10) * tenth).div_ceil(10)))
            .ok_or_else(arithmetic)?;
        let mut cumulative = 0u128;
        for (&score, &weight) in mass {
            cumulative = cumulative.checked_add(weight).ok_or_else(arithmetic)?;
            if cumulative >= rank {
                return Ok(score);
            }
        }
        Err(Error::Domain("score quantile exceeds mass".into()))
    };
    let (probability_at_least, expected_shortfall) = if let Some(target) = target {
        let mut success = 0u128;
        let mut shortfall = 0i128;
        for (&score, &weight) in mass {
            if score >= target {
                success = success.checked_add(weight).ok_or_else(arithmetic)?;
            } else {
                shortfall = shortfall
                    .checked_add(
                        (target as i128 - score as i128)
                            .checked_mul(i128::try_from(weight).map_err(|_| arithmetic())?)
                            .ok_or_else(arithmetic)?,
                    )
                    .ok_or_else(arithmetic)?;
            }
        }
        (
            Some(Fraction { numerator: success.to_string(), denominator: total.to_string() }),
            Some(Fraction { numerator: shortfall.to_string(), denominator: total.to_string() }),
        )
    } else {
        (None, None)
    };
    Ok(ScoreSummary {
        minimum: *mass.first_key_value().expect("nonempty").0,
        maximum: *mass.last_key_value().expect("nonempty").0,
        p10: quantile(1)?,
        p50: quantile(5)?,
        p90: quantile(9)?,
        target_score: target,
        probability_at_least,
        expected_shortfall,
    })
}

/// Fixed-deck evaluator over the shared core, exposed for independent checks.
/// Context identity must match. Network arrivals use controller frame snapshots and explicit aggregate ranks.
/// Explicit finished lifecycle remains unsupported.
/// No wall clock is used by this unbounded numeric evaluator.
pub fn evaluate_declared_context(
    master: &ournotes_sim::master::Master,
    physical: &PhysicalDeck,
    input: &FiniteSeedContext,
    root_seed: i32,
    network: Option<&[RankConfirmation]>,
    simulation: &SimulationInput,
) -> Result<(expectation::ConditionalOutcome, Vec<(usize, usize)>), Error> {
    reject_unsupported_lifecycle(network, simulation.live_finished_from_frame)?;
    if input.physical() != *physical {
        return Err(Error::Input("physical deck differs from declared context".into()));
    }
    let mut input = input.clone();
    if let Some(v) = simulation.music_length_ms {
        if v <= 0 {
            return Err(Error::Input("musicLengthMs must be positive".into()));
        }
        input.params.music_length_ms = v;
    }
    if let Some(v) = simulation.score_music_length_ms {
        if v <= 0 {
            return Err(Error::Input("scoreMusicLengthMs must be positive".into()));
        }
        input.params.score_music_length_ms = Some(v);
    }
    if input.delta_times.len() != input.play.frames.len() {
        return Err(Error::Input("one deltaTime per declared frame required".into()));
    }
    if simulation.live_finished_from_frame.is_some_and(|v| v >= input.play.frames.len()) {
        return Err(Error::Input("lifecycle frame outside declared play".into()));
    }
    if let Some(cs) = network {
        let g = input.gekisou.as_ref().ok_or_else(|| Error::Input("network confirmations require Gekisou".into()))?;
        if g.fevers.len() > 3 {
            return Err(Error::Input("at most three native Gekisou ranges supported".into()));
        }
        let missions: [i64; 3] =
            g.missions.clone().try_into().map_err(|_| Error::Input("three native missions required".into()))?;
        let factors = ournotes_sim::live::full::gekisou_rank_factors(master, &missions)?;
        let mut ranges = HashSet::new();
        for c in cs {
            if c.frame >= input.play.frames.len()
                || c.range >= g.fevers.len().min(3)
                || !(1..=5).contains(&c.rank)
                || !ranges.insert(c.range)
                || c.percent != factors[c.range][c.rank as usize - 1]
            {
                return Err(Error::Input(
                    "invalid aggregate network packet arrival/range/group rank/percentage".into(),
                ));
            }
        }
        if ranges.len() != g.fevers.len() {
            return Err(Error::Input("one aggregate packet per fever required".into()));
        }
    }
    let outcome = simulate_current(&input, master, root_seed, network, simulation.live_finished_from_frame, || false)?
        .ok_or_else(|| Error::Domain("unbounded declared evaluator cancelled unexpectedly".into()))?;
    Ok((outcome.terminal, outcome.network_applications))
}

/// Low-level exact-model search; request.time_limit is respected in addition to Limits.
/// Played input is COMPLETE declared judgement/clock data; a played live is valued by its mean payoff over the
/// 120 performance orders (`super::uniform`).
#[allow(clippy::too_many_arguments)] // The audit inputs stay separately borrowed; JSON callers use RecommendationRequest.
pub fn solve_physical(
    pool: &Pool,
    request: &SearchRequest,
    metric: &Metric,
    event_input: Option<&EventPayoffInput>,
    limits: &Limits,
    strategy: &Strategy,
    network: Option<&[RankConfirmation]>,
    simulation: &SimulationInput,
) -> Result<RecommendationOutcome, Error> {
    let origin = Instant::now();
    solve_physical_impl(
        pool,
        request,
        metric,
        event_input,
        limits,
        strategy,
        network,
        simulation,
        None,
        &[],
        None,
        origin,
        None,
    )
}

/// `origin` is the request start; telemetry phases and incumbents are timed from it. `progress` receives reports
/// (`progress.rs`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn solve_physical_impl(
    pool: &Pool,
    request: &SearchRequest,
    metric: &Metric,
    event_input: Option<&EventPayoffInput>,
    limits: &Limits,
    strategy: &Strategy,
    network: Option<&[RankConfirmation]>,
    simulation: &SimulationInput,
    fixed: Option<PhysicalDeck>,
    initial: &[PhysicalDeck],
    compiled: Option<&crate::handler::ExecutionPlan>,
    origin: Instant,
    progress: Option<ProgressHook<'_>>,
) -> Result<RecommendationOutcome, Error> {
    reject_unsupported_lifecycle(network, simulation.live_finished_from_frame)?;
    let start = Instant::now();
    let mut limits = limits.clone();
    if let Some(d) = request.time_limit {
        let ms = d.as_millis().min(u64::MAX as u128) as u64;
        limits.time_limit_ms = Some(limits.time_limit_ms.map_or(ms, |m| m.min(ms)));
    }
    validate(request.k, &limits, strategy)?;
    let mut normalized;
    let request = if compiled.is_none() {
        normalized =
            SearchRequest { objective: expectation::normalized_objective(&request.objective), ..request.clone() };
        if let Some(confirmations) = network {
            let Objective::InScenario { context, .. } = &mut normalized.objective else {
                return Err(Error::Input("network ranking requires a resolved scenario".into()));
            };
            context.rank_confirmations = Some(confirmations.to_vec());
        }
        &normalized
    } else {
        request
    };
    let owned_plan;
    let plan = match compiled {
        Some(plan) => plan,
        None => {
            owned_plan =
                crate::handler::compile_execution(pool, request, metric, event_input, network, simulation, strategy)?;
            &owned_plan
        }
    };
    let candidates = plan.domain.members();
    let required = plan.domain.required();
    let snaps = plan.domain.snaps();
    let leader = plan.domain.leader();
    for p in fixed.iter().chain(initial) {
        plan.domain.check_fixed(pool, p)?;
    }
    let feasible = plan.domain.is_feasible();
    let song = &plan.song;
    let event = plan.event;
    let skip = &plan.skip;
    let mut tel = Telemetry::default();
    let env = &mut tel.environment;
    env.k = request.k;
    env.time_limit_ms = limits.time_limit_ms;
    env.max_candidates = limits.max_candidates;
    env.cache_entries = limits.cache_entries;
    let live = matches!(request.objective.inner(), Objective::LiveScore { .. });
    let orders = if live { ORDERS } else { 1 };
    env.target = Some(telemetry::Target { orders, denominator: orders.to_string() });
    env.domain = Some(telemetry::Domain {
        members: candidates.len(),
        snaps: snaps.len(),
        required: required.len(),
        leader_fixed: leader.is_some(),
    });
    env.bounds.compiled = plan.joint.is_some() || plan.deck_payoff.is_some() || plan.team_power.is_some();
    env.bounds.fallback = plan.bound_fallback.clone();
    env.bounds.compile_ms = plan.bound_compile_ms;
    if let Some(b) = &plan.joint {
        env.bounds.choices = b.choices.len();
        env.bounds.fine = b.has_fine();
        env.bounds.class_search = b.uses_class_search();
    }
    if let Some(b) = &plan.deck_payoff {
        if plan.joint.is_none() {
            env.bounds.choices = b.members.len();
        }
        env.bounds.deck_payoff = Some(telemetry::DeckPayoffSetup {
            score_cap: b.power_cap(pool, &plan.domain).and_then(|power| b.score_cap(power)).map(|v| v.to_string()),
            ..Default::default()
        });
    } else if let Some(refusal) = &plan.deck_payoff_refusal {
        env.bounds.deck_payoff =
            Some(telemetry::DeckPayoffSetup { refusal: Some(refusal.clone()), ..Default::default() });
    }
    if let Some(b) = &plan.team_power {
        env.bounds.choices = b.members.len();
    }
    let lottery = certified_engine::lottery_mode(pool, request, &plan.domain)?;
    let mut engine = Engine {
        pool,
        request,
        metric,
        event_input,
        simulation,
        limits: &limits,
        budget: super::budget::SearchBudget::new(start, limits.time_limit_ms.map(Duration::from_millis))?,
        stop: None,
        tel,
        rec: Recorder::new(origin),
        correlated: false,
        resource: false,
        bound_scratch: super::snaps::JointScratch::default(),
        bonus_scratch: super::joint::BonusScratch::default(),
        order_steps: Default::default(),
        top: Vec::new(),
        certified: if lottery == certified_engine::LotteryMode::Certified {
            Some(CertifiedState::new(
                request.k,
                if limits.cache_entries == 0 { 0 } else { certified_engine::LUCK_CURVE_CACHE_BYTES },
            )?)
        } else {
            None
        },
        lottery_free: if lottery == certified_engine::LotteryMode::Free {
            Some(std::sync::Arc::new(ournotes_sim::live::full::luck_skills(pool.master)?))
        } else {
            None
        },
        seen: HashSet::new(),
        fifo: VecDeque::new(),
        team_scores: team_scores::TeamScores::new(limits.cache_entries),
        programs: program_cache::ProgramCache::new(if limits.cache_entries == 0 {
            0
        } else {
            program_cache::DEFAULT_PROGRAM_CACHE_BYTES
        }),
        song: song.as_ref(),
        event,
        skip: skip.as_ref(),
        live,
        orders: uniform::all_orders(),
        positions: uniform::order_positions().into_iter().map(|(p, _)| p).collect(),
        seeded: HashSet::new(),
        root_order: None,
        warm: None,
        progress: progress.map(|hook| progress::Reporter::new(hook.interval, hook.report, strategy, start)),
        offered: None,
    };
    if let Some(p) = fixed {
        engine.tel.environment.traversal = Traversal::Fixed;
        engine.rec.begin(&mut engine.tel, "evaluate", None);
        engine.consider(p)?;
        engine.rec.end(&mut engine.tel);
    } else if feasible {
        if !initial.is_empty() {
            // Supplied incumbents, evaluated like warm-start decks: the traversal treats them as considered.
            engine.rec.begin(&mut engine.tel, "initial", None);
            for p in initial {
                let cut = plan.joint.as_ref().map(|b| (b, &plan.domain));
                if !engine.consider_with(*p, cut)? {
                    break;
                }
                let p = if engine.team_identity() { uniform::canonical(pool, p) } else { *p };
                engine.seeded.insert(p);
            }
            engine.rec.end(&mut engine.tel);
        }
        match strategy {
            Strategy::Exhaustive | Strategy::BranchAndBound => {
                #[cfg(not(target_arch = "wasm32"))]
                let mut luck_screen = luck_seed::Screen::default();
                #[cfg(not(target_arch = "wasm32"))]
                if matches!(strategy, Strategy::BranchAndBound) && engine.certified.is_some() && !engine.expired() {
                    engine.rec.begin(&mut engine.tel, "luckWarmStart", None);
                    luck_seed::seed(&plan.domain, &mut engine, &mut luck_screen)?;
                    engine.rec.end(&mut engine.tel);
                }
                let mut physical = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
                // The deck payoff ranking either settles the Top-K or hands the search to the joint traversal.
                let mut joint_next = true;
                if let Some(bounds) = &plan.team_power {
                    engine.tel.environment.traversal = Traversal::Joint;
                    engine.rec.begin(&mut engine.tel, "search", Some("teamPower".into()));
                    team_power_search::solve(&plan.domain, bounds, &mut engine)?;
                    engine.rec.end(&mut engine.tel);
                    joint_next = false;
                } else if let Some(bounds) = plan
                    .deck_payoff
                    .as_ref()
                    .filter(|_| plan.joint.is_none() || !super::snaps::ablated(super::ablate::NO_DECK_PAYOFF))
                {
                    engine.tel.environment.traversal = Traversal::Joint;
                    engine.rec.begin(&mut engine.tel, "search", Some("deckPayoff".into()));
                    joint_next = !deck_payoff_search::solve(&plan.domain, bounds, &mut engine, plan.joint.is_some())?;
                    engine.rec.end(&mut engine.tel);
                    if joint_next {
                        engine.rec.frontier.clear();
                    }
                }
                if joint_next && let Some(bounds) = &plan.joint {
                    engine.rec.begin(&mut engine.tel, "setup", None);
                    // Node bounds read position-mean gains: one pseudo-order carrying the mass of all of them.
                    let orders = uniform::MEAN_ORDERS.to_vec();
                    engine.correlated = bounds.correlation_worthwhile(pool, &plan.domain, &orders)?;
                    engine.resource = !bounds.prefers_compositions()
                        && bounds.resource_worthwhile(pool, &plan.domain, &orders, engine.correlated)?;
                    engine.warm = warm::Warm::new(&plan.domain, bounds, &orders);
                    engine.rec.end(&mut engine.tel);
                    if bounds.is_pt() && !bounds.prefers_compositions() {
                        engine.tel.environment.traversal = Traversal::Joint;
                        // The maximum-bonus warmup fills the Top-K; one already full (from the deck payoff ranking)
                        // leaves it nothing to do, and its pruned walk to a first leaf would repeat the search's.
                        if engine.safe_cutoff().is_none() {
                            engine.rec.begin(&mut engine.tel, "ptWarmStart", None);
                            let seed_bonus = bounds.bonus_upper(pool, &plan.domain, &physical, 0);
                            joint_rec(0, 0, &mut physical, &plan.domain, bounds, &orders, &mut engine, seed_bonus)?;
                            engine.rec.end(&mut engine.tel);
                        }
                        // After the maximum-bonus warmup, whose full Top-K ends it.
                        warm::seed(&mut engine)?;
                        let mut searched = false;
                        if !engine.expired() {
                            engine.rec.begin(&mut engine.tel, "ptRegimeCompile", None);
                            let restricted = if let Some((numerator, _)) = engine.safe_cutoff() {
                                bounds.qualifying_pt_domain(pool, &plan.domain, numerator, ORDERS as u128)?
                            } else {
                                None
                            };
                            let mut regime = telemetry::PtRegime::default();
                            let compiled = if let Some(domain) = &restricted {
                                let prepare = Instant::now();
                                let result = super::joint::JointBounds::compile(
                                    pool,
                                    request,
                                    domain,
                                    metric,
                                    event_input,
                                    simulation,
                                );
                                regime.compile_ms = prepare.elapsed().as_secs_f64() * 1000.0;
                                match result {
                                    Ok(b) => Some(b),
                                    Err(error) => {
                                        regime.fallback = Some(error.to_string());
                                        None
                                    }
                                }
                            } else {
                                None
                            };
                            engine.rec.end(&mut engine.tel);
                            engine.rec.begin(&mut engine.tel, "search", None);
                            engine.rec.frontier.clear();
                            (engine.rec.tracked, engine.rec.bounded, engine.rec.unexplored) = (true, true, None);
                            searched = true;
                            if let (Some(domain), Some(refined)) = (&restricted, &compiled) {
                                regime.members_removed = plan.domain.members().len() - domain.members().len();
                                engine.correlated = refined.correlation_worthwhile(pool, domain, &orders)?;
                                engine.tel.environment.bounds.pt_regime = Some(regime);
                                ordered_root(&mut physical, domain, refined, &orders, &mut engine)?;
                            } else {
                                engine.tel.environment.bounds.pt_regime = Some(regime);
                                ordered_root(&mut physical, &plan.domain, bounds, &orders, &mut engine)?;
                            }
                            engine.rec.end(&mut engine.tel);
                        }
                        if engine.stop.is_some() && !searched {
                            // The warm start's bonus filter skipped prefixes: the whole domain remains to be proven.
                            engine.rec.frontier.clear();
                            (engine.rec.tracked, engine.rec.bounded) = (true, true);
                            let started = engine.bound_start();
                            let upper = joint_root_upper(pool, &plan.domain, bounds, &orders, 0)?;
                            engine.fold_unexplored(upper, started);
                        }
                    } else if bounds.prefers_compositions() {
                        engine.tel.environment.traversal = Traversal::Composition;
                        warm::seed(&mut engine)?;
                        engine.rec.begin(&mut engine.tel, "search", None);
                        (engine.rec.tracked, engine.rec.bounded) = (true, true);
                        composition::solve(&plan.domain, bounds, &orders, &mut engine)?;
                        engine.rec.end(&mut engine.tel);
                    } else {
                        engine.tel.environment.traversal = Traversal::Joint;
                        warm::seed(&mut engine)?;
                        joint_regimes(&mut physical, plan, bounds, &orders, &mut engine)?;
                    }
                    engine.tel.environment.bounds.correlated = engine.correlated;
                    engine.tel.environment.bounds.resource = engine.resource;
                } else if joint_next {
                    engine.tel.environment.traversal = Traversal::Exhaustive;
                    // A refused live bound otherwise starts with the first five IDs and empty Snaps.
                    // Seed native LUCK searches with complete power teams, then retain the full traversal.
                    // These are proposals only: the ordinary LUCK evaluator establishes their score intervals.
                    #[cfg(not(target_arch = "wasm32"))]
                    if matches!(strategy, Strategy::BranchAndBound) && engine.certified.is_some() && !engine.expired() {
                        engine.rec.begin(&mut engine.tel, "fallbackWarmStart", None);
                        let power = Objective::Power { music_id: None, event };
                        let power = match request.objective.context() {
                            Some(c) => power.in_scenario(c.clone()),
                            None => Objective::Power { music_id: engine.song.as_ref().map(|s| s.id), event },
                        };
                        let seeds = super::search(
                            pool,
                            &SearchRequest {
                                objective: power,
                                k: 8,
                                constraints: request.constraints.clone(),
                                time_limit: Some(Duration::from_millis(
                                    limits.time_limit_ms.map_or(1000, |ms| (ms / 4).min(1000)),
                                )),
                            },
                        )?;
                        for seed in seeds.results {
                            if engine.expired() {
                                break;
                            }
                            let deck = pool.deck(seed.members, seed.snaps, [0, 1, 2, 3, 4])?;
                            let physical =
                                uniform::canonical(pool, &PhysicalDeck { members: deck.members, snaps: deck.snaps });
                            if !luck_screen.admit(&physical, false, &mut engine)? {
                                continue;
                            }
                            engine.tel.incumbents.warm_start.evaluations += 1;
                            if !engine.consider(physical)? {
                                break;
                            }
                            engine.seeded.insert(physical);
                        }
                        engine.rec.end(&mut engine.tel);
                    }
                    engine.rec.begin(&mut engine.tel, "search", None);
                    engine.rec.tracked = true;
                    members_rec(0, 0, &mut physical, candidates, required, leader, snaps, &mut engine)?;
                    engine.rec.end(&mut engine.tel);
                }
            }
            Strategy::Candidate { power_seeds, proposals, proposal_seed } => {
                engine.tel.environment.traversal = Traversal::Candidate;
                engine.rec.begin(&mut engine.tel, "warmStart", None);
                if *power_seeds > 0 && !engine.expired() {
                    let power = Objective::Power { music_id: None, event };
                    let power = match request.objective.context() {
                        Some(c) => power.in_scenario(c.clone()),
                        None => match &engine.song {
                            Some(s) => Objective::Power { music_id: Some(s.id), event },
                            None => power,
                        },
                    };
                    let duration = limits.time_limit_ms.map(|m| Duration::from_millis((m / 4).min(1000)));
                    let seeds = super::search(
                        pool,
                        &SearchRequest {
                            objective: power,
                            k: *power_seeds,
                            constraints: request.constraints.clone(),
                            time_limit: duration,
                        },
                    )?;
                    let seeds = seeds
                        .results
                        .into_iter()
                        .map(|seed| pool.deck(seed.members, seed.snaps, [0, 1, 2, 3, 4]))
                        .collect::<Result<Vec<_>, _>>()?;
                    // Cover every power seed BEFORE refining any one's physical layout.
                    // The former 120 permutations of seed 0 could consume the entire
                    // browser budget before another member set or snap layout was tried.
                    // Five rounds cap warmup at 5 * powerSeeds; the remaining budget
                    // explores skills, pairings and member substitutions below.
                    'seeds: for round in 0..5 {
                        for d in &seeds {
                            let mut p = PhysicalDeck { members: d.members, snaps: d.snaps };
                            match round {
                                0 => {}
                                1 => p.snaps = [None; 5],
                                _ => {
                                    let nonleaders = [0, 1, 3, 4];
                                    for (at, &slot) in nonleaders.iter().enumerate() {
                                        let from = nonleaders[(at + round - 1) % 4];
                                        p.members[slot] = d.members[from];
                                        p.snaps[slot] = d.snaps[from];
                                    }
                                }
                            }
                            engine.tel.candidate.warmup_proposals += 1;
                            if !engine.consider(p)? {
                                break 'seeds;
                            }
                            if round == 0 {
                                engine.tel.candidate.warmup_member_sets += 1;
                            }
                        }
                    }
                }
                engine.rec.end(&mut engine.tel);
                engine.rec.begin(&mut engine.tel, "proposals", None);
                let mut rng = ProposalRandom(*proposal_seed | 1);
                for n in 0..*proposals {
                    if engine.expired() {
                        break;
                    }
                    engine.tel.candidate.exploration_proposals += 1;
                    let mut p = if n % 3 != 0 && !engine.top.is_empty() {
                        let mut p = engine.top[rng.index(engine.top.len())].physical;
                        if rng.index(2) == 0 {
                            let slot = rng.index(5);
                            let m = candidates[rng.index(candidates.len())];
                            if (slot != 2 || leader.is_none()) && !required.contains(&p.members[slot]) {
                                p.members[slot] = m;
                            }
                        } else {
                            let slot = rng.index(5);
                            let s = rng.index(snaps.len() + 1);
                            p.snaps[slot] = if s == snaps.len() { None } else { Some(snaps[s]) };
                        }
                        p
                    } else {
                        random_deck(pool, candidates, required, leader, snaps, &mut rng)
                    };
                    // Additional physical swaps preserve member/snap pairs and fixed leader.
                    if n % 4 == 0 {
                        let a = rng.index(5);
                        let b = rng.index(5);
                        if leader.is_none() || (a != 2 && b != 2) {
                            p.members.swap(a, b);
                            p.snaps.swap(a, b);
                        }
                    }
                    if pool.check_deck(&p.as_deck()).is_err() {
                        continue;
                    }
                    if !engine.consider(p)? {
                        break;
                    }
                }
                engine.rec.end(&mut engine.tel);
                if engine.stop.is_none() {
                    engine.stop = Some(ExitReason::ProposalLimit)
                }
            }
        }
    }
    engine.tel.incumbents.warm_start.final_top_k = engine.seeded_in_top();
    if engine.certified.is_some() && engine.stop.is_none() {
        engine.refine_certified_frontier()?;
    }
    let standing = engine.standing();
    engine.rec.begin(&mut engine.tel, "finish", None);
    let results = if engine.certified.is_some() {
        let (results, proof) = engine.certified_results(engine.stop.is_none())?;
        if engine.stop.is_none() && !proof.complete {
            engine.stop = Some(ExitReason::RefinementRequired);
        }
        results
    } else {
        std::mem::take(&mut engine.top).into_iter().map(|e| e.wire(metric)).collect::<Result<Vec<_>, _>>()?
    };
    let exit_reason = engine.stop.unwrap_or(ExitReason::Exhausted);
    engine.rec.end(&mut engine.tel);
    engine.finish_telemetry(standing);
    let telemetry = std::mem::take(&mut engine.tel);
    Ok(engine.outcome(strategy, exit_reason, fixed.is_some(), results, telemetry, start.elapsed()))
}

/// Gekisou score: partition the physical domain by its converting Snaps.
/// - none: the conversion-free sub-domain keeps the raw judgement reach;
/// - exactly one, `c` in physical slot `s`: the sub-domain holds no other converting Snap, so per-entry reach
///   widens only by the windows of `c`, and `s` is forced to `c`;
/// - two or more: split by the first two slots, in search order, that hold converting Snaps. Both are forced to
///   converting Snaps and the other earlier slots exclude them.
///
/// The parts are disjoint and cover the domain. They share one Top-K and its canonical order. A failed part compile
/// falls back to one pool-wide search.
fn joint_regimes(
    p: &mut PhysicalDeck,
    plan: &crate::handler::ExecutionPlan,
    bounds: &super::joint::JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<(), Error> {
    use super::joint::{JointBounds, SLOTS, SlotRules};
    let converting = super::snaps::conversion_snaps(e.pool, plan.domain.snaps())?;
    (e.rec.tracked, e.rec.bounded) = (true, true);
    if converting.is_empty() {
        e.rec.begin(&mut e.tel, "search", None);
        ordered_root(p, &plan.domain, bounds, orders, e)?;
        e.rec.end(&mut e.tel);
        return Ok(());
    }
    let is_converting = |s: usize| converting.contains(&s);
    let mask = |domain: &crate::domain::CandidateDomain, keep: &dyn Fn(usize) -> bool| -> Vec<bool> {
        std::iter::once(false).chain(domain.snaps().iter().map(|&s| keep(s))).collect()
    };
    type Rules = Vec<(Option<SlotRules>, String)>;
    let mut parts: Vec<(crate::domain::CandidateDomain, Rules)> = Vec::new();
    parts.push((plan.domain.retain_snaps(|s| !is_converting(s)), vec![(None, "free".into())]));
    for &c in &converting {
        let domain = plan.domain.retain_snaps(|s| !is_converting(s) || s == c);
        let only = mask(&domain, &|s| s == c);
        let rules = SLOTS
            .iter()
            .map(|&slot| {
                let mut r = SlotRules::default();
                for &other in &SLOTS {
                    if other == slot {
                        r.forced[other] = Some(only.clone());
                    } else {
                        r.excluded[other] = Some(only.clone());
                    }
                }
                (Some(r), format!("snap {} slot {slot}", e.pool.snaps[c].id))
            })
            .collect();
        parts.push((domain, rules));
    }
    if converting.len() >= 2 {
        let conv = mask(&plan.domain, &is_converting);
        let mut rules = Vec::new();
        for j in 1..5 {
            for i in 0..j {
                let mut r = SlotRules::default();
                r.forced[SLOTS[i]] = Some(conv.clone());
                r.forced[SLOTS[j]] = Some(conv.clone());
                for k in (0..j).filter(|&k| k != i) {
                    r.excluded[SLOTS[k]] = Some(conv.clone());
                }
                rules.push((Some(r), format!("pair slots {},{}", SLOTS[i], SLOTS[j])));
            }
        }
        parts.push((plan.domain.clone(), rules));
    }
    e.rec.begin(&mut e.tel, "conversionCompile", None);
    let prepare = Instant::now();
    let mut conversion = telemetry::Conversion { snaps: converting.len(), ..Default::default() };
    let mut compiled = Vec::with_capacity(parts.len());
    for (domain, rules) in parts {
        match JointBounds::compile(e.pool, e.request, &domain, e.metric, e.event_input, e.simulation) {
            Ok(b) => compiled.push((domain, rules, b)),
            Err(error) => {
                conversion.compile_ms = prepare.elapsed().as_secs_f64() * 1000.0;
                conversion.fallback = Some(error.to_string());
                e.tel.environment.bounds.conversion = Some(conversion);
                e.rec.end(&mut e.tel);
                e.rec.begin(&mut e.tel, "search", None);
                ordered_root(p, &plan.domain, bounds, orders, e)?;
                e.rec.end(&mut e.tel);
                return Ok(());
            }
        }
    }
    conversion.compile_ms = prepare.elapsed().as_secs_f64() * 1000.0;
    conversion.parts = compiled.iter().map(|(_, rules, _)| rules.len()).sum();
    let total = conversion.parts as u64;
    e.tel.environment.bounds.conversion = Some(conversion);
    e.rec.end(&mut e.tel);
    e.rec.parts = total;
    // One root traversal per (part, slot rules); every traversal runs. With the visit order on, the traversals that
    // hold Top-K incumbents run first, best incumbent first, the others in the static order, and each root loop
    // follows its children's bound order (warm.rs).
    let mut flags = Vec::with_capacity(compiled.len());
    let mut traversals = Vec::new();
    for (i, (domain, rules, part)) in compiled.iter().enumerate() {
        let correlated = part.correlation_worthwhile(e.pool, domain, orders)?;
        flags.push((correlated, part.resource_worthwhile(e.pool, domain, orders, correlated)?));
        traversals.extend((0..rules.len()).map(|j| (i, j)));
    }
    if !warm::static_order() {
        let mut best = std::collections::HashMap::new();
        for t in &e.top {
            let key = warm::traversal_of(&t.physical, &converting);
            best.entry(key).or_insert((t.evaluation.expected_payoff.numerator, t.power));
        }
        traversals.sort_by_key(|t| std::cmp::Reverse(best.get(t).copied()));
    }
    for (i, j) in traversals {
        let (domain, rules, part) = &mut compiled[i];
        (e.correlated, e.resource) = flags[i];
        let (r, label) = &rules[j];
        part.set_rules(e.pool, domain, r.clone());
        *p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
        e.rec.frontier.clear();
        e.rec.begin(&mut e.tel, "search", Some(label.clone()));
        let more = ordered_root(p, domain, part, orders, e)?;
        e.rec.end(&mut e.tel);
        if !more {
            if e.rec.parts_done + 1 < total {
                // The later parts: the pool-wide bounds hold for every deck of the domain.
                let started = e.bound_start();
                let upper = joint_root_upper(e.pool, &plan.domain, bounds, orders, 0)?;
                e.fold_unexplored(upper, started);
            }
            return Ok(());
        }
        e.rec.parts_done += 1;
    }
    Ok(())
}

/// One whole-domain traversal, root children in bound order (static under the validation ablation `STATIC_ORDER`).
fn ordered_root(
    p: &mut PhysicalDeck,
    domain: &crate::domain::CandidateDomain,
    bounds: &super::joint::JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    if !warm::static_order() {
        // Root-level bound work.
        e.rec.clock.lap(0);
        let root = warm::RootOrder::new(domain, bounds, orders, e)?;
        if let Some(threshold) = super::snaps::census() {
            let open = root.caps.iter().filter(|c| c.0 >= threshold).count();
            let census = e.tel.leaves.census.get_or_insert_with(|| telemetry::Census::new(threshold));
            census.root.push([open, root.caps.len()]);
        }
        e.root_order = Some(root);
    }
    joint_rec(0, 0, p, domain, bounds, orders, e, None)
}

/// One node of the joint member/Snap traversal: the leader pair at depth 0, then the other slots in `SLOTS` order.
/// The pairs of the other slots take ascending indices of the choice order (`start` is the first index left to the
/// next one), so every team is visited once, in one layout; its canonical layout is the one evaluated.
#[allow(clippy::too_many_arguments)]
fn joint_rec(
    depth: usize,
    start: usize,
    p: &mut PhysicalDeck,
    domain: &crate::domain::CandidateDomain,
    bounds: &super::joint::JointBounds,
    orders: &[([usize; 5], u128)],
    e: &mut Engine<'_, '_>,
    seed_bonus: Option<i64>,
) -> Result<bool, Error> {
    use super::joint::SLOTS;
    // The root loop may follow the bound order of `warm::RootOrder`.
    let root = if depth == 0 { e.root_order.take() } else { None };
    e.tel.nodes += 1;
    e.tel.joint.nodes[depth] += 1;
    if depth == 0 {
        let carriers = &mut e.tel.joint.carriers;
        carriers.levels = carriers.levels.max(bounds.carrier_level_count());
    }
    // One clock read serves both the deadline and the per-depth time.
    let (now, _) = e.rec.clock.lap(depth);
    if e.expired_at(now) {
        e.unexplored_node(p, depth, domain, bounds, orders)?;
        return Ok(false);
    }
    if domain.required().iter().filter(|r| !SLOTS[..depth].iter().any(|&slot| p.members[slot] == **r)).count()
        > 5 - depth
    {
        return Ok(true);
    }
    if let Some(target) = seed_bonus
        && bounds.bonus_upper(e.pool, domain, p, depth).is_some_and(|b| b < target)
    {
        e.tel.joint.seed_bonus_skipped[depth] += 1;
        return Ok(true);
    }
    // Every completion of this prefix has at most `placed + 5 - depth` Gekisou combo carriers; the cheap bounds of
    // that carrier level cover it (raw, fine and cutoff caps read the candidate's own factors). The envelope keyed by
    // the placed performers (their carriers' windows and their factor commands) tightens its `A0` and the placed
    // slots' gains.
    let choices = super::joint::JointBounds::prefix_choices(domain, p, depth);
    let placed = bounds.carriers_placed(p, depth, &choices);
    let level = (placed + 5 - depth).min(5);
    let node_bounds = bounds.carrier_level(level);
    let keyed = if depth > 0 { bounds.keyed(p, depth, &choices, 5 - depth, 5 - depth) } else { None };
    e.order_steps[depth].reset();
    if depth > 0
        && let Some((threshold, cutoff_power)) = e.safe_cutoff()
    {
        // i32::MIN marks a certified cutoff without a proved power tie-break.
        let exact_ties = cutoff_power > i32::MIN;
        let joint = &mut e.tel.joint;
        joint.branch.check(depth);
        joint.carriers.nodes[level] += 1;
        let (numerator, power) = node_bounds.expected_upper_keyed(e.pool, domain, p, depth, orders, keyed.as_ref())?;
        if numerator < threshold || (exact_ties && numerator == threshold && power < i64::from(cutoff_power)) {
            joint.branch.prune(depth);
            return Ok(true);
        }
        if numerator == threshold {
            joint.node_ties[depth] += 1;
        }
        // The open slots take ascending candidate indices from `start` on.
        if depth < 5
            && node_bounds.order_steps_prepare(
                e.pool,
                domain,
                p,
                depth,
                &e.positions,
                keyed.as_ref(),
                &mut e.order_steps[depth],
            )
            && let Some((steps, steps_power)) =
                node_bounds.order_steps_open(e.pool, &e.positions, &mut e.order_steps[depth], &bounds.choices[start..])
        {
            let module = joint.modules.entry("orderSteps").or_default();
            module.checks += 1;
            if steps < threshold || (exact_ties && steps == threshold && steps_power < i64::from(cutoff_power)) {
                module.pruned += 1;
                return Ok(true);
            }
        }
        if exact_ties
            && numerator == threshold
            && let Some(assigned_power) = bounds.assignment_power_upper(e.pool, domain, p, depth)
        {
            joint.assignment.check(depth);
            if assigned_power < i64::from(cutoff_power) {
                joint.assignment.prune(depth);
                return Ok(true);
            }
        }
        if depth < 5
            && let Some(cap) = bounds.carrier_split_expected_upper(domain, p, depth, start, orders, threshold)
        {
            let module = joint.modules.entry("carrierSplit").or_default();
            module.checks += 1;
            if cap < threshold || (exact_ties && cap == threshold && power < i64::from(cutoff_power)) {
                module.pruned += 1;
                return Ok(true);
            }
        }
        if depth < 5 && e.correlated {
            joint.correlated.check(depth);
            let correlated =
                node_bounds.correlated_expected_upper_keyed(e.pool, domain, p, depth, orders, keyed.as_ref())?;
            if correlated < threshold || (exact_ties && correlated == threshold && power < i64::from(cutoff_power)) {
                joint.correlated.prune(depth);
                return Ok(true);
            }
        }
        if depth < 5 && e.resource {
            joint.resource.check(depth);
            if let Some(cap) = node_bounds.resource_expected_upper(e.pool, domain, p, depth, orders)
                && (cap < threshold || (exact_ties && cap == threshold && power < i64::from(cutoff_power)))
            {
                joint.resource.prune(depth);
                return Ok(true);
            }
        }
        if depth < 5 {
            for module in bounds.modules() {
                let Some(cap) = module.node_upper(e.pool, p, depth, true, orders) else { continue };
                joint.modules.entry(module.name()).or_default().checks += 1;
                if cap < threshold || (exact_ties && cap == threshold && power < i64::from(cutoff_power)) {
                    joint.modules.entry(module.name()).or_default().pruned += 1;
                    return Ok(true);
                }
            }
        }
        if seed_bonus.is_none() && depth < 5 && bounds.is_pt() {
            joint.bonus.check(depth);
            if let Some((cap, cap_power)) =
                node_bounds.bonus_expected_upper_with_power(e.pool, domain, p, depth, orders, &mut e.bonus_scratch)?
            {
                if cap < threshold || (exact_ties && cap == threshold && power.min(cap_power) < i64::from(cutoff_power))
                {
                    joint.bonus.prune(depth);
                    return Ok(true);
                }
                if cap == threshold {
                    joint.node_ties[depth] += 1;
                }
            } else {
                joint.bonus_unavailable[depth] += 1;
            }
        }
    }
    if depth == 5 {
        let more = e.consider_with(*p, Some((bounds, domain)))?;
        if !more {
            e.unexplored_node(p, depth, domain, bounds, orders)?;
        }
        return Ok(more && !(seed_bonus.is_some() && e.safe_cutoff().is_some()));
    }
    let slot = SLOTS[depth];
    // A child that is not a carrier leaves at most `placed + 4 - depth` carriers to its completions, a carrier one
    // more; the tail check covers both kinds of children. Each reads the keyed envelope of as many carriers to come
    // (the child's own commands are not known yet).
    let low = bounds.carrier_level((placed + 4 - depth).min(5));
    let high = bounds.carrier_level((placed + 5 - depth).min(5));
    let (tail, tail_high) = if std::ptr::eq(low, high) {
        (low.tail_state_keyed(e.pool, domain, p, depth, orders, keyed.as_ref()), None)
    } else {
        let keyed_low = bounds.keyed(p, depth, &choices, 4 - depth, 5 - depth);
        (
            low.tail_state_keyed(e.pool, domain, p, depth, orders, keyed_low.as_ref()),
            high.tail_state_keyed(e.pool, domain, p, depth, orders, keyed.as_ref()),
        )
    };
    // The tail bound of every kind of child, also for what a stop leaves unexplored.
    let tail_check = tail_high.as_ref().or(tail.as_ref()).map(|state| (high, state));
    // There is no tail state at the root, so its loop may run in any order.
    let children = root.as_ref().map_or(&bounds.choices[..], |r| &r.children[..]);
    let width = children.len();
    if root.as_ref().and_then(|r| r.best()).is_some_and(|cap| warm::inferior(cap, e)) {
        e.tel.joint.root_order.traversals_pruned += 1;
    }
    for (offset, &(m, choice)) in children.iter().enumerate().skip(start) {
        if let Some(r) = &root
            && seed_bonus.is_none()
        {
            // Ordered root children: each cap covers its child and every later one.
            e.note_open(Some(r.caps[offset].0));
        }
        if let Some(r) = &root
            && warm::inferior(r.caps[offset], e)
        {
            e.tel.joint.root_order.skipped += (width - offset) as u64;
            break;
        }
        if offset % 16 == 0
            && let Some((threshold, cutoff_power)) = e.safe_cutoff()
            && let Some(state) = tail_high.as_ref().or(tail.as_ref())
        {
            e.tel.joint.tail.check(depth);
            let (upper, power) = high.tail_upper(state, offset)?;
            if upper < threshold || (upper == threshold && power < i64::from(cutoff_power)) {
                e.tel.joint.tail.prune(depth);
                e.tel.joint.tail_choices_skipped[depth] += (width - offset) as u64;
                break;
            }
        }
        // Below the leader every child and its completions take candidates from `offset` on: their order steps bound
        // all the children left (the node's own check covered `start`).
        if depth > 0
            && offset > start
            && offset % if depth < 4 { 4 } else { 16 } == 0
            && let Some((threshold, cutoff_power)) = e.safe_cutoff()
            && node_bounds.order_steps_prepare(
                e.pool,
                domain,
                p,
                depth,
                &e.positions,
                keyed.as_ref(),
                &mut e.order_steps[depth],
            )
            && let Some((steps, steps_power)) =
                node_bounds.order_steps_suffix(e.pool, &e.positions, &mut e.order_steps[depth], offset)
        {
            // The suffix summaries hold the open pairs' gains and frontier: the bound of one pass over the suffix.
            debug_assert_eq!(
                Some((steps, steps_power)),
                node_bounds.order_steps_open(
                    e.pool,
                    &e.positions,
                    &mut e.order_steps[depth],
                    &bounds.choices[offset..]
                )
            );
            let exact_ties = cutoff_power > i32::MIN;
            let module = e.tel.joint.modules.entry("orderStepsTail").or_default();
            module.checks += 1;
            if steps < threshold || (exact_ties && steps == threshold && steps_power < i64::from(cutoff_power)) {
                module.pruned += 1;
                e.tel.joint.tail_choices_skipped[depth] += (width - offset) as u64;
                break;
            }
        }
        // Every child from `offset` on and its completions take candidates from `offset` on: the carrier split of
        // that suffix bounds them all.
        if depth > 0
            && offset > start
            && offset % 16 == 0
            && let Some((threshold, _)) = e.safe_cutoff()
            && let Some(cap) = bounds.carrier_split_expected_upper(domain, p, depth, offset, orders, threshold)
        {
            let module = e.tel.joint.modules.entry("carrierSplitTail").or_default();
            module.checks += 1;
            if cap < threshold {
                module.pruned += 1;
                e.tel.joint.tail_choices_skipped[depth] += (width - offset) as u64;
                break;
            }
        }
        // The node itself checked the deadline; inside the choice loop the clock is read every 32 offsets.
        if offset % 32 == 31 && e.expired() {
            e.unexplored_choices(p, depth, offset, tail_check, root.as_ref(), domain, bounds, orders)?;
            return Ok(false);
        }
        if slot == 2 && domain.leader().is_some_and(|l| l != m) {
            continue;
        }
        let character = e.pool.members[m].character_id;
        if SLOTS[..depth].iter().any(|&s| e.pool.members[p.members[s]].character_id == character)
            || domain.required().iter().any(|&r| r != m && e.pool.members[r].character_id == character)
        {
            continue;
        }
        let snap = if choice == 0 { None } else { Some(domain.snaps()[choice - 1]) };
        if snap.is_some() && SLOTS[..depth].iter().any(|&s| p.snaps[s] == snap) {
            continue;
        }
        if !bounds.allows(slot, choice) {
            continue;
        }
        let (pair_bounds, pair_tail) = match &tail_high {
            Some(state) if bounds.is_carrier(m, choice) => (high, Some(state)),
            _ => (low, tail.as_ref()),
        };
        if let Some((threshold, cutoff_power)) = e.safe_cutoff()
            && let Some(state) = pair_tail
        {
            e.tel.joint.pair.check(depth);
            let (upper, power) = pair_bounds.pair_upper(state, m, choice)?;
            if upper < threshold || (upper == threshold && power < i64::from(cutoff_power)) {
                e.tel.joint.pair.prune(depth);
                continue;
            }
            if upper == threshold {
                e.tel.joint.pair_ties[depth] += 1;
            }
        }
        // The order steps of the child's completions: it takes this slot, the slots after it take pairs from
        // `offset + 1` on (the node's suffix summaries).
        if (1..4).contains(&depth)
            && let Some((threshold, cutoff_power)) = e.safe_cutoff()
            && node_bounds.order_steps_prepare(
                e.pool,
                domain,
                p,
                depth,
                &e.positions,
                keyed.as_ref(),
                &mut e.order_steps[depth],
            )
            && let Some((steps, steps_power)) =
                node_bounds.order_steps_pair(e.pool, &e.positions, &mut e.order_steps[depth], offset, m, choice)
        {
            let exact_ties = cutoff_power > i32::MIN;
            let module = e.tel.joint.modules.entry("orderStepsPair").or_default();
            module.checks += 1;
            if steps < threshold || (exact_ties && steps == threshold && steps_power < i64::from(cutoff_power)) {
                module.pruned += 1;
                continue;
            }
        }
        p.members[slot] = m;
        p.snaps[slot] = snap;
        e.rec.frontier.set(depth, offset, width);
        // The leader's children start the ascending run of the other slots.
        let next = if depth == 0 { 0 } else { offset + 1 };
        let more = joint_rec(depth + 1, next, p, domain, bounds, orders, e, seed_bonus)?;
        e.rec.clock.lap(depth);
        if !more {
            e.unexplored_choices(p, depth, offset + 1, tail_check, root.as_ref(), domain, bounds, orders)?;
            return Ok(false);
        }
    }
    Ok(true)
}

/// Every deck of the domain, slot by slot. For played lives the non-leader slots 0, 1, 3, 4 take ascending candidate
/// indices (`start` is the first index left to the next one), one layout per team.
#[allow(clippy::too_many_arguments)]
fn members_rec(
    slot: usize,
    start: usize,
    p: &mut PhysicalDeck,
    candidates: &[usize],
    required: &[usize],
    leader: Option<usize>,
    snaps: &[usize],
    e: &mut Engine<'_, '_>,
) -> Result<bool, Error> {
    e.tel.nodes += 1;
    if e.expired() {
        return Ok(false);
    }
    if required.iter().filter(|r| !p.members[..slot].contains(r)).count() > 5 - slot {
        return Ok(true);
    }
    if slot == 5 {
        return snaps_rec(0, p, snaps, e);
    }
    let first = if e.team_identity() && slot != 2 { start } else { 0 };
    for (index, &m) in candidates.iter().enumerate().skip(first) {
        if slot == 2 && leader.is_some_and(|l| l != m) {
            continue;
        }
        if p.members[..slot].iter().any(|&i| e.pool.members[i].character_id == e.pool.members[m].character_id) {
            continue;
        }
        // Picking another card of a required character can never satisfy that requirement.
        if required.iter().any(|&r| r != m && e.pool.members[r].character_id == e.pool.members[m].character_id) {
            continue;
        }
        p.members[slot] = m;
        e.rec.frontier.set(slot, index, candidates.len());
        let next = if slot == 2 { start } else { index + 1 };
        if !members_rec(slot + 1, next, p, candidates, required, leader, snaps, e)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn snaps_rec(slot: usize, p: &mut PhysicalDeck, snaps: &[usize], e: &mut Engine<'_, '_>) -> Result<bool, Error> {
    e.tel.nodes += 1;
    if e.expired() {
        return Ok(false);
    }
    if slot == 5 {
        return e.consider(*p);
    }
    p.snaps[slot] = None;
    e.rec.frontier.set(5 + slot, 0, snaps.len() + 1);
    if !snaps_rec(slot + 1, p, snaps, e)? {
        return Ok(false);
    }
    for (index, &s) in snaps.iter().enumerate() {
        if p.snaps[..slot].contains(&Some(s)) {
            continue;
        }
        p.snaps[slot] = Some(s);
        e.rec.frontier.set(5 + slot, index + 1, snaps.len() + 1);
        if !snaps_rec(slot + 1, p, snaps, e)? {
            return Ok(false);
        }
    }
    p.snaps[slot] = None;
    Ok(true)
}
struct ProposalRandom(u64);
impl ProposalRandom {
    fn index(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}
fn random_deck(
    pool: &Pool,
    candidates: &[usize],
    required: &[usize],
    leader: Option<usize>,
    snaps: &[usize],
    r: &mut ProposalRandom,
) -> PhysicalDeck {
    let mut selected = required.to_vec();
    let offset = r.index(candidates.len());
    if selected.len() < 5 {
        for i in 0..candidates.len() {
            let m = candidates[(i + offset) % candidates.len()];
            if selected.iter().any(|&a| pool.members[a].character_id == pool.members[m].character_id) {
                continue;
            }
            selected.push(m);
            if selected.len() == 5 {
                break;
            }
        }
    }
    // Required may already contain five cards.
    selected.truncate(5);
    for i in (1..5).rev() {
        let j = r.index(i + 1);
        selected.swap(i, j);
    }
    if let Some(l) = leader {
        let at = selected.iter().position(|&i| i == l).expect("leader is required");
        selected.swap(2, at);
    }
    let mut p = PhysicalDeck { members: selected.try_into().expect("feasible five characters"), snaps: [None; 5] };
    for i in 0..5 {
        let s = r.index(snaps.len() + 1);
        if s < snaps.len() && !p.snaps[..i].contains(&Some(snaps[s])) {
            p.snaps[i] = Some(snaps[s]);
        }
    }
    p
}

struct CurrentDeclaredOutcome {
    terminal: expectation::ConditionalOutcome,
    network_applications: Vec<(usize, usize)>,
}
fn simulate_current<F: FnMut() -> bool>(
    input: &FiniteSeedContext,
    master: &ournotes_sim::master::Master,
    root_seed: i32,
    confirmations: Option<&[RankConfirmation]>,
    finished_from_frame: Option<usize>,
    mut cancelled: F,
) -> Result<Option<CurrentDeclaredOutcome>, Error> {
    if finished_from_frame.is_some() {
        return Err(Error::Unsupported("finished lifecycle is unsupported".into()));
    }
    if cancelled() {
        return Ok(None);
    }
    let mut input = input.clone();
    if let Some(confirmations) = confirmations {
        input.rank_confirmations = Some(confirmations.to_vec());
    }
    let terminal = input.simulate(master, root_seed)?;
    if cancelled() {
        return Ok(None);
    }
    let network_applications = terminal.model.rank_confirmation_applications().to_vec();
    Ok(Some(CurrentDeclaredOutcome { terminal, network_applications }))
}
