//! Public recommendation facade: parse -> build -> search. No game formula lives here.
use crate::clock::Instant;
use crate::owned_snapshot::{GoalDependencies, Issue, OwnedSnapshot, Resolution};
use crate::search::physical::ProgressHook;
use crate::types::{RecommendationOutcome, RecommendationRequest};
use ournotes_sim::{Error, cards::Roster, data::DeckData};
use serde::Serialize;
use std::time::Duration;

/// Progress reports of a running search. A report is the result the search would return if its time limit expired
/// at that point: `completion` TimedOut, the exactly evaluated Top-K so far and the telemetry so far. Reports are
/// made at the search's deadline checks and after Top-K insertions, at most once per `interval`; the first one once
/// `interval` has passed since the search started. Physical-deck searches report; fixed-deck evaluation and the
/// canonical power/skip search do not. Reporting never changes the search or its result.
pub struct Progress<'a> {
    pub interval: Duration,
    pub report: &'a mut dyn FnMut(&RecommendationOutcome),
}

/// Recommend using an already loaded dataset and parsed roster/request.
/// The request deadline includes building the problem and running the solver.
pub fn recommend(
    data: &DeckData,
    roster: &Roster,
    request: &RecommendationRequest,
) -> Result<RecommendationOutcome, Error> {
    recommend_hooked(data, roster, request, None)
}

/// [`recommend`] with progress reports.
pub fn recommend_with_progress(
    data: &DeckData,
    roster: &Roster,
    request: &RecommendationRequest,
    progress: Progress<'_>,
) -> Result<RecommendationOutcome, Error> {
    let Progress { interval, report } = progress;
    let mut forward = |out: RecommendationOutcome| report(&out);
    recommend_hooked(data, roster, request, Some(ProgressHook { interval, report: &mut forward }))
}

pub(crate) fn recommend_hooked(
    data: &DeckData,
    roster: &Roster,
    request: &RecommendationRequest,
    progress: Option<ProgressHook<'_>>,
) -> Result<RecommendationOutcome, Error> {
    recommend_hooked_started(data, roster, request, progress, Instant::now())
}

/// Continue a request whose budget already began at its transport boundary. Validation and problem construction
/// still run before the solver observes the remaining budget, so an expired request does not hide invalid input.
pub(crate) fn recommend_hooked_started(
    data: &DeckData,
    roster: &Roster,
    request: &RecommendationRequest,
    progress: Option<ProgressHook<'_>>,
    start: Instant,
) -> Result<RecommendationOutcome, Error> {
    #[cfg(not(target_arch = "wasm32"))]
    if progress.is_none() && crate::parallel::native_enabled() {
        return crate::parallel::recommend_started(
            data,
            roster,
            request,
            crate::parallel::native_workers(),
            crate::parallel::Cancellation::default(),
            start,
        );
    }
    let built = crate::handler::build_card_pool(data, roster, request)?;
    crate::search::dispatch::execute(&built, None, start, start.elapsed().as_secs_f64() * 1000.0, progress)
}

/// JSON transport over the same typed entry point. Retain DeckData between calls.
pub fn recommend_json(data: &DeckData, roster_json: &str, request_json: &str) -> Result<String, Error> {
    let roster = Roster::from_json(roster_json)?;
    let request = serde_json::from_str(request_json).map_err(|e| Error::Input(format!("request: {e}")))?;
    let result = recommend(data, &roster, &request)?;
    serde_json::to_string(&result).map_err(|e| Error::Domain(format!("result JSON: {e}")))
}

pub const SNAPSHOT_RESULT_FORMAT: &str = "ournotes-deck.snapshot-recommendation/1";

/// How far a snapshot recommendation got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotStatus {
    /// `result` holds the recommendation.
    Ok,
    /// The snapshot lacks facts the request's goal reads (`missing`); nothing else is wrong.
    Incomplete,
    /// The snapshot or the request is invalid (`errors`, possibly with `missing`).
    Invalid,
    /// The inputs resolved but the computation failed (`errors`).
    Failed,
}

