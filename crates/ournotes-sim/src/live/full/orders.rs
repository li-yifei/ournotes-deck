//! One live played in many performance orders.
//!
//! Until a member acts (its chart skill event fires, or one of its condition skills starts an effect or draws a random
//! value), the live does not depend on where that member performs: its skills only read the live's state, and their
//! own state moves with them. So a model can move members that have not acted to other positions, and the result
//! plays on exactly as a model built for the new order. [`OrderedLive::simulate_orders`] uses this to play the
//! frames that orders share only once.
//!
//! [`OrderedLive::simulate_orders_bounded`] also gives up on the orders as soon as their payoffs cannot reach a
//! threshold. The played orders form a tree: a node is a set of orders that share a model, and it splits into one
//! child per position a member takes in its orders. The play keeps a bound on the payoff sum of every node on the
//! path from the root to the node being played, from the caller's bounds and the exact payoffs of the orders done.

use std::cmp::Reverse;
use std::ops::ControlFlow;

use super::*;

/// The inputs of a live whose members can perform in any order.
#[derive(Clone, Debug)]
pub struct OrderedLive {
    /// The members in a reference order. An order lists, for each performance position, an index into this list.
    pub performers: Vec<Performer>,
    pub notes: Vec<LiveNote>,
    /// The chart's skill events `(performance position, time)`.
    pub events: Vec<(i32, i32)>,
    pub params: LiveParams,
    pub gekisou: Option<GekisouSetup>,
    pub rank_confirmations: Option<Vec<crate::replay::RankConfirmation>>,
    pub play: LivePlay,
    /// The delta time of each frame of `play`, in seconds.
    pub delta_times: Vec<f32>,
    /// The lottery-related skills of the master: a deck that reads a probability in a live without a LUCK range then
    /// plays its native score without a random draw ([`prepare_lottery_free`]). `None` plays every deck as it is.
    pub lottery_free: Option<std::sync::Arc<LuckSkills>>,
}

/// The work [`OrderedLive::simulate_orders`] or [`OrderedLive::simulate_orders_bounded`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrderSharing {
    /// Frames played, every replay included.
    pub frames: u64,
    /// Frames a separate simulation of each distinct order plays.
    pub separate_frames: u64,
    /// Points where the orders went separate ways.
    pub branches: u64,
    /// Frames played again after a member acted unannounced (not through a chart skill event).
    pub replayed: u64,
    /// Model copies made.
    pub clones: u64,
    /// Calls of the bound.
    pub bounds: u64,
    /// The last bound on the payoff sum of all orders the play worked out: the exact sum when it played every order.
    /// `None` without bounds.
    pub bound: Option<i128>,
}

/// How [`OrderedLive::simulate_orders_bounded`] ended. Either way the orders visited have their exact results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrdersOutcome {
    /// Every order was visited.
    Complete(OrderSharing),
    /// The payoff sum of all orders is below the threshold; the orders not visited were given up.
    Stopped(OrderSharing),
    /// The bound asked to stop; the orders not visited were given up, and the payoff sum is not known to lie below the
    /// threshold.
    Interrupted(OrderSharing),
}

/// One completed order's power-parameterized native program and power-independent terminal life.
#[derive(Clone, Debug)]
pub struct RecordedOrder {
    /// Index in the caller's order array.
    pub index: usize,
    pub program: std::sync::Arc<ScoreProgram>,
    pub final_life: i32,
    pub random_draws: u64,
}

/// One order's finished live from [`OrderedLive::simulate_orders_grouped`].
#[derive(Clone, Copy, Debug)]
pub struct GroupedOrder {
    /// Index in the caller's order array.
    pub index: usize,
    pub score: i32,
    pub final_life: i32,
    pub draws: u64,
    pub payoff: i128,
}

/// A set of orders that agree on the `fixed` positions, with a model that has played a common prefix for all of them.
/// The model holds the members in the arrangement `assign` (position -> member) and no member of an open position has
/// acted yet.
struct Node {
    model: LiveModel,
    assign: Vec<usize>,
    fixed: u32,
    orders: Vec<usize>,
}

/// The caller's bound on payoff sums, see [`OrderedLive::simulate_orders_bounded`].
type UpperFn<'u> = dyn FnMut(&[usize], &LiveModel, Settled) -> Result<ControlFlow<(), i128>, Error> + 'u;

struct Bounds<'u> {
    stop_below: i128,
    check_every: usize,
    upper: &'u mut UpperFn<'u>,
}

/// Why a bounded play returned early.
enum Halt {
    Stopped,
    Interrupted,
}

