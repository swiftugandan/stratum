//! Mock implementations of all port traits for testing.

use async_trait::async_trait;
use std::sync::Mutex;
use stratum_core::*;
use stratum_types::*;

// ---------------------------------------------------------------------------
// Shared error type for mocks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MockError(pub String);

impl std::fmt::Display for MockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mock error: {}", self.0)
    }
}

impl std::error::Error for MockError {}

// ---------------------------------------------------------------------------
// Mock SessionManager
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockSessionManager {
    pub runs: Mutex<Vec<StratumRun>>,
    pub checkpoints: Mutex<Vec<Checkpoint>>,
}

#[async_trait]
impl SessionManager for MockSessionManager {
    type Error = MockError;

    async fn create_run(&self, config: StratumRun) -> Result<StratumRun, Self::Error> {
        self.runs.lock().unwrap().push(config.clone());
        Ok(config)
    }

    async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), Self::Error> {
        self.checkpoints.lock().unwrap().push(checkpoint.clone());
        Ok(())
    }

    async fn resume(&self, run_id: RunId) -> Result<StratumRun, Self::Error> {
        let runs = self.runs.lock().unwrap();
        runs.iter()
            .find(|r| r.id == run_id)
            .cloned()
            .ok_or_else(|| MockError(format!("run {run_id} not found")))
    }

    async fn transition_state(
        &self,
        run_id: RunId,
        new_state: RunState,
    ) -> Result<(), Self::Error> {
        let mut runs = self.runs.lock().unwrap();
        if let Some(run) = runs.iter_mut().find(|r| r.id == run_id) {
            run.state = new_state;
            Ok(())
        } else {
            Err(MockError(format!("run {run_id} not found")))
        }
    }

    async fn get_run(&self, run_id: RunId) -> Result<Option<StratumRun>, Self::Error> {
        let runs = self.runs.lock().unwrap();
        Ok(runs.iter().find(|r| r.id == run_id).cloned())
    }

    async fn get_latest_checkpoint(
        &self,
        run_id: RunId,
    ) -> Result<Option<Checkpoint>, Self::Error> {
        let checkpoints = self.checkpoints.lock().unwrap();
        Ok(checkpoints
            .iter()
            .filter(|c| c.run_id == run_id)
            .next_back()
            .cloned())
    }
}

// ---------------------------------------------------------------------------
// Mock ContextEngine
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockContextEngine;

#[async_trait]
impl ContextEngine for MockContextEngine {
    type Error = MockError;

    async fn assemble_context(&self, _run_id: RunId) -> Result<AssembledContext, Self::Error> {
        Ok(AssembledContext {
            system_anchor: "You are a helpful assistant.".to_string(),
            task_manifest: "Build a thing.".to_string(),
            injected_knowledge: vec![],
            tool_results: vec![],
            history: String::new(),
            total_tokens: 100,
            slot_tokens: SlotTokenCounts::default(),
        })
    }

    fn check_budget(&self, _context: &AssembledContext, _budget: &ContextBudget) -> BudgetStatus {
        BudgetStatus::WithinBudget
    }

