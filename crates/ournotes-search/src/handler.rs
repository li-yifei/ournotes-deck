//! Build an immutable search problem from one dataset, roster and explicit goal.
//! No search, candidate truncation or gameplay simulation runs during construction.
use crate::search::expectation;
use crate::search::{GekisouObjective, Objective, PlayInput, SearchRequest, SeedSet};
use crate::types::*;
use ournotes_sim::pool::Pool;
use ournotes_sim::{
    Error,
    cards::{Roster, SongView},
    data::DeckData,
};
mod validation;
use crate::domain::CandidateDomain;
use ournotes_sim::live::model::{JudgementStream, JustRule};
use ournotes_sim::replay::RankConfirmation;
use ournotes_sim::scenario::{ContextInput, EventPayoffInput, PowerSnapshotInput};
use validation::{goal_description, validate_payoff, validate_play};
pub(crate) use validation::{reject_unsupported_lifecycle, validate};

/// The algorithm route selected by the objective, never by an arbitrary pool-size cutoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SolverRoute {
    /// The member-set route of the wire format. Formal requests do not select it.
    CanonicalPowerSkip,
    PhysicalExhaustive,
    PhysicalBranchAndBound,
    PhysicalCandidate,
}

/// Frozen semantic inputs shared by search and fixed-deck evaluation.
pub struct SearchContext {
    pub(crate) request: SearchRequest,
    pub(crate) context_input: ContextInput,
    pub(crate) player_goal: GoalDescription,
    pub(crate) resolved_context: serde_json::Value,
    pub(crate) spec: RecommendationRequest,
    pub(crate) route: SolverRoute,
    pub(crate) plan: ExecutionPlan,
    pub(crate) data: crate::search::telemetry::DataIdentity,
}
impl SearchContext {
    /// Exact request snapshot. Mutating the caller's original cannot change this problem.
    pub fn request(&self) -> &RecommendationRequest {
        &self.spec
    }
    pub fn route(&self) -> SolverRoute {
        self.route
    }
    pub fn resolved_context(&self) -> &serde_json::Value {
        &self.resolved_context
    }
}

/// Built once, borrowed for any number of fresh searches/evaluations.
/// Its lifetime prevents the source dataset changing under a running search.
pub struct BuiltProblem<'m> {
    pub(crate) pool: Pool<'m>,
    pub(crate) context: SearchContext,
}
impl<'m> BuiltProblem<'m> {
    pub fn pool(&self) -> &Pool<'m> {
        &self.pool
    }
    pub fn context(&self) -> &SearchContext {
        &self.context
    }
    pub fn domain(&self) -> &CandidateDomain {
        &self.context.plan.domain
    }

    /// Native partitions change only constraints and warm-start decks. Keep the
    /// resolved pool/objective on its owning thread; rebuild domain-specific bounds.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn repartition(
        &mut self,
        constraints: &crate::search::Constraints,
        initial_decks: &[DeckInput],
    ) -> Result<(), Error> {
        let ctx = &mut self.context;
        let previous = std::mem::replace(&mut ctx.request.constraints, constraints.clone());
        let plan = compile_execution(
            &self.pool,
            &ctx.request,
            &ctx.spec.metric,
            ctx.context_input.event_payoff.as_ref(),
            ctx.spec.network_confirmations.as_deref(),
            &ctx.spec.simulation,
            &ctx.spec.strategy,
        );
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                ctx.request.constraints = previous;
                return Err(error);
            }
        };
        ctx.spec.constraints = constraints.clone();
        ctx.spec.initial_decks = initial_decks.to_vec();
        ctx.plan = plan;
        Ok(())
    }
}