/// What the play knows of the payoff sum of one node on the path from the root to the node being played.
struct Level {
    /// The least bound given for the node.
    cap: i128,
    /// Whether the node has split. Its orders are then those of the children done (payoffs summing to `done`), those
    /// of the children not started (bounds `rest`) and those of the child being played (the next level).
    split: bool,
    done: i128,
    rest: Vec<i128>,
}

struct Driver<'a, 'u, 'v> {
    live: &'a OrderedLive,
    orders: &'a [Vec<usize>],
    /// `future[p]`: the earliest chart time of a note judged in frame `p` or later.
    future: Vec<i32>,
    bounds: Option<Bounds<'u>>,
    visit: &'v mut (dyn FnMut(usize, &LiveModel) -> Result<i128, Error> + 'v),
    /// Entries of the orders not visited yet.
    left: usize,
    levels: Vec<Level>,
    stats: OrderSharing,
}

impl OrderedLive {
    fn check_order(&self, order: &[usize]) -> Result<(), Error> {
        let mut seen = vec![false; self.performers.len()];
        if order.len() != seen.len() || order.iter().any(|&s| s >= seen.len() || std::mem::replace(&mut seen[s], true))
        {
            return Err(Error::Input(format!("{order:?} is not an order of {} members", seen.len())));
        }
        Ok(())
    }

    /// Builds the live for one order: `order[p]` performs at position `p`.
    pub fn model(&self, master: &Master, order: &[usize]) -> Result<LiveModel, Error> {
        self.check_order(order)?;
        let deck: Vec<Performer> = order.iter().map(|&s| self.performers[s].clone()).collect();
        let mut model = match &self.gekisou {
            None => LiveModel::new(master, &deck, &self.notes, &self.events, self.params)?,
            Some(g) if self.rank_confirmations.is_some() => {
                LiveModel::new_gekisou_external(master, &deck, &self.notes, &self.events, self.params, g)?
            }
            Some(g) => LiveModel::new_gekisou(master, &deck, &self.notes, &self.events, self.params, g)?,
        };
        if let Some(confirmations) = &self.rank_confirmations {
            model.set_rank_confirmation_timeline(confirmations)?;
        }
        if let Some(skills) = &self.lottery_free {
            prepare_lottery_free(&mut model, skills)?;
        }
        Ok(model)
    }

    /// Plays the whole live in one order with the given random streams.
    pub fn simulate(&self, master: &Master, order: &[usize], random: LiveRandom) -> Result<LiveModel, Error> {
        let mut model = self.model(master, order)?;
        model.set_random(random);
        model.play_frames(&self.play, &self.delta_times, self.play.frames.len())?;
        Ok(model)
    }

    /// Plays the whole live in every order of `orders`, all with the same random streams, and calls `visit` with the
    /// index of each order and its finished model (equal orders share one). The result for each order is that of
    /// [`OrderedLive::simulate`].
    ///
    /// The orders share a model until a member whose position is still open acts. The model then goes back to the
    /// frame before, and continues once for each position that member takes in the orders (for a skill event, once for
    /// each member the orders put at the event's position).
    pub fn simulate_orders(
        &self,
        master: &Master,
        orders: &[Vec<usize>],
        random: LiveRandom,
        mut visit: impl FnMut(usize, &LiveModel) -> Result<(), Error>,
    ) -> Result<OrderSharing, Error> {
        let mut payoff = |i: usize, m: &LiveModel| visit(i, m).map(|()| 0i128);
        match self.play_orders(master, orders, random, false, None, &mut payoff)? {
            OrdersOutcome::Complete(s) | OrdersOutcome::Stopped(s) | OrdersOutcome::Interrupted(s) => Ok(s),
        }
    }

    /// Plays the orders like [`OrderedLive::simulate_orders`] while their payoffs can still sum to `stop_below` or
    /// more. `visit` returns the payoff of an order from its finished model; every entry of `orders` counts once,
    /// repeated orders included.
    ///
    /// `upper(ids, model, settled)` bounds the payoff sum of the orders `ids` (indices into `orders`) of a node:
    /// `Continue(c)` claims that their payoffs sum to at most `c`, and `Break(())` stops the play. The model has
    /// played the frames the node's orders share and holds the members of the positions they agree on in place;
    /// `settled` is [`LiveModel::settle`] of it. The play asks for a bound
    /// - on all orders, before the first frame;
    /// - on every child of a node that splits, after moving its members;
    /// - on the node being played, every `check_every` frames it plays (never with `check_every` 0).
    ///
    /// The bound on the payoff sum of all orders combines, along the path from the root to the node being played,
    /// each node's least bound with the exact payoffs of the orders done and the bounds of the children not started.
    /// It never grows when the bounds hold; the children of a node are played from the largest bound down. Once it is
    /// below `stop_below` the play returns [`OrdersOutcome::Stopped`]. A node whose payoffs sum to more than one of
    /// its bounds is an error.
    #[allow(clippy::too_many_arguments)]
    pub fn simulate_orders_bounded(
        &self,
        master: &Master,
        orders: &[Vec<usize>],
        random: LiveRandom,
        stop_below: i128,
        check_every: usize,
        mut upper: impl FnMut(&[usize], &LiveModel, Settled) -> Result<ControlFlow<(), i128>, Error>,
        mut visit: impl FnMut(usize, &LiveModel) -> Result<i128, Error>,
    ) -> Result<OrdersOutcome, Error> {
        let bounds = Bounds { stop_below, check_every, upper: &mut upper };
        self.play_orders(master, orders, random, false, Some(bounds), &mut visit)
    }

