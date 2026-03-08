//! AppContext: concrete-typed wiring, no generics gymnastics.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use stratum_engine::dispatch::RfbmqDispatcher;
use stratum_engine::error::EngineError;
use stratum_engine::gateway::DefaultToolGateway;
use stratum_engine::llm::AnthropicClient;
use stratum_engine::memory::TwoTierMemoryStore;
use stratum_engine::openai::OpenAiClient;
use stratum_engine::orchestrator::{DefaultOrchestrator, OrchestratorConfig};
use stratum_engine::registry::PersistentToolRegistry;
use stratum_engine::session::SqliteSessionManager;
use stratum_engine::subprocess::{CompositeExecutor, SubprocessExecutor, SubprocessExecutorConfig};
use stratum_engine::tools::BuiltinExecutor;
use stratum_engine::trajectory::SqliteTrajectoryStore;

use crate::config::{LlmProvider, StratumConfig};

/// Type-erased LLM client wrapping either Anthropic or OpenAI-compatible.
pub enum LlmBox {
    Anthropic(AnthropicClient),
    OpenAi(OpenAiClient),
}

#[async_trait::async_trait]
impl stratum_core::ports::LlmClient for LlmBox {
    type Error = EngineError;

    async fn complete(
        &self,
        messages: &[stratum_core::LlmMessage],
        model: &str,
        tools: Option<&[stratum_core::ToolDefinition]>,
    ) -> Result<stratum_core::LlmResponse, Self::Error> {
        match self {
            LlmBox::Anthropic(c) => c.complete(messages, model, tools).await,
            LlmBox::OpenAi(c) => c.complete(messages, model, tools).await,
        }
    }
}

/// Full application context with concrete types.
pub struct AppContext {
    pub config: StratumConfig,
    pub session: Arc<SqliteSessionManager>,
    #[allow(dead_code)]
    pub trajectory: Arc<SqliteTrajectoryStore>,
    pub llm: Arc<LlmBox>,
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
        let llm: Arc<LlmBox> = match config.provider {
            LlmProvider::Anthropic => {
                let mut client = AnthropicClient::new(config.api_key.clone());
                if let Some(ref url) = config.base_url {
                    client = client.with_base_url(url.clone());
                }
                Arc::new(LlmBox::Anthropic(client.with_max_tokens(config.max_tokens)))
            }
            LlmProvider::Groq => {
                let client = if let Some(ref url) = config.base_url {
                    OpenAiClient::new(config.api_key.clone(), url.clone())
                } else {
                    OpenAiClient::groq(config.api_key.clone())
                };
                Arc::new(LlmBox::OpenAi(client.with_max_tokens(config.max_tokens)))
            }
            LlmProvider::Openai => {
                let base_url = config.base_url.clone().unwrap_or_else(|| {
                    "https://api.openai.com/v1".to_string()
                });
                let client = OpenAiClient::new(config.api_key.clone(), base_url);
                Arc::new(LlmBox::OpenAi(client.with_max_tokens(config.max_tokens)))
            }
        };

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
