//! Explicitly synthetic UTF-8 transport corpus input; no game assets or native truth.
#[path = "../../ournotes-sim/tests/common/mod.rs"]
mod common;
#[path = "fixtures/damage_reduction.rs"]
mod damage_reduction;
#[path = "fixtures/release_ranges.rs"]
mod release_ranges;
use common::{Rng, Synth, extend_table, replace_table, set_column, synth_snaps};
use ournotes_search::types::{DeckInput, Limits, Metric, SimulationInput, Strategy};
use ournotes_sim::{cards::Roster, data::DeckData};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
const FIXTURE_SEED: u64 = 20_261_001;
const SCORE_ID: i64 = 1004;
const EVENT_ID: i64 = 7;

#[path = "fixtures/carrier_cache.rs"]
#[cfg(feature = "search-diagnostics")]
mod carrier_cache;
#[path = "fixtures/combo_integer.rs"]
mod combo_integer;
#[path = "fixtures/conversion_partitions.rs"]
mod conversion_partitions;
#[path = "fixtures/cp_priorities.rs"]
mod cp_priorities;
#[path = "fixtures/effect_identity.rs"]
mod effect_identity;
#[path = "fixtures/luck_refinement.rs"]
mod luck_refinement;
#[path = "fixtures/network_snapshots.rs"]
mod network_snapshots;
#[path = "fixtures/numeric_domain.rs"]
mod numeric_domain;
#[path = "fixtures/proof_telemetry.rs"]
mod proof_telemetry;
#[path = "fixtures/scenario_completion.rs"]
mod scenario_completion;
#[path = "fixtures/score_paths.rs"]
mod score_paths;
#[path = "fixtures/sustained_combo.rs"]
mod sustained_combo;

fn joint_request(mode: &str, gekisou: bool, metric: Value) -> ournotes_search::types::RecommendationRequest {
    serde_json::from_value(joint_request_json(mode, gekisou, metric)).unwrap()
}

#[path = "fixtures/conversion_regimes.rs"]
mod conversion_regimes;

fn joint_request_json(mode: &str, gekisou: bool, metric: Value) -> Value {
    json!({"format":"ournotes-deck.search-request/1",
        "execution":{"kind":"live","scoreId":SCORE_ID,"gekisou":gekisou,"play":{"kind":"theoreticalBest"}},
        "scenario":{"kind":mode,"musicId":10},"context":context_document(false,false,false),
        "metric":metric,"k":12,"constraints":{"leader":3},
        "strategy":{"kind":"branchAndBound"},"limits":{"timeLimitMs":null,"maxCandidates":null,"cacheEntries":0}
    })
}

#[test]
fn production_luck_intervals_keep_five_leaders_and_prove_equal_program_ties() {
    use ournotes_search::{engine, search::Completion};
    let mut synth = synthetic_master(5, 0, 5);
    set_column(&mut synth, "MasterLiveMusic", &mut |row| {
        row["_gekisouMission1"] = json!(2);
        row["_gekisouMission2"] = json!(3);
        row["_gekisouMission3"] = json!(1);
    });
    let mut document = data_document(&synth, 5, 0, 5);
    document["charts"][0]["fevers"] = json!({"startMs":[150],"endMs":[400]});
    let mut data = DeckData::from_json(&document.to_string()).unwrap();
    for effect in &mut data.master.leader_skill_effects {
        effect.effect_value = 0;
    }
    let roster = Roster::from_json(&roster_document(5, 0, 5).to_string()).unwrap();
    let mut request = joint_request("mission", true, json!({"kind":"score"}));
    request.constraints.leader = None;
    request.constraints.no_snaps = true;
    request.strategy = Strategy::Exhaustive;
    request.k = 5;
    request.limits.cache_entries = 64;
    let result = engine::recommend(&data, &roster, &request).unwrap();
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.results.len(), 5);
    assert_eq!(result.telemetry.leaves.visited, 5, "nonleader arrangements are the same team");
    assert_eq!(result.telemetry.leaves.simulations, 120, "same full random program/power reuses all 120 laws");
    assert_eq!(result.results.iter().map(|r| r.members[2]).collect::<BTreeSet<_>>().len(), 5);
    for deck in &result.results {
        assert_eq!(deck.rank_certified, Some(true));
        assert!(deck.score_interval.is_some() && deck.payoff_interval.is_some());
        assert!(deck.order_outcomes.is_empty());
    }
    assert!(result.results.windows(2).all(|v| v[0].members < v[1].members));
    // A song having LUCK mission metadata does not activate Gekisou for a Free live.
    let mut free = joint_request("free", false, json!({"kind":"score"}));
    free.constraints.leader = None;
    free.constraints.no_snaps = true;
    free.k = 5;
    let free_result = engine::recommend(&data, &roster, &free).unwrap();
    assert_eq!(free_result.completion, Completion::Complete);
    assert_eq!(free_result.results.len(), 5);
    assert!(free_result.results.iter().all(|r| r.score_interval.is_none() && r.expected_payoff.is_some()));
}

#[test]
fn production_probability_chain_without_luck_mission_plays_lottery_free() {
    use ournotes_search::{engine, search::Completion};
    use ournotes_sim::live::full::{GekisouSetup, LiveModel, LiveParams, Performer};
    let mut synth = synthetic_master(5, 0, 5);
    extend_table(
        &mut synth,
        "MasterSkillCondition",
        vec![json!({"_id":990001,"_conditionType":4011,
        "_conditionValues":[50],"_isPositive":true,"_conditionTargetIDs":[]})],
    );
    extend_table(
        &mut synth,
        "MasterSkillConditionSet",
        vec![json!({"_id":990001,"_group":990001,"_conditionIds":[990001]})],
    );
    extend_table(
        &mut synth,
        "MasterSkillEffectSetting",
        vec![json!({"_id":990001,"_skillEffectType":11003,"_phase":1})],
    );
    set_column(&mut synth, "MasterGekisouSkillEffect", &mut |row| {
        row["_skillEffectType"] = json!(11003);
        row["_skillTriggerConditionGroup"] = json!(990001);
        row["_activationTimeSecond"] = json!(0);
        row["_effectValue"] = json!(1);
    });
    let mut document = data_document(&synth, 5, 0, 5);
    document["charts"][0]["fevers"] = json!({"startMs":[150],"endMs":[400]});
    let mut data = DeckData::from_json(&document.to_string()).unwrap();
    for effect in &mut data.master.leader_skill_effects {
        effect.effect_value = 0;
    }
    // Independent native playback proves the 4011 checker really consumes random draws without a LUCK mission.
    let performer = Performer { gekisou_skill: Some((1, 1)), gekisou_mission_type: 1, ..Default::default() };
    let params = LiveParams {
        skill_target_music_type: 0,
        total_power: 100_000,
        music_level: 24,
        converted_note_count: 1,
        music_length_ms: 1000,
        score_music_length_ms: None,
        assist_factor: 1.0,
    };
    let setup = GekisouSetup { fevers: vec![(150, 400)], missions: vec![1, 3, 1] };
    let mut native = LiveModel::new_gekisou(&data.master, &[performer], &[], &[], params, &setup).unwrap();
    for frame in 0..70 {
        native.frame_timed(frame * 20, &[], 0.02).unwrap();
    }
    assert!(native.draws() > 0);
    let roster = Roster::from_json(&roster_document(5, 0, 5).to_string()).unwrap();
    let mut request = joint_request("mission", true, json!({"kind":"score"}));
    request.constraints.leader = None;
    request.constraints.no_snaps = true;
    request.k = 5;
    request.limits.cache_entries = 64;
    let result = engine::recommend(&data, &roster, &request).unwrap();
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.results.len(), 5);
    assert_eq!(result.probability_law["lottery"], "noLuckRange");
    // Without a LUCK range the draws never reach the score: every order has one exact score.
    assert!(result.results.iter().all(|r| r.score_interval.is_none() && r.order_outcomes.len() == 120));
    let mut exhaustive = request.clone();
    exhaustive.strategy = Strategy::Exhaustive;
    let reference = engine::recommend(&data, &roster, &exhaustive).unwrap();
    assert_eq!(reference.completion, Completion::Complete);
    let key = |r: &ournotes_search::types::RecommendedDeck| (r.members, r.snaps, r.expected_payoff.clone());
    assert_eq!(
        result.results.iter().map(key).collect::<Vec<_>>(),
        reference.results.iter().map(key).collect::<Vec<_>>()
    );
}

#[test]
fn network_battle_arena_bounds_match_exhaustive_with_room_payoffs() {
    use ournotes_search::{engine, search::Completion};
    use ournotes_sim::scenario::MultiplayerScorePolicy;
    let synth = synthetic_master(6, 1, 6);
    let data = DeckData::from_json(&data_document(&synth, 6, 1, 6).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 1, 6).to_string()).unwrap();
    for (mode, id) in [("battle", 10), ("arena", 80)] {
        for room in [
            MultiplayerScorePolicy::SameScore { players: 5 },
            MultiplayerScorePolicy::FixedOthersAverage { players: 3, score: 150_000 },
        ] {
            for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
                let mut wire = joint_request_json(mode, true, metric.clone());
                wire["scenario"]["musicId"] = json!(id);
                wire["networkConfirmations"] = json!(
                    (0..3).map(|range| json!({"frame":0,"range":range,"rank":1,"percent":10})).collect::<Vec<_>>()
                );
                wire["context"]["eventPayoff"]["multiplayerScorePolicy"] = serde_json::to_value(&room).unwrap();
                let mut request: ournotes_search::types::RecommendationRequest = serde_json::from_value(wire).unwrap();
                request.k = 5;
                let bounded = engine::recommend(&data, &roster, &request).unwrap();
                assert_eq!(bounded.completion, Completion::Complete);
                assert!(
                    bounded.telemetry.environment.bounds.compiled,
                    "{mode} {metric}: {:?}",
                    bounded.telemetry.environment.bounds.fallback
                );
                // Nonnegative rank bonuses and a declared room: event points step with the local score.
                let ranked = bounded.telemetry.environment.bounds.deck_payoff.as_ref();
                assert_eq!(
                    ranked.is_some_and(|setup| setup.score_cap.is_some()),
                    metric["kind"] == "clientEventPoints",
                    "{mode} {metric} {room:?}: {ranked:?}"
                );
                request.strategy = Strategy::Exhaustive;
                let oracle = engine::recommend(&data, &roster, &request).unwrap();
                assert_eq!(oracle.completion, Completion::Complete);
                assert_eq!(bounded.results, oracle.results, "{mode} {metric} {room:?}");
                #[cfg(feature = "search-diagnostics")]
                {
                    use ournotes_search::{handler, search::diagnostics};
                    request.strategy = Strategy::BranchAndBound;
                    let built = handler::build_card_pool(&data, &roster, &request).unwrap();
                    let mut scratch = diagnostics::PrefixAuditScratch::default();
                    for deck in &oracle.results {
                        let exact = deck
                            .expected_payoff
                            .as_ref()
                            .expect("exact deterministic fixture")
                            .numerator
                            .parse::<i128>()
                            .unwrap();
                        for depth in 1..=5 {
                            let (upper, _) =
                                diagnostics::prefix_upper(&built, deck.members, deck.snaps, depth, &mut scratch)
                                    .unwrap()
                                    .unwrap();
                            assert!(upper >= exact, "{mode} {metric} depth {depth}: {upper} < {exact}");
                        }
                        let audit = diagnostics::audit_order_caps(&built, deck.members, deck.snaps).unwrap();
                        assert_eq!(audit["violations"], 0, "{mode} {metric}: {}", audit["first"]);
                    }
                }
            }
        }
    }
}

#[test]
fn default_exact_search_has_no_hidden_candidate_prefix_limit() {
    use ournotes_search::{
        engine,
        search::Completion,
        types::{RecommendationRequest, Strategy},
    };
    let synth = synthetic_master(5, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 5, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 2, 5).to_string()).unwrap();
    let request: RecommendationRequest = serde_json::from_value(json!({
        "format":"ournotes-deck.search-request/1",
        "execution":{"kind":"live","scoreId":SCORE_ID,"gekisou":false,"play":{"kind":"theoreticalBest"}},
        "scenario":{"kind":"free","musicId":10},"metric":{"kind":"cappedScore","threshold":2_000_000_000},
        "k":1,"limits":{"timeLimitMs":null,"cacheEntries":0}
    }))
    .unwrap();
    let outcome = engine::recommend(&data, &roster, &request).unwrap();
    assert!(matches!(outcome.strategy, Strategy::BranchAndBound));
    assert_eq!(outcome.completion, Completion::Complete);
    assert_eq!(outcome.telemetry.environment.max_candidates, None);
    // The proof covers every team (5 leaders times 31 Snap pairings) even where the bounds skip one.
    let mut exhaustive = request.clone();
    exhaustive.strategy = Strategy::Exhaustive;
    let oracle = engine::recommend(&data, &roster, &exhaustive).unwrap();
    assert_eq!(oracle.telemetry.leaves.visited, 155);
    assert_eq!(outcome.results, oracle.results);
}

#[test]
fn joint_search_matches_exhaustive_full_topk_and_preserves_resource_constraints() {
    use ournotes_search::{engine, search::Completion, types::Strategy};
    let synth = synthetic_master(6, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let mut pruned = 0;
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            for constrained in [false, true] {
                let mut req = joint_request(mode, gekisou, metric.clone());
                if constrained {
                    req.constraints.include_members = vec![6];
                    req.constraints.exclude_snaps = vec![1];
                }
                let bounded = engine::recommend(&data, &roster, &req).unwrap();
                assert_eq!(bounded.completion, Completion::Complete);
                assert!(
                    bounded.telemetry.environment.bounds.fallback.is_none(),
                    "{} {:?}: {:?}",
                    mode,
                    metric,
                    bounded.telemetry.environment.bounds.fallback
                );
                let tel = &bounded.telemetry;
                pruned += tel.joint.branch.pruned.iter().sum::<u64>()
                    + tel.composition.composition.pruned
                    + tel.composition.modules.values().map(|c| c.pruned).sum::<u64>()
                    + tel.joint.modules.values().map(|c| c.pruned).sum::<u64>()
                    + tel.composition.team.pruned;
                req.strategy = Strategy::Exhaustive;
                let baseline = engine::recommend(&data, &roster, &req).unwrap();
                assert_eq!(bounded.results, baseline.results, "{mode} {metric:?} constrained={constrained}");
                assert!(
                    bounded.telemetry.leaves.simulations <= baseline.telemetry.leaves.simulations,
                    "{mode} {metric:?} constrained={constrained}: bounded={} oracle={}",
                    bounded.telemetry.leaves.simulations,
                    baseline.telemetry.leaves.simulations
                );
            }
        }
    }
    assert!(pruned > 0, "joint search must actually prune branches");
}

