//! One live in every performance order: playing the orders' common frames once gives each order exactly the result
//! of its own whole simulation. Synthetic tables; every number here is made up.

use std::cell::RefCell;
use std::ops::ControlFlow;

use ournotes_sim::error::Error;
use ournotes_sim::live::full::{
    GekisouRange, GekisouSetup, JudgedNote, LiveModel, LiveNote, LiveParams, LivePlay, OrderSharing, OrderedLive,
    OrdersOutcome, Performer, PlayFrame,
};
use ournotes_sim::live::random::LiveRandom;
use ournotes_sim::master::Master;
use serde_json::{Value, json};

const LENGTH: i32 = 12_000;

fn master_from(tables: &Value) -> Master {
    let texts: Vec<(String, String)> =
        tables.as_object().unwrap().iter().map(|(k, v)| (k.clone(), json!({ "_allData": v }).to_string())).collect();
    Master::from_json_tables(|n| texts.iter().find(|(k, _)| k == n).map(|(_, t)| t.as_str())).unwrap()
}

fn row(key: &str, id: i64, skill: i64, effect_type: i64, value: i64, extra: Value) -> Value {
    let mut r = json!({"_id": id, key: skill, "_level": 1, "_skillTriggerType": 1,
        "_skillTriggerConditionGroup": 0, "_skillConditionGroup": 0, "_skillReleaseConditionGroup": 0,
        "_skillTargetIDs": [], "_skillEffectType": effect_type, "_activationTimeSecond": 0.0, "_effectValue": value,
        "_maxEffectValue": 0, "_effectLimitCount": 0, "_skillCumulativeConditionID": 0,
        "_effectExecuteLimitCount": 0, "_effectExecuteLimitResetConditionGroup": 0});
    for (k, v) in extra.as_object().unwrap() {
        r[k] = v.clone();
    }
    r
}

fn condition(id: i64, ty: i64, values: Value, targets: Value) -> Value {
    json!({"_id": id, "_conditionType": ty, "_conditionValues": values, "_isPositive": true,
           "_conditionTargetIDs": targets})
}

