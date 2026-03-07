use stratum_types::{RunArtefacts, StratumRun, TaskManifest};

/// Generates the Initialiser Prompt for the two-prompt pattern.
///
/// The Initialiser runs once at the start of a run. It instructs the model to
/// produce four artefacts (TASK.md, PROGRESS.md, DECISIONS.md, and optionally
/// INIT.sh) that persist across all context window resets.
pub fn initialiser_prompt(run: &StratumRun, user_goal: &str) -> String {
    format!(
        r#"You are an AI agent operating within the Stratum harness.

## Your Task

{user_goal}

## Instructions

You MUST produce the following artefacts before doing any other work:

### 1. TASK.md
A structured task description containing:
- **Goal**: A clear, specific restatement of the task
- **Acceptance Criteria**: Numbered list of verifiable conditions that define "done"
- **Scope Boundaries**: What is explicitly out of scope

### 2. PROGRESS.md
A checklist tracking each step needed to complete the task:
- Use `- [ ]` for pending items
- Use `- [x]` for completed items
- Order by dependency (prerequisites first)
- Each item should be independently verifiable

### 3. DECISIONS.md
A log of architectural and implementation decisions:
- Start empty or with initial decisions about approach
- Each entry: timestamp, context, choice, rationale

### 4. INIT.sh (optional)
If the task requires environment setup (installing dependencies, creating directories, etc.),
produce a shell script. If not needed, omit this artefact.

## Constraints

- Model: {model_ref}
- Trust Level: {trust_level:?}
- Available Tools: {tools}
- Spawn Depth Limit: {depth}

## Output Format

Produce each artefact as a clearly delimited section. Use the exact headers:
```
=== TASK.md ===
(content)

=== PROGRESS.md ===
(content)

=== DECISIONS.md ===
(content)

=== INIT.sh ===
(content, or omit this section entirely)
```

Do NOT begin executing the task. Only produce the planning artefacts."#,
        user_goal = user_goal,
        model_ref = run.model_ref,
        trust_level = run.trust_level,
        tools = run.tool_manifest.join(", "),
        depth = run.spawn_depth_limit,
    )
}

