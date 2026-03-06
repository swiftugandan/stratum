//! stratum-tools: Tool Execution Gateway implementation (Stratum 4).
//!
//! Owns the intercept-validate-execute-log pipeline, trust levels,
//! tool manifest, and constraint enforcement.

pub mod config;
pub mod constraint;
pub mod error;
pub mod executor;
pub mod gateway;
pub mod registry;
mod retry;
pub mod subprocess;

// Re-export port traits from stratum-core.
pub use stratum_core::{ConstraintEnforcer, FrozenToolRegistry, ToolGateway, ToolRegistryBuilder};

// Re-export concrete types.
pub use config::ToolGatewayConfig;
pub use constraint::DefaultConstraintEnforcer;
pub use error::ToolError;
pub use executor::{NoOpExecutor, ToolExecutionError, ToolExecutor};
pub use gateway::DefaultToolGateway;
pub use registry::{InMemoryFrozenToolRegistry, InMemoryToolRegistryBuilder};
pub use subprocess::{ParamPassing, SubprocessExecutor, SubprocessExecutorConfig, ToolCommandSpec};
