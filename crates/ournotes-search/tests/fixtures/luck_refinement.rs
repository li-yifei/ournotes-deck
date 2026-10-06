//! Reproduce three exhausted 31-team LUCK frontiers using the existing synthetic generator.
//! These inputs declare nominal lotteries, not a distribution of native PRNG seeds.
use super::common::set_column;
use super::{data_document, joint_request, roster_document, synthetic_master};
use ournotes_search::{
    engine,
    search::Completion,
    types::{RecommendationRequest, Strategy},
};
use ournotes_sim::{cards::Roster, data::DeckData};
use serde_json::json;

fn inputs(threshold: i32, k: usize, cache_entries: usize) -> (DeckData, Roster, RecommendationRequest) {
    inputs_with_missions(threshold, k, cache_entries, [2, 3, 1])
}

fn inputs_with_missions(
    threshold: i32,
    k: usize,
    cache_entries: usize,
    missions: [i32; 3],
) -> (DeckData, Roster, RecommendationRequest) {
    let mut synth = synthetic_master(5, 2, 5);
    set_column(&mut synth, "MasterLiveMusic", &mut |row| {
        row["_gekisouMission1"] = json!(missions[0]);
        row["_gekisouMission2"] = json!(missions[1]);
        row["_gekisouMission3"] = json!(missions[2]);
    });
    set_column(&mut synth, "MasterLiveSettings", &mut |row| {
        if matches!(row["_key"].as_str(), Some("gekisou_luck_gauge_max" | "gekisou_luck_gauge_max_rush")) {
            row["_value"] = json!("10");
        }
    });
    if missions == [2, 2, 2] {
        set_column(&mut synth, "MasterLiveMusicScore", &mut |row| row["_fullComboCount"] = json!(60));
    }
    let mut document = data_document(&synth, 5, 2, 5);
    if missions == [2, 2, 2] {
        // Leave time for each range's END/DELAY/COMPLETE/FINISH lifecycle.
        document["charts"][0]["notes"] = json!({"id":(1..=60).collect::<Vec<_>>(),"op":vec![1;60],
            "judgementType":vec![1;60],"timeMs":(1..=60).map(|i|i*1000).collect::<Vec<_>>()});
    }
    document["charts"][0]["fevers"] = if missions == [2, 2, 2] {
        json!({"startMs":[1000, 21000, 41000],"endMs":[3000, 23000, 43000]})
    } else {
        json!({"startMs":[150],"endMs":[400]})
    };
    let data = DeckData::from_json(&document.to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 2, 5).to_string()).unwrap();
    let mut request = joint_request("mission", true, json!({"kind":"scoreAtLeast","threshold":threshold}));
    request.k = k;
    request.strategy = Strategy::Exhaustive;
    request.limits.cache_entries = cache_entries;
    (data, roster, request)
}

#[test]
fn probability_range_settles_the_k1_and_k12_certain_event_witnesses() {
    let powers = [127480, 125551, 125292, 125100, 124371, 124190, 124054, 123871, 123296, 122261, 122183, 122125];
    for (k, cache) in [(1, 0), (12, 64)] {
        let (data, roster, request) = inputs(545_749, k, cache);
        let result = engine::recommend(&data, &roster, &request).unwrap();
        assert_eq!(result.completion, Completion::Complete);
        assert_eq!(result.telemetry.leaves.visited, 31);
        assert_eq!(result.results.iter().map(|deck| deck.power).collect::<Vec<_>>(), powers[..k]);
        assert_eq!(
            result.telemetry.lottery_refinement.attempted_orders, 0,
            "the closed probability range already proves these power ties"
        );
        for deck in &result.results {
            assert_eq!(deck.rank_certified, Some(true));
            let payoff = deck.expected_payoff.as_ref().expect("the threshold is reached on every path");
            assert_eq!(payoff.numerator, payoff.denominator);
            let interval = deck.payoff_interval.as_ref().unwrap();
            assert!(interval.lower_f64() >= 0.0);
            assert_eq!(interval.upper_f64(), 1.0);
        }
    }
}

