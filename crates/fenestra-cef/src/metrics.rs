use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const FENESTRA_TRACE_ENV: &str = "FENESTRA_TRACE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FenestraLaunchMetric {
    pub stage: String,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FenestraLaunchMetricsSnapshot {
    pub label: String,
    pub elapsed: Duration,
    pub stages: Vec<FenestraLaunchMetric>,
}

#[derive(Clone, Debug)]
pub(crate) struct LaunchMetrics {
    started: Instant,
    label: String,
    trace: bool,
    stages: Arc<Mutex<Vec<FenestraLaunchMetric>>>,
}

impl LaunchMetrics {
    pub(crate) fn new(label: impl Into<String>) -> Self {
        Self {
            started: Instant::now(),
            label: label.into(),
            trace: trace_enabled(),
            stages: Arc::default(),
        }
    }

    pub(crate) fn mark(&self, stage: impl Into<String>) {
        let stage = stage.into();
        let elapsed = self.started.elapsed();
        if self.trace {
            eprintln!(
                "fenestra trace [{}] +{}ms {stage}",
                self.label,
                elapsed.as_millis()
            );
        }
        if let Ok(mut stages) = self.stages.lock() {
            stages.push(FenestraLaunchMetric { stage, elapsed });
        }
    }

    pub(crate) fn snapshot(&self) -> FenestraLaunchMetricsSnapshot {
        FenestraLaunchMetricsSnapshot {
            label: self.label.clone(),
            elapsed: self.started.elapsed(),
            stages: self
                .stages
                .lock()
                .map(|stages| stages.clone())
                .unwrap_or_default(),
        }
    }
}

fn trace_enabled() -> bool {
    std::env::var(FENESTRA_TRACE_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "trace"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_snapshot_keeps_stage_order() {
        let metrics = LaunchMetrics::new("test");
        metrics.mark("start");
        metrics.mark("ready");
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.label, "test");
        assert_eq!(snapshot.stages[0].stage, "start");
        assert_eq!(snapshot.stages[1].stage, "ready");
    }
}