    /// Record programs through the same prefix-sharing tree. `program_bytes` bounds retained exported
    /// programs; None means the complete group exceeded that budget. Native evaluation still completes.
    pub fn simulate_orders_recorded(
        &self,
        master: &Master,
        orders: &[Vec<usize>],
        random: LiveRandom,
        program_bytes: usize,
        mut visit: impl FnMut(usize, &LiveModel) -> Result<(), Error>,
    ) -> Result<(OrderSharing, Option<Vec<RecordedOrder>>), Error> {
        let mut collector = ProgramCollector::new(program_bytes, orders.len());
        let record = collector.rows.is_some();
        let mut visitor = |index: usize, model: &LiveModel| {
            visit(index, model)?;
            collector.visit(index, model)?;
            Ok(0)
        };
        let outcome = self.play_orders(master, orders, random, record, None, &mut visitor)?;
        let stats = match outcome {
            OrdersOutcome::Complete(stats) | OrdersOutcome::Stopped(stats) | OrdersOutcome::Interrupted(stats) => stats,
        };
        Ok((stats, collector.finish(outcome)))
    }

    /// Bounded prefix-sharing evaluation with optional complete program capture. A pruned/interrupted group
    /// never yields cached programs, even if some orders completed before the stop.
    #[allow(clippy::too_many_arguments)]
    pub fn simulate_orders_bounded_recorded(
        &self,
        master: &Master,
        orders: &[Vec<usize>],
        random: LiveRandom,
        program_bytes: usize,
        stop_below: i128,
        check_every: usize,
        upper: impl FnMut(&[usize], &LiveModel, Settled) -> Result<ControlFlow<(), i128>, Error>,
        visit: impl FnMut(usize, &LiveModel) -> Result<i128, Error>,
    ) -> Result<(OrdersOutcome, Option<Vec<RecordedOrder>>), Error> {
        let (outcome, programs) = self.simulate_orders_bounded_recorded_partial(
            master,
            orders,
            random,
            program_bytes,
            stop_below,
            check_every,
            upper,
            visit,
        )?;
        let complete = matches!(outcome, OrdersOutcome::Complete(_))
            && programs.as_ref().is_some_and(|programs| programs.len() == orders.len());
        Ok((outcome, complete.then_some(programs).flatten()))
    }

    /// Like the complete recorded variant, but retain independently completed order programs when the
    /// remaining tree is pruned or interrupted. No unfinished order ever has a program entry.
    #[allow(clippy::too_many_arguments)]
    pub fn simulate_orders_bounded_recorded_partial(
        &self,
        master: &Master,
        orders: &[Vec<usize>],
        random: LiveRandom,
        program_bytes: usize,
        stop_below: i128,
        check_every: usize,
        mut upper: impl FnMut(&[usize], &LiveModel, Settled) -> Result<ControlFlow<(), i128>, Error>,
        mut visit: impl FnMut(usize, &LiveModel) -> Result<i128, Error>,
    ) -> Result<(OrdersOutcome, Option<Vec<RecordedOrder>>), Error> {
        let mut collector = ProgramCollector::new(program_bytes, orders.len());
        let record = collector.rows.is_some();
        let mut visitor = |index: usize, model: &LiveModel| {
            let payoff = visit(index, model)?;
            collector.visit(index, model)?;
            Ok(payoff)
        };
        let bounds = Bounds { stop_below, check_every, upper: &mut upper };
        let outcome = self.play_orders(master, orders, random, record, Some(bounds), &mut visitor)?;
        Ok((outcome, collector.finish_partial()))
    }