    async fn trigger_compaction(
        &self,
        _run_id: RunId,
        _stage: CompactionStage,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn compute_hygiene_score(
        &self,
        _run_id: RunId,
        _window_size: usize,
    ) -> Result<HygieneScore, Self::Error> {
        Ok(HygieneScore {
            score: 0.95,
            on_task_fraction: 0.95,
            consecutive_degraded: 0,
        })
    }

    async fn inject_todo_recitation(&self, _run_id: RunId) -> Result<(), Self::Error> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mock MemoryStore
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockMemoryStore {
    pub entries: Mutex<Vec<MemoryEntry>>,
}

#[async_trait]
impl MemoryStore for MockMemoryStore {
    type Error = MockError;

    async fn read(&self, tier: MemoryTier, id: &str) -> Result<Option<MemoryEntry>, Self::Error> {
        let entries = self.entries.lock().unwrap();
        Ok(entries
            .iter()
            .find(|e| e.tier == tier && e.id == id)
            .cloned())
    }

    async fn write(&self, _tier: MemoryTier, entry: &MemoryEntry) -> Result<(), Self::Error> {
        self.entries.lock().unwrap().push(entry.clone());
        Ok(())
    }

    async fn search(
        &self,
        tier: MemoryTier,
        _query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchResult>, Self::Error> {
        let entries = self.entries.lock().unwrap();
        Ok(entries
            .iter()
            .filter(|e| e.tier == tier)
            .take(limit)
            .map(|e| MemorySearchResult {
                entry: e.clone(),
                relevance_score: 1.0,
            })
            .collect())
    }

    async fn promote(
        &self,
        _entry_id: &str,
        _from: MemoryTier,
        _to: MemoryTier,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mock SkillLoader
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockSkillLoader;

#[async_trait]
impl SkillLoader for MockSkillLoader {
    type Error = MockError;

    async fn list_skills(&self) -> Result<Vec<(String, String)>, Self::Error> {
        Ok(vec![])
    }

    async fn load_skill(&self, _name: &str) -> Result<Option<Skill>, Self::Error> {
        Ok(None)
    }

    async fn match_skills(&self, _task_context: &str) -> Result<Vec<String>, Self::Error> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Mock ToolGateway
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockToolGateway {
    pub call_log: Mutex<Vec<ToolInvocation>>,
}

#[async_trait]
impl ToolGateway for MockToolGateway {
    type Error = MockError;

    async fn call_tool(&self, invocation: ToolInvocation) -> Result<ToolResult, Self::Error> {
        self.call_log.lock().unwrap().push(invocation.clone());
        Ok(ToolResult {
            tool_name: invocation.tool_name,
            status: ToolResultStatus::Success,
            output: serde_json::json!({"result": "ok"}),
            remediation_hint: None,
            latency_ms: 1,
            cached_tokens_used: 0,
            uncached_tokens_used: 0,
        })
    }
}

// ---------------------------------------------------------------------------
// Mock FrozenToolRegistry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct MockFrozenToolRegistry {
    pub tools: Vec<ToolDefinition>,
}

impl FrozenToolRegistry for MockFrozenToolRegistry {
    fn get_manifest(&self) -> &[ToolDefinition] {
        &self.tools
    }

    fn is_permitted(&self, tool_name: &str, _trust_level: TrustLevel) -> bool {
        self.tools.iter().any(|t| t.name == tool_name)
    }

    fn get_tool(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.iter().find(|t| t.name == name)
    }
}

// ---------------------------------------------------------------------------
// Mock Orchestrator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockOrchestrator;

#[async_trait]
impl Orchestrator for MockOrchestrator {
    type Error = MockError;

    async fn spawn(
        &self,
        _parent_run_id: RunId,
        _config: SpawnConfig,
    ) -> Result<RunId, Self::Error> {
        Ok(uuid::Uuid::new_v4())
    }

    async fn await_result(&self, sub_run_id: RunId) -> Result<SubAgentResult, Self::Error> {
        Ok(SubAgentResult {
            run_id: sub_run_id,
            status: RunState::Completed,
            summary: "Mock sub-agent completed.".to_string(),
            artefacts: vec![],
        })
    }

    fn current_depth(&self, _run_id: RunId) -> Result<u32, Self::Error> {
        Ok(0)
    }

    fn can_spawn(&self, _parent_run_id: RunId) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Mock TaskDispatch
// ---------------------------------------------------------------------------

pub struct MockTaskDispatch {
    tasks: Mutex<std::collections::VecDeque<(String, String, DispatchOptions)>>,
    claimed: Mutex<std::collections::HashMap<String, ClaimedTask>>,
    completed: Mutex<std::collections::HashSet<String>>,
}

impl std::fmt::Debug for MockTaskDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockTaskDispatch").finish()
    }
}

impl Default for MockTaskDispatch {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(std::collections::VecDeque::new()),
            claimed: Mutex::new(std::collections::HashMap::new()),
            completed: Mutex::new(std::collections::HashSet::new()),
        }
    }
}

impl TaskDispatch for MockTaskDispatch {
    type Error = MockError;

    fn enqueue(&self, body: &str, opts: DispatchOptions) -> Result<String, Self::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        self.tasks
            .lock()
            .unwrap()
            .push_back((id.clone(), body.to_string(), opts));
        Ok(id)
    }

    fn list_ready(&self) -> Result<Vec<String>, Self::Error> {
        let tasks = self.tasks.lock().unwrap();
        let completed = self.completed.lock().unwrap();
        Ok(tasks
            .iter()
            .filter(|(_, _, opts)| opts.depends_on.iter().all(|dep| completed.contains(dep)))
            .map(|(id, _, _)| id.clone())
            .collect())
    }

    fn dequeue(&self) -> Result<Option<ClaimedTask>, Self::Error> {
        let mut tasks = self.tasks.lock().unwrap();
        match tasks.pop_front() {
            None => Ok(None),
            Some((id, body, _opts)) => {
                let task = ClaimedTask {
                    id: id.clone(),
                    body,
                    reply_to: None,
                };
                self.claimed.lock().unwrap().insert(id, task.clone());
                Ok(Some(task))
            }
        }
    }

    fn complete(&self, task: &ClaimedTask) -> Result<(), Self::Error> {
        self.claimed.lock().unwrap().remove(&task.id);
        self.completed.lock().unwrap().insert(task.id.clone());
        Ok(())
    }

    fn fail(&self, task: &ClaimedTask) -> Result<(), Self::Error> {
        self.claimed.lock().unwrap().remove(&task.id);
        Ok(())
    }

    fn depth(&self) -> Result<i64, Self::Error> {
        let tasks = self.tasks.lock().unwrap().len() as i64;
        let claimed = self.claimed.lock().unwrap().len() as i64;
        Ok(tasks + claimed)
    }
}

// ---------------------------------------------------------------------------
// Mock HitlController
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockHitlController {
    pub gates: Mutex<Vec<HitlRecord>>,
}

#[async_trait]
impl HitlController for MockHitlController {
    type Error = MockError;

