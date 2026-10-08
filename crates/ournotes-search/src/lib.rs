//! The exact Top-K deck search of BanG Dream! Our Notes over the `ournotes-sim` model.
//!
//! Every game formula lives in `ournotes-sim`; this crate decides which decks to evaluate and proves the Top-K.
//!
//! Layout:
//! - [`engine`]: typed/JSON recommendation facade;
//! - [`handler`]: build a frozen pool, legal domain and search context;
//! - [`types`]: explicit objective, request and result contracts;
//! - [`domain`]: immutable legal candidate indexes, shared by search and fixed evaluation;
//! - [`auxiliary`]: fixed-deck evaluation and fixed-deck song ranking;
//! - [`search`]: the exact Top-K deck search and its brute-force oracle;
//! - [`skip_event`]: the exhaustive skip event-point oracle;
//! - [`owned_snapshot`]: strict, goal-scoped owned facts for the recommendation.

pub mod account;
pub mod auxiliary;
mod clock;
pub mod domain;
pub mod engine;
pub mod handler;
#[cfg(not(target_arch = "wasm32"))]
mod native_jobs;
pub mod owned_snapshot;
#[cfg(not(target_arch = "wasm32"))]
pub mod parallel;
pub mod recommendation;
pub mod search;
pub mod skip_event;
pub mod types;