    fn play_orders(
        &self,
        master: &Master,
        orders: &[Vec<usize>],
        random: LiveRandom,
        record: bool,
        bounds: Option<Bounds<'_>>,
        visit: &mut dyn FnMut(usize, &LiveModel) -> Result<i128, Error>,
    ) -> Result<OrdersOutcome, Error> {
        let has_bounds = bounds.is_some();
        let Some(first) = orders.first() else {
            let stats = OrderSharing { bound: has_bounds.then_some(0), ..OrderSharing::default() };
            return Ok(OrdersOutcome::Complete(stats));
        };
        for o in orders {
            self.check_order(o)?;
        }
        let mut model = self.model(master, first)?;
        model.set_random(random);
        if record {
            model.begin_score_program_recording()?;
        }
        let mut distinct: Vec<&Vec<usize>> = orders.iter().collect();
        distinct.sort();
        distinct.dedup();
        let frames = self.play.frames.len();
        let mut future = vec![i32::MAX; frames + 1];
        for (p, frame) in self.play.frames.iter().enumerate().rev() {
            let judged = frame.judged.iter().filter_map(|j| model.notes.get(&j.note_id));
            future[p] = judged.fold(future[p + 1], |t, n| t.min(n.time_ms));
        }
        let stats = OrderSharing { separate_frames: distinct.len() as u64 * frames as u64, ..OrderSharing::default() };
        let mut driver =
            Driver { live: self, orders, future, bounds, visit, left: orders.len(), levels: Vec::new(), stats };
        let mut root = Node { model, assign: first.clone(), fixed: 0, orders: (0..orders.len()).collect() };
        let cap = match driver.bound_of(&root.orders, &mut root.model)? {
            ControlFlow::Break(()) => return Ok(OrdersOutcome::Interrupted(driver.stats)),
            ControlFlow::Continue(cap) => cap,
        };
        Ok(match driver.run_bounded(root, cap)? {
            ControlFlow::Continue(total) => {
                if has_bounds {
                    driver.stats.bound = Some(total);
                }
                OrdersOutcome::Complete(driver.stats)
            }
            ControlFlow::Break(Halt::Stopped) => OrdersOutcome::Stopped(driver.stats),
            ControlFlow::Break(Halt::Interrupted) => OrdersOutcome::Interrupted(driver.stats),
        })
    }

