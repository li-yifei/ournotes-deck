//! Opt-in bound auditing and harness ablations, separate from production policy.
//!
//! Every bound audited here is a payoff numerator over the 120 performance orders (the denominator of every played
//! result), so an admissible bound of a prefix is at least the numerator of each of its completions.
use super::expectation::PhysicalDeck;
use super::uniform::{self, MEAN_ORDERS, ORDERS};
mod cutoff;
pub use cutoff::audit_cutoff;

/// Harness-only schedules; never part of the player recommendation request.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Schedule {
    #[default]
    Production,
    Classes,
    ClassesWithResource,
}

/// Construct and execute an ablation with the same deadline and evaluator contract.
pub fn recommend_experiment(
    data: &ournotes_sim::data::DeckData,
    roster: &ournotes_sim::cards::Roster,
    request: &crate::types::RecommendationRequest,
    schedule: Schedule,
) -> Result<crate::types::RecommendationOutcome, Error> {
    let start = crate::clock::Instant::now();
    let mut built = crate::handler::build_card_pool(data, roster, request)?;
    configure_schedule(&mut built, schedule)?;
    super::dispatch::execute(&built, None, start, start.elapsed().as_secs_f64() * 1000.0, None)
}

fn configure_schedule(built: &mut BuiltProblem<'_>, schedule: Schedule) -> Result<(), Error> {
    if matches!(schedule, Schedule::Production) {
        return Ok(());
    }
    let plan = &mut built.context.plan;
    let joint = plan.joint.as_mut().ok_or_else(|| Error::Domain("class schedule requires joint bounds".into()))?;
    let start = crate::clock::Instant::now();
    joint.enable_class_search(&built.pool, &plan.domain, matches!(schedule, Schedule::ClassesWithResource))?;
    plan.bound_compile_ms += start.elapsed().as_secs_f64() * 1000.0;
    Ok(())
}

/// Enable the strongest class cap where applicable for independent oracle audits.
pub fn prepare_class_audit(built: &mut BuiltProblem<'_>) -> Result<bool, Error> {
    if !built.context.plan.joint.as_ref().is_some_and(|b| b.has_class_bounds()) {
        return Ok(false);
    }
    configure_schedule(built, Schedule::ClassesWithResource)?;
    Ok(true)
}

/// The performance orders the per-order audits sample: the five cyclic shifts of the slots (every slot takes every
/// position once) and their reverses.
pub fn audit_orders() -> Vec<[usize; 5]> {
    let cyclic: Vec<[usize; 5]> = (0..5).map(|r| std::array::from_fn(|k| (k + r) % 5)).collect();
    let reversed: Vec<[usize; 5]> = cyclic.iter().map(|o| std::array::from_fn(|k| o[4 - k])).collect();
    cyclic.into_iter().chain(reversed).collect()
}

/// A legal deck of the built domain by public IDs.
fn physical(built: &BuiltProblem<'_>, members: [i64; 5], snaps: [Option<i64>; 5]) -> Result<PhysicalDeck, Error> {
    let d = built.pool.deck(members, snaps, [0, 1, 2, 3, 4])?;
    let p = PhysicalDeck { members: d.members, snaps: d.snaps };
    built.domain().check_fixed(&built.pool, &p)?;
    Ok(p)
}

/// The exact power of a deck.
fn power_of(built: &BuiltProblem<'_>, p: &PhysicalDeck) -> Result<i64, Error> {
    let plan = &built.context.plan;
    Ok(i64::from(built.pool.deck_power(&p.as_deck(), plan.song.as_ref(), plan.event)?.power()))
}

/// Explain the complete-deck relaxation at one performance order without running or changing the scorer.
pub fn describe_bound(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    order: [usize; 5],
) -> Result<Option<serde_json::Value>, Error> {
    let p = physical(built, members, snaps)?;
    let Some(b) = built.context.plan.joint.as_ref() else { return Ok(None) };
    Ok(Some(b.describe(&built.pool, built.domain(), &p, &uniform::positions_of(&order))))
}
use crate::handler::BuiltProblem;
use ournotes_sim::Error;
use ournotes_sim::live::{full::LiveModel, random::LiveRandom};

/// Pair the fine cap's per-entry terms with one actual simulation of the same deck at each audited order. Each note's
/// last executed score and the filed fixed scores come from the scorer; the bound terms are not themselves a bound.
pub fn slack_profile(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
) -> Result<Vec<serde_json::Value>, Error> {
    let p = physical(built, members, snaps)?;
    let Some(b) = built.context.plan.joint.as_ref() else { return Ok(Vec::new()) };
    let input = super::expectation::context(&built.pool, &p, &built.context.request.objective)?;
    let power = power_of(built, &p)?;
    let mut scratch = super::snaps::JointScratch::default();
    let mut out = Vec::new();
    for order in audit_orders() {
        let positions = uniform::positions_of(&order);
        let Some((fine, terms)) = b.fine_trace(built.domain(), &p, power, &positions) else { continue };
        let leaf = b.fine_upper(built.domain(), &p, power, &positions, &mut scratch);
        let outcome = input.simulate_performance_order(built.pool.master, order)?;
        let (notes, fixed) = outcome.model.filed_scores();
        let bound_cb = b.fine_cb_windows(built.domain(), &p, &positions);
        let actual_cb = outcome.model.gk_combo_bonus_commands();
        out.push(serde_json::json!({"order":order,"positions":positions,"power":power,
            "boundComboWindows":bound_cb,"actualComboBonus":actual_cb,"leafFineUpper":leaf.map(|v| v.to_string()),
            "actualPower":input.params.total_power,"actualScore":outcome.final_score,"fineUpper":fine,
            "boundTerms":terms,"actualNotes":notes,"actualFixed":fixed}));
    }
    Ok(out)
}

