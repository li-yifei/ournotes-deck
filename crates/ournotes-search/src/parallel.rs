//! Native, disjoint-domain parallel recommendation. Each worker owns its mutable
//! search caches; the dataset, deadline, candidate budget and cancellation are shared.
use crate::{clock::Instant, handler::build_card_pool, search::Completion, types::*};
use ournotes_sim::{Error, cards::Roster, data::DeckData};
use std::{
    cell::RefCell,
    collections::HashSet,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

/// Cancellation is cooperative at the solver's deadline and candidate checks.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

struct Control {
    cancellation: Cancellation,
    visits: AtomicU64,
    limit: Option<u64>,
    k: usize,
    denominator: u128,
    top: Mutex<Vec<RecommendedDeck>>,
    cutoff: RwLock<Option<(i128, i64)>>,
    share: bool,
    deadline: Option<Instant>,
}
impl Control {
    fn new(request: &RecommendationRequest, cancellation: Cancellation, share: bool, start: Instant) -> Self {
        Self {
            cancellation,
            visits: AtomicU64::new(0),
            limit: request.limits.max_candidates,
            k: request.k,
            denominator: if matches!(request.execution, Execution::Live { .. }) { 120 } else { 1 },
            top: Mutex::new(Vec::new()),
            cutoff: RwLock::new(None),
            share,
            deadline: request
                .limits
                .time_limit_ms
                .and_then(|ms| start.checked_add(std::time::Duration::from_millis(ms))),
        }
    }
    fn stopped(&self) -> bool {
        self.cancellation.is_cancelled()
            || self.deadline.is_some_and(|d| Instant::now() >= d)
            || self.limit.is_some_and(|n| self.visits.load(Ordering::Relaxed) >= n)
    }
}
thread_local! {
    static CONTROL: RefCell<Option<Arc<Control>>> = const { RefCell::new(None) };
    static NATIVE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
pub(crate) fn native_enabled() -> bool {
    NATIVE.with(|v| v.get().is_some()) && CONTROL.with(|v| v.borrow().is_none())
}
/// Opt a native transport into parallel execution while keeping WASM and the
/// ordinary serial/oracle entry points unchanged.
pub fn with_native<T>(run: impl FnOnce() -> T) -> T {
    with_native_threads(default_workers(), run).expect("default worker count is valid")
}
/// Select the worker allowance for an account/native solver call.
pub fn with_native_threads<T>(workers: usize, run: impl FnOnce() -> T) -> Result<T, Error> {
    validate_workers(workers)?;
    let _guard = NATIVE_REQUEST.lock().unwrap_or_else(|p| p.into_inner());
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            NATIVE.with(|v| v.set(self.0));
        }
    }
    let _reset = Reset(NATIVE.with(|v| v.replace(Some(workers))));
    Ok(run())
}
pub(crate) fn native_workers() -> usize {
    NATIVE.with(|v| v.get().unwrap_or_else(default_workers))
}
fn controlled<T>(control: Arc<Control>, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            CONTROL.with(|v| *v.borrow_mut() = None);
        }
    }
    CONTROL.with(|v| {
        assert!(v.borrow().is_none());
        *v.borrow_mut() = Some(control);
    });
    let _reset = Reset;
    run()
}
pub(crate) fn cancelled() -> bool {
    CONTROL.with(|v| {
        v.borrow()
            .as_ref()
            .is_some_and(|c| c.cancellation.is_cancelled() || c.deadline.is_some_and(|d| Instant::now() >= d))
    })
}
pub(crate) fn visit() -> bool {
    CONTROL.with(|v| {
        let control = v.borrow();
        let Some(c) = control.as_ref() else { return true };
        match c.limit {
            Some(limit) => c
                .visits
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| (n < limit).then_some(n.saturating_add(1)))
                .is_ok(),
            None => {
                c.visits.fetch_add(1, Ordering::Relaxed);
                true
            }
        }
    })
}

