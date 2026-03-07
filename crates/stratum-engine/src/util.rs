//! Shared utilities: serialization, I/O helpers.

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::AsyncReadExt;

use crate::error::EngineError;

pub fn serialize_enum<T: Serialize>(val: &T) -> Result<String, EngineError> {
    let v = serde_json::to_value(val)?;
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| EngineError::InvalidState("enum did not serialize to string".into()))
}

pub fn deserialize_enum<T: DeserializeOwned>(s: &str) -> Result<T, EngineError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| EngineError::InvalidState(format!("unknown enum value '{s}': {e}")))
}

pub fn parse_utc(s: &str) -> Result<chrono::DateTime<chrono::Utc>, EngineError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| EngineError::InvalidState(format!("invalid timestamp: {e}")))
}

/// Read from an async reader up to `max_bytes`, returning the content as a lossy UTF-8 string.
pub async fn read_limited(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    max_bytes: usize,
) -> std::io::Result<String> {
    let mut buf = Vec::with_capacity(std::cmp::min(8192, max_bytes));
    let mut chunk = [0u8; 8192];
    loop {
        if buf.len() >= max_bytes {
            break;
        }
        let to_read = std::cmp::min(chunk.len(), max_bytes - buf.len());
        let n = reader.read(&mut chunk[..to_read]).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Compute exponential backoff with jitter for retry attempt.
pub fn retry_delay(base: std::time::Duration, max: std::time::Duration, attempt: u32) -> std::time::Duration {
    use rand::Rng;
    let base_ms = base.as_millis() as u64;
    let exp_ms = base_ms.saturating_mul(1u64 << attempt.min(10));
    let capped_ms = exp_ms.min(max.as_millis() as u64);
    let jitter_ms = rand::thread_rng().gen_range(0..=capped_ms);
    std::time::Duration::from_millis(jitter_ms)
}