/// Compare one exact recorded program with fresh complete simulations at other
/// initial powers, at each audited order. Only power varies: this does not merge physical decks, prove
/// monotonicity, settle PT, or authorize any pruning.
pub fn audit_score_program(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    powers: &[i32],
) -> Result<serde_json::Value, Error> {
    use super::expectation::context;
    use crate::clock::Instant;
    let physical = physical(built, members, snaps)?;
    let mut input = context(&built.pool, &physical, &built.context.request.objective)?;
    let spec = &built.context.spec;
    if spec.network_confirmations.is_some() || spec.simulation.live_finished_from_frame.is_some() {
        return Err(Error::Unsupported("score programs require the declared solo lifecycle".into()));
    }
    if let Some(v) = spec.simulation.music_length_ms {
        input.params.music_length_ms = v;
    }
    if let Some(v) = spec.simulation.score_music_length_ms {
        input.params.score_music_length_ms = Some(v);
    }
    let mut results = Vec::new();
    for order in audit_orders() {
        let random = LiveRandom::new(0);
        let performers = order.map(|slot| input.performers[slot].clone());
        let model = |power| {
            let params = ournotes_sim::live::full::LiveParams { total_power: power, ..input.params };
            match &input.gekisou {
                Some(g) => {
                    LiveModel::new_gekisou(built.pool.master, &performers, &input.notes, &input.events, params, g)
                }
                None => LiveModel::new(built.pool.master, &performers, &input.notes, &input.events, params),
            }
        };
        let start = Instant::now();
        let (program, origin) =
            model(input.params.total_power)?.compile_score_program(&input.play, &input.delta_times, random.clone())?;
        let compile_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        let normalized = program.normalized_additive();
        let normalize_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        let certified = program.certify_nondecreasing(0, 2_000_000);
        let certificate_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut samples = Vec::new();
        for &power in powers {
            let start = Instant::now();
            let score = program.evaluate(power);
            let program_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let normalized_score = normalized.evaluate(power);
            let normalized_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let mut fresh = model(power)?;
            let expected = fresh.run_with_random(&input.play, &input.delta_times, random.clone())?;
            let native_ms = start.elapsed().as_secs_f64() * 1000.0;
            if score != expected
                || normalized_score != expected
                || origin.current_life() != fresh.current_life()
                || origin.converted_judgements() != fresh.converted_judgements()
            {
                return Err(Error::Game(format!("score program differs at order {order:?}, power {power}")));
            }
            if (0..=2_000_000).contains(&power)
                && certified.is_some_and(|(lo, hi)| {
                    score < lo || score > hi || power == 0 && score != lo || power == 2_000_000 && score != hi
                })
            {
                return Err(Error::Game("score program certificate disagrees with evaluation".into()));
            }
            samples.push(
                serde_json::json!({"power":power,"score":score,"programMs":program_ms,"normalizedProgramMs":normalized_ms,"freshSimulationMs":native_ms}),
            );
        }
        results.push(serde_json::json!({"order":order,"nodes":program.node_count(),
            "bytes":program.allocated_bytes(),"normalizedNodes":normalized.node_count(),"normalizedBytes":normalized.allocated_bytes(),"normalizeMs":normalize_ms,
            "compileMs":compile_ms,"originPower":program.origin_power(),"originScore":program.origin_score(),"samples":samples,
            "certificatePowerRange":[0,2_000_000],"certifiedScoreRange":certified,"certificateMs":certificate_ms}));
    }
    Ok(
        serde_json::json!({"scope":"Same complete performer programs and declared clocks per performance order; only total power varies. Exact score replay; optional checked monotonicity only within certificatePowerRange, no cross-deck or PT certificate.",
        "members":members,"snaps":snaps,"allComparedValuesEqual":true,"orders":results}),
    )
}

/// One compiled problem only; recreate when changing BuiltProblem or its domain.
#[derive(Default)]
pub struct PrefixAuditScratch {
    bonus: super::joint::BonusScratch,
    classes: super::snaps::JointScratch,
    class_caps: std::collections::HashMap<ClassAuditKey, (i128, i64)>,
    pub class_bound_computations: u64,
    pub class_bound_cache_hits: u64,
    pub checked_bonus_prefixes: u64,
    pub checked_resource_prefixes: u64,
    /// Prefixes also bounded by the carrier split (see `JointBounds::carrier_split_expected_upper`).
    pub checked_carrier_split_prefixes: u64,
    pub checked_character_prefixes: u64,
    /// Complete decks whose per-order caps (cheap, raw, fine) were summed.
    pub checked_order_leaves: u64,
    fine: super::snaps::JointScratch,
}
type ClassAuditKey = ([i64; 5], [Vec<usize>; 5], bool);

/// A class-prefix relaxation or a physical Snap prefix within the complete class vector.
pub fn class_prefix_upper(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    depth: usize,
    bindings: bool,
    scratch: &mut PrefixAuditScratch,
) -> Result<Option<(i128, i64)>, Error> {
    if depth > 5 {
        return Err(Error::Input("class depth must be 0..=5".into()));
    }
    let Some(b) = built.context.plan.joint.as_ref().filter(|b| b.has_class_bounds()) else {
        return Ok(None);
    };
    let p = physical(built, members, snaps)?;
    let all: Vec<_> = (0..=built.domain().snaps().len()).collect();
    let mut allowed = std::array::from_fn(|_| all.clone());
    for (at, &slot) in super::joint::SLOTS.iter().enumerate() {
        let choice = p.snaps[slot].map_or(0, |s| built.domain().snaps().iter().position(|&v| v == s).unwrap() + 1);
        if bindings && at < depth {
            allowed[slot] = vec![choice];
        } else if bindings || at < depth {
            allowed[slot] =
                b.effect_groups(p.members[slot], &all).into_iter().find(|g| g.contains(&choice)).expect("actual class");
        }
    }
    let key = (members, allowed.clone(), bindings || depth == 5);
    if let Some(&value) = scratch.class_caps.get(&key) {
        scratch.class_bound_cache_hits += 1;
        return Ok(Some(value));
    }
    let value = b
        .class_bound(built.domain(), &p, &allowed, &MEAN_ORDERS, bindings || depth == 5, &mut scratch.classes)?
        .ok_or_else(|| Error::Domain("actual legal completion lost by class matching".into()))?;
    scratch.class_bound_computations += 1;
    let value = (value.payoff, value.power);
    if scratch.class_caps.len() < 100_000 {
        scratch.class_caps.insert(key, value);
    }
    Ok(Some(value))
}

/// Audit either a leader/nonleader composition (depth members, their Snaps free), or a whole team with the Snaps of
/// its first `depth` search slots assigned (`team`).
pub fn split_prefix_upper(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    depth: usize,
    team: bool,
) -> Result<Option<(i128, i64)>, Error> {
    if depth > 5 || (!team && depth == 0) {
        return Err(Error::Input("invalid split audit depth".into()));
    }
    let p = physical(built, members, snaps)?;
    let Some(b) = built.context.plan.joint.as_ref() else { return Ok(None) };
    let member_depth = if team { 5 } else { depth };
    let snap_depth = if team { depth } else { 0 };
    b.composition_expected_upper(&built.pool, built.domain(), &p, member_depth, snap_depth, !team, &MEAN_ORDERS)
        .map(Some)
}

/// One bound module's bound of a prefix.
pub struct ModuleBound {
    pub name: &'static str,
    /// The prefix's Snaps placed (joint traversal) or free (composition traversal).
    pub snaps_placed: bool,
    pub upper: Option<i128>,
}

/// The bounds of the bound modules (see `joint::NodeBound`) at the prefix of the first `depth` search slots, with the
/// prefix's Snaps placed and free.
pub fn module_prefix_uppers(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    depth: usize,
) -> Result<Vec<ModuleBound>, Error> {
    if !(1..=5).contains(&depth) {
        return Err(Error::Input("bound module audit depth must be 1..=5".into()));
    }
    let p = physical(built, members, snaps)?;
    let Some(b) = built.context.plan.joint.as_ref() else { return Ok(Vec::new()) };
    Ok(b.modules()
        .iter()
        .flat_map(|m| {
            [true, false].map(|snaps_placed| ModuleBound {
                name: m.name(),
                snaps_placed,
                upper: m.node_upper(&built.pool, &p, depth, snaps_placed, &MEAN_ORDERS),
            })
        })
        .collect())
}

/// A bonus-conditioned cap on the payoff numerator.
pub fn bonus_expected_prefix_upper(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    depth: usize,
    scratch: &mut PrefixAuditScratch,
) -> Result<Option<i128>, Error> {
    if !(1..5).contains(&depth) {
        return Err(Error::Input("bonus audit depth must be 1..=4".into()));
    }
    let p = physical(built, members, snaps)?;
    let Some(bounds) = &built.context.plan.joint else {
        return Ok(None);
    };
    bounds.bonus_expected_upper(&built.pool, built.domain(), &p, depth, &MEAN_ORDERS, &mut scratch.bonus)
}

/// Drop request-local carrier envelopes/tables to audit cold/warm equivalence.
pub fn reset_carrier_split_cache(built: &BuiltProblem<'_>) -> bool {
    built.context.plan.joint.as_ref().is_some_and(|b| b.reset_carrier_split_cache())
}