/// Generates the Worker Prompt for the two-prompt pattern.
///
/// The Worker runs for all subsequent context windows after initialisation.
/// It reads the current artefact state and identifies the highest-priority
/// incomplete item to work on next.
pub fn worker_prompt(
    run: &StratumRun,
    artefacts: &RunArtefacts,
    manifest: &TaskManifest,
) -> String {
    let progress_summary = manifest
        .progress
        .iter()
        .map(|p| {
            let check = if p.status == stratum_types::ProgressStatus::Completed {
                "x"
            } else {
                " "
            };
            format!("- [{}] {}", check, p.description)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let decisions_summary = manifest
        .decisions
        .iter()
        .map(|d| format!("- {}: {} ({})", d.context, d.choice, d.rationale))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You are an AI agent operating within the Stratum harness.

## Current Task

{goal}

## TASK.md
{task_md}

## PROGRESS.md
{progress_md}

Progress Summary:
{progress_summary}

## DECISIONS.md
{decisions_md}

Decisions Summary:
{decisions_summary}

## Instructions

1. Read the current PROGRESS.md state
2. Identify the highest-priority incomplete item (first unchecked `- [ ]`)
3. Execute that item using available tools
4. Update PROGRESS.md to mark the item complete
5. If you encounter a decision point, log it in DECISIONS.md
6. If blocked, add a blocker entry and move to the next unblocked item

## Constraints

- Model: {model_ref}
- Trust Level: {trust_level:?}
- Available Tools: {tools}
- Work incrementally. Complete one item at a time.
- Update artefact files after each completed step.
- Do NOT skip ahead or work on items whose prerequisites are incomplete."#,
        goal = manifest.goal,
        task_md = artefacts.task_md,
        progress_md = artefacts.progress_md,
        progress_summary = if progress_summary.is_empty() {
            "(no progress items yet)".to_string()
        } else {
            progress_summary
        },
        decisions_md = artefacts.decisions_md,
        decisions_summary = if decisions_summary.is_empty() {
            "(no decisions yet)".to_string()
        } else {
            decisions_summary
        },
        model_ref = run.model_ref,
        trust_level = run.trust_level,
        tools = run.tool_manifest.join(", "),
    )
}

/// Generates the system anchor for daemon mode with full tool descriptions.
pub fn daemon_system_anchor(tool_names: &[String]) -> String {
    format!(
        r#"You are an autonomous AI agent operating within the Stratum daemon harness.

## Capabilities

You have full computer access through your tools. You can:

- **Execute commands**: Use `bash` to run any shell command
- **File operations**: Use `read_file`, `write_file`, `list_directory`, `search_files`
- **Memory**: Use `memory_write`, `memory_search`, `memory_promote` to store and recall knowledge across runs
- **Create tools**: Use `create_tool` to register new reusable tools from scripts you write
- **Create skills**: Use `create_skill` to create skill documents that provide context for future tasks

## Available Tools

{tools}

## Operating Principles

1. **Be autonomous**: Complete tasks fully without asking for human input
2. **Use memory actively**: Search for relevant memories before starting work; write useful learnings after completing tasks
3. **Create reusable tools**: If you find yourself doing something repeatedly, create a tool for it
4. **Create skills**: Document approaches and patterns as skills for future reference
5. **Work incrementally**: Complete one step at a time, verifying each step works
6. **Signal completion**: When the task is fully done, respond with "TASK COMPLETE" in your message"#,
        tools = tool_names.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use stratum_test_utils::mocks::make_test_run;
    use stratum_types::*;

    #[test]
    fn test_initialiser_prompt_contains_goal() {
        let run = make_test_run();
        let prompt = initialiser_prompt(&run, "Build a REST API");
        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("TASK.md"));
        assert!(prompt.contains("PROGRESS.md"));
        assert!(prompt.contains("DECISIONS.md"));
        assert!(prompt.contains("test-model"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn test_initialiser_prompt_includes_constraints() {
        let run = make_test_run();
        let prompt = initialiser_prompt(&run, "Do something");
        assert!(prompt.contains("Supervised"));
        assert!(prompt.contains("Spawn Depth Limit: 2"));
    }

    #[test]
    fn test_worker_prompt_contains_artefacts() {
        let run = make_test_run();
        let artefacts = RunArtefacts {
            task_md: "# Goal\nBuild a REST API".to_string(),
            progress_md: "- [ ] Set up project\n- [ ] Add endpoints".to_string(),
            decisions_md: "No decisions yet.".to_string(),
            init_sh: None,
        };
        let manifest = TaskManifest {
            goal: "Build a REST API".to_string(),
            acceptance_criteria: vec![],
            progress: vec![
                ProgressItem {
                    id: "1".to_string(),
                    description: "Set up project".to_string(),
                    status: ProgressStatus::Completed,
                    completed_at: Some(Utc::now()),
                },
                ProgressItem {
                    id: "2".to_string(),
                    description: "Add endpoints".to_string(),
                    status: ProgressStatus::Pending,
                    completed_at: None,
                },
            ],
            decisions: vec![],
            blockers: vec![],
        };

        let prompt = worker_prompt(&run, &artefacts, &manifest);
        assert!(prompt.contains("Build a REST API"));
        assert!(prompt.contains("[x] Set up project"));
        assert!(prompt.contains("[ ] Add endpoints"));
        assert!(prompt.contains("highest-priority incomplete item"));
    }

    #[test]
    fn test_worker_prompt_empty_progress() {
        let run = make_test_run();
        let artefacts = RunArtefacts {
            task_md: "# Goal".to_string(),
            progress_md: "Empty".to_string(),
            decisions_md: "Empty".to_string(),
            init_sh: None,
        };
        let manifest = TaskManifest {
            goal: "Do a thing".to_string(),
            acceptance_criteria: vec![],
            progress: vec![],
            decisions: vec![],
            blockers: vec![],
        };

        let prompt = worker_prompt(&run, &artefacts, &manifest);
        assert!(prompt.contains("(no progress items yet)"));
        assert!(prompt.contains("(no decisions yet)"));
    }
}
