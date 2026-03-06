//! Stratum 6: HITL Controller — gate management, decision recording,
//! durable pause/resume, policy engine, and notification adapters.

pub mod controller;
pub mod notifier;
pub mod policy;

pub use controller::SqliteHitlController;
pub use notifier::{StdoutNotifier, WebhookNotifier};
pub use policy::{GateAction, GatePolicyEngine};