pub(crate) struct ExecutionPlan {
    pub(crate) joint: Option<crate::search::joint::JointBounds>,
    pub(crate) deck_payoff: Option<crate::search::deck_payoff::DeckPayoffBounds>,
    pub(crate) team_power: Option<crate::search::team_power::TeamPowerBounds>,
    /// When the bound compile began, and its duration (zero without branch-and-bound).
    pub(crate) bound_compile_started: crate::clock::Instant,
    pub(crate) bound_compile_ms: f64,
    pub(crate) bound_fallback: Option<String>,
    /// Why a played solo Live's payoff steps have no deck payoff ranking.
    pub(crate) deck_payoff_refusal: Option<String>,
    pub(crate) domain: CandidateDomain,
    pub(crate) song: Option<SongView>,
    pub(crate) event: bool,
    pub(crate) skip: Option<ournotes_sim::live::skip::SkipEvaluator>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_execution(
    pool: &Pool,
    request: &SearchRequest,
    metric: &Metric,
    event_input: Option<&EventPayoffInput>,
    network: Option<&[RankConfirmation]>,
    simulation: &SimulationInput,
    strategy: &Strategy,
) -> Result<ExecutionPlan, Error> {
    reject_unsupported_lifecycle(network, simulation.live_finished_from_frame)?;
    let (song, event, skip) = crate::search::objective_song(pool, &request.objective)?;
    validate_payoff(pool, request, metric, event_input)?;
    if matches!(request.objective.inner(), Objective::LiveScore { .. }) {
        validate_play(pool, request, network, simulation)?;
    } else if network.is_some()
        || simulation.music_length_ms.is_some()
        || simulation.score_music_length_ms.is_some()
        || simulation.live_finished_from_frame.is_some()
    {
        return Err(Error::Input("simulation/network inputs apply only to played live".into()));
    }
    let domain = CandidateDomain::build(pool, &request.constraints)?;
    let started = crate::clock::Instant::now();
    let mut deck_payoff = None;
    let mut deck_payoff_refusal = None;
    let mut team_power = None;
    let (joint, bound_fallback) = if matches!(strategy, Strategy::BranchAndBound)
        && crate::search::team_power::applies(&request.objective, metric)
    {
        match crate::search::team_power::TeamPowerBounds::compile(pool, request, &domain, metric) {
            Ok(bound) => {
                team_power = Some(bound);
                (None, None)
            }
            Err(reason) => (None, Some(reason.to_string())),
        }
    } else if matches!(strategy, Strategy::BranchAndBound)
        && (matches!(metric, Metric::ConditionalClientEventItems { .. })
            || (matches!(request.objective.inner(), Objective::SkipScore { .. })
                && matches!(metric, Metric::ClientEventPoints { .. } | Metric::ClientChallengePoints { .. })))
    {
        match crate::search::deck_payoff::DeckPayoffBounds::compile(pool, request, &domain, metric, event_input, None) {
            Ok(bound) => {
                deck_payoff = Some(bound);
                (None, None)
            }
            Err(reason) => (None, Some(reason.to_string())),
        }
    } else if matches!(strategy, Strategy::BranchAndBound) {
        match crate::search::joint::JointBounds::compile(pool, request, &domain, metric, event_input, simulation) {
            Ok(bound) => {
                // A played Live whose payoff steps with the local score is first ranked by deck under its score cap.
                if let Some(steps) = bound.score_steps() {
                    match crate::search::deck_payoff::DeckPayoffBounds::compile(
                        pool,
                        request,
                        &domain,
                        metric,
                        event_input,
                        Some(steps),
                    ) {
                        Ok(b) => deck_payoff = Some(b),
                        Err(reason) => deck_payoff_refusal = Some(reason.to_string()),
                    }
                }
                (Some(bound), None)
            }
            Err(reason) => (None, Some(reason.to_string())),
        }
    } else {
        (None, None)
    };
    let bound_compile_ms =
        if matches!(strategy, Strategy::BranchAndBound) { started.elapsed().as_secs_f64() * 1000.0 } else { 0.0 };
    Ok(ExecutionPlan {
        joint,
        deck_payoff,
        team_power,
        bound_compile_started: started,
        bound_compile_ms,
        bound_fallback,
        deck_payoff_refusal,
        domain,
        song,
        event,
        skip: skip.map(|s| s.fast),
    })
}

/// Resolve cultivation, scenario, full Snap skill execution, objective and legal domain.
pub fn build_card_pool<'m>(
    data: &'m DeckData,
    roster: &Roster,
    r: &RecommendationRequest,
) -> Result<BuiltProblem<'m>, Error> {
    build_card_pool_inner(data, roster, r, true)
}

/// Validate and resolve the original domain for native partitioning. Workers
/// compile bounds for their actual domains after this preparation is discarded.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn prepare_partition_domain<'m>(
    data: &'m DeckData,
    roster: &Roster,
    r: &RecommendationRequest,
) -> Result<BuiltProblem<'m>, Error> {
    build_card_pool_inner(data, roster, r, false)
}