/// Whether the request already owns K exact incumbents. This says nothing
/// about a worker's local result vector or its canonical identity tie-breaks.
pub(crate) fn has_cutoff() -> bool {
    CONTROL.with(|v| {
        v.borrow().as_ref().is_some_and(|c| c.share && c.cutoff.read().unwrap_or_else(|p| p.into_inner()).is_some())
    })
}
/// Avoid constructing a wire result when this solver has no shared ranking.
pub(crate) fn sharing() -> bool {
    CONTROL.with(|v| v.borrow().as_ref().is_some_and(|c| c.share))
}

/// Only fully evaluated distinct teams publish a cutoff. Equal primary/power
/// bounds stay open, preserving every canonical identity tie.
pub(crate) fn inferior(upper: i128, power: i64) -> bool {
    CONTROL.with(|v| {
        let control = v.borrow();
        let Some(c) = control.as_ref() else { return false };
        if !c.share {
            return false;
        }
        let cutoff = c.cutoff.read().unwrap_or_else(|p| p.into_inner());
        cutoff.is_some_and(|threshold| (upper, power) < threshold)
    })
}
pub(crate) fn publish(deck: RecommendedDeck) -> Result<(), Error> {
    CONTROL.with(|v| {
        let control = v.borrow();
        let Some(c) = control.as_ref() else { return Ok(()) };
        if !c.share {
            return Ok(());
        }
        let mut top = c.top.lock().unwrap_or_else(|p| p.into_inner());
        if top.iter().any(|d| d.members == deck.members && d.snaps == deck.snaps) {
            return Ok(());
        }
        let mut at = top.len();
        for (i, other) in top.iter().enumerate() {
            if compare(&deck, other)?.is_lt() {
                at = i;
                break;
            }
        }
        if at < c.k {
            top.insert(at, deck);
            top.truncate(c.k);
            if top.len() == c.k {
                let last = top.last().unwrap();
                let threshold = last.expected_payoff.as_ref().and_then(|f| {
                    (f.denominator.parse::<u128>().ok()? == c.denominator)
                        .then_some((f.numerator.parse::<i128>().ok()?, i64::from(last.power)))
                });
                // Publish a consistent numeric pair once per incumbent update. Readers
                // may see an older (weaker) cutoff, which only reduces pruning.
                *c.cutoff.write().unwrap_or_else(|p| p.into_inner()) = threshold;
            }
        }
        Ok(())
    })
}

/// Half of the logical CPUs available to this process, rounded upward.
pub fn default_workers() -> usize {
    max_workers().div_ceil(2)
}

/// Logical CPUs available to the current process.
pub fn max_workers() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}
pub fn validate_workers(workers: usize) -> Result<(), Error> {
    let maximum = max_workers();
    if !(1..=maximum).contains(&workers) {
        return Err(Error::Input(format!("workers must be in 1..={maximum}, got {workers}")));
    }
    Ok(())
}

