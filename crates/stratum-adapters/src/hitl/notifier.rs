//! Notifier implementations — stdout and webhook.

use async_trait::async_trait;
use stratum_core::Notifier;
use stratum_types::HitlRecord;

use crate::error::AdapterError;

/// Prints gate information to stdout. Default notifier for local development.
#[derive(Debug)]
pub struct StdoutNotifier;

#[async_trait]
impl Notifier for StdoutNotifier {
    type Error = AdapterError;

    async fn notify(&self, record: &HitlRecord) -> Result<(), Self::Error> {
        println!(
            "[HITL] Gate opened: id={} run={} category={} action={}",
            record.id, record.run_id, record.gate_category, record.action_attempted
        );
        if !record.alternatives.is_empty() {
            println!("[HITL] Alternatives: {}", record.alternatives.join(", "));
        }
        println!("[HITL] Context: {}", record.context_summary);
        Ok(())
    }
}

/// Sends gate information as an HTTP POST to a configured webhook URL.
#[derive(Debug)]
pub struct WebhookNotifier {
    client: reqwest::Client,
    url: String,
}

impl WebhookNotifier {
    /// Create a new webhook notifier targeting the given URL.
    pub fn new(url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
        }
    }
}

#[async_trait]
impl Notifier for WebhookNotifier {
    type Error = AdapterError;

    async fn notify(&self, record: &HitlRecord) -> Result<(), Self::Error> {
        let body = serde_json::json!({
            "gate_id": record.id,
            "run_id": record.run_id.to_string(),
            "gate_category": record.gate_category,
            "action_attempted": record.action_attempted,
            "alternatives": record.alternatives,
            "context_summary": record.context_summary,
        });

        self.client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AdapterError::Webhook(e.to_string()))?
            .error_for_status()
            .map_err(|e| AdapterError::Webhook(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample_record() -> HitlRecord {
        HitlRecord {
            id: "gate-1".to_string(),
            run_id: Uuid::new_v4(),
            gate_category: "destructive".to_string(),
            action_attempted: "rm -rf /".to_string(),
            alternatives: vec!["trash".to_string()],
            context_summary: "User requested deletion".to_string(),
            decision: None,
        }
    }

    #[tokio::test]
    async fn test_stdout_notifier_does_not_panic() {
        let notifier = StdoutNotifier;
        let record = sample_record();
        notifier.notify(&record).await.unwrap();
    }
}
