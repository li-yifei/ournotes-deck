# Native parallel search, first version

The macOS FFI and local HTTP recommendation entry points use
`ournotes_search::parallel`. One recommendation is partitioned into disjoint
complementary member-included/member-excluded domains for ordinary Live.
All legal leaders of a member composition stay in the same task, preserving the
worker's exact 120-order score-law cache. Power/Skip start with leader domains,
then membership decisions, retaining their stronger leader-specific bounds.
Fixed-leader requests also split through membership decisions. An atomic
task index distributes work dynamically. The worker allowance defaults to
`available_parallelism().div_ceil(2)` (logical CPUs visible to the process).
Native transport admission serializes simultaneous requests around this shared
allowance. An explicit worker count accepts `1..=available_parallelism()`;
invalid counts return input errors. The serial `engine::recommend` remains available for oracle comparisons.
The account FFI opts its typed solver call into the same native path.

Every worker borrows the immutable dataset and roster and builds its own pool,
bounds and mutable caches. Rc/RefCell-based search state stays inside one worker.
A shared mutex protects the exact global Top-K. Incumbent updates publish a
numeric payoff/power pair through an RwLock; node checks read this pair without
locking the full ranking or parsing decimal strings. Complete candidate values
publish safe primary/payoff-power cutoffs. Equal bounds stay open for canonical
ID ties. The cutoff is used by power, deck-payoff and composition traversals.
All candidates entering the global ranking are fully evaluated. Partition
completion proves exhaustion/pruning conditional on these global incumbents;
the coordinator certifies the original domain only after every part completes.

The deadline includes initial preparation and all worker preparation. Candidate
visits consume a single atomic request budget. `Cancellation` is a cooperative
request-local token; running atomic evaluations may complete before stopping.
Unstarted parts keep the global result unproven. A partition's mutable caches
are never reused as an authority for a different request.

The native path parallelizes Power, Skip, ordinary Live and deterministic
Gekisou, including Snap skills. Candidate strategy retains its sequential
proposal stream. Certified LUCK keeps a request-local serial frontier while
parallelizing order scoring and exact-law refinement batches.
These fallbacks are explicit in `telemetry.parallel.fallback`. Worker progress
callbacks remain on the serial API in this version. The native FFI currently
returns the final answer. WASM behavior remains unchanged.

`telemetry.parallel` reports the worker allowance, workers used, task counts,
global candidate count, cancellation and individual partition telemetry. Top
level proof describes the original domain; partition phase/counter details are
kept separately. A fixed domain with only one feasible member team can remain
serial. Snap-only partitioning is a future improvement.

## Validation and reproducible measurement

```sh
cargo test --release -p ournotes-search --test power_team_identity parallel
cargo test --release -p ournotes-search --test power_team_identity parallel_live_throughput -- --ignored --nocapture
cargo run --release -p ournotes-search --example parallel-search -- DATA ROSTER REQUEST 4
```

Tests compare serial and parallel results with an independent small-domain
oracle, including exact fractions, canonical ties, fixed leaders, member/Snap
constraints, ordinary Live/Snap skills, cancellation, zero deadlines and a
global candidate budget. The throughput test is a manual synthetic measurement;
it is not a timing assertion. Native resource versions and CPU contention can
change throughput.

Repeated per-part pool/bound construction and per-worker caches are the main
first-version costs. Small requests can take longer than the serial solver.
Next: reuse immutable precomputation and reduce root partition construction
without changing canonical result/proof rules.

## Configuring the worker allowance

- Rust: `parallel::recommend(..., workers, cancellation)`,
  `recommend_json_with_threads(data, roster, request, workers)` and
  `with_native_threads(workers, || engine::recommend_account(...))`.
- C ABI: `ournotes_recommend_json_with_threads(path, roster, request, uint32_t threads)`
  and `ournotes_recommend_account_json_with_threads(path, account, request, uint32_t threads)`.
  Existing three-argument functions use the default. Free returned JSON using
  `ournotes_free_string`. `ournotes_search_threads_json()` returns
  `{"min":1,"max":N,"default":ceil(N/2)}`.
- HTTP: POST `/recommend` with `{"roster":{...},"request":{...},"threads":4}`.
  Omit `threads` to use the default; fractions, strings, zero and values above N
  return 422. GET `/health` includes the range/default in its `threads` field.
- CLI: final positional argument to `parallel-search` selects the worker count.

The allowance is a maximum: small domains and serial fallbacks can use fewer
threads. `telemetry.parallel.workers` reports the requested allowance and
`workersUsed` reports the workers that executed tasks.

## Arena assessment and measured optimization (2026-10-06)

Rc supports shared ownership within a worker; RefCell supports caches populated
through shared references. These are worker-local, so cross-thread races and
atomic reference-count traffic do not account for their cost. Their remaining
costs are allocation/indirection, non-atomic reference counting and dynamic
borrow checks. Replacing them requires separating immutable bounds from mutable
worker scratch, passing `&mut` cache state explicitly, and using stable arena
indexes where shared ownership was previously needed. Arena allocation alone
keeps the RefCell checks if the mutation API stays the same.

