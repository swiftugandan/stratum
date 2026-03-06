use std::collections::HashMap;
use std::sync::Arc;

use stratum_core::ToolRegistryBuilder;

use stratum_adapters::{
    InMemoryMetrics, SqliteHitlController, SqliteSessionManager, SqliteTrajectoryStore,
    StdoutNotifier,
};
use stratum_context::DefaultContextEngine;
use stratum_memory::DefaultMemoryStore;
use stratum_orchestrator::{DefaultOrchestrator, RfbmqDispatcher};
use stratum_tools::{
    DefaultToolGateway, InMemoryFrozenToolRegistry, InMemoryToolRegistryBuilder, SubprocessExecutor,
};

use crate::config::StratumConfig;
use crate::llm_adapter::LlmClientAdapter;

/// Shared core services used by both `AppContext` and `QueryContext`.
struct CoreServices {
    trajectory: Arc<SqliteTrajectoryStore>,
    session: Arc<SqliteSessionManager>,
    hitl: Arc<SqliteHitlController>,
}

impl CoreServices {
    fn build(config: &StratumConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        let db_path = config.db_path();

        let trajectory = Arc::new(SqliteTrajectoryStore::new(&db_path)?);
        let session = Arc::new(SqliteSessionManager::new(
            &db_path,
            Arc::clone(&trajectory) as _,
        )?);
        let notifier = Arc::new(StdoutNotifier);
        let hitl = Arc::new(SqliteHitlController::new(
            &db_path,
            Arc::clone(&trajectory) as _,
            Arc::clone(&session) as _,
            notifier as _,
        )?);

        Ok(Self {
            trajectory,
            session,
            hitl,
        })
    }
}

/// Full wiring for run/resume commands requiring LLM access.
pub struct AppContext {
    pub trajectory: Arc<SqliteTrajectoryStore>,
    pub session: Arc<SqliteSessionManager>,
    pub llm: Arc<LlmClientAdapter>,
    pub context_engine:
        Arc<DefaultContextEngine<SqliteSessionManager, SqliteTrajectoryStore, LlmClientAdapter>>,
    pub tool_registry: Arc<InMemoryFrozenToolRegistry>,
    pub tool_gateway: Arc<DefaultToolGateway<SqliteTrajectoryStore, SubprocessExecutor>>,
    #[allow(dead_code)]
    pub hitl: Arc<SqliteHitlController>,
    pub config: StratumConfig,
    // Constructed but accessed only by future phases:
    #[allow(dead_code)]
    memory: Arc<DefaultMemoryStore<SqliteTrajectoryStore>>,
    #[allow(dead_code)]
    orchestrator: Arc<DefaultOrchestrator<SqliteSessionManager, SqliteTrajectoryStore>>,
    #[allow(dead_code)]
    metrics: Arc<InMemoryMetrics>,
}

impl AppContext {
    /// Build the full application context from config.
    pub fn build(config: StratumConfig) -> anyhow::Result<Self> {
        let core = CoreServices::build(&config)?;

        // LLM client
        let llm = Arc::new(LlmClientAdapter::from_config(&config));

        // Context engine
        let context_engine = Arc::new(DefaultContextEngine::new(
            config.context_engine_config(),
            Arc::clone(&core.session),
            Arc::clone(&core.trajectory),
            Arc::clone(&llm),
        ));

        // Memory store
        let memory = Arc::new(DefaultMemoryStore::new(
            config.memory_store_config(),
            Arc::clone(&core.trajectory),
        )?);

        // Tool registry (empty by default; tools populated per-run config)
        let builder = InMemoryToolRegistryBuilder::default();
        let tool_registry = Arc::new(builder.build()?);

        // Subprocess executor
        let executor = Arc::new(SubprocessExecutor::new(
            config.subprocess_executor_config(),
            HashMap::new(),
        ));

        // Tool gateway
        let tool_gateway = Arc::new(DefaultToolGateway::new(
            config.tool_gateway_config(),
            Arc::clone(&tool_registry),
            Arc::clone(&core.trajectory),
            executor,
            config.trust_level,
            None,
        ));

        // Orchestrator
        let orchestrator = Arc::new(DefaultOrchestrator::new(
            config.orchestrator_config(),
            Arc::clone(&core.session),
            Arc::clone(&core.trajectory),
        ));

        // Metrics
        let metrics = Arc::new(InMemoryMetrics::new());

        Ok(Self {
            trajectory: core.trajectory,
            session: core.session,
            llm,
            context_engine,
            memory,
            tool_registry,
            tool_gateway,
            orchestrator,
            hitl: core.hitl,
            metrics,
            config,
        })
    }
}

/// Lightweight context for read-only commands (no LLM client needed).
pub struct QueryContext {
    pub trajectory: Arc<SqliteTrajectoryStore>,
    pub session: Arc<SqliteSessionManager>,
    pub hitl: Arc<SqliteHitlController>,
    pub dispatch: RfbmqDispatcher,
}

impl QueryContext {
    /// Build a query context from config (no API key required).
    pub fn build(config: &StratumConfig) -> anyhow::Result<Self> {
        let core = CoreServices::build(config)?;

        let orch_config = config.orchestrator_config();
        let dispatch = RfbmqDispatcher::init_or_open(
            &orch_config.queue_root,
            orch_config.max_pending_per_queue,
        )?;

        Ok(Self {
            trajectory: core.trajectory,
            session: core.session,
            hitl: core.hitl,
            dispatch,
        })
    }
}
