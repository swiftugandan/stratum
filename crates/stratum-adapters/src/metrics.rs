use std::collections::HashMap;
use std::sync::Mutex;

use stratum_core::MetricsExporter;

struct MetricsState {
    gauges: HashMap<String, HashMap<String, f64>>,
    counters: HashMap<String, HashMap<String, u64>>,
}

pub struct InMemoryMetrics {
    state: Mutex<MetricsState>,
}

impl InMemoryMetrics {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MetricsState {
                gauges: HashMap::new(),
                counters: HashMap::new(),
            }),
        }
    }
}

impl Default for InMemoryMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for InMemoryMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryMetrics").finish()
    }
}

fn labels_key(labels: &[(&str, &str)]) -> String {
    let mut sorted: Vec<_> = labels.to_vec();
    sorted.sort_by_key(|(k, _)| *k);
    sorted
        .iter()
        .map(|(k, v)| format!("{k}=\"{v}\""))
        .collect::<Vec<_>>()
        .join(",")
}

fn render_metric_type<V: std::fmt::Display>(
    out: &mut String,
    metrics: &HashMap<String, HashMap<String, V>>,
    type_name: &str,
) {
    let mut names: Vec<_> = metrics.keys().collect();
    names.sort();
    for name in names {
        out.push_str(&format!("# TYPE {name} {type_name}\n"));
        let entries = &metrics[name];
        let mut keys: Vec<_> = entries.keys().collect();
        keys.sort();
        for key in keys {
            let value = &entries[key];
            if key.is_empty() {
                out.push_str(&format!("{name} {value}\n"));
            } else {
                out.push_str(&format!("{name}{{{key}}} {value}\n"));
            }
        }
    }
}

impl MetricsExporter for InMemoryMetrics {
    fn gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        let key = labels_key(labels);
        let mut state = self.state.lock().unwrap();
        state
            .gauges
            .entry(name.to_string())
            .or_default()
            .insert(key, value);
    }

    fn counter(&self, name: &str, labels: &[(&str, &str)]) {
        let key = labels_key(labels);
        let mut state = self.state.lock().unwrap();
        *state
            .counters
            .entry(name.to_string())
            .or_default()
            .entry(key)
            .or_insert(0) += 1;
    }

    fn export_metrics(&self) -> String {
        let state = self.state.lock().unwrap();
        let mut out = String::new();

        render_metric_type(&mut out, &state.gauges, "gauge");
        render_metric_type(&mut out, &state.counters, "counter");

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_increments() {
        let metrics = InMemoryMetrics::new();
        metrics.counter("requests_total", &[("method", "GET")]);
        metrics.counter("requests_total", &[("method", "GET")]);
        metrics.counter("requests_total", &[("method", "GET")]);

        let output = metrics.export_metrics();
        assert!(output.contains("requests_total{method=\"GET\"} 3"));
    }

    #[test]
    fn test_gauge_overwrites() {
        let metrics = InMemoryMetrics::new();
        metrics.gauge("cpu_usage", 0.5, &[("host", "a")]);
        metrics.gauge("cpu_usage", 0.8, &[("host", "a")]);

        let output = metrics.export_metrics();
        assert!(output.contains("cpu_usage{host=\"a\"} 0.8"));
        assert!(!output.contains("0.5"));
    }

    #[test]
    fn test_prometheus_format() {
        let metrics = InMemoryMetrics::new();
        metrics.gauge("temp", 36.6, &[("location", "cpu")]);
        metrics.counter("errors", &[("code", "500")]);

        let output = metrics.export_metrics();
        assert!(output.contains("# TYPE temp gauge"));
        assert!(output.contains("# TYPE errors counter"));
        assert!(output.contains("temp{location=\"cpu\"} 36.6"));
        assert!(output.contains("errors{code=\"500\"} 1"));
    }
}