/// Every stop reports a true upper bound of what it leaves: each deck of the full Top-K that the stopped search
/// did not keep pays at most `max(upperBound, stopped K-th)`. Candidate limits stop the search deterministically at
/// every depth of the joint and composition traversals.
#[test]
fn proof_upper_bound_covers_every_deck_a_stop_leaves() {
    use ournotes_search::{
        engine,
        search::{Completion, telemetry::Traversal},
    };
    let synth = synthetic_master(6, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let value = |d: &ournotes_search::types::RecommendedDeck| {
        d.expected_payoff.as_ref().expect("exact deterministic fixture").numerator.parse::<i128>().unwrap()
    };
    let mut traversals = BTreeSet::new();
    let mut stops = 0;
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            for constrained in [false, true] {
                let mut req = joint_request(mode, gekisou, metric.clone());
                if constrained {
                    req.constraints.include_members = vec![6];
                }
                let full = engine::recommend(&data, &roster, &req).unwrap();
                assert_eq!(full.completion, Completion::Complete);
                let proof = &full.telemetry.proof;
                assert!(proof.complete && proof.fraction == Some(1.0) && proof.upper_bound.is_none());
                assert_eq!(
                    proof.best.as_deref(),
                    Some(
                        full.results[0]
                            .expected_payoff
                            .as_ref()
                            .expect("exact deterministic fixture")
                            .numerator
                            .as_str()
                    )
                );
                traversals.insert(format!("{:?}", full.telemetry.environment.traversal));
                let visited = full.telemetry.leaves.visited;
                let mut limits: Vec<u64> = (1..=8).chain((1..=24).map(|i| visited * i / 25)).collect();
                limits.sort_unstable();
                limits.dedup();
                for limit in limits.into_iter().filter(|&n| n > 0 && n < visited) {
                    req.limits.max_candidates = Some(limit);
                    let stopped = engine::recommend(&data, &roster, &req).unwrap();
                    assert_eq!(stopped.completion, Completion::TimedOut, "{mode} {metric} limit {limit}");
                    let proof = &stopped.telemetry.proof;
                    assert!(!proof.complete);
                    assert!(matches!(
                        stopped.telemetry.environment.traversal,
                        Traversal::Joint | Traversal::Composition
                    ));
                    let fraction = proof.fraction.expect("tracked traversal");
                    assert!((0.0..1.0).contains(&fraction), "fraction {fraction}");
                    let upper = proof.upper_bound.as_ref().map(|v| v.parse::<i128>().unwrap());
                    let kth = (stopped.results.len() == req.k).then(|| value(stopped.results.last().unwrap()));
                    for deck in &full.results {
                        if stopped.results.iter().any(|r| r.members == deck.members && r.snaps == deck.snaps) {
                            continue;
                        }
                        let cover = upper.max(kth);
                        assert!(
                            cover.is_some_and(|c| value(deck) <= c),
                            "{mode} {metric} constrained={constrained} limit {limit}: {} above {upper:?}/{kth:?}",
                            value(deck)
                        );
                    }
                    if let Some(gap) = proof.best_gap {
                        assert!(gap >= 0.0);
                    }
                    stops += 1;
                }
            }
        }
    }
    assert_eq!(traversals.len(), 2, "{traversals:?}");
    assert!(stops > 50, "{stops}");
}

/// Every node bound of a team's prefixes (position-mean gains), every bound module and every per-order cap covers the
/// exact value of each team of the exhaustive ranking.
#[cfg(feature = "search-diagnostics")]
#[test]
fn position_mean_bounds_cover_the_exact_uniform_value() {
    use ournotes_search::{engine, handler, search::diagnostics};
    let synth = synthetic_master(6, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let (mut checks, mut modules) = (0, 0);
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            let mut req = joint_request(mode, gekisou, metric.clone());
            req.k = 64;
            req.strategy = Strategy::Exhaustive;
            let oracle = engine::recommend(&data, &roster, &req).unwrap();
            req.strategy = Strategy::BranchAndBound;
            let built = handler::build_card_pool(&data, &roster, &req).unwrap();
            let mut scratch = diagnostics::PrefixAuditScratch::default();
            for row in &oracle.results {
                let numerator = row
                    .expected_payoff
                    .as_ref()
                    .expect("exact deterministic fixture")
                    .numerator
                    .parse::<i128>()
                    .unwrap();
                for depth in 1..=5 {
                    let bound = diagnostics::prefix_upper(&built, row.members, row.snaps, depth, &mut scratch).unwrap();
                    if let Some((cap, power)) = bound {
                        assert!(cap >= numerator && power >= i64::from(row.power), "{mode} {metric} depth {depth}");
                        checks += 1;
                    }
                    for module in diagnostics::module_prefix_uppers(&built, row.members, row.snaps, depth).unwrap() {
                        if let Some(cap) = module.upper {
                            assert!(cap >= numerator, "{mode} {metric} {} depth {depth}", module.name);
                            modules += 1;
                        }
                    }
                }
                let orders = diagnostics::audit_order_caps(&built, row.members, row.snaps).unwrap();
                assert_eq!(orders["violations"], 0, "{mode} {metric}: {}", orders["first"]);
            }
        }
    }
    assert!(checks > 0 && modules > 0, "{checks} {modules}");
}

/// A score and life target under a stream with misses, with Snaps that recover life at their own skill event, guard,
/// or neither: every per-order cap covers the exact payoff of each team, some orders are capped at zero by their
/// final life alone, and the search returns the full canonical Top-K of the exhaustive ranking.
#[cfg(feature = "search-diagnostics")]
#[test]
fn final_life_caps_cover_every_team_and_keep_the_full_topk() {
    use ournotes_search::{
        engine, handler,
        search::{Completion, diagnostics},
        types::{Execution, PlayPolicy},
    };
    let mut synth = synthetic_master(6, 3, 5);
    // Support skill 5 recovers life at its own skill event, 8 guards, 3 and 6 add score and conversions (Snap 3 is
    // left out of the domain).
    set_column(&mut synth, "MasterSupportCard", &mut |r| {
        let (a, b) = match r["_id"].as_i64().unwrap() {
            1 => (5, 0),
            2 => (8, 3),
            _ => (3, 6),
        };
        r["_supportSkillId01"] = json!(a);
        r["_supportSkillId02"] = json!(b);
    });
    let data = DeckData::from_json(&data_document(&synth, 6, 3, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 3, 5).to_string()).unwrap();
    let mut stream = ournotes_sim::live::model::JudgementStream::theoretical_best(&data.chart(SCORE_ID).unwrap());
    // Eight of the twelve notes are missed: 8 * 140 damage empties the base life of 1000 without a recovery.
    for (i, row) in stream.judged.iter_mut().enumerate() {
        if i % 3 != 2 {
            row[2] = 1;
        }
    }
    let (mut zero_orders, mut passing, mut zero_prefixes) = (0, 0, 0);
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for least in [1, 150, 400, 900] {
            let metric = json!({"kind":"scoreAndLifeAtLeast","threshold":1,"minFinalLife":least});
            let mut request = joint_request(mode, gekisou, metric);
            request.execution =
                Execution::Live { score_id: SCORE_ID, gekisou, play: PlayPolicy::Stream { stream: stream.clone() } };
            // Every team of the domain: two member sets under the fixed leader, 31 pairings of Snaps 1 and 2 each.
            request.constraints.exclude_snaps = vec![3];
            request.k = 100;
            request.strategy = Strategy::Exhaustive;
            let oracle = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(oracle.completion, Completion::Complete);
            assert_eq!(oracle.results.len(), 62);
            request.strategy = Strategy::BranchAndBound;
            let built = handler::build_card_pool(&data, &roster, &request).unwrap();
            for row in &oracle.results {
                let audit = diagnostics::audit_order_caps(&built, row.members, row.snaps).unwrap();
                assert_eq!(audit["violations"], 0, "{mode} {least}: {}", audit["first"]);
                zero_orders += audit["lifeCapped"].as_u64().unwrap();
                passing += row.order_outcomes.iter().filter(|&&(_, _, payoff)| payoff > 0).count();
                // The composition bounds of the team's prefixes, members first and then Snaps.
                let numerator = row.expected_payoff.as_ref().unwrap().numerator.parse::<i128>().unwrap();
                for team in [false, true] {
                    for depth in usize::from(!team)..=5 {
                        let bound = diagnostics::split_prefix_upper(&built, row.members, row.snaps, depth, team);
                        let (cap, power) = bound.unwrap().unwrap();
                        assert!(cap >= numerator && power >= i64::from(row.power), "{mode} {least} {team} {depth}");
                        zero_prefixes += usize::from(cap == 0);
                    }
                }
            }
            for k in [1, 5, 64] {
                request.k = k;
                let bounded = engine::recommend(&data, &roster, &request).unwrap();
                assert_eq!(bounded.completion, Completion::Complete, "{mode} {least} k={k}");
                assert!(bounded.telemetry.environment.bounds.fallback.is_none(), "{mode} {least} k={k}");
                let expected: Vec<_> = oracle.results.iter().take(k).cloned().collect();
                assert_eq!(bounded.results, expected, "{mode} {least} k={k}");
            }
        }
    }
    assert!(zero_orders > 0 && passing > 0 && zero_prefixes > 0, "{zero_orders} {passing} {zero_prefixes}");
}

#[test]
fn warm_start_and_visit_order_leave_the_canonical_topk_unchanged() {
    use ournotes_search::{
        engine,
        search::{Completion, ablate, set_bound_ablation},
    };
    let synth = synthetic_master(6, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let mut seeded = 0;
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            for (k, constrained) in [(12, false), (12, true), (1, false), (3, true)] {
                let mut req = joint_request(mode, gekisou, metric.clone());
                req.k = k;
                if constrained {
                    req.constraints.include_members = vec![6];
                    req.constraints.exclude_snaps = vec![1];
                }
                req.strategy = Strategy::Exhaustive;
                let oracle = engine::recommend(&data, &roster, &req).unwrap();
                req.strategy = Strategy::BranchAndBound;
                for bits in
                    [0, ablate::NO_WARM_START, ablate::STATIC_ORDER, ablate::NO_WARM_START | ablate::STATIC_ORDER]
                {
                    set_bound_ablation(bits);
                    let out = engine::recommend(&data, &roster, &req).unwrap();
                    set_bound_ablation(0);
                    let case = format!("{mode} {metric:?} k={k} constrained={constrained} ablation={bits}");
                    assert_eq!(out.completion, Completion::Complete, "{case}");
                    assert_eq!(out.results, oracle.results, "{case}");
                    assert!(out.telemetry.leaves.simulations <= oracle.telemetry.leaves.simulations, "{case}");
                    let warm = &out.telemetry.incumbents.warm_start;
                    if bits & ablate::NO_WARM_START == 0 {
                        seeded += warm.evaluations;
                    } else {
                        assert_eq!(warm.evaluations + warm.polish_evaluations, 0, "{case}");
                    }
                }
                // Supplied initial decks, the K-th first and in a permuted layout, only fill the Top-K earlier.
                let mut supplied = req.clone();
                supplied.initial_decks = oracle
                    .results
                    .iter()
                    .rev()
                    .map(|d| DeckInput { members: permuted(d.members), snaps: permuted(d.snaps) })
                    .collect();
                let out = engine::recommend(&data, &roster, &supplied).unwrap();
                assert_eq!(out.completion, Completion::Complete);
                assert_eq!(out.results, oracle.results, "{mode} {metric:?} k={k} constrained={constrained} initial");
            }
        }
    }
    assert!(seeded > 0, "the warm start must evaluate decks");
}

#[test]
fn ranked_bound_winner_evicted_by_initial_deck_cannot_close_search() {
    use ournotes_search::{auxiliary, engine, handler, search::Completion};

    let mut synth = synthetic_master(6, 2, 5);
    // Member 1 trades lower power for a stronger skill than the same-character member 6.
    set_column(&mut synth, "MasterLiveSkillEffect", &mut |row| {
        if row["_liveSkillID"] == json!(1) {
            row["_effectValue"] = json!(10000);
        }
    });
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let mut request = joint_request("free", false, json!({"kind":"scoreAtLeast","threshold":991147}));
    request.k = 1;
    request.constraints.leader = Some(2);
    request.strategy = Strategy::Exhaustive;
    let oracle = engine::recommend(&data, &roster, &request).unwrap();
    assert_eq!(oracle.completion, Completion::Complete);
    let a = DeckInput { members: [3, 4, 2, 5, 6], snaps: [Some(1), None, None, None, Some(2)] };
    let b = DeckInput { members: [1, 3, 2, 4, 5], snaps: [None; 5] };
    let built = handler::build_card_pool(&data, &roster, &request).unwrap();
    let a_value = auxiliary::evaluate_built(&built, a.members, a.snaps).unwrap();
    let b_value = auxiliary::evaluate_built(&built, b.members, b.snaps).unwrap();
    let numerator = |row: &ournotes_search::types::RecommendedDeck| {
        row.expected_payoff.as_ref().unwrap().numerator.parse::<i128>().unwrap()
    };
    assert_eq!(numerator(&a_value.results[0]), 0);
    assert_eq!(numerator(&b_value.results[0]), 16);
    assert_eq!(numerator(&oracle.results[0]), 64);
    assert!(a_value.results[0].power > oracle.results[0].power);
    request.strategy = Strategy::BranchAndBound;
    request.initial_decks = vec![a, b];
    for cache in [0, 64] {
        request.limits.cache_entries = cache;
        let result = engine::recommend(&data, &roster, &request).unwrap();
        assert_eq!(result.completion, Completion::Complete);
        assert_eq!(result.results, oracle.results, "evicted initial bound winner; cache={cache}");
        assert!(result.telemetry.environment.bounds.deck_payoff.as_ref().unwrap().handed_over);
    }
}

#[test]
fn leader_score_cache_preserves_native_order_values_and_full_topk() {
    use ournotes_search::{auxiliary, engine, search::Completion};
    let synth = synthetic_master(5, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 5, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 2, 5).to_string()).unwrap();
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [
            json!({"kind":"score"}),
            json!({"kind":"clientEventPoints","eventId":EVENT_ID}),
            json!({"kind":"scoreAndLifeAtLeast","threshold":1,"minFinalLife":1}),
        ] {
            let mut request = joint_request(mode, gekisou, metric);
            request.constraints.leader = None;
            if matches!(request.metric, Metric::ScoreAndLifeAtLeast { .. }) {
                let mut stream =
                    ournotes_sim::live::model::JudgementStream::theoretical_best(&data.chart(SCORE_ID).unwrap());
                stream.judged[0][2] = 1; // An explicit Miss makes terminal-life reuse part of the comparison.
                request.execution = ournotes_search::types::Execution::Live {
                    score_id: SCORE_ID,
                    gekisou,
                    play: ournotes_search::types::PlayPolicy::Stream { stream },
                };
            }
            request.strategy = Strategy::Exhaustive;
            request.k = 5;
            request.limits.cache_entries = 0;
            let cold = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(cold.completion, Completion::Complete);
            assert!(cold.results.iter().any(|deck| deck.snaps.iter().any(Option::is_some)));
            for capacity in [1, 64] {
                request.limits.cache_entries = capacity;
                let cached = engine::recommend(&data, &roster, &request).unwrap();
                assert_eq!(cached.completion, Completion::Complete);
                assert_eq!(cached.results, cold.results, "{mode} capacity={capacity}");
                for deck in &cached.results {
                    let evaluated =
                        auxiliary::evaluate_fixed(&data, &roster, &request, deck.members, deck.snaps).unwrap();
                    assert_eq!(
                        &evaluated.results[0], deck,
                        "{mode}: cached ordered scores must match independent fixed play"
                    );
                }
                if capacity == 64 {
                    assert!(cached.telemetry.caches.team_scores.hits + cached.telemetry.caches.programs.hits > 0);
                    assert!(cached.telemetry.leaves.simulations < cold.telemetry.leaves.simulations);
                }
            }
        }
    }
}

