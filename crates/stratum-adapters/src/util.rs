use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::AdapterError;

/// Serialize a serde enum variant to its string representation.
pub fn serialize_enum<T: Serialize>(val: &T) -> Result<String, AdapterError> {
    let v = serde_json::to_value(val)?;
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| AdapterError::InvalidState("enum did not serialize to string".into()))
}

/// Deserialize a string into a serde enum variant.
pub fn deserialize_enum<T: DeserializeOwned>(s: &str) -> Result<T, AdapterError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| AdapterError::InvalidState(format!("unknown enum value '{s}': {e}")))
}

/// Parse an RFC 3339 timestamp string to DateTime<Utc>.
pub fn parse_utc(s: &str) -> Result<chrono::DateTime<chrono::Utc>, AdapterError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| AdapterError::InvalidState(format!("invalid timestamp: {e}")))
}
