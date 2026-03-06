//! FTS5 query building, temporal decay, and score normalization.

/// Build an FTS5 search query from user input.
///
/// Sanitizes special characters and quotes individual terms for safe FTS5 matching.
pub fn build_fts5_query(query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| {
            // Strip FTS5 special chars: *, ^, ", (, ), :, +, -, NEAR
            let sanitized: String = term
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if sanitized.is_empty() {
                return String::new();
            }
            format!("\"{sanitized}\"")
        })
        .filter(|t| !t.is_empty())
        .collect();

    if terms.is_empty() {
        // Fallback: return original query quoted to avoid empty match
        let fallback: String = query
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '_')
            .collect();
        return format!("\"{fallback}\"");
    }

    terms.join(" OR ")
}

/// Apply temporal decay to a base relevance score.
///
/// Uses exponential decay: `score * 0.5^(age / half_life)`.
pub fn apply_temporal_decay(base_score: f32, entry_age_hours: f64, half_life: f64) -> f32 {
    if half_life <= 0.0 {
        return base_score;
    }
    let decay = 0.5_f64.powf(entry_age_hours / half_life);
    base_score * decay as f32
}

/// Normalize FTS5 bm25() scores to approximately 0.0–1.0.
///
/// FTS5 bm25() returns negative values (more negative = more relevant).
/// We negate and clamp to produce a usable relevance score.
pub fn normalize_bm25_score(raw: f64) -> f32 {
    let positive = -raw;
    positive.max(0.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_fts5_query_simple() {
        let q = build_fts5_query("hello world");
        assert!(q.contains("\"hello\""));
        assert!(q.contains("\"world\""));
        assert!(q.contains(" OR "));
    }

    #[test]
    fn test_build_fts5_query_special_chars() {
        let q = build_fts5_query("hello* (world)");
        assert!(q.contains("\"hello\""));
        assert!(q.contains("\"world\""));
        // Special chars should be stripped
        assert!(!q.contains('*'));
        assert!(!q.contains('('));
    }

    #[test]
    fn test_build_fts5_query_empty() {
        let q = build_fts5_query("");
        assert!(!q.is_empty());
    }

    #[test]
    fn test_temporal_decay_zero_age() {
        let score = apply_temporal_decay(1.0, 0.0, 168.0);
        assert!((score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_temporal_decay_one_half_life() {
        let score = apply_temporal_decay(1.0, 168.0, 168.0);
        assert!((score - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_temporal_decay_two_half_lives() {
        let score = apply_temporal_decay(1.0, 336.0, 168.0);
        assert!((score - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_normalize_bm25_negative() {
        let score = normalize_bm25_score(-2.5);
        assert!((score - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_normalize_bm25_zero() {
        let score = normalize_bm25_score(0.0);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_normalize_bm25_positive() {
        // Positive raw values should stay positive
        let score = normalize_bm25_score(1.0);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }
}