#[test]
fn score_program_cache_reuses_different_snap_power_and_recomputes_event_payoff() {
    use ournotes_search::{auxiliary, engine, handler, search::Completion};

    let mut synth = synthetic_master(5, 3, 5);
    set_column(&mut synth, "MasterSupportCard", &mut |row| {
        let id = row["_id"].as_i64().unwrap();
        // These are distinct physical Snaps with the same complete native skill program.
        row["_supportSkillId01"] = json!(3);
        row["_supportSkillId02"] = json!(1);
        row["_performancePowerMax"] = json!(500 * id);
        row["_technicPowerMax"] = json!(700 * id);
        row["_visualPowerMax"] = json!(900 * id);
    });
    extend_table(
        &mut synth,
        "MasterEventEffect",
        vec![json!({"_id":99,"_eventId":EVENT_ID,"_eventBonusType":0,"_resourceTypeConstraint":3,
            "_supportCardId":2,"_rank1EffectValue":30000,"_rank2EffectValue":30000,
            "_rank3EffectValue":30000,"_rank4EffectValue":30000,"_rank5EffectValue":30000})],
    );
    let data = DeckData::from_json(&data_document(&synth, 5, 3, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 3, 5).to_string()).unwrap();
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            let mut request = joint_request(mode, gekisou, metric);
            // The first class encounter only records identity, the second captures its program, and the
            // third proves reuse at another unseen power. No same-power score cache can explain the hit.
            request.strategy = Strategy::Candidate { power_seeds: 0, proposals: 0, proposal_seed: 0 };
            request.k = 3;
            request.initial_decks = [1, 2, 3]
                .map(|id| DeckInput { members: [1, 2, 3, 4, 5], snaps: [Some(id), None, None, None, None] })
                .to_vec();
            let built = handler::build_card_pool(&data, &roster, &request).unwrap();
            let snaps = &built.pool().snaps;
            // Snap support and Gekisou-support vectors are its entire contribution to Performer;
            // every member and its remaining Performer fields are unchanged between these decks.
            assert_eq!(snaps[0].support_skills().unwrap(), snaps[1].support_skills().unwrap());
            assert_eq!(snaps[0].gekisou_support_skills().unwrap(), snaps[1].gekisou_support_skills().unwrap());
            assert_eq!(snaps[0].support_skills().unwrap(), snaps[2].support_skills().unwrap());
            assert_eq!(snaps[0].gekisou_support_skills().unwrap(), snaps[2].gekisou_support_skills().unwrap());

            request.limits.cache_entries = 0;
            let cold = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(cold.results.len(), 3);
            assert_eq!(cold.results.iter().map(|deck| deck.power).collect::<BTreeSet<_>>().len(), 3);
            assert_ne!(cold.results[0].expected_score, cold.results[1].expected_score);
            if matches!(request.metric, Metric::ClientEventPoints { .. }) {
                assert_ne!(cold.results[0].expected_payoff, cold.results[1].expected_payoff);
            }
            assert_eq!(cold.telemetry.leaves.simulations, 360);
            request.limits.cache_entries = 64;
            let cached = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(cached.results, cold.results, "{mode}: exact scores, orders, ties and current Snap payoff");
            assert_eq!(cached.telemetry.caches.team_scores.hits, 0, "different powers must miss the score cache");
            assert_eq!(cached.telemetry.caches.programs.hits, 1);
            assert_eq!(cached.telemetry.caches.program_orders_reused, 120);
            assert_eq!(cached.telemetry.caches.program_admissions.lookups, 2);
            assert_eq!(cached.telemetry.caches.program_admissions.hits, 1);
            assert_eq!(cached.telemetry.caches.program_recordings, 1);
            assert!(cached.telemetry.caches.program_recorded_nodes > 0);
            assert!(cached.telemetry.caches.program_recorded_bytes > 0);
            assert_eq!(cached.telemetry.leaves.simulations, 240);
            for deck in &cached.results {
                request.limits.cache_entries = 0;
                let native = auxiliary::evaluate_fixed(&data, &roster, &request, deck.members, deck.snaps).unwrap();
                assert_eq!(&native.results[0], deck, "{mode}: all 120 outcomes match an independent native run");
            }

            // The same reuse must also preserve the exhaustive canonical Top-K, beyond the seeded decks.
            request.strategy = Strategy::Exhaustive;
            request.k = 5;
            request.limits.cache_entries = 0;
            let cold = engine::recommend(&data, &roster, &request).unwrap();
            request.limits.cache_entries = 64;
            let cached = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(cold.completion, Completion::Complete);
            assert_eq!(cached.completion, Completion::Complete);
            assert_eq!(cached.results, cold.results, "{mode}: complete canonical Top-K");
            assert!(cached.telemetry.caches.program_orders_reused >= 120);
        }
    }
}

#[test]
fn joint_pt_tiers_preserve_nonmonotone_rewards_and_full_canonical_topk() {
    use ournotes_search::{engine, search::Completion};
    let mut synth = synthetic_master(6, 2, 5);
    // A lower grade can pay more. Bounding by the reward at the maximum reachable
    // score instead of the maximum over ALL reachable tiers would lose solutions.
    set_column(&mut synth, "MasterLiveEventPoint", &mut |r| {
        r["_value"] = json!(match r["_scoreRank"].as_i64().unwrap() {
            2 => 300,
            3 => 900,
            4 => 20,
            _ => 80,
        });
    });
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    for (mode, gk) in [("free", false), ("mission", true)] {
        let mut req = joint_request(mode, gk, json!({"kind":"clientEventPoints","eventId":EVENT_ID}));
        let bounded = engine::recommend(&data, &roster, &req).unwrap();
        assert_eq!(bounded.completion, Completion::Complete);
        assert!(bounded.telemetry.environment.bounds.fallback.is_none());
        req.strategy = Strategy::Exhaustive;
        let oracle = engine::recommend(&data, &roster, &req).unwrap();
        assert_eq!(bounded.results, oracle.results);
    }
}

#[test]
fn joint_score_targets_preserve_full_topk_against_exhaustive() {
    use ournotes_search::{engine, search::Completion};
    let synth = synthetic_master(6, 1, 5);
    let data = DeckData::from_json(&data_document(&synth, 6, 1, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 1, 5).to_string()).unwrap();
    for (mode, gk) in [("free", false), ("mission", true)] {
        for metric in [
            json!({"kind":"scoreAtLeast","threshold":1}),
            json!({"kind":"scoreAtLeast","threshold":100000000}),
            json!({"kind":"cappedScore","threshold":1000}),
            json!({"kind":"scoreAndLifeAtLeast","threshold":1,"minFinalLife":4000}),
        ] {
            let mut request = joint_request(mode, gk, metric.clone());
            request.k = 5;
            if matches!(request.metric, Metric::ScoreAndLifeAtLeast { .. }) {
                let stream =
                    ournotes_sim::live::model::JudgementStream::theoretical_best(&data.chart(SCORE_ID).unwrap());
                request.execution = ournotes_search::types::Execution::Live {
                    score_id: SCORE_ID,
                    gekisou: gk,
                    play: ournotes_search::types::PlayPolicy::Stream { stream },
                };
            }
            let bounded = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(bounded.completion, Completion::Complete, "{mode} {metric}");
            assert!(bounded.telemetry.environment.bounds.compiled, "{mode} {metric}");
            assert!(bounded.telemetry.environment.bounds.fallback.is_none(), "{mode} {metric}");
            request.strategy = Strategy::Exhaustive;
            let exhaustive = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(bounded.results, exhaustive.results, "{mode} {metric}");
        }
    }
}

#[test]
fn joint_challenge_point_tiers_match_exhaustive_without_deck_event_bonus() {
    use ournotes_search::{engine, search::Completion};
    let mut synth = synthetic_master(6, 2, 5);
    set_column(&mut synth, "MasterLiveChallengePoint", &mut |r| {
        r["_value"] = json!(match r["_scoreRank"].as_i64().unwrap() {
            2 => 300,
            3 => 900,
            4 => 20,
            _ => 80,
        });
    });
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    for (mode, gk) in [("free", false), ("mission", true)] {
        let mut req = joint_request(mode, gk, json!({"kind":"clientChallengePoints","eventId":EVENT_ID}));
        let bounded = engine::recommend(&data, &roster, &req).unwrap();
        assert_eq!(bounded.completion, Completion::Complete);
        assert!(bounded.telemetry.environment.bounds.fallback.is_none());
        req.strategy = Strategy::Exhaustive;
        let oracle = engine::recommend(&data, &roster, &req).unwrap();
        assert_eq!(bounded.results, oracle.results, "{mode}");
    }
}

#[test]
fn composition_frontier_recovers_every_team_when_k_exceeds_binding_count() {
    use ournotes_search::{
        engine,
        search::{Completion, ablate, set_bound_ablation},
        types::Strategy,
    };
    let synth = synthetic_master(6, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let mut request = joint_request("free", false, json!({"kind":"clientEventPoints","eventId":EVENT_ID}));
    request.k = 64;
    request.constraints.no_snaps = true;
    let ranked = engine::recommend(&data, &roster, &request).unwrap();
    // The composition traversal runs when the deck payoff ranking hands the search over.
    set_bound_ablation(ablate::NO_DECK_PAYOFF);
    let actual = engine::recommend(&data, &roster, &request);
    set_bound_ablation(0);
    let actual = actual.unwrap();
    request.strategy = Strategy::Exhaustive;
    let oracle = engine::recommend(&data, &roster, &request).unwrap();
    for out in [&ranked, &actual] {
        assert_eq!(out.completion, Completion::Complete);
        assert_eq!(out.results, oracle.results);
    }
    // Members 1 and 6 share a character: two teams under the fixed leader.
    assert_eq!(actual.results.len(), 2);
    assert!(actual.telemetry.composition.power_frontier_closed > 0);
}

#[test]
fn pt_incumbent_regime_removes_only_strictly_inferior_members() {
    use ournotes_search::{
        engine,
        search::{Completion, ablate, set_bound_ablation},
    };
    for (equal, wide_k) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut synth = synthetic_master(6, 2, 5);
        replace_table(
            &mut synth,
            "MasterLiveScoreRank",
            json!([{"_id":1,"_group":1,"_liveScoreRank":2,"_requiredScore":0,"_battleLiveRequiredScore":0}]),
        );
        replace_table(&mut synth,"MasterEventEffect",Value::Array((1..=if equal {6} else {5}).map(|id|json!({
            "_id":id,"_eventId":EVENT_ID,"_resourceTypeConstraint":2,"_memberCardId":id,"_eventBonusType":0,
            "_rank1EffectValue":1000,"_rank2EffectValue":1000,"_rank3EffectValue":1000,"_rank4EffectValue":1000,"_rank5EffectValue":1000
        })).collect()));
        let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
        let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
        let mut request = joint_request("mission", true, json!({"kind":"clientEventPoints","eventId":EVENT_ID}));
        if wide_k {
            request.k = 64;
            request.constraints.no_snaps = true;
        }
        let ranked = engine::recommend(&data, &roster, &request).unwrap();
        assert_eq!(ranked.completion, Completion::Complete);
        // The incumbent regime runs when the deck payoff ranking hands the search over.
        set_bound_ablation(ablate::NO_DECK_PAYOFF);
        let actual = engine::recommend(&data, &roster, &request);
        set_bound_ablation(0);
        let actual = actual.unwrap();
        assert_eq!(actual.completion, Completion::Complete);
        assert_eq!(
            actual.telemetry.environment.bounds.pt_regime.as_ref().map_or(0, |r| r.members_removed),
            usize::from(!equal && !wide_k),
            "telemetry={:?}; result={:?}",
            actual.telemetry,
            actual.results.first().map(|r| &r.expected_payoff)
        );
        assert!(actual.telemetry.environment.bounds.pt_regime.as_ref().is_none_or(|r| r.fallback.is_none()));
        request.strategy = Strategy::Exhaustive;
        let oracle = engine::recommend(&data, &roster, &request).unwrap();
        assert_eq!(actual.results, oracle.results, "equal-bonus alternative={equal}");
        assert_eq!(ranked.results, oracle.results, "equal-bonus alternative={equal}");
    }
}

#[test]
fn joint_search_falls_back_for_wrapping_pt_and_explicit_clocks() {
    use ournotes_search::{engine, search::Completion, types::Strategy};
    let mut synth = synthetic_master(5, 1, 5);
    set_column(&mut synth, "MasterLiveEventPoint", &mut |row| row["_value"] = json!(1_000_000_000));
    let data = DeckData::from_json(&data_document(&synth, 5, 1, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 1, 5).to_string()).unwrap();
    let mut req = joint_request("free", false, json!({"kind":"clientEventPoints","eventId":EVENT_ID}));
    let bounded = engine::recommend(&data, &roster, &req).unwrap();
    assert!(bounded.telemetry.environment.bounds.fallback.as_ref().unwrap().contains("wrapping"));
    req.strategy = Strategy::Exhaustive;
    assert_eq!(bounded.results, engine::recommend(&data, &roster, &req).unwrap().results);
    let mut req = joint_request("free", false, json!({"kind":"score"}));
    req.simulation.music_length_ms = Some(20_000);
    let bounded = engine::recommend(&data, &roster, &req).unwrap();
    assert!(bounded.telemetry.environment.bounds.fallback.as_ref().unwrap().contains("clocks"));
    assert_eq!(bounded.completion, Completion::Complete);
    req.strategy = Strategy::Exhaustive;
    assert_eq!(bounded.results, engine::recommend(&data, &roster, &req).unwrap().results);
    req.strategy = Strategy::BranchAndBound;
    req.limits.time_limit_ms = Some(0);
    let stopped = engine::recommend(&data, &roster, &req).unwrap();
    assert_eq!(stopped.completion, Completion::TimedOut);
    assert!(stopped.results.is_empty());
}

#[test]
fn built_problem_reuses_frozen_inputs_across_live_snap_and_gekisou_score_pt() {
    use ournotes_search::{auxiliary, engine, handler, search};
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            let synth = synthetic_master(5, 2, 5);
            let data = DeckData::from_json(&data_document(&synth, 5, 2, 5).to_string()).unwrap();
            let roster_json = roster_document(5, 2, 5).to_string();
            let mut roster = Roster::from_json(&roster_json).unwrap();
            let request_json = json!({"format":"ournotes-deck.search-request/1",
                "execution":{"kind":"live","scoreId":SCORE_ID,"gekisou":gekisou,"play":{"kind":"theoreticalBest"}},
                "scenario":{"kind":mode,"musicId":10}, "context":context_document(false,false,false),
                "metric":metric,"k":3,"constraints":{"leader":3},
                "strategy":{"kind":"exhaustive"},"limits":{"cacheEntries":0}})
            .to_string();
            let mut request: ournotes_search::types::RecommendationRequest =
                serde_json::from_str(&request_json).unwrap();
            let built = handler::build_card_pool(&data, &roster, &request).unwrap();
            assert_eq!(built.context().route(), handler::SolverRoute::PhysicalExhaustive);
            assert_eq!(built.domain().members().len(), 5);
            assert_eq!(built.domain().snaps().len(), 2);
            assert!(built.domain().is_feasible());
            let direct = engine::recommend(&data, &roster, &request).unwrap();
            let first = search::recommend_built(&built).unwrap();
            assert_eq!(first.results, direct.results);
            assert_eq!(first.completion, search::Completion::Complete);
            assert!(first.telemetry.phases.iter().all(|p| p.name != "prepare"));
            // Every team under the fixed leader: 31 Snap pairings of one member set.
            assert_eq!(first.telemetry.leaves.visited, 31);
            let json_result: Value =
                serde_json::from_str(&engine::recommend_json(&data, &roster_json, &request_json).unwrap()).unwrap();
            assert_eq!(json_result["results"], serde_json::to_value(&first.results).unwrap());
            // The caller can reuse/change its transport buffers, never the compiled problem.
            roster.members.clear();
            request.constraints.no_snaps = true;
            request.k = 1;
            let again = search::recommend_built(&built).unwrap();
            assert_eq!(again.results, first.results);
            assert_eq!(again.telemetry.leaves.simulations, first.telemetry.leaves.simulations);
            assert_eq!(built.context().request().k, 3);
            for row in &first.results {
                let evaluated = auxiliary::evaluate_built(&built, row.members, row.snaps).unwrap();
                assert_eq!(evaluated.results, std::slice::from_ref(row));
                assert_eq!(evaluated.optimality, ournotes_search::types::Optimality::NotApplicable);
                assert_eq!(evaluated.telemetry.leaves.visited, 1);
            }
            assert!(auxiliary::evaluate_built(&built, [1, 2, 3, 4, 5], [Some(1), Some(1), None, None, None]).is_err());
            assert!(auxiliary::evaluate_built(&built, [3, 2, 1, 4, 5], [None; 5]).is_err());
        }
    }
}

