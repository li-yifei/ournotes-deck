//! Versioned structured telemetry of one search (`ournotes-deck.telemetry/1`), carried by every recommendation
//! result and every session progress snapshot. Field meanings, units and the page/developer split are documented in
//! `docs/telemetry.md`.
//!
//! Every wall-clock quantity has a key ending in `Ms`. Removing those keys leaves content that is identical for
//! identical inputs whenever the search stops at the same point (in particular for every completed search).
//!
//! Recording is cheap by construction: counters are array increments, one clock read is added per joint node (the
//! node already read the clock for its deadline), and leaf activities (bounds, simulations) are timed once each.
use crate::clock::Instant;
use serde::Serialize;
use std::collections::BTreeMap;

pub const TELEMETRY_FORMAT: &str = "ournotes-deck.telemetry/1";
/// Joint search depths: members placed, leader first.
pub(crate) const DEPTHS: usize = 6;
/// Incumbent timeline entries kept; older entries are thinned to every other one when it fills.
const TIMELINE: usize = 256;
/// Chart-progress buckets of stopped simulations.
const STOP_BUCKETS: usize = 10;
/// Frontier levels tracked for the position-based progress.
const LEVELS: usize = 16;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Telemetry {
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<Box<crate::parallel::ParallelTelemetry>>,
    pub format: &'static str,
    pub environment: Environment,
    pub proof: Proof,
    pub incumbents: Incumbents,
    pub phases: Vec<Phase>,
    pub time: TimeBreakdown,
    /// Search tree nodes of every traversal (joint, composition, classes, exhaustive, session cursor steps).
    pub nodes: u64,
    pub leaves: Leaves,
    pub joint: Joint,
    pub composition: Composition,
    pub candidate: CandidateStrategy,
    pub caches: Caches,
    pub memory: Memory,
    /// Bounded nominal LUCK refinement, separately counted from the 120-order coarse scorer.
    pub lottery_refinement: LotteryRefinement,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            parallel: None,
            format: TELEMETRY_FORMAT,
            environment: Environment::default(),
            proof: Proof::default(),
            incumbents: Incumbents {
                updates: 0,
                stride: 1,
                timeline: Vec::new(),
                first_full: None,
                warm_start: WarmStart::default(),
            },
            phases: Vec::new(),
            time: TimeBreakdown::default(),
            nodes: 0,
            leaves: Leaves::default(),
            joint: Joint::default(),
            composition: Composition::default(),
            candidate: CandidateStrategy::default(),
            caches: Caches::default(),
            memory: Memory::default(),
            lottery_refinement: LotteryRefinement::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LotteryRefinement {
    pub attempted_orders: u64,
    /// Orders for which the provider completed every positive-mass nominal path.
    pub completed_orders: u64,
    /// Complete laws successfully installed in the ranking frontier.
    pub installed_orders: u64,
    pub declined_orders: u64,
    /// Complete laws whose search-side exact payoff arithmetic exceeded its representation.
    pub arithmetic_declines: u64,
    pub replay_runs: u64,
    pub terminal_paths: u64,
    pub frames: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Traversal {
    /// Nothing ran (preparation stopped, empty domain).
    #[default]
    None,
    /// Power/skip canonical member-set search.
    Canonical,
    /// One requested deck evaluated.
    Fixed,
    /// Joint member/Snap branch-and-bound (Gekisou objectives).
    Joint,
    /// Leader, member compositions and Snap pairings (Live without Gekisou, class schedules).
    Composition,
    /// Every physical deck, no bounds.
    Exhaustive,
    /// Heuristic proposals.
    Candidate,
    /// Resumable exhaustive session cursor.
    Session,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub crate_version: &'static str,
    /// `OURNOTES_DECK_COMMIT` at build time; null when the build did not declare it.
    pub commit: Option<&'static str>,
    pub features: Vec<&'static str>,
    pub arch: &'static str,
    pub os: &'static str,
    /// Built without debug assertions.
    pub optimized: bool,
    pub data: Option<DataIdentity>,
    pub route: Option<crate::handler::SolverRoute>,
    pub traversal: Traversal,
    pub k: usize,
    /// Effective deadline from the search start (after problem construction), and the other limits.
    pub time_limit_ms: Option<u64>,
    pub max_candidates: Option<u64>,
    pub cache_entries: usize,
    pub target: Option<Target>,
    pub domain: Option<Domain>,
    pub bounds: BoundSetup,
}

impl Default for Environment {
    fn default() -> Self {
        let mut features = Vec::new();
        if cfg!(feature = "search-diagnostics") {
            features.push("search-diagnostics");
        }
        Self {
            crate_version: env!("CARGO_PKG_VERSION"),
            commit: option_env!("OURNOTES_DECK_COMMIT"),
            features,
            arch: std::env::consts::ARCH,
            os: std::env::consts::OS,
            optimized: !cfg!(debug_assertions),
            data: None,
            route: None,
            traversal: Traversal::None,
            k: 0,
            time_limit_ms: None,
            max_candidates: None,
            cache_entries: 0,
            target: None,
            domain: None,
            bounds: BoundSetup::default(),
        }
    }
}

/// The dataset searched, from its provenance (absent fields are null) and the SHA-256 of its JSON text.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataIdentity {
    pub region: Option<String>,
    pub master_version: Option<String>,
    pub client_version: Option<String>,
    pub resource_version: Option<String>,
    pub sha256: Option<String>,
}

impl DataIdentity {
    pub(crate) fn of(data: &ournotes_sim::data::DeckData) -> Self {
        let text = |pointer: &str| data.provenance.pointer(pointer).and_then(|v| v.as_str()).map(str::to_owned);
        Self {
            region: text("/region"),
            master_version: text("/master/version"),
            client_version: text("/client/versionName"),
            resource_version: text("/catalog/resourceVersion"),
            sha256: data.sha256.clone(),
        }
    }
}

/// The value of a deck: the mean of its payoff over `orders` performance orders (120 for played lives, 1 for power
/// and skip).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub orders: usize,
    /// Denominator of every payoff numerator in this document.
    pub denominator: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Domain {
    pub members: usize,
    pub snaps: usize,
    pub required: usize,
    pub leader_fixed: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundSetup {
    /// Joint bounds compiled for branch-and-bound; false with exhaustive/candidate strategies or after a fallback.
    pub compiled: bool,
    /// Why branch-and-bound fell back to exhaustive traversal.
    pub fallback: Option<String>,
    pub compile_ms: f64,
    /// Joint (member, Snap choice) list length: the branching of every joint depth.
    pub choices: usize,
    /// Correlated and resource node bounds enabled (for the last search part started).
    pub correlated: bool,
    pub resource: bool,
    pub fine: bool,
    pub class_search: bool,
    pub pt_regime: Option<PtRegime>,
    pub conversion: Option<Conversion>,
    /// The deck payoff ranking (`deckPayoff` search) of the plan, or why a played Live has none.
    pub deck_payoff: Option<DeckPayoffSetup>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckPayoffSetup {
    /// Played Lives: the score cap of the strongest legal team, from which the payoff steps are read.
    pub score_cap: Option<String>,
    /// Why a played Live's payoff steps are not ranked by deck.
    pub refusal: Option<String>,
    /// Rankings run; each later one ranks again after evaluated decks paid less than their bound.
    pub rounds: usize,
    /// Evaluated decks that paid less than their bound.
    pub shortfalls: usize,
    /// The joint traversal took the search over.
    pub handed_over: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtRegime {
    pub members_removed: usize,
    pub fallback: Option<String>,
    pub compile_ms: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversion {
    /// Snaps selected from the compiled per-entry conversion reach.
    pub snaps: usize,
    /// Search parts (each partition with each of its slot rules), searched one after another.
    pub parts: usize,
    /// Domain envelopes compiled for these parts; several slot-rule parts share each envelope.
    pub prepared_domains: usize,
    pub fallback: Option<String>,
    pub compile_ms: f64,
}

/// Proof status. `upperBound` is a true upper bound of the payoff numerator of every deck the search has not
/// examined or pruned; with it, the best deck of the whole domain pays at most `max(best, upperBound)`.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proof {
    pub complete: bool,
    /// Position-based share of the search tree already decided, 0..=1 (1 when complete). Branches differ in size,
    /// so this is a progress indicator, not a time estimate. Null for traversals without a tracked position.
    pub fraction: Option<f64>,
    /// Sequential search parts and those finished (conversion partitions; 1 otherwise).
    pub parts: u64,
    pub parts_done: u64,
    /// Top-level branches (depth-0 choices, or leaders) decided in the current part, of the total.
    pub top_level_done: Option<u64>,
    pub top_level_total: Option<u64>,
    /// Payoff numerators over `environment.target.denominator`.
    pub best: Option<String>,
    pub kth: Option<String>,
    pub upper_bound: Option<String>,
    /// `(upperBound - x) / x` for the best and K-th payoffs, 0 when the bound does not exceed them.
    pub best_gap: Option<f64>,
    pub kth_gap: Option<f64>,
    /// Time spent computing `upperBound` after the stop (not part of the search deadline).
    pub bound_ms: f64,
    /// Upper bound of the best payoff over the whole domain: the larger of the best payoff and the bounds of the
    /// branches still open. Equals `best` for a complete nonempty deterministic result. At a stop, finalization
    /// folds the complete remaining domain across search parts; running multiple-part progress may be unknown.
    /// Null for an empty domain, an unknown bound, or a certified lottery frontier (use its payoff intervals).
    pub global_upper_bound: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Incumbents {
    /// Top-K insertions.
    pub updates: u64,
    /// Every `stride`-th update is in the timeline (doubling whenever it fills); the last update is always kept.
    pub stride: u64,
    pub timeline: Vec<IncumbentPoint>,
    /// The update that first filled the Top-K (the timeline may have thinned it out).
    pub first_full: Option<IncumbentPoint>,
    pub warm_start: WarmStart,
}

/// Incumbents of the joint search evaluated outside its traversal (`search/warm.rs`): the warm start before it (phase
/// `seed`) and the polishing around new best decks during it. Each is an exact evaluation of a legal deck.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarmStart {
    /// Exact evaluations of the warm start, and leaf-bound evaluations of its local search and of polishing.
    pub evaluations: u64,
    pub leaf_bound_checks: u64,
    /// The K-th payoff numerator after the warm start, once the Top-K is full.
    pub kth: Option<String>,
    /// Final Top-K decks that the warm start or polishing evaluated first.
    pub final_top_k: usize,
    /// Polishing after strictly better best decks: rounds, exact evaluations, wall time (inside the search phases).
    pub polish_rounds: u64,
    pub polish_evaluations: u64,
    pub polish_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncumbentPoint {
    /// Ordinal of the update (1-based).
    pub update: u64,
    pub at_ms: f64,
    pub nodes: u64,
    pub candidates: u64,
    pub simulations: u64,
    /// Decks in the Top-K after the update.
    pub filled: usize,
    pub best: String,
    /// The K-th payoff once the Top-K is full.
    pub kth: Option<String>,
    pub fraction: Option<f64>,
    /// The global upper bound at the update (`proof.globalUpperBound`).
    pub upper: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase {
    pub name: &'static str,
    pub label: Option<String>,
    /// Offset from the request start.
    pub start_ms: f64,
    pub wall_ms: f64,
    pub nodes: u64,
    pub candidates: u64,
    pub simulations: u64,
}

/// Exclusive wall time of the search loop by activity. The activities partition the time from the first search
/// phase to the end of the last one.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeBreakdown {
    /// Joint node work at each depth: bounds, choice loops and bookkeeping, excluding the activities below.
    pub depth_ms: [f64; DEPTHS],
    pub composition_ms: f64,
    /// Per-order raw and fine caps of complete teams.
    pub fine_bound_ms: f64,
    pub cutoff_table_ms: f64,
    pub simulation_ms: f64,
    /// Simulations stopped early by the cutoff.
    pub stopped_simulation_ms: f64,
    /// Warm start and polishing, outside the simulations and cutoff tables they run.
    pub warm_start_ms: f64,
    pub other_ms: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leaves {
    /// Candidate decks handed to evaluation (cache hits in `caches.candidates`), and the new decks among them.
    pub proposed: u64,
    pub visited: u64,
    /// Decks evaluated completely (every performance order).
    pub evaluated: u64,
    /// A stop interrupted their evaluation.
    pub partial: u64,
    /// Played-live teams dropped before any simulation because the sum of their per-order cheap caps, or of their
    /// raw and fine caps, stays below the K-th.
    pub cheap_pruned: u64,
    pub fine_pruned: u64,
    /// Performance orders whose raw and fine caps were computed; the computation of a team's caps stops once
    /// they prove it below the K-th.
    pub fine_orders: u64,
    /// Played-live teams whose performance orders started to run.
    pub started: u64,
    /// Teams dropped after some orders: their exact payoffs plus the caps of the other orders stay below the K-th.
    pub order_bound_pruned: u64,
    /// Whole-live simulations (one performance order each) run to the end.
    pub simulations: u64,
    pub cutoff: Cutoff,
    pub order_tree: OrderTree,
    pub peak_retained: usize,
    /// Diagnostics only: the census of the teams the caps leave open at a fixed threshold (`set_census`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub census: Option<Census>,
}

/// The teams whose per-order caps reach a census threshold, counted instead of simulated.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Census {
    pub threshold: String,
    /// Teams counted, and the distinct member sets among them.
    pub open: u64,
    pub member_sets: u64,
    /// Teams reaching a leaf without joint bounds (not counted in `open`).
    pub unbounded: u64,
    /// The counted teams by the ratio of their cap sum to the threshold, minus one: below each of
    /// `CENSUS_EDGES`, then at least the last one; once with the cheap caps, once with the fine caps.
    pub cheap_ratio: [u64; CENSUS_EDGES.len() + 1],
    pub fine_ratio: [u64; CENSUS_EDGES.len() + 1],
    /// Per traversal with an ordered root: the root children whose depth-1 bound reaches the threshold, and all.
    pub root: Vec<[usize; 2]>,
    /// The first `CENSUS_TEAMS` counted teams: member card IDs, Snap IDs, power, cheap and fine cap sums.
    pub teams: Vec<CensusTeam>,
    #[serde(skip)]
    sets: std::collections::HashSet<[usize; 5]>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CensusTeam {
    pub members: [i64; 5],
    pub snaps: [Option<i64>; 5],
    pub power: i32,
    pub cheap: String,
    pub fine: String,
}

pub const CENSUS_EDGES: [f64; 8] = [0.001, 0.0025, 0.005, 0.01, 0.02, 0.03, 0.05, 0.08];
const CENSUS_TEAMS: usize = 100_000;

impl Census {
    pub(crate) fn new(threshold: i128) -> Self {
        Self { threshold: threshold.to_string(), ..Default::default() }
    }

    pub(crate) fn record(&mut self, members: [usize; 5], team: CensusTeam, cheap: i128, fine: i128) {
        if self.teams.len() < CENSUS_TEAMS {
            self.teams.push(team);
        }
        let threshold: f64 = self.threshold.parse().expect("census threshold");
        let bucket = |cap: i128| {
            let excess = cap as f64 / threshold - 1.0;
            CENSUS_EDGES.iter().position(|&edge| excess < edge).unwrap_or(CENSUS_EDGES.len())
        };
        self.cheap_ratio[bucket(cheap)] += 1;
        self.fine_ratio[bucket(fine)] += 1;
        self.open += 1;
        let mut set = members;
        set.sort_unstable();
        if self.sets.insert(set) {
            self.member_sets += 1;
        }
    }
}

/// The performance orders of a team played as one tree (frames the orders share are played once).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderTree {
    /// Frames played, every replay included, and the frames a separate simulation of each order plays.
    pub frames: u64,
    pub separate_frames: u64,
    pub branches: u64,
    pub replayed: u64,
    pub clones: u64,
    /// Calls of the payoff bound of a node.
    pub bounds: u64,
}

impl OrderTree {
    pub(crate) fn add(&mut self, s: &ournotes_sim::live::full::OrderSharing) {
        self.frames += s.frames;
        self.separate_frames += s.separate_frames;
        self.branches += s.branches;
        self.replayed += s.replayed;
        self.clones += s.clones;
        self.bounds += s.bounds;
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cutoff {
    /// Performance orders simulated with a cutoff table, and those without a finite table.
    pub tables: u64,
    pub unavailable: u64,
    /// Simulations stopped early (their candidate cannot reach the Top-K), and the chart progress (played frames)
    /// at the stop in tenths: `stoppedAt[i]` counts stops in `[i/10, (i+1)/10)`.
    pub stopped: u64,
    pub stopped_at: [u64; STOP_BUCKETS],
}

/// Bound checks and prunes by the depth of the node checked (tail and pair: the parent's depth).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checks {
    pub checks: [u64; DEPTHS],
    pub pruned: [u64; DEPTHS],
}

impl Checks {
    #[inline]
    pub(crate) fn check(&mut self, depth: usize) {
        self.checks[depth] += 1;
    }
    #[inline]
    pub(crate) fn prune(&mut self, depth: usize) {
        self.pruned[depth] += 1;
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Joint {
    pub nodes: [u64; DEPTHS],
    pub branch: Checks,
    /// Assignment power cap, checked on payoff ties only.
    pub assignment: Checks,
    pub correlated: Checks,
    pub resource: Checks,
    pub bonus: Checks,
    pub bonus_unavailable: [u64; DEPTHS],
    pub tail: Checks,
    pub tail_choices_skipped: [u64; DEPTHS],
    pub pair: Checks,
    pub root_order: RootOrder,
    pub carriers: Carriers,
    /// Node bounds (branch, bonus) and pair bounds equal to the K-th payoff, surviving on power.
    pub node_ties: [u64; DEPTHS],
    pub pair_ties: [u64; DEPTHS],
    /// PT warm start: prefixes outside the maximum-bonus regime.
    pub seed_bonus_skipped: [u64; DEPTHS],
    /// Bound modules by name.
    pub modules: BTreeMap<&'static str, Count>,
}

/// Gekisou score with a combo range: the cheap bounds of decks with at most `n` combo carriers (a member and Snap
/// bringing combo bonus windows). A node with `c` carriers placed and `r` slots to fill reads level `c + r`.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Carriers {
    /// Levels compiled apart from the pool-wide bounds (the largest over the search parts).
    pub levels: usize,
    /// Bounded nodes by the level they read, 0 to 5.
    pub nodes: [u64; 6],
}

/// Root children visited in descending order of their depth-1 bound: once one is strictly inferior to the K-th, the
/// loop skips it and every later one (each would be pruned at depth 1).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootOrder {
    pub skipped: u64,
    /// Root traversals (whole domain, or one Gekisou conversion part with its slot rules) whose best root child was
    /// already inferior.
    pub traversals_pruned: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    pub checks: u64,
    pub pruned: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Composition {
    /// Member-composition nodes by members placed (leader first).
    pub member_nodes: [u64; DEPTHS],
    pub snap_nodes: u64,
    pub class_nodes: u64,
    pub binding_nodes: u64,
    /// Member sets reached.
    pub compositions: u64,
    pub composition: Count,
    /// Bound modules by name (`memberAdditive`: score of Lives without Gekisou).
    pub modules: BTreeMap<&'static str, Count>,
    /// Bounds of a whole composition and of its partial Snap pairings.
    pub team: Count,
    pub class: Count,
    pub class_binding: Count,
    pub class_infeasible: u64,
    pub class_resource_checks: u64,
    pub class_resource_tightened: u64,
    pub seeds: Seeds,
    pub power_frontier_closed: u64,
}

/// Seed decks evaluated before their composition's Snap search.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Seeds {
    pub preseed: u64,
    pub team: u64,
    pub weighted: u64,
    pub class: u64,
    pub power_frontier: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateStrategy {
    pub warmup_member_sets: usize,
    pub warmup_proposals: u64,
    pub exploration_proposals: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheUse {
    pub lookups: u64,
    pub hits: u64,
    /// Entries dropped (one at a time, or all at once when the cache is cleared).
    pub evictions: u64,
    pub peak_entries: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Caches {
    /// Decks (teams in their canonical layout for played lives) already evaluated or pruned.
    pub candidates: CacheUse,
    /// Complete 120-order score laws reused across leaders with the same member/Snap pairs and exact power.
    pub team_scores: CacheUse,
    /// Native programs reused across powers for identical member and Performer classes.
    pub programs: CacheUse,
    /// Complete-identity encounters after leaf bounds, at native order-tree starts. A hit admits recording.
    pub program_admissions: CacheUse,
    pub program_recordings: u64,
    /// Unique exported node/byte totals, including exports later evicted or refused by the resident budget.
    pub program_recorded_nodes: u64,
    pub program_recorded_bytes: u64,
    /// Cached exact program evaluation only (not key lookup or the fresh native simulation).
    pub program_evaluation_ms: f64,
    /// Entire native order-tree work with recording enabled, including its original native simulation.
    /// This is not an estimate of the recording overhead; use controlled A/B runs for that comparison.
    pub program_recording_ms: f64,
    pub program_orders_reused: u64,
    pub program_bytes: usize,
    /// PT bonus cap rows by prefix state; `bonusRowsRefused` lookups found the table full.
    pub bonus_rows: CacheUse,
    pub bonus_rows_refused: u64,
    /// Rush entry windows by (spec, masks).
    pub rush_windows: CacheUse,
    /// Certified lottery curves of LUCK lives, shared across performance orders and teams.
    pub luck_curves: LuckCurves,
}

/// Use of the certified lottery-curve cache.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuckCurves {
    pub usage: CacheUse,
    /// The largest total key size held.
    pub peak_key_bytes: usize,
    /// Compiled recorder states considered and reused within score sessions.
    pub recording_lookups: u64,
    pub recording_hits: u64,
    /// Diagnostic builds only: the reduced recordings and the propagations of newly encountered curves.
    pub record_ms: f64,
    pub propagate_ms: f64,
}

impl LuckCurves {
    pub(crate) fn record(&mut self, stats: ournotes_sim::live::full::LuckDpCacheStats) {
        *self = Self {
            usage: CacheUse {
                lookups: stats.lookups,
                hits: stats.hits,
                evictions: stats.evictions,
                peak_entries: stats.peak_entries,
            },
            peak_key_bytes: stats.peak_key_bytes,
            recording_lookups: stats.recording_lookups,
            recording_hits: stats.recording_hits,
            record_ms: stats.record_ms,
            propagate_ms: stats.propagate_ms,
        };
    }
}

/// Memory of the running program when the document was written.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    /// The most memory the program has held so far: on WebAssembly the size of its linear memory
    /// (`memory.buffer.byteLength`, which never shrinks); on Linux the peak resident set size of the process; null
    /// elsewhere.
    pub peak_bytes: Option<u64>,
}

impl Memory {
    pub(crate) fn now() -> Self {
        Self { peak_bytes: peak_bytes() }
    }
}

#[cfg(target_arch = "wasm32")]
fn peak_bytes() -> Option<u64> {
    Some(core::arch::wasm32::memory_size::<0>() as u64 * 65_536)
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]
fn peak_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "linux")))]
fn peak_bytes() -> Option<u64> {
    None
}

/// Exclusive-time activities of the search loop.
pub(crate) mod slot {
    pub(crate) const COMPOSITION: usize = super::DEPTHS;
    pub(crate) const FINE: usize = COMPOSITION + 1;
    pub(crate) const CUTOFF_TABLE: usize = FINE + 1;
    pub(crate) const SIMULATION: usize = CUTOFF_TABLE + 1;
    pub(crate) const STOPPED: usize = SIMULATION + 1;
    pub(crate) const WARM: usize = STOPPED + 1;
    pub(crate) const OTHER: usize = WARM + 1;
    pub(crate) const COUNT: usize = OTHER + 1;
}

/// Exclusive activity clock: each lap charges the time since the previous lap to the current activity.
pub(crate) struct Clock {
    mark: Instant,
    slot: usize,
    nanos: [u64; slot::COUNT],
}

impl Clock {
    pub(crate) fn new(now: Instant) -> Self {
        Self { mark: now, slot: slot::OTHER, nanos: [0; slot::COUNT] }
    }
    /// Charge the elapsed time to the current activity and switch to `next`; returns the time read and the activity
    /// left.
    #[inline]
    pub(crate) fn lap(&mut self, next: usize) -> (Instant, usize) {
        let now = super::budget::now();
        self.nanos[self.slot] += now.saturating_duration_since(self.mark).as_nanos() as u64;
        self.mark = now;
        (now, std::mem::replace(&mut self.slot, next))
    }
    /// Like `lap`, charging the elapsed time to `charged` instead of the current activity.
    #[inline]
    pub(crate) fn lap_as(&mut self, charged: usize, next: usize) {
        let now = super::budget::now();
        self.nanos[charged] += now.saturating_duration_since(self.mark).as_nanos() as u64;
        self.mark = now;
        self.slot = next;
    }
    /// Close the clock into the breakdown.
    pub(crate) fn add_to(&mut self, time: &mut TimeBreakdown) {
        self.lap(slot::OTHER);
        add_nanos(&self.nanos, time);
        self.nanos = [0; slot::COUNT];
    }
    /// The breakdown `add_to` would give at `now`, leaving the clock running.
    pub(crate) fn peek_into(&self, now: Instant, time: &mut TimeBreakdown) {
        let mut nanos = self.nanos;
        nanos[self.slot] += now.saturating_duration_since(self.mark).as_nanos() as u64;
        add_nanos(&nanos, time);
    }
}

fn add_nanos(nanos: &[u64; slot::COUNT], time: &mut TimeBreakdown) {
    let ms = |n: u64| n as f64 / 1e6;
    for d in 0..DEPTHS {
        time.depth_ms[d] += ms(nanos[d]);
    }
    time.composition_ms += ms(nanos[slot::COMPOSITION]);
    time.fine_bound_ms += ms(nanos[slot::FINE]);
    time.cutoff_table_ms += ms(nanos[slot::CUTOFF_TABLE]);
    time.simulation_ms += ms(nanos[slot::SIMULATION]);
    time.stopped_simulation_ms += ms(nanos[slot::STOPPED]);
    time.warm_start_ms += ms(nanos[slot::WARM]);
    time.other_ms += ms(nanos[slot::OTHER]);
}

/// The current path of a traversal: at each level the index of the branch being explored and the branch count.
/// Everything before the path in traversal order is decided.
#[derive(Default)]
pub(crate) struct Frontier {
    len: usize,
    at: [(u32, u32); LEVELS],
}

impl Frontier {
    #[inline]
    pub(crate) fn set(&mut self, level: usize, index: usize, count: usize) {
        self.at[level] = (index as u32, count as u32);
        self.len = level + 1;
    }
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }
    pub(crate) fn fraction(&self) -> f64 {
        let (mut done, mut scale) = (0.0, 1.0);
        for &(index, count) in &self.at[..self.len] {
            debug_assert!(index < count);
            scale /= count as f64;
            done += index as f64 * scale;
        }
        done
    }
    /// Decided top-level branches and their count.
    pub(crate) fn top_level(&self) -> Option<(u64, u64)> {
        (self.len > 0).then(|| (self.at[0].0 as u64, self.at[0].1 as u64))
    }
}

/// An open phase: its index, start and the node, candidate and simulation counts at its start.
type OpenPhase = (usize, Instant, [u64; 3]);

/// Recording state that is not part of the wire document.
pub(crate) struct Recorder {
    pub(crate) origin: Instant,
    pub(crate) clock: Clock,
    pub(crate) frontier: Frontier,
    open: Option<OpenPhase>,
    /// Sequential search parts and those finished.
    pub(crate) parts: u64,
    pub(crate) parts_done: u64,
    /// Whether the running traversal maintains `frontier`.
    pub(crate) tracked: bool,
    /// Whether the running traversal bounds what a stop leaves unexplored, and that bound (None: nothing left).
    pub(crate) bounded: bool,
    pub(crate) unexplored: Option<i128>,
    /// The global upper bound so far (`Proof::global_upper_bound`).
    pub(crate) upper: Option<i128>,
}

impl Recorder {
    pub(crate) fn new(origin: Instant) -> Self {
        Self {
            origin,
            clock: Clock::new(super::budget::now()),
            frontier: Frontier::default(),
            open: None,
            parts: 1,
            parts_done: 0,
            tracked: false,
            bounded: false,
            unexplored: None,
            upper: None,
        }
    }

    /// Offer a global upper bound; the recorded one is the smallest offered.
    pub(crate) fn offer_upper(&mut self, upper: i128) {
        self.upper = Some(self.upper.map_or(upper, |u| u.min(upper)));
    }

    /// Position-based progress over all parts.
    pub(crate) fn progress(&self) -> Option<f64> {
        self.tracked.then(|| (self.parts_done as f64 + self.frontier.fraction()) / self.parts as f64)
    }

    pub(crate) fn since_origin_ms(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.origin).as_secs_f64() * 1000.0
    }

    /// Open a phase; the previous one must be closed.
    pub(crate) fn begin(&mut self, tel: &mut Telemetry, name: &'static str, label: Option<String>) {
        assert!(self.open.is_none(), "phase {name} opened inside another phase");
        let now = super::budget::now();
        tel.phases.push(Phase {
            name,
            label,
            start_ms: self.since_origin_ms(now),
            wall_ms: 0.0,
            nodes: 0,
            candidates: 0,
            simulations: 0,
        });
        self.open = Some((tel.phases.len() - 1, now, [tel.nodes, tel.leaves.visited, tel.leaves.simulations]));
    }

    pub(crate) fn end(&mut self, tel: &mut Telemetry) {
        let open = self.open.take().expect("an open phase");
        self.clock.lap(slot::OTHER);
        close_phase(tel, open, super::budget::now());
    }

    /// Close any open phase (an error or a stop unwound through it).
    pub(crate) fn end_open(&mut self, tel: &mut Telemetry) {
        if self.open.is_some() {
            self.end(tel);
        }
    }

    /// Close the open phase, if any, at `now` in a copy of the document, leaving it open here.
    pub(crate) fn peek_open(&self, tel: &mut Telemetry, now: Instant) {
        if let Some(open) = self.open {
            close_phase(tel, open, now);
        }
    }

    /// Fold an upper bound of a region the stop leaves unexplored.
    pub(crate) fn unexplored(&mut self, upper: i128) {
        self.unexplored = Some(self.unexplored.map_or(upper, |u| u.max(upper)));
    }

    /// Record a Top-K insertion: the best payoff, the K-th once the Top-K is full, and the decks held.
    pub(crate) fn incumbent(&self, tel: &mut Telemetry, best: i128, kth: Option<i128>, filled: usize) {
        tel.incumbents.updates += 1;
        if kth.is_some() && tel.incumbents.first_full.is_none() {
            tel.incumbents.first_full = Some(self.point(tel, best, kth, filled, self.progress()));
        }
        let i = &mut tel.incumbents;
        if !(i.updates - 1).is_multiple_of(i.stride) {
            return;
        }
        if i.timeline.len() == TIMELINE {
            let mut keep = 0;
            i.timeline.retain(|_| {
                keep += 1;
                keep % 2 == 1
            });
            i.stride *= 2;
            if !(i.updates - 1).is_multiple_of(i.stride) {
                return;
            }
        }
        let point = self.point(tel, best, kth, filled, self.progress());
        tel.incumbents.timeline.push(point);
    }

    /// The last update is always in the timeline.
    pub(crate) fn close_timeline(&self, tel: &mut Telemetry, best: i128, kth: Option<i128>, filled: usize) {
        let i = &tel.incumbents;
        if i.updates > 0 && i.timeline.last().is_none_or(|p| p.update != i.updates) {
            let point = self.point(tel, best, kth, filled, None);
            tel.incumbents.timeline.push(point);
        }
    }

    /// The incumbent standing at the latest update.
    fn point(
        &self,
        tel: &Telemetry,
        best: i128,
        kth: Option<i128>,
        filled: usize,
        fraction: Option<f64>,
    ) -> IncumbentPoint {
        IncumbentPoint {
            update: tel.incumbents.updates,
            at_ms: self.since_origin_ms(super::budget::now()),
            nodes: tel.nodes,
            candidates: tel.leaves.visited,
            simulations: tel.leaves.simulations,
            filled,
            best: best.to_string(),
            kth: kth.map(|v| v.to_string()),
            fraction,
            upper: self.upper.map(|v| v.to_string()),
        }
    }
}

fn close_phase(tel: &mut Telemetry, (index, started, [nodes, candidates, simulations]): OpenPhase, now: Instant) {
    let phase = &mut tel.phases[index];
    phase.wall_ms = now.saturating_duration_since(started).as_secs_f64() * 1000.0;
    phase.nodes = tel.nodes - nodes;
    phase.candidates = tel.leaves.visited - candidates;
    phase.simulations = tel.leaves.simulations - simulations;
}

/// Record where in the chart a stopped simulation stopped.
pub(crate) fn record_stop(cutoff: &mut Cutoff, played: usize, frames: usize) {
    cutoff.stopped += 1;
    let bucket = (played * STOP_BUCKETS).checked_div(frames).unwrap_or(0).min(STOP_BUCKETS - 1);
    cutoff.stopped_at[bucket] += 1;
}

/// `(upper - x) / x`, zero when the bound does not exceed `x`; null for a nonpositive `x`.
pub(crate) fn gap(upper: i128, x: i128) -> Option<f64> {
    (x > 0).then(|| if upper <= x { 0.0 } else { (upper - x) as f64 / x as f64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontier_fraction_is_the_mixed_radix_position() {
        let mut f = Frontier::default();
        assert_eq!(f.fraction(), 0.0);
        f.set(0, 1, 4);
        assert_eq!(f.fraction(), 0.25);
        f.set(1, 2, 4);
        assert_eq!(f.fraction(), 0.25 + 2.0 / 16.0);
        f.set(0, 3, 4);
        assert_eq!(f.fraction(), 0.75);
        assert_eq!(f.top_level(), Some((3, 4)));
    }

    #[test]
    fn timeline_thins_to_every_other_update_and_keeps_the_last() {
        let rec = Recorder::new(super::super::budget::now());
        let mut tel = Telemetry::default();
        for v in 1..=1000 {
            tel.nodes = v as u64;
            rec.incumbent(&mut tel, v, None, 1);
        }
        assert!(tel.incumbents.timeline.len() <= TIMELINE);
        assert_eq!(tel.incumbents.stride, 4);
        assert!(tel.incumbents.timeline.iter().all(|p| (p.update - 1) % 4 == 0));
        rec.close_timeline(&mut tel, 1000, None, 1);
        assert_eq!(tel.incumbents.timeline.last().unwrap().update, 1000);
        assert_eq!(tel.incumbents.timeline[0].update, 1);
    }

    #[test]
    fn the_first_full_top_k_is_kept_apart_from_the_thinned_timeline() {
        let rec = Recorder::new(super::super::budget::now());
        let mut tel = Telemetry::default();
        for v in 1..=600 {
            tel.nodes = v as u64;
            let kth = (v >= 3).then_some(v - 2);
            rec.incumbent(&mut tel, v, kth, (v as usize).min(3));
        }
        let first = tel.incumbents.first_full.as_ref().expect("the Top-K filled");
        assert_eq!((first.update, first.nodes, first.filled), (3, 3, 3));
        assert_eq!(first.kth.as_deref(), Some("1"));
        assert!(tel.incumbents.timeline.iter().all(|p| p.update != 3));
    }

    #[test]
    fn stops_fall_in_tenths_of_the_chart() {
        let mut c = Cutoff::default();
        record_stop(&mut c, 0, 300);
        record_stop(&mut c, 299, 300);
        record_stop(&mut c, 150, 300);
        assert_eq!(c.stopped, 3);
        assert_eq!(c.stopped_at[0], 1);
        assert_eq!(c.stopped_at[5], 1);
        assert_eq!(c.stopped_at[9], 1);
    }

    #[test]
    fn gaps_are_relative_and_never_negative() {
        assert_eq!(gap(110, 100), Some(0.1));
        assert_eq!(gap(90, 100), Some(0.0));
        assert_eq!(gap(5, 0), None);
    }
}