fn build_card_pool_inner<'m>(
    data: &'m DeckData,
    roster: &Roster,
    r: &RecommendationRequest,
    compile_bounds: bool,
) -> Result<BuiltProblem<'m>, Error> {
    // Explicitly unavailable in the unchanged current numerical core.
    reject_unsupported_lifecycle(r.network_confirmations.as_deref(), r.simulation.live_finished_from_frame)?;

    if r.format != REQUEST_FORMAT {
        return Err(Error::Input(format!("unsupported recommendation format {}", r.format)));
    }
    let player_goal = goal_description(r)?;
    let score_id = match r.execution {
        Execution::Power { .. } => None,
        Execution::Skip { score_id } | Execution::Live { score_id, .. } => Some(score_id),
    };
    if score_id.is_some() && r.scenario.is_none() {
        return Err(Error::Input("skip/live require an explicit scenario".into()));
    }
    let context_input = r.context.clone().unwrap_or(ContextInput {
        power_snapshot: PowerSnapshotInput { event_ids: roster.player.events.clone(), captured_jst_ticks: None },
        result_clock: None,
        event_payoff: None,
    });
    let fevers = score_id.and_then(|i| data.data_chart(i)).map(|c| c.fevers.as_slice()).unwrap_or(&[]);
    let mut context =
        r.scenario.as_ref().map(|s| context_input.resolve(&data.master, s.scenario(), score_id, fevers)).transpose()?;
    if let Some(context) = &mut context {
        context.rank_confirmations = r.network_confirmations.clone();
    }
    let pool = match &context {
        Some(c) => c.pool(&data.master, roster)?,
        None => {
            let mut frozen = roster.clone();
            frozen.player.events = context_input.power_snapshot.event_ids.clone();
            Pool::new(&data.master, &frozen)?
        }
    };
    let objective = match &r.execution {
        Execution::Power { music_id, event_parameter } => {
            if context.is_some() && *event_parameter {
                return Err(Error::Input("explicit scenario determines event parameters".into()));
            }
            Objective::Power { music_id: *music_id, event: *event_parameter }
        }
        Execution::Skip { score_id } => Objective::SkipScore { score_id: *score_id, chart: data.chart(*score_id)? },
        Execution::Live { score_id, gekisou, play } => {
            let chart = data.chart(*score_id)?;
            let dc = data.data_chart(*score_id).ok_or_else(|| Error::Input("chart is absent".into()))?;
            let ctx = context.as_ref().expect("explicit live scene");
            let stream = match play {
                PlayPolicy::Stream { stream } => stream.clone(),
                PlayPolicy::TheoreticalBest if *gekisou => JudgementStream::theoretical_best_gekisou(
                    &chart,
                    &dc.judgement_types,
                    &JustRule::new(&data.master, &ctx.gekisou)?,
                )?,
                PlayPolicy::TheoreticalBest => JudgementStream::theoretical_best(&chart),
                PlayPolicy::Accuracy(accuracy) => {
                    let rule = gekisou.then(|| JustRule::new(&data.master, &ctx.gekisou)).transpose()?;
                    JudgementStream::with_accuracy(&chart, &dc.judgement_types, rule.as_ref(), *accuracy)?.0
                }
            };
            Objective::LiveScore {
                score_id: *score_id,
                chart,
                play: PlayInput::Stream { stream, judgement_types: dc.judgement_types.clone() },
                event: false,
                exclude_snap_skills: false,
                gekisou: gekisou.then(|| GekisouObjective { seeds: SeedSet::List(vec![0]), fevers: dc.fevers.clone() }),
            }
        }
    };
    let objective = match &context {
        Some(c) => objective.in_scenario(c.clone()),
        None => objective,
    };
    let objective = expectation::normalized_objective(&objective);
    let request = SearchRequest { objective, k: r.k, constraints: r.constraints.clone(), time_limit: None };
    validate(r.k, &r.limits, &r.strategy)?;
    if r.initial_decks.len() > MAX_INITIAL_DECKS {
        return Err(Error::Capacity(format!("at most {MAX_INITIAL_DECKS} initialDecks")));
    }
    validate_payoff(&pool, &request, &r.metric, context_input.event_payoff.as_ref())?;
    let resolved_context = serde_json::json!({"dataProvenance":data.provenance,"scenario":context.as_ref().map(|c|format!("{:?}",c.scenario)),"baseLiveMusicId":context.as_ref().map(|c|c.resolved.live_music_id),"scoreId":score_id,"calcEventParameter":context.as_ref().map(|c|c.resolved.calc_event_parameter),"skillTargetMusicType":context.as_ref().map(|c|c.resolved.skill_target_music_type),"gekisouMissions":context.as_ref().map(|c|c.resolved.gekisou_missions),"context":context_input,"simulation":r.simulation,"networkConfirmations":r.network_confirmations,"playPolicy":match &r.execution {Execution::Live{play:PlayPolicy::TheoreticalBest,..}=>"explicit theoretical AP/Just scenario",Execution::Live{play:PlayPolicy::Accuracy(_),..}=>"theoretical play with declared Great/Just shares",Execution::Live{..}=>"declared judgement stream",_=>"deterministic"},"eligibility":"unlock/progression eligibility not inferred","nativeEvidence":"1.0.1-25 AArch64; selected regional master is explicit data, online patch/version parity not inferred"});
    let plan = compile_execution(
        &pool,
        &request,
        &r.metric,
        context_input.event_payoff.as_ref(),
        r.network_confirmations.as_deref(),
        &r.simulation,
        if compile_bounds { &r.strategy } else { &Strategy::Exhaustive },
    )?;
    let route = match &r.strategy {
        Strategy::Exhaustive => SolverRoute::PhysicalExhaustive,
        Strategy::BranchAndBound => SolverRoute::PhysicalBranchAndBound,
        Strategy::Candidate { .. } => SolverRoute::PhysicalCandidate,
    };
    if crate::search::team_power::applies(&request.objective, &r.metric)
        && matches!(r.strategy, Strategy::Candidate { .. })
    {
        return Err(Error::Input("power/skip score and monotone score targets use exact canonical search; candidate strategy applies to physical metrics".into()));
    }
    let context = SearchContext {
        request,
        context_input,
        player_goal,
        resolved_context,
        spec: r.clone(),
        route,
        plan,
        data: crate::search::telemetry::DataIdentity::of(data),
    };
    Ok(BuiltProblem { pool, context })
}
