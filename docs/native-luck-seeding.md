# Native LUCK warm search

Native branch-and-bound searches use upstream warm starts when joint or deck-payoff bounds compile. The native LUCK-first proposals run only when both bounds are unavailable and the search has a certified LUCK frontier. That fallback proposes teams in this order:

1. Maximum feasible coverage of members whose Gekisou mission is LUCK.
2. One fewer LUCK member, filled with a strong generalist.
3. Two fewer LUCK members, filled with strong generalists.
4. Power-search seeds when live bound compilation falls back to exhaustive traversal.

Each LUCK tier keeps up to eight proposals. Formation-compatible LUCK support skills lead the greedy Snap pairing order; card power breaks ties. Member power orders proposal generation and complete team power orders proposals within a tier. These heuristics preserve required cards, fixed leaders, character uniqueness and Snap uniqueness. Full simulation evaluates activation conditions and the actual score.

Lower-priority warm proposals use ten pilot performer permutations: two cyclic sets, with every performer appearing twice at every position. Once five pilot references exist, a proposal below 98% of their top-five average is deferred. High-priority LUCK proposals receive full evaluations. Pilot interval midpoints are scheduling heuristics only; they never enter the result frontier. Deferred teams remain in the exhaustive domain and are eligible for later full evaluation, so this preserves the existing proof contract. This screen currently applies to warm proposals, not every traversal leaf.

`warmStart.pilotOrders` and `warmStart.deferredProposals` expose the screen's work. A returned candidate still aggregates all 120 performer orders. Time-limited results remain unproven.

## Current upstream integration (2026-10-08)

Based on upstream `bca2fdd`, with native baseline `391aa39`. The same frozen dataset and roster below were replayed with eight workers:

| Budget | Baseline LUCK-first | Upstream warm start with native workers |
| --- | ---: | ---: |
| 5 seconds | 23,706,974–24,104,901 | 26,289,435–26,738,438 |
| 15 seconds | 26,289,435–26,738,438 | 26,289,435–26,738,438 |

In the 15-second run, the extra LUCK phase consumed 4.83 seconds. Restricting it to fallback searches increased normal traversal time from 9.43 to 14.27 seconds. Fully evaluated teams were 64 and 66 respectively. These are single-run observations; all four results were time-limited and unproven. The scoring law, certified frontier, native worker count, progress bridge and cancellation contract are unchanged.

## Reproduction

`cargo run --release -p ournotes-search --example narasu-bench -- DATA ROSTER THREADS MILLISECONDS`

The example fixes music 100071 / score 10007103 (鳴らす EXPERT), Great 0%, Just 100%, three LUCK ranges and rank 1 on each range. It rejects missing skill events/ranges. DATA and ROSTER are local user inputs and are not committed.

### Historical results before upstream certified bounds

2026-10-06 local dataset `4c6ce0256d2ba7b63acd6c10562edc2e57245d9cdc62c207b414539c337289a3`, 56 members / 63 Snaps, 920 chart objects / five skill events, 60 seconds:

| Variant | Threads | Fully evaluated teams | Best score interval |
| --- | ---: | ---: | ---: |
| Previous search | 8 | 172 | 2,572,700–2,604,385 |
| Power warm seeds | 8 | 349 | 16,948,764–17,053,510 |
| LUCK-first seeds | 8 | 368 | 23,706,974–24,104,901 |
| LUCK-first + pilot screen | 8 | 361 | 23,706,974–24,104,901 |
| LUCK-first + pilot screen | 15 | 400 | 23,706,974–24,104,901 |

These are single runs on the same frozen inputs, not stable scaling estimates. The pilot screen deferred 14 of 32 proposals after 320 pilot orders. Each run timed out with unproven optimality. At that historical revision, bound compilation refused effect 3004 and uses exhaustive traversal; the new proposal sequence addresses the poor initial candidate coverage. The global default remains ceil(logical CPUs / 2); explicit callers already accept 1 through all available logical CPUs.
