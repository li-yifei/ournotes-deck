//! Carrier-cache refactor: compare bounds to exact complete evaluations and
//! provide a repeatable warm-bound benchmark independent of scorer throughput.
use super::{
    common::{extend_table, set_column},
    data_document, joint_request, roster_document, synthetic_master,
};
use ournotes_search::{auxiliary, handler, search::diagnostics};
use ournotes_sim::{cards::Roster, data::DeckData};
use serde_json::json;

fn fixture() -> (DeckData, Roster, ournotes_search::types::RecommendationRequest) {
    let mut synth = synthetic_master(6, 2, 6);
    set_column(&mut synth, "MasterGekisouSkillEffect", &mut |row| {
        row["_skillEffectType"] = json!(12000);
    });
    extend_table(&mut synth, "MasterSkillEffectSetting", vec![json!({"_id":9001,"_skillEffectType":12000,"_phase":2})]);
    let data = DeckData::from_json(&data_document(&synth, 6, 2, 6).to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(6, 2, 6).to_string()).unwrap();
    let mut request = joint_request("mission", true, json!({"kind":"score"}));
    request.constraints.leader = Some(3);
    request.k = 1;
    (data, roster, request)
}

fn decks() -> Vec<([i64; 5], [Option<i64>; 5])> {
    let mut out = Vec::new();
    for members in [[1, 2, 3, 4, 5], [1, 2, 3, 4, 6], [1, 2, 3, 5, 6]] {
        out.push((members, [None; 5]));
        for slot in 0..5 {
            let mut snaps = [None; 5];
            snaps[slot] = Some(1);
            out.push((members, snaps));
            for other in 0..5 {
                if other != slot {
                    snaps[other] = Some(2);
                    out.push((members, snaps));
                    snaps[other] = None;
                }
            }
        }
    }
    out
}

#[test]
fn carrier_cache_warm_reads_preserve_admissible_bounds() {
    let (data, roster, request) = fixture();
    let built = handler::build_card_pool(&data, &roster, &request).unwrap();
    let mut scratch = diagnostics::PrefixAuditScratch::default();
    for (members, snaps) in decks() {
        let exact = auxiliary::evaluate_built(&built, members, snaps).unwrap().results[0]
            .expected_payoff
            .as_ref()
            .unwrap()
            .numerator
            .parse::<i128>()
            .unwrap();
        for depth in 1..5 {
            let first = diagnostics::prefix_upper(&built, members, snaps, depth, &mut scratch).unwrap().unwrap();
            let warm = diagnostics::prefix_upper(&built, members, snaps, depth, &mut scratch).unwrap().unwrap();
            assert_eq!(first, warm);
            assert!(diagnostics::reset_carrier_split_cache(&built));
            let rebuilt = diagnostics::prefix_upper(&built, members, snaps, depth, &mut scratch).unwrap().unwrap();
            assert_eq!(warm, rebuilt, "arena clear/rebuild preserves bounds");
            assert!(first.0 >= exact, "depth={depth}, members={members:?}, snaps={snaps:?}: {} < {exact}", first.0);
        }
    }
    assert!(scratch.checked_carrier_split_prefixes > 0, "fixture exercises carrier split");
}

#[test]
fn carrier_cache_production_topk_matches_exhaustive_after_early_returns() {
    let (data, roster, mut request) = fixture();
    request.k = 5;
    request.limits.cache_entries = 64;
    let bounded = ournotes_search::engine::recommend(&data, &roster, &request).unwrap();
    assert_eq!(bounded.completion, ournotes_search::search::Completion::Complete);
    request.strategy = ournotes_search::types::Strategy::Exhaustive;
    let exact = ournotes_search::engine::recommend(&data, &roster, &request).unwrap();
    assert_eq!(exact.completion, ournotes_search::search::Completion::Complete);
    assert_eq!(bounded.results, exact.results);
}

#[test]
#[ignore = "manual carrier-cache warm-bound throughput measurement"]
fn carrier_cache_throughput() {
    let (data, roster, request) = fixture();
    let built = handler::build_card_pool(&data, &roster, &request).unwrap();
    let mut scratch = diagnostics::PrefixAuditScratch::default();
    let decks = decks();
    let mut checksum = 0i128;
    for pass in 0..6 {
        let started = std::time::Instant::now();
        for _ in 0..50 {
            for &(members, snaps) in &decks {
                for depth in 1..5 {
                    let bound =
                        diagnostics::prefix_upper(&built, members, snaps, depth, &mut scratch).unwrap().unwrap();
                    checksum = checksum.wrapping_add(std::hint::black_box(bound.0));
                }
            }
        }
        println!(
            "carrier pass={pass} calls={} elapsed_ms={:.3} checksum={checksum}",
            50 * decks.len() * 4,
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    assert!(scratch.checked_carrier_split_prefixes > 0);
}
