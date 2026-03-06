//! Token counting using tiktoken-rs.
//!
//! Uses `cl100k_base` encoding (GPT-4/ChatGPT family) as the default.
//! This provides accurate token counts for Anthropic and OpenAI models
//! which use similar BPE tokenisation.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

static BPE: OnceLock<CoreBPE> = OnceLock::new();

pub(crate) fn bpe() -> &'static CoreBPE {
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().expect("failed to load cl100k_base tokenizer"))
}

/// Count tokens in a string using cl100k_base encoding.
pub fn count_tokens(text: &str) -> u64 {
    bpe().encode_with_special_tokens(text).len() as u64
}

/// Count tokens across a slice of strings.
pub fn count_tokens_many(texts: &[String]) -> u64 {
    texts.iter().map(|t| count_tokens(t)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_string() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn test_hello_world() {
        let count = count_tokens("Hello, world!");
        // tiktoken should give a definite answer; just verify it's reasonable
        assert!(count > 0 && count < 10);
    }

    #[test]
    fn test_many() {
        let texts = vec!["Hello".to_string(), "world".to_string()];
        let total = count_tokens_many(&texts);
        assert!(total >= 2);
    }

    #[test]
    fn test_longer_text() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(100);
        let count = count_tokens(&text);
        // ~1000 tokens for ~4500 chars of English
        assert!(count > 500);
        assert!(count < 2000);
    }
}