#[test]
fn built_problem_validates_before_zero_budget_and_preserves_solver_routes() {
    use ournotes_search::{engine, handler, search};
    let synth = synthetic_master(5, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 5, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 2, 5).to_string()).unwrap();
    for execution in
        [json!({"kind":"power","musicId":10,"eventParameter":false}), json!({"kind":"skip","scoreId":SCORE_ID})]
    {
        let metric = if execution["kind"] == "power" { json!({"kind":"power"}) } else { json!({"kind":"score"}) };
        let request = fixed_request(execution, metric);
        let built = handler::build_card_pool(&data, &roster, &request).unwrap();
        assert_eq!(built.context().route(), handler::SolverRoute::PhysicalBranchAndBound);
        let via_built = search::recommend_built(&built).unwrap();
        assert_eq!(via_built.result_identity, "team");
        assert_eq!(via_built.results, engine::recommend(&data, &roster, &request).unwrap().results);
    }
    let mut request = fixed_request(
        json!({"kind":"live","scoreId":SCORE_ID,"gekisou":false,"play":{"kind":"theoreticalBest"}}),
        json!({"kind":"score"}),
    );
    request.limits.time_limit_ms = Some(0);
    let built = handler::build_card_pool(&data, &roster, &request).unwrap();
    let empty = search::recommend_built(&built).unwrap();
    assert_eq!(empty.completion, search::Completion::TimedOut);
    assert!(empty.results.is_empty());
    request.constraints.include_members = vec![999];
    assert!(handler::build_card_pool(&data, &roster, &request).is_err());
    request.constraints.include_members.clear();
    request.execution = ournotes_search::types::Execution::Live {
        score_id: SCORE_ID,
        gekisou: false,
        play: ournotes_search::types::PlayPolicy::Stream {
            stream: ournotes_sim::live::model::JudgementStream {
                frames: vec![],
                judged: vec![],
                base_seed: 0,
                assist: false,
                delta_times: None,
            },
        },
    };
    assert!(handler::build_card_pool(&data, &roster, &request).is_err());
}

fn synthetic_master(members: i64, snaps: i64, characters: i64) -> Synth {
    let mut s = synth_snaps(&mut Rng::new(FIXTURE_SEED), members, snaps, &[1, 3, 6, 10, 11]);
    replace_table(
        &mut s,
        "MasterCharacter",
        Value::Array((1..=characters).map(|id| json!({"_id":id,"_bandID":(id-1)%3+1})).collect()),
    );
    set_column(&mut s, "MasterMemberCard", &mut |r| {
        let id = r["_id"].as_i64().unwrap();
        r["_characterID"] = json!((id - 1) % characters + 1);
        r["_cardType"] = json!((id - 1) % 5 + 1);
        r["_liveSkillID"] = json!((id - 1) % 3 + 1);
        r["_leaderSkillID"] = json!(4);
    });
    set_column(&mut s, "MasterSupportCard", &mut |r| {
        let id = r["_id"].as_i64().unwrap();
        r["_characterIDs"] = json!([(id - 1) % characters + 1]);
        r["_supportSkillId01"] = json!(if id % 2 == 1 { 3 } else { 9 });
        r["_supportSkillId02"] = json!(if id % 2 == 1 { 1 } else { 6 });
    });
    replace_table(
        &mut s,
        "MasterLiveMusic",
        json!([{
            "_id":10,"_musicType":1,"_bestMusicTagIDs":[1],"_expertID":SCORE_ID,
            "_liveScoreRankGroup":1,"_gekisouMission1":1,"_gekisouMission2":3,"_gekisouMission3":1
        }]),
    );
    replace_table(
        &mut s,
        "MasterLiveMusicScore",
        json!([{
            "_id":SCORE_ID,"_musicScoreLevel":24,"_fullComboCount":12
        }]),
    );
    replace_table(
        &mut s,
        "MasterChallengeMusic",
        json!([
            {"_id":70,"_eventId":EVENT_ID,"_liveMusicId":10,"_musicType":4,"_bestMusicTagIDs":[2]},
            {"_id":71,"_eventId":EVENT_ID,"_liveMusicId":10,"_musicType":0,"_bestMusicTagIDs":[]}
        ]),
    );
    replace_table(
        &mut s,
        "MasterArenaMusic",
        json!([{
            "_id":80,"_liveMusicId":10,"_liveMusicType":5,
            "_gekisouMission1":3,"_gekisouMission2":1,"_gekisouMission3":3
        }]),
    );
    replace_table(
        &mut s,
        "MasterLiveJudgementTiming",
        json!([
            {"_id":1,"_noteJudgementType":1,"_noteSimulateJudgement":6,"_beforeMs":40,"_afterMs":40},
            {"_id":2,"_noteJudgementType":1,"_noteSimulateJudgement":5,"_beforeMs":80,"_afterMs":80},
            {"_id":3,"_noteJudgementType":2,"_noteSimulateJudgement":5,"_beforeMs":80,"_afterMs":80}
        ]),
    );
    extend_table(
        &mut s,
        "MasterLiveSettings",
        vec![
            json!({"_id":30,"_key":"gekisou_luck_gauge_max","_value":"40"}),
            json!({"_id":31,"_key":"gekisou_luck_gauge_max_rush","_value":"20"}),
            json!({"_id":32,"_key":"gekisou_luck_rush_score_bonus_percent","_value":"10"}),
        ],
    );
    extend_table(
        &mut s,
        "MasterLiveComboScoreBonus",
        (1..=4).map(|i| json!({"_id":100+i,"_comboBonusType":1,"_requiredComboCount":i,"_bonusFactor":0.02})).collect(),
    );
    replace_table(
        &mut s,
        "MasterLiveGekisouRankingScoreBonus",
        Value::Array(
            (1..=3)
                .flat_map(|pattern| {
                    (1..=3).map(move |count| {
                        json!({"_id":pattern*10+count,"_missionPattern":pattern,
            "_count":count,"_rank":1,"_scoreBonusPercent":10})
                    })
                })
                .collect(),
        ),
    );
    replace_table(&mut s, "MasterLiveGekisouLuckBasePoint", Value::Array((3..=6).map(|judgement| {
        json!({"_id":judgement,"_noteCategory":0,"_noteSimulateJudgement":judgement,"_weight":1,"_basePoint":10})
    }).collect()));
    replace_table(
        &mut s,
        "MasterLiveGekisouLuckBonusLot",
        Value::Array(
            (0..5)
                .flat_map(|kind| {
                    (0..4).map(move |result| {
                        json!({"_id":kind*10+result+1,"_chanceLotType":kind,
            "_lotResult":result,"_weight":([5,4,2,1][result as usize])})
                    })
                })
                .collect(),
        ),
    );
    // Own skill-event trigger without a probability check (the uniform member-order target is lottery-free).
    replace_table(
        &mut s,
        "MasterGekisouSkillEffect",
        json!([{
            "_id":1,"_gekisouSkillID":1,"_level":1,"_skillTriggerType":1,
            "_skillTriggerConditionGroup":53,"_skillConditionGroup":0,"_skillReleaseConditionGroup":0,
            "_skillTargetIDs":[],"_skillEffectType":2000,"_activationTimeSecond":0.4,"_effectValue":900,
            "_maxEffectValue":0,"_effectLimitCount":0,"_skillCumulativeConditionID":0,
            "_effectExecuteLimitCount":0,"_effectExecuteLimitResetConditionGroup":0
        }]),
    );
    replace_table(
        &mut s,
        "MasterEvent",
        json!([{
            "_id":EVENT_ID,"_liveEventPointGroup":1,"_challengeLiveEventPointGroup":2,
            "_liveEventRewardGroup":37,"_challengeLiveEventRewardGroup":41
        }]),
    );
    replace_table(
        &mut s,
        "MasterEventEffect",
        Value::Array(
            (0..=2)
                .map(|kind| {
                    json!({"_id":kind+1,"_eventId":EVENT_ID,"_eventBonusType":kind,"_resourceTypeConstraint":2,
            "_rank1EffectValue":1000,"_rank2EffectValue":1000,"_rank3EffectValue":1000,
            "_rank4EffectValue":1000,"_rank5EffectValue":1000})
                })
                .collect(),
        ),
    );
    replace_table(&mut s, "MasterLiveScoreRank", Value::Array([0,300_000,450_000,600_000].iter().enumerate().map(|(i, score)| {
        json!({"_id":i+1,"_group":1,"_liveScoreRank":i+2,"_requiredScore":score,"_battleLiveRequiredScore":score})
    }).collect()));
    for (table, group, base) in [
        ("MasterLiveEventPoint", 1, 100),
        ("MasterChallengeLiveEventPoint", 2, 300),
        ("MasterLiveChallengePoint", 1, 5),
    ] {
        replace_table(
            &mut s,
            table,
            Value::Array(
                (2..=5)
                    .map(|rank| json!({"_id":rank,"_group":group,"_scoreRank":rank,"_value":base+(rank-2)*base/3}))
                    .collect(),
            ),
        );
    }
    replace_table(
        &mut s,
        "MasterLiveMusicBoostBonus",
        json!([{
            "_id":1,"_consumedLiveBoostCount":1,"_liveMusicRewardRate":2,"_playerExpRate":2,
            "_memberCardExpRate":2,"_friendshipExpRate":2,"_eventPointRate":2
        }]),
    );
    replace_table(
        &mut s,
        "MasterChallengeMusicBoostBonus",
        json!([{
            "_id":1,"_consumedChallengePointCount":201,"_liveMusicRewardRate":2,"_playerExpRate":2,
            "_memberCardExpRate":2,"_friendshipExpRate":2,"_eventPointRate":2
        }]),
    );
    for (table, count, group) in [("MasterLiveEventReward", 3, 37), ("MasterChallengeLiveEventReward", 4, 41)] {
        replace_table(
            &mut s,
            table,
            Value::Array(
                (0..=7)
                    .map(|rank| {
                        json!({
                            "_id":rank+3,"_group":1,"_eventGroup":group,"_scoreRank":rank,"_resourceType":11,
                            "_resourceId":9,"_resourceCount":count,"_probability":10000
                        })
                    })
                    .collect(),
            ),
        );
    }
    replace_table(
        &mut s,
        "MasterEventAchievementReward",
        json!([{
            "_id":1,"_eventId":EVENT_ID,"_eventPoint":100,"_rewardIds":[5]
        }]),
    );
    s
}

