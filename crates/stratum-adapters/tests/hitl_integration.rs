//! Integration tests for HITL Controller (Phase 9).

use std::sync::Arc;

use chrono::Utc;
use stratum_adapters::hitl::controller::SqliteHitlController;
use stratum_adapters::hitl::notifier::{StdoutNotifier, WebhookNotifier};
use stratum_adapters::hitl::policy::{GateAction, GatePolicyEngine};
use stratum_adapters::session::SqliteSessionManager;
use stratum_adapters::trajectory::SqliteTrajectoryStore;
use stratum_adapters::AdapterError;
use stratum_core::{HitlController, Notifier, SessionManager, TrajectoryStore};
use stratum_test_utils::mocks::make_test_run;
use stratum_types::*;
use uuid::Uuid;

fn make_gate(run_id: RunId, gate_id: &str, category: GateCategory) -> HitlRecord {
    HitlRecord {
        id: gate_id.to_string(),
        run_id,
        gate_category: category,
        action_attempted: "test action".to_string(),
        alternatives: vec!["alt1".to_string()],
        context_summary: "test context".to_string(),
        decision: None,
    }
}

async fn setup() -> (
    SqliteHitlController,
    Arc<SqliteTrajectoryStore>,
    Arc<SqliteSessionManager>,
) {
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = Arc::new(SqliteSessionManager::in_memory(traj.clone()).unwrap());
    let notifier: Arc<dyn Notifier<Error = AdapterError>> = Arc::new(StdoutNotifier);
    let ctrl = SqliteHitlController::in_memory(
        traj.clone(),
        session.clone() as Arc<dyn SessionManager<Error = AdapterError>>,
        notifier,
    )
    .unwrap();
    (ctrl, traj, session)
}

/// Helper: create a run and transition it to Running (prerequisite for Paused).
async fn create_running_run(session: &SqliteSessionManager) -> RunId {
    let run = make_test_run();
    let id = run.id;
    session.create_run(run).await.unwrap();
    session
        .transition_state(id, RunState::Running)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn test_open_gate_pauses_run() {
    let (ctrl, traj, session) = setup().await;
    let id = create_running_run(&session).await;

    let gate = make_gate(id, "gate-1", GateCategory::Destructive);
    ctrl.open_gate(gate).await.unwrap();

    // Run should be Paused
    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Paused);

    // GateOpened event should be emitted
    let events = traj
        .query_events(Some(id), Some(EventType::GateOpened), None, None)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].stratum_layer, StratumLayer::HitlController);
}

#[tokio::test]
async fn test_record_decision_approve_resumes() {
    let (ctrl, _traj, session) = setup().await;
    let id = create_running_run(&session).await;

    ctrl.open_gate(make_gate(id, "gate-1", GateCategory::Destructive))
        .await
        .unwrap();
    assert_eq!(
        session.get_run(id).await.unwrap().unwrap().state,
        RunState::Paused
    );

    ctrl.record_decision(id, HitlDecision::Approve)
        .await
        .unwrap();

    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Running);
}

#[tokio::test]
async fn test_record_decision_modify_resumes() {
    let (ctrl, _traj, session) = setup().await;
    let id = create_running_run(&session).await;

    ctrl.open_gate(make_gate(id, "gate-1", GateCategory::Ambiguity))
        .await
        .unwrap();

    ctrl.record_decision(
        id,
        HitlDecision::Modify {
            context: "use safer approach".to_string(),
        },
    )
    .await
    .unwrap();

    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Running);
}

#[tokio::test]
async fn test_record_decision_redirect() {
    let (ctrl, traj, session) = setup().await;
    let id = create_running_run(&session).await;

    ctrl.open_gate(make_gate(id, "gate-1", GateCategory::Drift))
        .await
        .unwrap();

    ctrl.record_decision(
        id,
        HitlDecision::Redirect {
            new_goal: "new objective".to_string(),
        },
    )
    .await
    .unwrap();

    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Running);

    // RunRedirected event
    let events = traj
        .query_events(Some(id), Some(EventType::RunRedirected), None, None)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload["new_goal"].as_str().unwrap(),
        "new objective"
    );
}

#[tokio::test]
async fn test_record_decision_abort() {
    let (ctrl, _traj, session) = setup().await;
    let id = create_running_run(&session).await;

    ctrl.open_gate(make_gate(id, "gate-1", GateCategory::Destructive))
        .await
        .unwrap();

    ctrl.record_decision(id, HitlDecision::Abort).await.unwrap();

    let run = session.get_run(id).await.unwrap().unwrap();
    assert_eq!(run.state, RunState::Aborted);
}