fn tables() -> Value {
    let live = |id: i64, ty: i64, value: i64, act: f64, targets: Value| {
        json!({"_id": id, "_liveSkillID": id, "_level": 1, "_skillConditionGroup": 0,
               "_skillReleaseConditionGroup": 0, "_skillTargetIDs": targets, "_skillEffectType": ty,
               "_activationTimeSecond": act, "_effectValue": value, "_maxEffectValue": 0, "_effectLimitCount": 0,
               "_skillCumulativeConditionID": 0, "_effectExecuteLimitCount": 0,
               "_effectExecuteLimitResetConditionGroup": 0})
    };
    let snap = |id, ty, value, extra| row("_supportSkillID", id, id, ty, value, extra);
    let gk = |id, ty, value, extra| row("_gekisouSkillID", id, id, ty, value, extra);
    let gks = |id, ty, value, extra| row("_gekisouSupportSkillID", id, id, ty, value, extra);
    let lots = [(0, [10, 26, 20, 44]), (4, [0, 0, 100, 900]), (3, [0, 0, 300, 700]), (2, [0, 0, 500, 500])]
        .into_iter()
        .chain([(1, [0, 0, 700, 300])])
        .flat_map(|(kind, weights)| {
            (0..4).map(move |result| {
                json!({"_id": kind * 4 + result + 1, "_chanceLotType": kind, "_lotResult": result,
                       "_weight": weights[result as usize]})
            })
        })
        .collect::<Vec<_>>();
    let ranks: Vec<Value> = (1..=3)
        .map(|c| json!({"_id": c, "_missionPattern": 2, "_count": c, "_rank": 1, "_scoreBonusPercent": 370}))
        .collect();
    json!({
        "MasterLiveNoteParameter": [{"_id": 1, "_noteOperateType": 1, "_scorePercent": 100}],
        "MasterLiveJudgementParameter": [
            {"_id": 1, "_noteSimulateJudgement": 6, "_scorePercent": 200, "_damage": 0},
            {"_id": 2, "_noteSimulateJudgement": 5, "_scorePercent": 100, "_damage": 0},
            {"_id": 3, "_noteSimulateJudgement": 4, "_scorePercent": 80, "_damage": 10},
            {"_id": 4, "_noteSimulateJudgement": 3, "_scorePercent": 50, "_damage": 20},
            {"_id": 5, "_noteSimulateJudgement": 2, "_scorePercent": 0, "_damage": 50},
            {"_id": 6, "_noteSimulateJudgement": 1, "_scorePercent": 0, "_damage": 100}],
        "MasterLiveSettings": [
            {"_id": 1, "_key": "note_score_adjustment_factor", "_value": "3"},
            {"_id": 2, "_key": "note_score_life_onus_factor", "_value": "0.5"},
            {"_id": 3, "_key": "life_base", "_value": "1000"},
            {"_id": 4, "_key": "life_denger", "_value": "300"},
            {"_id": 5, "_key": "gekisou_luck_gauge_max", "_value": "30"},
            {"_id": 6, "_key": "gekisou_luck_gauge_max_rush", "_value": "15"},
            {"_id": 7, "_key": "gekisou_luck_rush_score_bonus_percent", "_value": "10"}],
        "MasterLiveComboScoreBonus": [
            {"_id": 1, "_comboBonusType": 0, "_requiredComboCount": 10, "_bonusFactor": 0.01},
            {"_id": 2, "_comboBonusType": 0, "_requiredComboCount": 20, "_bonusFactor": 0.01}],
        "MasterSkillEffectSetting": [
            {"_id": 1, "_skillEffectType": 2000, "_phase": 2}, {"_id": 2, "_skillEffectType": 2002, "_phase": 2},
            {"_id": 3, "_skillEffectType": 2004, "_phase": 2}, {"_id": 4, "_skillEffectType": 3001, "_phase": 1},
            {"_id": 5, "_skillEffectType": 11001, "_phase": 2}, {"_id": 6, "_skillEffectType": 11003, "_phase": 2},
            {"_id": 7, "_skillEffectType": 11005, "_phase": 2}],
        "MasterLiveJudgementTiming": [
            {"_id": 1, "_noteJudgementType": 1, "_noteSimulateJudgement": 6, "_afterMs": 100},
            {"_id": 2, "_noteJudgementType": 1, "_noteSimulateJudgement": 5, "_afterMs": 100},
            {"_id": 3, "_noteJudgementType": 1, "_noteSimulateJudgement": 4, "_afterMs": 100}],
        "MasterLiveGekisouRankingScoreBonus": ranks,
        "MasterLiveGekisouLuckBasePoint": [
            {"_id": 1, "_noteCategory": 0, "_noteSimulateJudgement": 5, "_weight": 200, "_basePoint": 9},
            {"_id": 2, "_noteCategory": 0, "_noteSimulateJudgement": 5, "_weight": 300, "_basePoint": 8},
            {"_id": 3, "_noteCategory": 0, "_noteSimulateJudgement": 5, "_weight": 500, "_basePoint": 7},
            {"_id": 4, "_noteCategory": 0, "_noteSimulateJudgement": 4, "_weight": 1, "_basePoint": 5}],
        "MasterLiveGekisouLuckBonusLot": lots,
        "MasterSkillTarget": [
            {"_id": 3, "_skillTargetType": 3, "_bandID": 1},
            {"_id": 42, "_skillTargetType": 4, "_judgement": 5},
            {"_id": 56, "_skillTargetType": 5, "_gekisouMissionType": 2}],
        "MasterSkillCondition": [
            condition(61, 4010, json!([]), json!([])), condition(62, 4002, json!([15]), json!([])),
            condition(63, 4001, json!([25]), json!([])), condition(64, 4011, json!([50]), json!([])),
            condition(65, 8000, json!([]), json!([])), condition(66, 4000, json!([7]), json!([])),
            condition(70, 5000, json!([]), json!([3])), condition(80, 7010, json!([]), json!([56])),
            condition(81, 7020, json!([]), json!([56])), condition(82, 7021, json!([]), json!([])),
            condition(83, 7000, json!([0]), json!([])), condition(84, 7013, json!([]), json!([])),
            condition(85, 4011, json!([40]), json!([]))],
        "MasterSkillConditionSet": [
            {"_id": 1, "_group": 53, "_conditionIds": [61]}, {"_id": 2, "_group": 54, "_conditionIds": [62]},
            {"_id": 3, "_group": 55, "_conditionIds": [63]}, {"_id": 4, "_group": 56, "_conditionIds": [64]},
            {"_id": 5, "_group": 57, "_conditionIds": [65]}, {"_id": 6, "_group": 58, "_conditionIds": [66]},
            {"_id": 7, "_group": 70, "_conditionIds": [70]}, {"_id": 8, "_group": 90, "_conditionIds": [80]},
            {"_id": 9, "_group": 91, "_conditionIds": [81]}, {"_id": 10, "_group": 92, "_conditionIds": [82]},
            {"_id": 11, "_group": 93, "_conditionIds": [83]}, {"_id": 12, "_group": 94, "_conditionIds": [84]},
            {"_id": 13, "_group": 95, "_conditionIds": [85, 70]}],
        "MasterLiveSkillEffect": [
            live(1, 2000, 1000, 3.0, json!([])), live(2, 2002, 500, 4.0, json!([])),
            live(3, 3001, 200, 0.0, json!([])), live(4, 2004, 300, 3.0, json!([42])),
            live(5, 2000, 700, 2.0, json!([]))],
        "MasterSupportSkillEffect": [
            snap(1, 2000, 500, json!({"_skillTriggerConditionGroup": 54, "_activationTimeSecond": 2.0,
                "_effectExecuteLimitCount": 1})),
            snap(2, 2002, 300, json!({"_skillTriggerConditionGroup": 53, "_activationTimeSecond": 2.0})),
            snap(3, 2000, 700, json!({"_skillTriggerConditionGroup": 55, "_skillConditionGroup": 56,
                "_activationTimeSecond": 1.5})),
            snap(4, 2000, 900, json!({"_skillTriggerConditionGroup": 57, "_activationTimeSecond": 1.0})),
            snap(5, 2004, 200, json!({"_skillTriggerConditionGroup": 58, "_activationTimeSecond": 2.0,
                "_skillTargetIDs": [42]}))],
        "MasterGekisouSkill": [{"_id": 1, "_gekisouMissionType": 2}, {"_id": 2, "_gekisouMissionType": 2}],
        "MasterGekisouSkillEffect": [
            gk(1, 11001, 10_000, json!({"_skillTriggerType": 2, "_skillTriggerConditionGroup": 91})),
            gk(2, 11001, 20_000, json!({"_skillTriggerConditionGroup": 90, "_activationTimeSecond": 2.0}))],
        "MasterGekisouSupportSkill": [
            {"_id": 1, "_gekisouMissionType": 2}, {"_id": 2, "_gekisouMissionType": 2},
            {"_id": 3, "_gekisouMissionType": 2}],
        "MasterGekisouSupportSkillEffect": [
            gks(1, 11005, 3, json!({"_skillTriggerConditionGroup": 90, "_skillConditionGroup": 95,
                "_skillReleaseConditionGroup": 94, "_effectLimitCount": 1})),
            gks(2, 2000, 2000, json!({"_skillTriggerType": 2, "_skillTriggerConditionGroup": 92,
                "_skillConditionGroup": 70})),
            gks(3, 11003, 5000, json!({"_skillTriggerConditionGroup": 93, "_skillConditionGroup": 70,
                "_skillReleaseConditionGroup": 94, "_effectExecuteLimitCount": 1,
                "_effectExecuteLimitResetConditionGroup": 94}))],
    })
}

fn performers(gekisou: bool) -> Vec<Performer> {
    (0..5)
        .map(|k| Performer {
            live_skill: Some((k as i64 + 1, 1)),
            support_skills: vec![(k as i64 + 1, 1)],
            band_id: 1 + (k as i64 % 2),
            character_id: k as i64 + 1,
            card_type: 1,
            gekisou_mission_type: 2,
            gekisou_skill: gekisou.then_some((1 + (k as i64 % 2), 1)),
            gekisou_support_skills: if gekisou && k < 3 { vec![(k as i64 + 1, 1)] } else { vec![] },
            ..Performer::default()
        })
        .collect()
}