/// Bound the leader-first prefix of a complete legal deck by every node bound of the joint traversal, as a payoff
/// numerator; a complete deck (depth 5) also by the sum of its per-order caps. The caller independently enumerates
/// completions and checks each one's numerator against this cap.
pub fn prefix_upper(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    depth: usize,
    scratch: &mut PrefixAuditScratch,
) -> Result<Option<(i128, i64)>, Error> {
    if !(1..=5).contains(&depth) {
        return Err(Error::Input("bound audit depth must be 1..=5".into()));
    }
    let p = physical(built, members, snaps)?;
    let Some(b) = built.context.plan.joint.as_ref() else { return Ok(None) };
    // Position-mean gains: any positions read the same mean bound.
    let positions = [0, 1, 2, 3, 4];
    b.check_relax_tables(&built.pool, built.domain(), &p, depth, &super::joint::SLOTS[depth..], &positions)?;
    if depth < 4 {
        b.check_relax_tables(&built.pool, built.domain(), &p, depth, &super::joint::SLOTS[depth + 1..], &positions)?;
    }
    let (mut cap, power) = b.upper(&built.pool, built.domain(), &p, depth, &positions);
    if let Some(bonus) = b.bonus_upper_at(&built.pool, built.domain(), &p, depth, &positions, &mut scratch.bonus) {
        cap = cap.min(bonus);
        scratch.checked_bonus_prefixes += 1;
    }
    if depth < 5 {
        if let Some(character) = b.character_prefix_upper(&built.pool, built.domain(), &p, depth, &positions) {
            cap = cap.min(character);
            scratch.checked_character_prefixes += 1;
        } else {
            cap = cap.min(b.correlated_upper(&built.pool, built.domain(), &p, depth, &positions));
        }
        if let Some(resource) = b.resource_prefix_upper(&built.pool, built.domain(), &p, depth, &positions) {
            cap = cap.min(resource);
            scratch.checked_resource_prefixes += 1;
        }
    }
    let mut numerator = cap.saturating_mul(ORDERS as i128);
    if let Some(split) = b.carrier_split_coupled_upper(built.domain(), &p, depth, 0, &MEAN_ORDERS) {
        numerator = numerator.min(split);
        scratch.checked_carrier_split_prefixes += 1;
    }
    if depth == 5 {
        let exact = power_of(built, &p)?;
        let positions: Vec<_> = uniform::all_orders().iter().map(uniform::positions_of).collect();
        let mut caps = b.order_cheap_caps(built.domain(), &p, exact, &positions);
        b.tighten_order_caps(built.domain(), &p, exact, &positions, &mut caps, &mut scratch.fine);
        numerator = numerator.min(caps.iter().fold(0i128, |a, &c| a.saturating_add(c)));
        scratch.checked_order_leaves += 1;
    }
    let power = b.assignment_power_upper(&built.pool, built.domain(), &p, depth).map_or(power, |m| power.min(m));
    Ok(Some((numerator, power)))
}

/// Check each per-order cap of a complete deck (cheap, raw and fine, as the leaf evaluation reads them) against the
/// exact payoff of that order, and its final life cap, when it has one, against the order's final life, over all 120
/// orders. Returns the orders checked, the violations found and the orders a score and life target caps at zero for
/// their final life.
pub fn audit_order_caps(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
) -> Result<serde_json::Value, Error> {
    let p = physical(built, members, snaps)?;
    let Some(b) = built.context.plan.joint.as_ref() else { return Ok(serde_json::Value::Null) };
    let power = power_of(built, &p)?;
    let orders = uniform::all_orders();
    let positions: Vec<_> = orders.iter().map(uniform::positions_of).collect();
    let mut caps = b.order_cheap_caps(built.domain(), &p, power, &positions);
    b.tighten_order_caps(built.domain(), &p, power, &positions, &mut caps, &mut super::snaps::JointScratch::default());
    let input = super::expectation::context(&built.pool, &p, &built.context.request.objective)?;
    let spec = &built.context.spec;
    let mut bad = Vec::new();
    let mut life_capped = 0;
    for (i, order) in orders.iter().enumerate() {
        let outcome = input.simulate_performance_order(built.pool.master, *order)?;
        let life = outcome.model.current_life();
        if let Some(cap) = b.final_life_cap(built.domain(), &p, &positions[i])
            && i64::from(life) > cap
        {
            bad.push(serde_json::json!({"order":order,"finalLifeCap":cap,"finalLife":life}));
        }
        life_capped += usize::from(b.order_short_of_final_life(built.domain(), &p, &positions[i]));
        let payoff = super::physical::payoff_of(
            &built.pool,
            &built.context.request,
            &spec.metric,
            built.context.context_input.event_payoff.as_ref(),
            &p,
            outcome.final_score,
            power as i32,
            Some(outcome.model.current_life()),
        )?;
        if caps[i] < payoff {
            bad.push(serde_json::json!({"order":order,"cap":caps[i].to_string(),"payoff":payoff.to_string()}));
        }
    }
    Ok(serde_json::json!({"orders":orders.len(),"violations":bad.len(),"first":bad.first(),"lifeCapped":life_capped}))
}

