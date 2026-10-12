//! Combined metrics over complete synthetic domains, against the weighted sums of independently evaluated terms on
//! every level.
use super::{
    EVENT_ID, SCORE_ID, account_recommendation_fixture, account_request, context_document, data_document,
    joint_request_json, replace_table, roster_document, set_column, synthetic_master,
};
use ournotes_search::{
    auxiliary::evaluate_built,
    engine,
    handler::build_card_pool,
    recommendation::{Status, capabilities},
    search::Completion,
    types::{Fraction, Optimality, RecommendationRequest, Strategy},
};
use ournotes_sim::{cards::Roster, data::DeckData};
use serde_json::{Value, json};

fn inputs() -> (DeckData, Roster) {
    let mut synth = synthetic_master(6, 2, 5);
    // A lower grade can pay more of one reward and less of another.
    set_column(&mut synth, "MasterLiveEventPoint", &mut |row| {
        row["_value"] = json!(match row["_scoreRank"].as_i64().unwrap() {
            2 => 300,
            3 => 900,
            4 => 20,
            _ => 80,
        });
    });
    set_column(&mut synth, "MasterLiveChallengePoint", &mut |row| {
        row["_value"] = json!(match row["_scoreRank"].as_i64().unwrap() {
            2 => 7,
            3 => 2,
            4 => 40,
            _ => 11,
        });
    });
    replace_table(
        &mut synth,
        "MasterEventEffect",
        json!([
            {"_id":1,"_eventId":EVENT_ID,"_eventBonusType":0,"_resourceTypeConstraint":2,"_memberCardId":1,
             "_rank1EffectValue":10000,"_rank2EffectValue":10000,"_rank3EffectValue":10000,
             "_rank4EffectValue":10000,"_rank5EffectValue":10000},
            {"_id":2,"_eventId":EVENT_ID,"_eventBonusType":0,"_resourceTypeConstraint":2,"_memberCardId":4,
             "_rank1EffectValue":2500,"_rank2EffectValue":2500,"_rank3EffectValue":2500,
             "_rank4EffectValue":2500,"_rank5EffectValue":2500},
            {"_id":3,"_eventId":EVENT_ID,"_eventBonusType":1,"_resourceTypeConstraint":2,"_memberCardId":6,
             "_rank1EffectValue":20000,"_rank2EffectValue":20000,"_rank3EffectValue":20000,
             "_rank4EffectValue":20000,"_rank5EffectValue":20000}
        ]),
    );
    (
        DeckData::from_json(&data_document(&synth, 6, 2, 5).to_string()).unwrap(),
        Roster::from_json(&roster_document(6, 2, 5).to_string()).unwrap(),
    )
}

fn request(mode: &str, gekisou: bool, skip: bool, metric: Value) -> RecommendationRequest {
    let mut value = joint_request_json(mode, gekisou, metric);
    if skip {
        value["execution"] = json!({"kind":"skip","scoreId":SCORE_ID});
        value["context"] = context_document(true, false, false);
    }
    value["k"] = json!(100);
    value["strategy"] = json!({"kind":"exhaustive"});
    serde_json::from_value(value).unwrap()
}

fn leveled(levels: &[Vec<(Value, u32)>]) -> Value {
    let levels: Vec<_> = levels
        .iter()
        .map(|terms| {
            let terms: Vec<_> = terms.iter().map(|(metric, weight)| json!({"metric":metric,"weight":weight})).collect();
            json!({"terms":terms})
        })
        .collect();
    json!({"kind":"combined","levels":levels})
}

fn combined(terms: &[(Value, u32)]) -> Value {
    leveled(&[terms.to_vec()])
}

fn numerator(value: &Fraction) -> i128 {
    value.numerator.parse().unwrap()
}

fn points() -> Value {
    json!({"kind":"clientEventPoints","eventId":EVENT_ID})
}
fn challenge_points() -> Value {
    json!({"kind":"clientChallengePoints","eventId":EVENT_ID})
}
fn items() -> Value {
    json!({"kind":"rankedEventItems","eventId":EVENT_ID,"resourceType":11,"resourceId":9})
}