fn live(gekisou: bool) -> OrderedLive {
    let notes: Vec<LiveNote> = (0..100)
        .map(|i| LiveNote { note_id: i + 1, time_ms: 1000 + 100 * i, note_operate_type: 1, judgement_type: 1 })
        .collect();
    let mut frames: Vec<PlayFrame> =
        (0..LENGTH / 16).map(|k| PlayFrame { time_ms: 16 * k, judged: Vec::new() }).collect();
    for n in &notes {
        let fi = frames.iter().position(|f| f.time_ms >= n.time_ms).unwrap();
        let judgement = match n.note_id % 13 {
            0 => 4,
            k if k % 2 == 0 => 6,
            _ => 5,
        };
        frames[fi].judged.push(JudgedNote { note_id: n.note_id, judgement, judgement_time_ms: n.time_ms });
    }
    let delta_times = vec![0.016f32; frames.len()];
    OrderedLive {
        performers: performers(gekisou),
        notes,
        events: vec![(0, 2500), (1, 4100), (2, 4100), (3, 6300), (4, 8800), (0, 9600)],
        params: LiveParams {
            skill_target_music_type: 0,
            total_power: 200_000,
            music_level: 25,
            converted_note_count: 100,
            music_length_ms: LENGTH,
            score_music_length_ms: None,
            assist_factor: 1.0,
        },
        gekisou: gekisou.then(|| GekisouSetup { fevers: vec![(5000, 7500)], missions: vec![2, 1, 3] }),
        rank_confirmations: None,
        play: LivePlay { frames, base_seed: 0 },
        delta_times,
        lottery_free: None,
    }
}

#[derive(Debug, PartialEq)]
struct Outcome {
    score: i32,
    trace: Vec<(i32, i32)>,
    ranges: Vec<GekisouRange>,
    bonuses: Vec<(usize, i32, i32, i64)>,
    draws: u64,
    life: i32,
    factors: String,
}

fn outcome(m: &LiveModel) -> Outcome {
    Outcome {
        score: m.score(),
        trace: m.trace().to_vec(),
        ranges: m.gekisou_ranges(),
        bonuses: m.gekisou_rank_bonuses().to_vec(),
        draws: m.draws(),
        life: m.current_life(),
        factors: format!("{:?}", m.factor_state()),
    }
}

fn all_orders(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for rest in all_orders(n - 1) {
        for at in 0..=rest.len() {
            let mut o = rest.clone();
            o.insert(at, n - 1);
            out.push(o);
        }
    }
    out
}

fn check_every_order(gekisou: bool, seed: i32) {
    let master = master_from(&tables());
    let live = live(gekisou);
    let orders = all_orders(5);
    let random = LiveRandom::new(seed);
    let mut shared: Vec<Option<Outcome>> = (0..orders.len()).map(|_| None).collect();
    let sharing = live
        .simulate_orders(&master, &orders, random.clone(), |i, m| {
            assert!(shared[i].replace(outcome(m)).is_none());
            Ok(())
        })
        .unwrap();
    let mut scores = Vec::new();
    for (i, order) in orders.iter().enumerate() {
        let alone = outcome(&live.simulate(&master, order, random.clone()).unwrap());
        assert_eq!(shared[i].as_ref(), Some(&alone), "order {order:?}");
        scores.push(alone.score);
    }
    scores.sort_unstable();
    scores.dedup();
    assert!(scores.len() > 1, "the fixture depends on the order");
    assert!(sharing.branches > 0 && sharing.frames < sharing.separate_frames, "{sharing:?}");
}

#[test]
fn shared_frames_give_each_order_its_own_result() {
    check_every_order(false, 1);
}

#[test]
fn settled_prefix_preserves_scores_across_capped_effect_finishes() {
    use ournotes_sim::live::score::get_frame;

    let master = master_from(&tables());
    let performers = [Performer { live_skill: Some((1, 1)), ..Default::default() }];
    let notes = [
        LiveNote { note_id: 1, time_ms: 40, note_operate_type: 1, judgement_type: 1 },
        LiveNote { note_id: 2, time_ms: 600, note_operate_type: 1, judgement_type: 1 },
    ];
    let params = LiveParams {
        total_power: 200_000,
        music_level: 5,
        converted_note_count: 2,
        music_length_ms: 500,
        score_music_length_ms: None,
        skill_target_music_type: 0,
        assist_factor: 1.0,
    };
    let mut model = LiveModel::new(&master, &performers, &notes, &[(0, 0)], params).unwrap();
    model.frame_timed(0, &[], 0.0).unwrap();
    for note in notes {
        model
            .frame_timed(
                note.time_ms,
                &[JudgedNote { note_id: note.note_id, judgement: 5, judgement_time_ms: note.time_ms }],
                0.04,
            )
            .unwrap();
    }
    model.frame_timed(1000, &[], 0.04).unwrap();
    let before_finish = model.score();
    let settled = model.settle(i32::MAX);
    // The three-second skill ends at the music length when its duration elapses.
    model.frame_timed(3040, &[], 0.04).unwrap();
    assert!(model.score() < before_finish);
    assert!(settled.total <= i64::from(model.score()), "settled={settled:?}, final={}", model.score());
    assert_eq!(settled.frame, get_frame(params.music_length_ms));
    #[cfg(feature = "search-diagnostics")]
    {
        let final_prefix: i64 = model
            .filed_scores()
            .0
            .iter()
            .filter(|note| get_frame(note.0) < settled.frame)
            .map(|note| i64::from(note.2))
            .sum();
        assert_eq!(settled.total, final_prefix);
        assert_eq!(model.settled_violations(), 0);
    }
}