    /// Plays `orders` on up to `threads` threads. Orders that agree on their first `depth` positions form one
    /// group; a group shares frames through the tree of [`OrderedLive::simulate_orders_bounded`], and groups replay
    /// the frames they would have shared. Every order's result is that of [`OrderedLive::simulate`]; equal orders
    /// in different groups are not merged. `payoff(i, model)` returns the payoff of `orders[i]`.
    ///
    /// `stop`: `(stop_below, caps)` with `caps[i]` an upper bound on the payoff of `orders[i]`. The play returns
    /// [`OrdersOutcome::Stopped`] once the exact payoffs of the orders done plus the caps of the others fall below
    /// `stop_below`; the orders not visited are given up. The bound is checked when a group splits and every
    /// `check_every` frames, as in the serial tree, but without the tree's per-node refinement. `cancelled` ends the
    /// play with [`OrdersOutcome::Interrupted`]. Programs record like
    /// [`OrderedLive::simulate_orders_bounded_recorded_partial`]; `None` once all groups exceed `program_bytes`.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn simulate_orders_grouped(
        &self,
        master: &Master,
        orders: &[Vec<usize>],
        random: LiveRandom,
        program_bytes: usize,
        threads: usize,
        depth: usize,
        stop: Option<(i128, &[i128])>,
        check_every: usize,
        cancelled: &(dyn Fn() -> bool + Sync),
        payoff: &(dyn Fn(usize, &LiveModel) -> Result<i128, Error> + Sync),
    ) -> Result<(OrdersOutcome, Vec<Option<GroupedOrder>>, Option<Vec<RecordedOrder>>), Error> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        for o in orders {
            self.check_order(o)?;
        }
        if let Some((_, caps)) = stop
            && caps.len() != orders.len()
        {
            return Err(Error::Input("one payoff cap per order".into()));
        }
        let mut groups: std::collections::BTreeMap<Vec<usize>, Vec<usize>> = Default::default();
        for (i, o) in orders.iter().enumerate() {
            groups.entry(o[..depth.min(o.len())].to_vec()).or_default().push(i);
        }
        let groups: Vec<Vec<usize>> = groups.into_values().collect();
        let workers = threads.clamp(1, groups.len().max(1));
        struct Remaining {
            done: i128,
            rest: i128,
            left: usize,
        }
        let remaining = std::sync::Mutex::new(Remaining {
            done: 0,
            rest: stop.map_or(0, |(_, caps)| caps.iter().fold(0i128, |a, &c| a.saturating_add(c))),
            left: orders.len(),
        });
        // The root check of the serial tree: give every order up before a frame plays.
        if let Some((stop_below, _)) = stop
            && !orders.is_empty()
            && remaining.lock().unwrap_or_else(|e| e.into_inner()).rest < stop_below
        {
            let stats = OrderSharing { bound: Some(0), ..OrderSharing::default() };
            return Ok((OrdersOutcome::Stopped(stats), vec![None; orders.len()], None));
        }
        let stopped = AtomicBool::new(false);
        let failed = AtomicBool::new(false);
        let next = AtomicUsize::new(0);
        let halted = || cancelled() || stopped.load(Ordering::Relaxed) || failed.load(Ordering::Relaxed);
        type Part = (Vec<GroupedOrder>, ProgramCollector, OrderSharing, bool);
        let work = || -> Result<Part, Error> {
            let mut results = Vec::new();
            let mut collector = ProgramCollector::new(program_bytes, orders.len());
            let record = collector.rows.is_some();
            let mut stats = OrderSharing::default();
            let mut interrupted = false;
            while !halted() {
                let g = next.fetch_add(1, Ordering::Relaxed);
                let Some(group) = groups.get(g) else { break };
                let subset: Vec<Vec<usize>> = group.iter().map(|&i| orders[i].clone()).collect();
                let mut visit = |local: usize, model: &LiveModel| -> Result<i128, Error> {
                    let i = group[local];
                    let value = payoff(i, model)?;
                    results.push(GroupedOrder {
                        index: i,
                        score: model.score(),
                        final_life: model.current_life(),
                        draws: model.draws(),
                        payoff: value,
                    });
                    collector.visit(i, model)?;
                    if let Some((stop_below, caps)) = stop {
                        let mut r = remaining.lock().unwrap_or_else(|e| e.into_inner());
                        r.done = r.done.saturating_add(value);
                        r.rest = r.rest.saturating_sub(caps[i]);
                        r.left -= 1;
                        // Like the serial tree, only an order left to give up makes this a stop.
                        if r.left > 0 && r.done.saturating_add(r.rest) < stop_below {
                            stopped.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(value)
                };
                let mut upper = |ids: &[usize], _: &LiveModel, _: Settled| -> Result<ControlFlow<(), i128>, Error> {
                    if halted() {
                        return Ok(ControlFlow::Break(()));
                    }
                    Ok(ControlFlow::Continue(match stop {
                        Some((_, caps)) => ids.iter().fold(0i128, |a, &l| a.saturating_add(caps[group[l]])),
                        None => i128::MAX,
                    }))
                };
                // The group's own threshold never triggers: the shared remaining-sum check above decides.
                let bounds = Bounds { stop_below: i128::MIN, check_every, upper: &mut upper };
                let outcome = match self.play_orders(master, &subset, random.clone(), record, Some(bounds), &mut visit)
                {
                    Ok(outcome) => outcome,
                    Err(e) => {
                        failed.store(true, Ordering::Relaxed);
                        return Err(e);
                    }
                };
                let s = match outcome {
                    OrdersOutcome::Complete(s) | OrdersOutcome::Stopped(s) => s,
                    OrdersOutcome::Interrupted(s) => {
                        interrupted = true;
                        s
                    }
                };
                stats.frames += s.frames;
                stats.separate_frames += s.separate_frames;
                stats.branches += s.branches;
                stats.replayed += s.replayed;
                stats.clones += s.clones;
                stats.bounds += s.bounds;
            }
            Ok((results, collector, stats, interrupted))
        };
        let parts = std::thread::scope(|scope| {
            let handles: Vec<_> = (1..workers).map(|_| scope.spawn(work)).collect();
            let mut parts = vec![work()];
            for h in handles {
                parts.push(h.join().unwrap_or_else(|_| Err(Error::Domain("grouped order play panicked".into()))));
            }
            parts
        });
        let mut results = vec![None; orders.len()];
        let mut rows = Vec::new();
        let mut used = 0usize;
        let mut stats = OrderSharing::default();
        let mut interrupted = false;
        let mut recorded = program_bytes > 0;
        for part in parts {
            let (done, collector, s, halt) = part?;
            for r in done {
                results[r.index] = Some(r);
            }
            match collector.rows {
                Some(part_rows) if collector.collecting => {
                    used = used.saturating_add(collector.used);
                    rows.extend(part_rows);
                }
                _ => recorded = false,
            }
            stats.frames += s.frames;
            stats.separate_frames += s.separate_frames;
            stats.branches += s.branches;
            stats.replayed += s.replayed;
            stats.clones += s.clones;
            stats.bounds += s.bounds;
            interrupted |= halt;
        }
        let programs = (recorded && used <= program_bytes && !rows.is_empty()).then_some(rows);
        let outcome = if stopped.load(Ordering::Relaxed) {
            OrdersOutcome::Stopped(stats)
        } else if interrupted || results.iter().any(Option::is_none) {
            OrdersOutcome::Interrupted(stats)
        } else {
            OrdersOutcome::Complete(stats)
        };
        Ok((outcome, results, programs))
    }

    /// The positions whose chart skill events fire in frame `frame` of a model that has played the frames before it.
    fn announced_positions(&self, model: &LiveModel, frame: usize) -> u32 {
        let t = self.play.frames[frame].time_ms;
        self.events
            .iter()
            .zip(&model.fired)
            .filter(|&(&(_, at), &fired)| !fired && at <= t)
            .fold(0, |m, (&(position, _), _)| m | position_bit(position))
    }
}

