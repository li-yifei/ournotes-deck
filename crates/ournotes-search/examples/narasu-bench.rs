//! Native real-chart benchmark: DATA ROSTER [THREADS] [MILLISECONDS].
use ournotes_search::{
    parallel::{self, Cancellation},
    types::RecommendationRequest,
};
use ournotes_sim::{cards::Roster, data::DeckData, scenario::Scenario};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return Err("usage: narasu-bench DATA ROSTER [THREADS] [MILLISECONDS]".into());
    }
    let data = DeckData::from_path(&args[0])?;
    let roster = Roster::from_json(&std::fs::read_to_string(&args[1])?)?;
    let threads = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or_else(parallel::default_workers);
    let milliseconds: u64 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(60_000);
    let chart = data.data_chart(10007103).ok_or("Narasu EXPERT chart missing")?;
    let scene = Scenario::Battle(100071).resolve(&data.master)?;
    if scene.gekisou_missions != [2, 2, 2] || chart.fevers.len() != 3 || chart.skill_event_ms.is_empty() {
        return Err("benchmark requires complete three-LUCK chart and skill events".into());
    }
    let factors = ournotes_sim::live::full::gekisou_rank_factors(&data.master, &scene.gekisou_missions)?;
    let confirmations: Vec<_> =
        (0..3).map(|range| json!({"frame":0,"range":range,"rank":1,"percent":factors[range][0]})).collect();
    let request: RecommendationRequest = serde_json::from_value(json!({
        "format":"ournotes-deck.search-request/1",
        "execution":{"kind":"live","scoreId":10007103,"gekisou":true,
            "play":{"kind":"accuracy","greatFraction":0.0,"justFraction":1.0}},
        "scenario":{"kind":"battle","musicId":100071},
        "networkConfirmations":confirmations,
        "metric":{"kind":"score"},"k":5,"strategy":{"kind":"branchAndBound"},
        "limits":{"timeLimitMs":milliseconds,"cacheEntries":256}
    }))?;
    eprintln!(
        "dataset={:?} notes={} skillEvents={} members={} snaps={} threads={threads} budgetMs={milliseconds}",
        data.sha256,
        chart.notes.len(),
        chart.skill_event_ms.len(),
        roster.members.len(),
        roster.snaps.len()
    );
    let result = parallel::recommend(&data, &roster, &request, threads, Cancellation::default())?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
