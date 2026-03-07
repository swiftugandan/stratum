use std::collections::HashMap;
use std::sync::Arc;

use stratum_adapters::{
    InMemoryMetrics, SqliteHitlController, SqliteSessionManager, SqliteTrajectoryStore,
    StdoutNotifier,
};
use stratum_context::DefaultContextEngine;
use stratum_core::ToolRegistryBuilder;
use stratum_memory::DefaultMemoryStore;
use stratum_orchestrator::{DefaultOrchestrator, RfbmqDispatcher};
use stratum_tools::{
    BuiltinExecutor, CompositeExecutor, DefaultToolGateway, PersistentToolRegistry,
    SubprocessExecutor,
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
    pub tool_registry: Arc<PersistentToolRegistry>,
    pub tool_gateway: Arc<DefaultToolGateway<SqliteTrajectoryStore, CompositeExecutor>>,
    #[allow(dead_code)]
    pub hitl: Arc<SqliteHitlController>,
    pub config: StratumConfig,
    #[allow(dead_code)]
    pub memory: Arc<DefaultMemoryStore<SqliteTrajectoryStore>>,
    #[allow(dead_code)]
    orchestrator: Arc<DefaultOrchestrator<SqliteSessionManager, SqliteTrajectoryStore>>,
    #[allow(dead_code)]
    metrics: Arc<InMemoryMetrics>,
}

impl AppContext {
    /// Build the full application context from config.
    pub fn build(config: StratumConfig) -> anyhow::Result<Self> {
        Self::build_inner(config, false)
    }

    /// Build the application context for daemon mode (auto-approves global memory promotions).
    pub fn build_daemon(config: StratumConfig) -> anyhow::Result<Self> {
        Self::build_inner(config, true)
    }

    fn build_inner(config: StratumConfig, auto_approve_global: bool) -> anyhow::Result<Self> {
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
        let mut mem_config = config.memory_store_config();
        mem_config.auto_approve_global = auto_approve_global;
        let memory = Arc::new(DefaultMemoryStore::new(
            mem_config,
            Arc::clone(&core.trajectory),
        )?);

        // Persistent tool registry with built-in tools
        let registry_db_path = config.data_dir.join("tools.db");
        let registry_conn = Arc::new(std::sync::Mutex::new(rusqlite::Connection::open(
            &registry_db_path,
        )?));
        let tool_registry = Arc::new(PersistentToolRegistry::new(registry_conn)?);

        // Register built-in tool definitions
        tool_registry.register_builtins(stratum_tools::builtin::all_builtin_definitions())?;

        // Composite executor (builtin + subprocess)
        let executor = Arc::new(CompositeExecutor::new(
            BuiltinExecutor,
            SubprocessExecutor::new(config.subprocess_executor_config(), HashMap::new()),
        ));

        // Tool gateway (uses PersistentToolRegistry via the trait)
        // Note: The gateway still needs an InMemoryFrozenToolRegistry for trait compatibility.
        // We build one from the current persistent registry state.
        let frozen_registry = build_frozen_from_persistent(&tool_registry)?;
        let tool_gateway = Arc::new(DefaultToolGateway::new(
            config.tool_gateway_config(),
            Arc::new(frozen_registry),
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

/// Build an InMemoryFrozenToolRegistry from a PersistentToolRegistry's current state.
fn build_frozen_from_persistent(
    registry: &PersistentToolRegistry,
) -> anyhow::Result<stratum_tools::InMemoryFrozenToolRegistry> {
    let mut builder = stratum_tools::InMemoryToolRegistryBuilder::default();
    for def in registry.get_manifest_owned() {
        builder.register(def)?;
    }
    Ok(builder.build()?)
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