struct ProgramCollector {
    rows: Option<Vec<RecordedOrder>>,
    programs: std::collections::HashSet<usize>,
    used: usize,
    budget: usize,
    expected: usize,
    collecting: bool,
}

impl ProgramCollector {
    fn new(budget: usize, orders: usize) -> Self {
        let used = orders.saturating_mul(std::mem::size_of::<RecordedOrder>());
        Self {
            rows: (used < budget).then(|| Vec::with_capacity(orders)),
            programs: Default::default(),
            used,
            budget,
            expected: orders,
            collecting: true,
        }
    }

    fn visit(&mut self, index: usize, model: &LiveModel) -> Result<(), Error> {
        if !self.collecting {
            return Ok(());
        }
        let Some(rows) = &mut self.rows else {
            return Ok(());
        };
        let Some(program) = model.recorded_score_program()? else {
            self.collecting = false;
            return Ok(());
        };
        let address = std::sync::Arc::as_ptr(&program) as usize;
        let bytes = if self.programs.insert(address) {
            program.allocated_bytes() + 2 * std::mem::size_of::<usize>()
        } else {
            0
        };
        self.used = self.used.saturating_add(bytes);
        if self.used > self.budget {
            self.collecting = false;
            self.programs.clear();
            return Ok(());
        }
        rows.push(RecordedOrder { index, program, final_life: model.current_life(), random_draws: model.draws() });
        Ok(())
    }

    fn finish(self, outcome: OrdersOutcome) -> Option<Vec<RecordedOrder>> {
        if matches!(outcome, OrdersOutcome::Complete(_))
            && self.rows.as_ref().is_some_and(|rows| rows.len() == self.expected)
        {
            self.rows
        } else {
            None
        }
    }

    fn finish_partial(self) -> Option<Vec<RecordedOrder>> {
        self.rows.filter(|rows| !rows.is_empty())
    }
}

