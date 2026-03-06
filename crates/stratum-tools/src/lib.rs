//! stratum-tools: Tool Execution Gateway implementation (Stratum 4).
//!
//! Owns the intercept-validate-execute-log pipeline, trust levels,
//! tool manifest, and constraint enforcement.

pub use stratum_core::{ConstraintEnforcer, FrozenToolRegistry, ToolGateway, ToolRegistryBuilder};