    async fn open_gate(&self, record: HitlRecord) -> Result<(), Self::Error> {
        self.gates.lock().unwrap().push(record);
        Ok(())
    }

    async fn record_decision(
        &self,
        run_id: RunId,
        decision: HitlDecision,
    ) -> Result<(), Self::Error> {
        let mut gates = self.gates.lock().unwrap();
        if let Some(gate) = gates.iter_mut().find(|g| g.run_id == run_id) {
            gate.decision = Some(decision);
            Ok(())
        } else {
            Err(MockError(format!("no gate for run {run_id}")))
        }
    }

    async fn pending_gates(&self) -> Result<Vec<HitlRecord>, Self::Error> {
        let gates = self.gates.lock().unwrap();
        Ok(gates
            .iter()
            .filter(|g| g.decision.is_none())
            .cloned()
            .collect())
    }

    async fn get_decision(
        &self,
        run_id: RunId,
        _gate_id: &str,
    ) -> Result<Option<HitlDecision>, Self::Error> {
        let gates = self.gates.lock().unwrap();
        Ok(gates
            .iter()
            .find(|g| g.run_id == run_id)
            .and_then(|g| g.decision.clone()))
    }
}

// ---------------------------------------------------------------------------
// Mock Notifier
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockNotifier {
    pub notifications: Mutex<Vec<String>>,
}

#[async_trait]
impl Notifier for MockNotifier {
    type Error = MockError;