#[test]
fn settled_prefix_preserves_scores_with_signed_duration_adjustments() {
    #[cfg(feature = "search-diagnostics")]
    use ournotes_sim::live::score::get_frame;

    for score_up in [1000, -1000] {
        let mut data = tables();
        data["MasterSkillEffectSetting"]
            .as_array_mut()
            .unwrap()
            .push(json!({"_id": 8, "_skillEffectType": 15000, "_phase": 2}));
        data["MasterSkillCondition"][1]["_conditionValues"] = json!([2]);
        data["MasterSupportSkillEffect"][0]["_skillEffectType"] = json!(15000);
        data["MasterSupportSkillEffect"][0]["_effectValue"] = json!(-2500);
        data["MasterSupportSkillEffect"][0]["_activationTimeSecond"] = json!(0.0);
        data["MasterLiveSkillEffect"][0]["_effectValue"] = json!(score_up);
        let master = master_from(&data);
        let performers = [Performer { live_skill: Some((1, 1)), support_skills: vec![(1, 1)], ..Default::default() }];
        let notes = [
            LiveNote { note_id: 1, time_ms: 40, note_operate_type: 1, judgement_type: 1 },
            LiveNote { note_id: 2, time_ms: 560, note_operate_type: 1, judgement_type: 1 },
        ];
        let params = LiveParams {
            total_power: 200_000,
            music_level: 5,
            converted_note_count: 2,
            music_length_ms: 5000,
            score_music_length_ms: None,
            skill_target_music_type: 0,
            assist_factor: 1.0,
        };
        let mut model = LiveModel::new(&master, &performers, &notes, &[(0, 0)], params).unwrap();
        model.frame_timed(0, &[], 0.0).unwrap();
        model.frame_timed(40, &[JudgedNote { note_id: 1, judgement: 5, judgement_time_ms: 40 }], 0.04).unwrap();
        model.frame_timed(80, &[], 0.04).unwrap();
        model.frame_timed(600, &[JudgedNote { note_id: 2, judgement: 5, judgement_time_ms: 600 }], 0.52).unwrap();
        let before_finish = model.score();
        let settled = model.settle(i32::MAX);
        model.frame_timed(640, &[], 0.04).unwrap();
        assert!(if score_up > 0 { model.score() < before_finish } else { model.score() > before_finish });
        assert_eq!((settled.frame, settled.total, settled.fixed), (0, 0, 0));
        #[cfg(feature = "search-diagnostics")]
        {
            let final_prefix: i64 = model
                .filed_scores()
                .0
                .iter()
                .filter(|note| get_frame(note.0) < settled.frame)
                .map(|note| i64::from(note.2))
                .sum();
            assert_eq!(settled.total, final_prefix);
            assert_eq!(model.settled_violations(), 0);
        }
    }
}

#[test]
fn settled_prefix_keeps_the_last_addressable_score_frame_open() {
    use ournotes_sim::live::score::get_frame;

    let master = master_from(&tables());
    let notes = [
        LiveNote { note_id: 1, time_ms: 2000, note_operate_type: 1, judgement_type: 1 },
        LiveNote { note_id: 2, time_ms: 5100, note_operate_type: 1, judgement_type: 1 },
    ];
    let params = LiveParams {
        total_power: 200_000,
        music_level: 5,
        converted_note_count: 2,
        music_length_ms: 10_000,
        score_music_length_ms: Some(40),
        skill_target_music_type: 0,
        assist_factor: 1.0,
    };
    let mut model = LiveModel::new(&master, &[], &notes, &[], params).unwrap();
    model.frame_timed(2000, &[JudgedNote { note_id: 1, judgement: 5, judgement_time_ms: 2000 }], 2.0).unwrap();
    model.frame_timed(5000, &[], 3.0).unwrap();
    let settled = model.settle(notes[1].time_ms);
    model.frame_timed(5100, &[JudgedNote { note_id: 2, judgement: 5, judgement_time_ms: 5100 }], 0.1).unwrap();
    assert_eq!(settled.total, 0);
    assert_eq!(settled.frame, get_frame(notes[0].time_ms));
    #[cfg(feature = "search-diagnostics")]
    assert_eq!(model.settled_violations(), 0);
}

#[test]
fn settled_prefix_preserves_scores_across_delayed_rank_arrivals() {
    use ournotes_sim::live::score::get_frame;
    use ournotes_sim::replay::RankConfirmation;

    let master = master_from(&tables());
    let mut live = live(true);
    live.rank_confirmations = Some(vec![RankConfirmation { frame: 690, range: 0, rank: 1, percent: 370 }]);
    let mut model = live.model(&master, &[0, 1, 2, 3, 4]).unwrap();
    model.set_seed(3);
    model.play_frames(&live.play, &live.delta_times, 690).unwrap();
    assert!(model.gekisou_rank_bonuses().is_empty());
    let settled = model.settle(i32::MAX);
    model.play_frames(&live.play, &live.delta_times, live.play.frames.len()).unwrap();
    assert_eq!(model.gekisou_rank_bonuses().len(), 1);
    assert!(model.gekisou_rank_bonuses()[0].2 > 0);
    #[cfg(feature = "search-diagnostics")]
    {
        let (notes, fixed) = model.filed_scores();
        let final_prefix = notes
            .iter()
            .filter(|note| get_frame(note.0) < settled.frame)
            .map(|note| i64::from(note.2))
            .chain(fixed.iter().filter(|&&(frame, _)| frame < settled.frame).map(|&(_, score)| i64::from(score)))
            .sum::<i64>();
        assert_eq!(settled.total, final_prefix);
        assert_eq!(model.settled_violations(), 0);
    }
    assert_eq!(settled.frame, get_frame(live.gekisou.as_ref().unwrap().fevers[0].1));
}

#[test]
fn shared_frames_give_each_order_its_own_result_with_gekisou_lotteries() {
    for seed in [3, 11] {
        check_every_order(true, seed);
    }
}