// The desktop entry point admits one CPU search at a time. Its internal workers
// consume the process-wide allowance; concurrent requests wait outside their budget.
static NATIVE_REQUEST: Mutex<()> = Mutex::new(());
pub fn recommend_json(data: &DeckData, roster: &str, request: &str) -> Result<String, Error> {
    recommend_json_with_threads(data, roster, request, default_workers())
}
/// JSON entry point with an explicit worker count in `1..=max_workers()`.
pub fn recommend_json_with_threads(
    data: &DeckData,
    roster: &str,
    request: &str,
    workers: usize,
) -> Result<String, Error> {
    validate_workers(workers)?;
    let roster = Roster::from_json(roster)?;
    let request: RecommendationRequest =
        serde_json::from_str(request).map_err(|e| Error::Input(format!("request: {e}")))?;
    let _guard = NATIVE_REQUEST.lock().map_err(|_| Error::Domain("native search gate poisoned".into()))?;
    let result = recommend(data, &roster, &request, workers, Cancellation::default())?;
    serde_json::to_string(&result).map_err(|e| Error::Domain(format!("result JSON: {e}")))
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallelTelemetry {
    pub workers: usize,
    pub workers_used: usize,
    pub tasks: usize,
    pub tasks_finished: usize,
    pub candidates: u64,
    pub cancelled: bool,
    pub fallback: Option<&'static str>,
    /// Per-part counters remain separate from the aggregate global proof.
    pub partitions: Vec<crate::search::telemetry::Telemetry>,
}

/// Explicit worker count is useful for native callers and serial/parallel oracle comparisons.
pub fn recommend(
    data: &DeckData,
    roster: &Roster,
    request: &RecommendationRequest,
    workers: usize,
    cancellation: Cancellation,
) -> Result<RecommendationOutcome, Error> {
    recommend_started(data, roster, request, workers, cancellation, Instant::now())
}

pub(crate) fn recommend_started(
    data: &DeckData,
    roster: &Roster,
    request: &RecommendationRequest,
    workers: usize,
    cancellation: Cancellation,
    start: Instant,
) -> Result<RecommendationOutcome, Error> {
    validate_workers(workers)?;
    let fallback = if matches!(request.strategy, Strategy::Candidate { .. }) {
        Some("candidate strategy preserves its serial proposal sequence")
    } else if matches!(request.execution, Execution::Live { gekisou: true, .. }) {
        Some("certified lottery frontier remains request-local")
    } else {
        None
    };
    // Validate the original request, including all initial decks, before any split.
    let partitioning = workers > 1 && fallback.is_none() && !cancellation.is_cancelled();
    let built = if partitioning {
        crate::handler::prepare_partition_domain(data, roster, request)?
    } else {
        build_card_pool(data, roster, request)?
    };
    for d in &request.initial_decks {
        let d = built.pool().deck(d.members, d.snaps, [0, 1, 2, 3, 4])?;
        built.domain().check_fixed(
            built.pool(),
            &crate::search::expectation::PhysicalDeck { members: d.members, snaps: d.snaps },
        )?;
    }
    let tasks = if workers == 1 || fallback.is_some() || cancellation.is_cancelled() {
        vec![request.clone()]
    } else {
        // Live branch-and-bound recompiles expensive domain-specific envelopes
        // per task. Two tasks per worker amortize that cost; exhaustive search
        // keeps finer balancing because it has no bound compilation.
        let factor = if matches!(request.execution, Execution::Live { .. })
            && matches!(request.strategy, Strategy::BranchAndBound)
        {
            2
        } else {
            4
        };
        partitions(&built, request, workers.saturating_mul(factor))?
    };
    if workers == 1 || tasks.len() <= 1 || fallback.is_some() || cancellation.is_cancelled() {
        // A single feasible part still uses the ordinary serial bound plan.
        let built = if partitioning { build_card_pool(data, roster, request)? } else { built };
        let mut out = controlled(Arc::new(Control::new(request, cancellation.clone(), false, start)), || {
            crate::search::dispatch::execute(&built, None, start, start.elapsed().as_secs_f64() * 1000.0, None)
        })?;
        out.telemetry.parallel = Some(Box::new(ParallelTelemetry {
            workers,
            workers_used: 1,
            tasks: 1,
            tasks_finished: usize::from(out.completion == Completion::Complete),
            candidates: out.telemetry.leaves.visited,
            cancelled: cancellation.is_cancelled(),
            fallback,
            partitions: vec![],
        }));
        return Ok(out);
    }
    // BuiltProblem's Rc/RefCell caches stay on the creating thread. Workers build
    // Power/Skip retain their pool/objective for later tasks. Live retains its
    // fresh-build path: preparation reuse regressed the measured short-chart
    // workload and needs separate bound/cache lifetime investigation.
    drop(built);
    let next = AtomicUsize::new(0);
    let used = AtomicUsize::new(0);
    let control = Arc::new(Control::new(request, cancellation.clone(), true, start));
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers.min(tasks.len()))
            .map(|_| {
                let tasks = &tasks;
                let next = &next;
                let used = &used;
                let control = control.clone();
                scope.spawn(move || {
                    controlled(control.clone(), || {
                        let mut out = Vec::new();
                        let mut worked = false;
                        let mut prepared: Option<crate::handler::BuiltProblem<'_>> = None;
                        loop {
                            if worked && control.stopped() {
                                break;
                            }
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(task) = tasks.get(i) else { break };
                            worked = true;
                            let result = (|| {
                                if let Some(built) = &mut prepared {
                                    built.repartition(&task.constraints, &task.initial_decks)?;
                                } else {
                                    prepared = Some(build_card_pool(data, roster, task)?);
                                }
                                crate::search::dispatch::execute(
                                    prepared.as_ref().expect("worker problem initialized"),
                                    None,
                                    start,
                                    start.elapsed().as_secs_f64() * 1000.0,
                                    None,
                                )
                            })();
                            if matches!(task.execution, Execution::Live { .. }) {
                                prepared = None;
                            }
                            let failed = result.is_err();
                            out.push((i, result));
                            if failed {
                                break;
                            }
                        }
                        if worked {
                            used.fetch_add(1, Ordering::Relaxed);
                        }
                        out
                    })
                })
            })
            .collect();
        let mut all = Vec::new();
        for h in handles {
            all.extend(h.join().map_err(|_| Error::Domain("parallel search worker panicked".into()))?);
        }
        Ok::<_, Error>(all)
    })?;
    let mut outcomes = outcomes;
    outcomes.sort_by_key(|(i, _)| *i);
    let mut parts = outcomes.into_iter().map(|(_, r)| r).collect::<Result<Vec<_>, _>>()?;
    let complete = parts.len() == tasks.len() && parts.iter().all(|p| p.completion == Completion::Complete);
    let finished = parts.iter().filter(|p| p.completion == Completion::Complete).count();
    let mut out = parts.remove(0);
    // Each task prunes against fully evaluated global incumbents. Completion
    // of all complementary domains proves the coordinator's exact global Top-K.
    out.results = control.top.lock().unwrap_or_else(|p| p.into_inner()).clone();
    out.completion = if complete { Completion::Complete } else { Completion::TimedOut };
    out.optimality = if complete { Optimality::Proven } else { Optimality::Unproven };
    out.exit_reason = if complete {
        ExitReason::Exhausted
    } else if control.limit.is_some_and(|n| control.visits.load(Ordering::Relaxed) >= n)
        || parts.iter().chain(std::iter::once(&out)).any(|p| p.exit_reason == ExitReason::CandidateLimit)
    {
        ExitReason::CandidateLimit
    } else {
        ExitReason::TimeLimit
    };
    let mut telemetry = vec![out.telemetry.clone()];
    telemetry.extend(parts.into_iter().map(|p| p.telemetry));
    let environment = out.telemetry.environment.clone();
    out.telemetry = Default::default();
    out.telemetry.environment = environment;
    out.telemetry.nodes = telemetry.iter().map(|p| p.nodes).sum();
    out.telemetry.leaves.visited = telemetry.iter().map(|p| p.leaves.visited).sum();
    out.telemetry.leaves.evaluated = telemetry.iter().map(|p| p.leaves.evaluated).sum();
    out.telemetry.leaves.simulations = telemetry.iter().map(|p| p.leaves.simulations).sum();
    out.telemetry.environment.domain = None; // partition counters below name each actual domain
    out.telemetry.proof = Default::default();
    out.telemetry.proof.complete = complete;
    out.telemetry.proof.parts = tasks.len() as u64;
    out.telemetry.proof.parts_done = finished as u64;
    out.telemetry.proof.fraction = Some(finished as f64 / tasks.len() as f64);
    out.telemetry.proof.best =
        out.results.first().and_then(|d| d.expected_payoff.as_ref()).map(|f| f.numerator.clone());
    out.telemetry.proof.kth = (out.results.len() == request.k)
        .then(|| out.results.last().unwrap())
        .and_then(|d| d.expected_payoff.as_ref())
        .map(|f| f.numerator.clone());
    if complete {
        out.telemetry.proof.global_upper_bound = out.telemetry.proof.best.clone();
        out.telemetry.proof.upper_bound = out.telemetry.proof.best.clone();
        out.telemetry.proof.best_gap = out.telemetry.proof.best.as_ref().map(|_| 0.0);
    }
    out.telemetry.parallel = Some(Box::new(ParallelTelemetry {
        workers,
        workers_used: used.load(Ordering::Relaxed),
        tasks: tasks.len(),
        tasks_finished: finished,
        candidates: control.visits.load(Ordering::Relaxed),
        cancelled: cancellation.is_cancelled(),
        fallback: None,
        partitions: telemetry,
    }));
    out.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok(out)
}