/// The joint traversal's bounds along the path that reaches a complete legal deck: the leader, then the other pairs
/// in choice order. Per depth: the carrier level and the node bounds as payoff numerators (pool-wide, carrier level,
/// keyed, correlated, resource) and the pair cap of the path's next pair from the parent; at depth 5 the sums of the
/// per-order cheap and fine caps. Numerators are over the 120 orders.
pub fn path_bounds(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
) -> Result<serde_json::Value, Error> {
    use super::joint::{JointBounds, SLOTS};
    let full = physical(built, members, snaps)?;
    let Some(b) = built.context.plan.joint.as_ref() else { return Ok(serde_json::Value::Null) };
    let (pool, domain) = (&built.pool, built.domain());
    let choice_of = |slot: usize| {
        let c = full.snaps[slot].map_or(0, |s| domain.snaps().iter().position(|&v| v == s).expect("compiled Snap") + 1);
        b.choices.iter().position(|&x| x == (full.members[slot], c)).expect("compiled pair")
    };
    let mut rest: Vec<usize> = [0, 1, 3, 4].into_iter().map(choice_of).collect();
    rest.sort_unstable();
    let mut path = vec![choice_of(2)];
    path.extend(rest);
    let mut p = PhysicalDeck { members: [0; 5], snaps: [None; 5] };
    let s = |v: i128| v.to_string();
    let mut out = Vec::new();
    for depth in 0..=5 {
        let choices = JointBounds::prefix_choices(domain, &p, depth);
        let placed = b.carriers_placed(&p, depth, &choices);
        let level = (placed + 5 - depth).min(5);
        let nb = b.carrier_level(level);
        let keyed = if depth > 0 { b.keyed(&p, depth, &choices, 5 - depth, 5 - depth) } else { None };
        let mut row = serde_json::json!({"depth":depth,"carriersPlaced":placed,"level":level,"keyed":keyed.is_some()});
        if depth > 0 {
            row["poolWide"] = s(b.expected_upper(pool, domain, &p, depth, &MEAN_ORDERS)?.0).into();
            row["levelUpper"] = s(nb.expected_upper(pool, domain, &p, depth, &MEAN_ORDERS)?.0).into();
            let (keyed_upper, power) =
                nb.expected_upper_keyed(pool, domain, &p, depth, &MEAN_ORDERS, keyed.as_ref())?;
            row["keyedUpper"] = s(keyed_upper).into();
            row["power"] = power.into();
            row["scoreProfile"] = nb.node_score_profile(pool, domain, &p, depth, keyed.as_ref());
        }
        if (1..5).contains(&depth) {
            row["correlated"] =
                s(nb.correlated_expected_upper_keyed(pool, domain, &p, depth, &MEAN_ORDERS, keyed.as_ref())?).into();
            row["resource"] = nb.resource_expected_upper(pool, domain, &p, depth, &MEAN_ORDERS).map(s).into();
            row["carrierSplit"] =
                b.carrier_split_expected_upper(domain, &p, depth, 0, &MEAN_ORDERS, i128::MAX).map(s).into();
            // the search's node reads the suffix after the last placed pair below the leader
            let start = if depth == 1 { 0 } else { path[depth - 1] + 1 };
            row["carrierSplitStart"] =
                b.carrier_split_expected_upper(domain, &p, depth, start, &MEAN_ORDERS, i128::MAX).map(s).into();
            row["carrierSplitCoupled"] =
                b.carrier_split_coupled_upper(domain, &p, depth, start, &MEAN_ORDERS).map(s).into();
            if depth >= 2 && std::env::var_os("PATH_BOUNDS_COMPLETIONS").is_some() {
                row["carrierSplitCompletions"] =
                    b.carrier_split_completions(pool, domain, &p, depth, start, &MEAN_ORDERS, 1 << 29).into();
            }
            row["modules"] = b
                .modules()
                .iter()
                .map(|m| serde_json::json!([m.name(), m.node_upper(pool, &p, depth, true, &MEAN_ORDERS).map(s)]))
                .collect::<Vec<_>>()
                .into();
        }
        if depth == 5 {
            let power = power_of(built, &p)?;
            let positions: Vec<_> = uniform::all_orders().iter().map(uniform::positions_of).collect();
            let scores = b.order_score_caps(domain, &p, power, &positions);
            row["orderScoreCaps"] = serde_json::json!({"min":s(*scores.iter().min().expect("orders")),
                "max":s(*scores.iter().max().expect("orders")),"sum":s(scores.iter().sum())});
            let mut caps = b.order_cheap_caps(domain, &p, power, &positions);
            row["cheapOrderSum"] = s(caps.iter().sum()).into();
            b.tighten_order_caps(domain, &p, power, &positions, &mut caps, &mut super::snaps::JointScratch::default());
            row["fineOrderSum"] = s(caps.iter().sum()).into();
            row["attribution"] = audit_orders()
                .iter()
                .take(2)
                .map(|order| b.cheap_fine_attribution(domain, &p, power, &uniform::positions_of(order)))
                .collect::<Vec<_>>()
                .into();
        } else {
            let (m, choice) = b.choices[path[depth]];
            if (1..5).contains(&depth) {
                let low = b.carrier_level((placed + 4 - depth).min(5));
                let high = b.carrier_level((placed + 5 - depth).min(5));
                let carrier = b.is_carrier(m, choice) && !std::ptr::eq(low, high);
                let (pair_bounds, keyed) =
                    if carrier { (high, keyed) } else { (low, b.keyed(&p, depth, &choices, 4 - depth, 5 - depth)) };
                if let Some(state) = pair_bounds.tail_state_keyed(pool, domain, &p, depth, &MEAN_ORDERS, keyed.as_ref())
                {
                    row["nextPair"] = s(pair_bounds.pair_upper(&state, m, choice)?.0).into();
                }
            }
            p.members[SLOTS[depth]] = m;
            p.snaps[SLOTS[depth]] = (choice != 0).then(|| domain.snaps()[choice - 1]);
        }
        out.push(row);
    }
    Ok(serde_json::json!({"path":path,"points":b.point_tiers(),"depths":out}))
}

/// Audit the smallest candidate suffix containing this completion's next pair.
/// Unlike prefix_upper, this cap certifies a suffix-restricted completion set.
pub struct ChoiceBounds {
    pub suffix: (i128, i64),
    pub pair: (i128, i64),
}

pub fn next_choice_bounds(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    depth: usize,
) -> Result<Option<ChoiceBounds>, Error> {
    if !(1..5).contains(&depth) {
        return Err(Error::Input("suffix audit depth must be 1..=4".into()));
    }
    let p = physical(built, members, snaps)?;
    let Some(bounds) = &built.context.plan.joint else {
        return Ok(None);
    };
    let Some(state) = bounds.tail_state(&built.pool, built.domain(), &p, depth, &MEAN_ORDERS) else {
        return Ok(None);
    };
    let slot = super::joint::SLOTS[depth];
    let choice =
        p.snaps[slot].map_or(0, |s| built.domain().snaps().iter().position(|&v| v == s).expect("compiled Snap") + 1);
    let offset = bounds.choices.iter().position(|&x| x == (p.members[slot], choice)).expect("compiled pair");
    let mut suffix = bounds.tail_upper(&state, offset)?;
    // the carrier split from the first choice index of the pairs to fill
    let from = super::joint::SLOTS[depth..]
        .iter()
        .map(|&s| {
            let c = p.snaps[s]
                .map_or(0, |v| built.domain().snaps().iter().position(|&x| x == v).expect("compiled Snap") + 1);
            bounds.choices.iter().position(|&x| x == (p.members[s], c)).expect("compiled pair")
        })
        .min()
        .expect("a slot to fill");
    if let Some(split) = bounds.carrier_split_coupled_upper(built.domain(), &p, depth, from, &MEAN_ORDERS) {
        suffix.0 = suffix.0.min(split);
    }
    Ok(Some(ChoiceBounds { suffix, pair: bounds.pair_upper(&state, p.members[slot], choice)? }))
}

/// Original-domain per-member PT caps (every performance order) for independent membership-filter auditing.
pub fn member_pt_caps(built: &BuiltProblem<'_>) -> Option<Vec<(i64, i128)>> {
    let bounds = built.context.plan.joint.as_ref()?;
    Some(
        bounds
            .member_pt_caps(&built.pool, built.domain())?
            .into_iter()
            .map(|(m, cap)| (built.pool.members[m].id, cap))
            .collect(),
    )
}

/// The seed of played sample `k` of audited order `oi` from base `base`.
fn luck_seed(base: u32, seeds: u32, oi: usize, k: u32) -> LiveRandom {
    let n = base.wrapping_add((oi as u32).wrapping_mul(seeds)).wrapping_add(k);
    LiveRandom::new((n as i32).wrapping_add(1).wrapping_mul(-0x61C8_864F))
}