#[test]
fn shared_orders_preserve_wrapping_effect_key_order() {
    let mut tables = tables();
    tables["MasterSkillEffectSetting"]
        .as_array_mut()
        .unwrap()
        .push(json!({"_id": 8, "_skillEffectType": 3002, "_phase": 1}));
    let trigger = json!({"_skillTriggerConditionGroup": 53});
    tables["MasterSupportSkillEffect"] = json!([
        row("_supportSkillID", 1, 9, 3002, 1500, trigger.clone()),
        row("_supportSkillID", 2_674_777_890_687_884_984, 9, 3001, 500, trigger)
    ]);
    let master = master_from(&tables);
    let mut live = live(false);
    for p in &mut live.performers {
        p.live_skill = None;
        p.support_skills.clear();
    }
    live.performers[0].support_skills.push((9, 1));
    live.notes = vec![LiveNote { note_id: 1, time_ms: 200, note_operate_type: 1, judgement_type: 1 }];
    live.events = vec![(0, 100), (2, 100)];
    live.params.converted_note_count = 1;
    live.params.music_length_ms = 1000;
    live.play.frames = vec![
        PlayFrame { time_ms: 0, judged: Vec::new() },
        PlayFrame { time_ms: 100, judged: Vec::new() },
        PlayFrame { time_ms: 200, judged: vec![JudgedNote { note_id: 1, judgement: 5, judgement_time_ms: 200 }] },
    ];
    live.delta_times = vec![0.1; live.play.frames.len()];
    let mut orders = vec![vec![0, 1, 2, 3, 4], vec![2, 1, 0, 3, 4]];
    // At position 0 damage precedes recovery (life 501); at position 2 the
    // wrapped recovery key is i64::MIN and precedes damage (life 1).
    for _ in 0..2 {
        let mut seen = vec![false; orders.len()];
        let sharing = live
            .simulate_orders(&master, &orders, LiveRandom::new(5), |i, shared| {
                assert!(!std::mem::replace(&mut seen[i], true));
                let separate = live.simulate(&master, &orders[i], LiveRandom::new(5))?;
                assert_eq!(separate.current_life(), if orders[i][0] == 0 { 501 } else { 1 });
                assert_eq!(outcome(shared), outcome(&separate), "order {:?}", orders[i]);
                Ok(())
            })
            .unwrap();
        assert!(seen.iter().all(|&visited| visited));
        assert!(sharing.branches > 0);
        orders.reverse();
    }
}

#[cfg(feature = "search-diagnostics")]
#[test]
fn applier_event_plan_matches_reference_after_every_frame() {
    use ournotes_sim::live::full::with_applier_plan_disabled;
    use ournotes_sim::replay::RankConfirmation;

    let master = master_from(&tables());
    for (gekisou, external) in [(false, false), (true, false), (true, true)] {
        let live = live(gekisou);
        for order in [[0, 1, 2, 3, 4], [4, 3, 2, 1, 0], [2, 4, 0, 3, 1]] {
            let performers: Vec<_> = order.iter().map(|&slot| live.performers[slot].clone()).collect();
            let build = || {
                let mut model = match (&live.gekisou, external) {
                    (Some(setup), true) => LiveModel::new_gekisou_external(
                        &master,
                        &performers,
                        &live.notes,
                        &live.events,
                        live.params,
                        setup,
                    ),
                    (Some(setup), false) => {
                        LiveModel::new_gekisou(&master, &performers, &live.notes, &live.events, live.params, setup)
                    }
                    (None, _) => LiveModel::new(&master, &performers, &live.notes, &live.events, live.params),
                }
                .unwrap();
                if external {
                    model
                        .set_rank_confirmation_timeline(&[RankConfirmation {
                            frame: 690,
                            range: 0,
                            rank: 1,
                            percent: 370,
                        }])
                        .unwrap();
                }
                model
            };
            for seed in [3, 11, -17] {
                let mut fast = build();
                let mut reference = with_applier_plan_disabled(build);
                fast.set_seed(seed);
                reference.set_seed(seed);
                for (index, (frame, &dt)) in live.play.frames.iter().zip(&live.delta_times).enumerate() {
                    fast.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
                    reference.frame_timed(frame.time_ms, &frame.judged, dt).unwrap();
                    // Includes every previous score query, life, raw float factor state, range snapshots,
                    // late rank bonuses and RNG draw count, not just the final score.
                    assert_eq!(
                        outcome(&fast),
                        outcome(&reference),
                        "gk={gekisou} external={external} order={order:?} seed={seed} frame={index}"
                    );
                }
            }
        }
    }
}

#[test]
fn network_rank_timeline_survives_shared_prefix_clones_for_all_orders() {
    use ournotes_sim::replay::RankConfirmation;
    let master = master_from(&tables());
    let orders = all_orders(5);
    for frame in [0, 690] {
        let mut live = live(true);
        let confirmations = vec![RankConfirmation { frame, range: 0, rank: 1, percent: 370 }];
        live.rank_confirmations = Some(confirmations.clone());
        let mut visited = vec![false; orders.len()];
        live.simulate_orders(&master, &orders, LiveRandom::new(11), |i, shared| {
            assert!(!std::mem::replace(&mut visited[i], true));
            // Independent adapter: inject packets manually before native frames, without the new timeline helper.
            let performers: Vec<_> = orders[i].iter().map(|&s| live.performers[s].clone()).collect();
            let mut independent = LiveModel::new_gekisou_external(
                &master,
                &performers,
                &live.notes,
                &live.events,
                live.params,
                live.gekisou.as_ref().unwrap(),
            )?;
            independent.set_random(LiveRandom::new(11));
            for (f, (play, dt)) in live.play.frames.iter().zip(&live.delta_times).enumerate() {
                if f == frame {
                    independent.queue_gekisou_rank_confirmation(0, 1, 370)?;
                }
                independent.frame_timed(play.time_ms, &play.judged, *dt)?;
            }
            assert_eq!(outcome(shared), outcome(&independent), "arrival {frame}, order {:?}", orders[i]);
            assert_eq!(shared.rank_confirmation_applications(), independent.rank_confirmation_applications());
            assert_eq!(shared.rank_confirmation_applications().len(), 1);
            assert!(shared.rank_confirmation_applications()[0].0 >= frame);
            Ok(())
        })
        .unwrap();
        assert!(visited.iter().all(|&v| v));
    }
}

