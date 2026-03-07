//! Simple context assembler: system prompt + task + last N messages, truncate oldest.

use stratum_core::*;

/// Assemble a message list for an LLM call.
///
/// Strategy: system prompt first, then the task goal, then the last N history
/// messages (truncating oldest when over budget).
pub fn assemble_context(
    system_prompt: &str,
    task_goal: &str,
    history: &[LlmMessage],
    budget: &ContextBudget,
) -> Vec<LlmMessage> {
    let mut messages = Vec::new();

    // System message
    messages.push(LlmMessage {
        role: "system".to_string(),
        content: system_prompt.to_string(),
    });

    // Task goal as the first user message
    messages.push(LlmMessage {
        role: "user".to_string(),
        content: task_goal.to_string(),
    });

    // Take last N history messages (budget.max_history_messages minus the 2 we already added)
    let max_history = budget.max_history_messages.saturating_sub(2);
    let start = history.len().saturating_sub(max_history);
    messages.extend_from_slice(&history[start..]);

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_assembly() {
        let msgs = assemble_context("sys", "goal", &[], &ContextBudget::default());
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content, "sys");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, "goal");
    }

    #[test]
    fn truncates_oldest_history() {
        let history: Vec<LlmMessage> = (0..100)
            .map(|i| LlmMessage {
                role: "user".to_string(),
                content: format!("msg {i}"),
            })
            .collect();

        let budget = ContextBudget {
            max_tokens: 128_000,
            max_history_messages: 10,
        };

        let msgs = assemble_context("sys", "goal", &history, &budget);
        // 2 (system + goal) + 8 (10 - 2 overhead) = 10
        assert_eq!(msgs.len(), 10);
        // Last message should be msg 99
        assert_eq!(msgs.last().unwrap().content, "msg 99");
        // First history message should be msg 92 (100 - 8 = 92)
        assert_eq!(msgs[2].content, "msg 92");
    }

    #[test]
    fn small_budget_still_includes_system_and_goal() {
        let history: Vec<LlmMessage> = (0..5)
            .map(|i| LlmMessage {
                role: "user".to_string(),
                content: format!("msg {i}"),
            })
            .collect();

        let budget = ContextBudget {
            max_tokens: 128_000,
            max_history_messages: 2,
        };

        let msgs = assemble_context("sys", "goal", &history, &budget);
        // Only system + goal, no room for history
        assert_eq!(msgs.len(), 2);
    }
}