    async fn notify(&self, record: &HitlRecord) -> Result<(), Self::Error> {
        self.notifications.lock().unwrap().push(format!(
            "gate:{} run:{}",
            record.gate_category, record.run_id
        ));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mock TrajectoryStore
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockTrajectoryStore {
    pub events: Mutex<Vec<TrajectoryEvent>>,
}

#[async_trait]
impl TrajectoryStore for MockTrajectoryStore {
    type Error = MockError;

    async fn emit_event(&self, event: TrajectoryEvent) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    async fn query_events(
        &self,
        run_id: Option<RunId>,
        event_type: Option<EventType>,
        _time_range: Option<TimeRange>,
        limit: Option<usize>,
    ) -> Result<Vec<TrajectoryEvent>, Self::Error> {
        let events = self.events.lock().unwrap();
        let filtered: Vec<_> = events
            .iter()
            .filter(|e| run_id.is_none() || Some(e.run_id) == run_id)
            .filter(|e| event_type.is_none() || Some(e.event_type) == event_type)
            .cloned()
            .collect();
        match limit {
            Some(n) => Ok(filtered.into_iter().take(n).collect()),
            None => Ok(filtered),
        }
    }

    async fn export(&self, run_id: RunId, _format: ExportFormat) -> Result<Vec<u8>, Self::Error> {
        let events = self.events.lock().unwrap();
        let run_events: Vec<_> = events.iter().filter(|e| e.run_id == run_id).collect();
        let json = serde_json::to_vec(&run_events).map_err(|e| MockError(e.to_string()))?;
        Ok(json)
    }
}

// ---------------------------------------------------------------------------
// Mock LlmClient
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct MockLlmClient {
    pub responses: Mutex<Vec<LlmResponse>>,
}

impl Default for MockLlmClient {
    fn default() -> Self {
        Self {
            responses: Mutex::new(vec![LlmResponse {
                content: "Hello from mock LLM.".to_string(),
                tool_calls: vec![],
                usage: LlmUsage {
                    input_tokens: 50,
                    output_tokens: 10,
                    cached_tokens: 40,
                },
                stop_reason: Some("end_turn".to_string()),
            }]),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    type Error = MockError;

    async fn complete(
        &self,
        _messages: &[LlmMessage],
        _model: &str,
        _tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmResponse, Self::Error> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Err(MockError("no mock responses remaining".to_string()));
        }
        Ok(responses.remove(0))
    }
}

// ---------------------------------------------------------------------------
// Mock MetricsExporter
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockMetricsExporter {
    pub metrics: Mutex<Vec<String>>,
}

impl MetricsExporter for MockMetricsExporter {
    fn export_metrics(&self) -> String {
        self.metrics.lock().unwrap().join("\n")
    }

    fn gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        let label_str: Vec<_> = labels.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
        self.metrics.lock().unwrap().push(format!(
            "{name}{{{labels}}} {value}",
            labels = label_str.join(",")
        ));
    }

    fn counter(&self, name: &str, labels: &[(&str, &str)]) {
        let label_str: Vec<_> = labels.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
        self.metrics.lock().unwrap().push(format!(
            "{name}{{{labels}}} +1",
            labels = label_str.join(",")
        ));
    }
}

// ---------------------------------------------------------------------------
// Mock ConstraintEnforcer
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockConstraintEnforcer {
    pub constraints_list: Vec<ConstraintDefinition>,
}

#[async_trait]
impl ConstraintEnforcer for MockConstraintEnforcer {
    type Error = MockError;

    fn constraints(&self) -> &[ConstraintDefinition] {
        &self.constraints_list
    }

    async fn check_all(&self, _run_id: RunId) -> Result<Vec<ConstraintResult>, Self::Error> {
        Ok(self
            .constraints_list
            .iter()
            .map(|c| ConstraintResult {
                constraint_name: c.name.clone(),
                passed: true,
                violations: vec![],
            })
            .collect())
    }

    async fn check_one(
        &self,
        _run_id: RunId,
        constraint_name: &str,
    ) -> Result<ConstraintResult, Self::Error> {
        Ok(ConstraintResult {
            constraint_name: constraint_name.to_string(),
            passed: true,
            violations: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Mock ArtefactValidator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MockArtefactValidator;

#[async_trait]
impl ArtefactValidator for MockArtefactValidator {
    type Error = MockError;

    async fn validate_initialiser_output(
        &self,
        _run_id: RunId,
        artefacts: &RunArtefacts,
    ) -> Result<(), Self::Error> {
        artefacts.validate().map_err(|e| MockError(e.to_string()))
    }
}
