//! AppContext: concrete-typed wiring, no generics gymnastics.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use stratum_engine::dispatch::RfbmqDispatcher;
use stratum_engine::gateway::DefaultToolGateway;
use stratum_engine::llm::AnthropicClient;
use stratum_engine::memory::TwoTierMemoryStore;
use stratum_engine::orchestrator::{DefaultOrchestrator, OrchestratorConfig};
use stratum_engine::registry::PersistentToolRegistry;
use stratum_engine::session::SqliteSessionManager;
use stratum_engine::subprocess::{CompositeExecutor, SubprocessExecutor, SubprocessExecutorConfig};
use stratum_engine::tools::BuiltinExecutor;
use stratum_engine::trajectory::SqliteTrajectoryStore;

use crate::config::StratumConfig;

/// Full application context with concrete types.
pub struct AppContext {
    pub config: StratumConfig,
    pub session: Arc<SqliteSessionManager>,
    #[allow(dead_code)]
    pub trajectory: Arc<SqliteTrajectoryStore>,
    pub llm: Arc<AnthropicClient>,
    pub memory: Arc<TwoTierMemoryStore>,
    pub tool_registry: Arc<PersistentToolRegistry>,
    pub tool_gateway: Arc<DefaultToolGateway<CompositeExecutor>>,
    pub dispatch: Arc<RfbmqDispatcher>,
    #[allow(dead_code)]
    pub orchestrator: Arc<DefaultOrchestrator<SqliteSessionManager>>,
}

impl AppContext {
    pub fn build(config: StratumConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        let db_path = config.db_path();

        // Core stores
        let trajectory = Arc::new(SqliteTrajectoryStore::new(&db_path)?);
        let session = Arc::new(SqliteSessionManager::new(&db_path)?);

        // LLM client
        let llm = Arc::new(AnthropicClient::new(config.api_key.clone()));

        // Memory
        let memory_db_path = config.data_dir.join("memory.db");
        let memory = Arc::new(TwoTierMemoryStore::new(&memory_db_path.to_string_lossy())?);

        // Tool registry
        let registry_db_path = config.data_dir.join("tools.db");
        let registry_conn = Arc::new(Mutex::new(rusqlite::Connection::open(&registry_db_path)?));
        let tool_registry = Arc::new(PersistentToolRegistry::new(registry_conn)?);
        tool_registry.register_builtins(stratum_engine::tools::all_builtin_definitions())?;

        // Composite executor
        let executor = Arc::new(CompositeExecutor::new(
            BuiltinExecutor,
            SubprocessExecutor::new(SubprocessExecutorConfig::default(), HashMap::new()),
        ));

        // Tool gateway
        let tool_gateway = Arc::new(DefaultToolGateway::new(
            Default::default(),
            Arc::clone(&tool_registry),
            executor,
        ));

        // Dispatch
        let queue_root = config.queue_root();
        std::fs::create_dir_all(&queue_root)?;
        let dispatch = Arc::new(RfbmqDispatcher::init_or_open(&queue_root, 10000)?);

        // Orchestrator
        let orchestrator = Arc::new(DefaultOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::clone(&session),
        ));

        Ok(Self {
            config,
            session,
            trajectory,
            llm,
            memory,
            tool_registry,
            tool_gateway,
            dispatch,
            orchestrator,
        })
    }
}
