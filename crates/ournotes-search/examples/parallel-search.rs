//! Reproducible native search: DATA ROSTER REQUEST [WORKERS].
use ournotes_search::{
    parallel::{self, Cancellation},
    types::RecommendationRequest,
};
use ournotes_sim::{cards::Roster, data::DeckData};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("usage: parallel-search DATA ROSTER REQUEST [WORKERS]".into());
    }
    let data = DeckData::from_path(&args[0])?;
    let roster = Roster::from_json(&std::fs::read_to_string(&args[1])?)?;
    let request: RecommendationRequest = serde_json::from_str(&std::fs::read_to_string(&args[2])?)?;
    let workers = args.get(3).map(|v| v.parse()).transpose()?.unwrap_or_else(parallel::default_workers);
    let out = parallel::recommend(&data, &roster, &request, workers, Cancellation::default())?;
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}