fn columns_and_rows(s: &Synth) -> Value {
    let mut master: serde_json::Map<_, _> = s
        .tables
        .iter()
        .map(|(name, rows)| {
            let objects = rows.as_array().expect("synthetic table is an array");
            let columns: Vec<_> = objects
                .iter()
                .flat_map(|r| r.as_object().unwrap().keys().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let data_rows: Vec<Vec<Value>> = objects
                .iter()
                .map(|r| columns.iter().map(|key| r.get(key).cloned().unwrap_or(Value::Null)).collect())
                .collect();
            (name.clone(), json!({"columns":columns,"rows":data_rows}))
        })
        .collect();
    common::every_table(&mut master);
    json!(master)
}

fn data_document(s: &Synth, members: i64, snaps: i64, characters: i64) -> Value {
    json!({
        "format":"nnnotes.deck-data/1",
        "provenance":{"synthetic":true,"fixtureSeed":FIXTURE_SEED,"region":"synthetic",
            "masterVersion":"synthetic-recommend-fixture-20261001","clientVersion":"synthetic-inputs",
            "source":"tests/recommend_fixture_export.rs + tests/common::synth_snaps",
            "members":members,"snaps":snaps,"characters":characters,"isOCRTruth":false,
            "assetSha256Policy":"64 zero placeholder; no game asset or real chart is claimed"},
        "master":columns_and_rows(s),
        "charts":[{"scoreId":SCORE_ID,"asset":{"key":"SYNTHETIC-short-chart-1004",
            "sha256":"0000000000000000000000000000000000000000000000000000000000000000"},
            "notes":{"id":(1..=12).collect::<Vec<_>>(),"op":vec![1;12],
                "judgementType":vec![1;12],"timeMs":(1..=12).map(|i|i*100).collect::<Vec<_>>()},
            "skillEvents":{"timeMs":[0,250,500,750,1000]},
            "fevers":{"startMs":[150,450,850],"endMs":[400,800,1150]}}]
    })
}

fn roster_document(members: i64, snaps: i64, characters: i64) -> Value {
    let ranks: BTreeMap<_, _> = (1..=characters).map(|id| (id.to_string(), 10)).collect();
    json!({
        "provenance":{"synthetic":true,"isOCRTruth":false,"source":"manually fixed synthetic progress"},
        "player":{"characterRanks":ranks,"bandItems":{},"vipRank":3,"events":[EVENT_ID],
            "memory":null,"ownedMemberCardIds":(1..=members).collect::<Vec<_>>(),
            "ownedSupportCardIds":(1..=snaps).collect::<Vec<_>>()},
        "members":(1..=members).map(|id| json!({"id":id,"level":40,"exp":null,"awake":2,
            "rank":3,"liveSkillLevel":4,"gekisouSkillLevel":1})).collect::<Vec<_>>(),
        "snaps":(1..=snaps).map(|id| json!({"id":id,"level":30,"exp":null,"rank":3})).collect::<Vec<_>>()
    })
}

fn context_document(skip: bool, multiplayer: bool, expired: bool) -> Value {
    let mut value = json!({
        "powerSnapshot":{"eventIds":[EVENT_ID],"capturedJstTicks":50},
        "resultClock":if skip {json!({"execution":"skip","serverNowJstTicks":if expired {200} else {150}})}
            else {json!({"execution":"played","savedStartJstTicks":150,"serverNowJstTicks":201})},
        "eventPayoff":{"consumedCount":0,"localEvents":[{"eventId":EVENT_ID,"points":0,
            "challengePoints":500,"added":[]}],"eventWindows":[{"eventId":EVENT_ID,
            "startJstTicks":100,"endJstTicks":200}]}
    });
    if multiplayer {
        value["eventPayoff"]["multiplayerResultPanel"] = json!({"localPlayerIndex":1,
            "localDisconnected":false,"otherPlayers":[{"finalScore":100000,"disconnected":false},
                {"finalScore":50000,"disconnected":true}]});
    }
    value
}

#[test]
#[ignore = "export manual synthetic input for explicit CLI/WASM checks"]
fn export_adapter_inputs() {
    let directory = std::env::var_os("BDON_FIXTURE_OUT").expect("BDON_FIXTURE_OUT");
    let out = Path::new(&directory);
    fs::create_dir_all(out).unwrap();
    let synth = synthetic_master(7, 3, 7);
    let document = data_document(&synth, 7, 3, 7);
    let roster = roster_document(7, 3, 7);
    let data = DeckData::from_json(&document.to_string()).unwrap();
    Roster::from_json(&roster.to_string()).unwrap();
    let stream = ournotes_sim::live::model::JudgementStream::theoretical_best(&data.chart(SCORE_ID).unwrap());
    for (name, value) in [
        ("DeckData.json", document),
        ("roster.json", roster),
        ("play-ordinary.json", serde_json::to_value(stream).unwrap()),
        ("context-played.json", context_document(false, false, false)),
        ("context-skip.json", context_document(true, false, false)),
    ] {
        let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
        bytes.push(b'\n');
        fs::write(out.join(name), bytes).unwrap();
    }
}

#[test]
#[ignore = "export synthetic Live/Gekisou search-harness corpus"]
fn export_search_harness_inputs() {
    let directory = std::env::var_os("BDON_HARNESS_OUT").expect("BDON_HARNESS_OUT");
    let out = Path::new(&directory);
    fs::create_dir_all(out).unwrap();
    // Same-character alternatives plus two distinct support programs. All optional
    // Snap bindings and every team remain in the declared domain.
    let stress = std::env::var_os("BDON_HARNESS_STRESS").is_some();
    let (members, snaps, characters) = if stress { (8, 3, 6) } else { (6, 2, 5) };
    let synth = synthetic_master(members, snaps, characters);
    let data = data_document(&synth, members, snaps, characters);
    let roster = roster_document(members, snaps, characters);
    for (name, value) in [("DeckData.json", data), ("roster.json", roster)] {
        fs::write(out.join(name), serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
    let mut cases = Vec::new();
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for (objective, metric) in
            [("score", json!({"kind":"score"})), ("pt", json!({"kind":"clientEventPoints","eventId":EVENT_ID}))]
        {
            if stress && (mode != "free" || objective != "pt") {
                continue;
            }
            let id = format!("{mode}-{objective}");
            let request = json!({"format":"ournotes-deck.search-request/1",
                "execution":{"kind":"live","scoreId":SCORE_ID,"gekisou":gekisou,
                    "play":{"kind":"theoreticalBest"}},
                "scenario":{"kind":mode,"musicId":10},
                "context":context_document(false,false,false),"metric":metric,"k":3,
                "strategy":{"kind":"exhaustive"},
                "limits":{"timeLimitMs":null,"maxCandidates":null,"cacheEntries":0}});
            let request_name = format!("{id}-request.json");
            fs::write(out.join(&request_name), serde_json::to_vec_pretty(&request).unwrap()).unwrap();
            let mut case = json!({"id":id,"data":"DeckData.json","roster":"roster.json",
                "request":request_name,"oracleMaxCandidates":if stress {500000} else {10000},
                "dominance":[{"kind":"member","from":1,"to":6},
                    {"kind":"snap","from":1,"to":2}],
                "experiments":[
                    {"name":"exhaustive","patch":{},"repeats":2},
                    {"name":"joint-bnb","patch":{"strategy":{"kind":"branchAndBound"}},"repeats":2},
                    {"name":"candidate-128","patch":{"strategy":{"kind":"candidate",
                        "powerSeeds":2,"proposals":128,"proposalSeed":8419}},"repeats":2},
                    {"name":"remove-snap-1-unproved","patch":{"constraints":{"excludeSnaps":[1]}},"repeats":1}]});
            if stress {
                case["dominance"] = json!([]);
                case["experiments"] =
                    json!([{"name":"joint-bnb","patch":{"strategy":{"kind":"branchAndBound"}},"repeats":3}]);
            }
            let name = format!("{id}.json");
            fs::write(out.join(&name), serde_json::to_vec_pretty(&case).unwrap()).unwrap();
            cases.push(name);
        }
    }
    cases.push(sustained_combo::export(out));
    fs::write(
        out.join("suite.json"),
        serde_json::to_vec_pretty(&json!({
        "format":"ournotes-deck.search-harness-suite/1","scope":"synthetic current-model",
        "cases":cases}))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn unsupported_lifecycle_rejected_before_low_level_feasibility_or_validation() {
    use ournotes_search::search::{Constraints, Objective, SearchRequest, solve_physical};
    use ournotes_sim::{Error, pool::Pool};
    let synth = synthetic_master(7, 3, 7);
    let data = DeckData::from_json(&data_document(&synth, 7, 3, 7).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(7, 3, 7).to_string()).unwrap();
    let pool = Pool::new(&data.master, &roster).unwrap();
    let request = SearchRequest {
        objective: Objective::Power { music_id: Some(10), event: false },
        k: 1,
        constraints: Constraints { include_members: vec![999], ..Default::default() },
        time_limit: None,
    };
    let result = solve_physical(
        &pool,
        &request,
        &Metric::Power,
        None,
        &Limits::default(),
        &Strategy::Exhaustive,
        None,
        &SimulationInput { live_finished_from_frame: Some(0), ..Default::default() },
    );
    assert!(matches!(result, Err(Error::Unsupported(_))));
}

fn fixed_request(execution: Value, metric: Value) -> ournotes_search::types::RecommendationRequest {
    let value = json!({
        "format":"ournotes-deck.search-request/1", "execution":execution,
        "scenario":{"kind":"free","musicId":10}, "metric":metric, "k":100,
        "constraints":{"leader":3,"noSnaps":true},
        "limits":{"timeLimitMs":null,"maxCandidates":null,"cacheEntries":128}
    });
    serde_json::from_value(value).unwrap()
}

#[test]
fn fixed_deck_matches_search_without_changing_slots_or_payoff() {
    use ournotes_search::{auxiliary::evaluate_fixed, engine::recommend, types::Optimality};
    let s = synthetic_master(5, 0, 5);
    let data = DeckData::from_json(&data_document(&s, 5, 0, 5).to_string()).unwrap();
    let owned = Roster::from_json(&roster_document(5, 0, 5).to_string()).unwrap();
    for execution in [
        json!({"kind":"power","musicId":10,"eventParameter":false}),
        json!({"kind":"skip","scoreId":SCORE_ID}),
        json!({"kind":"live","scoreId":SCORE_ID,"gekisou":false,"play":{"kind":"theoreticalBest"}}),
    ] {
        let power = execution["kind"] == "power";
        let live = execution["kind"] == "live";
        let r = fixed_request(execution, json!({"kind":if power {"power"} else {"score"}}));
        let search = recommend(&data, &owned, &r).unwrap();
        let best = &search.results[0];
        let fixed = evaluate_fixed(&data, &owned, &r, best.members, best.snaps).unwrap();
        assert_eq!(fixed.optimality, Optimality::NotApplicable);
        assert_eq!(fixed.result_identity, if live { "fixedTeam" } else { "fixedPhysicalDeck" });
        assert_eq!(fixed.telemetry.leaves.evaluated, 1);
        let actual = &fixed.results[0];
        assert_eq!(actual.members, best.members);
        assert_eq!(actual.snaps, best.snaps);
        assert_eq!(actual.power, best.power);
        assert_eq!(actual.expected_score, best.expected_score);
        assert_eq!(actual.expected_payoff, best.expected_payoff);
        assert_eq!(actual.score_summary, best.score_summary);
        assert_eq!(actual.best_order, best.best_order);
        if live {
            // A team's value does not depend on the layout of its non-leader slots: every layout of the same
            // (member, Snap) pairs evaluates to the same canonical result.
            assert_eq!(search.result_identity, "team");
            // The canonical layout: the non-leader members in ascending ID order.
            let others = [actual.members[0], actual.members[1], actual.members[3], actual.members[4]];
            assert!(others.windows(2).all(|w| w[0] < w[1]), "{others:?}");
            let order = actual.best_order.as_ref().unwrap();
            assert_eq!(order.members, order.performance_order.map(|slot| actual.members[slot]));
            for layout in nonleader_layouts() {
                let members = layout.map(|slot| best.members[slot]);
                let other = evaluate_fixed(&data, &owned, &r, members, best.snaps).unwrap();
                assert_eq!(other.results, fixed.results, "{layout:?}");
            }
        }
    }
}

/// Every placement of the four non-leader slots (slot 2 leads).
fn nonleader_layouts() -> Vec<[usize; 5]> {
    let slots = [0, 1, 3, 4];
    let mut out = Vec::new();
    for a in slots {
        for b in slots {
            for c in slots {
                for d in slots {
                    if BTreeSet::from([a, b, c, d]).len() == 4 {
                        out.push([a, b, 2, c, d]);
                    }
                }
            }
        }
    }
    assert_eq!(out.len(), 24);
    out
}

/// A fixed non-identity permutation of the non-leader slots.
fn permuted<T: Copy>(slots: [T; 5]) -> [T; 5] {
    [slots[4], slots[3], slots[2], slots[0], slots[1]]
}

#[test]
fn fixed_deck_rejects_constraints_and_never_substitutes_another_deck() {
    use ournotes_search::{auxiliary::evaluate_fixed, search::Completion};
    use ournotes_sim::Error;
    let s = synthetic_master(6, 2, 6);
    let data = DeckData::from_json(&data_document(&s, 6, 2, 6).to_string()).unwrap();
    let owned = Roster::from_json(&roster_document(6, 2, 6).to_string()).unwrap();
    let r = fixed_request(json!({"kind":"power","musicId":10,"eventParameter":false}), json!({"kind":"power"}));
    for (members, snaps) in [
        ([3, 2, 1, 4, 5], [None; 5]), // declared leader is in the wrong physical slot
        ([1, 2, 3, 4, 5], [Some(1), None, None, None, None]), // noSnaps
        ([1, 2, 3, 4, 4], [None; 5]), // duplicated character
    ] {
        assert!(matches!(evaluate_fixed(&data, &owned, &r, members, snaps), Err(Error::Input(_))));
    }
    let mut excluded = r.clone();
    excluded.constraints.exclude_members = vec![1];
    assert!(matches!(evaluate_fixed(&data, &owned, &excluded, [1, 2, 3, 4, 5], [None; 5]), Err(Error::Input(_))));
    let mut required = r.clone();
    required.constraints.include_members = vec![6];
    assert!(matches!(evaluate_fixed(&data, &owned, &required, [1, 2, 3, 4, 5], [None; 5]), Err(Error::Input(_))));
    let mut zero = r;
    zero.limits.time_limit_ms = Some(0);
    let out = evaluate_fixed(&data, &owned, &zero, [1, 2, 3, 4, 5], [None; 5]).unwrap();
    assert_eq!(out.completion, Completion::TimedOut);
    assert!(out.results.is_empty());
    assert_eq!(out.telemetry.leaves.evaluated, 0);
}

#[test]
fn song_ranking_resolves_each_song_and_shares_the_total_candidate_budget() {
    use ournotes_search::{
        auxiliary::{SongTarget, evaluate_fixed, rank_fixed_songs},
        search::Completion,
        types::{Execution, Scene},
    };
    let mut s = synthetic_master(5, 0, 5);
    set_column(&mut s, "MasterMemberCard", &mut |row| {
        row["_cardType"] = json!(1);
    });
    extend_table(
        &mut s,
        "MasterLiveMusic",
        vec![json!({
            "_id":11,"_musicType":2,"_bestMusicTagIDs":[],"_expertID":2004,
            "_liveScoreRankGroup":1,"_gekisouMission1":1,"_gekisouMission2":3,"_gekisouMission3":1
        })],
    );
    extend_table(&mut s, "MasterLiveMusicScore", vec![json!({"_id":2004,"_musicScoreLevel":24,"_fullComboCount":12})]);
    let mut doc = data_document(&s, 5, 0, 5);
    let mut second = doc["charts"][0].clone();
    second["scoreId"] = json!(2004);
    doc["charts"].as_array_mut().unwrap().push(second);
    let data = DeckData::from_json(&doc.to_string()).unwrap();
    let owned = Roster::from_json(&roster_document(5, 0, 5).to_string()).unwrap();
    let r = fixed_request(json!({"kind":"skip","scoreId":SCORE_ID}), json!({"kind":"score"}));
    let targets = [
        SongTarget { score_id: SCORE_ID, scenario: Scene::Free { music_id: 10 } },
        SongTarget { score_id: 2004, scenario: Scene::Free { music_id: 11 } },
    ];
    let ranked = rank_fixed_songs(&data, &owned, &r, [1, 2, 3, 4, 5], [None; 5], &targets).unwrap();
    assert_eq!(ranked.completion, Completion::Complete);
    assert!(ranked.remaining_score_ids.is_empty());
    assert_eq!(ranked.results.len(), 2);
    for target in &targets {
        let mut one = r.clone();
        one.scenario = Some(target.scenario.clone());
        one.execution = Execution::Skip { score_id: target.score_id };
        let expected = evaluate_fixed(&data, &owned, &one, [1, 2, 3, 4, 5], [None; 5]).unwrap();
        let actual = ranked.results.iter().find(|row| row.score_id == target.score_id).unwrap();
        assert_eq!(actual.evaluation.results, expected.results);
    }
    assert_ne!(ranked.results[0].evaluation.results[0].power, ranked.results[1].evaluation.results[0].power);
    assert_eq!(ranked.results.iter().map(|row| row.score_id).collect::<Vec<_>>(), [SCORE_ID, 2004]);
    let reverse =
        rank_fixed_songs(&data, &owned, &r, [1, 2, 3, 4, 5], [None; 5], &[targets[1].clone(), targets[0].clone()])
            .unwrap();
    assert_eq!(reverse.results.iter().map(|row| row.score_id).collect::<Vec<_>>(), [SCORE_ID, 2004]);
    let mut limited = r.clone();
    limited.limits.max_candidates = Some(1);
    let out = rank_fixed_songs(&data, &owned, &limited, [1, 2, 3, 4, 5], [None; 5], &targets).unwrap();
    assert_eq!(out.completion, Completion::TimedOut);
    assert_eq!(out.results.len(), 1);
    assert_eq!(out.remaining_score_ids, [2004]);
    limited.limits.time_limit_ms = Some(0);
    let out = rank_fixed_songs(&data, &owned, &limited, [1, 2, 3, 4, 5], [None; 5], &targets).unwrap();
    assert!(out.results.is_empty());
    assert_eq!(out.remaining_score_ids, [SCORE_ID, 2004]);
    assert!(
        rank_fixed_songs(&data, &owned, &r, [1, 2, 3, 4, 5], [None; 5], &[targets[0].clone(), targets[0].clone()])
            .is_err()
    );

    let live = fixed_request(
        json!({"kind":"live","scoreId":SCORE_ID,"gekisou":false,"play":{"kind":"theoreticalBest"}}),
        json!({"kind":"score"}),
    );
    let ranked = rank_fixed_songs(&data, &owned, &live, [1, 2, 3, 4, 5], [None; 5], &targets).unwrap();
    for target in &targets {
        let mut one = live.clone();
        one.scenario = Some(target.scenario.clone());
        one.execution = Execution::Live {
            score_id: target.score_id,
            gekisou: false,
            play: ournotes_search::types::PlayPolicy::TheoreticalBest,
        };
        let fixed = evaluate_fixed(&data, &owned, &one, [1, 2, 3, 4, 5], [None; 5]).unwrap();
        let row = ranked.results.iter().find(|row| row.score_id == target.score_id).unwrap();
        assert_eq!(row.evaluation.results, fixed.results);
        assert_eq!(
            row.evaluation.results[0].expected_payoff.as_ref().expect("exact deterministic fixture").denominator,
            "120"
        );
    }
}

#[test]
fn song_ranking_validates_zero_budget_inputs() {
    use ournotes_search::{
        auxiliary::{SongTarget, rank_fixed_songs},
        types::Scene,
    };
    use ournotes_sim::Error;
    let s = synthetic_master(5, 0, 5);
    let data = DeckData::from_json(&data_document(&s, 5, 0, 5).to_string()).unwrap();
    let owned = Roster::from_json(&roster_document(5, 0, 5).to_string()).unwrap();
    let targets = [SongTarget { score_id: SCORE_ID, scenario: Scene::Free { music_id: 10 } }];
    let mut r = fixed_request(json!({"kind":"power","musicId":10,"eventParameter":false}), json!({"kind":"power"}));
    r.limits.time_limit_ms = Some(0);
    assert!(matches!(rank_fixed_songs(&data, &owned, &r, [1, 2, 3, 4, 5], [None; 5], &targets), Err(Error::Input(_))));
    let mut r = fixed_request(json!({"kind":"skip","scoreId":SCORE_ID}), json!({"kind":"score"}));
    r.limits.time_limit_ms = Some(0);
    assert!(matches!(rank_fixed_songs(&data, &owned, &r, [1, 2, 3, 4, 4], [None; 5], &targets), Err(Error::Input(_))));
    r.format = "wrong-format".into();
    assert!(matches!(rank_fixed_songs(&data, &owned, &r, [1, 2, 3, 4, 5], [None; 5], &targets), Err(Error::Input(_))));
}

/// A LUCK mission draws its success at random, outside the lottery-free uniform member-order target.
#[test]
fn luck_missions_use_certified_intervals_and_free_live_stays_exact() {
    use ournotes_search::{engine, search::Completion};
    let mut synth = synthetic_master(5, 2, 5);
    set_column(&mut synth, "MasterLiveMusic", &mut |r| {
        r["_gekisouMission1"] = json!(2);
        r["_gekisouMission2"] = json!(3);
        r["_gekisouMission3"] = json!(1);
    });
    let mut document = data_document(&synth, 5, 2, 5);
    document["charts"][0]["fevers"] = json!({"startMs":[150],"endMs":[400]});
    let data = DeckData::from_json(&document.to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 2, 5).to_string()).unwrap();
    let mut req = joint_request("mission", true, json!({"kind":"score"}));
    let result = engine::recommend(&data, &roster, &req).unwrap();
    assert!(matches!(result.completion, Completion::Complete | Completion::RefinementRequired));
    assert!(!result.results.is_empty());
    assert!(result.results.iter().all(|r| r.score_interval.is_some() && r.payoff_interval.is_some()));
    if result.completion == Completion::Complete {
        assert!(result.results.iter().all(|r| r.rank_certified == Some(true)));
    } else {
        assert!(result.results.iter().any(|r| r.rank_certified == Some(false)));
    }
    req.execution = ournotes_search::types::Execution::Live {
        score_id: SCORE_ID,
        gekisou: false,
        play: ournotes_search::types::PlayPolicy::TheoreticalBest,
    };
    req.scenario = Some(ournotes_search::types::Scene::Free { music_id: 10 });
    let free = engine::recommend(&data, &roster, &req).unwrap();
    assert_eq!(free.completion, Completion::Complete);
    assert!(free.results.iter().all(|r| r.expected_payoff.is_some() && r.score_interval.is_none()));
}

#[test]
fn resolved_owned_facts_use_all_three_shared_entrypoints_without_leaking_defaults() {
    use ournotes_search::{
        auxiliary::{SongTarget, evaluate_fixed},
        owned_snapshot::{GoalDependencies, OwnedSnapshot},
        types::Scene,
    };
    use ournotes_sim::Error;
    let mut s = synthetic_master(6, 0, 6);
    extend_table(
        &mut s,
        "MasterMemberCardLevelLimit",
        (1..=5)
            .flat_map(|rarity| {
                (1..=5).map(
                    move |awake| json!({"_id":rarity*10+awake,"_rarity":rarity,"_awakeCount":awake,"_limitLevel":100}),
                )
            })
            .collect(),
    );
    let document = data_document(&s, 6, 0, 6);
    let data = DeckData::from_json(&document.to_string()).unwrap();
    let mut member_facts = roster_document(6, 0, 6)["members"].as_array().unwrap().clone();
    member_facts.retain(|row| row["id"] != 6);
    for row in &mut member_facts {
        row.as_object_mut().unwrap().remove("liveSkillLevel");
        row.as_object_mut().unwrap().remove("gekisouSkillLevel");
    }
    let value = json!({
        "format":"ournotes.owned-snapshot/1","datasetId":"synthetic-shared","revision":"owned-r1",
        "ownedFacts":{"memberIds":[1,2,3,4,5,6],"snapIds":[],"memberCoverage":"complete","snapCoverage":"complete"},
        "eligible":{"members":member_facts,"snaps":[]},
        "player":{"characterRanks":{"coverage":"complete","values":(1..=6).map(|id|json!({"id":id,"value":10})).collect::<Vec<_>>()},
            "characterTotalRank":null,"vipRank":3,"bandItems":[],"memory":{"musicRanks":[],"unlockedMembers":[],"unlockedSnaps":[]},"eventIds":[EVENT_ID]},
        "assumptions":[{"path":"player.vipRank","reason":"synthetic fixture"}]
    });
    let snapshot = OwnedSnapshot::from_json(&value.to_string()).unwrap();
    let mut reference = Roster::from_json(&roster_document(6, 0, 6).to_string()).unwrap();
    reference.members.retain(|member| member.id != 6);
    for (goal, execution, metric) in [
        (GoalDependencies::Power, json!({"kind":"power","musicId":10,"eventParameter":false}), json!({"kind":"power"})),
        (GoalDependencies::Skip, json!({"kind":"skip","scoreId":SCORE_ID}), json!({"kind":"score"})),
    ] {
        let resolution = snapshot.resolve_data(&data, "synthetic-shared", goal);
        assert!(resolution.missing.is_empty(), "{:?}", resolution.missing);
        assert!(resolution.errors.is_empty(), "{:?}", resolution.errors);
        let resolved = resolution.resolved.unwrap();
        let r = fixed_request(execution, metric);
        let out = resolved.evaluate_fixed(&data, &r, [1, 2, 3, 4, 5], [None; 5]).unwrap();
        let expected = evaluate_fixed(&data, &reference, &r, [1, 2, 3, 4, 5], [None; 5]).unwrap();
        assert_eq!(out.results, expected.results);
        let scope = &out.resolved_context["ownedSnapshot"];
        assert_eq!(scope["revision"], "owned-r1");
        assert_eq!(scope["eligibleCoversDeclaredOwned"], false);
        assert_eq!(scope["totalRankOrigin"], "derivedCompleteRanks");
        assert_eq!(scope["assumptions"][0]["reason"], "synthetic fixture");
        assert!(resolved.snapshot().eligible.members[0].live_skill_level.is_none());
        assert!(resolved.snapshot().player.character_total_rank.is_none());
        assert!(resolved.snapshot().owned_facts.member_ids.contains(&6));
        let searched = resolved.recommend(&data, &r).unwrap();
        assert_eq!(searched.resolved_context["ownedSnapshot"], *scope);
        assert!(searched.results.iter().all(|result| !result.members.contains(&6)));
        assert!(resolved.evaluate_fixed(&data, &r, [1, 2, 3, 4, 6], [None; 5]).is_err());
        let foreign = DeckData::from_json(&document.to_string()).unwrap();
        assert!(matches!(resolved.evaluate_fixed(&foreign, &r, [1, 2, 3, 4, 5], [None; 5]), Err(Error::Input(_))));
        let mut changed_events = r.clone();
        changed_events.context = Some(serde_json::from_value(json!({"powerSnapshot":{"eventIds":[]}})).unwrap());
        assert!(matches!(
            resolved.evaluate_fixed(&data, &changed_events, [1, 2, 3, 4, 5], [None; 5]),
            Err(Error::Input(_))
        ));
        let live = fixed_request(
            json!({"kind":"live","scoreId":SCORE_ID,"gekisou":false,"play":{"kind":"theoreticalBest"}}),
            json!({"kind":"score"}),
        );
        assert!(matches!(resolved.evaluate_fixed(&data, &live, [1, 2, 3, 4, 5], [None; 5]), Err(Error::Input(_))));
        if goal == GoalDependencies::Skip {
            let targets = [SongTarget { score_id: SCORE_ID, scenario: Scene::Free { music_id: 10 } }];
            let ranked = resolved.rank_fixed_songs(&data, &r, [1, 2, 3, 4, 5], [None; 5], &targets).unwrap();
            assert_eq!(ranked.results[0].evaluation.results, out.results);
            assert_eq!(ranked.owned_snapshot_scope.as_ref().unwrap(), scope);
            let mut zero = r;
            zero.limits.time_limit_ms = Some(0);
            let empty = resolved.rank_fixed_songs(&data, &zero, [1, 2, 3, 4, 5], [None; 5], &targets).unwrap();
            assert!(empty.results.is_empty());
            assert_eq!(empty.owned_snapshot_scope.unwrap()["revision"], "owned-r1");
        }
    }
}

/// The owned snapshot stating exactly the facts of `roster_document`.
fn snapshot_document(dataset_id: &str, members: i64, snaps: i64, characters: i64) -> Value {
    let roster = roster_document(members, snaps, characters);
    json!({
        "format":"ournotes.owned-snapshot/1","datasetId":dataset_id,"revision":"r1",
        "ownedFacts":{"memberIds":(1..=members).collect::<Vec<_>>(),"snapIds":(1..=snaps).collect::<Vec<_>>(),
            "memberCoverage":"complete","snapCoverage":"complete"},
        "eligible":{"members":roster["members"],"snaps":roster["snaps"]},
        "player":{"characterRanks":{"coverage":"complete",
                "values":(1..=characters).map(|id|json!({"id":id,"value":10})).collect::<Vec<_>>()},
            "characterTotalRank":null,"vipRank":3,"bandItems":[],
            "memory":{"musicRanks":[],"unlockedMembers":[],"unlockedSnaps":[]},"eventIds":[EVENT_ID]},
        "assumptions":[]
    })
}

#[test]
fn snapshot_recommendation_matches_the_same_roster_and_locates_every_input_problem() {
    use ournotes_search::engine::{SnapshotStatus, recommend, recommend_snapshot};
    let mut s = synthetic_master(6, 2, 5);
    extend_table(
        &mut s,
        "MasterMemberCardLevelLimit",
        (1..=5)
            .flat_map(|rarity| {
                (1..=5).map(
                    move |awake| json!({"_id":rarity*10+awake,"_rarity":rarity,"_awakeCount":awake,"_limitLevel":100}),
                )
            })
            .collect(),
    );
    let data = DeckData::from_json(&data_document(&s, 6, 2, 5).to_string()).unwrap();
    let id = data.sha256.clone().unwrap();
    assert!(id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let snapshot = snapshot_document(&id, 6, 2, 5);
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            let request = joint_request_json(mode, gekisou, metric.clone()).to_string();
            let answer = recommend_snapshot(&data, &snapshot.to_string(), &request, None);
            assert_eq!(
                answer.status,
                SnapshotStatus::Ok,
                "{mode} {metric:?}: {:?} {:?}",
                answer.missing,
                answer.errors
            );
            assert_eq!(answer.dataset_id.as_deref(), Some(id.as_str()));
            let result = answer.result.unwrap();
            let expected = recommend(&data, &roster, &joint_request(mode, gekisou, metric.clone())).unwrap();
            assert_eq!(result.results, expected.results, "{mode} {metric:?}");
            assert_eq!(result.resolved_context["ownedSnapshot"]["revision"], "r1");
        }
    }

    let gekisou = joint_request_json("mission", true, json!({"kind":"score"})).to_string();
    let normal = joint_request_json("free", false, json!({"kind":"score"})).to_string();
    let mut unknown = snapshot.clone();
    unknown["eligible"]["members"][3]["gekisouSkillLevel"] = Value::Null;
    let answer = recommend_snapshot(&data, &unknown.to_string(), &gekisou, None);
    assert_eq!(answer.status, SnapshotStatus::Incomplete);
    assert!(answer.result.is_none() && answer.errors.is_empty());
    let missing: Vec<_> = answer.missing.iter().map(|i| (i.path.as_str(), i.code.as_str())).collect();
    assert_eq!(missing, [("eligible.members[4].gekisouSkillLevel", "missing")]);
    let wire = serde_json::to_value(&answer).unwrap();
    assert_eq!(wire["format"], "ournotes-deck.snapshot-recommendation/1");
    assert_eq!(wire["status"], "incomplete");
    assert_eq!(wire["result"], Value::Null);
    // Normal Live does not read Gekisou levels.
    assert_eq!(recommend_snapshot(&data, &unknown.to_string(), &normal, None).status, SnapshotStatus::Ok);

    let mut foreign = snapshot.clone();
    foreign["datasetId"] = json!("0".repeat(64));
    let answer = recommend_snapshot(&data, &foreign.to_string(), &normal, None);
    assert_eq!(answer.status, SnapshotStatus::Invalid);
    assert!(answer.errors.iter().any(|i| i.path == "datasetId" && i.code == "dataset_mismatch"));

    let answer = recommend_snapshot(&data, "{", "{}", None);
    assert_eq!(answer.status, SnapshotStatus::Invalid);
    let errors: Vec<_> = answer.errors.iter().map(|i| (i.path.as_str(), i.code.as_str())).collect();
    assert_eq!(errors, [("request", "parse"), ("snapshot", "parse")]);
}

/// Telemetry without its millisecond fields, which are the only ones allowed to differ between two runs.
fn untimed(outcome: &ournotes_search::types::RecommendationOutcome) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.retain(|key, _| !key.ends_with("Ms"));
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut telemetry = serde_json::to_value(&outcome.telemetry).unwrap();
    strip(&mut telemetry);
    // the peak memory is the process's
    telemetry.as_object_mut().unwrap().remove("memory");
    telemetry
}

#[test]
fn progress_reports_exact_decks_and_leave_the_search_unchanged() {
    use ournotes_search::{
        auxiliary::evaluate_fixed,
        engine::{Progress, recommend, recommend_with_progress},
        search::Completion,
        types::RecommendationOutcome,
    };
    use std::time::Duration;
    let synth = synthetic_master(6, 2, 5);
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap();
    let numerator = |out: &RecommendationOutcome| {
        out.results.first().map(|d| {
            d.expected_payoff.as_ref().expect("exact deterministic fixture").numerator.parse::<i128>().unwrap()
        })
    };
    for (mode, gekisou) in [("free", false), ("mission", true)] {
        for metric in [json!({"kind":"score"}), json!({"kind":"clientEventPoints","eventId":EVENT_ID})] {
            let case = format!("{mode} {metric:?}");
            let request = joint_request(mode, gekisou, metric);
            let plain = recommend(&data, &roster, &request).unwrap();
            let mut reports = Vec::new();
            let mut report = |out: &RecommendationOutcome| reports.push(out.clone());
            let progress = Progress { interval: Duration::ZERO, report: &mut report };
            let hooked = recommend_with_progress(&data, &roster, &request, progress).unwrap();
            assert_eq!(hooked.completion, Completion::Complete, "{case}");
            assert_eq!(hooked.results, plain.results, "{case}");
            assert_eq!(untimed(&hooked), untimed(&plain), "{case}");
            assert!(!reports.is_empty(), "{case}");
            assert_eq!(reports.last().unwrap().results, hooked.results, "{case}");
            let mut decks = BTreeMap::new();
            for (before, after) in reports.iter().zip(&reports[1..]) {
                assert!(before.telemetry.nodes <= after.telemetry.nodes, "{case}");
                assert!(numerator(before) <= numerator(after), "{case}");
            }
            for r in &reports {
                assert_eq!(r.completion, Completion::TimedOut, "{case}");
                assert_eq!(r.resolved_context, plain.resolved_context, "{case}");
                assert!(r.telemetry.nodes <= hooked.telemetry.nodes, "{case}");
                assert!(!r.telemetry.proof.complete && r.telemetry.proof.upper_bound.is_none(), "{case}");
                for d in &r.results {
                    decks.insert((d.members, d.snaps), d.expected_payoff.clone());
                }
            }
            // Every reported deck is exactly evaluated.
            for ((members, snaps), payoff) in decks {
                let fixed = evaluate_fixed(&data, &roster, &request, members, snaps).unwrap();
                assert_eq!(fixed.results[0].expected_payoff, payoff, "{case} {members:?} {snaps:?}");
            }
        }
    }
    // The power search reports exactly evaluated decks too and ends with the plain answer.
    let power = fixed_request(json!({"kind":"power","musicId":10,"eventParameter":false}), json!({"kind":"power"}));
    let mut reports = Vec::new();
    let mut report = |out: &RecommendationOutcome| reports.push(out.results.clone());
    let progress = Progress { interval: Duration::ZERO, report: &mut report };
    let out = recommend_with_progress(&data, &roster, &power, progress).unwrap();
    assert_eq!(out.results, recommend(&data, &roster, &power).unwrap().results);
    for deck in reports.iter().flatten() {
        let fixed = evaluate_fixed(&data, &roster, &power, deck.members, deck.snaps).unwrap();
        assert_eq!(fixed.results[0].expected_payoff, deck.expected_payoff, "{:?}", deck.members);
    }
}

fn account_recommendation_data() -> Value {
    // Six distinct characters give five feasible member sets with the fixed leader, so even the
    // canonical-member-set power route can exercise a full K=5 (five cards would have only one set).
    let mut master = synthetic_master(6, 1, 6);
    set_column(&mut master, "MasterCharacterRank", &mut |row| {
        row["_exp"] = json!((row["_rank"].as_i64().unwrap() - 1) * 100)
    });
    replace_table(&mut master, "MasterVip", json!([{"_id":1,"_vipRank":1}]));
    replace_table(
        &mut master,
        "MasterMemberCardLevelLimit",
        Value::Array(
            (1..=5)
                .flat_map(|rarity| {
                    (1..=5).map(move |awake| {
        json!({"_id":rarity*10+awake,"_rarity":rarity,"_awakeCount":awake,"_limitLevel":40})
    })
                })
                .collect(),
        ),
    );
    let mut document = data_document(&master, 6, 1, 6);
    document["provenance"]["region"] = json!("jp");
    document
}

fn account_recommendation_fixture() -> (DeckData, Value) {
    let data = DeckData::from_json(&account_recommendation_data().to_string()).unwrap();
    let account = json!({
        "format":"ournotes.account/1", "datasetId":data.sha256, "server":"jp", "revision":"made-up-account",
        "coverage":{
            "_player._memberCards":"complete", "_player._supportCards":"complete", "_player._characters":"complete",
            "_player._bandItems":"complete", "_player._memory._musicGroups":"complete",
            "_player._memory._members":"complete", "_player._memory._supports":"complete"
        }, "assumptions":[], "declared":{"_vip":{"_rank":1}}, "account":{"_player":{
            "_name":"PRIVATE_NAME_SENTINEL", "_accountid":9007199254740993_i64, "_profileId":9223372036854775806_i64,
            "_memberCards":(1..=6).map(|id| json!({"_masterId":id,"_exp":0,"_awakeCount":1,"_rank":1,
                "_liveSkillLevel":1,"_performanceSkillLevel":1})).collect::<Vec<_>>(),
            "_supportCards":[{"_masterId":1,"_exp":0,"_rank":1}], "_characters":[], "_bandItems":[],
            "_memory":{"_musicGroups":[],"_members":[],"_supports":[]}
        }}
    });
    (data, account)
}

fn account_request(goal: Value) -> Value {
    json!({"format":"ournotes-deck.recommendation-request/2", "goal":goal,
        "k":5, "constraints":{"leader":3}, "limits":{"timeLimitMs":null}})
}

#[test]
fn account_complete_play_supports_life_and_rejects_missing_or_ambiguous_streams() {
    use ournotes_search::{engine, recommendation::Status};
    use ournotes_sim::live::model::JudgementStream;
    let (data, account) = account_recommendation_fixture();
    let mut stream = JudgementStream::theoretical_best(&data.chart(SCORE_ID).unwrap());
    for (i, row) in stream.judged.iter_mut().enumerate() {
        if i % 3 == 0 {
            row[2] = 1;
        } // NoteSimulateJudgement::Miss, before any skill conversion.
    }
    let mut request = account_request(json!({"kind":"freeLive","musicId":10,"difficulty":"expert",
        "play":{"kind":"stream","stream":stream}}));
    request["metric"] = json!({"kind":"scoreAndLife","threshold":1,"minFinalLife":1});
    let result = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(result.status, Status::Ok), "{:?}", result.errors);
    let result = serde_json::to_value(result).unwrap();
    assert_eq!(result["result"]["optimality"]["proven"], true);
    assert_eq!(result["result"]["teams"].as_array().unwrap().len(), 5);
    assert!(result["result"]["goal"]["play"]["misses"].as_u64().unwrap() > 0);
    assert!(result["result"]["goal"].get("accuracy").is_none());
    assert!(result["result"]["goal"]["play"].get("stream").is_none(), "echo summaries do not copy a full private play");
    let check = |changed: Value, path: &str| {
        let answer = engine::recommend_account(&data, &account.to_string(), &changed.to_string(), None);
        assert!(matches!(answer.status, Status::Invalid), "{:?}", answer.errors);
        assert!(answer.result.is_none());
        assert!(answer.errors.iter().any(|e| e.path == path), "expected {path}: {:?}", answer.errors);
    };
    let mut changed = request.clone();
    changed["goal"].as_object_mut().unwrap().remove("play");
    check(changed, "goal.play");
    let mut changed = request.clone();
    changed["goal"]["accuracy"] = json!({"greatFraction":0.1});
    check(changed, "goal.accuracy");
    let mut changed = request.clone();
    changed["goal"]["play"] = json!({"kind":"stream"});
    check(changed, "goal.play.stream");
    let mut changed = request.clone();
    changed["goal"]["play"]["stream"]["judged"].as_array_mut().unwrap().pop();
    check(changed, "goal.play.stream");
    let mut changed = request.clone();
    let row = changed["goal"]["play"]["stream"]["judged"][0].clone();
    changed["goal"]["play"]["stream"]["judged"].as_array_mut().unwrap().push(row);
    check(changed, "goal.play.stream");
    let mut changed = request.clone();
    changed["goal"]["play"]["stream"]["judged"][0][2] = json!(6);
    check(changed, "goal.play.stream");
    let mut changed = request;
    changed["goal"]["play"]["stream"]["unexpected"] = json!(true);
    check(changed, "goal.play.stream");
}

