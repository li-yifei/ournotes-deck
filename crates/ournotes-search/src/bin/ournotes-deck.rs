//! Command-line front end: `ournotes-deck <power|skip|live> --data FILE --roster FILE [options]`, and
//! `ournotes-deck chart-stats --data FILE` and `ournotes-deck recommend --request FILE ...`.

#[path = "../cli/recommend.rs"]
mod recommend_cli;

use std::process::ExitCode;
use std::time::Duration;

use ournotes_search::search::{
    Constraints, GekisouObjective, Objective, PlayInput, SearchRequest, SeedSet, music_of_score, search,
};
use ournotes_sim::cards::Roster;
use ournotes_sim::data::DeckData;
use ournotes_sim::live::model::{JudgementStream, JustRule};
use ournotes_sim::pool::Pool;
use ournotes_sim::scenario::{ContextInput, PowerSnapshotInput, Scenario};
use serde_json::json;

const USAGE: &str = "usage:
  ournotes-deck recommend --data FILE (--roster FILE | --snapshot FILE) --request FILE
                      [--progress-ms N] [-o FILE]
  ournotes-deck power --data FILE --roster FILE [common options]
  ournotes-deck skip  --data FILE --roster FILE --score ID [common options]
  ournotes-deck live  --data FILE --roster FILE --score ID --expectation finite --seed-law FILE [--play FILE]
                      [--gekisou] [common options]
  ournotes-deck chart-stats --data FILE [--seeds N] [--no-gekisou-aptitude]
                      [--charts ID[,ID...]] [--jobs N] [-o FILE]
recommend consumes the JSON recommendation request and writes the unified recommendation result.
Use ournotes-deck recommend --help for its input and progress options.
--data is a deck data file (nnnotes.deck-data/1). live ranks by the expected score of the whole-live simulation
with snap skills over the native member-order roots of the finite law in --seed-law (JSON
[[rootSeed,positiveWeight],...]); --play is a judgement stream and defaults to the theoretical best play; --gekisou
plays the live with Gekisou on.
objectives: --objective power|score|client-event-points|conditional-client-event-items [--event-id ID]
conditional items also require --resource-type ID --resource-id ID and context.eventPayoff.selectedRewards
scenario options: --scenario free|mission|battle|arena|challenge --scenario-music ID --context FILE
--scenario-music is the special row ID for arena/challenge; --score always denotes the base chart.
--context uses explicit powerSnapshot.eventIds and separate resultClock normalized DateTime ticks.
chart-stats measures every chart on the whole-live simulation (ournotes-deck.chart-stats/3): nominal Gekisou
expectations and score-up weights, range weights for fixed ranks, the Perfect play's scores and deterministic
Free Live figures (offSeeds). Each Gekisou skill shape is measured alone, including plain score-up cross terms.
Estimates are [center, outward interval half-width] under independent nominal lottery and skill probabilities.
--seeds N controls replay seeds for charts with a luck range (default 8); statistics use nominal expectations.
--no-gekisou-aptitude leaves both aptitude fields null and includes the baseline measurements.
--charts keeps the listed score ids in file order; --jobs N measures N charts at once (default 1).
-o FILE writes JSON to a file; without it JSON goes to stdout.
common options: -k N (default 10), --leader ID, --include ID[,ID...], --exclude ID[,ID...],
                --exclude-snaps ID[,ID...], --no-snaps, --time-limit-ms N";

