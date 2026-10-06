//! Live-score objective with snap skills: the whole-live simulation (Gekisou off) scores every deck, and the leaf
//! searches snap placements and performance orders together.
//!
//! The argument is in `docs/search.md` ("Live score with snap skills"); the names below follow it. In short:
//! - every snap is classified per member card: its support skills either cannot change the simulation for that
//!   member under this play and pool (the empty class), or they fall in a class of snaps whose effect rows are
//!   interchangeable in the simulation; within one class assignment the score depends on the deck power alone and is
//!   non-decreasing in it, so the best snaps of a class assignment come from a constrained max-weight matching;
//! - an upper bound of the score is linear in the power: `P * (A0 + sum over positions of G)`, where `A0` is the
//!   no-skill value per unit of power and `G` the largest value per unit of power the performer at one position can
//!   add (its live skill, extended by its snap, and its snap's own score effects), each with a margin `eps` for the
//!   float arithmetic of the simulation (rounding and the drift of re-executed frames);
//! - candidates (order, class assignment) are enumerated below the bound and simulated in order of a per-note bound
//!   that reads the candidate's own conversions and, when it guards nowhere and recovers life only at its skill
//!   events, the notes whose filed damage empties the life; a candidate is dropped only when a bound is strictly
//!   below the best exact score or the Top-K threshold.

use crate::clock::Instant;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::search::matching::constrained_assignment;
use crate::search::power::PowerStats;
use crate::search::tables::Tables;
use crate::search::topk::SnapKey;
use ournotes_sim::cards::{MemberView, SnapView};
use ournotes_sim::error::Error;
use ournotes_sim::live::full::{GekisouSetup, LiveModel, LiveNote, LiveParams, LivePlay, Performer};
use ournotes_sim::live::model::JudgementStream;
use ournotes_sim::live::score::{
    COMBO, ComboTable, GEKISOU_COMBO, LiveScoreSettings, convert_score_type, get_frame, get_music_score_level_factor,
};
use ournotes_sim::live::skill::{judgement_factor_mill, note_factor_mill};
use ournotes_sim::live::skip::Chart;
use ournotes_sim::master::{GekisouSkillEffectRow, Master, SkillTargetRow};
use ournotes_sim::pool::{Deck, Pool};

/// Relative error bound of the per-note float chain (as for the per-order model).
const CHAIN_EPS: f64 = 2e-6;
/// Concurrent executions of one snap effect.
const POOL: f64 = 5.0;
/// Pending candidates of one leaf before the best of them is simulated to raise the cutoff.
const FLUSH: usize = 64;

/// Validation switches of the live objective with Gekisou on. The first seven bits each replace one part of the
/// bound or of the seed loop by an inadmissible variant, so that the search must disagree with exhaustive
/// enumeration or report bound violations; `NO_EARLY_STOP` and `NO_PREFIX` switch off an exact shortcut, and
/// `NO_WARM_START` and `STATIC_ORDER` the joint search's incumbents and visit order, and `NO_DECK_PAYOFF` the deck
/// payoff ranking before it.
pub mod ablate {
    /// No rank bonus factor.
    pub const RANK_BONUS: u32 = 1;
    /// No luck factor.
    pub const LUCK: u32 = 2;
    /// No Gekisou combo factor.
    pub const GEKISOU_COMBO: u32 = 4;
    /// The early stop also drops a candidate whose remaining bound equals the cutoff.
    pub const EARLY_STOP_EQUAL: u32 = 8;
    /// The per-seed bound replaced by the largest seed score simulated so far in the leaf.
    pub const OBSERVED_MAX: u32 = 16;
    /// Gekisou support rows left out of the snap class key.
    pub const CLASS_KEY: u32 = 32;
    /// The shared prefix of the seed runs cloned one frame after its first possible draw.
    pub const PREFIX_LATE: u32 = 64;
    /// Every candidate simulated on every seed.
    pub const NO_EARLY_STOP: u32 = 128;
    /// Every seed run from the first frame.
    pub const NO_PREFIX: u32 = 256;
    /// No warm-start incumbents before the joint search (`search/warm.rs`).
    pub const NO_WARM_START: u32 = 512;
    /// Static visit order of the root children and of the Gekisou conversion parts.
    pub const STATIC_ORDER: u32 = 1024;
    /// The joint search alone, without the deck payoff ranking that may settle the Top-K before it.
    pub const NO_DECK_PAYOFF: u32 = 2048;
}