impl Driver<'_, '_, '_> {
    /// The caller's bound on the payoff sum of the orders `ids`, which `model` plays.
    fn bound_of(&mut self, ids: &[usize], model: &mut LiveModel) -> Result<ControlFlow<(), i128>, Error> {
        let Some(bounds) = &mut self.bounds else { return Ok(ControlFlow::Continue(i128::MAX)) };
        let played = model.frames_played();
        let settled = model.settle(self.future[played]);
        self.stats.bounds += 1;
        (bounds.upper)(ids, &*model, settled)
    }

    /// The bound on the payoff sum of all orders.
    fn total_bound(&self) -> i128 {
        self.levels.iter().rev().fold(0, |below, l| {
            if l.split {
                let rest = l.rest.iter().fold(0i128, |s, &c| s.saturating_add(c));
                l.cap.min(l.done.saturating_add(rest).saturating_add(below))
            } else {
                l.cap
            }
        })
    }

    /// Stops once the bound on the payoff sum of all orders is below the threshold and some order is left to give up.
    fn check(&mut self) -> Option<Halt> {
        let stop_below = self.bounds.as_ref()?.stop_below;
        let bound = self.total_bound();
        self.stats.bound = Some(bound);
        (bound < stop_below && self.left > 0).then_some(Halt::Stopped)
    }

    fn visit_order(&mut self, i: usize, model: &LiveModel) -> Result<i128, Error> {
        self.left -= 1;
        (self.visit)(i, model)
    }

    /// Takes a new bound on the node being played, which `model` plays.
    fn refine(&mut self, ids: &[usize], model: &mut LiveModel) -> Result<Option<Halt>, Error> {
        match self.bound_of(ids, model)? {
            ControlFlow::Break(()) => Ok(Some(Halt::Interrupted)),
            ControlFlow::Continue(c) => {
                let level = self.levels.last_mut().expect("level of the node");
                level.cap = level.cap.min(c);
                Ok(self.check())
            }
        }
    }

    /// Plays a node whose orders' payoffs sum to at most `cap`; returns their exact sum.
    fn run_bounded(&mut self, node: Node, cap: i128) -> Result<ControlFlow<Halt, i128>, Error> {
        self.levels.push(Level { cap, split: false, done: 0, rest: Vec::new() });
        let flow = match self.check() {
            Some(halt) => ControlFlow::Break(halt),
            None => self.run_node(node)?,
        };
        let level = self.levels.pop().expect("level of the node");
        match flow {
            ControlFlow::Continue(sum) if sum > level.cap => {
                Err(Error::Input(format!("orders whose payoffs sum to {sum} were bounded by {}", level.cap)))
            }
            flow => Ok(flow),
        }
    }

    fn run_node(&mut self, node: Node) -> Result<ControlFlow<Halt, i128>, Error> {
        let Node { mut model, mut assign, fixed, orders: ids } = node;
        let (live, orders) = (self.live, self.orders);
        let end = live.play.frames.len();
        let start = model.frames_played();
        let every = self.bounds.as_ref().map_or(0, |b| b.check_every);
        let only = &orders[ids[0]];
        if ids.iter().all(|&i| orders[i] == *only) {
            model.move_to(&mut assign, only)?;
            let mut at = start;
            while at < end {
                let next = if every == 0 { end } else { end.min(at + every) };
                model.play_frames(&live.play, &live.delta_times, next)?;
                self.stats.frames += (next - at) as u64;
                at = next;
                if at < end
                    && let Some(halt) = self.refine(&ids, &mut model)?
                {
                    return Ok(ControlFlow::Break(halt));
                }
            }
            let mut sum = 0i128;
            for &i in &ids {
                sum += self.visit_order(i, &model)?;
            }
            return Ok(ControlFlow::Continue(sum));
        }
        let open = position_mask(assign.len()) & !fixed;
        // Play ahead on a copy until a member of an open position acts. A chart skill event of an open position is
        // known before its frame, so the copy itself is then the state before that frame; any other action is seen
        // only after its frame, and the frames since the node's start are played again on the original.
        let mut probe = model.clone();
        self.stats.clones += 1;
        let mut acted = None;
        for i in start..end {
            let t = live.play.frames[i].time_ms;
            let announced = live
                .events
                .iter()
                .zip(&probe.fired)
                .any(|(&(position, at), &fired)| !fired && at <= t && position_bit(position) & open != 0);
            if announced {
                acted = Some((i, true));
                break;
            }
            probe.play_frames(&live.play, &live.delta_times, i + 1)?;
            self.stats.frames += 1;
            if (probe.touched_events | probe.touched_skills) & open != 0 {
                acted = Some((i, false));
                break;
            }
            if every != 0
                && (i + 1 - start).is_multiple_of(every)
                && i + 1 < end
                && let Some(halt) = self.refine(&ids, &mut probe)?
            {
                return Ok(ControlFlow::Break(halt));
            }
        }
        let Some((frame, announced)) = acted else {
            // Nobody in an open position ever acted: the orders differ only in where idle members stand.
            let mut sum = 0i128;
            for &i in &ids {
                probe.move_to(&mut assign, &orders[i])?;
                sum += self.visit_order(i, &probe)?;
            }
            return Ok(ControlFlow::Continue(sum));
        };
        let (skills, events) = if announced {
            model = probe;
            (0, live.announced_positions(&model, frame) & open)
        } else {
            model.play_frames(&live.play, &live.delta_times, frame)?;
            self.stats.frames += (frame - start) as u64;
            self.stats.replayed += (frame - start) as u64;
            (probe.touched_skills & open, probe.touched_events & open)
        };
        self.stats.branches += 1;
        // Each child fixes one more (position, member) pair.
        let mut groups: Vec<(usize, usize, Vec<usize>)> = Vec::new();
        let mut add = |position: usize, member: usize, i: usize| match groups
            .iter_mut()
            .find(|c| c.0 == position && c.1 == member)
        {
            Some(c) => c.2.push(i),
            None => groups.push((position, member, vec![i])),
        };
        if skills != 0 {
            let member = assign[skills.trailing_zeros() as usize];
            for &i in &ids {
                let position = orders[i].iter().position(|&s| s == member).expect("checked order");
                add(position, member, i);
            }
        } else {
            let position = events.trailing_zeros() as usize;
            for &i in &ids {
                add(position, orders[i][position], i);
            }
        }
        let last = groups.len() - 1;
        let mut shared = Some(model);
        let mut children = Vec::with_capacity(groups.len());
        for (c, (position, member, child_ids)) in groups.into_iter().enumerate() {
            let mut child = if c == last {
                shared.take().expect("shared model")
            } else {
                self.stats.clones += 1;
                shared.as_ref().expect("shared model").clone()
            };
            let mut child_assign = assign.clone();
            let from = child_assign.iter().position(|&s| s == member).expect("member in arrangement");
            let mut map: Vec<usize> = (0..child_assign.len()).collect();
            map.swap(from, position);
            child.move_members(&map)?;
            child_assign.swap(from, position);
            let cap = match self.bound_of(&child_ids, &mut child)? {
                ControlFlow::Break(()) => return Ok(ControlFlow::Break(Halt::Interrupted)),
                ControlFlow::Continue(cap) => cap,
            };
            let fixed = fixed | position_bit(position as i32);
            children.push((cap, Node { model: child, assign: child_assign, fixed, orders: child_ids }));
        }
        // The largest bounds first: their exact payoffs replace the most of the bound early.
        children.sort_by_key(|c| Reverse(c.0));
        let level = self.levels.last_mut().expect("level of the node");
        level.split = true;
        level.rest = children.iter().rev().map(|c| c.0).collect();
        if let Some(halt) = self.check() {
            return Ok(ControlFlow::Break(halt));
        }
        let mut sum = 0i128;
        for (cap, child) in children {
            self.levels.last_mut().expect("level of the node").rest.pop();
            match self.run_bounded(child, cap)? {
                ControlFlow::Break(halt) => return Ok(ControlFlow::Break(halt)),
                ControlFlow::Continue(payoffs) => {
                    sum += payoffs;
                    self.levels.last_mut().expect("level of the node").done += payoffs;
                    if let Some(halt) = self.check() {
                        return Ok(ControlFlow::Break(halt));
                    }
                }
            }
        }
        Ok(ControlFlow::Continue(sum))
    }
}

