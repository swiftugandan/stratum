//! Three-stage compaction pipeline for the Context Engine.
//!
//! - Stage 1 (Offload): Large tool results written to filesystem, replaced with reference + preview.
//! - Stage 2 (Truncate): Already-persisted content reduced to reference only.
//! - Stage 3 (Summarise): LLM-based structured summarisation. Full history archived to trajectory.

use std::path::{Path, PathBuf};

use crate::token::count_tokens;

/// Default threshold in tokens for offloading a single tool result.
pub const OFFLOAD_TOKEN_THRESHOLD: u64 = 3_750; // ~15K chars

/// Prefix used in offloaded references, checked during truncation.
const OFFLOAD_PREFIX: &str = "[Offloaded to ";

/// Number of preview lines to keep when offloading.
const PREVIEW_LINES: usize = 10;

/// Result of running a compaction stage on a list of context slot entries.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The compacted entries (replacements for originals).
    pub entries: Vec<String>,
    /// How many tokens were freed.
    pub tokens_freed: u64,
    /// Files written during offload (stage 1 only).
    pub offloaded_files: Vec<PathBuf>,
}

/// Stage 1: Offload large tool results to filesystem.
///
/// Any entry exceeding `threshold_tokens` is written to `offload_dir/<index>.md`
/// and replaced with a reference + preview. Entries where `protect[i]` is true
/// are never offloaded (error entries stay in context per SAD §7.2).
pub fn offload(
    entries: &[String],
    offload_dir: &Path,
    threshold_tokens: u64,
    protect: &[bool],
) -> Result<CompactionResult, std::io::Error> {
    std::fs::create_dir_all(offload_dir)?;

    let mut compacted = Vec::with_capacity(entries.len());
    let mut tokens_freed = 0u64;
    let mut offloaded_files = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let is_protected = protect.get(i).copied().unwrap_or(false);
        let entry_tokens = count_tokens(entry);
        if !is_protected && entry_tokens > threshold_tokens {
            let file_path = offload_dir.join(format!("{i}.md"));
            std::fs::write(&file_path, entry)?;
            offloaded_files.push(file_path.clone());

            let preview = make_preview(entry, PREVIEW_LINES);
            let reference = format!(
                "{}{}] ({} tokens)\n\n{}",
                OFFLOAD_PREFIX,
                file_path.display(),
                entry_tokens,
                preview,
            );
            let ref_tokens = count_tokens(&reference);
            tokens_freed += entry_tokens.saturating_sub(ref_tokens);
            compacted.push(reference);
        } else {
            compacted.push(entry.clone());
        }
    }

    Ok(CompactionResult {
        entries: compacted,
        tokens_freed,
        offloaded_files,
    })
}

/// Stage 2: Truncate already-offloaded entries to reference only (remove preview).
/// Protected entries are never truncated.
pub fn truncate(entries: &[String], protect: &[bool]) -> CompactionResult {
    let mut compacted = Vec::with_capacity(entries.len());
    let mut tokens_freed = 0u64;

    for (i, entry) in entries.iter().enumerate() {
        let is_protected = protect.get(i).copied().unwrap_or(false);
        if !is_protected && entry.starts_with(OFFLOAD_PREFIX) {
            // Keep only the first line (the reference)
            let first_line = entry.lines().next().unwrap_or(entry);
            let original_tokens = count_tokens(entry);
            let new_tokens = count_tokens(first_line);
            tokens_freed += original_tokens.saturating_sub(new_tokens);
            compacted.push(first_line.to_string());
        } else {
            compacted.push(entry.clone());
        }
    }

    CompactionResult {
        entries: compacted,
        tokens_freed,
        offloaded_files: vec![],
    }
}

/// Build a structured summarisation prompt for Stage 3.
///
/// The caller is responsible for sending this to the LLM and using the response.
pub fn build_summarisation_prompt(history: &str, task_goal: &str) -> String {
    format!(
        r#"Summarise the following agent conversation history into a structured summary.
Preserve the following information:
- Goal: {task_goal}
- Completed steps (with key outcomes)
- Key decisions made (with rationale)
- Files modified
- Errors encountered and resolutions
- Current blockers
- Next planned action

Be concise. Use bullet points. Do not include raw tool output.

## History

{history}"#
    )
}