type Team = ([i64; 5], [Option<i64>; 5], i32);

/// Every team of the complete domain with the independently evaluated expectation numerator of each term.
struct Domain {
    terms: Vec<Value>,
    teams: Vec<(Team, Vec<i128>)>,
}
impl Domain {
    fn new(data: &DeckData, roster: &Roster, base: &RecommendationRequest, terms: Vec<Value>) -> Self {
        let complete = engine::recommend(data, roster, base).unwrap();
        assert_eq!(complete.completion, Completion::Complete);
        assert!(complete.results.len() < 100, "the reference holds the whole domain");
        let problems: Vec<_> = terms
            .iter()
            .map(|metric| {
                let mut term = base.clone();
                term.metric = serde_json::from_value(metric.clone()).unwrap();
                build_card_pool(data, roster, &term).unwrap()
            })
            .collect();
        let teams = complete
            .results
            .iter()
            .map(|row| {
                let values = problems
                    .iter()
                    .map(|problem| {
                        let value = evaluate_built(problem, row.members, row.snaps).unwrap();
                        numerator(value.results[0].expected_payoff.as_ref().unwrap())
                    })
                    .collect();
                ((row.members, row.snaps, row.power), values)
            })
            .collect();
        Self { terms, teams }
    }

    fn metric(&self, levels: &[&[(usize, u32)]]) -> Value {
        let levels: Vec<Vec<_>> = levels
            .iter()
            .map(|weights| weights.iter().map(|&(term, weight)| (self.terms[term].clone(), weight)).collect())
            .collect();
        leveled(&levels)
    }