#[test]
fn typed_skip_items_rank_five_canonical_leaders_before_account_projection() {
    use ournotes_search::{auxiliary::evaluate_built, engine, handler::build_card_pool, search::Completion};
    let data = DeckData::from_json(&account_recommendation_data().to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 0, 5).to_string()).unwrap();
    for scenario in [json!({"kind":"free","musicId":10}), json!({"kind":"challenge","musicId":70})] {
        let mut context = context_document(true, false, false);
        context["eventPayoff"]["consumedCount"] = json!(if scenario["kind"] == "challenge" { 201 } else { 1 });
        let mut request: ournotes_search::types::RecommendationRequest = serde_json::from_value(json!({
            "format":"ournotes-deck.search-request/1",
            "execution":{"kind":"skip","scoreId":SCORE_ID},
            "scenario":scenario,"context":context,
            "metric":{"kind":"rankedEventItems","eventId":EVENT_ID,"resourceType":11,"resourceId":9},
            "k":5,"constraints":{"noSnaps":true},"strategy":{"kind":"branchAndBound"},
            "limits":{"timeLimitMs":null,"maxCandidates":null,"cacheEntries":0}
        }))
        .unwrap();
        let built = build_card_pool(&data, &roster, &request).unwrap();
        let mut expected = Vec::new();
        for leader in 1..=5 {
            let others: Vec<_> = (1..=5).filter(|&id| id != leader).collect();
            let members = [others[0], others[1], leader, others[2], others[3]];
            let evaluated = evaluate_built(&built, members, [None; 5]).unwrap();
            expected.push(evaluated.results.into_iter().next().unwrap());
        }
        let payoff = |v: &ournotes_search::types::RecommendedDeck| {
            let exact = v.expected_payoff.as_ref().unwrap();
            assert_eq!(exact.denominator, "1");
            exact.numerator.parse::<i128>().unwrap()
        };
        expected.sort_by(|a, b| payoff(b).cmp(&payoff(a)).then(b.power.cmp(&a.power)).then(a.members.cmp(&b.members)));
        for strategy in [Strategy::BranchAndBound, Strategy::Exhaustive] {
            request.strategy = strategy;
            let actual = engine::recommend(&data, &roster, &request).unwrap();
            assert_eq!(actual.completion, Completion::Complete);
            assert_eq!(actual.result_identity, "team");
            assert_eq!(actual.results, expected);
            assert_eq!(actual.results.iter().map(|team| team.members[2]).collect::<BTreeSet<_>>().len(), 5);
        }
    }
}