fn make_preview(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().take(max_lines).collect();
    let preview = lines.join("\n");
    // Check if there are more lines without counting them all
    if text.lines().nth(max_lines).is_some() {
        format!("{preview}\n... (truncated)")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_offload_small_entries_unchanged() {
        let dir = TempDir::new().unwrap();
        let entries = vec!["short result".to_string()];
        let result = offload(&entries, dir.path(), OFFLOAD_TOKEN_THRESHOLD, &[false]).unwrap();
        assert_eq!(result.entries, entries);
        assert_eq!(result.tokens_freed, 0);
        assert!(result.offloaded_files.is_empty());
    }

    #[test]
    fn test_offload_large_entry() {
        let dir = TempDir::new().unwrap();
        // Create multi-line text that tokenises well above threshold.
        let large: String = (0..500)
            .map(|i| format!("Line {i}: The quick brown fox jumps over the lazy dog."))
            .collect::<Vec<_>>()
            .join("\n");
        let entries = vec![large.clone()];
        // Use a low threshold to guarantee offload triggers
        let result = offload(&entries, dir.path(), 100, &[false]).unwrap();

        assert_eq!(result.entries.len(), 1);
        assert!(result.entries[0].starts_with("[Offloaded to "));
        assert_eq!(result.offloaded_files.len(), 1);
        // Preview is only 10 lines, so freed tokens should be substantial
        assert!(result.tokens_freed > 0);

        // Verify file was written
        let written = std::fs::read_to_string(&result.offloaded_files[0]).unwrap();
        assert_eq!(written, large);
    }

    #[test]
    fn test_truncate_offloaded_entries() {
        let entries = vec![
            "[Offloaded to /tmp/0.md] (5000 tokens)\n\nline1\nline2\nline3".to_string(),
            "normal entry".to_string(),
        ];
        let result = truncate(&entries, &[false, false]);
        assert_eq!(result.entries[0], "[Offloaded to /tmp/0.md] (5000 tokens)");
        assert_eq!(result.entries[1], "normal entry");
        assert!(result.tokens_freed > 0);
    }

    #[test]
    fn test_truncate_no_offloaded_entries() {
        let entries = vec!["normal".to_string()];
        let result = truncate(&entries, &[false]);
        assert_eq!(result.entries, entries);
        assert_eq!(result.tokens_freed, 0);
    }

    #[test]
    fn test_make_preview() {
        let text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let preview = make_preview(&text, 3);
        assert!(preview.contains("line 0"));
        assert!(preview.contains("line 2"));
        assert!(preview.contains("... (truncated)"));
        assert!(!preview.contains("line 3"));
    }

    #[test]
    fn test_build_summarisation_prompt() {
        let prompt = build_summarisation_prompt("some history", "build API");
        assert!(prompt.contains("build API"));
        assert!(prompt.contains("some history"));
        assert!(prompt.contains("Completed steps"));
    }

    #[test]
    fn test_offload_protected_entry_not_offloaded() {
        let dir = TempDir::new().unwrap();
        let large: String = (0..500)
            .map(|i| format!("Line {i}: Error details here."))
            .collect::<Vec<_>>()
            .join("\n");
        let entries = vec![large.clone()];
        // Protected entry should not be offloaded even if above threshold
        let result = offload(&entries, dir.path(), 100, &[true]).unwrap();
        assert_eq!(result.entries[0], large);
        assert_eq!(result.tokens_freed, 0);
        assert!(result.offloaded_files.is_empty());
    }

    #[test]
    fn test_truncate_protected_entry_not_truncated() {
        let entries = vec!["[Offloaded to /tmp/0.md] (5000 tokens)\n\npreview".to_string()];
        // Protected — should not be truncated
        let result = truncate(&entries, &[true]);
        assert!(result.entries[0].contains("preview"));
        assert_eq!(result.tokens_freed, 0);
    }
}