The demonstrated higher-cost problem was splitting different leaders of the
same composition into separate engines, discarding exact score-law reuse.
Release synthetic test: 8 members, 2 Snaps, 64 chart notes, 8,680 evaluated teams:

| Configuration | Simulations | Elapsed ms |
| --- | ---: | ---: |
| Serial worker | 215,040 | 290 |
| 4 workers, earlier leader partitions | 1,041,600 | 291 |
| 4 workers, member partitions | 215,040 | 105 |

The 4-worker revision was about 2.8x faster in this sample, with unchanged team
results and candidate counts. A local roster sample (5 paired runs) improved
ordinary Live wall-time median from 504.87 to 243.00 ms. Its locally available
chart has only four notes, so it does not represent full-game chart throughput.
Member partitions slowed Power (37.20 to 75.95 ms); Power/Skip therefore retain
leader partitions. The numeric-cutoff change alone showed comparable times
(Power 45.52 to 45.52 ms; Live 525.01 to 526.77 ms, 7 paired runs).

Prioritize cache-preserving partitioning and precomputation reuse. Profile
allocation and borrow-check hot spots before committing to a broad arena rewrite.


## CarrierSplit arena experiment (2026-10-06)

CarrierSplit now separates immutable lookup/bound tables from one request-local
SplitCache. The cache holds envelopes and coupled tables in Vec arenas; hash
maps store indexes. References live inside one node check, and cache eviction
clears the index maps and arenas together. `Rc<SplitEnv>` and `Rc<Coupled>` are
removed. Gain values use `Cell<f64>`, and fixed-size placed/list buffers use
stack arrays. A single outer `RefCell` borrow per node preserves the existing
`JointBounds` shared-reference interface; the shared immutable CarrierKeys Rc
remains, cloned at compilation. Lazy tables continue using OnceCell.

The fixture explicitly activates effect 12000 (combo bonus). Exact fixed-deck
evaluations audit every sampled prefix. Cold, warm and clear/rebuilt arena
values match, and production Top-K is compared to exhaustive search. A
diagnostics-only reset uses the same clear operation as normal cache eviction.

Seven alternating benchmark rounds, five warm passes per round (35 samples
per version), 15,600 prefix checks per pass, identical bound checksums:

| Version | Median ms |
| --- | ---: |
| Original Rc/RefCell caches | 78.216 |
| Index arenas and one cache borrow | 78.974 |
| Arenas plus fixed-size stack buffers | 78.602 |

Whole-prefix throughput is comparable in this synthetic workload. This is a
local ownership/allocation simplification; these measurements provide limited
evidence for a speed gain. CarrierSplit serves Gekisou bounds, whose native
parallel entry point still uses its serial/certified-frontier fallback. Measure
further changes against request-level preparation and search counters; prioritize
fewer bound evaluations and reused precomputation over expanding arena conversion.

```sh
cargo test --release -p ournotes-search --features search-diagnostics --test adapter_fixture_export carrier_cache
cargo test --release -p ournotes-search --features search-diagnostics --test adapter_fixture_export carrier_cache_throughput -- --ignored --nocapture
```

## Global cutoff and warm-start sharing (2026-10-06)

Composition member/Snap bounds now run as soon as either the local Top-K or the
request-wide exact ranking supplies a cutoff. Local identity tie-breaks continue
to read only the local full Top-K; the global threshold closes strictly inferior
payoff/power pairs. Warm-start candidate bounds also consult the global cutoff.
Once the global ranking fills, remaining partitions skip their heuristic seed
phase; active seeding checks at dive boundaries and every 64 neighbour checks.
The full traversal still visits/proves the remaining domain.

Partition discovery resolves and validates the original inputs without compiling
a root bound plan that would be discarded. Each worker still compiles its actual
partition. Single-part and serial fallbacks construct the normal bound plan.
Wire-result cloning/conversion runs only when a shared ranking is active.

Five paired runs per configuration using the local roster and four-note chart,
identical result vectors in every run. Request elapsed-time medians in ms:

| Goal / threads | Before | After |
| --- | ---: | ---: |
| Power / 1 | 11.30 | 10.94 |
| Power / 4 | 14.91 | 13.97 |
| Power / 8 | 10.70 | 10.93 |
| Live / 1 | 67.36 | 69.35 |
| Live / 4 | 245.98 | 113.35 |
| Live / 8 | 230.01 | 123.05 |

The final 4-worker run used 5,640 simulations versus 13,200 before; 8 workers
used 7,560 versus 23,760. Small charts still favor serial execution. Remaining
work is per-worker prepared-plan/cache reuse and workload-aware task sizing.
Phase sums in the investigation excluded `prepare`: its current timestamps
include the shared request origin and can therefore include partition queue time.

Validation includes exact serial/oracle rankings and canonical ties, fixed
leaders, K=1/3/100 (including K larger than a fixed-leader domain), candidate/time
budgets, cancellation, full adapter fixtures, and the native ABI tests.


## Native worker preparation reuse (2026-10-06)

