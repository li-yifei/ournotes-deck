//! Public Power and monotone Skip targets must rank complete Team identities.
//! The oracle independently enumerates combinations, leaders and injective Snap
//! bindings. Only each fixed deck's value shares the normal evaluator; this does
//! not claim independent certification of the native numerical model.
#[path = "../../ournotes-sim/tests/common/mod.rs"]
mod common;

use common::{Rng, set_column, synth};
use ournotes_search::{
    auxiliary::evaluate_built,
    engine::recommend,
    handler::build_card_pool,
    search::{Completion, Constraints},
    types::{ExitReason, Metric, Optimality, RecommendationRequest, RecommendedDeck, Strategy},
};
use ournotes_sim::{
    cards::Roster,
    data::{DataChart, DeckData},
    live::skip::ChartNote,
};
use serde_json::json;
use std::collections::BTreeSet;

fn fixture(n: i64, snap_bp: &[i64]) -> (DeckData, Roster) {
    let mut s = synth(&mut Rng::new(20261005), n, snap_bp.len() as i64);
    set_column(&mut s, "MasterMemberCard", &mut |r| {
        r["_characterID"] = r["_id"].clone();
        r["_cardType"] = json!(1);
        r["_memberCardLevelGroup"] = json!(1);
        r["_leaderSkillID"] = json!(1);
        r["_bestMusicTagIDs"] = json!([]);
        let p = if r["_id"] == 6 { 100 } else { 1000 };
        for key in ["_performancePowerMax", "_technicPowerMax", "_visualPowerMax"] {
            r[key] = json!(p);
        }
    });
    set_column(&mut s, "MasterSupportCard", &mut |r| {
        r["_cardType"] = json!(2);
        let p = snap_bp[r["_id"].as_i64().unwrap() as usize - 1];
        for key in ["_performancePowerMax", "_technicPowerMax", "_visualPowerMax"] {
            r[key] = json!(p);
        }
    });
    for table in ["MasterMemberCardLevel", "MasterSupportCardLevel"] {
        set_column(&mut s, table, &mut |r| {
            for key in ["_performanceRate", "_technicRate", "_visualRate"] {
                r[key] = json!(10000);
            }
        });
    }
    for table in ["MasterMemberCardAwake", "MasterMemberCardRank"] {
        set_column(&mut s, table, &mut |r| {
            for key in ["_performanceRate", "_technicRate", "_visualRate"] {
                r[key] = json!(0);
            }
        });
    }
    for table in ["MasterCharacterRank", "MasterCharacterTotalRank"] {
        set_column(&mut s, table, &mut |r| r["_bonus"] = json!(0));
    }
    set_column(&mut s, "MasterLeaderSkillEffect", &mut |r| r["_effectValue"] = json!(0));
    set_column(&mut s, "MasterParameter", &mut |r| {
        if matches!(
            r["_id"].as_str(),
            Some("music_type_base_bonus_rate" | "music_tag_base_bonus_rate" | "type_link_base_bonus_rate")
        ) {
            r["_value"] = json!("0");
        }
    });
    // Reverse both input pools: public-ID canonical ordering cannot use their indexes.
    let roster = Roster::from_json(
        &json!({
            "player":{},
            "members":(1..=n).rev().map(|id| json!({"id":id,"level":1,"awake":1,"rank":1})).collect::<Vec<_>>(),
            "snaps":(1..=snap_bp.len()).rev().map(|id| json!({"id":id,"level":1,"rank":1})).collect::<Vec<_>>()
        })
        .to_string(),
    )
    .unwrap();
    let chart = DataChart {
        score_id: 1004,
        asset_key: "synthetic-power-team-identity".into(),
        asset_sha256: "0".repeat(64),
        notes: (1..=6).map(|id| ChartNote { id, time_ms: id * 200, note_type: 1 }).collect(),
        judgement_types: vec![1; 6],
        skill_event_ms: vec![],
        fevers: vec![],
    };
    (DeckData { master: s.master(), charts: vec![chart], provenance: json!({"synthetic":true}), sha256: None }, roster)
}