#[test]
fn exact_nominal_refinement_settles_the_remaining_threshold_witness_without_a_cache() {
    let (data, roster, request) = inputs(610_000, 1, 0);
    let result = engine::recommend(&data, &roster, &request).unwrap();
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.telemetry.leaves.visited, 31);
    assert_eq!(result.results.len(), 1);
    let winner = &result.results[0];
    assert_eq!(winner.members, [1, 2, 3, 4, 5]);
    assert_eq!(winner.snaps, [Some(1), Some(2), None, None, None]);
    assert_eq!(winner.power, 127_480);
    assert_eq!(winner.rank_certified, Some(true));
    let counters = &result.telemetry.lottery_refinement;
    assert!(counters.installed_orders > 0, "moment bounds alone cannot settle this witness");
    assert_eq!(counters.completed_orders, counters.installed_orders);
    assert_eq!(counters.declined_orders, 0);
    assert!(counters.terminal_paths > counters.completed_orders);
    let probability = winner.payoff_interval.as_ref().unwrap();
    assert!(
        probability.lower_f64() > 0.388_928_092_021_209_54,
        "the provider must add information beyond the original first-moment lower bound"
    );
    assert!(probability.upper_f64() <= 1.0);
    // Complete certifies the ranking. Numeric expectations remain absent until all 120 relevant order
    // expectations are exact; no midpoint is filled in just because the winner has been identified.
    if winner.expected_payoff.is_none() {
        assert!(probability.lower_f64() < probability.upper_f64());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_luck_orders_and_refinement_match_serial_proof() {
    use ournotes_search::parallel::{self, Cancellation};
    for (threshold, k, cache) in [(545_749, 12, 64), (610_000, 1, 0)] {
        let (data, roster, mut request) = inputs(threshold, k, cache);
        request.limits.time_limit_ms = None;
        let serial = engine::recommend(&data, &roster, &request).unwrap();
        for workers in [2, 4].map(|n| n.min(parallel::max_workers())) {
            let out = parallel::recommend(&data, &roster, &request, workers, Cancellation::default()).unwrap();
            assert_eq!(out.completion, serial.completion);
            assert_eq!(out.results, serial.results);
            assert_eq!(out.optimality, serial.optimality);
            assert_eq!(out.telemetry.parallel.unwrap().simulation_worker_limit, workers);
            assert!(out.telemetry.lottery_refinement.replay_runs <= 240_000);
            assert!(out.telemetry.lottery_refinement.frames <= 8_000_000);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_deterministic_gekisou_partitions_match_exhaustive() {
    use ournotes_search::parallel::{self, Cancellation};
    let synth = synthetic_master(6, 1, 6);
    let mut document = data_document(&synth, 6, 1, 6);
    document["charts"][0]["fevers"] = json!({"startMs":[150],"endMs":[400]});
    let data = DeckData::from_json(&document.to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 1, 6).to_string()).unwrap();
    let mut request = joint_request("mission", true, json!({"kind":"score"}));
    request.limits.time_limit_ms = None;
    request.strategy = Strategy::Exhaustive;
    let serial = engine::recommend(&data, &roster, &request).unwrap();
    for strategy in [Strategy::Exhaustive, Strategy::BranchAndBound] {
        request.strategy = strategy;
        let n = 3.min(parallel::max_workers());
        let result = parallel::recommend(&data, &roster, &request, n, Cancellation::default()).unwrap();
        assert_eq!(result.completion, Completion::Complete);
        assert_eq!(result.results, serial.results);
        let t = result.telemetry.parallel.unwrap();
        assert!(t.fallback.is_none());
        if n > 1 {
            assert!(t.tasks > 1);
        }
        assert_eq!(t.simulation_worker_limit, 1, "no nested parallelism");
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_luck_deadline_and_precancel_never_claim_proven() {
    use ournotes_search::{
        parallel::{self, Cancellation},
        types::Optimality,
    };
    let (data, roster, mut request) = inputs(610_000, 1, 0);
    for zero_deadline in [true, false] {
        request.limits.time_limit_ms = if zero_deadline { Some(0) } else { None };
        let cancel = Cancellation::default();
        if !zero_deadline {
            cancel.cancel();
        }
        let result = parallel::recommend(&data, &roster, &request, 2.min(parallel::max_workers()), cancel).unwrap();
        assert_ne!(result.optimality, Optimality::Proven);
        assert!(result.results.is_empty());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "manual native LUCK timing"]
fn native_luck_throughput() {
    use ournotes_search::parallel::{self, Cancellation};
    let (data, roster, mut request) = inputs(610_000, 1, 64);
    request.limits.time_limit_ms = None;
    let mut expected = None;
    for n in [1, 2, 4, 8].map(|n| n.min(parallel::max_workers())) {
        let result = parallel::recommend(&data, &roster, &request, n, Cancellation::default()).unwrap();
        assert_eq!(result.completion, Completion::Complete);
        if let Some(expected) = &expected {
            assert_eq!(&result.results, expected);
        } else {
            expected = Some(result.results.clone());
        }
        eprintln!(
            "LUCK threads={n} ms={:.2} runs={} installed={}",
            result.elapsed_ms,
            result.telemetry.lottery_refinement.replay_runs,
            result.telemetry.lottery_refinement.installed_orders
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn three_luck_ranges_parallel_match_serial() {
    use ournotes_search::parallel::{self, Cancellation};
    for cache in [0, 64] {
        let (data, roster, mut request) = inputs_with_missions(1, 3, cache, [2, 2, 2]);
        request.limits.time_limit_ms = None;
        let expected = engine::recommend(&data, &roster, &request).unwrap();
        let actual =
            parallel::recommend(&data, &roster, &request, 3.min(parallel::max_workers()), Cancellation::default())
                .unwrap();
        assert_eq!(actual.completion, expected.completion);
        assert_eq!(actual.results, expected.results);
    }
}