    /// The teams in the order of their weighted sums on each level in turn, then power and canonical identity.
    fn ranked(&self, levels: &[&[(usize, u32)]]) -> Vec<(Team, Vec<i128>)> {
        let mut rows: Vec<_> = self
            .teams
            .iter()
            .map(|(team, values)| {
                let sums = levels
                    .iter()
                    .map(|weights| weights.iter().map(|&(term, weight)| i128::from(weight) * values[term]).sum())
                    .collect::<Vec<i128>>();
                (*team, sums)
            })
            .collect();
        rows.sort_by(|(a, x), (b, y)| y.cmp(x).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
        rows
    }

    fn value(&self, team: &Team, term: usize) -> i128 {
        self.teams.iter().find(|(candidate, _)| candidate == team).expect("a team of the domain").1[term]
    }

    fn best(&self, term: usize) -> i128 {
        self.teams.iter().map(|(_, values)| values[term]).max().expect("a nonempty domain")
    }
}

const POINTS: usize = 0;
const CHALLENGE: usize = 1;
const ITEMS: usize = 2;
const SCORE: usize = 3;
const AT_LEAST: usize = 4;
const CAPPED: usize = 5;

fn domain(data: &DeckData, roster: &Roster, mode: &str, gekisou: bool, skip: bool) -> (RecommendationRequest, Domain) {
    let score = json!({"kind":"score"});
    let base = request(mode, gekisou, skip, score.clone());
    let scores = engine::recommend(data, roster, &base).unwrap();
    let middle = scores.results[scores.results.len() / 2].expected_score.as_ref().unwrap();
    let threshold = (numerator(middle) / middle.denominator.parse::<i128>().unwrap()) as i32;
    let terms = vec![
        points(),
        challenge_points(),
        items(),
        score,
        json!({"kind":"scoreAtLeast","threshold":threshold}),
        json!({"kind":"cappedScore","threshold":threshold}),
    ];
    let domain = Domain::new(data, roster, &base, terms);
    (base, domain)
}

/// Every strategy returns the first teams of `levels` over the complete domain with their level and term values.
fn assert_ranking(
    data: &DeckData,
    roster: &Roster,
    base: &RecommendationRequest,
    domain: &Domain,
    levels: &[&[(usize, u32)]],
    label: &str,
) {
    let expected = domain.ranked(levels);
    let mut search = base.clone();
    search.metric = serde_json::from_value(domain.metric(levels)).unwrap();
    search.k = 5;
    let skip = matches!(search.execution, ournotes_search::types::Execution::Skip { .. });
    for (strategy, cache_entries) in
        [(Strategy::Exhaustive, 0), (Strategy::BranchAndBound, 0), (Strategy::BranchAndBound, 64)]
    {
        let bounded = matches!(strategy, Strategy::BranchAndBound);
        search.strategy = strategy;
        search.limits.cache_entries = cache_entries;
        let actual = engine::recommend(data, roster, &search).unwrap();
        let label = format!("{label} {levels:?} bounded={bounded}");
        assert_eq!(actual.completion, Completion::Complete, "{label}");
        assert_eq!(actual.optimality, Optimality::Proven, "{label}");
        if bounded && !skip {
            assert!(actual.telemetry.environment.bounds.compiled, "{label}");
            assert!(actual.telemetry.environment.bounds.fallback.is_none(), "{label}");
        }
        // Terms the deck alone determines on a Skip, or that step with the local score of a played Live, are ranked
        // by deck. On the fixture's played Lives the event points, the items and the score target have such steps.
        let ranked = |term: usize| {
            if skip { [POINTS, CHALLENGE, ITEMS].contains(&term) } else { [POINTS, ITEMS, AT_LEAST].contains(&term) }
        };
        if bounded && levels.iter().flat_map(|terms| terms.iter()).all(|&(term, _)| ranked(term)) {
            let setup = actual
                .telemetry
                .environment
                .bounds
                .deck_payoff
                .as_ref()
                .unwrap_or_else(|| panic!("{label}: a deck payoff bound"));
            assert!(setup.refusal.is_none(), "{label}: {:?}", setup.refusal);
            assert!(setup.rounds > 0, "{label}");
            assert!(actual.telemetry.environment.bounds.compiled, "{label}");
        }
        assert_eq!(actual.results.len(), expected.len().min(5), "{label}");
        for (row, (team, sums)) in actual.results.iter().zip(&expected) {
            assert_eq!((row.members, row.snaps, row.power), *team, "{label}");
            assert_eq!(numerator(row.expected_payoff.as_ref().unwrap()), sums[0], "{label}");
            let values: Vec<_> = row.level_payoffs.as_ref().expect("level payoffs").iter().map(numerator).collect();
            assert_eq!(&values, sums, "{label}");
            let terms: Vec<_> = row
                .term_payoffs
                .as_ref()
                .expect("term payoffs")
                .iter()
                .map(|term| numerator(term.as_ref().expect("a term without terminal life")))
                .collect();
            let expected: Vec<_> =
                levels.iter().flat_map(|weights| weights.iter().map(|&(term, _)| domain.value(team, term))).collect();
            assert_eq!(terms, expected, "{label}");
        }
    }
}

const MODES: [(&str, bool, bool); 3] = [("free", false, false), ("mission", true, false), ("free", false, true)];

#[test]
fn combined_metrics_rank_by_the_weighted_sum_of_independent_terms() {
    let (data, roster) = inputs();
    for (mode, gekisou, skip) in MODES {
        let (base, domain) = domain(&data, &roster, mode, gekisou, skip);
        let cases: [&[(usize, u32)]; 10] = [
            &[(POINTS, 2), (CHALLENGE, 35)],
            &[(CHALLENGE, 35), (POINTS, 2)],
            &[(POINTS, 1), (ITEMS, 3), (CHALLENGE, 1)],
            &[(ITEMS, 5), (POINTS, 1)],
            &[(SCORE, 1), (CHALLENGE, 1000)],
            &[(AT_LEAST, 1_000_000), (POINTS, 1)],
            &[(CAPPED, 1), (ITEMS, 7)],
            &[(CHALLENGE, 3)],
            &[(SCORE, 2), (CAPPED, 1)],
            &[(POINTS, 1_000_000_000), (CHALLENGE, 999_999_937)],
        ];
        for weights in cases {
            assert_ranking(&data, &roster, &base, &domain, &[weights], &format!("{mode} skip={skip}"));
        }
    }
}

#[test]
fn combined_metric_levels_rank_tied_teams_by_each_later_level() {
    let (data, roster) = inputs();
    for (mode, gekisou, skip) in MODES {
        let (base, domain) = domain(&data, &roster, mode, gekisou, skip);
        let cases: [&[&[(usize, u32)]]; 9] = [
            &[&[(CHALLENGE, 1)], &[(POINTS, 1)]],
            &[&[(CHALLENGE, 1)], &[(ITEMS, 1)], &[(POINTS, 1)]],
            &[&[(AT_LEAST, 1)], &[(POINTS, 2), (CHALLENGE, 35)]],
            &[&[(POINTS, 2), (CHALLENGE, 35)], &[(ITEMS, 1)]],
            &[&[(ITEMS, 1)], &[(SCORE, 1)]],
            &[&[(CAPPED, 1)], &[(CHALLENGE, 7), (ITEMS, 3)], &[(SCORE, 1)]],
            &[&[(AT_LEAST, 1)], &[(CHALLENGE, 1)], &[(ITEMS, 1)], &[(POINTS, 1)]],
            &[&[(ITEMS, 1_000_000_000)], &[(POINTS, 1_000_000_000), (CHALLENGE, 1_000_000_000)]],
            &[&[(CHALLENGE, 2), (ITEMS, 2)], &[(AT_LEAST, 5), (CAPPED, 1)], &[(POINTS, 1), (SCORE, 1)]],
        ];
        for levels in cases {
            assert_ranking(&data, &roster, &base, &domain, levels, &format!("{mode} skip={skip}"));
        }
    }
}

#[test]
fn a_later_level_separates_teams_the_first_level_ties() {
    let (data, roster) = inputs();
    let (_, domain) = domain(&data, &roster, "free", false, false);
    let alone = domain.ranked(&[&[(CHALLENGE, 1)]]);
    let layer = alone.iter().take_while(|(_, sums)| sums[0] == alone[0].1[0]).count();
    assert!(layer > 1, "several teams earn the most challenge points");
    let leveled = domain.ranked(&[&[(CHALLENGE, 1)], &[(POINTS, 1)]]);
    let best = leveled[..layer].iter().map(|(team, _)| domain.value(team, POINTS)).max().unwrap();
    assert_eq!(leveled[0].1[1], best);
    assert!(leveled[..layer].iter().any(|(_, sums)| sums[1] < best), "the layer differs on event points");
    assert!(leveled[..layer].windows(2).all(|pair| pair[0].1[1] >= pair[1].1[1]));
}

#[test]
fn combined_metric_weights_move_the_best_team_between_rewards() {
    let (data, roster) = inputs();
    let (_, domain) = domain(&data, &roster, "free", false, false);
    let toward_points = domain.ranked(&[&[(POINTS, 1_000_000), (CHALLENGE, 1)]])[0].0;
    let toward_challenge = domain.ranked(&[&[(POINTS, 1), (CHALLENGE, 1_000_000)]])[0].0;
    assert_eq!(domain.value(&toward_points, POINTS), domain.best(POINTS));
    assert_eq!(domain.value(&toward_challenge, CHALLENGE), domain.best(CHALLENGE));
    // No team earns the most of both rewards: the weights decide.
    assert!(domain.value(&toward_points, CHALLENGE) < domain.best(CHALLENGE));
    assert!(domain.value(&toward_challenge, POINTS) < domain.best(POINTS));
}

#[test]
fn combined_metric_inputs_are_validated() {
    let (data, roster) = inputs();
    for (metric, message) in [
        (json!({"kind":"combined","levels":[]}), "levels"),
        (leveled(&vec![vec![(points(), 1)]; 5]), "levels"),
        (json!({"kind":"combined","levels":[{"terms":[]}]}), "term"),
        (leveled(&[vec![(points(), 1)], vec![]]), "term"),
        (combined(&[(points(), 0)]), "weight"),
        (combined(&[(points(), 1_000_000_001)]), "weight"),
        (leveled(&[vec![(points(), 1)], vec![(points(), 0)]]), "weight"),
        (combined(&[(json!({"kind":"power"}), 1)]), ""),
        (combined(&[(combined(&[(points(), 1)]), 1)]), ""),
        (combined(&[(json!({"kind":"scoreAtLeast","threshold":0}), 1)]), "target"),
        (leveled(&[vec![(points(), 1)], vec![(json!({"kind":"power"}), 1)]]), ""),
        (combined(&(0..9).map(|_| (points(), 1)).collect::<Vec<_>>()), "terms"),
        (leveled(&[vec![(points(), 1); 5], vec![(challenge_points(), 1); 4]]), "terms"),
        (
            leveled(&[
                vec![(items(), 1_000_000_000)],
                vec![(points(), 1_000_000_000)],
                vec![(json!({"kind":"score"}), 1_000_000_000)],
                vec![(json!({"kind":"score"}), 1_000_000_000)],
            ]),
            "exact integer range",
        ),
    ] {
        let request = request("free", false, false, metric.clone());
        let error = engine::recommend(&data, &roster, &request).err().unwrap_or_else(|| panic!("{metric}"));
        assert!(error.to_string().contains(message), "{metric}: {error}");
    }
}

#[test]
fn active_luck_ranges_reject_combined_metrics() {
    let mut synth = synthetic_master(5, 0, 5);
    set_column(&mut synth, "MasterLiveMusic", &mut |row| row["_gekisouMission1"] = json!(2));
    let mut document = data_document(&synth, 5, 0, 5);
    document["charts"][0]["fevers"] = json!({"startMs":[150],"endMs":[400]});
    let data = DeckData::from_json(&document.to_string()).unwrap();
    let roster = Roster::from_json(&roster_document(5, 0, 5).to_string()).unwrap();
    for metric in [
        combined(&[(points(), 1), (challenge_points(), 1)]),
        leveled(&[vec![(challenge_points(), 1)], vec![(points(), 1)]]),
    ] {
        let request = super::joint_request("mission", true, metric);
        let error = engine::recommend(&data, &roster, &request).unwrap_err();
        assert!(matches!(error, ournotes_sim::Error::Unsupported(_)), "{error}");
        assert!(error.to_string().contains("lottery-free terminal outcomes"), "{error}");
    }
}

fn account_leveled(levels: Value) -> Value {
    let mut request = account_request(json!({"kind":"freeLive","musicId":10,"difficulty":"expert"}));
    request["metric"] = json!({"kind":"combined","consumption":1,"levels":levels});
    request["eventContext"] = json!({"rewardProjection":true,
        "resultClock":{"kind":"played","serverNowJstTicks":150},
        "eventWindows":[{"eventId":EVENT_ID,"startJstTicks":100,"endJstTicks":200}]});
    request
}

fn account_combined(terms: Value) -> Value {
    account_leveled(json!([terms]))
}

fn exact(value: &Value) -> (i128, i128) {
    (
        value["exact"]["numerator"].as_str().unwrap().parse().unwrap(),
        value["exact"]["denominator"].as_str().unwrap().parse().unwrap(),
    )
}

#[test]
fn account_combined_metric_contract_echoes_levels_and_rejects_incomplete_fields() {
    let (data, account) = account_recommendation_fixture();
    assert_eq!(
        capabilities()["combinedMetric"],
        json!({
            "kind":"combined","field":"metric.levels","objective":"lexicographicWeightedExpectedSums",
            "termMetrics":"metrics","weights":"integer","maxLevels":4,"maxTerms":8,"maxWeight":1_000_000_000,
            "consumption":"metric.consumption","lotteryFree":true
        })
    );
    let levels = json!([
        [{"kind":"eventPoints","eventId":EVENT_ID,"weight":2}, {"kind":"challengePoints","eventId":EVENT_ID,"weight":35}],
        [{"kind":"eventItems","eventId":EVENT_ID,"resourceType":11,"resourceId":9,"weight":1}],
        [{"kind":"cappedScore","threshold":1000,"weight":3}, {"kind":"score","weight":1}]
    ]);
    let request = account_leveled(levels.clone());
    let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(answer.status, Status::Ok), "{:?}", answer.errors);
    let value = serde_json::to_value(answer).unwrap();
    assert_eq!(value["result"]["metric"]["kind"], "combined");
    assert_eq!(value["result"]["metric"]["consumption"], 1);
    assert_eq!(value["result"]["metric"]["levels"], levels);
    assert_eq!(value["result"]["optimality"]["proven"], true);
    let teams = value["result"]["teams"].as_array().unwrap();
    assert!(!teams.is_empty() && teams.len() <= 5);
    let mut order = Vec::new();
    for team in teams {
        assert!(team["eventRewards"].is_null());
        let rows = team["levels"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(exact(&team["value"]["payoff"]), exact(&rows[0]["value"]));
        let mut key = Vec::new();
        for (row, terms) in rows.iter().zip(levels.as_array().unwrap()) {
            let (total, denominator) = exact(&row["value"]);
            let parts = row["terms"].as_array().unwrap();
            assert_eq!(parts.len(), terms.as_array().unwrap().len());
            let sum: i128 = parts
                .iter()
                .zip(terms.as_array().unwrap())
                .map(|(part, term)| {
                    let (numerator, same) = exact(part);
                    assert_eq!(same, denominator);
                    assert!(part["score"].is_number() && part["interval"].is_null());
                    i128::from(term["weight"].as_i64().unwrap()) * numerator
                })
                .sum();
            assert_eq!(sum, total);
            key.push(total);
        }
        order.push(key);
    }
    assert!(order.windows(2).all(|pair| pair[0] >= pair[1]), "{order:?}");

    // A score-only combination takes neither consumption nor an event context.
    let mut request = account_request(json!({"kind":"freeLive","musicId":10,"difficulty":"expert"}));
    request["metric"] = json!({"kind":"combined","levels":[
        [{"kind":"scoreAtLeast","threshold":1000,"weight":1}],[{"kind":"score","weight":1}]]});
    let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
    assert!(matches!(answer.status, Status::Ok), "{:?}", answer.errors);

    let term = |term: Value| account_combined(json!([term]));
    let points = json!({"kind":"eventPoints","eventId":EVENT_ID,"weight":1});
    let mut invalid = vec![
        (account_leveled(json!([])), "metric.levels"),
        (account_leveled(Value::Array(vec![json!([points]); 5])), "metric.levels"),
        (account_combined(Value::Array(vec![points.clone(); 9])), "metric.levels"),
        (account_combined(json!([])), "metric.levels[0]"),
        (account_leveled(json!([[points], []])), "metric.levels[1]"),
    ];
    let mut request = account_combined(json!([points]));
    request["metric"].as_object_mut().unwrap().remove("levels");
    invalid.push((request, "metric.levels"));
    request = account_combined(json!([points]));
    request["metric"].as_object_mut().unwrap().remove("consumption");
    invalid.push((request, "metric.consumption"));
    request = account_combined(json!([{"kind":"score","weight":1}]));
    invalid.push((request, "metric.consumption"));
    request = account_combined(json!([points]));
    request["metric"]["eventId"] = json!(EVENT_ID);
    invalid.push((request, "metric.eventId"));
    request = account_combined(json!([points]));
    request["metric"]["secondaryPriority"] = json!("eventPointsFirst");
    invalid.push((request, "metric.secondaryPriority"));
    invalid.push((term(json!({"kind":"eventPoints","eventId":EVENT_ID})), "metric.levels[0][0].weight"));
    invalid.push((term(json!({"kind":"eventPoints","eventId":EVENT_ID,"weight":0})), "metric.levels[0][0].weight"));
    invalid.push((
        term(json!({"kind":"eventPoints","eventId":EVENT_ID,"weight":1_000_000_001i64})),
        "metric.levels[0][0].weight",
    ));
    invalid.push((term(json!({"kind":"eventPoints","weight":1})), "metric.levels[0][0].eventId"));
    invalid.push((
        term(json!({"kind":"eventPoints","eventId":EVENT_ID,"threshold":5,"weight":1})),
        "metric.levels[0][0].threshold",
    ));
    invalid.push((term(json!({"kind":"combined","weight":1})), "metric.levels[0][0].kind"));
    invalid.push((term(json!({"kind":"power","weight":1})), "metric.levels[0][0].kind"));
    request =
        account_leveled(json!([[points], [{"kind":"eventItems","eventId":EVENT_ID,"resourceType":11,"weight":1}]]));
    invalid.push((request, "metric.levels[1][0].resourceId"));
    request = account_request(json!({"kind":"freeLive","musicId":10,"difficulty":"expert"}));
    request["metric"] = json!({"kind":"score","levels":[[{"kind":"score","weight":1}]]});
    invalid.push((request, "metric.levels"));
    for (request, path) in invalid {
        let answer = engine::recommend_account(&data, &account.to_string(), &request.to_string(), None);
        assert!(matches!(answer.status, Status::Invalid), "{path}: {request}");
        assert!(answer.errors.iter().any(|issue| issue.path == path), "{path}: {:?}", answer.errors);
    }
}

/// Native parallel search ranks combined metrics as the serial engine does: partitioned domains (exhaustive and
/// Skip branch-and-bound) share one folded-level cutoff across parts whose multipliers differ, and Live
/// branch-and-bound keeps the serial traversal while its performance orders play on several threads.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_search_ranks_combined_metric_levels_as_the_serial_engine() {
    use ournotes_search::parallel::{self, Cancellation};
    let (data, roster) = inputs();
    let workers = 4.min(parallel::max_workers()).max(2);
    let cases: [&[&[(usize, u32)]]; 5] = [
        &[&[(POINTS, 2), (CHALLENGE, 35)]],
        &[&[(CHALLENGE, 1)], &[(POINTS, 1)]],
        &[&[(CHALLENGE, 1)], &[(ITEMS, 1)], &[(POINTS, 1)]],
        &[&[(AT_LEAST, 1)], &[(CHALLENGE, 1)], &[(ITEMS, 1)], &[(POINTS, 1)]],
        &[&[(CHALLENGE, 2), (ITEMS, 2)], &[(AT_LEAST, 5), (CAPPED, 1)], &[(POINTS, 1), (SCORE, 1)]],
    ];
    let mut partitioned = 0;
    for (mode, gekisou, skip) in MODES {
        let (base, domain) = domain(&data, &roster, mode, gekisou, skip);
        for levels in cases {
            let expected = domain.ranked(levels);
            for strategy in [Strategy::Exhaustive, Strategy::BranchAndBound] {
                for k in [1, 3, 5] {
                    let mut search = base.clone();
                    search.metric = serde_json::from_value(domain.metric(levels)).unwrap();
                    search.k = k;
                    search.strategy = strategy.clone();
                    let label = format!("{mode} skip={skip} {strategy:?} k={k} {levels:?}");
                    let serial = engine::recommend(&data, &roster, &search).unwrap();
                    let actual =
                        parallel::recommend(&data, &roster, &search, workers, Cancellation::default()).unwrap();
                    assert_eq!(actual.completion, Completion::Complete, "{label}");
                    assert_eq!(actual.optimality, Optimality::Proven, "{label}");
                    assert_eq!(actual.results, serial.results, "{label}");
                    let teams: Vec<_> = actual.results.iter().map(|r| (r.members, r.snaps, r.power)).collect();
                    let reference: Vec<_> = expected.iter().take(k).map(|(team, _)| *team).collect();
                    assert_eq!(teams, reference, "{label}");
                    let t = actual.telemetry.parallel.as_ref().expect("parallel telemetry");
                    if t.fallback.is_none() && t.tasks > 1 {
                        partitioned += 1;
                    }
                }
            }
        }
    }
    assert!(partitioned > 0, "some combined requests ran on partitioned domains");
}