#[test]
fn account_challenge_skip_names_its_scene_and_supports_explicit_rewards() {
    use ournotes_search::{engine, recommendation::Status};
    let (data, account) = account_recommendation_fixture();
    let mut request = account_request(json!({"kind":"skip","challengeMusicId":70,"difficulty":"expert"}));
    request["eventIds"] = json!([EVENT_ID]);
    let score = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(score.status, Status::Ok), "{:?}", score.errors);
    let score = serde_json::to_value(score).unwrap();
    assert_eq!(score["result"]["goal"]["challengeMusicId"], 70);
    assert_eq!(score["result"]["goal"]["musicId"], 10);
    assert_eq!(score["result"]["goal"]["scoreId"], SCORE_ID);
    assert_eq!(score["result"]["optimality"]["proven"], true);
    request["goal"]["musicId"] = json!(10);
    let conflict = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(conflict.status, Status::Invalid));
    assert!(conflict.errors.iter().any(|e| e.path == "goal.challengeMusicId"));
    request["goal"].as_object_mut().unwrap().remove("musicId");
    request["eventContext"] = json!({
        "resultClock":{"kind":"skip","serverNowJstTicks":150},
        "eventWindows":[{"eventId":EVENT_ID,"startJstTicks":100,"endJstTicks":200}],
        "rewardProjection":true
    });
    for kind in ["eventPoints", "eventItems"] {
        request["metric"] = json!({"kind":kind,"eventId":EVENT_ID,"consumption":201});
        if kind == "eventItems" {
            request["metric"]["resourceType"] = json!(11);
            request["metric"]["resourceId"] = json!(9);
        }
        let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(answer.status, Status::Ok), "{kind}: {:?}", answer.errors);
        let answer = serde_json::to_value(answer).unwrap();
        assert_eq!(answer["result"]["optimality"]["proven"], true);
        assert_eq!(answer["result"]["teams"].as_array().unwrap().len(), 5, "{kind}: canonical K=5 before the facade");
        assert!(answer["result"]["teams"][0]["value"]["payoff"]["score"].as_f64().unwrap() > 0.0);
    }
    request["metric"] = json!({"kind":"challengePoints","eventId":EVENT_ID,"consumption":201});
    let invalid = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(invalid.status, Status::Invalid));
    assert!(invalid.errors.iter().any(|e| e.path == "metric.kind"));
}

#[test]
fn account_challenge_point_metric_keeps_per_order_payoffs_and_requires_context() {
    use ournotes_search::{engine, recommendation::Status};
    let (data, account) = account_recommendation_fixture();
    for kind in ["freeLive", "skip"] {
        let mut request = account_request(json!({"kind":kind,"musicId":10,"difficulty":"expert"}));
        request["metric"] = json!({"kind":"challengePoints","eventId":EVENT_ID,"consumption":1});
        let missing = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(missing.status, Status::Invalid));
        assert!(missing.errors.iter().any(|e| e.path == "eventContext"));
        request["eventContext"] = json!({
            "resultClock":{"kind":if kind == "skip" {"skip"} else {"played"},"serverNowJstTicks":150},
            "eventWindows":[{"eventId":EVENT_ID,"startJstTicks":100,"endJstTicks":200}],
            "localEvents":[{"eventId":EVENT_ID,"points":0,"challengePoints":i32::MAX,"added":[]}]
        });
        let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(answer.status, Status::Ok), "{:?}", answer.errors);
        let value = serde_json::to_value(answer).unwrap();
        assert_eq!(value["result"]["metric"]["kind"], "challengePoints");
        assert_eq!(value["result"]["optimality"]["proven"], true);
        for team in value["result"]["teams"].as_array().unwrap() {
            assert!(team["value"]["payoff"]["score"].as_f64().unwrap() > 0.0);
            if kind == "freeLive" {
                let payoffs = team["orders"]["payoffValues"].as_array().unwrap();
                assert_eq!(payoffs.len(), 120);
                let sum: i128 = payoffs.iter().map(|p| i128::from(p.as_i64().unwrap())).sum();
                assert_eq!(
                    team["value"]["payoff"]["exact"]["numerator"].as_str().unwrap().parse::<i128>().unwrap(),
                    sum
                );
                assert_eq!(team["value"]["payoff"]["exact"]["denominator"], "120");
            }
        }
        request["eventContext"]["rewardProjection"] = json!(true);
        let conflict = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(conflict.status, Status::Invalid));
        assert!(conflict.errors.iter().any(|e| e.path == "eventContext.localEvents"));
        request["eventContext"].as_object_mut().unwrap().remove("localEvents");
        let projected = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(projected.status, Status::Ok), "{:?}", projected.errors);
        let projected = serde_json::to_value(projected).unwrap();
        assert_eq!(projected["result"]["metric"]["rewardProjection"], true);
        assert_eq!(projected["result"]["teams"], value["result"]["teams"]);
    }
}

#[test]
fn account_recommendation_power_and_free_live_use_real_search_and_preserve_privacy() {
    use ournotes_search::{engine, recommendation::Status};
    for goal in [
        json!({"kind":"power"}),
        json!({"kind":"freeLive","musicId":10,"difficulty":"expert","accuracy":{"greatFraction":0.2}}),
    ] {
        let (data, account) = account_recommendation_fixture();
        let request = account_request(goal.clone());
        let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(answer.status, Status::Ok), "{:?}", answer.errors);
        assert_eq!(answer.format, "ournotes-deck.account-recommendation/1");
        assert_eq!(answer.dataset_id, data.sha256);
        assert!(answer.is_final);
        let result = answer.result.as_ref().unwrap();
        assert!(result.optimality.proven);
        assert_eq!(result.teams.len(), 5, "the transport must preserve the complete K=5");
        assert!(
            result.teams.iter().any(|team| team.layout.snaps.iter().any(Option::is_some)),
            "the corpus must exercise nonempty Snap bindings"
        );
        for team in &result.teams {
            assert_eq!(team.leader.member, 3);
            assert_eq!(team.layout.members[2], 3);
            assert!(team.others.windows(2).all(|w| w[0].member < w[1].member));
            if goal["kind"] == "freeLive" {
                let orders = team.orders.as_ref().unwrap();
                assert_eq!(orders.count, 120);
                assert_eq!(orders.values.len(), 120);
                let sum: i128 = orders.values.iter().map(|v| i128::from(*v)).sum();
                let value = team.value.as_ref().unwrap();
                let exact = value.exact.as_ref().expect("deterministic order law has an exact value");
                assert_eq!(exact.numerator.parse::<i128>().unwrap(), sum);
                assert_eq!(exact.denominator.parse::<u128>().unwrap(), 120);
            }
        }
        let text = serde_json::to_string(&answer).unwrap();
        for private in ["PRIVATE_NAME_SENTINEL", "9007199254740993", "9223372036854775806"] {
            assert!(!text.contains(private));
        }
    }
}