#[test]
fn repeated_and_partial_order_lists() {
    let master = master_from(&tables());
    let live = live(true);
    let orders = vec![vec![4, 3, 2, 1, 0], vec![0, 1, 2, 3, 4], vec![4, 3, 2, 1, 0], vec![0, 1, 2, 4, 3]];
    let mut seen = vec![0; orders.len()];
    live.simulate_orders(&master, &orders, LiveRandom::new(5), |i, m| {
        seen[i] += 1;
        let alone = live.simulate(&master, &orders[i], LiveRandom::new(5))?;
        assert_eq!(outcome(m), outcome(&alone));
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, vec![1; orders.len()]);
    assert!(live.simulate_orders(&master, &[vec![0, 1, 2, 3, 3]], LiveRandom::new(5), |_, _| Ok(())).is_err());
}

/// The bound on the payoff sum of all orders, worked out again from the calls a bounded play made: the sets of orders
/// it bounded (each inside an earlier one or apart from it, the first one all orders) with their least bounds, and the
/// payoffs of the orders visited. `seen` holds the bound after every call.
#[derive(Default)]
struct BoundTrace {
    sets: Vec<(Vec<usize>, i128, Option<usize>)>,
    paid: Vec<Option<i128>>,
    seen: Vec<i128>,
}

impl BoundTrace {
    fn bounded(&mut self, ids: &[usize], c: i128) {
        let mut set = ids.to_vec();
        set.sort_unstable();
        if let Some(s) = self.sets.iter_mut().find(|s| s.0 == set) {
            s.1 = s.1.min(c);
        } else {
            let parent = (0..self.sets.len())
                .filter(|&k| self.sets[k].0.len() > set.len() && self.sets[k].0.binary_search(&set[0]).is_ok())
                .min_by_key(|&k| self.sets[k].0.len());
            if let Some(p) = parent {
                assert!(set.iter().all(|i| self.sets[p].0.binary_search(i).is_ok()), "nested sets");
            }
            self.sets.push((set, c, parent));
        }
        self.seen.push(self.bound(0));
    }

    fn visited(&mut self, i: usize, payoff: i128) {
        assert!(self.paid[i].replace(payoff).is_none());
        self.seen.push(self.bound(0));
    }

    fn bound(&self, k: usize) -> i128 {
        let (set, cap, _) = &self.sets[k];
        if let Some(paid) = set.iter().map(|&i| self.paid[i]).sum::<Option<i128>>() {
            return paid;
        }
        let children: Vec<usize> = (0..self.sets.len()).filter(|&j| self.sets[j].2 == Some(k)).collect();
        if children.iter().map(|&j| self.sets[j].0.len()).sum::<usize>() < set.len() {
            return *cap;
        }
        (*cap).min(children.iter().map(|&j| self.bound(j)).sum())
    }
}

type Bounded = (OrdersOutcome, Vec<Option<Outcome>>, BoundTrace);

/// A bounded play with payoff = score: the outcome, each visited order's result and the bounds worked out again.
fn play_bounded(
    live: &OrderedLive,
    master: &Master,
    orders: &[Vec<usize>],
    seed: i32,
    stop_below: i128,
    check_every: usize,
    mut upper: impl FnMut(&[usize], &LiveModel) -> ControlFlow<(), i128>,
) -> Result<Bounded, Error> {
    let trace = RefCell::new(BoundTrace { paid: vec![None; orders.len()], ..BoundTrace::default() });
    let mut visited: Vec<Option<Outcome>> = (0..orders.len()).map(|_| None).collect();
    let result = live.simulate_orders_bounded(
        master,
        orders,
        LiveRandom::new(seed),
        stop_below,
        check_every,
        |ids, m, _| {
            let c = upper(ids, m);
            if let ControlFlow::Continue(c) = c {
                trace.borrow_mut().bounded(ids, c);
            }
            Ok(c)
        },
        |i, m| {
            assert!(visited[i].replace(outcome(m)).is_none());
            let payoff = i128::from(m.score());
            trace.borrow_mut().visited(i, payoff);
            Ok(payoff)
        },
    )?;
    Ok((result, visited, trace.into_inner()))
}

fn sharing(result: &OrdersOutcome) -> OrderSharing {
    match result {
        OrdersOutcome::Complete(s) | OrdersOutcome::Stopped(s) | OrdersOutcome::Interrupted(s) => *s,
    }
}

/// Every order's own result, and its payoff.
fn alone(live: &OrderedLive, master: &Master, orders: &[Vec<usize>], seed: i32) -> (Vec<Outcome>, Vec<i128>) {
    let alone: Vec<Outcome> =
        orders.iter().map(|o| outcome(&live.simulate(master, o, LiveRandom::new(seed)).unwrap())).collect();
    let payoffs = alone.iter().map(|o| i128::from(o.score)).collect();
    (alone, payoffs)
}

fn check_visited(visited: &[Option<Outcome>], alone: &[Outcome], orders: &[Vec<usize>]) -> usize {
    for (i, v) in visited.iter().enumerate() {
        if let Some(v) = v {
            assert_eq!(v, &alone[i], "order {:?}", orders[i]);
        }
    }
    visited.iter().filter(|v| v.is_some()).count()
}

#[test]
fn bounded_play_stops_exactly_when_the_orders_fall_below_the_threshold() {
    let master = master_from(&tables());
    for (gekisou, seed) in [(false, 1), (true, 3)] {
        let live = live(gekisou);
        let orders = all_orders(5);
        let (alone, exact) = alone(&live, &master, &orders, seed);
        let total: i128 = exact.iter().sum();
        let end = live.play.frames.len();
        // A true bound that tightens as the play goes on.
        let slack = |played: usize| 2 * (end - played) as i128;
        let oracle = |ids: &[usize], m: &LiveModel| {
            ControlFlow::Continue(ids.iter().map(|&i| exact[i] + slack(m.frames_played())).sum::<i128>())
        };
        // Without a reachable threshold the bounds change nothing.
        let plain = live.simulate_orders(&master, &orders, LiveRandom::new(seed), |_, _| Ok(())).unwrap();
        let (result, visited, trace) = play_bounded(&live, &master, &orders, seed, i128::MIN, 0, oracle).unwrap();
        let s = sharing(&result);
        assert!(matches!(result, OrdersOutcome::Complete(_)));
        assert_eq!((s.frames, s.branches, s.clones), (plain.frames, plain.branches, plain.clones));
        assert_eq!((s.bound, trace.seen.last()), (Some(total), Some(&total)));
        assert_eq!(check_visited(&visited, &alone, &orders), orders.len());
        let root = total + slack(0) * orders.len() as i128;
        let mut previous: Option<(u64, usize)> = None;
        for stop_below in [i128::MIN, total - 1, total, total + 1, total + 500, total + 5_000, total + 50_000, root + 1]
        {
            let (result, visited, trace) = play_bounded(&live, &master, &orders, seed, stop_below, 37, oracle).unwrap();
            let s = sharing(&result);
            assert!(trace.seen.windows(2).all(|w| w[1] <= w[0]), "the bound never grows");
            assert_eq!(s.bound, trace.seen.last().copied());
            let done = check_visited(&visited, &alone, &orders);
            match result {
                OrdersOutcome::Complete(_) => {
                    assert_eq!((done, s.bound), (orders.len(), Some(total)));
                    let before_last = trace.seen[..trace.seen.len() - 1].iter().copied().min().unwrap();
                    assert!(stop_below <= before_last);
                }
                OrdersOutcome::Stopped(_) => {
                    assert!(s.bound.unwrap() < stop_below && stop_below > total && done < orders.len());
                }
                OrdersOutcome::Interrupted(_) => panic!("nothing interrupts"),
            }
            if stop_below <= total {
                assert!(matches!(result, OrdersOutcome::Complete(_)));
            }
            if stop_below > root {
                assert_eq!((s.frames, done), (0, 0));
            }
            // A higher threshold stops no later.
            if let Some((frames, count)) = previous {
                assert!(s.frames <= frames && done <= count);
            }
            previous = Some((s.frames, done));
        }
    }
}

#[test]
fn interrupted_play_keeps_the_orders_visited() {
    let master = master_from(&tables());
    let live = live(true);
    let orders = all_orders(5);
    let (alone, exact) = alone(&live, &master, &orders, 11);
    for after in [0, 1, 40, 300, 100_000] {
        let mut calls = 0;
        let upper = |ids: &[usize], _: &LiveModel| {
            calls += 1;
            if calls > after {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(ids.iter().map(|&i| exact[i]).sum::<i128>() + 1)
            }
        };
        let (result, visited, _) = play_bounded(&live, &master, &orders, 11, i128::MIN, 25, upper).unwrap();
        let s = sharing(&result);
        let done = check_visited(&visited, &alone, &orders);
        match result {
            OrdersOutcome::Interrupted(_) => assert_eq!(s.bounds, after as u64 + 1),
            OrdersOutcome::Complete(_) => assert!(s.bounds <= after as u64 && done == orders.len()),
            OrdersOutcome::Stopped(_) => panic!("nothing is below the threshold"),
        }
        if after == 0 {
            assert!(matches!(result, OrdersOutcome::Interrupted(_)));
            assert_eq!((s.frames, done), (0, 0));
        }
    }
}

#[test]
fn a_bound_below_the_payoffs_is_an_error() {
    let master = master_from(&tables());
    let live = live(true);
    let orders = all_orders(5);
    let (_, exact) = alone(&live, &master, &orders, 3);
    let low = |ids: &[usize], _: &LiveModel| ControlFlow::Continue(ids.iter().map(|&i| exact[i]).sum::<i128>() - 1);
    assert!(play_bounded(&live, &master, &orders, 3, i128::MIN, 0, low).is_err());
}

#[test]
fn bounded_play_counts_repeated_orders_once_per_entry() {
    let master = master_from(&tables());
    let live = live(true);
    let orders = vec![vec![4, 3, 2, 1, 0], vec![0, 1, 2, 3, 4], vec![4, 3, 2, 1, 0], vec![0, 1, 2, 4, 3]];
    let (alone, exact) = alone(&live, &master, &orders, 5);
    let total: i128 = exact.iter().sum();
    let tight = |ids: &[usize], _: &LiveModel| ControlFlow::Continue(ids.iter().map(|&i| exact[i]).sum::<i128>());
    let (result, visited, _) = play_bounded(&live, &master, &orders, 5, total, 10, tight).unwrap();
    assert!(matches!(result, OrdersOutcome::Complete(_)));
    assert_eq!(sharing(&result).bound, Some(total));
    assert_eq!(check_visited(&visited, &alone, &orders), orders.len());
}

#[test]
fn recorded_prefix_orders_keep_exact_scores_at_new_powers() {
    let master = master_from(&tables());
    let mut live = live(false);
    let orders = all_orders(5);
    let (sharing, records) =
        live.simulate_orders_recorded(&master, &orders, LiveRandom::new(5), 64 * 1024 * 1024, |_, _| Ok(())).unwrap();
    let records = records.unwrap();
    assert_eq!(records.len(), orders.len());
    assert!(sharing.frames < sharing.separate_frames);
    for power in [123_457, 411_113] {
        live.params.total_power = power;
        for record in &records {
            let actual = live.simulate(&master, &orders[record.index], LiveRandom::new(5)).unwrap();
            assert_eq!(record.program.evaluate(power), actual.score(), "order {} power {power}", record.index);
            assert_eq!(record.final_life, actual.current_life());
        }
    }
}

#[test]
fn partial_or_over_budget_recordings_never_publish_a_complete_group() {
    let master = master_from(&tables());
    let live = live(false);
    let orders = all_orders(5);
    let (outcome, records) = live
        .simulate_orders_bounded_recorded(
            &master,
            &orders,
            LiveRandom::new(5),
            64 * 1024 * 1024,
            1,
            30,
            |_, _, _| Ok(ControlFlow::Continue(0)),
            |_, _| panic!("initial proof should stop before an order"),
        )
        .unwrap();
    assert!(matches!(outcome, OrdersOutcome::Stopped(_)));
    assert!(records.is_none());
    let mut visited = 0;
    let (_, records) = live
        .simulate_orders_recorded(&master, &orders, LiveRandom::new(5), 1, |_, _| {
            visited += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(visited, orders.len());
    assert!(records.is_none());
}

#[test]
fn interrupted_prefix_recording_retains_only_finished_order_programs() {
    let master = master_from(&tables());
    let mut live = live(false);
    let orders = all_orders(5);
    let completed = RefCell::new(Vec::new());
    let (outcome, records) = live
        .simulate_orders_bounded_recorded_partial(
            &master,
            &orders,
            LiveRandom::new(5),
            64 * 1024 * 1024,
            i128::MIN,
            30,
            |_, _, _| {
                Ok(if completed.borrow().len() >= 5 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(i128::MAX)
                })
            },
            |index, model| {
                completed.borrow_mut().push(index);
                Ok(i128::from(model.score()))
            },
        )
        .unwrap();
    assert!(matches!(outcome, OrdersOutcome::Interrupted(_)));
    let records = records.unwrap();
    assert!(records.len() >= 5 && records.len() < orders.len());
    assert_eq!(records.len(), completed.borrow().len());
    live.params.total_power = 317_003;
    for recorded in records {
        assert!(completed.borrow().contains(&recorded.index));
        let native = live.simulate(&master, &orders[recorded.index], LiveRandom::new(5)).unwrap();
        assert_eq!(recorded.program.evaluate(live.params.total_power), native.score());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn grouped_play_matches_every_order_and_records_programs_at_any_depth() {
    let master = master_from(&tables());
    for (gekisou, seed) in [(false, 7), (true, 9)] {
        let live = live(gekisou);
        let orders = all_orders(5);
        let (alone, exact) = alone(&live, &master, &orders, seed);
        let serial = live.simulate_orders(&master, &orders, LiveRandom::new(seed), |_, _| Ok(())).unwrap();
        for (threads, depth) in [(1, 1), (2, 1), (3, 2), (8, 2), (16, 3), (4, 5)] {
            let (outcome, results, records) = live
                .simulate_orders_grouped(
                    &master,
                    &orders,
                    LiveRandom::new(seed),
                    64 * 1024 * 1024,
                    threads,
                    depth,
                    None,
                    30,
                    &|| false,
                    &|_, m| Ok(i128::from(m.score())),
                )
                .unwrap();
            assert!(matches!(outcome, OrdersOutcome::Complete(_)), "{threads} threads depth {depth}");
            let s = sharing(&outcome);
            assert!(s.frames >= serial.frames && s.frames <= s.separate_frames, "{s:?} vs {serial:?}");
            for (i, r) in results.iter().enumerate() {
                let r = r.expect("every order played");
                assert_eq!((r.index, r.score, r.final_life, r.payoff), (i, alone[i].score, alone[i].life, exact[i]));
            }
            let records = records.expect("complete recording");
            assert_eq!(records.len(), orders.len());
            let mut seen: Vec<usize> = records.iter().map(|r| r.index).collect();
            seen.sort_unstable();
            assert_eq!(seen, (0..orders.len()).collect::<Vec<_>>());
            for record in &records {
                assert_eq!(record.program.evaluate(live.params.total_power), alone[record.index].score);
                assert_eq!(record.final_life, alone[record.index].life);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn grouped_play_stops_below_the_threshold_and_interrupts_on_cancellation() {
    let master = master_from(&tables());
    let live = live(true);
    let orders = all_orders(5);
    let (alone, exact) = alone(&live, &master, &orders, 3);
    let total: i128 = exact.iter().sum();
    let caps: Vec<i128> = exact.iter().map(|p| p + 2).collect();
    let root: i128 = caps.iter().sum();
    let payoff = |_: usize, m: &LiveModel| Ok(i128::from(m.score()));
    // With one order left its cap exceeds its payoff by 2, so the remaining-sum check
    // can stop exactly when the threshold exceeds `total + 2`.
    for stop_below in [i128::MIN, total, total + 2, total + 3, root, root + 1] {
        let (result, results, records) = live
            .simulate_orders_grouped(
                &master,
                &orders,
                LiveRandom::new(3),
                64 * 1024 * 1024,
                4,
                2,
                Some((stop_below, &caps)),
                10,
                &|| false,
                &payoff,
            )
            .unwrap();
        let visited: Vec<Option<Outcome>> = results
            .iter()
            .enumerate()
            .map(|(i, r)| r.map(|_| outcome(&live.simulate(&master, &orders[i], LiveRandom::new(3)).unwrap())))
            .collect();
        let done = check_visited(&visited, &alone, &orders);
        for r in results.iter().flatten() {
            assert_eq!(r.payoff, exact[r.index]);
        }
        match result {
            OrdersOutcome::Complete(_) => {
                assert!(stop_below <= total + 2, "{stop_below} {total}");
                assert_eq!(done, orders.len());
                assert_eq!(records.map(|r| r.len()), Some(orders.len()));
            }
            OrdersOutcome::Stopped(_) => {
                assert!(stop_below > total + 2, "{stop_below} {total}");
                assert!(done < orders.len());
                // The orders done plus the caps of the rest prove the sum below the threshold.
                let proven: i128 = results.iter().enumerate().map(|(i, r)| r.map_or(caps[i], |r| r.payoff)).sum();
                assert!(proven < stop_below);
                if stop_below > root {
                    assert_eq!(done, 0);
                }
            }
            OrdersOutcome::Interrupted(_) => panic!("nothing interrupts"),
        }
    }
    let (outcome, results, records) = live
        .simulate_orders_grouped(
            &master,
            &orders,
            LiveRandom::new(3),
            64 * 1024 * 1024,
            4,
            2,
            Some((i128::MIN, &caps)),
            10,
            &|| true,
            &payoff,
        )
        .unwrap();
    assert!(matches!(outcome, OrdersOutcome::Interrupted(_)));
    assert!(results.iter().all(Option::is_none));
    assert!(records.is_none());
    let low: Vec<i128> = exact.iter().map(|p| p - 1).collect();
    assert!(
        live.simulate_orders_grouped(
            &master,
            &orders,
            LiveRandom::new(3),
            0,
            4,
            2,
            Some((i128::MIN, &low)),
            10,
            &|| false,
            &payoff,
        )
        .is_err()
    );
}
