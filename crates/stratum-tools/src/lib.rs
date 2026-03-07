//! stratum-tools: Tool Execution Gateway implementation (Stratum 4).
//!
//! Owns the intercept-validate-execute-log pipeline, trust levels,
//! tool manifest, and constraint enforcement.

pub mod builtin;
pub mod composite;
pub mod config;
pub mod constraint;
pub mod error;
pub mod executor;
pub mod gateway;
pub mod persistent_registry;
pub mod registry;
mod retry;
pub mod subprocess;

// Re-export port traits from stratum-core.
pub use stratum_core::{ConstraintEnforcer, FrozenToolRegistry, ToolGateway, ToolRegistryBuilder};

// Re-export concrete types.
pub use builtin::schemas::builtin_tool_definitions;
pub use builtin::BuiltinExecutor;
pub use composite::CompositeExecutor;
pub use config::ToolGatewayConfig;
pub use constraint::DefaultConstraintEnforcer;
pub use error::ToolError;
pub use executor::{NoOpExecutor, ToolExecutionError, ToolExecutor};
pub use gateway::DefaultToolGateway;
pub use persistent_registry::PersistentToolRegistry;
pub use registry::{InMemoryFrozenToolRegistry, InMemoryToolRegistryBuilder};
pub use subprocess::{ParamPassing, SubprocessExecutor, SubprocessExecutorConfig, ToolCommandSpec};