fn ids(s: &str) -> Result<Vec<i64>, String> {
    s.split(',').filter(|x| !x.is_empty()).map(|x| x.trim().parse().map_err(|_| format!("bad id {x:?}"))).collect()
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn chart_stats(args: &[String]) -> Result<Option<serde_json::Value>, String> {
    let (mut data, mut seeds, mut out) = (None, None, None);
    let mut options = ournotes_sim::chartstats::Options::default();
    let (mut only, mut jobs): (Option<Vec<i64>>, usize) = (None, 1);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || -> Result<String, String> {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{a} needs a value"))
        };
        match a {
            "--data" => data = Some(val()?),
            "--seeds" => seeds = Some(val()?.trim().parse::<usize>().map_err(|_| "bad --seeds".to_string())?),
            "--no-gekisou-aptitude" => options.aptitude = false,
            "--charts" => only = Some(ids(&val()?)?),
            "--jobs" => jobs = val()?.trim().parse::<usize>().map_err(|_| "bad --jobs".to_string())?.max(1),
            "-o" | "--out" => out = Some(val()?),
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
        i += 1;
    }
    let mut data = DeckData::from_path(data.ok_or("--data is required")?).map_err(|e| e.to_string())?;
    if let Some(only) = &only {
        data.charts.retain(|c| only.contains(&c.score_id));
    }
    if let Some(n) = seeds {
        options.replay_seeds = n;
    }
    let doc = if jobs <= 1 {
        ournotes_sim::chartstats::document_with(&data, &options).map_err(|e| e.to_string())?
    } else {
        chart_stats_parallel(data, &options, jobs)?
    };
    match out {
        Some(path) => {
            let mut text = serde_json::to_string(&doc).expect("json");
            text.push('\n');
            std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
            Ok(None)
        }
        None => Ok(Some(doc)),
    }
}

/// The chart-stats document with `jobs` charts measured at once (the same document as one at a time).
fn chart_stats_parallel(
    mut data: DeckData,
    options: &ournotes_sim::chartstats::Options,
    jobs: usize,
) -> Result<serde_json::Value, String> {
    use ournotes_sim::chartstats;
    let charts = std::mem::take(&mut data.charts);
    let mut doc = chartstats::document_with(&data, options).map_err(|e| e.to_string())?;
    let kinds = chartstats::kinds(&data.master);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<chartstats::ChartStats, String>>>> =
        charts.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(charts.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(c) = charts.get(i) else { break };
                    let t = std::time::Instant::now();
                    let r = chartstats::chart_stats_with(&data.master, c, &kinds, options)
                        .map_err(|e| format!("chart {}: {e}", c.score_id));
                    eprintln!(
                        "chart {} {:.1} s{}",
                        c.score_id,
                        t.elapsed().as_secs_f64(),
                        if r.is_err() { " FAILED" } else { "" }
                    );
                    *results[i].lock().expect("lock") = Some(r);
                }
            });
        }
    });
    let mut out = Vec::with_capacity(charts.len());
    for r in results {
        let r = r.into_inner().expect("lock").ok_or("a chart was not measured")?;
        out.push(serde_json::to_value(r?).expect("json"));
    }
    doc["charts"] = serde_json::Value::Array(out);
    Ok(doc)
}