#[test]
fn account_answers_distinguish_missing_invalid_and_unsupported_without_zero_filling() {
    use ournotes_search::{engine, recommendation::Status};
    let (mut data, mut account) = account_recommendation_fixture();
    let request = account_request(json!({"kind":"power"}));
    account["declared"] = Value::Null;
    let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(answer.status, Status::Incomplete));
    assert!(answer.missing.iter().any(|i| i.path == "declared._vip._rank"));
    account["declared"] = json!({"_vip":{"_rank":1}});
    account["server"] = json!("intl");
    let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(answer.status, Status::Invalid));
    assert!(answer.errors.iter().any(|i| i.code == "server_mismatch"));
    account["server"] = json!("jp");
    let mut battle = account_request(json!({"kind":"battleLive","musicId":10,"difficulty":"expert","rank":2}));
    battle["room"] = json!({"players":5,"othersAverageScore":null});
    let answer = engine::recommend_account(&data, &account.to_string(), &battle.to_string(), None);
    assert!(matches!(answer.status, Status::Failed));
    assert!(answer.errors.iter().any(|i| i.code == "unsupported"));
    assert!(!answer.errors.iter().any(|i| i.code == "parse"));
    data.master.character_ranks[0].exp = None;
    let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(
        answer.errors.iter().any(|i| i.code == "unsupported_master" && i.message.contains("MasterCharacterRank._exp"))
    );
}

#[test]
fn account_transport_rejects_missing_and_null_required_card_facts() {
    use ournotes_search::{engine, recommendation::Status};
    let (data, original) = account_recommendation_fixture();
    for (list, field, live) in [
        ("_memberCards", "_exp", false),
        ("_memberCards", "_awakeCount", false),
        ("_memberCards", "_rank", false),
        ("_memberCards", "_liveSkillLevel", true),
        ("_supportCards", "_exp", false),
        ("_supportCards", "_rank", false),
    ] {
        let request = account_request(if live {
            json!({"kind":"freeLive","musicId":10,"difficulty":"expert"})
        } else {
            json!({"kind":"power"})
        });
        for absent in [true, false] {
            let mut account = original.clone();
            let card = account["account"]["_player"][list][0].as_object_mut().unwrap();
            if absent {
                card.remove(field);
            } else {
                card.insert(field.into(), Value::Null);
            }
            let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
            let path = format!("_player.{list}[0].{field}");
            assert!(matches!(answer.status, Status::Incomplete), "{path}, absent={absent}: {:?}", answer.errors);
            assert!(answer.is_final && answer.result.is_none() && answer.errors.is_empty());
            assert!(answer.missing.iter().any(|issue| issue.path == path && issue.code == "missing"));
            let text = serde_json::to_string(&answer).unwrap();
            for private in ["PRIVATE_NAME_SENTINEL", "9007199254740993", "9223372036854775806"] {
                assert!(!text.contains(private));
            }
        }
    }
    for field in ["format", "datasetId", "server", "revision", "coverage", "assumptions", "account"] {
        let mut account = original.clone();
        account.as_object_mut().unwrap().remove(field);
        let request = account_request(json!({"kind":"power"}));
        let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(answer.status, Status::Invalid), "missing envelope field {field}");
        assert!(answer.is_final && answer.result.is_none());
        assert!(answer.errors.iter().any(|issue| issue.path == "account" && issue.code == "parse"));
    }
}

#[test]
fn account_progress_is_a_whole_answer_and_does_not_change_final_results() {
    use ournotes_search::engine::{self, AnswerProgress};
    let (data, account) = account_recommendation_fixture();
    let request = account_request(json!({"kind":"freeLive","musicId":10,"difficulty":"expert"}));
    let baseline = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    let mut reports = Vec::new();
    let mut collect = |answer: &engine::Answer| reports.push(serde_json::to_value(answer).unwrap());
    let actual = engine::recommend_account(
        &data,
        &account.to_string(),
        &request.to_string(),
        Some(AnswerProgress { interval: std::time::Duration::ZERO, report: &mut collect }),
    );
    assert!(!reports.is_empty());
    for report in reports {
        assert_eq!(report["format"], "ournotes-deck.account-recommendation/1");
        assert_eq!(report["datasetId"], json!(data.sha256));
        assert_eq!(report["final"], false);
        assert_eq!(report["result"]["optimality"]["proven"], false);
        for team in report["result"]["teams"].as_array().unwrap() {
            assert!(team["orders"].is_null());
        }
        let text = report.to_string();
        for private in ["PRIVATE_NAME_SENTINEL", "9007199254740993", "9223372036854775806"] {
            assert!(!text.contains(private));
        }
    }
    assert!(actual.is_final && actual.result.as_ref().unwrap().optimality.proven);
    assert_eq!(actual.result.as_ref().unwrap().teams.len(), 5);
    assert_eq!(
        serde_json::to_value(&baseline.result.as_ref().unwrap().teams).unwrap(),
        serde_json::to_value(&actual.result.as_ref().unwrap().teams).unwrap()
    );
}

/// Synthetic native/WASM transport comparison inputs; explicitly requested by the validation harness.
#[test]
#[ignore = "writes a synthetic account transport corpus to OURNOTES_ACCOUNT_CORPUS"]
fn export_account_transport_corpus() {
    use ournotes_search::engine;
    use ournotes_sim::{
        live::model::{JudgementStream, JustRule},
        scenario::Scenario,
    };
    let root = std::env::var("OURNOTES_ACCOUNT_CORPUS").expect("OURNOTES_ACCOUNT_CORPUS");
    let root = Path::new(&root);
    fs::create_dir_all(root).unwrap();
    let (data, account) = account_recommendation_fixture();
    let raw = account_recommendation_data().to_string();
    assert_eq!(DeckData::from_json(&raw).unwrap().sha256, data.sha256);
    fs::write(root.join("data.json"), raw).unwrap();
    fs::write(root.join("account.json"), account.to_string()).unwrap();
    let chart = data.chart(SCORE_ID).unwrap();
    let dc = data.data_chart(SCORE_ID).unwrap();
    let mut requests = Vec::new();
    let mut coverage = Vec::new();
    requests.push(("power".to_string(), account_request(json!({"kind":"power"}))));
    coverage.push(json!({"name":"power","scene":"power","metric":"power","scope":"synthetic"}));
    for (scene, kind, scene_id) in [
        ("free", "freeLive", 10),
        ("mission", "missionLive", 10),
        ("battle", "battleLive", 10),
        ("arena", "arenaLive", 80),
        ("challenge", "challengeLive", 70),
        ("skip", "skip", 10),
        ("challengeSkip", "skip", 70),
    ] {
        let skipped = kind == "skip";
        let challenge = scene == "challenge" || scene == "challengeSkip";
        let gekisou = matches!(scene, "mission" | "battle" | "arena");
        let mut goal = json!({"kind":kind,"difficulty":"expert"});
        goal[if scene == "arena" {
            "arenaMusicId"
        } else if challenge {
            "challengeMusicId"
        } else {
            "musicId"
        }] = json!(scene_id);
        if matches!(scene, "battle" | "arena") {
            goal["rank"] = json!(1);
        }
        for metric in
            ["score", "scoreAtLeast", "cappedScore", "scoreAndLife", "eventPoints", "challengePoints", "eventItems"]
        {
            if skipped && metric == "scoreAndLife" || challenge && metric == "challengePoints" {
                continue;
            }
            let name = match (scene, metric) {
                ("free", "score") => "free".to_string(),
                ("free", "challengePoints") => "challenge-points".to_string(),
                ("free", "scoreAndLife") => "life-stream".to_string(),
                ("challengeSkip", "eventPoints") => "challenge-skip-points".to_string(),
                ("challengeSkip", "eventItems") => "challenge-skip-items".to_string(),
                _ => format!("{scene}-{metric}"),
            };
            let mut request = account_request(goal.clone());
            request["eventIds"] = json!([EVENT_ID]);
            request["metric"] = json!({"kind":metric});
            if matches!(metric, "scoreAtLeast" | "cappedScore") {
                request["metric"]["threshold"] = json!(300000);
            }
            if metric == "scoreAndLife" {
                let mut stream = if gekisou {
                    let scenario = match scene {
                        "battle" => Scenario::Battle(10),
                        "arena" => Scenario::Arena(80),
                        _ => Scenario::Mission(10),
                    };
                    let setup = scenario.resolve(&data.master).unwrap().gekisou_setup(&dc.fevers);
                    JudgementStream::theoretical_best_gekisou(
                        &chart,
                        &dc.judgement_types,
                        &JustRule::new(&data.master, &setup).unwrap(),
                    )
                    .unwrap()
                } else {
                    JudgementStream::theoretical_best(&chart)
                };
                stream.judged[0][2] = 1;
                request["goal"]["play"] = json!({"kind":"stream","stream":stream});
                request["metric"]["threshold"] = json!(1);
                request["metric"]["minFinalLife"] = json!(1);
            }
            if matches!(metric, "eventPoints" | "challengePoints" | "eventItems") {
                request["metric"]["eventId"] = json!(EVENT_ID);
                request["metric"]["consumption"] = json!(if challenge { 201 } else { 1 });
                request["eventContext"] = json!({
                    "resultClock":{"kind":if skipped {"skip"} else {"played"},"serverNowJstTicks":150},
                    "eventWindows":[{"eventId":EVENT_ID,"startJstTicks":100,"endJstTicks":200}]
                });
                if metric == "eventItems" {
                    request["metric"]["resourceType"] = json!(11);
                    request["metric"]["resourceId"] = json!(9);
                }
                request["eventContext"]["rewardProjection"] = json!(true);
                if matches!(scene, "battle" | "arena") {
                    request["room"] = json!({"players":5,"othersAverageScore":null});
                }
            }
            coverage.push(json!({"name":name,"scene":scene,"metric":metric,"scope":"synthetic",
                "arenaSource":if scene=="arena" {Some("synthetic MasterArenaMusic row 80; not an actual published Arena domain")} else {None}}));
            requests.push((name, request));
        }
    }
    let accuracy =
        account_request(json!({"kind":"freeLive","musicId":10,"difficulty":"expert","accuracy":{"greatFraction":0.2}}));
    requests.push(("accuracy".to_string(), accuracy));
    coverage.push(json!({"name":"accuracy","scene":"free","metric":"score","scope":"synthetic","play":"accuracy"}));
    for (name, kind, just) in [("pattern-free-life", "freeLive", 0.0), ("pattern-mission-life", "missionLive", 0.8)] {
        let mut request = account_request(json!({"kind":kind,"musicId":10,"difficulty":"expert",
            "play":{"kind":"pattern","greatFraction":0.2,"justFraction":just,"missEvery":3}}));
        request["metric"] = json!({"kind":"scoreAndLife","threshold":1,"minFinalLife":1});
        requests.push((name.to_string(), request));
        coverage.push(json!({"name":name,"scene":kind,"metric":"scoreAndLife","scope":"synthetic","play":"pattern"}));
    }
    for scene in ["free", "mission", "battle", "arena", "skip"] {
        let base_name =
            if scene == "free" { "challenge-points".to_string() } else { format!("{scene}-challengePoints") };
        let base = requests.iter().find(|(name, _)| name == &base_name).unwrap().1.clone();
        for priority in ["eventPointsFirst", "eventItemsFirst"] {
            let mut request = base.clone();
            request["metric"]["secondaryPriority"] = json!(priority);
            request["metric"]["resourceType"] = json!(11);
            request["metric"]["resourceId"] = json!(9);
            let name = format!("{scene}-cp-{priority}");
            coverage.push(json!({"name":name,"scene":scene,"metric":"challengePoints","secondaryPriority":priority,"scope":"synthetic"}));
            requests.push((name, request));
        }
    }
    assert_eq!(requests.len(), 59, "46 scene/metric pairs, accuracy, two explicit patterns and ten reward priorities");
    let mut names = BTreeSet::new();
    for (name, request) in requests {
        assert!(names.insert(name.clone()), "duplicate corpus identity");
        let priority = request["metric"]["secondaryPriority"].is_string();
        let request = request.to_string();
        let answer = engine::recommend_account(&data, &account.to_string(), &request, None);
        assert!(matches!(answer.status, ournotes_search::recommendation::Status::Ok), "{name}: {:?}", answer.errors);
        let result = answer.result.as_ref().unwrap();
        assert!(answer.is_final && result.optimality.proven, "{name}: unproven");
        if priority {
            assert!(!result.teams.is_empty() && result.teams.len() <= 5, "{name}: maximum-CP layer");
        } else {
            assert_eq!(result.teams.len(), 5, "{name}: complete K=5");
        }
        assert!(result.teams.iter().any(|team| team.layout.snaps.iter().any(Option::is_some)), "{name}: nonempty Snap");
        fs::write(root.join(format!("{name}.request.json")), &request).unwrap();
        fs::write(root.join(format!("{name}.expected.json")), serde_json::to_string(&answer).unwrap()).unwrap();
    }
    fs::write(root.join("cases.json"), serde_json::to_string(&names).unwrap()).unwrap();
    fs::write(root.join("coverage.json"), serde_json::to_string(&json!({"scope":"synthetic formal account transport witnesses, not native-game truth or the real-domain benchmark", "cases":coverage})).unwrap()).unwrap();
}

#[test]
fn composition_power_frontier_keeps_canonical_ties_in_heuristic_layout() {
    use ournotes_search::{engine, search::Completion};
    let mut synth = synthetic_master(5, 2, 5);
    set_column(&mut synth, "MasterMemberCard", &mut |row| {
        row["_cardType"] = json!(1);
        row["_memberCardLevelGroup"] = json!(1);
        row["_leaderSkillID"] = json!(0);
        row["_liveSkillID"] = json!(0);
        row["_gekisouSkillID"] = json!(0);
        // Equal base power makes all Snap edges tie. Member 5's song-tag
        // bonus changes its member-only power and the heuristic layout.
        row["_bestMusicTagIDs"] = if row["_id"] == 5 { json!([1]) } else { json!([]) };
        for key in ["_performancePowerMax", "_technicPowerMax", "_visualPowerMax"] {
            row[key] = json!(5000);
        }
    });
    set_column(&mut synth, "MasterSupportCard", &mut |row| {
        row["_cardType"] = json!(2);
        row["_supportSkillId01"] = json!(0);
        row["_supportSkillId02"] = json!(0);
        for key in ["_performancePowerMax", "_technicPowerMax", "_visualPowerMax"] {
            row[key] = json!(1000);
        }
    });
    replace_table(&mut synth, "MasterEventEffect", json!([]));
    let data = DeckData::from_json(&data_document(&synth, 5, 2, 5).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 2, 5).to_string()).unwrap();
    let mut request = joint_request("free", false, json!({"kind":"cappedScore","threshold":1}));
    request.constraints.leader = Some(1);
    request.k = 12;
    request.strategy = Strategy::Exhaustive;
    let oracle = engine::recommend(&data, &roster, &request).unwrap();
    assert_eq!(oracle.completion, Completion::Complete);
    assert_eq!(oracle.results[0].snaps, [None, None, None, Some(1), Some(2)]);
    request.strategy = Strategy::BranchAndBound;
    for k in [1, 3, 12] {
        request.k = k;
        let actual = engine::recommend(&data, &roster, &request).unwrap();
        assert_eq!(actual.completion, Completion::Complete);
        assert!(actual.telemetry.composition.power_frontier_closed > 0);
        assert_eq!(actual.results, oracle.results[..k], "K={k}");
    }
}

#[path = "fixtures/correctness_matrix.rs"]
mod correctness_matrix;