Power/Skip workers retain one resolved pool and objective across their claimed
partitions. Repartitioning updates only constraints and initial decks and compiles
fresh domain-specific bounds. This avoids resolving cultivation, scenario, and
chart inputs again. Search frontiers, Top-K, and simulation caches remain fresh.
The original request-wide deadline and candidate budget are unchanged.

Ten alternating paired release runs, identical result vectors throughout:

| Goal / threads | Before ms | Reuse ms |
| --- | ---: | ---: |
| Power / 4 | 14.20 | 13.03 |
| Power / 8 | 10.79 | 9.99 |
| Live / 4 | 105.46 | 105.73 |
| Live / 8 | 128.61 | 142.53 |

Live reuse was excluded from the final path after the regression. The local Live
chart has only four notes. Retaining the previous bound plan during the next
compile also increases transient memory; further investigation is needed before
extending reuse to Live. Crossbeam scheduling remains deferred until preparation
and cache lifetime costs are addressed. No work-stealing dependency was added.

Validation: 168 library, 52 adapter, 12 parallel/oracle integration, and six FFI
tests passed. The final Live-path gate is covered by rerunning the parallel suite.


## Live partition granularity (2026-10-06)

Native ordinary Live branch-and-bound now targets two initial partitions per
worker instead of four. Exhaustive and Power/Skip retain the previous factor.
Domains remain complementary, with all leaders of one member composition kept
together. The shared candidate/deadline budgets and strict global cutoff are
unchanged. This reduces repeated domain-specific bound compilation; it can
increase load imbalance on some workloads.

An experiment retaining Live preparation while explicitly dropping old bounds
before the next compilation produced 101.28/120.43 ms at 4/8 threads versus
100.80/118.00 ms for fresh builds. That experiment was removed. It provides no
evidence that old-bound lifetime alone explains the previous regression.

Partition-factor comparison, alternating before/after binaries, exact result
vectors equal throughout:

| Workload | Threads | Before ms | After ms | Pairs |
| --- | ---: | ---: | ---: | ---: |
| Local chart | 4 | 107.36 | 101.82 | 12 |
| Local chart | 8 | 125.06 | 87.01 | 12 |
| Synthetic 512 notes / six skill events | 4 | 1371.79 | 1406.97 | 3 |
| Synthetic 512 notes / six skill events | 8 | 1402.26 | 1132.03 | 3 |

The local dataset currently contains 342 notes for score 10000100, with no skill
events; the earlier four-note description applies to the older dataset. Synthetic
checks used a temporary dataset with matching full-combo count, the local roster,
and no time limit; both binaries completed. The 4-thread synthetic result shows
that this tuning is workload-dependent. Crossbeam remains deferred pending a
measured need for dynamically redistributing long subtrees.


## Gekisou and certified LUCK parallelism (2026-10-06)

The resolved lottery mode selects the parallel layer. Absent/lottery-free
Gekisou uses existing disjoint-domain search. Certified LUCK uses one proof
coordinator and parallel batches of performance-order simulations. Outer search
workers have a simulation allowance of one, preventing nested oversubscription.
The coordinator participates in simulation work; an allowance N uses at most N
active computation threads per batch. `simulationWorkerLimit` reports this
allowance separately from `workersUsed`, which counts search-domain workers.

Native-only `crossbeam-deque::Injector` distributes individual order jobs.
Threads are scoped to each batch; this is not a persistent OS thread pool.
Each mutable DP cache has one owner during a batch and is retained across
candidate evaluations. The 32 MiB curve-key allowance is divided across the
caches, not multiplied by the thread count. Completed orders are sorted into
canonical input order before aggregation. A cancelled partial batch is discarded.
Initial order scoring polls cancellation between orders; one atomic model run
can finish before cancellation is observed.

Exact-law refinement prefetches at most N orders. Before dispatch it divides
remaining run/frame budgets among jobs; afterward it refunds unused allowances.
Only completed laws are installed, sequentially through the existing interval
frontier checks. Some prefetched laws can become unnecessary after an earlier
law proves the ranking. Their computation is still charged to the request.
Budget-limited convergence can differ from serial execution; exhaustion is never
reported as a proof. Errors and cancellations preserve the existing proof rules.

Three release measurements on the existing 31-team synthetic refinement witness
(cache enabled), identical completed results at every thread count:

| Threads | Median ms |
| --- | ---: |
| 1 | 1582.23 |
| 2 | 1038.48 |
| 4 | 542.20 |
| 8 | 344.13 |

In the first run, serial refinement performed 19,778 replays; eight threads
performed 21,824, with the same 58 installed order refinements. These figures
are synthetic evidence, not a claim about every real chart or three-LUCK speed.

Regression coverage includes deterministic Gekisou versus exhaustive search,
LUCK exact refinement and ranking parity at multiple thread counts, cache on/off,
three separated LUCK ranges, cancellation/deadline behavior, global work ceilings,
and batch ordering/exactly-once execution. The three-range fixture leaves room
for each range's END/DELAY/COMPLETE/FINISH lifecycle. Overlapping unfinished
LUCK ranges remain subject to the underlying model's explicit refusal.

The existing model reads Gekisou member and support skills. A dedicated
three-LUCK skill-match warm-start heuristic is separate future work; this
parallel change does not introduce heuristic card exclusions.