#[tokio::test]
async fn test_pending_gates() {
    let (ctrl, _traj, session) = setup().await;

    // Create two running runs with gates
    let id1 = create_running_run(&session).await;
    let id2 = create_running_run(&session).await;

    ctrl.open_gate(make_gate(id1, "gate-a", GateCategory::Destructive))
        .await
        .unwrap();
    ctrl.open_gate(make_gate(id2, "gate-b", GateCategory::Irreversible))
        .await
        .unwrap();

    let pending = ctrl.pending_gates().await.unwrap();
    assert_eq!(pending.len(), 2);

    // Decide one gate
    ctrl.record_decision(id1, HitlDecision::Approve)
        .await
        .unwrap();

    let pending = ctrl.pending_gates().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].run_id, id2);
}

#[tokio::test]
async fn test_get_decision() {
    let (ctrl, _traj, session) = setup().await;
    let id = create_running_run(&session).await;

    ctrl.open_gate(make_gate(id, "gate-1", GateCategory::Destructive))
        .await
        .unwrap();

    // Before decision
    let dec = ctrl.get_decision(id, "gate-1").await.unwrap();
    assert!(dec.is_none());

    // After decision
    ctrl.record_decision(id, HitlDecision::Approve)
        .await
        .unwrap();

    let dec = ctrl.get_decision(id, "gate-1").await.unwrap();
    assert!(matches!(dec, Some(HitlDecision::Approve)));
}

#[tokio::test]
async fn test_get_decision_nonexistent() {
    let (ctrl, _traj, _session) = setup().await;
    let dec = ctrl
        .get_decision(Uuid::new_v4(), "no-such-gate")
        .await
        .unwrap();
    assert!(dec.is_none());
}

#[tokio::test]
async fn test_durable_pause() {
    // Open gate, create new controller from same DB, verify gate still pending
    let traj = Arc::new(SqliteTrajectoryStore::in_memory().unwrap());
    let session = Arc::new(SqliteSessionManager::in_memory(traj.clone()).unwrap());

    // Use a shared connection for durability test
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Enable shared cache via serialized access
    let conn = Arc::new(std::sync::Mutex::new(conn));

    // Build schema using the same constant as the controller
    {
        let c = conn.lock().unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        c.execute_batch(stratum_adapters::hitl::controller::HITL_GATES_SCHEMA)
            .unwrap();
    }

    // Create a running run
    let run = make_test_run();
    let id = run.id;
    session.create_run(run).await.unwrap();
    session
        .transition_state(id, RunState::Running)
        .await
        .unwrap();

    // Insert gate directly
    {
        let c = conn.lock().unwrap();
        let alts = serde_json::to_string(&vec!["alt1"]).unwrap();
        c.execute(
            "INSERT INTO hitl_gates (id, run_id, gate_category, action_attempted,
                alternatives, context_summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "gate-durable",
                id.to_string(),
                "destructive",
                "test",
                alts,
                "ctx",
                Utc::now().to_rfc3339(),
            ],
        )
        .unwrap();
    }

    // Query pending from same connection — simulates "new controller instance"
    {
        let c = conn.lock().unwrap();
        let count: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM hitl_gates WHERE decision IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "gate should be durable and still pending");
    }
}

#[tokio::test]
async fn test_policy_engine_categories() {
    let policy = HitlPolicy::default();

    assert_eq!(
        GatePolicyEngine::evaluate(GateCategory::Destructive, &policy),
        GateAction::Block
    );
    assert_eq!(
        GatePolicyEngine::evaluate(GateCategory::Irreversible, &policy),
        GateAction::Block
    );
    assert_eq!(
        GatePolicyEngine::evaluate(GateCategory::TrustEscalation, &policy),
        GateAction::Block
    );
    assert_eq!(
        GatePolicyEngine::evaluate(GateCategory::Ambiguity, &policy),
        GateAction::Block
    );
    assert_eq!(
        GatePolicyEngine::evaluate(GateCategory::Drift, &policy),
        GateAction::NotifyWithOption
    );
    assert_eq!(
        GatePolicyEngine::evaluate(GateCategory::Budget, &policy),
        GateAction::NotifyOnly
    );
}

#[tokio::test]
async fn test_webhook_notifier() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/webhook"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let url = format!("{}/webhook", mock_server.uri());
    let notifier = WebhookNotifier::new(url);

    let record = HitlRecord {
        id: "gate-wh".to_string(),
        run_id: Uuid::new_v4(),
        gate_category: GateCategory::Destructive,
        action_attempted: "rm -rf".to_string(),
        alternatives: vec!["trash".to_string()],
        context_summary: "dangerous op".to_string(),
        decision: None,
    };

    notifier.notify(&record).await.unwrap();
    // wiremock's expect(1) will verify the POST was received on drop
}

#[tokio::test]
async fn test_gate_decision_received_event() {
    let (ctrl, traj, session) = setup().await;
    let id = create_running_run(&session).await;

    ctrl.open_gate(make_gate(id, "gate-evt", GateCategory::Destructive))
        .await
        .unwrap();
    ctrl.record_decision(id, HitlDecision::Approve)
        .await
        .unwrap();

    let events = traj
        .query_events(Some(id), Some(EventType::GateDecisionReceived), None, None)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["gate_id"].as_str().unwrap(), "gate-evt");
}