/// The mask of positions `0..n`.
fn position_mask(n: usize) -> u32 {
    (0..n).fold(0, |m, k| m | position_bit(k as i32))
}

impl LiveModel {
    /// Rearranges the members from `assign` (position -> member) to `order`, updating `assign`.
    fn move_to(&mut self, assign: &mut [usize], order: &[usize]) -> Result<(), Error> {
        let mut map = Vec::with_capacity(assign.len());
        for member in assign.iter() {
            map.push(
                order
                    .iter()
                    .position(|s| s == member)
                    .ok_or_else(|| Error::Input("orders of different members".into()))?,
            );
        }
        self.move_members(&map)?;
        assign.copy_from_slice(order);
        Ok(())
    }

    /// Moves the member at performance position `k` to position `map[k]`. A member that has acted (see
    /// [`OrderedLive`]) keeps its position.
    pub(crate) fn move_members(&mut self, map: &[usize]) -> Result<(), Error> {
        let mut sorted = map.to_vec();
        sorted.sort_unstable();
        if sorted.iter().enumerate().any(|(i, &k)| i != k) {
            return Err(Error::Input(format!("{map:?} is not a permutation")));
        }
        let moved = map.iter().enumerate().filter(|&(k, &to)| k != to).fold(0, |m, (k, _)| m | position_bit(k as i32));
        if moved & (self.touched_events | self.touched_skills) != 0 {
            return Err(Error::Input("a member that has acted keeps its position".into()));
        }
        #[cfg(feature = "search-diagnostics")]
        if self.rush_probes.is_some() {
            return Err(Error::Input("members cannot move while rush probes record".into()));
        }
        let to = |k: usize| map.get(k).copied().ok_or_else(|| Error::Input(format!("no position for member {k}")));
        for s in &mut self.live {
            let (k, t) = (s.member, to(s.member)?);
            if t != k {
                s.member = t;
                s.key = s.key.wrapping_add(t as i64 - k as i64);
                for e in &mut s.effects {
                    for c in [e.condition.as_mut(), e.release.as_mut()].into_iter().flatten() {
                        c.move_positions(map);
                    }
                }
            }
        }
        for p in &mut self.live_pools {
            let (k, t) = (p.member, to(p.member)?);
            p.member = t;
            p.key = p.key.wrapping_add(t as i64 - k as i64);
        }
        self.live_pools.sort_by_key(|p| p.key);
        for c in &mut self.cond {
            let (k, t) = (c.member, to(c.member)?);
            if t != k {
                c.updater.move_position(map, k);
                c.member = t;
            }
        }
        self.cond.sort_by_key(CondSkill::list_key);
        Ok(())
    }
}