/// Answer of [`recommend_snapshot`]: a result, or the structured reasons why there is none.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRecommendation {
    pub format: &'static str,
    /// The identity a snapshot's `datasetId` must name: the lowercase hexadecimal SHA-256 of the deck data text
    /// (`DeckData::sha256`).
    pub dataset_id: Option<String>,
    pub status: SnapshotStatus,
    pub missing: Vec<Issue>,
    pub errors: Vec<Issue>,
    pub result: Option<RecommendationOutcome>,
}

fn issue(path: &str, error: Error) -> Issue {
    let (code, message) = match error {
        Error::Master(m) => ("master", m),
        Error::Input(m) => ("input", m),
        Error::Game(m) => ("game", m),
        Error::Unsupported(m) => ("unsupported", m),
        Error::Domain(m) => ("domain", m),
        Error::Capacity(m) => ("capacity", m),
    };
    Issue { path: path.into(), code: code.into(), message }
}

/// Recommend for an owned snapshot (`ournotes.owned-snapshot/1`) and a request, both given as their original JSON
/// text. The request's execution selects the goal whose facts the snapshot must supply (see
/// [`GoalDependencies::of`]); the snapshot must name this deck data by its SHA-256. Unknown facts are reported in
/// `missing` and invalid input in `errors`, each with its JSON path; nothing unknown is replaced by a default.
pub fn recommend_snapshot(
    data: &DeckData,
    snapshot_json: &str,
    request_json: &str,
    progress: Option<Progress<'_>>,
) -> SnapshotRecommendation {
    let mut answer = SnapshotRecommendation {
        format: SNAPSHOT_RESULT_FORMAT,
        dataset_id: data.sha256.clone(),
        status: SnapshotStatus::Invalid,
        missing: Vec::new(),
        errors: Vec::new(),
        result: None,
    };
    let request = serde_json::from_str::<RecommendationRequest>(request_json)
        .map_err(|e| answer.errors.push(Issue { path: "request".into(), code: "parse".into(), message: e.to_string() }))
        .ok();
    let snapshot = OwnedSnapshot::from_json(snapshot_json)
        .map_err(|e| answer.errors.push(Issue { code: "parse".into(), ..issue("snapshot", e) }))
        .ok();
    let (Some(request), Some(snapshot)) = (request, snapshot) else { return answer };
    let goal = GoalDependencies::of(&request.execution);
    let Resolution { missing, errors, resolved } =
        snapshot.resolve_data(data, data.sha256.as_deref().unwrap_or_default(), goal);
    (answer.missing, answer.errors) = (missing, errors);
    let Some(resolved) = resolved else {
        if answer.errors.is_empty() {
            answer.status = SnapshotStatus::Incomplete;
        }
        return answer;
    };
    let mut forward;
    let hook = match progress {
        Some(Progress { interval, report }) => {
            forward = move |out: RecommendationOutcome| report(&out);
            Some(ProgressHook { interval, report: &mut forward })
        }
        None => None,
    };
    match resolved.recommend_hooked(data, &request, hook) {
        Ok(result) => {
            answer.status = SnapshotStatus::Ok;
            answer.result = Some(result);
        }
        Err(error) => {
            if !matches!(error, Error::Input(_)) {
                answer.status = SnapshotStatus::Failed;
            }
            answer.errors.push(issue("request", error));
        }
    }
    answer
}

pub use crate::recommendation::{Answer, AnswerProgress};

/// Recommend for an account (`ournotes.account/1`) and a request (`ournotes-deck.recommendation-request/2`), both
/// given as their original JSON text. The account must name this deck data by its SHA-256 and is resolved for the
/// facts the request's goal reads; unknown facts are reported in `missing` and invalid input in `errors`, each with
/// its JSON path, and nothing unknown is replaced by a default. `progress`, when given, receives complete answers
/// with `final: false` at most once per interval while the search runs. The cooperative search budget starts before
/// account/request parsing and resolution. Dataset loading precedes this call; validation, atomic operations and
/// result materialization are not preempted at the deadline. See `docs/recommendation.md`.
pub fn recommend_account(
    data: &DeckData,
    account_json: &str,
    request_json: &str,
    progress: Option<AnswerProgress<'_>>,
) -> Answer {
    crate::recommendation::recommend(data, account_json, request_json, progress)
}

/// What [`recommend_account`] computes: goals, metrics per goal, accuracy, ranks and the goals whose power reads the
/// held events.
pub fn capabilities() -> serde_json::Value {
    crate::recommendation::capabilities()
}