fn request(metric: Metric, k: usize, constraints: Constraints) -> RecommendationRequest {
    let power = matches!(&metric, Metric::Power);
    let mut r: RecommendationRequest = serde_json::from_value(json!({
        "format":"ournotes-deck.search-request/1",
        "execution":if power { json!({"kind":"power"}) } else { json!({"kind":"skip","scoreId":1004}) },
        "scenario":if power { serde_json::Value::Null } else { json!({"kind":"free","musicId":10}) },
        "context":if power { serde_json::Value::Null } else { json!({
            "powerSnapshot":{"eventIds":[]}, "resultClock":{"execution":"skip","serverNowJstTicks":1}
        }) },
        "metric":metric,"k":k,"strategy":{"kind":"branchAndBound"},
        "limits":{"timeLimitMs":null,"maxCandidates":null,"cacheEntries":0}
    }))
    .unwrap();
    r.constraints = constraints;
    r
}

fn metrics() -> Vec<Metric> {
    vec![Metric::Power, Metric::Score, Metric::ScoreAtLeast { threshold: 1 }, Metric::CappedScore { threshold: 1 }]
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_matches_serial_and_independent_oracle_including_fixed_leader_and_ties() {
    use ournotes_search::parallel::{self, Cancellation};
    let (data, roster) = fixture(6, &[0, 30]);
    let constraints = [
        Constraints::default(),
        Constraints { leader: Some(1), ..Default::default() },
        Constraints {
            leader: Some(2),
            include_members: vec![3],
            exclude_members: vec![6],
            exclude_snaps: vec![1],
            ..Default::default()
        },
        Constraints { no_snaps: true, ..Default::default() },
    ];
    for metric in metrics() {
        for c in &constraints {
            let req = request(metric.clone(), 5, c.clone());
            let serial = recommend(&data, &roster, &req).unwrap();
            assert_eq!(
                serial.results.iter().map(row).collect::<Vec<_>>(),
                oracle(&data, &roster, &req).into_iter().take(req.k).collect::<Vec<_>>()
            );
            for workers in [2, 3].map(|n| n.min(parallel::max_workers())) {
                let out = parallel::recommend(&data, &roster, &req, workers, Cancellation::default()).unwrap();
                assert_eq!(out.completion, Completion::Complete);
                assert_eq!(out.optimality, Optimality::Proven);
                assert_eq!(out.results, serial.results, "workers={workers}, constraints={c:?}");
                let p = out.telemetry.parallel.unwrap();
                assert!(p.workers_used <= workers);
                assert_eq!(p.tasks_finished, p.tasks);
                if workers > 1 && c.exclude_members.is_empty() {
                    assert!(p.tasks > 1);
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_budget_is_global_and_zero_deadline_and_cancellation_keep_unproven() {
    use ournotes_search::parallel::{self, Cancellation};
    let (data, roster) = fixture(7, &[10, 30]);
    for limit in [0, 1, 7] {
        let mut req = request(Metric::Power, 5, Constraints::default());
        req.limits.max_candidates = Some(limit);
        let out =
            parallel::recommend(&data, &roster, &req, 3.min(parallel::max_workers()), Cancellation::default()).unwrap();
        assert!(out.telemetry.leaves.visited <= limit);
        assert!(out.telemetry.parallel.as_ref().unwrap().candidates <= limit);
        assert_eq!(out.completion, Completion::TimedOut);
        assert_eq!(out.exit_reason, ExitReason::CandidateLimit);
        assert!(!out.telemetry.proof.complete);
    }
    let mut req = request(Metric::Power, 5, Constraints::default());
    req.limits.time_limit_ms = Some(0);
    let out =
        parallel::recommend(&data, &roster, &req, 3.min(parallel::max_workers()), Cancellation::default()).unwrap();
    assert!(out.results.is_empty());
    assert_eq!(out.completion, Completion::TimedOut);
    req.limits.time_limit_ms = None;
    let cancel = Cancellation::default();
    cancel.cancel();
    let out = parallel::recommend(&data, &roster, &req, 3.min(parallel::max_workers()), cancel).unwrap();
    assert!(out.results.is_empty());
    assert_eq!(out.completion, Completion::TimedOut);
    assert!(out.telemetry.parallel.unwrap().cancelled);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_normal_live_matches_serial_with_snap_skills() {
    use ournotes_search::parallel::{self, Cancellation};
    let (mut data, roster) = fixture(6, &[10]);
    let mut s = common::synth_snaps(&mut Rng::new(88), 6, 1, &[2000]);
    set_column(&mut s, "MasterMemberCard", &mut |r| r["_characterID"] = r["_id"].clone());
    data.master = s.master();
    let mut req: RecommendationRequest = serde_json::from_value(json!({
        "format":"ournotes-deck.search-request/1",
        "execution":{"kind":"live","scoreId":1004,"gekisou":false,"play":{"kind":"theoreticalBest"}},
        "scenario":{"kind":"free","musicId":10},
        "context":{"powerSnapshot":{"eventIds":[]}},
        "metric":{"kind":"score"},"k":3,"strategy":{"kind":"branchAndBound"},
        "limits":{"timeLimitMs":null,"maxCandidates":null,"cacheEntries":128}
    }))
    .unwrap();
    for k in [1, 3, 100] {
        req.k = k;
        for leader in [None, Some(1)] {
            req.constraints.leader = leader;
            let serial = recommend(&data, &roster, &req).unwrap();
            let out =
                parallel::recommend(&data, &roster, &req, 3.min(parallel::max_workers()), Cancellation::default())
                    .unwrap();
            assert_eq!(out.completion, Completion::Complete);
            assert_eq!(out.results, serial.results);
            // Ordinary Live keeps one traversal and plays each team's orders in parallel.
            let p = out.telemetry.parallel.unwrap();
            assert_eq!(p.tasks, 1);
            assert_eq!(p.simulation_worker_limit, 3.min(parallel::max_workers()));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_can_cancel_an_active_search_and_rejects_invalid_inputs() {
    use ournotes_search::parallel::{self, Cancellation};
    let (data, roster) = fixture(8, &[10, 20, 30]);
    let mut req = request(Metric::Power, 5, Constraints::default());
    req.strategy = Strategy::Exhaustive;
    let cancel = Cancellation::default();
    std::thread::scope(|s| {
        let c = cancel.clone();
        s.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            c.cancel();
        });
        let out = parallel::recommend(&data, &roster, &req, 3.min(parallel::max_workers()), cancel).unwrap();
        assert_eq!(out.completion, Completion::TimedOut);
        assert!(out.telemetry.parallel.unwrap().cancelled);
    });
    assert!(parallel::recommend(&data, &roster, &req, 0, Cancellation::default()).is_err());
    req.limits.time_limit_ms = Some(0);
    req.constraints.include_members = vec![999];
    assert!(
        parallel::recommend(&data, &roster, &req, 3.min(parallel::max_workers()), Cancellation::default()).is_err()
    );
    assert_eq!(parallel::default_workers(), std::thread::available_parallelism().map_or(1, usize::from));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_membership_partitions_preserve_initial_decks_and_all_leaders() {
    use ournotes_search::parallel::{self, Cancellation};
    let (data, roster) = fixture(6, &[0, 30]);
    for metric in metrics() {
        for leader in [None, Some(1)] {
            let mut req = request(metric.clone(), 5, Constraints { leader, ..Default::default() });
            let serial = recommend(&data, &roster, &req).unwrap();
            req.initial_decks = serial
                .results
                .iter()
                .map(|d| ournotes_search::types::DeckInput { members: d.members, snaps: d.snaps })
                .collect();
            let out = parallel::recommend(&data, &roster, &req, parallel::default_workers(), Cancellation::default())
                .unwrap();
            assert_eq!(out.completion, Completion::Complete);
            assert_eq!(out.results, serial.results);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_thread_parameter_range_defaults_and_native_dispatch() {
    use ournotes_search::parallel::{self, Cancellation};
    let (data, roster) = fixture(6, &[0, 30]);
    let req = request(Metric::Power, 5, Constraints::default());
    let maximum = parallel::max_workers();
    assert_eq!(parallel::default_workers(), maximum);
    let expected = recommend(&data, &roster, &req).unwrap();
    for threads in [1, maximum] {
        let actual = parallel::with_native_threads(threads, || recommend(&data, &roster, &req)).unwrap().unwrap();
        assert_eq!(actual.results, expected.results);
        assert_eq!(actual.telemetry.parallel.unwrap().workers, threads);
    }
    assert!(recommend(&data, &roster, &req).unwrap().telemetry.parallel.is_none(), "native scope restored");
    for threads in [0, maximum + 1, usize::MAX] {
        assert!(parallel::recommend(&data, &roster, &req, threads, Cancellation::default()).is_err());
        assert!(
            parallel::with_native_threads(threads, || panic!("invalid count must reject before calling closure"))
                .is_err()
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "manual multicore throughput measurement"]
fn parallel_live_throughput() {
    use ournotes_search::parallel::{self, Cancellation};
    let (mut data, roster) = fixture(8, &[10, 20]);
    let mut s = common::synth_snaps(&mut Rng::new(88), 8, 2, &[2000]);
    set_column(&mut s, "MasterMemberCard", &mut |r| r["_characterID"] = r["_id"].clone());
    data.master = s.master();
    data.charts[0].notes = (1..=64).map(|id| ChartNote { id, time_ms: id * 200, note_type: 1 }).collect();
    data.charts[0].judgement_types = vec![1; 64];
    let req: RecommendationRequest = serde_json::from_value(json!({
        "format":"ournotes-deck.search-request/1",
        "execution":{"kind":"live","scoreId":1004,"gekisou":false,"play":{"kind":"theoreticalBest"}},
        "scenario":{"kind":"free","musicId":10},"context":{"powerSnapshot":{"eventIds":[]}},
        "metric":{"kind":"score"},"k":5,"strategy":{"kind":"exhaustive"},
        "limits":{"timeLimitMs":2000,"cacheEntries":256}
    }))
    .unwrap();
    let mut expected = None;
    for workers in [1, 4, 8].map(|n| n.min(parallel::max_workers())) {
        let out = parallel::recommend(&data, &roster, &req, workers, Cancellation::default()).unwrap();
        assert_eq!(out.completion, Completion::Complete);
        if let Some(expected) = &expected {
            assert_eq!(&out.results, expected);
        } else {
            expected = Some(out.results.clone());
        }
        let p = out.telemetry.parallel.as_ref().unwrap();
        eprintln!(
            "workers={} used={} tasks={} evaluated={} simulations={} elapsed_ms={:.0}",
            workers,
            p.workers_used,
            p.tasks,
            out.telemetry.leaves.evaluated,
            out.telemetry.leaves.simulations,
            out.elapsed_ms
        );
        assert!(p.workers_used <= workers);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    power: i32,
    score: Option<i32>,
    payoff: i128,
}

fn row(deck: &RecommendedDeck) -> Row {
    let exact = |f: &ournotes_search::types::Fraction| {
        assert_eq!(f.denominator, "1");
        f.numerator.parse::<i128>().unwrap()
    };
    Row {
        members: deck.members,
        snaps: deck.snaps,
        power: deck.power,
        score: deck.expected_score.as_ref().map(|s| i32::try_from(exact(s)).unwrap()),
        payoff: exact(deck.expected_payoff.as_ref().expect("deterministic payoff")),
    }
}

// Neither production enumeration/canonicalization nor legacy member-set oracle
// code participates. No matching, bound, cache, proposal or Top-K code is reused.
fn oracle(data: &DeckData, roster: &Roster, req: &RecommendationRequest) -> Vec<Row> {
    let c = &req.constraints;
    let mut ids: Vec<_> = roster.members.iter().map(|m| m.id).filter(|m| !c.exclude_members.contains(m)).collect();
    ids.sort_unstable();
    let mut snaps = vec![None];
    if !c.no_snaps {
        let mut ids: Vec<_> = roster.snaps.iter().map(|s| s.id).filter(|s| !c.exclude_snaps.contains(s)).collect();
        ids.sort_unstable();
        snaps.extend(ids.into_iter().map(Some));
    }
    fn combinations(ids: &[i64], chosen: &mut Vec<i64>, out: &mut Vec<[i64; 5]>) {
        if chosen.len() == 5 {
            out.push(chosen.as_slice().try_into().unwrap());
            return;
        }
        for i in 0..ids.len() {
            chosen.push(ids[i]);
            combinations(&ids[i + 1..], chosen, out);
            chosen.pop();
        }
    }
    let mut sets = Vec::new();
    combinations(&ids, &mut Vec::new(), &mut sets);
    let mut fixed = req.clone();
    fixed.strategy = Strategy::Exhaustive;
    fixed.limits.time_limit_ms = None;
    fixed.limits.max_candidates = None;
    let built = build_card_pool(data, roster, &fixed).unwrap();
    let mut result = Vec::new();
    for chosen in sets {
        let chars: BTreeSet<_> = chosen.iter().map(|&m| data.master.member_card(m).unwrap().character_id).collect();
        if chars.len() != 5 || !c.include_members.iter().all(|m| chosen.contains(m)) {
            continue;
        }
        for lead in chosen {
            if c.leader.is_some_and(|l| l != lead) {
                continue;
            }
            let others: Vec<_> = chosen.iter().copied().filter(|m| *m != lead).collect();
            let members = [others[0], others[1], lead, others[2], others[3]];
            for mut code in 0..snaps.len().pow(5) {
                let mut binding = [None; 5];
                for s in &mut binding {
                    *s = snaps[code % snaps.len()];
                    code /= snaps.len();
                }
                let real: Vec<_> = binding.iter().flatten().copied().collect();
                if real.iter().copied().collect::<BTreeSet<_>>().len() != real.len() {
                    continue;
                }
                let out = evaluate_built(&built, members, binding).unwrap();
                assert_eq!(out.completion, Completion::Complete);
                assert_eq!(out.optimality, Optimality::NotApplicable);
                assert_eq!(out.results.len(), 1);
                let value = row(&out.results[0]);
                assert_eq!((value.members, value.snaps), (members, binding));
                // Pin this fixture's arithmetic independently of the common evaluator.
                let power: i64 = members
                    .iter()
                    .zip(binding)
                    .map(|(&m, s)| {
                        let base = if m == 6 { 100 } else { 1000 };
                        let pct = s.map_or(0, |id| data.master.support_card(id).unwrap().performance_power_max);
                        3 * base + 3 * (base * pct / 10000)
                    })
                    .sum();
                assert_eq!(i64::from(value.power), power);
                let payoff = match req.metric {
                    Metric::Power => i128::from(value.power),
                    Metric::Score => i128::from(value.score.unwrap()),
                    Metric::ScoreAtLeast { threshold } => i128::from(value.score.unwrap() >= threshold),
                    Metric::CappedScore { threshold } => i128::from(value.score.unwrap().min(threshold)),
                    _ => unreachable!(),
                };
                assert_eq!(value.payoff, payoff);
                result.push(value);
            }
        }
    }
    result.sort_by(|a, b| {
        b.payoff.cmp(&a.payoff).then(b.power.cmp(&a.power)).then(a.members.cmp(&b.members)).then(a.snaps.cmp(&b.snaps))
    });
    assert_eq!(result.iter().map(|r| (r.members, r.snaps)).collect::<BTreeSet<_>>().len(), result.len());
    result
}

fn check(data: &DeckData, roster: &Roster, req: &RecommendationRequest, expected: &[Row]) {
    for strategy in [Strategy::BranchAndBound, Strategy::Exhaustive] {
        let mut r = req.clone();
        r.strategy = strategy;
        let out = recommend(data, roster, &r).unwrap();
        assert_eq!(out.completion, Completion::Complete);
        assert_eq!(out.optimality, Optimality::Proven);
        assert_eq!(out.exit_reason, ExitReason::Exhausted);
        assert_eq!(out.result_identity, "team");
        let actual: Vec<_> = out.results.iter().map(row).collect();
        assert_eq!(
            actual,
            expected[..r.k.min(expected.len())],
            "metric={:?} strategy={:?} k={}",
            r.metric,
            r.strategy,
            r.k
        );
    }
}

#[test]
fn five_fixed_none_pairs_are_five_leader_teams() {
    let (data, roster) = fixture(5, &[]);
    for metric in metrics() {
        let mut r = request(metric, 5, Constraints::default());
        let expected = oracle(&data, &roster, &r);
        assert_eq!(expected.len(), 5);
        assert_eq!(expected[0].members, [1, 2, 3, 4, 5]);
        assert_eq!(expected.iter().map(|r| r.members[2]).collect::<BTreeSet<_>>(), BTreeSet::from([1, 2, 3, 4, 5]));
        for k in [1, 5] {
            r.k = k;
            check(&data, &roster, &r, &expected);
        }
    }
}

#[test]
fn six_members_two_snaps_rank_full_930_and_fixed_leader_155_domains() {
    let (data, roster) = fixture(6, &[1000, 500]);
    for metric in metrics() {
        for leader in [None, Some(1)] {
            let mut r = request(metric.clone(), 31, Constraints { leader, ..Default::default() });
            let expected = oracle(&data, &roster, &r);
            assert_eq!(expected.len(), if leader.is_some() { 155 } else { 930 });
            assert!(expected[..5].iter().all(|r| r.power == 15450 && !r.members.contains(&6)));
            assert_eq!(expected.iter().filter(|r| r.snaps == [None; 5]).count(), if leader.is_some() { 5 } else { 30 });
            for k in [1, 5, 31, 100] {
                r.k = k;
                check(&data, &roster, &r, &expected);
            }
        }
    }
}

#[test]
fn equal_power_snap_bindings_use_none_first_and_constraints_keep_the_full_domain() {
    let (data, roster) = fixture(5, &[0]);
    for metric in metrics() {
        let mut r = request(metric, 31, Constraints { leader: Some(3), ..Default::default() });
        let expected = oracle(&data, &roster, &r);
        assert_eq!(expected.len(), 6);
        assert_eq!(expected[0].snaps, [None; 5]);
        for k in [1, 5, 31] {
            r.k = k;
            check(&data, &roster, &r, &expected);
        }
    }
    let (data, roster) = fixture(6, &[1000, 500]);
    let variants = [
        (
            Constraints {
                include_members: vec![6],
                exclude_members: vec![2],
                exclude_snaps: vec![1],
                ..Default::default()
            },
            30,
        ),
        (Constraints { leader: Some(1), include_members: vec![6], no_snaps: true, ..Default::default() }, 4),
        (Constraints { include_members: vec![1, 2, 3, 4, 5, 6], ..Default::default() }, 0),
    ];
    for metric in metrics() {
        for (constraints, count) in &variants {
            let r = request(metric.clone(), 31, constraints.clone());
            let expected = oracle(&data, &roster, &r);
            assert_eq!(expected.len(), *count);
            check(&data, &roster, &r, &expected);
        }
    }
}

#[test]
fn score_target_plateaus_keep_power_ties_and_zero_budget_never_claims_complete() {
    let (data, roster) = fixture(6, &[1000, 500]);
    for metric in [Metric::ScoreAtLeast { threshold: i32::MAX }, Metric::CappedScore { threshold: i32::MAX }] {
        let r = request(metric, 31, Constraints { leader: Some(1), ..Default::default() });
        let expected = oracle(&data, &roster, &r);
        check(&data, &roster, &r, &expected);
    }
    for metric in metrics() {
        for strategy in [Strategy::BranchAndBound, Strategy::Exhaustive] {
            let mut r = request(metric.clone(), 31, Constraints::default());
            r.strategy = strategy;
            r.limits.time_limit_ms = Some(0);
            let out = recommend(&data, &roster, &r).unwrap();
            assert_eq!(out.completion, Completion::TimedOut);
            assert_eq!(out.optimality, Optimality::Unproven);
            assert_eq!(out.exit_reason, ExitReason::TimeLimit);
            assert!(out.results.is_empty());
        }
    }
}

#[test]
fn distinct_character_constraints_are_preserved_for_alternate_member_cards() {
    let (mut data, roster) = fixture(6, &[1000, 500]);
    data.master.member_cards.iter_mut().find(|m| m.id == 6).unwrap().character_id = 1;
    for metric in metrics() {
        let r = request(metric.clone(), 31, Constraints::default());
        let expected = oracle(&data, &roster, &r);
        assert_eq!(expected.len(), 310, "two valid member sets, five leaders and 31 Snap injections each");
        check(&data, &roster, &r, &expected);
        let r = request(metric, 31, Constraints { include_members: vec![1, 6], ..Default::default() });
        let expected = oracle(&data, &roster, &r);
        assert!(expected.is_empty());
        check(&data, &roster, &r, &expected);
    }
}

#[test]
fn candidate_limit_counts_evaluated_teams_and_cannot_certify_top_k() {
    let (data, roster) = fixture(6, &[1000, 500]);
    for metric in metrics() {
        for strategy in [Strategy::BranchAndBound, Strategy::Exhaustive] {
            let mut r = request(metric.clone(), 31, Constraints::default());
            r.strategy = strategy;
            r.limits.max_candidates = Some(1);
            let out = recommend(&data, &roster, &r).unwrap();
            assert_eq!(out.completion, Completion::TimedOut);
            assert_eq!(out.optimality, Optimality::Unproven);
            assert_eq!(out.exit_reason, ExitReason::CandidateLimit);
            assert_eq!(out.result_identity, "team");
            assert_eq!(out.results.len(), 1);
            assert_eq!(out.telemetry.leaves.evaluated, 1);
            assert!(!out.telemetry.proof.complete);
            let d = &out.results[0];
            assert!(d.members[0] < d.members[1] && d.members[1] < d.members[3] && d.members[3] < d.members[4]);
            let mut exact = r;
            exact.limits.max_candidates = None;
            exact.strategy = Strategy::Exhaustive;
            let fixed = ournotes_search::auxiliary::evaluate_fixed(&data, &roster, &exact, d.members, d.snaps).unwrap();
            assert_eq!(row(d), row(&fixed.results[0]));
        }
    }
}
