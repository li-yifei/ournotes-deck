# Native validation

Song statistics, per-note play calculation and deck search share one Rust scoring model. The model is checked in two ways: frame-by-frame comparison with the game client's native implementation on identical inputs, and comparison of the same Rust API built natively and for WebAssembly. This document describes the comparison method and how to reproduce it. The repository ships no client, master data, chart assets or native captures; reproduction uses your own copy of the client.

## Frame-by-frame comparison

The client's play logic runs offline in a CPU emulator: chart conversion, the live executor, skill state and triggers, member routing, effect-pool scheduling, skill conditions, randomness and scoring all execute the client's own code. The host supplies stand-ins for the file system, libc and engine services. Stand-ins only construct objects and return display text; scoring arithmetic, skill conditions, trigger outcomes and judgement results all come from the client's code.

Native auto-input receives controlled grades and timing parameters, and the harness records the actual original and converted results and their order. The Rust model consumes that native result stream at the same frame times and derives chart skill events independently. Both sides record per-frame state, which is compared under a field contract:

- Integer fields must match exactly; declared float32 fields are compared by their bit patterns.
- Frames are never shifted, no tolerance is applied, and no differing field is dropped. A field the contract requires but a trace lacks makes the result `incomplete`, which is not a pass.
- The frame comparison also covers effect-pool identities, inactive pools, start and finish times, and the score observations before and after ranking, so it checks update order that equal final scores alone cannot establish.

Comparison counts include repeated frames, empty states and pool identities. They count checks, not independent samples.

## Reproducing a comparison

1. Prepare a copy of the client and its master data. Record their identities with [`make_source_manifest.py`](../tools/native-validation/README.md) `--resource ROLE=PATH`; it records hashes only, never content or local paths.
2. Run the client's play logic offline and record a native frame trace in the comparator format: a top-level `chartId` and ordered `frames`, each frame holding at least `frame`, `timeMs` and the fields the contract declares.
3. Drive `live::full::LiveModel` with the same results (`frame_timed` advances one frame) and export a trace in the same format from its public state (`frame_score`, `score`, `current_life`, `current_combo`, `factor_state`, `gekisou_ranges` and so on). For score, life, combo and Gekisou ranges alone, the `trace: true` output of an `ournotes.replay/1` request is sufficient.
4. Write a field contract: integer fields, float32 bit fields, array lengths and the fields left out. [`ordinary-contract.json`](../tools/native-validation/ordinary-contract.json) is the contract of the ordinary-skill matrix; other skill configurations state their own effect-pool count.
5. Run the comparator:

   ```text
   python tools/native-validation/compare_native_frames.py --native NATIVE.json --rust RUST.json --contract CONTRACT.json --output comparison.json
   ```

   Exit code 0 means every declared field is equal, 1 means differences, and 2 means invalid, incomplete or absent input. The output keeps the first and all differences and the hashes of the inputs and the contract.

## Unit-level reference vectors

Judgement, note scheduling, assist judgement, the updaters, judgement-window limits, member-order roots, power and furniture bonuses each have reference vectors recorded from the client. The tests that replay them build with the `native-fixtures` feature, and the test code defines each file's JSON layout:

```text
OURNOTES_FIXTURES=/path/to/fixtures cargo test --release --features native-fixtures
```

A missing variable or file fails the test instead of skipping it. Without the feature, the tests use synthetic data only.

## Native and WebAssembly

The same `ournotes.replay/1` request is run by the native `ournotes_deck::replay::ReplaySession::run_json` and by `ReplaySession.run` in [`wasm/replay`](../wasm/replay/src/lib.rs). Score, life, frame-max combo, judgement totals and Gekisou ranges must be equal; repeated runs agree, and both sides reject a request with an unknown note. Normal lives and SoloGekisou are compared this way.

## Calculation contract of the shared model

Per-note replay accepts completed judgements or full `RawResult` values, with explicit frame times, original result order, skills, random seed and ranking policy. Accuracy percentages alone do not determine those inputs. The page calculates a declared play by running the shared Rust model. Summary outputs provide nominal expectation intervals and ordinary skill weights. The full model handles condition 4011 and executes probability skills and interactions in their draw order; the summary law uses independent nominal probabilities.

Score-operation registration is separate from life, combo and callback processing. Inactive conditional-pool execution and finish times are -1. Just missions switch Just enable state before raw grading and restore the client setting at the end. Audio/skill duration and score-table duration have independent sources: the latter is the last-note position time plus 1,000 ms. Aptitude computes both the best-grade and Perfect endpoints and checks full nominal expectations against interval predictions.

Solo ranking queries the calculator at range times; Network ranking uses controller frame snapshots. `frame_score()` is the pre-ranking display cache and `score()` is the post-ranking calculator value; they are distinct observation stages. Fixed-rank statistics use `new_gekisou_ranked` with Solo score-query semantics; Network simulation uses `new_gekisou_external` snapshots. Fixed-rank estimates do not simulate opponents or server confirmations.

## What a comparison establishes

A comparison establishes agreement on the declared resources, inputs, fields and phases. With completed results as input, it checks the downstream arithmetic and state transitions; touch classification is compared with separate input traces. Fields a contract leaves out take no part in the conclusion.