fn compare(a: &RecommendedDeck, b: &RecommendedDeck) -> Result<std::cmp::Ordering, Error> {
    let exact = |d: &RecommendedDeck| -> Result<crate::search::expectation::ExactExpectation, Error> {
        let f =
            d.expected_payoff.as_ref().ok_or_else(|| Error::Domain("parallel merge requires exact payoff".into()))?;
        Ok(crate::search::expectation::ExactExpectation {
            numerator: f.numerator.parse().map_err(|_| Error::Domain("invalid payoff numerator".into()))?,
            denominator: f.denominator.parse().map_err(|_| Error::Domain("invalid payoff denominator".into()))?,
        })
    };
    Ok(crate::search::interval_topk::compare_exact(exact(b)?, exact(a)?)?
        .then_with(|| b.power.cmp(&a.power))
        .then_with(|| a.members.cmp(&b.members))
        .then_with(|| a.snaps.cmp(&b.snaps)))
}

fn partitions(
    built: &crate::handler::BuiltProblem<'_>,
    request: &RecommendationRequest,
    target: usize,
) -> Result<Vec<RecommendationRequest>, Error> {
    let pool = built.pool();
    let domain = built.domain();
    if !domain.is_feasible() {
        return Ok(vec![request.clone()]);
    }
    // Keep all legal leaders of a member composition in the same task. The
    // worker's exact score-law cache can then reuse the 120 performance orders.
    // Complementary membership constraints cover the original domain exactly.
    let mut tasks = if matches!(request.execution, Execution::Live { .. }) {
        vec![request.clone()]
    } else {
        // Power/Skip profit from leader-specific bounds and have no 120-order
        // simulation law to reuse across leaders.
        let leaders = domain.leader().map_or_else(|| domain.members().to_vec(), |m| vec![m]);
        let mut tasks = Vec::new();
        for leader in leaders {
            let mut r = request.clone();
            r.constraints.leader = Some(pool.members[leader].id);
            if crate::domain::CandidateDomain::build(pool, &r.constraints)?.is_feasible() {
                tasks.push(r);
            }
        }
        tasks
    };
    let mut i = 0;
    while tasks.len() < target && i < tasks.len() {
        let r = &tasks[i];
        let d = crate::domain::CandidateDomain::build(pool, &r.constraints)?;
        if let Some(&member) = d.members().iter().find(|m| !d.required().contains(m)) {
            let id = pool.members[member].id;
            let mut yes = r.clone();
            yes.constraints.include_members.push(id);
            let mut no = r.clone();
            no.constraints.exclude_members.push(id);
            let yes_ok = crate::domain::CandidateDomain::build(pool, &yes.constraints)?.is_feasible();
            let no_ok = crate::domain::CandidateDomain::build(pool, &no.constraints)?.is_feasible();
            tasks.remove(i);
            if yes_ok {
                tasks.insert(i, yes);
            }
            if no_ok {
                tasks.push(no);
            }
            if !yes_ok {
                continue;
            }
        } else {
            i += 1;
        }
    }
    for r in &mut tasks {
        let required: HashSet<_> = r.constraints.include_members.iter().copied().chain(r.constraints.leader).collect();
        r.initial_decks.retain(|d| {
            required.iter().all(|id| d.members.contains(id))
                && !d.members.iter().any(|id| r.constraints.exclude_members.contains(id))
                && r.constraints.leader.is_none_or(|id| d.members[2] == id)
        });
    }
    if tasks.is_empty() {
        tasks.push(request.clone());
    }
    Ok(tasks)
}