thread_local! {
    static ABLATION: Cell<u32> = const { Cell::new(0) };
}

/// Sets the validation switches ([`ablate`]) of the searches run on the calling thread; 0 (the default) runs the
/// exact search with every shortcut. For validation only.
#[doc(hidden)]
pub fn set_bound_ablation(bits: u32) {
    ABLATION.with(|a| a.set(bits));
}

pub(crate) fn ablated(bit: u32) -> bool {
    ABLATION.with(|a| a.get() & bit != 0)
}

thread_local! {
    static CENSUS: Cell<Option<i128>> = const { Cell::new(None) };
}

/// Diagnostics: the joint searches run on the calling thread prune against the fixed payoff numerator `threshold`
/// instead of their K-th, and count the played-live teams whose per-order caps reach it (`leaves.census`) instead
/// of simulating them. None (the default) runs the search.
#[cfg(feature = "search-diagnostics")]
#[doc(hidden)]
pub fn set_census(threshold: Option<i128>) {
    CENSUS.with(|c| c.set(threshold));
}

pub(crate) fn census() -> Option<i128> {
    CENSUS.with(Cell::get)
}

#[path = "snaps/ramp.rs"]
mod ramp;
#[path = "snaps/raw.rs"]
mod raw;

mod combo_triggers;
mod conversion;
mod envelope_data;
mod fine_view;
mod float_margin;
mod gk_schedule;
mod gk_windows;
mod leaf_search;
mod rows;
mod rush;
mod score_windows;
mod setup;
mod snap_live;
mod snap_live_build;
use envelope_data::*;
use fine_view::*;
pub(crate) use fine_view::{JointFineBounds, JointScratch};
use gk_schedule::*;
pub(crate) use gk_schedule::{CarrierKeys, KeyedEnvelope};
use gk_windows::*;
use leaf_search::*;
use rows::*;
pub(crate) use rush::RushMasks;
use score_windows::*;
use setup::*;
pub(crate) use setup::{FullSetup, deck_performers};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use setup::performer;
pub(crate) use snap_live::SnapLive;
use snap_live::*;

type GkWindowKey = (i64, i64, i64, u32, i64);
type SimulationMemberKey = (i64, i64, i64, i64, i64, i64, i64, usize);
type SeededDeckKey = ([(u32, u32); 5], i64);
/// A Gekisou combo bonus window `(start, end, bonus, gate)` in chart time (closed). A gated window counts only once
/// the combo of its range can reach the threshold: `gate = (threshold, range)` (see `gate_open`).
type ComboBonusRow = (i64, i64, f64, Option<(i64, u32)>);
/// A component of a sustained row's factor spans `(start, end, playing range of every start frame in it)`.
type SpanPart = (i64, i64, Option<usize>);

/// The fields of a row that its conversion budget reads.
type BudgetKey = (GkWindowKey, (i64, i64, i64, i64, i64), Vec<i64>);
/// Gekisou missions.
const MISSION_COMBO: i64 = 1;
const MISSION_LUCK: i64 = 2;
const MISSION_ALL: i64 = 4;
/// Gekisou range states (as `live::full` reports them).
const RS_STANDBY: u8 = 2;
const RS_START: u8 = 3;
const RS_PLAYING: u8 = 4;
const RS_END: u8 = 5;
const RS_COMPLETE: u8 = 7;
const RS_FINISH: u8 = 8;