fn run(args: &[String]) -> Result<serde_json::Value, String> {
    let cmd = args.first().ok_or(USAGE)?.as_str();
    if cmd == "chart-stats" {
        return chart_stats(&args[1..]).map(|v| v.unwrap_or(serde_json::Value::Null));
    }
    let mut data = None;
    let mut roster = None;
    let mut scenario_name = None;
    let mut scenario_music = None;
    let mut context_file = None;
    let mut score = None;
    let mut play = None;
    let mut objective_name = None;
    let mut target_event_id = None;
    let mut resource_type = None;
    let mut resource_id = None;
    let mut expectation = None;
    let mut seed_law = None;
    let mut k = 10usize;
    let mut c = Constraints::default();
    let mut limit = None;
    let mut gekisou = false;
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || -> Result<String, String> {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{a} needs a value"))
        };
        match a {
            "--data" => data = Some(val()?),
            "--roster" => roster = Some(val()?),
            "--scenario" => scenario_name = Some(val()?),
            "--scenario-music" => scenario_music = Some(val()?.parse::<i64>().map_err(|e| e.to_string())?),
            "--context" => context_file = Some(val()?),
            "--score" => score = Some(val()?.parse::<i64>().map_err(|e| e.to_string())?),
            "--play" => play = Some(val()?),
            "--objective" => objective_name = Some(val()?),
            "--resource-type" => resource_type = Some(val()?.parse::<i64>().map_err(|e| e.to_string())?),
            "--resource-id" => resource_id = Some(val()?.parse::<i64>().map_err(|e| e.to_string())?),
            "--event-id" => target_event_id = Some(val()?.parse::<i64>().map_err(|e| e.to_string())?),
            "--expectation" => expectation = Some(val()?),
            "--seed-law" => seed_law = Some(val()?),
            "-k" => k = val()?.parse().map_err(|_| "bad -k".to_string())?,
            "--leader" => c.leader = Some(val()?.parse().map_err(|_| "bad --leader".to_string())?),
            "--include" => c.include_members = ids(&val()?)?,
            "--exclude" => c.exclude_members = ids(&val()?)?,
            "--exclude-snaps" => c.exclude_snaps = ids(&val()?)?,
            "--no-snaps" => c.no_snaps = true,
            "--gekisou" => gekisou = true,
            "--time-limit-ms" => {
                limit = Some(Duration::from_millis(val()?.parse().map_err(|_| "bad --time-limit-ms".to_string())?))
            }
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
        i += 1;
    }
    if !matches!(cmd, "power" | "skip" | "live") {
        return Err(USAGE.into());
    }
    let law = if cmd == "live" {
        match expectation.as_deref() {
            Some("finite") => {}
            Some(name) => {
                return Err(format!("unsupported expectation law {name:?}; supply finite with explicit masses"));
            }
            None => {
                return Err(
                    "live requires --expectation finite --seed-law FILE: native MemberShuffle is random, not a deck decision"
                        .into(),
                );
            }
        }
        let file = seed_law.ok_or("--expectation finite requires --seed-law FILE containing [[rootSeed,positiveWeight],...] (an explicit finite law, not the unknown real server seed distribution)")?;
        let atoms: Vec<(i32, u64)> = serde_json::from_str(&read(&file)?).map_err(|e| format!("seed law: {e}"))?;
        Some(ournotes_search::search::expectation::FiniteSeedLaw::new(atoms).map_err(|e| e.to_string())?)
    } else {
        if expectation.is_some() || seed_law.is_some() || gekisou || play.is_some() {
            return Err("expectation, play and Gekisou options apply only to live".into());
        }
        None
    };
    let data = DeckData::from_path(data.ok_or("--data is required")?).map_err(|e| e.to_string())?;
    let roster = Roster::from_json(&read(&roster.ok_or("--roster is required")?)?).map_err(|e| e.to_string())?;
    if scenario_name.is_none() && scenario_music.is_some() {
        return Err("--scenario-music requires --scenario".into());
    }
    let objective_name = objective_name.as_deref().unwrap_or(if cmd == "power" { "power" } else { "score" });
    if !matches!(objective_name, "power" | "score" | "client-event-points" | "conditional-client-event-items") {
        return Err(format!(
            "unknown objective {objective_name:?}; server-selected item rewards require an explicit reward adapter, never guessed drops"
        ));
    }
    let item_objective = objective_name == "conditional-client-event-items";
    let event_objective = objective_name == "client-event-points" || item_objective;
    let item_target = if item_objective {
        Some((
            resource_type.ok_or("conditional items require --resource-type")?,
            resource_id.ok_or("conditional items require --resource-id")?,
        ))
    } else {
        if resource_type.is_some() || resource_id.is_some() {
            return Err("resource target options require conditional-client-event-items".into());
        }
        None
    };
    if (objective_name == "power") != (cmd == "power") {
        return Err("power objective requires power command; score/event objectives require skip or live".into());
    }
    if !event_objective && target_event_id.is_some() {
        return Err("--event-id requires --objective client-event-points".into());
    }
    let context_input: ContextInput = match context_file {
        Some(p) => serde_json::from_str(&read(&p)?)
            .map_err(|e| format!("context: {e}; dates must be normalized DateTime ticks, not assumed-JST strings"))?,
        None => ContextInput {
            power_snapshot: PowerSnapshotInput { event_ids: roster.player.events.clone(), captured_jst_ticks: None },
            result_clock: None,
            event_payoff: None,
        },
    };
    let explicit_scenario = scenario_name.is_some();
    let selected_scenario = match scenario_name.as_deref() {
        Some(name) => {
            let id = scenario_music
                .ok_or("--scenario requires --scenario-music ID (special-table ID for arena/challenge)")?;
            Some(match name {
                "free" => Scenario::Free(id),
                "mission" => Scenario::Mission(id),
                "battle" => Scenario::Battle(id),
                "arena" => Scenario::Arena(id),
                "challenge" => Scenario::Challenge(id),
                _ => return Err(format!("unknown scenario {name:?}; expected free|mission|battle|arena|challenge")),
            })
        }
        None if cmd != "power" => Some(Scenario::Free(
            music_of_score(&data.master, score.ok_or("--score is required")?).map_err(|e| e.to_string())?,
        )),
        _ => None,
    };
    let context = selected_scenario
        .map(|scenario| {
            let fevers = match score {
                Some(id) => data.data_chart(id).map(|c| c.fevers.as_slice()).unwrap_or(&[]),
                None => &[],
            };
            context_input.resolve(&data.master, scenario, score, fevers).map_err(|e| e.to_string())
        })
        .transpose()?;
    if let Some(clock) = &context_input.result_clock
        && matches!(
            (cmd, clock),
            ("skip", ournotes_sim::scenario::ResultClockInput::Played { .. })
                | ("live", ournotes_sim::scenario::ResultClockInput::Skip { .. })
        )
    {
        return Err("resultClock execution does not match the command".into());
    }
    let objective = match cmd {
        "power" => Objective::Power { music_id: None, event: false },
        "skip" => {
            let score_id = score.ok_or("--score is required")?;
            Objective::SkipScore { score_id, chart: data.chart(score_id).map_err(|e| e.to_string())? }
        }
        _ => {
            let score_id = score.ok_or("--score is required")?;
            let chart = data.chart(score_id).map_err(|e| e.to_string())?;
            let data_chart = data.data_chart(score_id).ok_or_else(|| format!("no chart for score id {score_id}"))?;
            // The root law owns the seeds; the expectation replaces the objective's seed list.
            let gk = gekisou.then(|| {
                let setup = context.as_ref().expect("live resolves a scenario").gekisou.clone();
                (GekisouObjective { seeds: SeedSet::List(vec![0]), fevers: data_chart.fevers.clone() }, setup)
            });
            let judgement_types = data_chart.judgement_types.clone();
            let stream = match (play, &gk) {
                (Some(p), _) => {
                    serde_json::from_str::<JudgementStream>(&read(&p)?).map_err(|e| format!("play: {e}"))?
                }
                (None, None) => JudgementStream::theoretical_best(&chart),
                (None, Some((_, setup))) => {
                    let rule = JustRule::new(&data.master, setup).map_err(|e| e.to_string())?;
                    JudgementStream::theoretical_best_gekisou(&chart, &judgement_types, &rule)
                        .map_err(|e| e.to_string())?
                }
            };
            let play = PlayInput::Stream { stream, judgement_types };
            Objective::LiveScore {
                score_id,
                chart,
                play,
                event: false,
                exclude_snap_skills: false,
                gekisou: gk.map(|g| g.0),
            }
        }
    };
    let pool = match &context {
        Some(ctx) => ctx.pool(&data.master, &roster),
        None => {
            let mut frozen = roster.clone();
            frozen.player.events = context_input.power_snapshot.event_ids.clone();
            Pool::new(&data.master, &frozen)
        }
    }
    .map_err(|e| e.to_string())?;
    let objective = match &context {
        Some(ctx) => objective.in_scenario(ctx.clone()),
        None => objective,
    };
    let resolved_output = context.as_ref().map(|ctx| json!({
        "scenario": format!("{:?}",ctx.scenario), "explicitScenario":explicit_scenario,
        "baseLiveMusicId":ctx.resolved.live_music_id, "scoreId":ctx.score_id,
        "chartAsset": ctx.score_id.and_then(|id| data.data_chart(id)).map(|c| json!({"key":c.asset_key,"sha256":c.asset_sha256})),
        "powerMusic": {"id":ctx.resolved.power_music.id,"musicType":ctx.resolved.power_music.music_type,
            "bestMusicTagIds":ctx.resolved.power_music.best_music_tag_ids,
            "typeBonusRate":ctx.resolved.power_music.type_bonus_rate,"tagBonusRate":ctx.resolved.power_music.tag_bonus_rate},
        "calcEventParameter":ctx.resolved.calc_event_parameter,
        "skillTargetMusicType":ctx.resolved.skill_target_music_type,
        "gekisouMissions":ctx.resolved.gekisou_missions,"fevers":ctx.gekisou.fevers,
        "clocks":context_input,"resultJstTicks":ctx.result_clock.map(|c| c.jst_ticks()),
        "eligibility":"not evaluated; player progression and unlock eligibility are separate inputs",
        "serverRewards":"unknown; no server-selected rewards inferred"
    }));
    if let Some(law) = law {
        let request = SearchRequest { objective, k, constraints: c, time_limit: limit };
        let out = if event_objective {
            let ctx = context.as_ref().expect("live resolves a scenario");
            let event_id = target_event_id.ok_or("client-event-points requires --event-id")?;
            let event_input =
                context_input.event_payoff.as_ref().ok_or("client-event-points requires context.eventPayoff")?;
            ctx.event_request(&data.master, event_input, event_id).map_err(|e| e.to_string())?;
            if item_objective && event_input.selected_rewards.is_none() {
                return Err("UnknownServerAuthority: context.eventPayoff.selectedRewards is required; [] means explicitly no selected rewards".into());
            }
            ournotes_search::search::expectation::oracle_with_payoff_factory(
                &pool,
                &request,
                &law,
                || Ok(()),
                |physical, terminal, _| {
                    if let Some((ty,id))=item_target {
                        let items=ctx.preview_event_items(&pool,&physical.as_deck(),event_input,event_id,terminal.final_score)?;
                        ournotes_sim::scenario::item_payoff(&items,event_id,ty,id)
                    } else {
                        Ok(ctx.preview_event_points(&pool, &physical.as_deck(), event_input, event_id, terminal.final_score)?.points_for(event_id) as i128)
                    }
                },
            )
        } else {
            ournotes_search::search::expectation::oracle(&pool, &request, &law)
        }
        .map_err(|e| e.to_string())?;
        let mut previews = Vec::new();
        let mut item_previews = Vec::new();
        if event_objective {
            let ctx = context.as_ref().expect("validated context");
            let input = context_input.event_payoff.as_ref().expect("validated event input");
            for result in &out.results {
                let terminal_previews = result
                    .evaluation
                    .outcomes
                    .iter()
                    .map(|o| {
                        ctx.preview_event_points(
                            &pool,
                            &result.physical.as_deck(),
                            input,
                            target_event_id.expect("validated event id"),
                            o.final_score,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                if item_objective {
                    item_previews.push(
                        result
                            .evaluation
                            .outcomes
                            .iter()
                            .map(|o| {
                                ctx.preview_event_items(
                                    &pool,
                                    &result.physical.as_deck(),
                                    input,
                                    target_event_id.expect("validated event id"),
                                    o.final_score,
                                )
                            })
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|e| e.to_string())?,
                    );
                }
                previews.push(terminal_previews);
            }
        }
        return Ok(
            json!({"resolvedContext":resolved_output,"objective":objective_name,"targetEventId":target_event_id,
            "jsonNumberPolicy":"exact JSON integers; parse with arbitrary-precision integers (not JavaScript Number)","probabilityLaw":"explicit-finite-native-root-law","legacyStreamBaseSeed":"ignored; finite law is authoritative","arithmetic":"client-f32-i32; checked-i128-u128-expectation",
            "proofScope":"conditional on supplied finite root law and simulator supported domain; not inferred TickCount distribution",
            "clientCounterPreviews":previews,"conditionalItemPreviews":item_previews,"itemTarget":item_target,"search":out}),
        );
    }
    if event_objective {
        let event_id = target_event_id.ok_or("client-event-points requires --event-id")?;
        let input = context_input.event_payoff.as_ref().ok_or("client-event-points requires context.eventPayoff")?;
        let request = SearchRequest { objective, k, constraints: c, time_limit: limit };
        let out = ournotes_search::skip_event::search_skip_event_payoff(&pool, &request, input, event_id, item_target)
            .map_err(|e| e.to_string())?;
        return Ok(json!({"resolvedContext":resolved_output,"objective":objective_name,"targetEventId":event_id,
            "proofScope":"exhaustive physical decks; client counter preview only, no server award authority","search":out}));
    }
    let request = SearchRequest { objective, k, constraints: c, time_limit: limit };
    let out = search(&pool, &request).map_err(|e| e.to_string())?;
    Ok(json!({
        "objective": cmd,
        "resolvedContext": resolved_output,
        "completion": out.completion,
        "results": out.results,
        "stats": out.stats,
        "elapsedMs": out.elapsed.as_secs_f64() * 1e3,
        "verifyElapsedMs": out.verify_elapsed.as_secs_f64() * 1e3,
    }))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args[0] == "recommend" {
        return recommend_cli::main(&args[1..]);
    }
    match run(&args) {
        Ok(serde_json::Value::Null) => ExitCode::SUCCESS,
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).expect("json"));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