/// Whether no performer can alter the LUCK chain or its input judgements, so its Rush timeline is chart-only.
pub fn luck_chain_free(built: &BuiltProblem<'_>, members: [i64; 5], snaps: [Option<i64>; 5]) -> Result<bool, Error> {
    use super::expectation::context;
    use ournotes_sim::live::full::{luck_has_judgement_conversion, luck_signature};
    let p = physical(built, members, snaps)?;
    let input = context(&built.pool, &p, &built.context.request.objective)?;
    if luck_has_judgement_conversion(built.pool.master, &input.performers) {
        return Ok(false);
    }
    for performer in &input.performers {
        match luck_signature(built.pool.master, performer)? {
            Some(s) if s.member.is_none() && s.support.is_empty() => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

type LuckInput = (super::expectation::FiniteSeedContext, ournotes_sim::live::full::GekisouSetup);

fn luck_input(built: &BuiltProblem<'_>, members: [i64; 5], snaps: [Option<i64>; 5]) -> Result<LuckInput, Error> {
    use super::expectation::context;
    let p = physical(built, members, snaps)?;
    let mut input = context(&built.pool, &p, &built.context.request.objective)?;
    let sim = &built.context.spec.simulation;
    if let Some(v) = sim.music_length_ms {
        input.params.music_length_ms = v;
    }
    if let Some(v) = sim.score_music_length_ms {
        input.params.score_music_length_ms = Some(v);
    }
    let g = input.gekisou.clone().ok_or_else(|| Error::Input("LUCK sample without Gekisou".into()))?;
    Ok((input, g))
}

/// LUCK table combinations on the chart of a request (the deck only supplies the chart context): `combos` random sets
/// of 2 to `max_k` luck chain skills on four positions (at most one Gekisou skill and two Gekisou support skills per
/// position, formation matches compatible), each with its table deck's probabilities over `runs` lives beside the
/// base and the single entries, for checking how single entries compose.
pub fn luck_combos(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    runs: u32,
    combos: usize,
    max_k: usize,
    seed: u64,
) -> Result<serde_json::Value, Error> {
    use ournotes_sim::chartstats::{LuckSteps, luck_neutral, luck_table_steps};
    use ournotes_sim::live::{
        full::{LuckSkillKey, LuckSource, luck_skills},
        seeds::seed_candidate,
    };
    let master = built.pool.master;
    let (input, g) = luck_input(built, members, snaps)?;
    let skills = luck_skills(master)?;
    let neutral = luck_neutral(master, &skills);
    let seeds: Vec<i32> = (0..u64::from(runs)).map(seed_candidate).collect();
    let sample = |entries: &[(LuckSkillKey, usize)]| -> Result<LuckSteps, Error> {
        luck_table_steps(
            master,
            &skills,
            neutral,
            &input.notes,
            input.params,
            &g,
            &input.play,
            &input.delta_times,
            entries,
            &seeds,
        )
    };
    let mut state = seed | 1;
    let mut below = |n: usize| {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n
    };
    let max_k = max_k.clamp(2, 12);
    let t = std::time::Instant::now();
    let base = sample(&[])?;
    let mut singles: std::collections::HashMap<(LuckSkillKey, usize), LuckSteps> = Default::default();
    let mut out = Vec::new();
    for _ in 0..combos {
        let k = 2 + below(max_k - 1);
        let free = below(5);
        let mut entries: Vec<(LuckSkillKey, usize)> = Vec::with_capacity(k);
        let mut tries = 0;
        while entries.len() < k && tries < 1000 {
            tries += 1;
            let key = skills.chain[below(skills.chain.len())];
            let pos = (free + 1 + below(4)) % 5;
            let here: Vec<_> = entries.iter().filter(|e| e.1 == pos).map(|e| e.0).collect();
            let gk = here.iter().filter(|h| h.source == LuckSource::Gekisou).count();
            let gs = here.len() - gk;
            let fits = match key.source {
                LuckSource::Gekisou => gk == 0,
                LuckSource::GekisouSupport => gs < 2,
            };
            let compatible = here.iter().all(|h| match (h.matched, key.matched) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            });
            if fits && compatible && !here.contains(&key) {
                entries.push((key, pos));
            }
        }
        let Ok(exact) = sample(&entries) else { continue };
        for &e in &entries {
            if let std::collections::hash_map::Entry::Vacant(slot) = singles.entry(e) {
                slot.insert(sample(&[e])?);
            }
        }
        out.push(serde_json::json!({"entries":entries,"exact":exact,
            "singles":entries.iter().map(|e| &singles[e]).collect::<Vec<_>>()}));
    }
    Ok(serde_json::json!({"runs":runs,"base":base,"combos":out,"ms":t.elapsed().as_secs_f64() * 1e3}))
}

/// Replay archived combinations verbatim. `exact` is the joint Monte Carlo reference, not an exact expectation.
/// The DP is reported separately, including its own time and state/transition counts.
pub fn luck_combos_replay(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    runs: u32,
    archived: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    use ournotes_sim::chartstats::{luck_neutral, luck_table_dp, luck_table_steps};
    use ournotes_sim::live::{
        full::{LuckSkillKey, luck_skills},
        seeds::seed_candidate,
    };
    let master = built.pool.master;
    let (input, g) = luck_input(built, members, snaps)?;
    let skills = luck_skills(master)?;
    let neutral = luck_neutral(master, &skills);
    let seeds: Vec<i32> = (0..u64::from(runs)).map(seed_candidate).collect();
    let combos = archived["combos"].as_array().ok_or_else(|| Error::Input("LUCK DP replay needs combos".into()))?;
    let reference_runs = if runs > 0 {
        u64::from(runs)
    } else {
        archived["runs"].as_u64().ok_or_else(|| Error::Input("archived LUCK curve needs its sample count".into()))?
    };
    if reference_runs < 2 {
        return Err(Error::Input("LUCK DP comparison needs at least two reference samples".into()));
    }
    let mut out = Vec::with_capacity(combos.len());
    for (index, combo) in combos.iter().enumerate() {
        let entries: Vec<(LuckSkillKey, usize)> = serde_json::from_value(combo["entries"].clone())
            .map_err(|e| Error::Input(format!("LUCK DP replay combination {index}: {e}")))?;
        let started = std::time::Instant::now();
        let result = luck_table_dp(
            master,
            &skills,
            neutral,
            &input.notes,
            input.params,
            &g,
            &input.play,
            &input.delta_times,
            &entries,
        );
        let ms = started.elapsed().as_secs_f64() * 1e3;
        let dp = match result {
            Ok(dp) => serde_json::json!({"steps":dp.steps,"peakStates":dp.peak_states,
                "transitions":dp.transitions,"ms":ms}),
            Err(e) => serde_json::json!({"error":e.to_string(),"ms":ms}),
        };
        let exact = if runs > 0 {
            luck_table_steps(
                master,
                &skills,
                neutral,
                &input.notes,
                input.params,
                &g,
                &input.play,
                &input.delta_times,
                &entries,
                &seeds,
            )?
        } else {
            serde_json::from_value(combo["exact"].clone())
                .map_err(|e| Error::Input(format!("LUCK DP replay combination {index} has no MC reference: {e}")))?
        };
        out.push(
            serde_json::json!({"archiveComboIndex":combo.get("archiveComboIndex").cloned().unwrap_or(index.into()),
            "entries":entries,"exact":exact,"dp":dp}),
        );
    }
    Ok(serde_json::json!({"runs":reference_runs,"combos":out}))
}

fn luck_certified_output(result: ournotes_sim::live::full::LuckDpCertifiedResult) -> serde_json::Value {
    use ournotes_sim::live::certified::F64Interval;
    let bounds = |interval: F64Interval| serde_json::json!({"lower":interval.lower(),"upper":interval.upper()});
    let mut max_bucket_width = 0f64;
    let steps: Vec<_> = result
        .steps
        .iter()
        .map(|(time, joint)| {
            let buckets = joint.map(|mass| {
                let interval = mass.interval();
                let width = if interval.is_point() { 0.0 } else { (interval.upper() - interval.lower()).next_up() };
                max_bucket_width = max_bucket_width.max(width);
                bounds(interval)
            });
            // Add as real intervals, without ProbabilityMass's [0, 1] clipping: the report must expose
            // accumulated uncertainty on both sides of one rather than hiding it through normalization.
            let total = joint.iter().fold(F64Interval::ZERO, |sum, mass| sum.add(mass.interval()));
            serde_json::json!({"timeMs":time,"buckets":buckets,"totalMass":bounds(total)})
        })
        .collect();
    serde_json::json!({"status":"certified","steps":steps,"probes":result.probes,
        "maxBucketWidth":max_bucket_width,"peakStates":result.peak_states,"transitions":result.transitions})
}

/// All-path native score enclosure of one physical deck in its supplied performance order. Unsupported
/// premises are reported as a diagnostic result, never converted to an exact expectation or search result.
pub fn luck_score_bounds_replay(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
) -> Result<serde_json::Value, Error> {
    let (input, setup) = luck_input(built, members, snaps)?;
    let started = std::time::Instant::now();
    let result = ournotes_sim::live::full::luck_score_bounds_with_ranking(
        built.pool.master,
        &input.performers,
        &input.notes,
        &input.events,
        input.params,
        &setup,
        &input.play,
        &input.delta_times,
        input.rank_confirmations.as_deref(),
    );
    let ms = started.elapsed().as_secs_f64() * 1e3;
    Ok(match result {
        Ok(bounds) => {
            let max_width = bounds.queries.iter().flat_map(|query| query.factor_width).fold(0.0f64, f64::max);
            let rank_width =
                bounds.ranges.iter().map(|range| range.bonus_mean.upper - range.bonus_mean.lower).sum::<f64>();
            serde_json::json!({"status":"bounded","members":members,"snaps":snaps,"ms":ms,
                "width":bounds.final_mean.upper-bounds.final_mean.lower,"maxFactorWidth":max_width,
                "noteWidth":bounds.final_note_mean.upper-bounds.final_note_mean.lower,
                "rankWidth":bounds.final_rank_mean.upper-bounds.final_rank_mean.lower,
                "rankBonusWidthSum":rank_width,"bounds":bounds})
        }
        Err(error) => serde_json::json!({"status":"refused","members":members,"snaps":snaps,
            "error":error.to_string(),"ms":ms}),
    })
}

/// Per performance order of one deck, the certified LUCK curve DP and the production summary that contains it, timed
/// apart, and which orders share a DP curve.
pub fn luck_orders_profile(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
) -> Result<serde_json::Value, Error> {
    let (input, setup) = luck_input(built, members, snaps)?;
    let master = built.pool.master;
    let skills = ournotes_sim::live::full::luck_skills(master)?;
    let mut curves: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    // One cache for the curves alone and one for the summaries, as the search shares it across orders.
    let mut cache = ournotes_sim::live::full::LuckDpCache::new(usize::MAX);
    let mut summary_cache = ournotes_sim::live::full::LuckDpCache::new(usize::MAX);
    let curve_text = |result: &ournotes_sim::live::full::LuckDpCertifiedResult| {
        format!("{:?}{:?}{}/{}", result.steps, result.probes, result.peak_states, result.transitions)
    };
    for order in uniform::all_orders() {
        let performers: Vec<_> = order.iter().map(|&slot| input.performers[slot].clone()).collect();
        let started = std::time::Instant::now();
        let dp = ournotes_sim::live::full::luck_rush_dp_certified_with_ranking(
            master,
            &skills,
            &input.notes,
            &input.events,
            input.params,
            &setup,
            &input.play,
            &input.delta_times,
            &performers,
            None,
            input.rank_confirmations.as_deref(),
        );
        let dp_ms = started.elapsed().as_secs_f64() * 1e3;
        let started = std::time::Instant::now();
        let cached = cache.certified(
            master,
            &skills,
            &input.notes,
            &input.events,
            input.params,
            &setup,
            &input.play,
            &input.delta_times,
            &performers,
            None,
            input.rank_confirmations.as_deref(),
        );
        let cached_ms = started.elapsed().as_secs_f64() * 1e3;
        let same_curve = match (&dp, &cached) {
            (Ok(a), Ok(b)) => curve_text(a) == curve_text(b),
            (Err(a), Err(b)) => a == b,
            _ => false,
        };
        let (curve, states, transitions) = match &dp {
            Ok(result) => (format!("{:?}{:?}", result.steps, result.probes), result.peak_states, result.transitions),
            Err(error) => (format!("error {error}"), 0, 0),
        };
        let index = curves.iter().position(|c| *c == curve).unwrap_or_else(|| {
            curves.push(curve);
            curves.len() - 1
        });
        let started = std::time::Instant::now();
        let summary = ournotes_sim::live::full::luck_score_summary_with_ranking(
            master,
            &skills,
            &performers,
            &input.notes,
            &input.events,
            input.params,
            &setup,
            &input.play,
            &input.delta_times,
            input.rank_confirmations.as_deref(),
        );
        let summary_ms = started.elapsed().as_secs_f64() * 1e3;
        let started = std::time::Instant::now();
        let summary_cached = ournotes_sim::live::full::luck_score_summary_with_curves(
            master,
            &skills,
            &performers,
            &input.notes,
            &input.events,
            input.params,
            &setup,
            &input.play,
            &input.delta_times,
            input.rank_confirmations.as_deref(),
            Some(&mut summary_cache),
        );
        let summary_cached_ms = started.elapsed().as_secs_f64() * 1e3;
        let same_summary = format!("{summary:?}") == format!("{summary_cached:?}");
        rows.push(serde_json::json!({"order":order,"dpMs":dp_ms,"cachedMs":cached_ms,"sameCurve":same_curve,
            "summaryMs":summary_ms,"summaryCachedMs":summary_cached_ms,"sameSummary":same_summary,"curve":index,
            "peakStates":states,"transitions":transitions,
            "mean":summary.as_ref().ok().map(|s| [s.final_mean.lower, s.final_mean.upper]),
            "error":summary.err().map(|e| e.to_string())}));
    }
    Ok(serde_json::json!({"members":members,"snaps":snaps,"curves":curves.len(),"cache":cache.stats(),
        "summaryCache":summary_cache.stats(),"orders":rows}))
}

/// Benchmark the production summary for exactly the same supplied physical performance order.
/// This excludes diagnostic note/query rows while retaining every arithmetic and lifecycle guard.
pub fn luck_score_summary_replay(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
) -> Result<serde_json::Value, Error> {
    let (input, setup) = luck_input(built, members, snaps)?;
    let skills = ournotes_sim::live::full::luck_skills(built.pool.master)?;
    ournotes_sim::live::full::take_luck_score_profile();
    let started = std::time::Instant::now();
    let result = ournotes_sim::live::full::luck_score_summary_with_ranking(
        built.pool.master,
        &skills,
        &input.performers,
        &input.notes,
        &input.events,
        input.params,
        &setup,
        &input.play,
        &input.delta_times,
        input.rank_confirmations.as_deref(),
    );
    let ms = started.elapsed().as_secs_f64() * 1e3;
    let profile = ournotes_sim::live::full::take_luck_score_profile();
    Ok(match result {
        Ok(bounds) => serde_json::json!({"status":"bounded","members":members,"snaps":snaps,
            "mode":"productionSummary","ms":ms,"profile":profile,
            "width":bounds.final_mean.upper-bounds.final_mean.lower,"bounds":bounds}),
        Err(error) => serde_json::json!({"status":"refused","members":members,"snaps":snaps,
            "mode":"productionSummary","error":error.to_string(),"ms":ms}),
    })
}

fn replay_luck_certified_combos(
    combos: &[serde_json::Value],
    mut replay: impl FnMut(
        &[(ournotes_sim::live::full::LuckSkillKey, usize)],
    ) -> Result<ournotes_sim::live::full::LuckDpCertifiedResult, Error>,
) -> Vec<serde_json::Value> {
    combos
        .iter()
        .enumerate()
        .map(|(index, combo)| {
            let started = std::time::Instant::now();
            let result = serde_json::from_value(combo["entries"].clone())
                .map_err(|e| Error::Input(format!("LUCK certified DP replay combination {index}: {e}")))
                .and_then(|entries: Vec<_>| replay(&entries));
            let mut dp = match result {
                Ok(result) => luck_certified_output(result),
                Err(error) => {
                    let kind = match &error {
                        Error::Unsupported(_) => "unsupported",
                        Error::Input(_) => "invalidInput",
                        Error::Master(_) => "invalidMaster",
                        Error::Game(_) => "gameRejected",
                        Error::Domain(_) => "outsideDomain",
                        Error::Capacity(_) => "capacity",
                    };
                    serde_json::json!({"status":kind,"error":error.to_string()})
                }
            };
            dp["ms"] = serde_json::json!(started.elapsed().as_secs_f64() * 1e3);
            serde_json::json!({
                "archiveComboIndex":combo.get("archiveComboIndex").cloned().unwrap_or(index.into()),
                "entries":combo["entries"],"dp":dp,
            })
        })
        .collect()
}

/// Replay every archived combination through outward probability DP. Raw entry values and archive indices
/// are retained even for invalid or unsupported combinations. No Monte Carlo reference or rounded nominal
/// curve is used to construct these intervals, and this report does not certify whole-score expectations.
pub fn luck_combos_certified_replay(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    archived: &serde_json::Value,
) -> Result<serde_json::Value, Error> {
    use ournotes_sim::chartstats::{luck_neutral, luck_table_dp_certified};
    use ournotes_sim::live::full::luck_skills;
    let combos =
        archived["combos"].as_array().ok_or_else(|| Error::Input("LUCK certified DP replay needs combos".into()))?;
    let master = built.pool.master;
    let (input, g) = luck_input(built, members, snaps)?;
    let skills = luck_skills(master)?;
    let neutral = luck_neutral(master, &skills);
    let started = std::time::Instant::now();
    let out = replay_luck_certified_combos(combos, |entries| {
        luck_table_dp_certified(
            master,
            &skills,
            neutral,
            &input.notes,
            input.params,
            &g,
            &input.play,
            &input.delta_times,
            entries,
        )
    });
    Ok(serde_json::json!({
        "format":"ournotes-deck.luck-dp-certified/1",
        "model":{
            "scope":"lottery-state probabilities under independent nominal draws; not whole-score expectations or finite-seed means",
            "probabilities":"original integer lottery weights and native binary32 skill chances, propagated with outward binary64 arithmetic",
            "buckets":["00","01","10","11"],
            "bucketBits":["Rush","direct7021Score"],
            "probeBatches":"same supported direct 7021 predicate and identical directly accumulated joint curves; no averaging",
            "totalMass":"outward sum of all four bucket intervals without clipping or normalization",
        },
        "shapes":skills.shapes,"neutral":neutral,"combos":out,"ms":started.elapsed().as_secs_f64() * 1e3,
    }))
}

/// LUCK weight measurement of a fixed deck at each audited order: the live at constant LUCK weights all 0, rush only
/// and all 1 (`LiveModel::set_luck_weights`) beside the mean of `seeds` played lives that
/// draw the lottery, with independent random streams per order; with `curve_runs`, also the live weighted by the
/// order's own rush samples (`luck_rush_samples` over `curve_runs` lives, base seeds `seed_candidate(0..)`, apart from
/// the played ones); with `table_runs`, also the live weighted by the chart's LUCK table over that many lives
/// (`luck_compose` at `compose_power` of the base and the entries of the order's luck chain skills at their
/// positions). The effective
/// coefficient `(mean - s0) / (s1 - s0)` is the constant weight that reproduces the played mean. Also reports the
/// share of luck-range time the played rushes covered and the deck's performers.
#[allow(clippy::too_many_arguments)]
pub fn luck_coefficient_sample(
    built: &BuiltProblem<'_>,
    members: [i64; 5],
    snaps: [Option<i64>; 5],
    seeds: u32,
    curve_runs: u32,
    table_runs: u32,
    compose_power: f64,
    use_dp: bool,
) -> Result<serde_json::Value, Error> {
    use ournotes_sim::chartstats::{LuckSteps, luck_compose, luck_neutral, luck_table_steps};
    use ournotes_sim::live::{
        full::{LuckSkillKey, LuckSource, luck_rush_dp_with_ranking, luck_rush_samples, luck_skill_key, luck_skills},
        seeds::seed_candidate,
    };
    if seeds < 2 {
        return Err(Error::Input("LUCK score comparison needs at least two played samples".into()));
    }
    let master = built.pool.master;
    let (input, g) = luck_input(built, members, snaps)?;
    let skills = luck_skills(built.pool.master)?;
    let width = 1 + 2 * skills.shapes.len();
    let constant = |rush: f32, rest: f32| {
        let mut w = vec![rest; width];
        w[0] = rush;
        vec![(i32::MIN, w)]
    };
    let g = &g;
    let curve_seeds: Vec<i32> = (0..u64::from(curve_runs)).map(seed_candidate).collect();
    let table_seeds: Vec<i32> = (0..u64::from(table_runs)).map(seed_candidate).collect();
    let neutral = luck_neutral(master, &skills);
    let mut table: std::collections::HashMap<Option<(LuckSkillKey, usize)>, LuckSteps> = Default::default();
    let mut table_ms = 0f64;
    let mut out = Vec::new();
    for (oi, order) in audit_orders().into_iter().enumerate() {
        let performers = order.map(|slot| input.performers[slot].clone());
        let model = || input.model(master, &performers);
        let mut ends = [0f64; 3];
        let mut ranges = Vec::new();
        for (end, w) in ends.iter_mut().zip([constant(0.0, 0.0), constant(1.0, 0.0), constant(1.0, 1.0)]) {
            let mut m = model()?;
            m.set_luck_weights(&skills, w)?;
            *end = f64::from(m.run_with_random(&input.play, &input.delta_times, LiveRandom::new(0))?);
            ranges = m.rush_command_spans();
        }
        let (s_curve, curve_ms, curve_steps) = if curve_runs > 0 {
            let t = std::time::Instant::now();
            let w = luck_rush_samples(
                built.pool.master,
                &skills,
                &input.notes,
                input.params,
                g,
                &input.play,
                &input.delta_times,
                &performers,
                None,
                &curve_seeds,
            )?;
            let ms = t.elapsed().as_secs_f64() * 1e3;
            let steps = w.len();
            let mut m = model()?;
            m.set_luck_weights(&skills, w)?;
            let score = f64::from(m.run_with_random(&input.play, &input.delta_times, LiveRandom::new(0))?);
            (Some(score), Some(ms), Some(steps))
        } else {
            (None, None, None)
        };
        let (s_dp, dp_ms, dp_peak_states, dp_transitions, dp_error) = if use_dp {
            let t = std::time::Instant::now();
            let result = luck_rush_dp_with_ranking(
                master,
                &skills,
                &input.notes,
                &input.events,
                input.params,
                g,
                &input.play,
                &input.delta_times,
                &performers,
                None,
                input.rank_confirmations.as_deref(),
            );
            let ms = t.elapsed().as_secs_f64() * 1e3;
            match result {
                Ok(dp) => {
                    let mut m = model()?;
                    m.set_luck_weights(&skills, dp.steps)?;
                    let score = f64::from(m.run_with_random(&input.play, &input.delta_times, LiveRandom::new(0))?);
                    (Some(score), Some(ms), Some(dp.peak_states), Some(dp.transitions), None)
                }
                Err(e) => (None, Some(ms), None, None, Some(e.to_string())),
            }
        } else {
            (None, None, None, None, None)
        };
        let (s_table, table_keys) = if table_runs > 0 {
            let mut keys = Vec::new();
            for (k, p) in performers.iter().enumerate() {
                let held = p.gekisou_skill.map(|s| (LuckSource::Gekisou, s)).into_iter().chain(
                    p.gekisou_skill
                        .iter()
                        .flat_map(|_| &p.gekisou_support_skills)
                        .map(|&s| (LuckSource::GekisouSupport, s)),
                );
                for (source, (id, level)) in held {
                    let key = luck_skill_key(master, source, id, level, p)?;
                    if skills.chain.contains(&key) {
                        keys.push((key, k));
                    }
                }
            }
            for entry in std::iter::once(None).chain(keys.iter().copied().map(Some)) {
                if let std::collections::hash_map::Entry::Vacant(slot) = table.entry(entry) {
                    let t = std::time::Instant::now();
                    let steps = luck_table_steps(
                        master,
                        &skills,
                        neutral,
                        &input.notes,
                        input.params,
                        g,
                        &input.play,
                        &input.delta_times,
                        entry.as_slice(),
                        &table_seeds,
                    )?;
                    table_ms += t.elapsed().as_secs_f64() * 1e3;
                    slot.insert(steps);
                }
            }
            let entries: Vec<&LuckSteps> = keys.iter().map(|&e| &table[&Some(e)]).collect();
            let w = luck_compose(&table[&None], &entries, compose_power);
            let mut m = model()?;
            m.set_luck_weights(&skills, w)?;
            let score = f64::from(m.run_with_random(&input.play, &input.delta_times, LiveRandom::new(0))?);
            (Some(score), Some(keys.len()))
        } else {
            (None, None)
        };
        let luck_ms: i64 = ranges.iter().map(|&(a, b)| i64::from(b) - i64::from(a)).sum();
        let (mut sum, mut sum_sq, mut covered) = (0f64, 0f64, 0f64);
        for k in 0..seeds {
            let mut live = model()?;
            let score = f64::from(live.run_with_random(&input.play, &input.delta_times, luck_seed(0, seeds, oi, k))?);
            sum += score;
            sum_sq += score * score;
            for (a, b) in live.rush_command_spans() {
                let b = if b == i32::MAX { ranges.iter().map(|r| r.1).max().unwrap_or(a) } else { b };
                covered += ranges.iter().map(|&(s, e)| (b.min(e) - a.max(s)).max(0) as f64).sum::<f64>();
            }
        }
        let n = f64::from(seeds.max(1));
        let mean = sum / n;
        let se = ((sum_sq / n - mean * mean).max(0.0) / (n - 1.0).max(1.0)).sqrt();
        let span = ends[2] - ends[0];
        out.push(serde_json::json!({"order":order,"s0":ends[0],"sBonus":ends[1],"s1":ends[2],"sCurve":s_curve,
            "curveMs":curve_ms,"curveSteps":curve_steps,"sTable":s_table,"tableKeys":table_keys,"played":seeds,"mean":mean,"se":se,
            "sDp":s_dp,"dpMs":dp_ms,"dpPeakStates":dp_peak_states,"dpTransitions":dp_transitions,"dpError":dp_error,
            "c":if span > 0.0 { (mean - ends[0]) / span } else { f64::NAN },
            "cSe":if span > 0.0 { se / span } else { f64::NAN },
            "luckMs":luck_ms,"rushTimeShare":if luck_ms > 0 { covered / n / luck_ms as f64 } else { 0.0 }}));
    }
    let performers: Vec<_> = input
        .performers
        .iter()
        .map(|p| {
            serde_json::json!({"gekisouSkill":p.gekisou_skill,"gekisouSupportSkills":p.gekisou_support_skills,
                "supportSkills":p.support_skills,"liveSkill":p.live_skill})
        })
        .collect();
    Ok(serde_json::json!({"members":members,"snaps":snaps,"performers":performers,"orders":out,
        "tableEntries":table.len(),"tableMs":table_ms}))
}

#[cfg(test)]
mod luck_certified_tests {
    use super::*;
    use ournotes_sim::live::{certified::ProbabilityMass, full::LuckDpCertifiedResult};

    fn result() -> LuckDpCertifiedResult {
        let tenth = ProbabilityMass::from_ratio(1, 10).unwrap();
        LuckDpCertifiedResult {
            probe_transitions: Vec::new(),
            steps: vec![(123, [tenth, tenth, tenth, ProbabilityMass::from_ratio(7, 10).unwrap()])],
            probes: vec![true],
            range_moments: Vec::new(),
            peak_states: 17,
            transitions: 31,
        }
    }

    #[test]
    fn certified_archive_replay_keeps_failed_entries_and_original_indices() {
        let entries = serde_json::json!([[{"source":"gekisou","id":10,"level":1,"matched":null},0]]);
        let combos = vec![
            serde_json::json!({"archiveComboIndex":27,"entries":entries}),
            serde_json::json!({"archiveComboIndex":91,"entries":[]}),
            serde_json::json!({"entries":"malformed"}),
        ];
        let mut calls = 0;
        let replayed = replay_luck_certified_combos(&combos, |entries| {
            calls += 1;
            if entries.is_empty() { Ok(result()) } else { Err(Error::Unsupported("test shape".into())) }
        });
        assert_eq!(calls, 2);
        assert_eq!(replayed.len(), combos.len());
        for (before, after) in combos.iter().zip(&replayed) {
            assert_eq!(before["entries"], after["entries"]);
        }
        assert_eq!(replayed[0]["archiveComboIndex"], 27);
        assert_eq!(replayed[0]["dp"]["status"], "unsupported");
        assert_eq!(replayed[1]["archiveComboIndex"], 91);
        assert_eq!(replayed[1]["dp"]["status"], "certified");
        assert_eq!(replayed[2]["archiveComboIndex"], 2);
        assert_eq!(replayed[2]["dp"]["status"], "invalidInput");
    }

    #[test]
    fn certified_report_keeps_binary64_bounds_and_unclipped_total_mass() {
        let source = result();
        let expected = source.steps[0].1;
        let report = luck_certified_output(source);
        let point = &report["steps"][0];
        for (bucket, mass) in expected.into_iter().enumerate() {
            let interval = mass.interval();
            assert_eq!(point["buckets"][bucket]["lower"].as_f64(), Some(interval.lower()));
            assert_eq!(point["buckets"][bucket]["upper"].as_f64(), Some(interval.upper()));
        }
        assert_eq!(point["timeMs"], 123);
        assert!(point["totalMass"]["lower"].as_f64().unwrap() < 1.0);
        assert!(point["totalMass"]["upper"].as_f64().unwrap() > 1.0);
        assert!(report["maxBucketWidth"].as_f64().unwrap() > 0.0);
        assert!(report["maxBucketWidth"].as_f64().unwrap() < 1e-14);
        assert_eq!(report["peakStates"], 17);
        assert_eq!(report["transitions"], 31);
    }
}
