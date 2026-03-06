//! stratum-core: Port trait definitions for all seven strata.
//!
//! This crate is the hexagonal architecture's "ports" layer. It contains
//! only trait definitions and the TurnExecutor application service.
//! All concrete implementations live in their respective crates
//! (stratum-adapters, stratum-context, stratum-memory, etc.).

pub mod ports;
pub mod turn;

pub use ports::*;
pub use turn::TurnExecutor;
