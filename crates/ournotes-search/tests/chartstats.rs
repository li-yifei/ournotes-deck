//! Chart statistics measured on the whole-live simulation with Gekisou on and off, on a synthetic deck data file.

#[path = "../../ournotes-sim/tests/common/mod.rs"]
mod common;

use ournotes_sim::chartstats::{self, POWER};
use ournotes_sim::data::{DeckData, FORMAT};
use ournotes_sim::live::full::{self, GekisouSetup, LiveNote, LiveParams, Performer};
use ournotes_sim::live::model::{JudgementStream, JustRule, LiveModel, Play};
use ournotes_sim::live::score::{ComboTable, LiveScoreSettings};
use ournotes_sim::live::skip::skip_score;
use serde_json::{Value, json};

fn columns(rows: &Value) -> Value {
    let mut cols: Vec<String> = Vec::new();
    for r in rows.as_array().unwrap() {
        for k in r.as_object().unwrap().keys() {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    let rows: Vec<Value> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| Value::Array(cols.iter().map(|c| r.get(c).cloned().unwrap_or(Value::Null)).collect()))
        .collect();
    json!({"columns": cols, "rows": rows})
}

const EVENTS: [i32; 5] = [3000, 5000, 20000, 40000, 58500];

/// A chart of `n` notes with a note exactly at each event time and at each event time + 5000 ms, a slide pair and
/// combo ticks, and these fevers; notes listed out of time order.
fn chart_json_fevers(score_id: i64, n: i32, rng: &mut common::Rng, fevers: &[(i32, i32)]) -> (Value, i32) {
    let (mut ids, mut ops, mut jts, mut times) = (vec![], vec![], vec![], vec![]);
    let mut push = |id: i32, op: i32, t: i32| {
        ids.push(id);
        ops.push(op);
        jts.push(if op == 21 || op == 120 { 21 } else { 1 });
        times.push(t);
    };
    let mut id = 1;
    for &e in &EVENTS {
        push(id, 1, e);
        push(id + 1, 1, e + 5000);
        id += 2;
    }
    for _ in 0..n {
        let op = [1, 1, 1, 20, 21, 40, 120][rng.below(7) as usize];
        push(id, op, rng.range(1000, 60000) as i32);
        id += 1;
    }
    let last = *times.iter().max().unwrap();
    (
        json!({
            "scoreId": score_id,
            "asset": {"key": "Live/MusicScore/x", "sha256": "0".repeat(64)},
            "notes": {"id": ids, "op": ops, "judgementType": jts, "timeMs": times},
            "skillEvents": {"timeMs": EVENTS},
            "fevers": {"startMs": fevers.iter().map(|f| f.0).collect::<Vec<_>>(),
                       "endMs": fevers.iter().map(|f| f.1).collect::<Vec<_>>()},
        }),
        last,
    )
}

/// The judged notes of a chart (its full combo).
fn judged_of(chart: &Value) -> i64 {
    let ops = chart["notes"]["op"].as_array().unwrap();
    ops.iter().filter(|o| ![0, 80, 82, 100, 103, 121, 122, 123].contains(&o.as_i64().unwrap())).count() as i64
}

/// The synthetic master and these charts; with `extra`, also live skill 4 (2000 on the confirmed rank, condition
/// 7012) and 5 (2000 on a Gekisou combo, condition 7005), one level each.
fn document_with(charts: Vec<Value>, extra: bool) -> Value {
    let mut rng = common::Rng::new(7);
    document_from(charts, extra, common::synth(&mut rng, 12, 4))
}

fn document_from(charts: Vec<Value>, extra: bool, mut s: common::Synth) -> Value {
    if extra {
        let mut push = |table: &str, rows: Value| {
            let t = s.tables.iter_mut().find(|(n, _)| n == table).unwrap();
            t.1.as_array_mut().unwrap().extend(rows.as_array().unwrap().iter().cloned());
        };
        push(
            "MasterSkillCondition",
            json!([{"_id": 20, "_conditionType": 7012, "_conditionValues": [1], "_isPositive": true, "_conditionTargetIDs": []},
                   {"_id": 21, "_conditionType": 7005, "_conditionValues": [1], "_isPositive": true, "_conditionTargetIDs": []}]),
        );
        push(
            "MasterSkillConditionSet",
            json!([{"_id": 20, "_group": 20, "_conditionIds": [20]}, {"_id": 21, "_group": 21, "_conditionIds": [21]}]),
        );
        push("MasterLiveSkill", json!([{"_id": 4, "_skillCategories": [1]}, {"_id": 5, "_skillCategories": [1]}]));
        push(
            "MasterLiveSkillEffect",
            json!([{"_id": 1000, "_liveSkillID": 4, "_level": 1, "_skillConditionGroup": 20, "_skillTargetIDs": [],
                    "_skillEffectType": 2000, "_activationTimeSecond": 5.0, "_effectValue": 3000},
                   {"_id": 1001, "_liveSkillID": 5, "_level": 1, "_skillConditionGroup": 21, "_skillTargetIDs": [],
                    "_skillEffectType": 2000, "_activationTimeSecond": 5.0, "_effectValue": 3000}]),
        );
    }
    let mut master = serde_json::Map::new();
    for (name, rows) in &s.tables {
        master.insert(name.clone(), columns(rows));
    }
    let scores: Vec<Value> = charts
        .iter()
        .map(|c| {
            json!({"_id": c["scoreId"], "_musicScoreTextFileName": "x", "_musicScoreLevel": 24,
                   "_fullComboCount": judged_of(c)})
        })
        .collect();
    master.insert("MasterLiveMusicScore".into(), columns(&Value::Array(scores)));
    master.insert(
        "MasterLiveComboScoreBonus".into(),
        columns(&json!([{"_id": 1, "_comboBonusType": 0, "_requiredComboCount": 10, "_bonusFactor": 0.01},
                         {"_id": 2, "_comboBonusType": 0, "_requiredComboCount": 100, "_bonusFactor": 0.05},
                         {"_id": 3, "_comboBonusType": 0, "_requiredComboCount": 300, "_bonusFactor": 0.1},
                         {"_id": 4, "_comboBonusType": 1, "_requiredComboCount": 10, "_bonusFactor": 0.02},
                         {"_id": 5, "_comboBonusType": 1, "_requiredComboCount": 30, "_bonusFactor": 0.05}])),
    );
    master.insert(
        "MasterLiveJudgementParameter".into(),
        columns(&json!([{"_id": 1, "_noteSimulateJudgement": 5, "_scorePercent": 100, "_damage": 0},
                         {"_id": 2, "_noteSimulateJudgement": 4, "_scorePercent": 50, "_damage": 0},
                         {"_id": 3, "_noteSimulateJudgement": 6, "_scorePercent": 120, "_damage": 0}])),
    );
    let mut settings = s.tables.iter().find(|(n, _)| n == "MasterLiveSettings").unwrap().1.clone();
    settings.as_array_mut().unwrap().push(json!({"_id": 9, "_key": "life_base", "_value": "1000"}));
    for (id, key, value) in [
        (10, "gekisou_luck_gauge_max", "140"),
        (11, "gekisou_luck_gauge_max_rush", "70"),
        (12, "gekisou_luck_rush_score_bonus_percent", "10"),
        (13, "life_denger", "300"),
    ] {
        settings.as_array_mut().unwrap().push(json!({"_id": id, "_key": key, "_value": value}));
    }
    // Gekisou: the song's missions combo, luck, Just (pattern 2), a Just timing row, rank bonuses, luck tables
    let mut musics = s.tables.iter().find(|(n, _)| n == "MasterLiveMusic").unwrap().1.clone();
    for m in musics.as_array_mut().unwrap() {
        m["_gekisouMission1"] = json!(1);
        m["_gekisouMission2"] = json!(2);
        m["_gekisouMission3"] = json!(3);
    }
    master.insert("MasterLiveMusic".into(), columns(&musics));
    let mut timing = s.tables.iter().find(|(n, _)| n == "MasterLiveJudgementTiming").map_or(json!([]), |t| t.1.clone());
    if !timing.as_array().unwrap().iter().any(|r| r["_noteJudgementType"] == 1 && r["_noteSimulateJudgement"] == 6) {
        timing.as_array_mut().unwrap().push(json!({"_id": 90, "_noteJudgementType": 1, "_noteSimulateJudgement": 6}));
    }
    master.insert("MasterLiveJudgementTiming".into(), columns(&timing));
    let ranks: Vec<Value> = (1..=3)
        .map(|c| json!({"_id": c, "_missionPattern": 2, "_count": c, "_rank": 1, "_scoreBonusPercent": 10 * c}))
        .collect();
    master.insert("MasterLiveGekisouRankingScoreBonus".into(), columns(&Value::Array(ranks)));
    master.insert(
        "MasterLiveGekisouLuckBasePoint".into(),
        columns(&json!([{"_id": 1, "_noteCategory": 0, "_noteSimulateJudgement": 5, "_weight": 1, "_basePoint": 10},
                         {"_id": 2, "_noteCategory": 0, "_noteSimulateJudgement": 6, "_weight": 1, "_basePoint": 10},
                         {"_id": 3, "_noteCategory": 1, "_noteSimulateJudgement": 5, "_weight": 1, "_basePoint": 2}])),
    );
    let lots: Vec<Value> = (0..5)
        .flat_map(|k| {
            (0..4).map(move |r| json!({"_id": 1 + 4 * k + r, "_chanceLotType": k, "_lotResult": r, "_weight": 1}))
        })
        .collect();
    master.insert("MasterLiveGekisouLuckBonusLot".into(), columns(&Value::Array(lots)));
    master.insert("MasterLiveSettings".into(), columns(&settings));
    common::every_table(&mut master);
    json!({
        "format": FORMAT,
        "provenance": {"region": "test", "master": {"source": "api", "version": "v1"},
                       "exporter": {"name": "test", "version": "0", "chartFormat": "test"}},
        "master": master,
        "charts": charts,
    })
}

fn data(n: i32) -> (DeckData, i32) {
    data_fevers(n, &[])
}

fn data_fevers(n: i32, fevers: &[(i32, i32)]) -> (DeckData, i32) {
    data_with(n, fevers, false)
}

fn data_with(n: i32, fevers: &[(i32, i32)], extra: bool) -> (DeckData, i32) {
    let mut rng = common::Rng::new(11);
    let (chart, last) = chart_json_fevers(1004, n, &mut rng, fevers);
    (DeckData::from_json(&document_with(vec![chart], extra).to_string()).unwrap(), last)
}

const FEVERS: [(i32, i32); 3] = [(8000, 16000), (24000, 32000), (42000, 50000)];

fn notes_of(d: &DeckData) -> Vec<LiveNote> {
    let chart = d.chart(1004).unwrap();
    chart
        .notes
        .iter()
        .zip(&d.charts[0].judgement_types)
        .map(|(n, &jt)| LiveNote {
            note_id: n.id,
            time_ms: n.time_ms,
            note_operate_type: n.note_type,
            judgement_type: jt,
        })
        .collect()
}

#[test]
fn kinds_group_master_rows_by_shape() {
    let (d, _) = data(50);
    let kinds = chartstats::kinds(&d.master);
    // skill 1: 2000 for 5 s; skill 2: 2004 on two targets for 4.5 s; skill 3: two conditioned 2000 rows for 6 s
    assert_eq!(kinds.len(), 4);
    assert_eq!((kinds[0].effect_type, kinds[0].duration_ms, kinds[0].rows), (2000, 5000, 5));
    assert_eq!(kinds[0].values, vec![850, 1000, 1150, 1300, 1450]);
    assert_eq!((kinds[1].effect_type, kinds[1].skill_target_ids.clone()), (2004, vec![12, 13]));
    assert_eq!((kinds[2].skill_condition_group, kinds[3].skill_condition_group), (4, 5));
    assert_eq!(chartstats::kind_factor(2000, 10000), 1.0);
    assert_eq!(chartstats::kind_factor(2005, 10000), -1.0);
    assert_eq!(chartstats::kind_factor(2004, 10000), 1.0);
}

/// Every range mission, measured weights and the seed checks; then real decks of the master's own skills, simulated
/// directly, against the prediction from the statistics.
#[test]
fn skip_coefficient_bounds_the_skip_score() {
    let (d, _) = data(500);
    let s = chartstats::chart_stats(&d.master, &d.charts[0], &[], 1).unwrap();
    let chart = d.chart(1004).unwrap();
    let settings = LiveScoreSettings::from_master(&d.master).unwrap();
    let combo = ComboTable::from_master(&d.master).unwrap();
    for power in [1000, POWER, 2_000_000] {
        let exact =
            skip_score(power, 24, &chart, &settings, &settings.valid_note_types(), Some(&combo)).unwrap() as f64;
        let p = power as f64 * s.skip;
        assert!(exact <= p * (1.0 + 4e-6) && exact >= p * (1.0 - 4e-6) - chart.notes.len() as f64, "{exact} {p}");
    }
}

#[test]
fn per_order_model_matches_the_whole_live_simulation_with_live_skills() {
    let (d, _) = data(700);
    let chart = d.chart(1004).unwrap();
    let dc = &d.charts[0];
    let play = Play::theoretical_best(&d.master, &chart).unwrap();
    let model = LiveModel::new(&d.master, 24, &chart, &play).unwrap();
    let notes: Vec<LiveNote> = chart
        .notes
        .iter()
        .zip(&dc.judgement_types)
        .map(|(n, &jt)| LiveNote {
            note_id: n.id,
            time_ms: n.time_ms,
            note_operate_type: n.note_type,
            judgement_type: jt,
        })
        .collect();
    let events: Vec<(i32, i32)> = chart.skill_events.iter().map(|e| (e.index, e.time_ms)).collect();
    let stream = JudgementStream::theoretical_best(&chart).to_live_play().unwrap();
    let mut rng = common::Rng::new(5);
    let mut compared = 0;
    for _ in 0..40 {
        let perf: Vec<(i64, i64)> = (0..5).map(|_| (rng.range(1, 3), rng.range(1, 5))).collect();
        let power = rng.range(50_000, 900_000) as i32;
        let cmds = model.commands(&d.master, &perf).unwrap();
        let per_order = model.score(power, &cmds);
        let performers: Vec<Performer> =
            perf.iter().map(|&s| Performer { live_skill: Some(s), ..Default::default() }).collect();
        let params = LiveParams {
            skill_target_music_type: 0,
            total_power: power,
            music_level: 24,
            converted_note_count: chart.converted_note_count,
            music_length_ms: chart.last_timing_note_ms + 1000,
            score_music_length_ms: None,
            assist_factor: 1.0,
        };
        let whole =
            full::LiveModel::new(&d.master, &performers, &notes, &events, params).unwrap().run(&stream).unwrap();
        assert_eq!(per_order, whole, "skills {perf:?} power {power}");
        compared += 1;
    }
    assert_eq!(compared, 40);
}

/// The kind of a live skill effect row.
fn kind_of(kinds: &[chartstats::Kind], row: &ournotes_sim::master::LiveSkillEffectRow) -> usize {
    kinds
        .iter()
        .position(|k| {
            k.effect_type == row.skill_effect_type
                && k.activation_time_second == row.activation_time_second
                && k.skill_target_ids == row.skill_target_ids
                && k.skill_condition_group == row.skill_condition_group
        })
        .unwrap()
}

#[test]
fn off_seeds_match_the_per_order_model() {
    let (d, _) = data_fevers(700, &FEVERS);
    let kinds = chartstats::kinds(&d.master);
    let s = chartstats::chart_stats(&d.master, &d.charts[0], &kinds, 2).unwrap();
    let off = &s.off_seeds[0];
    assert_eq!((s.off_seeds.len(), off.seed), (1, chartstats::OFF_SEED));
    assert!(off.weights.iter().all(|w| w.as_ref().is_some_and(|w| w.len() == 5)));
    assert!((off.check.exact as f64 - off.check.predicted).abs() <= off.check.bound);
    // no Just, no rank bonus: less than Gekisou on
    assert!((off.score as f64) < s.expectation.as_ref().unwrap().score[0]);
    let chart = d.chart(1004).unwrap();
    let play = Play::theoretical_best(&d.master, &chart).unwrap();
    let model = LiveModel::new(&d.master, 24, &chart, &play).unwrap();
    assert_eq!(model.score(POWER, &[]), off.score);
    let mut rng = common::Rng::new(13);
    for _ in 0..40 {
        let power = rng.range(50_000, 900_000) as i32;
        let perf: Vec<(i64, i64)> =
            (0..5).map(|_| if rng.below(5) == 0 { (0, 0) } else { (rng.range(1, 3), rng.range(1, 5)) }).collect();
        let exact = model.score(power, &model.commands(&d.master, &perf).unwrap()) as f64;
        let mut predicted = off.score as f64 / POWER as f64;
        let mut gain = 0.0;
        for (k, &(id, lv)) in perf.iter().enumerate() {
            for row in d.master.live_skill_effects.iter().filter(|r| r.live_skill_id == id && r.level == lv) {
                let x = chartstats::kind_factor(row.skill_effect_type, row.effect_value);
                predicted += x * off.weights[kind_of(&kinds, row)].as_ref().unwrap()[k];
                gain += x;
            }
        }
        let p = power as f64 * predicted;
        let scale = power as f64 / POWER as f64;
        let bound = s.judged_notes as f64 * (1.0 + scale * (1.0 + 2.0 * gain)) + 4e-6 * p;
        assert!((exact - p).abs() <= bound, "power {power} deck {perf:?}: exact {exact} predicted {p}");
    }
}

const APT_FEVERS: [(i32, i32); 3] = [(1000, 1800), (2800, 3600), (4600, 5400)];

fn aptitude_json(fevers: &[(i32, i32)]) -> Value {
    let mut s = common::synth(&mut common::Rng::new(7), 12, 6);
    common::extend_table(
        &mut s,
        "MasterSkillTarget",
        vec![
            json!({"_id":55,"_skillTargetType":5,"_gekisouMissionType":1}),
            json!({"_id":56,"_skillTargetType":5,"_gekisouMissionType":2}),
            json!({"_id":57,"_skillTargetType":5,"_gekisouMissionType":3}),
            json!({"_id":58,"_skillTargetType":3,"_bandID":1}),
            json!({"_id":59,"_skillTargetType":3,"_bandID":2}),
            json!({"_id":60,"_skillTargetType":4,"_judgement":4}),
        ],
    );
    common::extend_table(
        &mut s,
        "MasterSkillCondition",
        (0..5)
            .map(|i| {
                json!({
                    "_id":80+i,"_conditionType":if i<3 {7010} else {5000},"_conditionValues":[],
                    "_isPositive":true,"_conditionTargetIDs":[55+i]
                })
            })
            .collect(),
    );
    common::extend_table(
        &mut s,
        "MasterSkillConditionSet",
        (0..5)
            .map(|i| {
                json!({
                    "_id":250+i,"_group":250+i,"_conditionIds":[80+i]
                })
            })
            .collect(),
    );
    // 1 and 2 have identical highest-level effects, but different level numbers and lower-level effects.
    let skills =
        [(1, 1, 3, 12000, 8), (2, 1, 5, 12000, 8), (3, 2, 2, 11001, 30000), (4, 3, 2, 13000, 2), (5, 2, 2, 11002, 17)];
    common::replace_table(
        &mut s,
        "MasterGekisouSkill",
        skills.iter().map(|&(id, m, _, _, _)| json!({"_id":id,"_gekisouMissionType":m})).collect(),
    );
    let mut rows = Vec::new();
    for &(id, mission, level, ty, value) in &skills {
        for (lv, v) in [(1, 1), (level, value)] {
            rows.push(json!({"_id":rows.len()+1,"_gekisouSkillID":id,"_level":lv,
                "_skillTriggerType":1,"_skillTriggerConditionGroup":249+mission,
                "_skillEffectType":ty,"_activationTimeSecond":if ty == 11002 {0.0} else {1.2},"_effectValue":v}));
        }
    }
    common::replace_table(&mut s, "MasterGekisouSkillEffect", Value::Array(rows));
    common::set_column(&mut s, "MasterMemberCard", &mut |r| {
        r["_gekisouSkillID"] = json!(1 + (r["_id"].as_i64().unwrap() - 1) % 5);
    });
    // Support 1 and 2 differ only in band. Rank 5 chooses level 3, NOT the highest effect level 5.
    common::replace_table(
        &mut s,
        "MasterGekisouSupportSkill",
        (1..=5).map(|id| json!({"_id":id,"_gekisouMissionType":if id==5 {3} else {1}})).collect(),
    );
    let mut rows = Vec::new();
    for id in 1..=5 {
        for level in [1, 3, 5] {
            rows.push(json!({"_id":rows.len()+1,"_gekisouSupportSkillID":id,"_level":level,
                "_skillTriggerType":1,"_skillTriggerConditionGroup":if id==5 {252} else {250},
                "_skillConditionGroup":if id<=2 {252+id} else {0},
                "_skillEffectType":match id {1|2=>2000,3=>12004,4=>12006,_=>4004},
                "_activationTimeSecond":1.2,"_effectValue":if id<=2 {1000*level} else {5},
                "_effectLimitCount":if id==3 || id==4 {3} else {0},
                "_skillTargetIDs":if id==3 || id==4 {vec![60]} else {vec![]}}));
        }
    }
    common::replace_table(&mut s, "MasterGekisouSupportSkillEffect", Value::Array(rows));
    common::set_column(&mut s, "MasterSupportCard", &mut |r| {
        r["_gekisouSupportSkillId01"] = json!(1 + (r["_id"].as_i64().unwrap() - 1) % 5);
    });
    common::set_column(&mut s, "MasterSupportCardRank", &mut |r| {
        r["_gekisouSupportSkill01Level"] = json!(if r["_rank"] == 5 { 3 } else { 1 });
    });
    let chart = json!({"scoreId":1004,"asset":{"key":"synthetic","sha256":"0".repeat(64)},
        "notes":{"id":(1..=64).collect::<Vec<_>>(),"op":vec![1;64],
            "judgementType":(1..=64).map(|i|if i%4==0 {21} else {1}).collect::<Vec<_>>(),
            "timeMs":(1..=64).map(|i|i*100).collect::<Vec<_>>()},
        "skillEvents":{"timeMs":[900,1200,2700,4500,5400]},
        "fevers":{"startMs":fevers.iter().map(|f|f.0).collect::<Vec<_>>(),
            "endMs":fevers.iter().map(|f|f.1).collect::<Vec<_>>()}});
    document_from(vec![chart], false, s)
}

fn aptitude_data(fevers: &[(i32, i32)]) -> DeckData {
    DeckData::from_json(&aptitude_json(fevers).to_string()).unwrap()
}

fn shape_for(shapes: &[chartstats::Shape], source: &str, skill: i64) -> usize {
    shapes.iter().find(|s| s.source == source && s.skills.iter().any(|x| x.id == skill)).unwrap().id
}

#[test]
fn aptitude_shapes_deduplicate_effects_and_abstract_bands() {
    let d = aptitude_data(&APT_FEVERS);
    let h = chartstats::aptitude_header(&d.master, &chartstats::kinds(&d.master));
    assert_eq!(h.plain_kind, Some(0));
    assert_eq!(h.law, "independent nominal lottery and skill probabilities");
    assert_eq!(h.shapes.len(), 8);
    assert_eq!(h.shapes.iter().map(|s| s.id).collect::<Vec<_>>(), (0..8).collect::<Vec<_>>());
    let member = &h.shapes[shape_for(&h.shapes, "member", 1)];
    assert_eq!(member.skills.iter().map(|s| (s.id, s.level)).collect::<Vec<_>>(), [(1, 3), (2, 5)]);
    assert!(!member.band_condition);
    assert!(member.skills.iter().all(|s| s.member_target_ids.is_none() && s.band_ids.is_none()));
    let support = &h.shapes[shape_for(&h.shapes, "support", 1)];
    assert_eq!(support.id, shape_for(&h.shapes, "support", 2));
    assert!(support.band_condition);
    assert_eq!(support.skills.iter().map(|s| (s.id, s.level)).collect::<Vec<_>>(), [(1, 3), (2, 3)]);
    for (i, s) in support.skills.iter().enumerate() {
        assert_eq!(s.member_target_ids, Some(vec![58 + i as i64]));
        assert_eq!(s.band_ids, Some(vec![1 + i as i64]));
    }
    assert_eq!(support.effects[0].condition[0][0].condition_type, 5000);
    assert!(support.effects[0].condition[0][0].target_ids.is_none());
    assert_eq!(support.effects[0].effect_value, 3000);
    let mut empty = d;
    empty.charts.clear();
    let doc = chartstats::document_with(&empty, &chartstats::Options::default()).unwrap();
    assert!(doc["charts"].as_array().unwrap().is_empty());
    assert_eq!(doc["gekisouAptitude"]["law"], h.law);
    assert_eq!(doc["gekisouAptitude"]["shapes"], serde_json::to_value(h.shapes).unwrap());
}

/// Direct engine run, assembling a performer independently of the aptitude implementation.
fn aptitude_run(
    d: &DeckData,
    performer: Option<Performer>,
    seed: i32,
    perfect: bool,
) -> (i32, Vec<full::GekisouRange>) {
    let chart = d.chart(1004).unwrap();
    let setup = GekisouSetup { fevers: d.charts[0].fevers.clone(), missions: vec![1, 2, 3] };
    let rule = JustRule::new(&d.master, &setup).unwrap();
    let mut stream = JudgementStream::theoretical_best_gekisou(&chart, &d.charts[0].judgement_types, &rule).unwrap();
    if perfect {
        for j in &mut stream.judged {
            if j[2] == 6 {
                j[2] = 5;
            }
        }
    }
    let dt = stream.delta_times().unwrap();
    let mut play = stream.to_live_play().unwrap();
    play.base_seed = seed;
    let params = LiveParams {
        total_power: POWER,
        music_level: 24,
        converted_note_count: chart.converted_note_count,
        music_length_ms: chart.last_timing_note_ms + 1000,
        score_music_length_ms: None,
        assist_factor: 1.0,
        skill_target_music_type: 1,
    };
    let mut deck: Vec<_> = performer.into_iter().collect();
    deck.resize(5, Performer::default());
    let events: Vec<_> = chart.skill_events.iter().map(|e| (e.index, e.time_ms)).collect();
    let mut model = full::LiveModel::new_gekisou(&d.master, &deck, &notes_of(d), &events, params, &setup).unwrap();
    let score = model.run_timed(&play, &dt).unwrap();
    (score, model.gekisou_ranges())
}

fn contains(estimate: [f64; 2], value: f64) -> bool {
    estimate[0] - estimate[1] <= value && value <= estimate[0] + estimate[1]
}

fn checked(check: &chartstats::ExpectationCheck) {
    let error = (check.expected[0] - check.predicted[0]).abs() + check.expected[1] + check.predicted[1];
    assert!(error <= check.bound + 1e-8, "{check:?}");
}

#[test]
fn expectation_without_ranges_matches_deterministic_free_score() {
    let (d, _) = data(120);
    let kinds = chartstats::kinds(&d.master);
    let s = chartstats::chart_stats(&d.master, &d.charts[0], &kinds, 8).unwrap();
    let e = s.expectation.as_ref().unwrap();
    assert_eq!(s.replay_seeds, [0]);
    assert!(e.ranges.is_empty() && s.ranges.is_empty());
    assert_eq!(e.score, e.score_perfect);
    assert!(contains(e.score, s.off_seeds[0].score as f64));
    for (on, off) in e.weights.iter().zip(&s.off_seeds[0].weights) {
        for (&on, &off) in on.iter().zip(off.as_ref().unwrap()) {
            assert!(contains(on, off));
        }
    }
    assert!(e.rank_check.is_none());
    checked(&e.check);
}

#[test]
fn luck_expectations_keep_replay_seeds_and_additive_indicator_contracts() {
    let (d, _) = data_fevers(220, &FEVERS);
    let kinds = chartstats::kinds(&d.master);
    let s = chartstats::chart_stats(&d.master, &d.charts[0], &kinds, 3).unwrap();
    let e = s.expectation.as_ref().unwrap();
    assert_eq!(s.replay_seeds, ournotes_sim::live::seeds::published_seeds(3));
    assert_eq!((s.positions, e.ranges.len(), e.weights.len()), (5, 3, kinds.len()));
    for (i, r) in e.ranges.iter().enumerate() {
        assert!(r.range_score[0] > 0.0 && r.max_combo > 0);
        if i != 1 {
            assert_eq!(r.luck_points, [0.0; 2]);
            assert_eq!(r.lot_results, [[0.0; 2]; 4]);
        }
        let [_, hit, super_hit, critical] = r.lot_results;
        let points = 5.0 * hit[0] + 10.0 * (super_hit[0] + critical[0]);
        let radius = 5.0 * hit[1] + 10.0 * (super_hit[1] + critical[1]);
        assert!((points - r.luck_points[0]).abs() <= radius + r.luck_points[1] + 1e-8);
    }
    assert!(e.ranges[1].luck_points[0] > 0.0);
    assert_eq!(e.ranges[2].just_count, s.just_notes);
    assert!(e.score_perfect[0] < e.score[0]);
    checked(&e.check);
    checked(e.rank_check.as_ref().unwrap());
    let doc = chartstats::document(&d, Some(3)).unwrap();
    assert_eq!(doc["format"], "ournotes-deck.chart-stats/3");
    assert_eq!(doc["charts"][0]["replaySeeds"], serde_json::to_value(s.replay_seeds).unwrap());
    assert_eq!(doc["charts"][0]["expectation"], serde_json::to_value(e).unwrap());
}

#[test]
fn fourth_fever_preserves_free_live_measurements() {
    let (d, _) = data_fevers(120, &[(8000, 16000), (24000, 32000), (42000, 50000), (55000, 57000)]);
    let s = chartstats::chart_stats(&d.master, &d.charts[0], &chartstats::kinds(&d.master), 3).unwrap();
    assert!(s.unplayable.is_some());
    assert!(s.expectation.is_none() && s.replay_seeds.is_empty());
    assert_eq!(s.off_seeds.len(), 1);
    assert!(s.off_seeds[0].score > 0);
}

#[test]
fn confirmed_rank_conditions_keep_explicit_range_weight_domains() {
    let (d, _) = data_with(120, &FEVERS, true);
    let kinds = chartstats::kinds(&d.master);
    let stats = chartstats::chart_stats(&d.master, &d.charts[0], &kinds, 2).unwrap();
    let weights = stats.expectation.as_ref().unwrap().range_weights.as_ref().unwrap();
    let rank = kinds.iter().position(|k| k.skill_condition_group == 20).unwrap();
    let combo = kinds.iter().position(|k| k.skill_condition_group == 21).unwrap();
    assert!(weights[rank].is_none());
    assert!(weights[combo].is_some());
    assert!(stats.off_seeds[0].weights[rank].as_ref().unwrap().iter().all(|&w| w == 0.0));
    assert!(stats.off_seeds[0].weights[combo].is_none());
}

#[test]
fn deterministic_skill_gains_match_independent_full_runs_and_exact_tail() {
    let mut d = aptitude_data(&APT_FEVERS[..1]);
    let shapes = chartstats::shapes(&d.master);
    let stats = chartstats::chart_stats(&d.master, &d.charts[0], &chartstats::kinds(&d.master), 4).unwrap();
    let variants = &stats.gekisou_aptitude.as_ref().unwrap().variants;
    assert_eq!(variants.len(), 5);
    for variant in variants {
        assert_eq!(shapes[variant.shape].mission, 1);
        checked(&variant.check);
        assert_eq!(variant.converted, [0.0; 2]);
        let sum = variant.tail[0] + variant.ranges.iter().map(|r| r.range_score[0] + r.rank_bonus[0]).sum::<f64>();
        assert!((variant.score[0] - sum).abs() < 1e-8);
        assert_eq!(variant.weights.as_ref().unwrap().len(), 5);
    }
    d.master.gekisou_skills.push(ournotes_sim::master::SkillRow {
        id: -71,
        gekisou_mission_type: 1,
        ..Default::default()
    });
    d.master.reindex().unwrap();
    let support = shape_for(&shapes, "support", 1);
    for band in [false, true] {
        let v = variants.iter().find(|v| v.shape == support && v.band_match == Some(band)).unwrap();
        let p = Performer {
            gekisou_skill: Some((-71, 1)),
            gekisou_mission_type: 1,
            gekisou_support_skills: vec![(1, 3)],
            band_id: if band { 1 } else { 0 },
            ..Default::default()
        };
        let (base, base_ranges) = aptitude_run(&d, None, 0, false);
        let (with, ranges) = aptitude_run(&d, Some(p), 0, false);
        let gain = with as f64 - base as f64;
        assert!(contains(v.score, gain), "band {band}: {gain} vs {:?}", v.score);
        let tail = gain
            - ranges
                .iter()
                .zip(base_ranges)
                .map(|(r, b)| {
                    (r.end_score - r.start_score - b.end_score + b.start_score) as f64
                        + (r.rank_bonus.unwrap() - b.rank_bonus.unwrap()) as f64
                })
                .sum::<f64>();
        assert!(contains(v.tail, tail));
    }
}

#[test]
fn aptitude_all_missions_publish_nominal_checks_and_complete_cross_terms() {
    let d = aptitude_data(&APT_FEVERS);
    let stats = chartstats::chart_stats(&d.master, &d.charts[0], &chartstats::kinds(&d.master), 4).unwrap();
    let aptitude = stats.gekisou_aptitude.as_ref().unwrap();
    assert_eq!(aptitude.factors.len(), 3);
    assert_eq!(aptitude.variants.len(), 9);
    for v in &aptitude.variants {
        assert_eq!(v.ranges.len(), 3);
        assert_eq!(v.weights.as_ref().unwrap().len(), 5);
        assert!(v.range_weights.as_ref().unwrap().iter().all(|r| r.len() == 3));
        checked(&v.check);
    }
    let baseline_lots = stats.expectation.as_ref().unwrap().ranges[1].lot_results.iter().map(|v| v[0]).sum::<f64>();
    assert!(contains(aptitude.factors[1].lotteries, baseline_lots));
    let disabled =
        chartstats::document_with(&d, &chartstats::Options { aptitude: false, ..Default::default() }).unwrap();
    assert_eq!(disabled["gekisouAptitude"], Value::Null);
    assert_eq!(disabled["charts"][0]["gekisouAptitude"], Value::Null);
    assert_eq!(disabled["charts"][0]["expectation"], serde_json::to_value(stats.expectation.unwrap()).unwrap());
}

#[test]
fn aptitude_without_plain_kind_keeps_expected_gains() {
    let d = aptitude_data(&APT_FEVERS[..1]);
    let stats = chartstats::chart_stats(&d.master, &d.charts[0], &[], 4).unwrap();
    for v in &stats.gekisou_aptitude.unwrap().variants {
        assert!(v.weights.is_none() && v.range_weights.is_none());
        checked(&v.check);
    }
}

#[test]
fn replay_count_does_not_change_nominal_expectations() {
    let (d, _) = data_fevers(120, &FEVERS);
    let kinds = chartstats::kinds(&d.master);
    let a = chartstats::chart_stats(&d.master, &d.charts[0], &kinds, 2).unwrap();
    let b = chartstats::chart_stats(&d.master, &d.charts[0], &kinds, 8).unwrap();
    assert_eq!(a.expectation, b.expectation);
    assert_eq!(a.gekisou_aptitude, b.gekisou_aptitude);
    assert_eq!(a.replay_seeds.len(), 2);
    assert_eq!(b.replay_seeds.len(), 8);
}

#[test]
fn command_line_selects_charts_and_replay_count() {
    let dir = std::env::temp_dir().join(format!("chart-expectation-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("data.json");
    let mut rng = common::Rng::new(17);
    let charts = [1002, 1004].into_iter().map(|id| chart_json_fevers(id, 40, &mut rng, &FEVERS).0).collect();
    std::fs::write(&path, document_with(charts, false).to_string()).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_ournotes-deck"))
            .args(["chart-stats", "--data", path.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    };
    let one = run(&["--seeds", "2"]);
    assert!(one.status.success(), "{}", String::from_utf8_lossy(&one.stderr));
    let one: Value = serde_json::from_slice(&one.stdout).unwrap();
    let parallel = run(&["--seeds", "2", "--jobs", "2"]);
    assert!(parallel.status.success(), "{}", String::from_utf8_lossy(&parallel.stderr));
    assert_eq!(one, serde_json::from_slice::<Value>(&parallel.stdout).unwrap());
    let kept = run(&["--seeds", "2", "--charts", "1004"]);
    let kept: Value = serde_json::from_slice(&kept.stdout).unwrap();
    assert_eq!(kept["charts"].as_array().unwrap().len(), 1);
    assert_eq!(kept["charts"][0]["replaySeeds"].as_array().unwrap().len(), 2);
    assert!(!run(&["--seeds", "0"]).status.success());
    std::fs::remove_dir_all(dir).unwrap();
}
